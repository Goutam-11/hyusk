use std::{
    collections::VecDeque,
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

use crate::{
    agent::Agent,
    speech::{SpeechToText, TextToSpeech},
    tools::media,
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
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

/// Release the paused wake detector when the listening flow ends, no matter
/// how it ends (success, error, timeout, or task cancellation).
///
/// The detector pauses inside its microphone thread after every detection;
/// if the listening task died without resuming it, the whole voice path
/// would stay deaf until the detector's own 120 s timeout.
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
    if let Some(turn) = active.take() {
        turn.cancel.cancel();
        let _ = turn.handle.await;
    }
}

async fn interrupt_listen(active: &mut Option<ActiveListen>) {
    if let Some(listen) = active.take() {
        listen.cancel.cancel();
        let _ = listen.handle.await;
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
    let agent = Arc::new(Mutex::new(agent));
    let tts = TextToSpeech::new();
    let mut active: Option<ActiveTurn> = None;
    let mut listen: Option<ActiveListen> = None;
    let mut completed_tasks = VecDeque::new();
    let mut scheduled_inputs = VecDeque::new();
    let mut next_turn_id = 1u64;

    /*
     * Set when media was playing and was paused for a voice command. The turn
     * resumes it afterwards unless the command itself controlled playback.
     */
    let resume_media = Arc::new(AtomicBool::new(false));

    while let Some(event) = event_rx.recv().await {
        match event {
            HyuskEvent::WakeWordDetected | HyuskEvent::ContinueListening => {
                println!("[Wake] Listening requested");

                /*
                 * A new voice request replaces any in-flight model, tool,
                 * TTS work, and any listening session that was already in
                 * progress (for example a double wake or a clap during the
                 * recording window).
                 */
                interrupt_active(&mut active).await;
                interrupt_listen(&mut listen).await;

                // Quiet any music so the command is heard, and remember to
                // resume it once the request is handled.
                if media::pause_if_playing().await {
                    resume_media.store(true, Ordering::Relaxed);
                }

                send_state(&ui_tx, HyuskState::Waking).await;
                send_state(&ui_tx, HyuskState::Listening).await;

                let Some(stt) = stt.clone() else {
                    eprintln!("[STT] Speech-to-text is not configured; ignoring wake word");
                    send_state(&ui_tx, HyuskState::Hidden).await;
                    wake_resume.resume();
                    continue;
                };

                let cancel = CancellationToken::new();
                let task_agent_tx = agent_tx.clone();
                let task_ui_tx = ui_tx.clone();
                let task_resume = wake_resume.clone();
                let task_cancel = cancel.clone();
                let task_media = Arc::clone(&resume_media);

                let handle = tokio::spawn(async move {
                    handle_listen(
                        stt,
                        task_agent_tx,
                        task_ui_tx,
                        task_resume,
                        task_cancel,
                        task_media,
                    )
                    .await;
                });

                listen = Some(ActiveListen { cancel, handle });
            }

            HyuskEvent::UserInput(text) => {
                if text.trim().is_empty() {
                    continue;
                }

                /*
                 * Interrupt the running turn before starting the new one so
                 * the agent is always responsive to the latest message.
                 */
                interrupt_active(&mut active).await;
                interrupt_listen(&mut listen).await;
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
                let id = next_turn_id;
                next_turn_id += 1;

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
                    )
                    .await;
                    let _ = turn_event_tx.send(HyuskEvent::TurnFinished { id }).await;
                });

                active = Some(ActiveTurn { id, cancel, handle });
            }

            HyuskEvent::TurnFinished { id } => {
                if active.as_ref().map(|turn| turn.id) == Some(id) {
                    active = None;
                    announce_completed_tasks(&mut completed_tasks, &ui_tx, &tts).await;
                    if let Some(text) = scheduled_inputs.pop_front() {
                        let _ = agent_tx.send(HyuskEvent::UserInput(text)).await;
                    }
                }
            }

            HyuskEvent::ScheduledWorkflow { id, name } => {
                println!("[Scheduler #{id}] Workflow due: {name}");
                crate::status::card(
                    "scheduled_workflow",
                    format!("Scheduled workflow ready: {name}"),
                    false,
                );
                let text = format!("run {name}");
                if active.is_some() {
                    scheduled_inputs.push_back(text);
                } else {
                    let _ = agent_tx.send(HyuskEvent::UserInput(text)).await;
                }
            }

            HyuskEvent::ScheduledAgentTask { id, prompt } => {
                println!("[Scheduler #{id}] Agent task due");
                crate::status::card("scheduled_task", "Scheduled agent task is starting", false);
                if active.is_some() {
                    scheduled_inputs.push_back(prompt);
                } else {
                    let _ = agent_tx.send(HyuskEvent::UserInput(prompt)).await;
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

            HyuskEvent::StopRequested => {
                println!("[Agent] Emergency stop requested");
                interrupt_active(&mut active).await;
                interrupt_listen(&mut listen).await;
                send_state(&ui_tx, HyuskState::Hidden).await;
            }

            HyuskEvent::StopListening => {
                interrupt_listen(&mut listen).await;
                crate::status::awaiting_reply(false);
                send_state(&ui_tx, HyuskState::Hidden).await;
            }

            HyuskEvent::ModelSelected { provider, model } => {
                interrupt_active(&mut active).await;
                let result = agent.lock().await.select_model(&provider, &model);
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
            }

            HyuskEvent::Shutdown => {
                println!("[Agent] Shutdown requested");
                interrupt_active(&mut active).await;
                interrupt_listen(&mut listen).await;
                break;
            }

            _ => {}
        }
    }

    Ok(())
}

async fn announce_completed_tasks(
    completed_tasks: &mut VecDeque<(String, bool)>,
    ui_tx: &Sender<HyuskEvent>,
    tts: &TextToSpeech,
) {
    while let Some((label, success)) = completed_tasks.pop_front() {
        let announcement = format!(
            "Background task {label} {}. Its report is ready.",
            if success { "completed" } else { "failed" }
        );
        let _ = ui_tx.try_send(HyuskEvent::Response(announcement.clone()));
        crate::status::card("task", &announcement, true);
        let tts = tts.clone();
        tokio::spawn(async move {
            let cancel = CancellationToken::new();
            let _ = tts.speak_cancellable(&announcement, &cancel).await;
        });
    }
}

/// Record a short command from the microphone and forward it as user input.
///
/// This runs as a dedicated task so a slow or failed recording never blocks
/// the event loop: typed input, another wake word, and shutdown all keep
/// working. The wake detector is resumed before this task even starts.
async fn handle_listen(
    stt: Arc<SpeechToText>,
    agent_tx: Sender<HyuskEvent>,
    ui_tx: Sender<HyuskEvent>,
    wake_resume: WakeResume,
    cancel: CancellationToken,
    resume_media: Arc<AtomicBool>,
) {
    // Always hand the microphone back to the wake detector, even when this
    // task is cancelled mid-recording.
    let _resume_guard = ResumeOnDrop(wake_resume);

    let stt_start = std::time::Instant::now();

    // A generous cap; recording still stops shortly after the speaker pauses,
    // so this only bounds unusually long requests.
    let recording = stt.transcribe_from_microphone(8.0);

    let transcription = match tokio::time::timeout(Duration::from_secs(45), async {
        tokio::select! {
            _ = cancel.cancelled() => None,
            result = recording => Some(result),
        }
    })
    .await
    {
        Ok(Some(Ok(text))) => Some(text),
        Ok(Some(Err(error))) => {
            eprintln!("[STT] {error}");
            None
        }
        Ok(None) => {
            println!("[STT] Listening replaced by a newer request");
            None
        }
        Err(_) => {
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
            crate::status::awaiting_reply(false);
            let _ = agent_tx.send(HyuskEvent::UserInput(text)).await;
        }

        _ => {
            crate::status::awaiting_reply(false);
            // Nothing usable was heard: hand media back immediately.
            if resume_media.swap(false, Ordering::Relaxed) {
                media::resume().await;
            }

            send_state(&ui_tx, HyuskState::Hidden).await;
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
) {
    // Keep the detector muted until this turn (including TTS) is done.
    let _output_guard = OutputOnDrop(output_active);
    let _media_guard = MediaOnDrop(resume_media);

    println!("[User] {text}");

    send_state(&ui_tx, HyuskState::Thinking).await;

    let turn_start = std::time::Instant::now();

    /*
     * TTS worker. It speaks reply sentences as the model streams them, so the
     * first words start before the full answer is generated.
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

        if let Some(fast) = agent.try_fast_command(&text, &cancel).await {
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

    let (requires_approval, model_needs_reply) = {
        let agent = agent.lock().await;
        (agent.awaiting_approval(), agent.reply_needs_follow_up())
    };
    crate::status::card(
        if requires_approval {
            "approval"
        } else {
            "response"
        },
        &response,
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
    if !cancel.is_cancelled() && needs_reply {
        crate::status::awaiting_reply(true);
        let _ = event_tx.send(HyuskEvent::ContinueListening).await;
    } else {
        crate::status::awaiting_reply(false);
    }

    if !cancel.is_cancelled() {
        send_state(&ui_tx, HyuskState::Hidden).await;
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
    use super::response_needs_reply;

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
}
