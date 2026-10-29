use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Result;
use tokio::{
    sync::{
        mpsc::{Receiver, Sender},
        Mutex,
    },
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::control::{self as control_state, ControlHandle};
use crate::types::event::ApprovalScope;
use crate::{
    agent::Agent,
    speech::{SpeechToText, TextToSpeech},
    tools::{media, registry::ToolRegistry},
    types::{HyuskEvent, HyuskState},
    wake::detector::WakeResume,
};

struct ActiveTurn {
    id: u64,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

/// The voice-listen flow (record + transcribe) runs as its own task so the
/// event loop stays responsive while the microphone is being read.
struct ActiveListen {
    id: u64,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
    probe_id: Option<u64>,
}

struct WorkflowJob {
    id: u64,
    name: String,
    actions: Vec<crate::agent::commands::Action>,
    desktop: bool,
}

struct ActiveWorkflow {
    id: u64,
    name: String,
    desktop: bool,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

const MAX_QUEUED_WORKFLOWS: usize = 16;

/// Release the paused wake detector when the listening flow ends, no matter
/// how it ends (success, error, timeout, or task cancellation).
///
/// The detector pauses inside its microphone thread after every detection;
/// if the listening task died without resuming it, the whole voice path
/// would stay deaf until the detector's own bounded pause timeout.
struct ResumeOnDrop(WakeResume);

impl Drop for ResumeOnDrop {
    fn drop(&mut self) {
        self.0.resume();
    }
}

/// Clear the "output active" flag when a turn ends, however it ends.
///
/// While the flag is set the wake detector discards microphone audio so the
/// agent's own text-to-speech cannot re-trigger it (there is no echo
/// cancellation).
struct OutputOnDrop(Arc<AtomicBool>);

impl Drop for OutputOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// Resume media that was paused for a voice command, when the turn ends.
///
/// The turn clears the flag first when the command itself controlled playback
/// (so "pause music" is not immediately undone). Dropping this guard spawns the
/// resume, so it also fires on cancellation and error paths.
struct MediaOnDrop(Arc<AtomicBool>);

impl Drop for MediaOnDrop {
    fn drop(&mut self) {
        if self.0.swap(false, Ordering::Relaxed) {
            tokio::spawn(media::resume());
        }
    }
}

async fn interrupt_active(active: &mut Option<ActiveTurn>) {
    if let Some(mut turn) = active.take() {
        turn.cancel.cancel();
        if tokio::time::timeout(Duration::from_millis(350), &mut turn.handle)
            .await
            .is_err()
        {
            eprintln!("[Agent] Active turn did not stop promptly; aborting worker");
            turn.handle.abort();
            let _ = turn.handle.await;
        }
    }
}

async fn interrupt_listen(active: &mut Option<ActiveListen>) {
    if let Some(mut listen) = active.take() {
        listen.cancel.cancel();
        if tokio::time::timeout(Duration::from_millis(350), &mut listen.handle)
            .await
            .is_err()
        {
            eprintln!("[Wake] Listening task did not stop promptly; aborting capture");
            listen.handle.abort();
            let _ = listen.handle.await;
        }
    }
}

async fn timeout_with_cancel<F: Future>(
    duration: Duration,
    capture_cancel: &CancellationToken,
    future: F,
) -> Option<F::Output> {
    match tokio::time::timeout(duration, future).await {
        Ok(output) => Some(output),
        Err(_) => {
            // Dropping a future that owns a spawn_blocking capture does not
            // stop that worker. Signal it before microphone ownership is
            // released back to the wake detector.
            capture_cancel.cancel();
            None
        }
    }
}

/// Prefer already-arrived external events over internal follow-ups so fresh
/// input and stop requests retain priority. Follow-ups stay local to the
/// receiver task, avoiding a blocking send into its own bounded channel.
async fn next_runtime_event(
    event_rx: &mut Receiver<HyuskEvent>,
    pending_followups: &mut VecDeque<HyuskEvent>,
) -> Option<HyuskEvent> {
    match event_rx.try_recv() {
        Ok(event) => Some(event),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
            if let Some(event) = pending_followups.pop_front() {
                Some(event)
            } else {
                event_rx.recv().await
            }
        }
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => pending_followups.pop_front(),
    }
}

pub async fn run_agent_task(
    agent: Agent,
    mut event_rx: Receiver<HyuskEvent>,
    agent_tx: Sender<HyuskEvent>,
    ui_tx: Sender<HyuskEvent>,
    wake_resume: WakeResume,
    stt: Option<Arc<SpeechToText>>,
    output_active: Arc<AtomicBool>,
) -> Result<()> {
    // Control state is taken before the agent is wrapped in its mutex, so the
    // event loop can reach it while a turn holds that mutex (L1).
    let control = agent.control_handle();
    let mut sonic_mode = agent.uses_bedrock_sonic();
    // Clone the Arc-backed tools before any model turn can hold Agent's mutex.
    let workflow_tools = agent.workflow_tools();
    let agent = Arc::new(Mutex::new(agent));
    let tts = TextToSpeech::new();
    let mut active: Option<ActiveTurn> = None;
    let mut listen: Option<ActiveListen> = None;
    let mut completed_tasks = VecDeque::new();
    let mut scheduled_inputs = VecDeque::new();
    let mut pending_followups = VecDeque::new();
    let mut workflow_queue = VecDeque::<WorkflowJob>::new();
    let mut active_workflow: Option<ActiveWorkflow> = None;
    let mut next_turn_id = 1u64;
    let mut last_finished_turn = None;
    let mut deferred_reply_listen: Option<u64> = None;
    let mut next_listen_id = 1u64;
    let mut next_workflow_id = 1u64;
    let mut voice_conversation = false;
    let mut voice_listen_until: Option<std::time::Instant> = None;

    /*
     * Set when media was playing and was paused for a voice command. The turn
     * resumes it afterwards unless the command itself controlled playback.
     */
    let resume_media = Arc::new(AtomicBool::new(false));

    loop {
        /*
         * Wait for input, but never longer than an armed confirmation has left.
         * Without this, an abandoned approval would stay armed indefinitely:
         * it would hold queued scheduled input and could be answered by an
         * unrelated later "yes".
         */
        let event = tokio::select! {
            event = next_runtime_event(&mut event_rx, &mut pending_followups) => event,
            _ = approval_expiry(&control) => Some(HyuskEvent::ApprovalExpired),
        };
        let Some(event) = event else {
            break;
        };

        // Normalize capture completions before dispatching them. In particular,
        // do not re-enqueue a transcript as untagged UserInput: an already
        // queued newer input could cancel this capture while the old result
        // waits in the channel.
        let event = match event {
            // A panel decision is only meaningful while a confirmation is
            // armed. A stale click reports expiry and starts no turn.
            HyuskEvent::ApprovalDecision { approved, scope } => {
                match apply_approval_decision(&control, approved, scope) {
                    Ok(input) => HyuskEvent::UserInput(input),
                    Err(reply) => {
                        println!("[Agent] Ignored a stale approval decision");
                        crate::status::card("response", reply, false);
                        let _ = ui_tx.try_send(HyuskEvent::Response(reply.to_string()));
                        continue;
                    }
                }
            }
            HyuskEvent::VoiceTranscript { id, text } => {
                let Some(event) = normalize_voice_transcript(id, text, &listen) else {
                    continue;
                };
                event
            }
            HyuskEvent::ListenCompleted {
                id,
                continue_listening,
            } => {
                let Some(event) = normalize_listen_completed(id, continue_listening, &listen)
                else {
                    continue;
                };
                crate::status::awaiting_reply(false);
                if continue_listening {
                    // The capture has ended, so its slot must not suppress the
                    // continuation wake while its task finishes returning.
                    listen = None;
                }
                event
            }
            HyuskEvent::WakeProbeFinished {
                id,
                text: Some(text),
            } => {
                if !listen.as_ref().is_some_and(|capture| {
                    capture.probe_id == Some(id) && !capture.cancel.is_cancelled()
                }) {
                    continue;
                }
                listen = None;
                voice_conversation = true;
                voice_listen_until = Some(std::time::Instant::now() + Duration::from_secs(30));
                if crate::agent::runner::is_voice_session_stop(&text) {
                    HyuskEvent::StopRequested
                } else {
                    HyuskEvent::UserInput(text)
                }
            }
            event @ HyuskEvent::SonicListenCompleted { id, .. } => {
                if !is_current_sonic_listen(id, &listen) {
                    continue;
                }
                event
            }
            event => event,
        };
        match event {
            event @ (HyuskEvent::WakeWordDetected
            | HyuskEvent::ContinueListening
            | HyuskEvent::AwaitReplyListening { .. }) => {
                if matches!(&event, HyuskEvent::ContinueListening) && !voice_conversation {
                    wake_resume.resume();
                    continue;
                }
                if let HyuskEvent::AwaitReplyListening { id } = &event {
                    let active_turn = active.as_ref().is_some_and(|turn| turn.id == *id);
                    if active_turn
                        && active
                            .as_ref()
                            .is_some_and(|turn| !turn.handle.is_finished())
                    {
                        // The turn worker publishes this before TurnFinished.
                        // Keep it as state rather than repeatedly requeueing
                        // it while the worker is still winding down.
                        deferred_reply_listen = Some(*id);
                        wake_resume.resume();
                        continue;
                    }
                    if !active_turn && last_finished_turn != Some(*id) {
                        wake_resume.resume();
                        continue;
                    }
                }
                if listen_capture_active(&listen) {
                    // Nova already has a live mic; duplicate wake/follow-up
                    // events must not tear down it or another capture.
                    continue;
                }
                let task_running = active
                    .as_ref()
                    .is_some_and(|turn| !turn.handle.is_finished());
                if task_running {
                    if !should_probe_busy_wake(&event, sonic_mode) {
                        wake_resume.resume();
                        continue;
                    }
                    let Some(probe_stt) = stt.clone() else {
                        wake_resume.resume();
                        continue;
                    };
                    let id = next_listen_id;
                    next_listen_id += 1;
                    let cancel = CancellationToken::new();
                    let probe_cancel = cancel.clone();
                    let capture_cancel = probe_cancel.child_token();
                    let probe_tx = agent_tx.clone();
                    let probe_resume = wake_resume.clone();
                    let handle = tokio::spawn(async move {
                        let _resume = ResumeOnDrop(probe_resume);
                        let text = match timeout_with_cancel(
                            Duration::from_secs(45),
                            &capture_cancel,
                            probe_stt.transcribe_from_microphone(8.0, &capture_cancel),
                        )
                        .await
                        {
                            Some(Ok(text)) if !text.trim().is_empty() => Some(text),
                            Some(Err(error)) => {
                                eprintln!(
                                    "[Wake] Speech probe failed; current task continues: {error:#}"
                                );
                                None
                            }
                            _ => None,
                        };
                        if !probe_cancel.is_cancelled() {
                            let _ = probe_tx
                                .send(HyuskEvent::WakeProbeFinished { id, text })
                                .await;
                        }
                    });
                    listen = Some(ActiveListen {
                        id,
                        cancel,
                        handle,
                        probe_id: Some(id),
                    });
                    println!("[Wake] Checking for a spoken instruction; current task continues");
                    send_state(&ui_tx, HyuskState::Listening).await;
                    continue;
                }
                if matches!(event, HyuskEvent::WakeWordDetected) {
                    voice_conversation = true;
                    voice_listen_until = Some(std::time::Instant::now() + Duration::from_secs(30));
                }
                if voice_conversation
                    && voice_listen_until
                        .is_some_and(|deadline| std::time::Instant::now() >= deadline)
                {
                    finish_voice_conversation(&mut voice_conversation, &mut voice_listen_until);
                    release_next_scheduled_input(
                        &mut scheduled_inputs,
                        &mut pending_followups,
                        active.is_some(),
                        listen_capture_active(&listen),
                        voice_conversation,
                        approval_blocks_scheduled_input(&control),
                    );
                    send_state(&ui_tx, HyuskState::Hidden).await;
                    wake_resume.resume();
                    start_next_workflow(
                        &mut workflow_queue,
                        &mut active_workflow,
                        active.is_some(),
                        listen_capture_active(&listen),
                        &workflow_tools,
                        &agent_tx,
                        &ui_tx,
                    );
                    continue;
                }
                println!("[Wake] Listening requested");

                /*
                 * A new voice request replaces any in-flight model, tool,
                 * TTS work, and any listening session that was already in
                 * progress (for example a double wake or a clap during the
                 * recording window).
                 */
                interrupt_interaction(
                    &mut active,
                    &mut listen,
                    &mut active_workflow,
                    &ui_tx,
                    false,
                )
                .await;

                // Quiet any music so the command is heard, and remember to
                // resume it once the request is handled.
                if media::pause_if_playing().await {
                    resume_media.store(true, Ordering::Relaxed);
                }

                send_state(&ui_tx, HyuskState::Waking).await;
                send_state(&ui_tx, HyuskState::Listening).await;
                crate::status::awaiting_reply(false);

                let Some(stt) = stt.clone() else {
                    eprintln!("[STT] Speech-to-text is not configured; ignoring wake word");
                    finish_voice_conversation(&mut voice_conversation, &mut voice_listen_until);
                    release_next_scheduled_input(
                        &mut scheduled_inputs,
                        &mut pending_followups,
                        active.is_some(),
                        listen_capture_active(&listen),
                        voice_conversation,
                        approval_blocks_scheduled_input(&control),
                    );
                    if resume_media.swap(false, Ordering::Relaxed) {
                        tokio::spawn(media::resume());
                    }
                    send_state(&ui_tx, HyuskState::Hidden).await;
                    wake_resume.resume();
                    start_next_workflow(
                        &mut workflow_queue,
                        &mut active_workflow,
                        active.is_some(),
                        listen_capture_active(&listen),
                        &workflow_tools,
                        &agent_tx,
                        &ui_tx,
                    );
                    continue;
                };

                let id = next_listen_id;
                next_listen_id += 1;
                let cancel = CancellationToken::new();
                let task_agent_tx = agent_tx.clone();
                let task_ui_tx = ui_tx.clone();
                let task_resume = wake_resume.clone();
                let task_agent = Arc::clone(&agent);
                let task_cancel = cancel.clone();
                let task_output = Arc::clone(&output_active);
                let task_control = Arc::clone(&control);
                let task_listen_until = voice_listen_until;

                let handle = tokio::spawn(async move {
                    handle_listen(
                        stt,
                        task_agent_tx,
                        task_ui_tx,
                        task_resume,
                        task_cancel,
                        task_agent,
                        task_control,
                        task_output,
                        task_listen_until,
                        id,
                    )
                    .await;
                });

                listen = Some(ActiveListen {
                    id,
                    cancel,
                    handle,
                    probe_id: None,
                });
            }

            HyuskEvent::WakeProbeFinished { id, text: None } => {
                if !listen.as_ref().is_some_and(|capture| {
                    capture.probe_id == Some(id) && !capture.cancel.is_cancelled()
                }) {
                    continue;
                }
                listen = None;
                if active
                    .as_ref()
                    .is_some_and(|turn| !turn.handle.is_finished())
                {
                    println!("[Wake] No new instruction; task was not cancelled");
                    send_state(&ui_tx, HyuskState::Working).await;
                } else if voice_conversation {
                    pending_followups.push_back(HyuskEvent::ContinueListening);
                } else {
                    send_state(&ui_tx, HyuskState::Hidden).await;
                }
            }

            HyuskEvent::SonicListenCompleted {
                id: _,
                text,
                waiting,
                error,
            } => {
                listen = None;
                let keep_listening = waiting && text.is_some() && error.is_none();
                if let Some(error) = error {
                    eprintln!("[Nova Sonic] Turn failed: {error}");
                    let message = format!(
                        "Nova Sonic failed: {}",
                        error.lines().next().unwrap_or("unknown error")
                    );
                    crate::status::card("error", &message, false);
                    let _ = ui_tx.try_send(HyuskEvent::Response(message));
                } else if let Some(text) = text {
                    crate::status::awaiting_reply(waiting);
                    crate::status::card(
                        if waiting {
                            crate::status::APPROVAL_CARD_KIND
                        } else {
                            "response"
                        },
                        &text,
                        waiting,
                    );
                    let _ = ui_tx.try_send(HyuskEvent::Response(text));
                }

                // This completion also performs the StopListening cleanup.
                // Keeping it in this ID-validated dispatch prevents an old
                // Sonic worker from hiding a newer capture or resuming media.
                if keep_listening {
                    voice_conversation = true;
                    voice_listen_until = Some(std::time::Instant::now() + Duration::from_secs(30));
                    pending_followups.push_back(HyuskEvent::ContinueListening);
                    crate::status::awaiting_reply(true);
                } else {
                    finish_voice_conversation(&mut voice_conversation, &mut voice_listen_until);
                }
                release_next_scheduled_input(
                    &mut scheduled_inputs,
                    &mut pending_followups,
                    active.is_some(),
                    listen_capture_active(&listen),
                    voice_conversation,
                    approval_blocks_scheduled_input(&control),
                );
                if resume_media.swap(false, Ordering::Relaxed) {
                    tokio::spawn(media::resume());
                }
                send_state(
                    &ui_tx,
                    if keep_listening {
                        HyuskState::Listening
                    } else {
                        HyuskState::Hidden
                    },
                )
                .await;
                start_next_workflow(
                    &mut workflow_queue,
                    &mut active_workflow,
                    active.is_some(),
                    listen_capture_active(&listen),
                    &workflow_tools,
                    &agent_tx,
                    &ui_tx,
                );
            }

            event @ (HyuskEvent::UserInput(_) | HyuskEvent::ScheduledAgentInput(_)) => {
                let scheduled = matches!(&event, HyuskEvent::ScheduledAgentInput(_));
                let text = match event {
                    HyuskEvent::UserInput(text) | HyuskEvent::ScheduledAgentInput(text) => text,
                    _ => unreachable!("matched an input event"),
                };
                if scheduled
                    && (should_defer_scheduled_input(
                        active.is_some(),
                        listen_capture_active(&listen),
                        voice_conversation,
                    ) || approval_blocks_scheduled_input(&control))
                {
                    // Never hand a scheduled prompt to a turn that is waiting
                    // for a yes/no confirmation: `handle` would treat it as
                    // the answer and drop the prompt.
                    scheduled_inputs.push_back(text);
                    continue;
                }
                if text.trim().is_empty() {
                    continue;
                }

                if let Some(name) = crate::agent::commands::exact_workflow_trigger(&text) {
                    let foreground_active = active.is_some();
                    interrupt_listen(&mut listen).await;
                    finish_voice_conversation(&mut voice_conversation, &mut voice_listen_until);
                    release_next_scheduled_input(
                        &mut scheduled_inputs,
                        &mut pending_followups,
                        foreground_active,
                        false,
                        voice_conversation,
                        approval_blocks_scheduled_input(&control),
                    );
                    if !foreground_active {
                        if resume_media.swap(false, Ordering::Relaxed) {
                            tokio::spawn(media::resume());
                        }
                        send_state(&ui_tx, HyuskState::Hidden).await;
                    }
                    pending_followups.push_back(HyuskEvent::RunWorkflow { name });
                    continue;
                }

                /*
                 * Interrupt the running turn before starting the new one so
                 * the agent is always responsive to the latest message.
                 */
                interrupt_interaction(
                    &mut active,
                    &mut listen,
                    &mut active_workflow,
                    &ui_tx,
                    false,
                )
                .await;
                crate::status::awaiting_reply(false);

                let cancel = CancellationToken::new();

                /*
                 * Mute the wake detector for the whole turn. Set it here so it
                 * covers the gap between the recording ending and the turn
                 * task starting; `OutputOnDrop` clears it when the turn ends.
                 */
                output_active.store(true, Ordering::Relaxed);

                let turn_agent = Arc::clone(&agent);
                let turn_ui = ui_tx.clone();
                let turn_tts = tts.clone();
                let turn_cancel = cancel.clone();
                let turn_output = Arc::clone(&output_active);
                let turn_resume = Arc::clone(&resume_media);
                let turn_event_tx = agent_tx.clone();
                let turn_voice_conversation = voice_conversation;
                let id = next_turn_id;
                next_turn_id += 1;
                last_finished_turn = None;
                deferred_reply_listen = None;

                let handle = tokio::spawn(async move {
                    run_turn(
                        turn_agent,
                        text,
                        turn_ui,
                        turn_tts,
                        turn_cancel,
                        turn_output,
                        turn_resume,
                        turn_event_tx.clone(),
                        turn_voice_conversation,
                        id,
                    )
                    .await;
                    let _ = turn_event_tx.send(HyuskEvent::TurnFinished { id }).await;
                });

                active = Some(ActiveTurn { id, cancel, handle });
                start_next_workflow(
                    &mut workflow_queue,
                    &mut active_workflow,
                    active.is_some(),
                    listen_capture_active(&listen) || voice_conversation,
                    &workflow_tools,
                    &agent_tx,
                    &ui_tx,
                );
            }

            HyuskEvent::RunWorkflow { name } => {
                match crate::agent::commands::parse_named_workflow(&name) {
                    Ok(plan) => {
                        if workflow_queue.len() + usize::from(active_workflow.is_some())
                            >= MAX_QUEUED_WORKFLOWS
                        {
                            let message = format!(
                                "Could not queue workflow {name}: the workflow queue is full."
                            );
                            crate::status::card("workflow_error", &message, true);
                            let _ = ui_tx.try_send(HyuskEvent::Response(message));
                            continue;
                        }
                        let desktop = plan.actions.iter().any(|action| {
                            matches!(
                                action.tool_name(),
                                "process" | "computer" | "window" | "shell"
                            )
                        });
                        let id = next_workflow_id;
                        next_workflow_id += 1;
                        workflow_queue.push_back(WorkflowJob {
                            id,
                            name: name.clone(),
                            actions: plan.actions,
                            desktop,
                        });
                        let message = if desktop
                            && (active.is_some()
                                || listen_capture_active(&listen)
                                || voice_conversation)
                        {
                            format!("Workflow {name} queued until the current interaction ends.")
                        } else {
                            format!("Workflow {name} queued to run.")
                        };
                        crate::status::card("workflow", &message, false);
                        start_next_workflow(
                            &mut workflow_queue,
                            &mut active_workflow,
                            active.is_some(),
                            listen_capture_active(&listen) || voice_conversation,
                            &workflow_tools,
                            &agent_tx,
                            &ui_tx,
                        );
                    }
                    Err(message) => {
                        crate::status::card("workflow_error", &message, true);
                        let _ = ui_tx.try_send(HyuskEvent::Response(message));
                    }
                }
            }

            HyuskEvent::TurnFinished { id } => {
                if active.as_ref().map(|turn| turn.id) == Some(id) {
                    active = None;
                    last_finished_turn = Some(id);
                    if deferred_reply_listen == Some(id) {
                        deferred_reply_listen = None;
                        pending_followups.push_back(HyuskEvent::AwaitReplyListening { id });
                    }
                    announce_completed_tasks(&mut completed_tasks, &ui_tx, &tts).await;
                    let scheduled_released = release_next_scheduled_input(
                        &mut scheduled_inputs,
                        &mut pending_followups,
                        false,
                        listen_capture_active(&listen),
                        voice_conversation,
                        approval_blocks_scheduled_input(&control),
                    );
                    if !scheduled_released && voice_conversation {
                        voice_listen_until =
                            Some(std::time::Instant::now() + Duration::from_secs(30));
                        pending_followups.push_back(HyuskEvent::ContinueListening);
                    }
                }
                start_next_workflow(
                    &mut workflow_queue,
                    &mut active_workflow,
                    active.is_some(),
                    listen_capture_active(&listen) || voice_conversation,
                    &workflow_tools,
                    &agent_tx,
                    &ui_tx,
                );
            }

            HyuskEvent::ScheduledWorkflow { id, name } => {
                println!("[Scheduler #{id}] Workflow due: {name}");
                crate::status::card(
                    "scheduled_workflow",
                    format!("Scheduled workflow ready: {name}"),
                    false,
                );
                pending_followups.push_back(HyuskEvent::RunWorkflow { name });
            }

            HyuskEvent::WorkflowFinished {
                id,
                name,
                success,
                summary,
            } => {
                if active_workflow.as_ref().map(|workflow| workflow.id) != Some(id) {
                    continue;
                }
                active_workflow = None;
                let message = format!(
                    "Workflow {name} {}: {summary}",
                    if success { "completed" } else { "failed" }
                );
                crate::status::card(
                    if success {
                        "workflow"
                    } else {
                        "workflow_error"
                    },
                    &message,
                    !success,
                );
                let _ = ui_tx.try_send(HyuskEvent::Response(message));
                start_next_workflow(
                    &mut workflow_queue,
                    &mut active_workflow,
                    active.is_some(),
                    listen_capture_active(&listen) || voice_conversation,
                    &workflow_tools,
                    &agent_tx,
                    &ui_tx,
                );
            }

            HyuskEvent::ScheduledAgentTask { id, prompt } => {
                println!("[Scheduler #{id}] Agent task due");
                if should_defer_scheduled_input(
                    active.is_some(),
                    listen_capture_active(&listen),
                    voice_conversation,
                ) || approval_blocks_scheduled_input(&control)
                {
                    println!("[Scheduler #{id}] Queued behind the current interaction");
                    crate::status::card(
                        "scheduled_task",
                        "Scheduled agent task is waiting for the current interaction to end",
                        false,
                    );
                    scheduled_inputs.push_back(prompt);
                } else {
                    crate::status::card(
                        "scheduled_task",
                        "Scheduled agent task is starting",
                        false,
                    );
                    pending_followups.push_back(HyuskEvent::ScheduledAgentInput(prompt));
                }
            }

            HyuskEvent::SubtaskFinished {
                id,
                label,
                success,
                summary,
            } => {
                println!(
                    "[Task #{id}] {label} {}:\n{summary}",
                    if success { "completed" } else { "failed" }
                );
                let report = format!(
                    "Background task {label} {}:\n{summary}",
                    if success { "completed" } else { "failed" }
                );
                crate::status::card("task", &report, true);
                let _ = ui_tx.try_send(HyuskEvent::Response(report));
                completed_tasks.push_back((label, success));
                if active.is_none() {
                    announce_completed_tasks(&mut completed_tasks, &ui_tx, &tts).await;
                }
            }

            HyuskEvent::ApprovalExpired => {
                // The timer reads the shared control, but applying the expiry
                // rewrites turn state, so it still needs the agent mutex. Try
                // the lock: a running turn ends the confirmation itself with
                // `expire_stale_approval`, and the timer retries shortly.
                let expired = agent
                    .try_lock()
                    .ok()
                    .and_then(|mut agent| agent.expire_stale_approval());
                let Some(message) = expired else {
                    continue;
                };
                println!("[Agent] Pending confirmation expired");
                // A response card, not an approval card: the decision is gone,
                // so the panels must not keep offering Approve and Deny.
                crate::status::card("response", &message, false);
                let _ = ui_tx.try_send(HyuskEvent::Response(message));
                release_next_scheduled_input(
                    &mut scheduled_inputs,
                    &mut pending_followups,
                    active.is_some(),
                    listen_capture_active(&listen),
                    voice_conversation,
                    approval_blocks_scheduled_input(&control),
                );
            }

            HyuskEvent::StopRequested => {
                println!("[Agent] Emergency stop requested");
                // Disarmed before anything is interrupted: the control lock is
                // free even while a turn holds the agent, so a slow model call
                // cannot keep a dangerous action armed (L1).
                if control_state::lock(&control).clear_pending_approval() {
                    crate::status::card("approval", "Pending confirmation cancelled.", false);
                }
                clear_pending_interactions(&mut pending_followups, &mut scheduled_inputs);
                last_finished_turn = None;
                deferred_reply_listen = None;
                interrupt_interaction(&mut active, &mut listen, &mut active_workflow, &ui_tx, true)
                    .await;
                let queued = workflow_queue.len();
                workflow_queue.clear();
                if queued > 0 {
                    let message = format!("Cancelled {queued} queued workflow(s).");
                    crate::status::card("workflow", &message, false);
                    let _ = ui_tx.try_send(HyuskEvent::Response(message));
                }
                voice_conversation = false;
                voice_listen_until = None;
                crate::status::awaiting_reply(false);
                if resume_media.swap(false, Ordering::Relaxed) {
                    tokio::spawn(media::resume());
                }
                announce_completed_tasks(&mut completed_tasks, &ui_tx, &tts).await;
                send_state(&ui_tx, HyuskState::Hidden).await;
            }

            HyuskEvent::StopListening => {
                if let Some(listen) = &listen {
                    listen.cancel.cancel();
                }
                interrupt_listen(&mut listen).await;
                finish_voice_conversation(&mut voice_conversation, &mut voice_listen_until);
                release_next_scheduled_input(
                    &mut scheduled_inputs,
                    &mut pending_followups,
                    active.is_some(),
                    listen_capture_active(&listen),
                    voice_conversation,
                    approval_blocks_scheduled_input(&control),
                );
                if resume_media.swap(false, Ordering::Relaxed) {
                    tokio::spawn(media::resume());
                }
                send_state(&ui_tx, HyuskState::Hidden).await;
                start_next_workflow(
                    &mut workflow_queue,
                    &mut active_workflow,
                    active.is_some(),
                    listen_capture_active(&listen) || voice_conversation,
                    &workflow_tools,
                    &agent_tx,
                    &ui_tx,
                );
            }

            HyuskEvent::ModelSelected { provider, model } => {
                if let Some(listen) = &listen {
                    listen.cancel.cancel();
                }
                remove_stale_listen_followups(&mut pending_followups);
                last_finished_turn = None;
                deferred_reply_listen = None;
                let _ = tokio::join!(interrupt_active(&mut active), interrupt_listen(&mut listen));
                finish_voice_conversation(&mut voice_conversation, &mut voice_listen_until);
                release_next_scheduled_input(
                    &mut scheduled_inputs,
                    &mut pending_followups,
                    active.is_some(),
                    listen_capture_active(&listen),
                    voice_conversation,
                    // Selecting a model clears any armed approval, so it must
                    // not hold back input released after the switch.
                    false,
                );
                let result = agent.lock().await.select_model(&provider, &model);
                sonic_mode = control_state::lock(&control).uses_sonic();
                let message = match result {
                    Ok(()) => {
                        crate::status::model(&provider, &model);
                        crate::settings::save(&crate::settings::ModelSelection {
                            provider: provider.clone(),
                            model: model.clone(),
                        });
                        format!("Switched to {provider}: {model}. Started a new conversation.")
                    }
                    Err(error) => format!("Could not switch model: {error}"),
                };
                crate::status::card("model", &message, false);
                let _ = ui_tx.try_send(HyuskEvent::Response(message));
                start_next_workflow(
                    &mut workflow_queue,
                    &mut active_workflow,
                    active.is_some(),
                    listen_capture_active(&listen),
                    &workflow_tools,
                    &agent_tx,
                    &ui_tx,
                );
            }

            HyuskEvent::ApiKeySaved { provider } => {
                // Read the key back from the keyring instead of carrying it in
                // the event, then hand the fresh client to the live agent so
                // the next model switch uses it without a service restart.
                let message = match crate::model::configured_client(&provider).await {
                    Some(client) => {
                        // Installed through the shared control, so a key save
                        // never waits for a running turn to finish (L1).
                        control_state::lock(&control).set_provider_client(&provider, Some(client));
                        format!("Saved the {provider} key. Pick a model from the Hyusk panel.")
                    }
                    None => format!(
                        "Saved the {provider} key, but it could not be read back from the keyring."
                    ),
                };
                crate::status::card("key", &message, false);
            }

            HyuskEvent::Shutdown => {
                println!("[Agent] Shutdown requested");
                interrupt_interaction(&mut active, &mut listen, &mut active_workflow, &ui_tx, true)
                    .await;
                workflow_queue.clear();
                break;
            }

            _ => {}
        }
    }

    // A closed event channel is also a runtime shutdown path. Do not leave
    // microphone, turn, or workflow workers detached if Shutdown was not sent.
    interrupt_interaction(&mut active, &mut listen, &mut active_workflow, &ui_tx, true).await;
    workflow_queue.clear();

    Ok(())
}

fn start_next_workflow(
    queue: &mut VecDeque<WorkflowJob>,
    active: &mut Option<ActiveWorkflow>,
    turn_active: bool,
    listen_active: bool,
    tools: &ToolRegistry,
    event_tx: &Sender<HyuskEvent>,
    ui_tx: &Sender<HyuskEvent>,
) {
    if active.is_some() {
        return;
    }
    let eligible = queue
        .iter()
        .position(|job| !job.desktop || (!turn_active && !listen_active));
    let Some(index) = eligible else {
        return;
    };

    let job = queue.remove(index).expect("eligible workflow exists");
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker_tx = event_tx.clone();
    let worker_tools = tools.clone();
    let id = job.id;
    let name = job.name.clone();
    let active_name = job.name;
    let actions = job.actions;
    crate::status::card("workflow", format!("Workflow {name} is running."), false);
    let _ = ui_tx.try_send(HyuskEvent::Response(format!("Workflow {name} is running.")));
    let handle = tokio::spawn(async move {
        let result = execute_workflow_actions(&actions, &worker_tools, &worker_cancel).await;
        let (success, summary) = match result {
            Ok(summary) => (true, summary),
            Err(error) => (false, error),
        };
        let _ = worker_tx
            .send(HyuskEvent::WorkflowFinished {
                id,
                name,
                success,
                summary,
            })
            .await;
    });
    *active = Some(ActiveWorkflow {
        id,
        name: active_name,
        desktop: job.desktop,
        cancel,
        handle,
    });
}

fn finish_voice_conversation(
    voice_conversation: &mut bool,
    voice_listen_until: &mut Option<std::time::Instant>,
) {
    *voice_conversation = false;
    *voice_listen_until = None;
    crate::status::awaiting_reply(false);
}

fn listen_capture_active(listen: &Option<ActiveListen>) -> bool {
    listen
        .as_ref()
        .is_some_and(|listen| !listen.handle.is_finished())
}

fn normalize_voice_transcript(
    id: u64,
    text: String,
    listen: &Option<ActiveListen>,
) -> Option<HyuskEvent> {
    listen
        .as_ref()
        .filter(|capture| {
            capture.id == id && capture.probe_id.is_none() && !capture.cancel.is_cancelled()
        })
        .map(|_| {
            if crate::agent::runner::is_voice_session_stop(&text) {
                HyuskEvent::StopListening
            } else {
                HyuskEvent::UserInput(text)
            }
        })
}

fn normalize_listen_completed(
    id: u64,
    continue_listening: bool,
    listen: &Option<ActiveListen>,
) -> Option<HyuskEvent> {
    listen
        .as_ref()
        .filter(|capture| {
            capture.id == id && capture.probe_id.is_none() && !capture.cancel.is_cancelled()
        })
        .map(|_| {
            if continue_listening {
                HyuskEvent::ContinueListening
            } else {
                HyuskEvent::StopListening
            }
        })
}

fn is_current_sonic_listen(id: u64, listen: &Option<ActiveListen>) -> bool {
    listen.as_ref().is_some_and(|capture| {
        capture.id == id && capture.probe_id.is_none() && !capture.cancel.is_cancelled()
    })
}

fn should_defer_scheduled_input(
    turn_active: bool,
    listen_active: bool,
    voice_active: bool,
) -> bool {
    turn_active || listen_active || voice_active
}

/// Resolve when the armed confirmation's deadline passes, or never when no
/// confirmation is armed.
///
/// The deadline comes from the shared control state, so a turn holding the
/// agent mutex can neither hide it nor delay the timer.
async fn approval_expiry(control: &ControlHandle) {
    // Bind the deadline first: a guard in a `match` scrutinee lives until the
    // end of the match, which would hold the control lock across the sleep.
    let deadline = control_state::lock(control).deadline();
    let Some(deadline) = deadline else {
        std::future::pending::<()>().await;
        return;
    };
    let delay = deadline.saturating_duration_since(std::time::Instant::now());
    if delay.is_zero() {
        // Already due but not yet applied, because a turn held the agent mutex
        // when the expiry arrived. Retry shortly instead of spinning the loop.
        tokio::time::sleep(Duration::from_millis(100)).await;
    } else {
        tokio::time::sleep(delay).await;
    }
}

/// The reply for a panel decision that has nothing left to decide.
const STALE_APPROVAL_REPLY: &str =
    "That confirmation expired, so I did not perform the action. Please ask again if you still want it.";

/// Apply a panel decision to the armed confirmation.
///
/// `Ok(input)` is the turn input that answers the stored call ("yes" or "no");
/// `Err(reply)` means no confirmation is armed, so the click is stale and must
/// not start a turn.
fn apply_approval_decision(
    control: &ControlHandle,
    approved: bool,
    scope: ApprovalScope,
) -> std::result::Result<String, &'static str> {
    // A turn grant rides the same decision path as the spoken form, so voice
    // and panel cannot diverge.
    let input = match (approved, scope) {
        (true, ApprovalScope::Turn) => crate::agent::runner::ALLOW_FOR_THIS_TURN,
        (true, ApprovalScope::Once) => "yes",
        (false, _) => "no",
    }
    .to_string();
    if armed_approval(control) {
        Ok(input)
    } else {
        Err(STALE_APPROVAL_REPLY)
    }
}

/// Whether a confirmation is armed.
///
/// Read from the shared control state, so the answer is always current: a turn
/// or a Sonic session holding the agent mutex cannot make it unknowable.
fn armed_approval(control: &ControlHandle) -> bool {
    control_state::lock(control).armed()
}

/// Whether an armed tool approval holds back queued scheduled input.
///
/// A scheduled prompt is not an answer to a confirmation. While one is armed
/// the prompt waits instead of being consumed as a yes/no reply.
fn approval_blocks_scheduled_input(control: &ControlHandle) -> bool {
    armed_approval(control)
}

fn release_next_scheduled_input(
    scheduled_inputs: &mut VecDeque<String>,
    pending_followups: &mut VecDeque<HyuskEvent>,
    turn_active: bool,
    listen_active: bool,
    voice_active: bool,
    approval_pending: bool,
) -> bool {
    if !should_defer_scheduled_input(turn_active, listen_active, voice_active) && !approval_pending
    {
        if let Some(prompt) = scheduled_inputs.pop_front() {
            pending_followups.push_back(HyuskEvent::ScheduledAgentInput(prompt));
            return true;
        }
    }
    false
}

fn clear_pending_interactions(
    pending_followups: &mut VecDeque<HyuskEvent>,
    scheduled_inputs: &mut VecDeque<String>,
) {
    pending_followups.clear();
    scheduled_inputs.clear();
}

fn remove_stale_listen_followups(pending_followups: &mut VecDeque<HyuskEvent>) {
    pending_followups.retain(|event| {
        !matches!(
            event,
            HyuskEvent::ContinueListening | HyuskEvent::AwaitReplyListening { .. }
        )
    });
}

fn should_probe_busy_wake(event: &HyuskEvent, sonic_mode: bool) -> bool {
    // Follow-up notifications cannot cancel or replace a running task. Nova
    // owns its live mic/barge-in path; text providers validate new speech first.
    matches!(event, HyuskEvent::WakeWordDetected) && !sonic_mode
}

async fn cancel_desktop_workflow(active: &mut Option<ActiveWorkflow>, ui_tx: &Sender<HyuskEvent>) {
    if active
        .as_ref()
        .is_some_and(|workflow| workflow.desktop && !workflow.handle.is_finished())
    {
        cancel_workflow(active, ui_tx).await;
    }
}

fn signal_interaction_cancellation(
    turn: &Option<ActiveTurn>,
    listen: &Option<ActiveListen>,
    workflow: &Option<ActiveWorkflow>,
    cancel_any_workflow: bool,
) {
    if let Some(turn) = turn {
        turn.cancel.cancel();
    }
    if let Some(listen) = listen {
        listen.cancel.cancel();
    }
    if let Some(workflow) = workflow {
        if cancel_any_workflow || workflow.desktop {
            workflow.cancel.cancel();
        }
    }
}

async fn interrupt_interaction(
    turn: &mut Option<ActiveTurn>,
    listen: &mut Option<ActiveListen>,
    workflow: &mut Option<ActiveWorkflow>,
    ui_tx: &Sender<HyuskEvent>,
    cancel_any_workflow: bool,
) {
    signal_interaction_cancellation(turn, listen, workflow, cancel_any_workflow);
    if cancel_any_workflow {
        let _ = tokio::join!(
            interrupt_active(turn),
            interrupt_listen(listen),
            cancel_workflow(workflow, ui_tx),
        );
    } else {
        let _ = tokio::join!(
            interrupt_active(turn),
            interrupt_listen(listen),
            cancel_desktop_workflow(workflow, ui_tx),
        );
    }
}

async fn cancel_workflow(active: &mut Option<ActiveWorkflow>, ui_tx: &Sender<HyuskEvent>) {
    if let Some(mut workflow) = active.take() {
        let message = format!("Workflow {} cancelled.", workflow.name);
        workflow.cancel.cancel();
        if tokio::time::timeout(Duration::from_millis(350), &mut workflow.handle)
            .await
            .is_err()
        {
            workflow.handle.abort();
            let _ = workflow.handle.await;
        }
        crate::status::card("workflow", &message, false);
        let _ = ui_tx.try_send(HyuskEvent::Response(message));
    }
}

async fn execute_workflow_actions(
    actions: &[crate::agent::commands::Action],
    tools: &ToolRegistry,
    cancel: &CancellationToken,
) -> std::result::Result<String, String> {
    let mut completed = 0usize;
    for action in actions {
        if cancel.is_cancelled() {
            return Err("cancelled before all steps completed".to_string());
        }
        let name = action.tool_name();
        let input = action.tool_input();
        // A credential read is refused outright, before the approval check, so
        // a workflow step cannot even be held for confirmation (S2).
        if let Some(refusal) = crate::tools::safety::secret_refusal(name, &input) {
            return Err(format!("step {} {refusal}", completed + 1));
        }
        // A capability the desktop has refused fails the step without asking
        // for an approval the workflow cannot show (D3).
        if let Some(failure) = crate::tools::capabilities::refusal(name, &input) {
            return Err(format!("step {} {failure}", completed + 1));
        }
        if crate::tools::safety::requires_approval(name, &input) {
            return Err(format!(
                "step {} ({name}) requires user approval; workflows cannot confirm it, so run it from the assistant instead",
                completed + 1
            ));
        }
        let Some(tool) = tools.get(name) else {
            return Err(format!(
                "step {} needs unavailable tool {name}",
                completed + 1
            ));
        };
        let result = tokio::select! {
            _ = cancel.cancelled() => return Err("cancelled before all steps completed".to_string()),
            result = tool.execute(&input) => result.map_err(|error| {
                format!("step {} ({name}) failed: {error}", completed + 1)
            })?,
        };
        if !result.success {
            let detail = result.error.unwrap_or(result.output);
            return Err(format!("step {} ({name}) failed: {detail}", completed + 1));
        }
        completed += 1;
    }
    Ok(format!("{completed} step(s) completed."))
}

async fn announce_completed_tasks(
    completed_tasks: &mut VecDeque<(String, bool)>,
    ui_tx: &Sender<HyuskEvent>,
    tts: &TextToSpeech,
) {
    for announcement in take_completed_task_announcements(completed_tasks) {
        let _ = ui_tx.try_send(HyuskEvent::Response(announcement.clone()));
        crate::status::card("task", &announcement, true);
        let tts = tts.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            let _ = tts.speak_cancellable(&announcement, &cancel).await;
        });
    }
}

