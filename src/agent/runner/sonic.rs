//! Nova Sonic conversation and audio lifecycle, separate from text-model turns.

use super::*;
use crate::{model::bedrock_sonic::SonicEvent, speech::SpeechToText};
use tokio::sync::mpsc;

/// Dropping a JoinHandle detaches its task. This guard instead stops the audio
/// producer on every error/early return as well as normal session shutdown.
struct AbortAudioOnDrop(tokio::task::AbortHandle);

impl Drop for AbortAudioOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Agent {
    pub async fn handle_sonic_live_audio(
        &mut self,
        stt: Arc<SpeechToText>,
        ui_tx: Option<&Sender<HyuskEvent>>,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        self.handle_sonic_turn(Some(stt), None, ui_tx, cancel).await
    }

    pub(super) async fn handle_sonic_text(
        &mut self,
        text: &str,
        ui_tx: Option<&Sender<HyuskEvent>>,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        self.handle_sonic_turn(None, Some(text), ui_tx, cancel)
            .await
    }

    async fn handle_sonic_turn(
        &mut self,
        live_stt: Option<Arc<SpeechToText>>,
        text_input: Option<&str>,
        ui_tx: Option<&Sender<HyuskEvent>>,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        let live_conversation = live_stt.is_some();
        let client = self
            .bedrock_sonic_client
            .clone()
            .context("Nova Sonic is not configured")?;
        if cancel.is_cancelled() {
            return Ok(None);
        }
        self.compact_history();
        self.task_paused = false;
        self.reply_needs_follow_up = None;
        let mut prompt = build_system_prompt(&self.tool_guide, text_input.unwrap_or(""), true);
        for message in self
            .messages
            .iter()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if message.role == "system" || message.text().trim().is_empty() {
                continue;
            }
            prompt.push_str(&format!(
                "\nRecent {}: {}",
                message.role,
                short(&message.text())
            ));
        }
        let awaiting_confirmation = self.pending_approval.is_some();
        if awaiting_confirmation {
            prompt.push_str("\nThe user is responding to a pending computer-action confirmation. Do not initiate any new action. Wait for Hyusk to send the verified outcome.");
            if let Some(pending) = self.pending_approval.as_ref() {
                prompt.push_str(&format!(
                    " Pending action: {} with arguments {}.",
                    pending.name, pending.arguments
                ));
            }
        } else {
            prompt.push_str("\nThere is no pending approval for this turn. Older assistant requests for confirmation in the conversation are not active approvals. Opening apps, websites, or music at the user's request is already authorized; act on the request without asking again. Only ask for confirmation when a tool result explicitly requires it or before a destructive action.");
        }
        // Start the microphone before the AWS handshake. Otherwise a slow
        // connection silently drops the command spoken right after wake-up.
        // Eight seconds of 32 ms PCM frames fit in this bounded queue.
        let mut capture_task = None;
        let mut frames_rx = None;
        let (speech_start_tx, mut speech_start_rx) = mpsc::channel(4);
        let capture_cancel = cancel.child_token();
        let _capture_guard = capture_cancel.clone().drop_guard();
        if let Some(stt) = live_stt {
            let (frames_tx, receiver) = mpsc::channel(256);
            frames_rx = Some(receiver);
            let worker_cancel = capture_cancel.clone();
            capture_task = Some(tokio::task::spawn_blocking(move || {
                stt.stream_continuous_audio_for_speech_model(
                    frames_tx,
                    speech_start_tx,
                    &worker_cancel,
                )
            }));
        }
        // Capture is already running so a desktop query cannot lose speech
        // spoken immediately after the wake word.
        append_desktop_context(&mut prompt).await;
        let mut session = match client.start_session(&prompt, &self.tool_specs).await {
            Ok(session) => session,
            Err(error) => {
                capture_cancel.cancel();
                drop(frames_rx);
                return Err(error);
            }
        };
        let mut audio_send = None;
        if let Some(text) = text_input {
            session.send_text(text).await?;
        } else if let Some(receiver) = frames_rx {
            audio_send = Some(session.send_persistent_live_audio_in_background(receiver)?);
        } else {
            anyhow::bail!("Nova Sonic turn needs text or microphone input");
        }
        let _audio_guard = audio_send
            .as_ref()
            .map(|task| AbortAudioOnDrop(task.abort_handle()));

