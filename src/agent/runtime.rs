use std::{
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
    types::{HyuskEvent, HyuskState},
    wake::detector::WakeResume,
};

struct ActiveTurn {
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

    while let Some(event) = event_rx.recv().await {
        match event {
            HyuskEvent::WakeWordDetected => {
                println!("[Wake] Wake word detected");

                /*
                 * A new voice request replaces any in-flight model, tool,
                 * TTS work, and any listening session that was already in
                 * progress (for example a double wake or a clap during the
                 * recording window).
                 */
                interrupt_active(&mut active).await;
                interrupt_listen(&mut listen).await;

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

                let handle = tokio::spawn(async move {
                    handle_listen(stt, task_agent_tx, task_ui_tx, task_resume, task_cancel).await;
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

                let handle = tokio::spawn(async move {
                    run_turn(
                        turn_agent,
                        text,
                        turn_ui,
                        turn_tts,
                        turn_cancel,
                        turn_output,
                    )
                    .await;
                });

                active = Some(ActiveTurn { cancel, handle });
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
) {
    // Always hand the microphone back to the wake detector, even when this
    // task is cancelled mid-recording.
    let _resume_guard = ResumeOnDrop(wake_resume);

    let stt_start = std::time::Instant::now();

    // A short cap keeps commands snappy; recording still stops as soon as the
    // speaker pauses, so this is only a safety net for very long requests.
    let recording = stt.transcribe_from_microphone(4.0);

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
            let _ = agent_tx.send(HyuskEvent::UserInput(text)).await;
        }

        _ => {
            send_state(&ui_tx, HyuskState::Hidden).await;
        }
    }
}

async fn run_turn(
    agent: Arc<Mutex<Agent>>,
    text: String,
    ui_tx: Sender<HyuskEvent>,
    tts: TextToSpeech,
    cancel: CancellationToken,
    output_active: Arc<AtomicBool>,
) {
    // Keep the detector muted until this turn (including TTS) is done.
    let _output_guard = OutputOnDrop(output_active);

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

    let _ = ui_tx.try_send(HyuskEvent::Response(response));

    // Close the sink so the worker drains and finishes.
    drop(sentence_tx);

    let tts_start = std::time::Instant::now();
    let _ = tts_worker.await;
    crate::timing::mark("tts", tts_start);

    if !cancel.is_cancelled() {
        send_state(&ui_tx, HyuskState::Hidden).await;
    }
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