fn take_completed_task_announcements(
    completed_tasks: &mut VecDeque<(String, bool)>,
) -> Vec<String> {
    completed_tasks
        .drain(..)
        .map(|(label, success)| {
            format!(
                "Background task {label} {}. Its report is ready.",
                if success { "completed" } else { "failed" }
            )
        })
        .collect()
}

/// Record a short command from the microphone and forward it as user input.
///
/// This runs as a dedicated task so a slow or failed recording never blocks
/// the event loop: typed input, another wake word, and shutdown all keep
/// working. The wake detector is resumed before this task even starts.
#[allow(clippy::too_many_arguments)]
async fn handle_listen(
    stt: Arc<SpeechToText>,
    agent_tx: Sender<HyuskEvent>,
    ui_tx: Sender<HyuskEvent>,
    wake_resume: WakeResume,
    cancel: CancellationToken,
    agent: Arc<Mutex<Agent>>,
    control: ControlHandle,
    output_active: Arc<AtomicBool>,
    listen_until: Option<std::time::Instant>,
    listen_id: u64,
) {
    // Always hand the microphone back to the wake detector, even when this
    // task is cancelled mid-recording.
    let _resume_guard = ResumeOnDrop(wake_resume);

    let stt_start = std::time::Instant::now();

    // Read the selection from the shared control: a turn may be finishing and
    // holding the agent mutex while this capture starts (L1).
    if control_state::lock(&control).uses_sonic() {
        output_active.store(true, Ordering::Relaxed);
        let _output_guard = OutputOnDrop(output_active);
        send_state(&ui_tx, HyuskState::Listening).await;
        let mut sonic_agent = agent.lock().await;
        let result = tokio::select! {
            _ = cancel.cancelled() => None,
            result = sonic_agent.handle_sonic_live_audio(Arc::clone(&stt), Some(&ui_tx), &cancel) => Some(result),
        };
        drop(sonic_agent);
        if !cancel.is_cancelled() {
            let (text, waiting, error) = match result {
                Some(Ok(Some(text))) => {
                    let waiting = agent.lock().await.reply_needs_follow_up() == Some(true);
                    (Some(text), waiting, None)
                }
                Some(Ok(None)) | None => (None, false, None),
                Some(Err(error)) => (None, false, Some(error.to_string())),
            };
            if !cancel.is_cancelled() {
                let _ = agent_tx
                    .send(HyuskEvent::SonicListenCompleted {
                        id: listen_id,
                        text,
                        waiting,
                        error,
                    })
                    .await;
            }
        }
        return;
    }

    // A generous cap; recording still stops shortly after the speaker pauses,
    // so this only bounds unusually long requests.
    let capture_cancel = cancel.child_token();
    let recording = stt.transcribe_from_microphone(8.0, &capture_cancel);

    let transcription = match timeout_with_cancel(Duration::from_secs(45), &capture_cancel, async {
        tokio::select! {
            _ = cancel.cancelled() => None,
            result = recording => Some(result),
        }
    })
    .await
    {
        Some(Some(Ok(text))) => Some(text),
        Some(Some(Err(error))) => {
            eprintln!("[STT] {error}");
            None
        }
        Some(None) => {
            println!("[STT] Listening replaced by a newer request");
            None
        }
        None => {
            eprintln!("[STT] Transcription timed out");
            None
        }
    };

    if cancel.is_cancelled() {
        return;
    }

    crate::timing::mark("record+transcribe", stt_start);

    match transcription {
        Some(text) if !text.trim().is_empty() => {
            let _ = agent_tx
                .send(HyuskEvent::VoiceTranscript {
                    id: listen_id,
                    text,
                })
                .await;
        }

        _ => {
            let continue_listening =
                listen_until.is_some_and(|deadline| std::time::Instant::now() < deadline);
            let _ = agent_tx
                .send(HyuskEvent::ListenCompleted {
                    id: listen_id,
                    continue_listening,
                })
                .await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_turn(
    agent: Arc<Mutex<Agent>>,
    text: String,
    ui_tx: Sender<HyuskEvent>,
    tts: TextToSpeech,
    cancel: CancellationToken,
    output_active: Arc<AtomicBool>,
    resume_media: Arc<AtomicBool>,
    event_tx: Sender<HyuskEvent>,
    voice_conversation: bool,
    turn_id: u64,
) {
    // Keep the detector muted until this turn (including TTS) is done.
    let _output_guard = OutputOnDrop(output_active);
    let _media_guard = MediaOnDrop(resume_media);

    println!("[User] {text}");

    send_state(&ui_tx, HyuskState::Thinking).await;

    let turn_start = std::time::Instant::now();

    /*
     * TTS worker. The agent forwards a sentence as soon as it completes while
     * the reply is still streaming, so the first words start before the full
     * answer is generated. A reply that was not streamed arrives as one chunk
     * and is spoken the same way.
     */
    let (sentence_tx, mut sentence_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let worker_tts = tts.clone();
    let worker_ui = ui_tx.clone();
    let worker_cancel = cancel.clone();

    let tts_worker = tokio::spawn(async move {
        let mut speaking = false;

        while let Some(sentence) = sentence_rx.recv().await {
            if worker_cancel.is_cancelled() {
                break;
            }

            if !speaking {
                speaking = true;
                send_state(&worker_ui, HyuskState::Speaking).await;
            }

            if let Err(error) = worker_tts
                .speak_cancellable(&sentence, &worker_cancel)
                .await
            {
                eprintln!("[TTS] Error: {error}");
            }
        }
    });

    let response = {
        let mut agent = agent.lock().await;

        // A paused agent task owns a bare continuation request. The instant
        // media router also recognizes "continue" and "resume", so bypass it
        // here only while there is a task to resume.
        let resume_paused_task = agent.has_paused_task()
            && matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "continue" | "resume"
            );
        let fast = if resume_paused_task {
            None
        } else {
            agent.try_fast_command(&text, &cancel).await
        };
        if let Some(fast) = fast {
            // Common commands run locally, with no model round trip.
            let _ = sentence_tx.send(fast.clone());
            fast
        } else {
            match agent
                .handle(text, Some(&ui_tx), &cancel, Some(&sentence_tx))
                .await
            {
                Ok(Some(response)) => response,

                Ok(None) => {
                    // Interrupted by a new request.
                    drop(sentence_tx);
                    let _ = tts_worker.await;
                    return;
                }

                Err(error) => {
                    eprintln!("[Agent] {error}");

                    if !cancel.is_cancelled() {
                        let detail = error.to_string();
                        let detail: String = detail.chars().take(400).collect();
                        crate::status::card("error", format!("Task stopped: {detail}"), true);
                        let message = "I hit an error and couldn't verify whether the task finished. The details are in the Hyusk card.".to_string();
                        let _ = ui_tx.try_send(HyuskEvent::Response(message.clone()));
                        let _ = sentence_tx.send(message);
                    }

                    drop(sentence_tx);
                    let _ = tts_worker.await;

                    if !cancel.is_cancelled() {
                        send_state(&ui_tx, HyuskState::Hidden).await;
                    }

                    return;
                }
            }
        }
    };

    crate::timing::mark("agent turn (model + tools)", turn_start);

    let (approval_preview, model_needs_reply) = {
        let agent = agent.lock().await;
        (
            agent.pending_approval_preview(),
            agent.reply_needs_follow_up(),
        )
    };
    let requires_approval = approval_preview.is_some();
    crate::status::card(
        if requires_approval {
            crate::status::APPROVAL_CARD_KIND
        } else {
            "response"
        },
        card_text(requires_approval, approval_preview.as_deref(), &response),
        requires_approval,
    );
    let _ = ui_tx.try_send(HyuskEvent::Response(response.clone()));

    // Close the sink so the worker drains and finishes.
    drop(sentence_tx);

    let tts_start = std::time::Instant::now();
    let _ = tts_worker.await;
    crate::timing::mark("tts", tts_start);

    // Only open a follow-up window when the response actually asks the user
    // for a decision or answer. Completed commands return to wake-word mode.
    let needs_reply =
        requires_approval || model_needs_reply.unwrap_or_else(|| response_needs_reply(&response));
    if !cancel.is_cancelled() {
        crate::status::awaiting_reply(needs_reply);
        if needs_reply && !voice_conversation {
            let _ = event_tx
                .send(HyuskEvent::AwaitReplyListening { id: turn_id })
                .await;
        }
    }

    if !cancel.is_cancelled() {
        send_state(&ui_tx, HyuskState::Hidden).await;
    }
}

/// The status card text for a finished turn.
///
/// An armed confirmation names the exact call (tool and arguments) instead of
/// the model's paraphrase, so the card, the tool result, and the approved call
/// all describe the same thing.
fn card_text(requires_approval: bool, preview: Option<&str>, response: &str) -> String {
    match (requires_approval, preview) {
        (true, Some(preview)) => crate::tools::safety::pending_text(preview),
        _ => response.to_string(),
    }
}

fn response_needs_reply(response: &str) -> bool {
    let text = response.trim().to_ascii_lowercase();
    text.ends_with('?')
        || [
            "please say",
            "would you like",
            "do you want",
            "which one",
            "what should i",
            "what would you",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
}

async fn send_state(ui_tx: &Sender<HyuskEvent>, state: HyuskState) {
    state.publish();

    /*
     * Never block on the UI channel. In GNOME-indicator mode nothing
     * consumes this channel, and a blocking send there would stall the
     * event loop (while a turn holds the agent lock) once the buffer
     * filled. Losing a visual status event is harmless; freezing the
     * agent is not.
     */
    let _ = ui_tx.try_send(HyuskEvent::StateChanged(state));
}

#[cfg(test)]
mod tests {
    use super::{
        approval_blocks_scheduled_input, approval_expiry, cancel_desktop_workflow, card_text,
        clear_pending_interactions, execute_workflow_actions, finish_voice_conversation,
        interrupt_active, interrupt_interaction, is_current_sonic_listen, listen_capture_active,
        next_runtime_event, normalize_listen_completed, normalize_voice_transcript,
        release_next_scheduled_input, remove_stale_listen_followups, response_needs_reply,
        run_agent_task, should_defer_scheduled_input, should_probe_busy_wake, start_next_workflow,
        take_completed_task_announcements, timeout_with_cancel, ActiveListen, ActiveTurn,
        ActiveWorkflow, WorkflowJob,
    };
    use crate::{
        agent::{commands::Action, control as control_state, Agent},
        tools::{Tool, ToolRegistry, ToolResult},
    };
    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    struct CountingTool(Arc<AtomicUsize>, bool);

    #[test]
    fn approval_card_shows_the_exact_call_not_the_models_paraphrase() {
        assert_eq!(
            card_text(
                true,
                Some("shell command=rm -rf build"),
                "I will clean up the build output.",
            ),
            "Pending confirmation for: shell command=rm -rf build"
        );
        // A completed turn still shows what the assistant said.
        assert_eq!(card_text(false, None, "Timer set."), "Timer set.");
        // An armed call without a preview falls back to the reply.
        assert_eq!(card_text(true, None, "Which one?"), "Which one?");
    }

    #[test]
    fn busy_wake_requires_new_speech_and_follow_up_cannot_interrupt() {
        use crate::types::HyuskEvent;
        assert!(should_probe_busy_wake(&HyuskEvent::WakeWordDetected, false));
        assert!(!should_probe_busy_wake(
            &HyuskEvent::ContinueListening,
            false
        ));
        assert!(!should_probe_busy_wake(&HyuskEvent::WakeWordDetected, true));
        assert!(!should_probe_busy_wake(
            &HyuskEvent::ContinueListening,
            true
        ));
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            "memory"
        }
        fn description(&self) -> &str {
            "test tool"
        }
        fn input_schema(&self) -> Value {
            json!({"type":"object"})
        }
        async fn execute(&self, _input: &str) -> Result<ToolResult> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(if self.1 {
                ToolResult::failure("deliberate failure")
            } else {
                ToolResult::success("ok")
            })
        }
    }

    #[test]
    fn only_explicit_questions_open_follow_up_listening() {
        assert!(response_needs_reply("Which timer duration should I use?"));
        assert!(response_needs_reply("Please say confirm to continue."));
        assert!(response_needs_reply("Would you like me to open it?"));
        assert!(!response_needs_reply(
            "The timer is set for thirty minutes."
        ));
        assert!(!response_needs_reply("Done."));
    }

    #[tokio::test]
    async fn emergency_interrupt_does_not_wait_forever_for_a_turn() {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(std::future::pending());
        let mut active = Some(ActiveTurn {
            id: 1,
            cancel: cancel.clone(),
            handle,
        });
        let started = Instant::now();

        interrupt_active(&mut active).await;

        assert!(cancel.is_cancelled());
        assert!(active.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn internal_followup_does_not_send_into_a_full_actor_channel() {
        use crate::types::HyuskEvent;

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tx.send(HyuskEvent::StopRequested).await.unwrap();
        let mut pending = VecDeque::from([HyuskEvent::ContinueListening]);

        let first = tokio::time::timeout(
            Duration::from_millis(50),
            next_runtime_event(&mut rx, &mut pending),
        )
        .await
        .expect("reading external events should not wait for internal send capacity");
        assert!(matches!(first, Some(HyuskEvent::StopRequested)));

        let second = tokio::time::timeout(
            Duration::from_millis(50),
            next_runtime_event(&mut rx, &mut pending),
        )
        .await
        .expect("the pending internal event should be delivered locally");
        assert!(matches!(second, Some(HyuskEvent::ContinueListening)));
    }

    #[tokio::test]
    async fn stale_or_cancelled_sonic_completion_is_rejected() {
        let active_handle = tokio::spawn(std::future::pending());
        let mut active = Some(ActiveListen {
            id: 9,
            cancel: CancellationToken::new(),
            handle: active_handle,
            probe_id: None,
        });
        assert!(is_current_sonic_listen(9, &active));
        assert!(!is_current_sonic_listen(8, &active));

        active.as_ref().unwrap().cancel.cancel();
        assert!(!is_current_sonic_listen(9, &active));
        active.as_mut().unwrap().handle.abort();
    }

    #[tokio::test]
    async fn stopping_clears_local_and_scheduled_followups() {
        use crate::types::HyuskEvent;
        let mut followups = VecDeque::from([
            HyuskEvent::ContinueListening,
            HyuskEvent::ScheduledAgentInput("later".into()),
        ]);
        let mut scheduled = VecDeque::from(["queued".to_string()]);

        clear_pending_interactions(&mut followups, &mut scheduled);

        assert!(followups.is_empty());
        assert!(scheduled.is_empty());
        assert!(should_defer_scheduled_input(true, false, false));
        assert!(should_defer_scheduled_input(false, true, false));
        assert!(should_defer_scheduled_input(false, false, true));
        assert!(!should_defer_scheduled_input(false, false, false));
    }

    #[tokio::test]
    async fn stale_voice_followups_are_removed_on_model_change() {
        use crate::types::HyuskEvent;
        let mut followups = VecDeque::from([
            HyuskEvent::ContinueListening,
            HyuskEvent::AwaitReplyListening { id: 4 },
            HyuskEvent::ScheduledAgentInput("keep this".into()),
        ]);

        remove_stale_listen_followups(&mut followups);

        assert!(matches!(
            followups.front(),
            Some(HyuskEvent::ScheduledAgentInput(text)) if text == "keep this"
        ));
        assert_eq!(followups.len(), 1);
    }

    #[test]
    fn scheduled_input_waits_for_interaction_to_end() {
        use crate::types::HyuskEvent;
        let mut scheduled = VecDeque::from(["scheduled prompt".to_string()]);
        let mut pending = VecDeque::new();

        assert!(!release_next_scheduled_input(
            &mut scheduled,
            &mut pending,
            true,
            false,
            false,
            false,
        ));
        assert!(pending.is_empty());
        assert!(release_next_scheduled_input(
            &mut scheduled,
            &mut pending,
            false,
            false,
            false,
            false,
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(HyuskEvent::ScheduledAgentInput(text)) if text == "scheduled prompt"
        ));
    }

    #[test]
    fn scheduled_input_waits_for_a_pending_confirmation() {
        use crate::types::HyuskEvent;
        let mut scheduled = VecDeque::from(["scheduled prompt".to_string()]);
        let mut pending = VecDeque::new();

        assert!(!release_next_scheduled_input(
            &mut scheduled,
            &mut pending,
            false,
            false,
            false,
            true,
        ));
        assert!(pending.is_empty());
        assert_eq!(scheduled.len(), 1, "the prompt is not consumed");

        assert!(release_next_scheduled_input(
            &mut scheduled,
            &mut pending,
            false,
            false,
            false,
            false,
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(HyuskEvent::ScheduledAgentInput(text)) if text == "scheduled prompt"
        ));
    }

    #[tokio::test]
    async fn a_running_turn_does_not_hide_an_armed_approval() {
        let mut agent = Agent::new(
            crate::model::openrouter::OpenRouterClient::new(
                "test-key".into(),
                "http://127.0.0.1".into(),
            ),
            "test-model".into(),
            ToolRegistry::new(),
        );
        agent.arm_test_approval();
        let control = agent.control_handle();
        let agent = Arc::new(tokio::sync::Mutex::new(agent));

        assert!(
            approval_blocks_scheduled_input(&control),
            "an armed confirmation holds scheduled input"
        );

        // The event loop reads the approval while a turn holds the agent
        // mutex, which is what the split exists for (L1).
        let running_turn = agent.lock().await;
        assert!(
            approval_blocks_scheduled_input(&control),
            "the armed confirmation is still visible during a turn"
        );
        assert!(running_turn.awaiting_approval());
        drop(running_turn);

        assert!(agent.lock().await.clear_pending_approval());
        assert!(!approval_blocks_scheduled_input(&control), "disarmed");

        // With nothing armed, a running turn no longer holds scheduled input
        // back on its own: the loop defers turns explicitly, and the old
        // "locked means defer" fallback would stall queued prompts forever.
        let holding = agent.lock().await;
        assert!(
            !approval_blocks_scheduled_input(&control),
            "a running turn is not an armed confirmation"
        );
        drop(holding);
    }

    /// L1: a turn holds the agent mutex across the model future. The stop path
    /// and a key save must still work immediately, and the turn must still see
    /// its cancellation token.
    #[tokio::test]
    async fn a_slow_model_call_cannot_hold_the_stop_path_or_a_key_save() {
        let mut agent = Agent::new(
            crate::model::openrouter::OpenRouterClient::new(
                "test-key".into(),
                "http://127.0.0.1".into(),
            ),
            "test-model".into(),
            ToolRegistry::new(),
        );
        agent.arm_test_approval();
        let control = agent.control_handle();
        assert!(
            control_state::lock(&control).armed(),
            "the handle must be the turn's own control state"
        );
        let agent = Arc::new(tokio::sync::Mutex::new(agent));

        // Stand-in for a turn: it owns the agent mutex and waits on a slow,
        // cancellable model future, exactly as `run_turn` does.
        let turn_cancel = CancellationToken::new();
        let slow_model = {
            let agent = Arc::clone(&agent);
            let cancel = turn_cancel.clone();
            tokio::spawn(async move {
                let _turn = agent.lock().await;
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Emergency stop: the armed approval is disarmed without the agent.
        let started = std::time::Instant::now();
        assert!(control_state::lock(&control).clear_pending_approval());
        assert!(!control_state::lock(&control).armed());
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "the stop path waited for the turn"
        );

        // Key save: the client is installed without the agent.
        let started = std::time::Instant::now();
        control_state::lock(&control).set_provider_client(
            "openrouter",
            Some(crate::model::openrouter::OpenRouterClient::new(
                "saved-key".into(),
                "https://example.test/v1".into(),
            )),
        );
        assert!(control_state::lock(&control)
            .provider_client("openrouter")
            .is_some());
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "the key save waited for the turn"
        );

        // The turn is still running and still owns the agent.
        assert!(agent.try_lock().is_err(), "the turn still holds the agent");

        // Cancelling the turn is observed promptly, so no lock was left held.
        let stopped = std::time::Instant::now();
        turn_cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), slow_model)
            .await
            .expect("the turn observes its cancellation")
            .expect("the turn task finished");
        assert!(stopped.elapsed() < Duration::from_millis(500));
        assert!(agent.try_lock().is_ok(), "the turn released the agent");
    }

    #[tokio::test]
    async fn approval_expiry_sleeps_until_the_deadline_or_forever() {
        let mut agent = Agent::new(
            crate::model::openrouter::OpenRouterClient::new(
                "test-key".into(),
                "http://127.0.0.1".into(),
            ),
            "test-model".into(),
            ToolRegistry::new(),
        );
        agent.arm_test_approval_at(std::time::Instant::now());
        let control = agent.control_handle();
        let agent = Arc::new(tokio::sync::Mutex::new(agent));

        tokio::time::timeout(Duration::from_secs(1), approval_expiry(&control))
            .await
            .expect("an expired deadline resolves immediately");

        agent.lock().await.clear_pending_approval();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), approval_expiry(&control))
                .await
                .is_err(),
            "no armed confirmation means no timer"
        );

        // A held agent mutex no longer defers the check: the deadline lives in
        // the shared control, so the timer still fires during a slow turn (L1).
        agent
            .lock()
            .await
            .arm_test_approval_at(std::time::Instant::now());
        let running_turn = agent.lock().await;
        tokio::time::timeout(Duration::from_secs(1), approval_expiry(&control))
            .await
            .expect("the timer fires while a turn holds the agent");
        drop(running_turn);
    }

    #[tokio::test]
    async fn an_expired_confirmation_releases_queued_scheduled_input() {
        use crate::types::HyuskEvent;

        let mut agent = Agent::new(
            crate::model::openrouter::OpenRouterClient::new(
                "test-key".into(),
                "http://127.0.0.1".into(),
            ),
            "test-model".into(),
            ToolRegistry::new(),
        );
        agent.arm_test_approval_at(std::time::Instant::now());
        let control = agent.control_handle();
        let agent = Arc::new(tokio::sync::Mutex::new(agent));

        assert!(approval_blocks_scheduled_input(&control));

        let mut scheduled = VecDeque::from(["scheduled prompt".to_string()]);
        let mut pending = VecDeque::new();
        assert!(!release_next_scheduled_input(
            &mut scheduled,
            &mut pending,
            false,
            false,
            false,
            approval_blocks_scheduled_input(&control),
        ));
        assert_eq!(scheduled.len(), 1, "held while the confirmation is armed");

        assert!(agent.lock().await.expire_stale_approval().is_some());

        assert!(release_next_scheduled_input(
            &mut scheduled,
            &mut pending,
            false,
            false,
            false,
            approval_blocks_scheduled_input(&control),
        ));
        assert!(matches!(
            pending.pop_front(),
            Some(HyuskEvent::ScheduledAgentInput(text)) if text == "scheduled prompt"
        ));
    }

    #[tokio::test]
    async fn event_loop_expires_an_abandoned_confirmation_then_runs_the_queued_task() {
        use crate::types::{HyuskEvent, HyuskState};

        let mut agent = Agent::new(
            crate::model::openrouter::OpenRouterClient::new(
                "test-key".into(),
                // The released turn never reaches a model: the loop hands it
                // over and the test stops there, well before this connects.
                "http://127.0.0.1:9".into(),
            ),
            "test-model".into(),
            ToolRegistry::new(),
        );
        /*
         * Nobody answers this confirmation, so only the loop's timer resolves
         * it. The deadline leaves the loop ample time to pick up the scheduled
         * task below first, so the ordering being asserted does not depend on
         * a fast machine.
         */
        agent.arm_test_approval_at(std::time::Instant::now() + Duration::from_millis(700));

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(8);
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel(32);
        let loop_handle = tokio::spawn(run_agent_task(
            agent,
            event_rx,
            event_tx.clone(),
            ui_tx,
            crate::wake::detector::WakeResume::new(),
            None,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ));

        // A scheduled task comes due while the confirmation is still armed.
        event_tx
            .send(HyuskEvent::ScheduledAgentTask {
                id: 1,
                prompt: "summarize the release notes".into(),
            })
            .await
            .unwrap();

        /*
         * Nothing else is ever sent to the loop, so the first ui event can
         * only be the expiry reply. A scheduled input that leaked into a turn
         * while the confirmation was armed would announce "Thinking" first.
         */
        let expired = tokio::time::timeout(Duration::from_secs(5), ui_rx.recv())
            .await
            .expect("the approval timer fires without new input")
            .expect("the ui channel stays open");
        let HyuskEvent::Response(message) = expired else {
            panic!("expected the expiry reply before any turn, got {expired:?}");
        };
        assert!(
            message.contains("expired"),
            "expected the abandoned confirmation to expire, got: {message}"
        );

        // With the confirmation gone, the queued prompt starts its own turn.
        let started = tokio::time::timeout(Duration::from_secs(5), ui_rx.recv())
            .await
            .expect("the queued task is released after the expiry")
            .expect("the ui channel stays open");
        assert!(
            matches!(started, HyuskEvent::StateChanged(HyuskState::Thinking)),
            "expected the released scheduled task to start a turn, got {started:?}"
        );

        loop_handle.abort();
        let _ = loop_handle.await;
    }

    #[tokio::test]
    async fn interaction_cleanup_waits_concurrently_for_uncooperative_workers() {
        let turn_cancel = CancellationToken::new();
        let listen_cancel = CancellationToken::new();
        let workflow_cancel = CancellationToken::new();
        let mut turn = Some(ActiveTurn {
            id: 1,
            cancel: turn_cancel.clone(),
            handle: tokio::spawn(std::future::pending()),
        });
        let mut listen = Some(ActiveListen {
            id: 1,
            cancel: listen_cancel.clone(),
            handle: tokio::spawn(std::future::pending()),
            probe_id: None,
        });
        let mut workflow = Some(ActiveWorkflow {
            id: 1,
            name: "slow".into(),
            desktop: true,
            cancel: workflow_cancel.clone(),
            handle: tokio::spawn(std::future::pending()),
        });
        let (ui_tx, _ui_rx) = tokio::sync::mpsc::channel(4);
        let started = Instant::now();

        interrupt_interaction(&mut turn, &mut listen, &mut workflow, &ui_tx, true).await;

        assert!(turn_cancel.is_cancelled());
        assert!(listen_cancel.is_cancelled());
        assert!(workflow_cancel.is_cancelled());
        assert!(started.elapsed() < Duration::from_millis(650));
    }

    #[tokio::test]
    async fn transcription_timeout_stops_capture_without_ending_listen_lifecycle() {
        let lifecycle_cancel = CancellationToken::new();
        let capture_cancel = lifecycle_cancel.child_token();
        let result = timeout_with_cancel(
            Duration::from_millis(1),
            &capture_cancel,
            std::future::pending::<()>(),
        )
        .await;

        assert!(result.is_none());
        assert!(capture_cancel.is_cancelled());
        assert!(!lifecycle_cancel.is_cancelled());
    }

    #[tokio::test]
    async fn external_listen_cancellation_still_suppresses_stale_capture_completion() {
        let lifecycle_cancel = CancellationToken::new();
        let capture_cancel = lifecycle_cancel.child_token();
        let result = timeout_with_cancel(Duration::from_secs(1), &capture_cancel, async {
            tokio::select! {
                _ = lifecycle_cancel.cancelled() => None,
                _ = std::future::pending::<()>() => Some(()),
            }
        });
        lifecycle_cancel.cancel();

        assert_eq!(result.await, Some(None));
        assert!(lifecycle_cancel.is_cancelled());
        assert!(capture_cancel.is_cancelled());
    }

    #[tokio::test]
    async fn queued_new_input_invalidates_an_older_voice_transcript() {
        use crate::types::HyuskEvent;

        let old_cancel = CancellationToken::new();
        let old_handle = tokio::spawn(std::future::pending());
        let listen = Some(ActiveListen {
            id: 7,
            cancel: old_cancel.clone(),
            handle: old_handle,
            probe_id: None,
        });
        let (tx, mut rx) = tokio::sync::mpsc::channel(2);
        tx.send(HyuskEvent::UserInput("new request".into()))
            .await
            .unwrap();
        tx.send(HyuskEvent::VoiceTranscript {
            id: 7,
            text: "stale request".into(),
        })
        .await
        .unwrap();

        assert!(
            matches!(rx.recv().await, Some(HyuskEvent::UserInput(text)) if text == "new request")
        );
        // The UserInput dispatch interrupts the active capture before the next
        // event is received, even if its worker had already queued a result.
        old_cancel.cancel();
        let Some(HyuskEvent::VoiceTranscript { id, text }) = rx.recv().await else {
            panic!("expected queued voice transcript");
        };
        assert!(normalize_voice_transcript(id, text, &listen).is_none());
    }

    #[tokio::test]
    async fn current_voice_transcript_normalizes_to_user_input() {
        use crate::types::HyuskEvent;

        let handle = tokio::spawn(std::future::pending());
        let listen = Some(ActiveListen {
            id: 8,
            cancel: CancellationToken::new(),
            handle,
            probe_id: None,
        });

        assert!(matches!(
            normalize_voice_transcript(8, "valid request".into(), &listen),
            Some(HyuskEvent::UserInput(text)) if text == "valid request"
        ));
    }

    #[tokio::test]
    async fn stale_completion_cannot_end_a_newer_listen() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(2);
        tx.send(crate::types::HyuskEvent::WakeWordDetected)
            .await
            .unwrap();
        tx.send(crate::types::HyuskEvent::ListenCompleted {
            id: 7,
            continue_listening: false,
        })
        .await
        .unwrap();

        assert!(matches!(
            rx.recv().await,
            Some(crate::types::HyuskEvent::WakeWordDetected)
        ));
        let new_listen = Some(ActiveListen {
            id: 9,
            cancel: CancellationToken::new(),
            handle: tokio::spawn(std::future::pending()),
            probe_id: None,
        });
        let Some(crate::types::HyuskEvent::ListenCompleted {
            id,
            continue_listening,
        }) = rx.recv().await
        else {
            panic!("expected queued old listen completion");
        };

        assert!(normalize_listen_completed(id, continue_listening, &new_listen).is_none());
        assert!(listen_capture_active(&new_listen));
    }

    #[tokio::test]
    async fn normal_capture_events_do_not_match_a_busy_probe() {
        let listen = Some(ActiveListen {
            id: 1,
            cancel: CancellationToken::new(),
            handle: tokio::spawn(std::future::pending()),
            probe_id: Some(1),
        });

        assert!(normalize_voice_transcript(1, "not a probe result".into(), &listen).is_none());
        assert!(normalize_listen_completed(1, false, &listen).is_none());
    }

    #[test]
    fn stopping_can_flush_queued_background_task_reports() {
        let mut completed_tasks =
            VecDeque::from([("backup".to_string(), true), ("sync".to_string(), false)]);

        let announcements = take_completed_task_announcements(&mut completed_tasks);

        assert_eq!(completed_tasks.len(), 0);
        assert_eq!(announcements.len(), 2);
        assert_eq!(
            announcements[0],
            "Background task backup completed. Its report is ready."
        );
        assert_eq!(
            announcements[1],
            "Background task sync failed. Its report is ready."
        );
    }

    #[tokio::test]
    async fn workflow_stops_before_an_action_that_needs_approval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(CountingTool(calls.clone(), false));
        let action = Action::Remember {
            text: "ordinary note".into(),
        };
        let protected = Action::Shell {
            command: "rm -rf /tmp/example".into(),
        };

        let result =
            execute_workflow_actions(&[action, protected], &tools, &CancellationToken::new()).await;

        let error = result.unwrap_err();
        assert!(error.contains("requires user approval"));
        assert!(error.contains("(shell)"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    struct ShellStub(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for ShellStub {
        fn name(&self) -> &str {
            "shell"
        }
        fn description(&self) -> &str {
            "Test shell action."
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn execute(&self, _input: &str) -> Result<ToolResult> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(ToolResult::success("ran"))
        }
    }

    #[tokio::test]
    async fn the_model_fast_and_workflow_lanes_agree_on_a_guarded_request() {
        // One identical request, "lock screen", which the instant router turns
        // into `shell loginctl lock-session`. The policy guards that command, so
        // every lane must stop short of running it (A9).
        let plan = crate::agent::commands::parse("lock screen").expect("a fast command");
        assert_eq!(
            plan.actions,
            vec![Action::Shell {
                command: "loginctl lock-session".into(),
            }]
        );
        let action = plan.actions[0].clone();

        // Model lane: the gate the runner consults asks for this exact call.
        assert!(
            crate::tools::safety::requires_approval(action.tool_name(), &action.tool_input()),
            "the model lane must ask"
        );

        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent_tools = ToolRegistry::new();
        agent_tools.register(ShellStub(Arc::clone(&executions)));
        let mut workflow_tools = ToolRegistry::new();
        workflow_tools.register(ShellStub(Arc::clone(&executions)));
        let mut agent = Agent::new(
            crate::model::openrouter::OpenRouterClient::new(
                "test-key".into(),
                "http://127.0.0.1".into(),
            ),
            "test-model".into(),
            agent_tools,
        );

        // Fast lane: the instant router defers before the first side effect.
        let fast = agent
            .try_fast_command("lock screen", &CancellationToken::new())
            .await;
        assert!(
            fast.is_none(),
            "a guarded plan must defer to the model path, not answer locally"
        );

        // Workflow lane: the step stops before executing and says why.
        let error =
            execute_workflow_actions(&plan.actions, &workflow_tools, &CancellationToken::new())
                .await
                .unwrap_err();
        assert!(error.contains("requires user approval"), "{error}");

        assert_eq!(
            executions.load(Ordering::Relaxed),
            0,
            "no lane may run the guarded step"
        );
    }

    #[tokio::test]
    async fn workflow_refuses_a_credential_read_before_holding_it_for_approval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(CountingTool(calls.clone(), false));
        let action = Action::Remember {
            text: "ordinary note".into(),
        };
        let protected = Action::Shell {
            command: "cat .env".into(),
        };

        let result =
            execute_workflow_actions(&[action, protected], &tools, &CancellationToken::new()).await;

        let error = result.unwrap_err();
        assert!(error.contains("secret denylist"), "{error}");
        assert!(
            !error.contains(".env"),
            "the refusal must not echo the path: {error}"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn workflow_reports_tool_failures_without_model_fallback() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(CountingTool(calls.clone(), true));
        let action = Action::Remember {
            text: "ordinary note".into(),
        };

        let result = execute_workflow_actions(&[action], &tools, &CancellationToken::new()).await;

        assert_eq!(
            result.unwrap_err(),
            "step 1 (memory) failed: deliberate failure"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn finished_listen_task_does_not_block_a_queued_desktop_workflow() {
        let mut completed_listen = tokio::spawn(async {});
        (&mut completed_listen).await.unwrap();
        // Use a completed handle, mirroring a listen task that sent UserInput
        // and has returned while its slot has not yet been cleared.
        let listen = Some(ActiveListen {
            id: 4,
            cancel: CancellationToken::new(),
            handle: completed_listen,
            probe_id: None,
        });
        assert!(!listen_capture_active(&listen));

        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(CountingTool(calls.clone(), false));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let (ui_tx, _ui_rx) = tokio::sync::mpsc::channel(4);
        let mut queue = VecDeque::from([WorkflowJob {
            id: 4,
            name: "deferred desktop".into(),
            actions: vec![Action::Remember {
                text: "note".into(),
            }],
            desktop: true,
        }]);
        let mut active = None;

        start_next_workflow(
            &mut queue,
            &mut active,
            false,
            listen_capture_active(&listen),
            &tools,
            &event_tx,
            &ui_tx,
        );

        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            crate::types::HyuskEvent::WorkflowFinished { success: true, .. }
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn foreground_input_cancels_and_awaits_desktop_workflow() {
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let handle = tokio::spawn(async move { worker_cancel.cancelled().await });
        let mut active = Some(ActiveWorkflow {
            id: 9,
            name: "open app".into(),
            desktop: true,
            cancel: cancel.clone(),
            handle,
        });
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel(4);

        cancel_desktop_workflow(&mut active, &ui_tx).await;

        assert!(cancel.is_cancelled());
        assert!(active.is_none());
        assert!(matches!(
            ui_rx.try_recv(),
            Ok(crate::types::HyuskEvent::Response(text)) if text == "Workflow open app cancelled."
        ));
    }

    #[tokio::test]
    async fn exact_workflow_voice_request_releases_desktop_queue() {
        let mut voice_conversation = true;
        let mut voice_listen_until = Some(std::time::Instant::now() + Duration::from_secs(30));
        finish_voice_conversation(&mut voice_conversation, &mut voice_listen_until);
        assert!(!voice_conversation);
        assert!(voice_listen_until.is_none());

        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(CountingTool(calls.clone(), false));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let (ui_tx, _ui_rx) = tokio::sync::mpsc::channel(4);
        let mut queue = VecDeque::from([WorkflowJob {
            id: 5,
            name: "voice workflow".into(),
            actions: vec![Action::Remember {
                text: "note".into(),
            }],
            desktop: true,
        }]);
        let mut active = None;

        start_next_workflow(
            &mut queue,
            &mut active,
            false,
            listen_capture_active(&None) || voice_conversation,
            &tools,
            &event_tx,
            &ui_tx,
        );

        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            crate::types::HyuskEvent::WorkflowFinished { success: true, .. }
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn background_workflow_skips_a_deferred_desktop_job() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(CountingTool(calls.clone(), false));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let (ui_tx, _ui_rx) = tokio::sync::mpsc::channel(4);
        let action = || Action::Remember {
            text: "note".into(),
        };
        let mut queue = VecDeque::from([
            WorkflowJob {
                id: 6,
                name: "desktop first".into(),
                actions: vec![action()],
                desktop: true,
            },
            WorkflowJob {
                id: 7,
                name: "background second".into(),
                actions: vec![action()],
                desktop: false,
            },
        ]);
        let mut active = None;

        start_next_workflow(
            &mut queue,
            &mut active,
            true,
            false,
            &tools,
            &event_tx,
            &ui_tx,
        );

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.front().unwrap().id, 6);
        assert_eq!(active.as_ref().unwrap().id, 7);
        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            crate::types::HyuskEvent::WorkflowFinished {
                id: 7,
                success: true,
                ..
            }
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}

#[cfg(test)]
mod approval_decision_tests {
    use super::apply_approval_decision;
    use crate::{
        agent::runner::Agent, model::openrouter::OpenRouterClient, status::APPROVAL_CARD_KIND,
        tools::ToolRegistry, types::event::ApprovalScope,
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn test_agent() -> Arc<Mutex<Agent>> {
        let agent = Agent::new(
            OpenRouterClient::new("test-key".into(), "http://127.0.0.1".into()),
            "test-model".into(),
            ToolRegistry::new(),
        );
        Arc::new(Mutex::new(agent))
    }

    #[tokio::test]
    async fn a_decision_without_a_pending_approval_reports_expiry() {
        let agent = test_agent();
        let control = agent.lock().await.control_handle();
        let reply = apply_approval_decision(&control, true, ApprovalScope::Once)
            .expect_err("a stale approval must not start a turn");
        assert!(reply.contains("expired"), "{reply}");
        assert!(!agent.lock().await.awaiting_approval());
        // Nothing was armed, so approving twice is still stale.
        assert!(apply_approval_decision(&control, false, ApprovalScope::Once).is_err());
    }

    #[tokio::test]
    async fn an_armed_confirmation_accepts_the_decision_as_turn_input() {
        let agent = test_agent();
        let mut armed = agent.lock().await;
        armed.arm_test_approval();
        let control = armed.control_handle();
        // The decision is read while the agent mutex is still held, which is
        // the point of the split (L1).
        assert_eq!(
            apply_approval_decision(&control, true, ApprovalScope::Once).as_deref(),
            Ok("yes")
        );
        assert_eq!(
            apply_approval_decision(&control, false, ApprovalScope::Once).as_deref(),
            Ok("no")
        );
        // "Allow for this turn" rides the spoken path, so the runner records the
        // grant exactly as it does for the spoken form.
        assert_eq!(
            apply_approval_decision(&control, true, ApprovalScope::Turn).as_deref(),
            Ok(crate::agent::runner::ALLOW_FOR_THIS_TURN)
        );
        // The decision does not itself consume the stored call.
        assert!(armed.awaiting_approval());
    }

    #[tokio::test]
    async fn a_stale_click_is_detected_while_a_turn_holds_the_agent() {
        let agent = test_agent();
        let control = agent.lock().await.control_handle();
        // Hold the agent mutex, as a turn does, and decide anyway. Before the
        // split this call gave up after half a second and forwarded the click.
        let holding = agent.lock().await;
        let reply = apply_approval_decision(&control, true, ApprovalScope::Once)
            .expect_err("nothing is armed, so the click is stale");
        assert!(reply.contains("expired"), "{reply}");
        drop(holding);
    }

    #[test]
    fn the_approval_card_kind_matches_what_both_panels_expect() {
        // `gnome-extension/hyusk@hyusk.local/lib/responseCard.js` and
        // `noctalia-plugin/hyusk.luau` show Approve/Deny for this exact tag.
        assert_eq!(APPROVAL_CARD_KIND, "approval");
    }
}