        let mut user_text = text_input.unwrap_or_default().to_string();
        let mut user_message_saved = false;
        if let Some(text) = text_input {
            self.messages.push(Message::user(text.to_string()));
            user_message_saved = true;
        }
        let mut assistant_text = String::new();
        #[cfg(target_os = "linux")]
        let mut audio_player: Option<crate::speech::speech::Pcm16Player> = None;
        #[cfg(not(target_os = "linux"))]
        let mut audio_chunks = Vec::new();
        let mut completed_actions = Vec::new();
        let mut rounds = 0usize;
        let mut confirmation_resolved = !awaiting_confirmation;
        let mut tool_history = String::new();
        let mut reply_deadline: Option<Instant> = None;
        let mut idle_deadline = live_conversation.then(|| Instant::now() + Duration::from_secs(30));
        let mut stream_stalled = false;
        let mut stalled_by_repetition = false;
        let mut sonic_round_limit_reached = false;
        let mut sonic_progress = ToolProgress::default();
        let mut discard_interrupted_audio = false;
        let mut turn_started = Instant::now();

        loop {
            if cancel.is_cancelled() {
                if let Some(task) = audio_send.take() {
                    task.abort();
                }
                self.task_paused = true;
                self.persist_session();
                return Ok(None);
            }
            if self.loop_limits.remaining(turn_started).is_none() {
                assistant_text
                    .push_str(" I paused the task at its time limit and kept the completed work.");
                break;
            }
            let event = tokio::select! {
                _ = cancel.cancelled() => {
                    if let Some(task) = audio_send.take() {
                        task.abort();
                    }
                    self.task_paused = true;
                    self.persist_session();
                    return Ok(None);
                }
                captured = async {
                    if let Some(task) = capture_task.as_mut() {
                        Some(task.await)
                    } else {
                        std::future::pending().await
                    }
                } => {
                    capture_task.take();
                    match captured.expect("capture branch requires a task") {
                        Ok(Ok(())) => break,
                        Ok(Err(error)) => {
                            if let Some(task) = audio_send.take() {
                                task.abort();
                            }
                            return Err(error);
                        }
                        Err(error) => {
                            if let Some(task) = audio_send.take() {
                                task.abort();
                            }
                            return Err(error.into());
                        }
                    }
                }
                sent = async {
                    if let Some(task) = audio_send.as_mut() {
                        Some(task.await)
                    } else {
                        std::future::pending().await
                    }
                } => {
                    audio_send.take();
                    sent.expect("audio sender branch requires a task")??;
                    if !live_conversation {
                        reply_deadline = Some(Instant::now() + Duration::from_secs(20));
                    }
                    continue;
                }
                _ = async {
                    if let Some(deadline) = idle_deadline {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => break,
                Some(()) = speech_start_rx.recv(), if live_conversation => {
                    #[cfg(target_os = "linux")]
                    if let Some(mut player) = audio_player.take() {
                        discard_interrupted_audio = true;
                        player.stop().await?;
                    }
                    continue;
                }
                result = async {
                    if let Some(deadline) = reply_deadline {
                        tokio::time::timeout_at(
                            tokio::time::Instant::from_std(deadline),
                            session.recv(),
                        ).await.ok()
                    } else {
                        Some(session.recv().await)
                    }
                } => match result {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => {
                        if let Some(task) = audio_send.take() {
                            task.abort();
                        }
                        if user_text.trim().is_empty() {
                            return Err(error);
                        }
                        eprintln!("[Nova Sonic] Response ended after recognizing speech: {error:#}");
                        stream_stalled = true;
                        break;
                    }
                    None => {
                        eprintln!("[Nova Sonic] No response progress for 20 seconds after audio or tool result");
                        stream_stalled = true;
                        break;
                    }
                },
            };
            match event {
                SonicEvent::UserText(text) => {
                    if live_conversation {
                        idle_deadline = None;
                        reply_deadline = Some(Instant::now() + Duration::from_secs(20));
                        #[cfg(target_os = "linux")]
                        if let Some(mut player) = audio_player.take() {
                            discard_interrupted_audio = true;
                            player.stop().await?;
                        }
                    }
                    if text_input.is_none() {
                        user_text.push_str(&text);
                    }
                    if live_conversation && is_voice_session_stop(&user_text) {
                        capture_cancel.cancel();
                        assistant_text.clear();
                        user_text.clear();
                        break;
                    }
                }
                SonicEvent::UserTranscriptEnd => {
                    if self.pending_approval.is_some() && !confirmation_resolved {
                        let Some(pending) = self.pending_approval.take() else {
                            confirmation_resolved = true;
                            continue;
                        };
                        let decision =
                            if confirms(&user_text) && Instant::now() <= pending.expires_at {
                                "confirm"
                            } else if is_denial(&user_text) || Instant::now() > pending.expires_at {
                                "deny"
                            } else {
                                "unclear"
                            };
                        if decision == "confirm" {
                            self.action_journal.push(ActionJournalEntry {
                                tool: pending.name.clone(),
                                arguments: pending.arguments.clone(),
                                status: "in_flight; verify before retry after interruption".into(),
                            });
                            let (result, timed_out) = execute_tool_with_deadline(
                                &self.tools,
                                &pending.name,
                                &pending.arguments,
                                cancel,
                                self.loop_limits
                                    .remaining(turn_started)
                                    .unwrap_or_default()
                                    .min(Duration::from_secs(40)),
                            )
                            .await?;
                            if let Some(entry) = self.action_journal.last_mut() {
                                entry.status = if timed_out || cancel.is_cancelled() {
                                    "in_flight; outcome uncertain"
                                } else if result.success {
                                    "completed"
                                } else {
                                    "failed"
                                }
                                .into();
                            }
                            tool_history.push_str(&format!(
                                "User confirmed {}. Result: {}. ",
                                pending.name,
                                result.as_agent_message()
                            ));
                        } else if decision == "deny" {
                            tool_history.push_str(&format!(
                                "User declined {}. Do not execute it. ",
                                pending.name
                            ));
                        } else {
                            self.pending_approval = Some(pending);
                            tool_history.push_str(
                                "The confirmation was unclear; ask for a clear yes or no. ",
                            );
                        }
                        confirmation_resolved = true;
                        session.send_text(&tool_history).await?;
                    }
                }
                SonicEvent::AssistantText(text) => {
                    assistant_text.push_str(&text);
                    reply_deadline = Some(Instant::now() + Duration::from_secs(20));
                }
                SonicEvent::AssistantAudio(chunk) => {
                    reply_deadline = Some(Instant::now() + Duration::from_secs(20));
                    if discard_interrupted_audio {
                        continue;
                    }
                    #[cfg(target_os = "linux")]
                    {
                        if audio_player.is_none() {
                            audio_player = Some(crate::speech::speech::Pcm16Player::start()?);
                        }
                        if let Some(player) = audio_player.as_mut() {
                            player.write(&chunk).await?;
                        }
                    }
                    #[cfg(not(target_os = "linux"))]
                    audio_chunks.push(chunk);
                }
                SonicEvent::ToolUse(call) => {
                    if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
                        eprintln!("[Nova Sonic] tool requested: {}", call.name);
                    }
                    if !live_conversation {
                        if let Some(task) = audio_send.take() {
                            task.await??;
                        }
                    }
                    rounds += 1;
                    if rounds > self.loop_limits.max_rounds {
                        sonic_round_limit_reached = true;
                        break;
                    }
                    publish_round(ui_tx, rounds);
                    if let Some(tx) = ui_tx {
                        let _ = tx.try_send(HyuskEvent::ToolStarted {
                            name: call.name.clone(),
                        });
                    }
                    let args =
                        serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".into());
                    let signature = tool_call_signature(&call.name, &call.arguments, &args);
                    let result = if let Some(error) = &call.validation_error {
                        ToolResult::failure(format!(
                            "Nova Sonic tool request was rejected: {error}"
                        ))
                    } else if self.pending_approval.is_some() {
                        ToolResult::failure(
                            "Only a confirmation response was expected; no new action was started.",
                        )
                    } else if self.tools.get(&call.name).is_none() {
                        // Unknown or unavailable tools never arm a confirmation.
                        ToolResult::failure(format!("Tool '{}' does not exist.", call.name))
                    } else if crate::tools::safety::requires_approval(&call.name, &args) {
                        self.pending_approval = Some(PendingApproval {
                            name: call.name.clone(),
                            arguments: crate::tools::safety::approval_arguments(&call.name, &args),
                            call_id: call.id.clone(),
                            result_index: self.messages.len(),
                            expires_at: Instant::now() + Duration::from_secs(300),
                        });
                        confirmation_resolved = false;
                        crate::status::awaiting_reply(true);
                        crate::status::card(
                            "confirmation",
                            format!("Confirm action: {}", call.name),
                            true,
                        );
                        ToolResult::failure(
                            "This action requires user confirmation. Ask the user to clearly confirm or decline; do not execute it yet.",
                        )
                    } else {
                        let remaining =
                            self.loop_limits.remaining(turn_started).unwrap_or_default();
                        self.action_journal.push(ActionJournalEntry {
                            tool: call.name.clone(),
                            arguments: args.clone(),
                            status: "in_flight; verify before retry after interruption".into(),
                        });
                        self.persist_session();
                        let (result, timed_out) = execute_tool_with_deadline(
                            &self.tools,
                            &call.name,
                            &args,
                            cancel,
                            remaining.min(Duration::from_secs(40)),
                        )
                        .await?;
                        if let Some(entry) = self.action_journal.last_mut() {
                            entry.status = if timed_out || cancel.is_cancelled() {
                                "in_flight; outcome uncertain"
                            } else if result.success {
                                "completed"
                            } else {
                                "failed"
                            }
                            .into();
                        }
                        if result.success {
                            completed_actions.push(format!(
                                "{} ({})",
                                call.name,
                                short(&result.output)
                            ));
                        }
                        result
                    };
                    if let Some(tx) = ui_tx {
                        let _ = tx.try_send(HyuskEvent::ToolFinished {
                            name: call.name.clone(),
                            success: result.success,
                        });
                    }
                    let outcome = result.as_agent_message();
                    if std::env::var_os("HYUSK_SONIC_DEBUG").is_some() {
                        eprintln!(
                            "[Nova Sonic] tool finished: {} success={} result_bytes={}",
                            call.name,
                            result.success,
                            outcome.len()
                        );
                    }
                    self.messages
                        .push(Message::tool_result(call.id.clone(), outcome.clone()));
                    self.persist_session();
                    session
                        .send_tool_result(&call, &outcome, result.success)
                        .await?;
                    reply_deadline = Some(Instant::now() + Duration::from_secs(20));
                    if sonic_progress.observe(vec![(signature, outcome)])
                        >= self.loop_limits.max_repeated_rounds
                    {
                        assistant_text.push_str(
                            " I paused because the same action returned the same result repeatedly. Please inspect the current screen before continuing.",
                        );
                        stalled_by_repetition = true;
                        break;
                    }
                }
                SonicEvent::CompletionEnd(reason) => {
                    if reason == "END_TURN" || reason == "INTERRUPTED" {
                        if !live_conversation {
                            break;
                        }
                        if !user_text.trim().is_empty() {
                            if !user_message_saved {
                                self.messages
                                    .push(Message::user(user_text.trim().to_string()));
                            }
                            let response = if assistant_text.trim().is_empty() {
                                if self.pending_approval.is_some() {
                                    "This action needs your confirmation before I continue."
                                        .to_string()
                                } else if completed_actions.is_empty() {
                                    "I couldn't complete that request.".to_string()
                                } else {
                                    format!("Done. {}", completed_actions.join("; "))
                                }
                            } else {
                                assistant_text.trim().to_string()
                            };
                            self.messages.push(Message::assistant(response.clone()));
                            self.reply_needs_follow_up = Some(self.pending_approval.is_some());
                            self.persist_session();
                            crate::status::card(
                                if self.pending_approval.is_some() {
                                    "confirmation"
                                } else {
                                    "response"
                                },
                                &response,
                                self.pending_approval.is_some(),
                            );
                            if let Some(tx) = ui_tx {
                                let _ = tx.try_send(HyuskEvent::Response(response));
                                let _ =
                                    tx.try_send(HyuskEvent::StateChanged(HyuskState::Listening));
                            }
                        }
                        user_text.clear();
                        user_message_saved = false;
                        assistant_text.clear();
                        completed_actions.clear();
                        tool_history.clear();
                        rounds = 0;
                        sonic_progress = ToolProgress::default();
                        discard_interrupted_audio = false;
                        confirmation_resolved = self.pending_approval.is_none();
                        turn_started = Instant::now();
                        reply_deadline = None;
                        idle_deadline = Some(Instant::now() + Duration::from_secs(30));
                    }
                }
                SonicEvent::Interrupted => {
                    discard_interrupted_audio = false;
                    #[cfg(target_os = "linux")]
                    if let Some(mut player) = audio_player.take() {
                        player.stop().await?;
                    }
                }
                SonicEvent::StreamEnd => {
                    stream_stalled = !user_text.trim().is_empty();
                    break;
                }
                SonicEvent::Other => {}
            }
        }
        if live_conversation {
            capture_cancel.cancel();
        }
        if let Some(task) = capture_task {
            task.await??;
        }
        if let Some(task) = audio_send {
            task.await??;
        }
        if stream_stalled {
            let _ = session.close().await;
        } else {
            session.close().await?;
        }
        stream_stalled |= sonic_round_limit_reached;
        if user_text.trim().is_empty() {
            return Ok(None);
        }
        if !user_message_saved {
            self.messages.push(Message::user(user_text.clone()));
        }
        if assistant_text.trim().is_empty() {
            assistant_text = if self.pending_approval.is_some() {
                "This action needs your confirmation before I continue.".into()
            } else if sonic_round_limit_reached {
                format!(
                    "I paused after reaching the {}-action safety limit. The task is not confirmed complete; say continue and I’ll resume from the verified results.",
                    self.loop_limits.max_rounds
                )
            } else if completed_actions.is_empty() {
                if stream_stalled {
                    "I heard your request, but Nova Sonic stopped before answering. Please try again.".into()
                } else {
                    "I couldn't complete that request.".into()
                }
            } else if stream_stalled {
                format!(
                    "I completed {}, but Nova Sonic stopped before I could confirm the rest of the task. Please ask me to continue.",
                    completed_actions.join("; ")
                )
            } else {
                format!("Done. {}", completed_actions.join("; "))
            };
        } else if sonic_round_limit_reached {
            assistant_text.push_str(
                " I paused at the action limit; the task is not confirmed complete. Say continue and I’ll resume from the verified results.",
            );
        } else if stream_stalled {
            assistant_text
                .push_str(" The response stopped early; I may not have finished the task.");
        }
        self.messages
            .push(Message::assistant(assistant_text.clone()));
        self.reply_needs_follow_up = Some(
            self.pending_approval.is_some() || sonic_round_limit_reached || stalled_by_repetition,
        );
        self.task_paused = stream_stalled || stalled_by_repetition;
        self.persist_session();
        #[cfg(target_os = "linux")]
        if let Some(player) = audio_player {
            player.finish().await?;
        } else if let Some(tx) = ui_tx {
            let _ = tx.try_send(HyuskEvent::Response(assistant_text.clone()));
        }
        #[cfg(not(target_os = "linux"))]
        if audio_chunks.is_empty() {
            if let Some(tx) = ui_tx {
                let _ = tx.try_send(HyuskEvent::Response(assistant_text.clone()));
            }
        } else {
            TextToSpeech::new()
                .speak_cancellable(&assistant_text, cancel)
                .await?;
        }
        Ok(Some(assistant_text))
    }
}

#[cfg(test)]
mod tests {
    use super::AbortAudioOnDrop;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn early_session_exit_cancels_capture_and_aborts_audio() {
        let lifecycle = CancellationToken::new();
        let capture = lifecycle.child_token();
        let task = tokio::spawn(std::future::pending::<()>());
        {
            let _capture_guard = capture.clone().drop_guard();
            let _audio_guard = AbortAudioOnDrop(task.abort_handle());
        }
        assert!(capture.is_cancelled());
        assert!(!lifecycle.is_cancelled());
        assert!(task.await.unwrap_err().is_cancelled());
    }
}
