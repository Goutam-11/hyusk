use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

pub(crate) use super::control::PendingApproval;
use super::control::{self as control_state, AgentControl, ControlHandle};
use crate::{
    model::{bedrock_sonic::BedrockSonicClient, openrouter::OpenRouterClient},
    model::{codex::CodexClient, SharedActiveModel},
    tools::{ToolRegistry, ToolResult},
    types::{HyuskEvent, HyuskState, Message},
};

#[path = "runner/prompts.rs"]
mod prompts;
#[path = "runner/reply.rs"]
mod reply;
#[path = "runner/session.rs"]
mod session;
#[path = "runner/sonic.rs"]
mod sonic;
#[path = "runner/stream.rs"]
mod stream;
use prompts::{append_desktop_context, build_system_prompt, build_tool_guide, build_tool_specs};
pub(crate) use reply::is_voice_session_stop;
pub(crate) use reply::ALLOW_FOR_THIS_TURN;
use reply::{
    confirms, confirms_turn, denial_instruction, is_denial, parse_structured_reply,
    tool_call_preamble,
};
use session::{load_session, save_session, ActionJournalEntry};
use stream::ReplyStream;

/// Spoken once when an unrelated request drops an armed confirmation.
const DROPPED_APPROVAL_NOTE: &str =
    "I dropped that pending action because you asked something else.";

pub struct Agent {
    /// Approval, model-selection, and paused-task state. It is reachable from
    /// the event loop while a turn holds this struct's mutex, so an emergency
    /// stop and a key save never queue behind the model.
    control: ControlHandle,
    tools: ToolRegistry,
    tool_specs: Vec<Value>,
    tool_guide: String,
    messages: Vec<Message>,
    vision: bool,
    /// Tool and shape pairs the user allowed for the rest of the current turn.
    /// Cleared at turn start and turn end, and never written to a session.
    turn_grants: Vec<(String, String)>,
    action_journal: Vec<ActionJournalEntry>,
    loaded_session_identity: Option<(String, String)>,
    reply_needs_follow_up: Option<bool>,
    loop_limits: AgentLoopLimits,
}

/// Generous per-turn limits keep useful multi-step tasks running while
/// guaranteeing that a model cannot loop forever on the same tool call.
#[derive(Debug, Clone, Copy)]
struct AgentLoopLimits {
    max_rounds: usize,
    max_duration: Duration,
    max_repeated_rounds: usize,
}

/// One assembled model message, plus whether its speakable text was already
/// delivered while it streamed.
struct ModelReply {
    message: Message,
    streamed: bool,
}

/// Forwards streamed reply text to speech and the UI as sentences complete.
struct ReplySink<'a> {
    stream: ReplyStream,
    sentences: Option<&'a tokio::sync::mpsc::UnboundedSender<String>>,
    ui_tx: Option<&'a Sender<HyuskEvent>>,
}

impl<'a> ReplySink<'a> {
    fn new(
        sentences: Option<&'a tokio::sync::mpsc::UnboundedSender<String>>,
        ui_tx: Option<&'a Sender<HyuskEvent>>,
    ) -> Self {
        Self {
            stream: ReplyStream::default(),
            sentences,
            ui_tx,
        }
    }

    fn on_delta(&mut self, delta: &str) {
        let mut chunks = Vec::new();
        self.stream
            .push(delta, &mut |chunk| chunks.push(chunk.to_string()));
        for chunk in chunks {
            self.emit(&chunk);
        }
    }

    fn finish(&mut self, final_text: &str) {
        let mut chunks = Vec::new();
        self.stream
            .finish(final_text, &mut |chunk| chunks.push(chunk.to_string()));
        for chunk in chunks {
            self.emit(&chunk);
        }
    }

    fn emit(&mut self, chunk: &str) {
        if let Some(sink) = self.sentences {
            let _ = sink.send(chunk.to_string());
        }
        if let Some(tx) = self.ui_tx {
            let _ = tx.try_send(HyuskEvent::ResponseDelta(chunk.to_string()));
        }
        // Mirror the reply-so-far into the status card so the indicator shows
        // progress instead of a static "Working" note.
        crate::status::card("response", self.stream.spoken(), false);
    }

    fn delivered(&self) -> bool {
        self.stream.delivered()
    }
}

impl AgentLoopLimits {
    fn from_env() -> Self {
        Self {
            // These are safety ceilings, not the normal definition of a
            // task. A continuing task remains in the conversation so a
            // simple "continue" resumes from the observed tool results.
            max_rounds: positive_env("HYUSK_AGENT_MAX_TOOL_ROUNDS", 128),
            max_duration: Duration::from_secs(positive_env("HYUSK_AGENT_MAX_TURN_SECS", 1_800)),
            max_repeated_rounds: positive_env("HYUSK_AGENT_MAX_REPEATED_TOOL_ROUNDS", 3),
        }
    }

    fn remaining(self, started: Instant) -> Option<Duration> {
        self.max_duration.checked_sub(started.elapsed())
    }
}

fn positive_env<T>(name: &str, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + From<u8>,
{
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value >= T::from(1u8))
        .unwrap_or(default)
}

#[derive(Debug, Default)]
struct ToolProgress {
    previous_round: Option<Vec<(String, String)>>,
    repeated_rounds: usize,
}

impl ToolProgress {
    /// A round is stuck only when its calls and outputs are unchanged. A
    /// changing output is evidence that polling or another iterative task is
    /// making progress.
    fn observe(&mut self, round: Vec<(String, String)>) -> usize {
        if self.previous_round.as_ref() == Some(&round) {
            self.repeated_rounds = self.repeated_rounds.saturating_add(1);
        } else {
            self.repeated_rounds = 1;
        }
        self.previous_round = Some(round);
        self.repeated_rounds
    }
}

impl Agent {
    pub fn workflow_tools(&self) -> ToolRegistry {
        self.tools.clone()
    }

    /// The shared control handle.
    ///
    /// The runtime takes it before it locks the agent, so control events never
    /// wait for a turn. See [`Agent::control`] for the guard rules.
    pub fn control_handle(&self) -> ControlHandle {
        Arc::clone(&self.control)
    }

    /// Short-lived control access.
    ///
    /// Never hold the guard across an `.await` and never take it while waiting
    /// for the agent mutex: the agent lock may be held while this one is taken
    /// (the turn reads its own state), not the other way round.
    fn control(&self) -> MutexGuard<'_, AgentControl> {
        control_state::lock(&self.control)
    }

    pub fn new(client: OpenRouterClient, model: String, tools: ToolRegistry) -> Self {
        let tool_specs = build_tool_specs(&tools);
        let tool_guide = build_tool_guide(&tools);
        let system_prompt = build_system_prompt(&tool_guide, "", false);
        let mut messages = vec![Message::system(system_prompt)];
        let (saved_messages, action_journal, task_paused, loaded_session_identity) = load_session();
        messages.extend(saved_messages);
        let vision = configured_vision();
        let control = control_state::new_control("openrouter", model, Some(client), task_paused);
        control_state::lock(&control).set_codex_client(CodexClient::discover().ok());

        Self {
            control,
            tools,
            tool_specs,
            tool_guide,
            messages,
            vision,
            turn_grants: Vec::new(),
            action_journal,
            loaded_session_identity,
            reply_needs_follow_up: Some(false),
            loop_limits: AgentLoopLimits::from_env(),
        }
    }

    pub fn with_openai(self, client: Option<OpenRouterClient>) -> Self {
        self.control().set_provider_client("openai", client);
        self
    }

    pub fn with_openrouter(self, client: Option<OpenRouterClient>) -> Self {
        self.control().set_provider_client("openrouter", client);
        self
    }

    pub fn with_bedrock(self, client: Option<OpenRouterClient>) -> Self {
        self.control().set_provider_client("bedrock", client);
        self
    }

    pub fn with_bedrock_sonic(self, client: Option<std::sync::Arc<BedrockSonicClient>>) -> Self {
        self.control().set_sonic_client(client);
        self
    }

    pub fn uses_bedrock_sonic(&self) -> bool {
        self.control().uses_sonic()
    }

    pub fn with_active_api_model(self, active: SharedActiveModel) -> Self {
        self.control().set_active_api_model(active);
        self
    }

    /// Approval accessors are test-only now: the runtime reads the control
    /// handle directly and never needs the agent mutex for them.
    #[cfg(test)]
    pub(crate) fn awaiting_approval(&self) -> bool {
        self.control().armed()
    }

    /// Allow later calls with the same tool shape for the rest of this turn.
    ///
    /// The card names what was granted, and the grant is dropped when the turn
    /// ends. A call whose shape cannot be determined is never granted.
    fn grant_for_turn(&mut self, name: &str, arguments: &str) {
        let Some((tool, shape)) = crate::tools::safety::grant_key(name, arguments) else {
            return;
        };
        if !self
            .turn_grants
            .iter()
            .any(|(granted_tool, granted_shape)| granted_tool == &tool && granted_shape == &shape)
        {
            self.turn_grants.push((tool, shape));
        }
        crate::status::card(
            "response",
            format!(
                "Allowed for this turn: {}",
                crate::tools::safety::preview(name, arguments)
            ),
            false,
        );
    }

    /// True when a turn-scoped grant covers this call.
    fn turn_granted(&self, name: &str, arguments: &str) -> bool {
        let Some((tool, shape)) = crate::tools::safety::grant_key(name, arguments) else {
            return false;
        };
        self.turn_grants
            .iter()
            .any(|(granted_tool, granted_shape)| granted_tool == &tool && granted_shape == &shape)
    }

    /// The exact call in the armed confirmation, as the card shows it.
    pub fn pending_approval_preview(&self) -> Option<String> {
        self.control().preview()
    }

    /// Disarm a stored confirmation without executing it.
    ///
    /// Used by an emergency stop: an aborted interaction must not leave a
    /// dangerous action armed for a later, unrelated confirmation.
    #[cfg(test)]
    pub(crate) fn clear_pending_approval(&mut self) -> bool {
        self.control().clear_pending_approval()
    }

    /// The deadline of the confirmation that is currently armed, if any.
    #[cfg(test)]
    pub(crate) fn approval_deadline(&self) -> Option<Instant> {
        self.control().deadline()
    }

    /// Disarm a confirmation whose deadline has passed.
    ///
    /// Returns the reply to show the user when it expired. The runtime also
    /// calls this from a timer, so an abandoned confirmation cannot stay armed
    /// for an unrelated "yes" or hold back queued scheduled input.
    pub fn expire_stale_approval(&mut self) -> Option<String> {
        // Take, test, and put back under one guard so the approval is never
        // momentarily visible as unarmed to the event loop.
        let pending = {
            let mut control = self.control();
            let pending = control.take_pending()?;
            if Instant::now() <= pending.expires_at {
                control.set_pending(Some(pending));
                return None;
            }
            pending
        };
        if let Some(message) = self.messages.get_mut(pending.result_index) {
            *message = Message::tool_result(
                pending.call_id,
                "Approval expired before the action was confirmed. The action was not executed.",
            );
        }
        self.reply_needs_follow_up = Some(false);
        self.persist_session();
        Some(
            "That confirmation expired, so I did not perform the action. Please ask again if you still want it."
                .to_string(),
        )
    }

    /// Arm a confirmation for lifecycle tests that do not call a model.
    #[cfg(test)]
    pub(crate) fn arm_test_approval(&mut self) {
        self.arm_test_approval_at(Instant::now() + Duration::from_secs(60));
    }

    #[cfg(test)]
    pub(crate) fn arm_test_approval_at(&mut self, expires_at: Instant) {
        let result_index = self.messages.len();
        self.control().set_pending(Some(PendingApproval {
            name: "shell".into(),
            arguments: "{\"command\":\"rm -rf build\"}".into(),
            preview: crate::tools::safety::preview("shell", "{\"command\":\"rm -rf build\"}"),
            call_id: "test-approval".into(),
            result_index,
            expires_at,
        }));
    }

    pub fn has_paused_task(&self) -> bool {
        self.control().task_paused()
    }

    pub fn reply_needs_follow_up(&self) -> Option<bool> {
        self.reply_needs_follow_up
    }

    pub fn select_model(&mut self, provider: &str, model: &str) -> Result<()> {
        self.set_model(provider, model, true)
    }

    pub fn restore_model(&mut self, provider: &str, model: &str) -> Result<()> {
        self.set_model(provider, model, false)
    }

    fn set_model(&mut self, provider: &str, model: &str, new_conversation: bool) -> Result<()> {
        if provider == "bedrock-sonic" {
            if self.control().sonic_client().is_none() {
                anyhow::bail!(
                    "Nova Sonic is unavailable: configure an AWS CLI profile/region and Bedrock access"
                );
            }
            self.control().set_identity(provider, model);
            self.control().set_pending(None);
            self.reply_needs_follow_up = Some(false);
            self.messages[0] = Message::system(build_system_prompt(&self.tool_guide, "", false));
            if new_conversation || self.session_belongs_to_another_model(provider, model) {
                self.reset_conversation();
            }
            self.loaded_session_identity = Some((provider.to_string(), model.to_string()));
            return Ok(());
        }
        let client = match provider {
            "openrouter" => self
                .control()
                .provider_client("openrouter")
                .context("OPENROUTER_API_KEY is not configured")?,
            "openai" => self
                .control()
                .provider_client("openai")
                .context("OPENAI_API_KEY is not configured")?,
            "bedrock" => self
                .control()
                .provider_client("bedrock")
                .context("Amazon Bedrock API key is not configured")?,
            "codex" => {
                if !self.control().has_codex_client() {
                    anyhow::bail!("Codex CLI or HYUSK_WORKSPACE is unavailable");
                }
                self.control().set_identity(provider, model);
                self.control().set_pending(None);
                self.messages[0] =
                    Message::system(build_system_prompt(&self.tool_guide, "", false));
                self.reply_needs_follow_up = Some(false);
                if new_conversation || self.session_belongs_to_another_model(provider, model) {
                    self.reset_conversation();
                }
                self.loaded_session_identity = Some((provider.to_string(), model.to_string()));
                return Ok(());
            }
            _ => anyhow::bail!("Provider '{provider}' is not available"),
        };
        self.control().select_api_model(provider, model, &client)?;
        self.control().set_text_client(Some(client));
        self.vision = configured_vision();
        self.control().set_identity(provider, model);
        self.messages[0] = Message::system(build_system_prompt(&self.tool_guide, "", false));
        self.control().set_pending(None);
        self.reply_needs_follow_up = Some(false);
        if new_conversation || self.session_belongs_to_another_model(provider, model) {
            self.reset_conversation();
        }
        self.loaded_session_identity = Some((provider.to_string(), model.to_string()));
        Ok(())
    }

    fn session_belongs_to_another_model(&self, provider: &str, model: &str) -> bool {
        self.loaded_session_identity
            .as_ref()
            .is_some_and(|(saved_provider, saved_model)| {
                saved_provider != provider || saved_model != model
            })
    }

    fn reset_conversation(&mut self) {
        self.messages.truncate(1);
        self.action_journal.clear();
        self.control().set_task_paused(false);
        self.persist_session();
    }

    fn persist_session(&self) {
        // Copy the control state out before the file write so the control lock
        // is never held across it.
        let (task_paused, provider, model) = {
            let control = self.control();
            (control.task_paused(), control.provider(), control.model())
        };
        save_session(
            &self.messages,
            &self.action_journal,
            task_paused,
            &provider,
            &model,
        );
    }

    /// Try to satisfy `user_input` with the local instant command router.
    ///
    /// Common desktop commands ("open Firefox", "go to YouTube", "next song")
    /// execute directly, without a model round trip. Returns `None` when the
    /// utterance is not a recognized command, a required tool is missing, or
    /// any step of the plan would ask for approval, so the caller falls back to
    /// the model for full reasoning or a confirmation (A9).
    pub async fn try_fast_command(
        &mut self,
        user_input: &str,
        cancel: &CancellationToken,
    ) -> Option<String> {
        let plan = crate::agent::commands::parse(user_input)?;

        // A deterministic command must not bypass an outstanding approval.
        // Otherwise a new fast-path request could run while the old dangerous
        // action remains armed for a later, potentially unrelated "confirm".
        if self.control().armed() {
            return None;
        }

        if plan
            .tools()
            .iter()
            .any(|name| self.tools.get(name).is_none())
        {
            return None;
        }

        // The instant lane obeys the same policy as the model and workflow
        // lanes. If any step would ask, nothing runs: the utterance goes to the
        // model path, where the runtime can hold the call and confirm it (A9).
        if plan.actions.iter().any(|action| {
            crate::tools::safety::requires_approval(action.tool_name(), &action.tool_input())
        }) {
            return None;
        }

        if cancel.is_cancelled() {
            self.control().set_task_paused(true);
            self.persist_session();
            return Some(String::new());
        }

        let start = std::time::Instant::now();

        let mut completed_actions = Vec::new();
        for action in &plan.actions {
            if cancel.is_cancelled() {
                self.control().set_task_paused(true);
                self.persist_session();
                // `None` means "not handled" to the caller and would fall
                // through to the model. An empty handled reply suppresses that
                // fallback for a command the user has already interrupted.
                return Some(String::new());
            }

            let input = action.tool_input();

            println!(
                "[Hyusk][fast] {} {}",
                action.tool_name(),
                journal_arguments(&input)
            );

            self.action_journal.push(ActionJournalEntry {
                tool: action.tool_name().into(),
                arguments: journal_arguments(&input),
                status: "in_flight; verify before retry after interruption".into(),
            });
            self.persist_session();
            let remaining = self.loop_limits.remaining(start).unwrap_or(Duration::ZERO);
            let execution = execute_tool_with_deadline(
                &self.tools,
                action.tool_name(),
                &input,
                cancel,
                remaining,
            )
            .await;
            let (result, timed_out) = match execution {
                Ok(execution) => execution,
                Err(error) => {
                    if let Some(entry) = self.action_journal.last_mut() {
                        entry.status = "failed".into();
                    }
                    self.persist_session();
                    eprintln!("[fast] {} error: {error}", action.tool_name());
                    return Some(format!("I couldn't do that: {error}"));
                }
            };
            if timed_out || cancel.is_cancelled() {
                if let Some(entry) = self.action_journal.last_mut() {
                    entry.status = "in_flight; outcome uncertain after interruption".into();
                }
                self.control().set_task_paused(true);
                self.persist_session();
                if cancel.is_cancelled() {
                    // Do not start remaining actions or let the caller send
                    // this command to the model after an explicit interrupt.
                    return Some(String::new());
                }
                let prior = if completed_actions.is_empty() {
                    String::new()
                } else {
                    format!(" I completed {} first.", completed_actions.join("; "))
                };
                let reply = format!("I paused that command at its time limit. The current action may have completed; please check before asking me to retry.{prior}");
                self.messages.push(Message::user(user_input.to_string()));
                self.messages.push(Message::assistant(reply.clone()));
                self.persist_session();
                return Some(reply);
            }

            if result.success {
                if let Some(entry) = self.action_journal.last_mut() {
                    entry.status = "completed".into();
                }
                self.persist_session();
                completed_actions.push(format!(
                    "{} ({})",
                    action.tool_name(),
                    short(&result.output)
                ));
            } else {
                let message = result
                    .error
                    .clone()
                    .unwrap_or_else(|| result.output.clone());

                eprintln!("[fast] {} failed: {}", action.tool_name(), message);

                if let Some(entry) = self.action_journal.last_mut() {
                    entry.status = "failed".into();
                }
                self.messages.push(Message::user(user_input.to_string()));
                self.messages.push(Message::assistant(format!(
                    "I couldn't do that: {}",
                    short(&message)
                )));
                self.persist_session();

                return Some(format!("I couldn't do that: {}", short(&message)));
            }
        }

        crate::timing::mark("fast command", start);

        self.messages.push(Message::user(user_input.to_string()));
        self.messages.push(Message::assistant(plan.spoken.clone()));
        self.persist_session();

        Some(plan.spoken)
    }

    /// The focused window, read only for calls whose decision depends on it
    /// (D1). An unreadable focus is [`FocusContext::unknown`], which asks.
    async fn focus_context(
        &self,
        name: &str,
        arguments: &str,
    ) -> crate::tools::safety::FocusContext {
        if !crate::tools::safety::requires_focus_context(name, arguments) {
            return crate::tools::safety::FocusContext::unknown();
        }
        crate::tools::window::focused_window()
            .await
            .map(|(app, title)| crate::tools::safety::FocusContext::new(app, title))
            .unwrap_or_default()
    }

    /// Perform one model request.
    ///
    /// When a speech sink is present the reply streams, and its speakable text
    /// is forwarded sentence by sentence as it arrives. The returned
    /// [`ModelReply::streamed`] flag tells the caller that the text was already
    /// delivered, so it must not be spoken or displayed twice.
    ///
    /// Returns `None` when cancelled. An explicit unsupported-stream or image
    /// rejection can fall back to a plain (non-streamed) request so a
    /// provider that rejects `stream: true` still works. That fallback is
    /// skipped once part of the reply was already spoken, because replaying
    /// the request would repeat it.
    async fn request_model(
        &mut self,
        cancel: &CancellationToken,
        sentences: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
        ui_tx: Option<&Sender<HyuskEvent>>,
    ) -> Option<Result<ModelReply>> {
        let model = self.control().model();
        if self.control().provider() == "codex" {
            let client = self
                .control()
                .codex_client()
                .expect("checked when selected");
            let result = tokio::select! {
                _ = cancel.cancelled() => return None,
                result = client.chat(&model, &self.messages) => result,
            };
            return Some(result.map(|message| ModelReply {
                message,
                streamed: false,
            }));
        }
        if sentences.is_none() {
            return self.request_plain_model(cancel).await.map(|result| {
                result.map(|message| ModelReply {
                    message,
                    streamed: false,
                })
            });
        }

        // Clone the client so the request does not borrow the control lock
        // while the stream is read.
        let Some(client) = self.control().text_client() else {
            return Some(Err(anyhow::anyhow!("No model client is selected")));
        };

        let mut sink = ReplySink::new(sentences, ui_tx);
        let streamed = {
            let mut on_delta = |delta: &str| sink.on_delta(delta);

            tokio::select! {
                _ = cancel.cancelled() => return None,
                result = client.chat_stream(
                    &model,
                    &self.messages,
                    &self.tool_specs,
                    &mut on_delta,
                ) => result,
            }
        };

        match streamed {
            Ok(message) => {
                sink.finish(&message.text());
                let streamed = sink.delivered();
                Some(Ok(ModelReply { message, streamed }))
            }

            Err(error) => {
                if sink.delivered() {
                    eprintln!(
                        "[Agent] Stream failed after part of the reply was spoken; not replaying it: {}",
                        first_line(&format!("{error:#}"))
                    );
                    return Some(Err(error));
                }
                if !self.disable_rejected_vision(&error)
                    && !crate::model::openrouter::rejects_streaming(&error)
                {
                    return Some(Err(error));
                }
                eprintln!(
                    "[Agent] Streaming unavailable ({}); using a plain request",
                    first_line(&format!("{error:#}"))
                );

                self.request_plain_model(cancel).await.map(|result| {
                    result.map(|message| ModelReply {
                        message,
                        streamed: false,
                    })
                })
            }
        }
    }

    fn disable_rejected_vision(&mut self, error: &anyhow::Error) -> bool {
        if !rejects_image_input(&format!("{error:#}")) || !strip_image_inputs(&mut self.messages) {
            return false;
        }
        self.vision = false;
        eprintln!(
            "[Agent] Selected endpoint rejects images; continuing with DOM/accessibility/OCR, not retrying the same image request"
        );
        true
    }

    async fn request_plain_model(&mut self, cancel: &CancellationToken) -> Option<Result<Message>> {
        let Some(client) = self.control().text_client() else {
            return Some(Err(anyhow::anyhow!("No model client is selected")));
        };
        let model = self.control().model();
        for attempt in 0..2 {
            let result = tokio::select! {
                _ = cancel.cancelled() => return None,
                result = client.chat(&model, &self.messages, &self.tool_specs) => result,
            };
            if attempt == 0
                && result
                    .as_ref()
                    .err()
                    .is_some_and(|error| self.disable_rejected_vision(error))
            {
                continue;
            }
            return Some(result);
        }
        unreachable!("bounded image fallback returns on its second attempt")
    }

    /// Run one turn for `user_input`.
    ///
    /// Cancelling `cancel` stops the turn. Completed actions and their results
    /// remain in the conversation so a later turn can inspect partial progress.
    ///
    /// Returns `Ok(None)` when the turn was cancelled.
    ///
    /// When `sentences` is provided, the parsed final reply is forwarded for
    /// speech after its structured envelope has been removed.
    pub async fn handle(
        &mut self,
        user_input: String,
        ui_tx: Option<&Sender<HyuskEvent>>,
        cancel: &CancellationToken,
        sentences: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<Option<String>> {
        // A grant lasts exactly one turn: drop the previous turn's before the
        // new input is interpreted, and again once this turn is over.
        self.turn_grants.clear();
        let outcome = self.run_turn(user_input, ui_tx, cancel, sentences).await;
        self.turn_grants.clear();
        outcome
    }

    async fn run_turn(
        &mut self,
        mut user_input: String,
        ui_tx: Option<&Sender<HyuskEvent>>,
        cancel: &CancellationToken,
        sentences: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<Option<String>> {
        if self.uses_bedrock_sonic() {
            return self.handle_sonic_text(&user_input, ui_tx, cancel).await;
        }
        // An approval that timed out while the user was away is resolved before
        // the new input is interpreted, so it can never be read as an answer.
        if let Some(reply) = self.expire_stale_approval() {
            self.messages.push(Message::user(user_input));
            self.messages.push(Message::assistant(reply.clone()));
            self.persist_session();
            return Ok(Some(reply));
        }
        // Bind first: a guard in an `if let` scrutinee lives for the whole
        // body, and the body mutates turn state.
        let pending = self.control().take_pending();
        if let Some(pending) = pending {
            if is_denial(&user_input) {
                if let Some(message) = self.messages.get_mut(pending.result_index) {
                    *message = Message::tool_result(
                        pending.call_id,
                        "The user declined this action. Do not execute it.",
                    );
                }
                self.persist_session();
                // "no, open Firefox" is a denial plus a new request: record the
                // refusal and answer the instruction in this same turn.
                match denial_instruction(&user_input) {
                    Some(instruction) => user_input = instruction,
                    None => {
                        self.messages.push(Message::user(user_input));
                        let reply = "Understood. I did not perform that action.";
                        self.messages.push(Message::assistant(reply));
                        self.reply_needs_follow_up = Some(false);
                        self.persist_session();
                        return Ok(Some(reply.to_string()));
                    }
                }
            } else if (confirms(&user_input) || confirms_turn(&user_input))
                && !cancel.is_cancelled()
            {
                // The confirmation authorizes this stored call directly. The model
                // does not get another opportunity to substitute its arguments.
                // "Allow for this turn" also covers the rest of this turn.
                self.action_journal.push(ActionJournalEntry {
                    tool: pending.name.clone(),
                    arguments: journal_arguments(&pending.arguments),
                    status: "in_flight; verify before retry after interruption".into(),
                });
                self.persist_session();
                let (result, timed_out) = execute_tool_with_deadline(
                    &self.tools,
                    &pending.name,
                    &pending.arguments,
                    cancel,
                    self.loop_limits.max_duration,
                )
                .await?;
                if let Some(entry) = self.action_journal.last_mut() {
                    entry.status = if cancel.is_cancelled() || timed_out {
                        "in_flight; outcome uncertain after interruption"
                    } else if result.success {
                        "completed"
                    } else {
                        "failed"
                    }
                    .into();
                }
                if let Some(message) = self.messages.get_mut(pending.result_index) {
                    *message = Message::tool_result(pending.call_id, result.as_agent_message());
                }
                if confirms_turn(&user_input) {
                    self.grant_for_turn(&pending.name, &pending.arguments);
                }
                self.persist_session();
            } else if !cancel.is_cancelled() {
                // The user said something else, so the stored call is dropped
                // instead of holding the conversation hostage. The pending
                // state is already cleared, the placeholder records what
                // happened, and the new request runs in this same turn.
                if let Some(message) = self.messages.get_mut(pending.result_index) {
                    *message = Message::tool_result(
                        pending.call_id,
                        "Not confirmed: this action was dropped because the user asked something else. Do not execute it; answer the new request instead.",
                    );
                }
                if let Some(sink) = sentences {
                    let _ = sink.send(DROPPED_APPROVAL_NOTE.to_string());
                }
                self.persist_session();
            } else {
                // A cancelled turn keeps its confirmation armed so a later
                // answer still applies to the same call.
                self.control().set_pending(Some(pending));
                self.messages.push(Message::user(user_input));
                let reply = "I still need a clear yes or no for that action. Say confirm to allow it once, or no to cancel.";
                self.messages.push(Message::assistant(reply));
                self.reply_needs_follow_up = Some(true);
                self.persist_session();
                return Ok(Some(reply.to_string()));
            }
        }
        /*
         * The system prompt contains the wall-clock time and retrieved
         * memories; both were frozen at process start. Rebuild it every turn
         * -- and retrieve memories relevant to this message -- so a
         * long-running session never acts on a stale date or misses something
         * the user told us earlier.
         */
        let mut prompt = build_system_prompt(&self.tool_guide, &user_input, false);
        // Approval is runtime state, not something inferred from old assistant
        // questions retained in conversation history. Any approval above has
        // already been consumed before we enter the model loop.
        prompt.push_str("\nThere is currently no pending tool approval. Earlier confirmation questions in conversation history are inactive. Never ask the user for permission before calling a tool, and do not ask to confirm an ordinary app launch: call the tool and let the runtime hold it when it is consequential. Only a new tool result explicitly requiring approval establishes a pending tool approval; launch failures, missing executables, timeouts, and other execution errors do not. Never claim that the tool requires confirmation unless its result actually says so.\n");
        if !self.vision {
            prompt.push_str("\nImage input is unavailable for this endpoint. Use browser DOM, accessibility, OCR/find_text rather than interpreting screenshot files.\n");
            strip_image_inputs(&mut self.messages);
        }
        append_desktop_context(&mut prompt).await;
        self.messages[0] = Message::system(prompt);

        // Keep the provider request bounded before adding this turn. This is
        // deliberately structural: complete old turns are removed first,
        // while the current turn and assistant/tool message ordering remain
        // untouched.
        if self.compact_history() {
            eprintln!(
                "[Agent] Compacted history before the turn ({} messages)",
                self.messages.len()
            );
        }

        self.control().set_task_paused(false);
        self.messages.push(Message::user(user_input));
        self.reply_needs_follow_up = None;
        let turn_started = Instant::now();
        let mut round = 0usize;
        let mut progress = ToolProgress::default();
        let mut completed_actions = Vec::new();

        loop {
            if cancel.is_cancelled() {
                self.control().set_task_paused(true);
                self.persist_session();
                return Ok(None);
            }

            let Some(remaining) = self.loop_limits.remaining(turn_started) else {
                return self.finish_guardrail(
                    "I stopped this task because it reached its time limit. I kept the partial progress; say continue and I’ll resume.",
                    sentences,
                );
            };

            if round >= self.loop_limits.max_rounds {
                return self.finish_guardrail(
                    "I paused this long task at its safety checkpoint. I kept the completed actions and current context; say continue and I’ll resume from here.",
                    sentences,
                );
            }
            round += 1;
            publish_round(ui_tx, round);

            let reply = match tokio::time::timeout(
                remaining,
                self.request_model(cancel, sentences, ui_tx),
            )
            .await
            {
                Err(_) => {
                    return self.finish_guardrail(
                        "I stopped this task because it reached its time limit while waiting for the model. I kept the partial progress; say continue and I’ll resume.",
                        sentences,
                    )
                }
                Ok(None) => {
                    self.control().set_task_paused(true);
                    self.persist_session();
                    return Ok(None);
                }

                Ok(Some(Ok(reply))) => reply,

                Ok(Some(Err(error))) => {
                    if !completed_actions.is_empty() {
                        return self.finish_guardrail(
                            format!(
                                "The model connection failed after these actions completed: {}. I kept their results; say continue to resume safely.",
                                completed_actions.join("; ")
                            ),
                            sentences,
                        );
                    }
                    self.control().set_task_paused(true);
                    self.persist_session();
                    return Err(error);
                }
            };

            if let Some(calls) = reply.message.tool_calls.clone() {
                if !calls.is_empty() {
                    // Models may provide a short natural-language preamble
                    // alongside tool calls. Speak/display that content before
                    // executing the calls; the final response still follows
                    // after the tool loop completes. A streamed reply already
                    // delivered it sentence by sentence.
                    if !reply.streamed {
                        if let Some(progress) = tool_call_preamble(&reply.message.text()) {
                            if let Some(sink) = sentences {
                                let _ = sink.send(progress.clone());
                            }
                            if let Some(tx) = ui_tx {
                                let _ = tx.try_send(HyuskEvent::Response(progress));
                            }
                        }
                    }

                    self.messages.push(reply.message.clone());
                    let mut round_progress = Vec::with_capacity(calls.len());
                    let mut timed_out = false;
                    let mut approval_waiting = false;
                    let mut screenshots = Vec::new();

                    let mut calls = calls.into_iter();
                    while let Some(call) = calls.next() {
                        if cancel.is_cancelled() {
                            self.messages.push(Message::tool_result(
                                call.id,
                                "The action was not started because the turn was cancelled.",
                            ));
                            for remaining in calls {
                                self.messages.push(Message::tool_result(
                                    remaining.id,
                                    "The action was not started because the turn was cancelled.",
                                ));
                            }
                            self.control().set_task_paused(true);
                            self.persist_session();
                            return Ok(None);
                        }

                        let name = call.function.name.clone();

                        let args_value: Value =
                            serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);

                        let args_str = call.function.arguments.clone();

                        // Arguments are logged, and a log line ends up in
                        // journald, so it gets the same treatment as the
                        // journal itself (X4).
                        println!(
                            "\n[Hyusk] Running {}: {}\n",
                            name,
                            journal_arguments(&args_value.to_string())
                        );

                        if let Some(tx) = ui_tx {
                            HyuskState::Working.publish();

                            // try_send: the UI channel must never block the
                            // agent while it holds the agent lock (in
                            // indicator mode nothing consumes this channel).
                            let _ = tx.try_send(HyuskEvent::StateChanged(HyuskState::Working));

                            let _ = tx.try_send(HyuskEvent::ToolStarted { name: name.clone() });
                        }

                        // Typing and key input are classified by the focused
                        // window, so read it only for those calls (D1).
                        let focus = self.focus_context(&name, &args_str).await;
                        let requires_approval =
                            crate::tools::safety::requires_approval_with_context(
                                &name, &args_str, &focus,
                            );
                        // "Allow for this turn" covers later calls with the same
                        // tool shape, so they run without another card.
                        let granted = requires_approval && self.turn_granted(&name, &args_str);
                        // A delegation that cannot start must never publish a
                        // confirmation card: the user would approve a dead end.
                        let unavailable =
                            crate::tools::task::delegation_preflight(&name, &args_str);
                        // A capability the desktop has already refused must not
                        // produce a card the user cannot satisfy (D3).
                        let capability = crate::tools::capabilities::refusal(&name, &args_str);
                        let result = if approval_waiting {
                            ToolResult::failure(
                                "Skipped because an earlier action in this batch is awaiting confirmation.",
                            )
                        } else if timed_out {
                            ToolResult::failure("Skipped because the turn time limit was reached.")
                        } else if self.tools.get(&name).is_none() {
                            // The policy holds unknown names closed, so resolve
                            // existence first: asking the user to approve a call
                            // that cannot run would be a dead end.
                            ToolResult::failure(format!("Tool '{name}' does not exist."))
                        } else if let Some(failure) = unavailable {
                            ToolResult::failure(failure)
                        } else if let Some(failure) = capability {
                            ToolResult::failure(failure)
                        } else if let Some(refusal) =
                            crate::tools::safety::secret_refusal(&name, &args_str)
                        {
                            // A credential read is refused outright: it never
                            // arms a card, so neither an approval nor a turn
                            // grant can run it (S2).
                            ToolResult::failure(refusal)
                        } else if requires_approval && !granted {
                            let index = self.messages.len();
                            // The preview is derived from the stored arguments,
                            // so the card names exactly what approval will run.
                            let arguments =
                                crate::tools::safety::approval_arguments(&name, &args_str);
                            let preview = crate::tools::safety::preview_with_context(
                                &name, &arguments, &focus,
                            );
                            self.control().set_pending(Some(PendingApproval {
                                name: name.clone(),
                                arguments,
                                preview: preview.clone(),
                                call_id: call.id.clone(),
                                result_index: index,
                                expires_at: Instant::now() + Duration::from_secs(300),
                            }));
                            approval_waiting = true;
                            ToolResult::failure(format!(
                                "{}. Say what this will do in one short sentence, naming the tool and the first few argument words, then ask for one yes or no. Do not retry it until the user answers.",
                                crate::tools::safety::pending_text(&preview)
                            ))
                        } else {
                            match self.loop_limits.remaining(turn_started) {
                                Some(remaining) => {
                                    self.action_journal.push(ActionJournalEntry {
                                        tool: name.clone(),
                                        arguments: journal_arguments(&args_str),
                                        status: "in_flight; verify before retry after interruption"
                                            .into(),
                                    });
                                    self.persist_session();
                                    let (result, did_timeout) = execute_tool_with_deadline(
                                        &self.tools,
                                        &name,
                                        &args_str,
                                        cancel,
                                        remaining,
                                    )
                                    .await?;
                                    timed_out = did_timeout;
                                    if let Some(entry) = self.action_journal.last_mut() {
                                        entry.status = if cancel.is_cancelled() || did_timeout {
                                            "in_flight; outcome uncertain after cancellation"
                                        } else if result.success {
                                            "completed"
                                        } else {
                                            "failed"
                                        }
                                        .into();
                                    }
                                    result
                                }
                                None => {
                                    timed_out = true;
                                    ToolResult::failure(
                                        "Tool execution skipped because the turn time limit was reached.",
                                    )
                                }
                            }
                        };

                        let success = result.success;

                        if success {
                            completed_actions.push(format!("{} ({})", name, short(&result.output)));
                        }

                        let (tool_output, image_path) = if self.vision && name == "computer" {
                            split_image_marker(&result.output)
                        } else {
                            (result.output.clone(), None)
                        };

                        let tool_result = ToolResult {
                            success,
                            output: tool_output,
                            error: result.error.clone(),
                        };

                        round_progress.push((
                            tool_call_signature(&name, &args_value, &args_str),
                            tool_result.as_agent_message(),
                        ));

                        self.messages.push(Message::tool_result(
                            call.id.clone(),
                            tool_result.as_agent_message(),
                        ));
                        self.persist_session();

                        if let Some(image_path) = image_path {
                            screenshots.push(image_path);
                        }

                        if let Some(tx) = ui_tx {
                            let _ = tx.try_send(HyuskEvent::ToolFinished { name, success });
                        }
                    }

                    // A provider expects every result for this assistant batch
                    // before any new user message, including a screenshot.
                    for image_path in screenshots {
                        match load_image_data_url(&image_path).await {
                            Some(data_url) => self.messages.push(Message::user_with_image(
                                format!("Screenshot at {image_path}"),
                                data_url,
                            )),
                            None => eprintln!("[Agent] Could not read screenshot {image_path}"),
                        }
                    }

                    if timed_out {
                        return self.finish_guardrail(
                            "I stopped this task because it reached its time limit during a tool action. I kept the partial progress; say continue and I’ll resume.",
                            sentences,
                        );
                    }

                    let repeated_rounds = progress.observe(round_progress);
                    if repeated_rounds >= self.loop_limits.max_repeated_rounds {
                        println!("[Agent] Stopping after {repeated_rounds} identical tool rounds");
                        return self.finish_guardrail(
                            "I stopped this task because the same tool step repeated without progress. I kept the partial progress; say continue with a correction or more detail.",
                            sentences,
                        );
                    }

                    /*
                     * A long task can collect more tool output than the
                     * provider accepts before the next user turn exists. Compact
                     * here, while every call in this batch already has its
                     * result, so the next request stays inside the window.
                     *
                     * A pending approval is bound to a message index, so leave
                     * history untouched until that confirmation is consumed.
                     */
                    if self.compact_at_tool_boundary() {
                        eprintln!(
                            "[Agent] Compacted history mid-turn ({} messages)",
                            self.messages.len()
                        );
                    }

                    continue;
                }
            }

            let (text, needs_reply) = parse_structured_reply(&reply.message.text());
            self.reply_needs_follow_up = needs_reply;
            self.control().set_task_paused(false);
            self.messages.push(Message::assistant(text.clone()));
            self.persist_session();

            // A streamed reply has already been spoken and displayed, sentence
            // by sentence. Re-sending it would repeat the whole answer.
            if !reply.streamed {
                if let Some(sink) = sentences {
                    if !text.trim().is_empty() {
                        let _ = sink.send(text.clone());
                    }
                }
            }

            return Ok(Some(text));
        }
    }

    fn finish_guardrail(
        &mut self,
        text: impl Into<String>,
        sentences: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<Option<String>> {
        let text = text.into();
        // Guardrail replies explicitly offer continuation, so keep the
        // hands-free follow-up window open for a spoken "continue".
        self.reply_needs_follow_up = Some(true);
        self.control().set_task_paused(true);
        self.messages.push(Message::assistant(text.clone()));
        self.persist_session();

        if let Some(sink) = sentences {
            if !text.trim().is_empty() {
                let _ = sink.send(text.clone());
            }
        }

        Ok(Some(text))
    }

    /// Compact at a tool boundary inside a turn, so accumulated tool output
    /// cannot outgrow the provider's window before the next user turn.
    ///
    /// A pending approval is stored as the index of its placeholder tool
    /// result. Compaction shifts message indices, so leave history alone until
    /// that confirmation is consumed.
    fn compact_at_tool_boundary(&mut self) -> bool {
        if self.control().armed() {
            return false;
        }
        self.compact_history()
    }

    /// Compact history to the budget derived from the selected model's context
    /// window. Returns whether anything changed.
    fn compact_history(&mut self) -> bool {
        let model = self.control().model();
        session::compact_history(&mut self.messages, session::history_budget(&model))
    }
}

/// Journal arguments with secret-shaped values replaced.
///
/// The journal is persisted in the session file and is reused by later prompts,
/// and a call's arguments never pass through X2's tool-output choke point, so
/// this is where a credential the model put into a call is dropped (X4).
fn journal_arguments(arguments: &str) -> String {
    crate::tools::redact::redact(arguments).0
}

fn configured_vision() -> bool {
    std::env::var("MODEL_VISION")
        .map(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true)
}

fn rejects_image_input(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "no endpoints found that support image input",
        "does not support image",
        "image input is not supported",
        "image_url is not supported",
        "does not support vision",
    ]
    .iter()
    .any(|message| error.contains(message))
}

fn strip_image_inputs(messages: &mut [Message]) -> bool {
    use crate::types::message::{ContentPart, MessageContent};
    let mut changed = false;
    for message in messages {
        if let Some(MessageContent::Parts(parts)) = &message.content {
            if parts
                .iter()
                .any(|part| matches!(part, ContentPart::ImageUrl { .. }))
            {
                let mut text = parts
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                text.push_str("\n[Image omitted: this model endpoint cannot accept images. Use browser DOM, accessibility, or OCR. Do not infer visual content from the screenshot path.]");
                message.content = Some(MessageContent::Text(text));
                changed = true;
            }
        }
    }
    changed
}

fn short(text: &str) -> String {
    let first = text.lines().next().unwrap_or(text).trim();

    if first.chars().count() > 120 {
        first.chars().take(117).collect::<String>() + "..."
    } else {
        first.to_string()
    }
}

fn publish_round(ui_tx: Option<&Sender<HyuskEvent>>, round: usize) {
    println!("[Agent] Starting tool round {round}");
    crate::status::card("progress", format!("Working… step {round}"), false);

    if let Some(tx) = ui_tx {
        HyuskState::Working.publish();
        // The UI channel may not be consumed in indicator mode. Progress
        // reporting must never hold up the model/tool loop in that case.
        let _ = tx.try_send(HyuskEvent::StateChanged(HyuskState::Working));
    }
}

fn tool_call_signature(name: &str, args: &Value, raw_args: &str) -> String {
    let normalized = serde_json::to_string(args).unwrap_or_else(|_| raw_args.to_string());
    format!("{name}:{normalized}")
}

async fn execute_tool(
    tools: &ToolRegistry,
    name: &str,
    arguments: &str,
    cancel: &CancellationToken,
) -> Result<ToolResult> {
    // A ready tool future can perform work on its first poll. Reject an
    // already-cancelled call before constructing/polling that future, and
    // prefer cancellation if both branches become ready together.
    if cancel.is_cancelled() {
        return Ok(ToolResult::failure("Action cancelled before execution."));
    }
    let Some(tool) = tools.get(name) else {
        return Ok(ToolResult::failure(format!(
            "Tool '{name}' does not exist."
        )));
    };

    match tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        result = tool.execute(arguments) => Some(result),
    } {
        None => Ok(ToolResult::failure("Action cancelled.")),
        Some(Ok(result)) => Ok(redact_tool_result(result)),
        Some(Err(error)) => Ok(redact_tool_result(ToolResult::failure(format!(
            "Tool execution error: {error:#}"
        )))),
    }
}

/// Redact secret-shaped values before a tool result reaches the session, the
/// journal, the provider request, or status text (X2). The note tells the model
/// that the output was edited and how many values were replaced.
fn redact_tool_result(mut result: ToolResult) -> ToolResult {
    let (output, output_replaced) = crate::tools::redact::redact(&result.output);
    let (error, error_replaced) = match result.error {
        Some(error) => {
            let (redacted, replaced) = crate::tools::redact::redact(&error);
            (Some(redacted), replaced)
        }
        None => (None, 0),
    };
    let replaced = output_replaced + error_replaced;
    result.output = output;
    result.error = error;
    if replaced > 0 {
        let note = crate::tools::redact::note(replaced);
        if !result.output.is_empty() {
            result.output.push_str(&note);
        } else if let Some(error) = result.error.as_mut() {
            error.push_str(&note);
        }
    }
    result
}

async fn execute_tool_with_deadline(
    tools: &ToolRegistry,
    name: &str,
    arguments: &str,
    cancel: &CancellationToken,
    remaining: Duration,
) -> Result<(ToolResult, bool)> {
    // Tokio timeout polls its inner future before checking the timer. A zero
    // budget must not start a quick side effect and then report success.
    if remaining.is_zero() {
        return Ok((
            ToolResult::failure("Tool deadline expired before execution."),
            true,
        ));
    }
    let started = Instant::now();
    let result =
        match tokio::time::timeout(remaining, execute_tool(tools, name, arguments, cancel)).await {
            Ok(result) => Ok((result?, false)),
            Err(_) => Ok((
                ToolResult::failure("Tool execution timed out before it completed."),
                true,
            )),
        };
    crate::timing::mark(&format!("tool {name}"), started);
    result
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

/// Extract `HYUSK_IMAGE:<path>` markers from tool output, preserving other text.
fn split_image_marker(output: &str) -> (String, Option<String>) {
    const PREFIX: &str = "HYUSK_IMAGE:";

    let mut cleaned = Vec::new();
    let mut path = None;

    for line in output.lines() {
        if let Some(rest) = line.trim_start().strip_prefix(PREFIX) {
            path = Some(rest.trim().to_string());
        } else {
            cleaned.push(line);
        }
    }

    (cleaned.join("\n"), path)
}

async fn load_image_data_url(path: &str) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let encoded = STANDARD.encode(bytes);

    Some(format!("data:image/png;base64,{encoded}"))
}

#[cfg(test)]
#[path = "runner/transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };

    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use tokio_util::sync::CancellationToken;

    use super::{
        build_system_prompt, confirms, execute_tool_with_deadline, is_denial,
        is_voice_session_stop, journal_arguments, parse_structured_reply, rejects_image_input,
        split_image_marker, strip_image_inputs, tool_call_signature, Agent, AgentLoopLimits,
        PendingApproval, ToolProgress,
    };
    use crate::{
        model::openrouter::OpenRouterClient,
        tools::{Tool, ToolRegistry, ToolResult},
        types::Message,
    };

    struct CountingTool(Arc<AtomicUsize>);

    struct HangingTool {
        calls: Arc<AtomicUsize>,
        started: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Tool for HangingTool {
        fn name(&self) -> &str {
            "media"
        }

        fn description(&self) -> &str {
            "Test media action."
        }

        async fn execute(&self, _input: &str) -> Result<ToolResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            std::future::pending().await
        }
    }

    #[test]
    fn image_fallback_preserves_message_roles_and_tool_correlation() {
        let mut messages = vec![
            Message::system("System rules"),
            Message::user("Inspect this page"),
            serde_json::from_value(json!({"role":"assistant","tool_calls":[{"id":"call-17","type":"function","function":{"name":"computer","arguments":"{\"action\":\"screenshot\"}"}}]})).unwrap(),
            Message::tool_result("call-17", "Screenshot saved"),
            Message::user_with_image("Screenshot observation", "data:image/png;base64,FAKE"),
        ];
        assert!(strip_image_inputs(&mut messages));
        assert!(!strip_image_inputs(&mut messages));
        assert_eq!(messages[3].role, "tool");
        assert_eq!(messages[3].tool_call_id.as_deref(), Some("call-17"));
        assert_eq!(messages[4].role, "user");
        assert!(messages[4].text().contains("Use browser DOM"));
        let serialized = serde_json::to_string(&messages).unwrap();
        assert!(!serialized.contains("data:image"));
        assert!(!serialized.contains("image_url"));
        assert!(rejects_image_input(
            "HTTP 404: No endpoints found that support image input"
        ));
        assert!(!rejects_image_input("HTTP 401: missing authentication"));
    }

    #[tokio::test]
    async fn rejected_image_is_retried_once_as_text_without_replaying_tools() -> Result<()> {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let body_start;
                loop {
                    let mut chunk = [0; 4096];
                    let read = stream.read(&mut chunk).await.unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&chunk[..read]);
                    assert!(bytes.len() < 1_000_000);
                    if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                        let length: usize = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();
                        if bytes.len() >= index + 4 + length {
                            body_start = index + 4;
                            break;
                        }
                    }
                }
                let request: Value = serde_json::from_slice(&bytes[body_start..]).unwrap();
                let has_image = request["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["content"].is_array());
                assert_eq!(has_image, attempt == 0);
                let (status, body) = if attempt == 0 {
                    (
                        "404 Not Found",
                        json!({"error":{"message":"No endpoints found that support image input"}}),
                    )
                } else {
                    (
                        "200 OK",
                        json!({"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}),
                    )
                };
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let mut agent = Agent::new(
            OpenRouterClient::new("local-test".into(), base),
            "test-model".into(),
            ToolRegistry::new(),
        );
        agent.vision = true;
        agent.messages = vec![
            Message::system("Test"),
            Message::user_with_image("Inspect", "data:image/png;base64,FAKE"),
        ];
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            agent.request_model(&CancellationToken::new(), None, None),
        )
        .await?
        .unwrap()?
        .message;
        assert_eq!(result.text(), "done");
        assert!(!agent.vision);
        server.await?;
        Ok(())
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            "media"
        }

        fn description(&self) -> &str {
            "Test media action."
        }

        async fn execute(&self, _input: &str) -> Result<ToolResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success("played"))
        }
    }

    #[test]
    fn journal_arguments_lose_a_secret_before_the_entry_is_stored() {
        let arguments = r#"{"command":"curl -H 'Authorization: Bearer ghp_0123456789abcdef' https://example.test"}"#;
        let stored = journal_arguments(arguments);

        assert!(!stored.contains("ghp_0123456789abcdef"), "{stored}");
        assert!(stored.contains("[redacted]"), "{stored}");
    }

    #[tokio::test]
    async fn fast_command_cannot_bypass_pending_confirmation() {
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(CountingTool(Arc::clone(&executions)));
        let mut agent = Agent::new(
            OpenRouterClient::new("test-key".into(), "http://127.0.0.1".into()),
            "test-model".into(),
            tools,
        );
        agent.control().set_pending(Some(PendingApproval {
            name: "shell".into(),
            arguments: "{\"command\":\"dangerous\"}".into(),
            preview: crate::tools::safety::preview("shell", "{\"command\":\"dangerous\"}"),
            call_id: "pending-call".into(),
            result_index: 0,
            expires_at: std::time::Instant::now() + Duration::from_secs(60),
        }));

        let result = agent
            .try_fast_command("play music", &CancellationToken::new())
            .await;

        assert!(result.is_none());
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(agent.awaiting_approval());
    }

    #[tokio::test]
    async fn fast_command_defers_a_guarded_plan_before_the_first_side_effect() {
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(ShellStub(Arc::clone(&executions)));
        tools.register(CountingTool(Arc::clone(&executions)));
        let mut agent = Agent::new(
            OpenRouterClient::new("test-key".into(), "http://127.0.0.1".into()),
            "test-model".into(),
            tools,
        );

        // "mute" runs `pactl`, which the shell policy guards; "volume up" is
        // free on its own. A plan containing the guarded step must run neither.
        let guarded = agent
            .try_fast_command("mute", &CancellationToken::new())
            .await;
        assert!(
            guarded.is_none(),
            "a guarded action must defer to the model path"
        );

        let mixed = agent
            .try_fast_command("mute; volume up", &CancellationToken::new())
            .await;
        assert!(
            mixed.is_none(),
            "a plan with one guarded step must defer as a whole"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            0,
            "no step may run when any step would ask"
        );
    }

    /// Requests the stub model received, in order.
    type RequestLog = Arc<std::sync::Mutex<Vec<Value>>>;

    /// Serve chat-completions requests: `calls` until the conversation carries
    /// a tool result, then `reply`. Streaming attempts receive the same JSON
    /// body and fall back to the plain request. Every request body is recorded
    /// so a test can check what actually reached the model.
    async fn serve_tool_call_then_reply(
        calls: Value,
        reply: Value,
    ) -> Result<(String, RequestLog)> {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let log: RequestLog = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&log);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut bytes = Vec::new();
                let mut body_start = None;
                while bytes.len() < 1_000_000 {
                    let mut chunk = [0; 8192];
                    let Ok(read) = stream.read(&mut chunk).await else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                        let length: usize = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap_or("0")
                            .trim()
                            .parse()
                            .unwrap_or(0);
                        if bytes.len() >= index + 4 + length {
                            body_start = Some(index + 4);
                            break;
                        }
                    }
                }
                let Some(body_start) = body_start else {
                    continue;
                };
                let request: Value =
                    serde_json::from_slice(&bytes[body_start..]).unwrap_or(Value::Null);
                recorded.lock().unwrap().push(request.clone());
                let has_tool_result = request["messages"]
                    .as_array()
                    .map(|messages| messages.iter().any(|message| message["role"] == "tool"))
                    .unwrap_or(false);
                let canned = if has_tool_result {
                    reply.clone()
                } else {
                    calls.clone()
                };
                // A streamed request needs SSE; replay the canned text as one
                // delta so the assembled reply matches the plain body.
                let (content_type, body) = if request["stream"].as_bool().unwrap_or(false) {
                    let text = canned["choices"][0]["message"]["content"]
                        .as_str()
                        .unwrap_or_default();
                    let delta = json!({"choices":[{
                        "delta": {"role": "assistant", "content": text},
                        "finish_reason": "stop"
                    }]});
                    (
                        "text/event-stream",
                        format!("data: {delta}\n\ndata: [DONE]\n\n"),
                    )
                } else {
                    ("application/json", canned.to_string())
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        Ok((base, log))
    }

    #[tokio::test]
    async fn unknown_tool_name_reports_a_missing_tool_instead_of_arming_an_approval() -> Result<()>
    {
        let (base, _requests) = serve_tool_call_then_reply(
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
                {"id":"call-ghost","type":"function","function":{"name":"ghost","arguments":"{}"}}
            ]},"finish_reason":"tool_calls"}]}),
            json!({"choices":[{"message":{"role":"assistant","content":"{\"text\":\"There is no such tool.\",\"needs_reply\":false,\"reply_type\":\"none\"}"},"finish_reason":"stop"}]}),
        )
        .await?;
        let mut agent = Agent::new(
            OpenRouterClient::new("local-test".into(), base),
            "test-model".into(),
            ToolRegistry::new(),
        );
        // Keep the turn independent of any session another test persisted.
        agent.messages = vec![Message::system("Test")];

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle(
                "use the ghost tool".into(),
                None,
                &CancellationToken::new(),
                None,
            ),
        )
        .await??;

        assert!(reply.is_some());
        assert!(
            !agent.awaiting_approval(),
            "an unknown tool name must not arm a confirmation"
        );
        assert!(agent
            .messages
            .iter()
            .any(|message| message.role == "tool" && message.text().contains("does not exist")));
        Ok(())
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

        async fn execute(&self, _input: &str) -> Result<ToolResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success("ran"))
        }
    }

    #[tokio::test]
    async fn guarded_call_is_held_once_and_the_pending_call_names_it() -> Result<()> {
        let (base, _requests) = serve_tool_call_then_reply(
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
                {"id":"call-shell","type":"function","function":{"name":"shell","arguments":"{\"command\":\"rm -rf build\"}"}}
            ]},"finish_reason":"tool_calls"}]}),
            final_reply(),
        )
        .await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(ShellStub(Arc::clone(&executions)));
        let mut agent = Agent::new(
            OpenRouterClient::new("local-test".into(), base),
            "test-model".into(),
            tools,
        );
        agent.messages = vec![Message::system("Test")];

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle(
                "delete the build folder".into(),
                None,
                &CancellationToken::new(),
                None,
            ),
        )
        .await??;

        assert!(reply.is_some());
        assert!(agent.awaiting_approval(), "the call must be held");
        assert_eq!(
            executions.load(Ordering::SeqCst),
            0,
            "a held call must not run"
        );
        // The stored preview is the exact call, and the model is told to repeat
        // it rather than paraphrase it.
        assert_eq!(
            agent.pending_approval_preview().as_deref(),
            Some("shell command=rm -rf build")
        );
        let result = agent
            .messages
            .iter()
            .find(|message| message.role == "tool")
            .expect("a tool result");
        assert!(
            result
                .text()
                .contains("Pending confirmation for: shell command=rm -rf build"),
            "{}",
            result.text()
        );
        Ok(())
    }

    #[tokio::test]
    async fn credential_read_is_refused_instead_of_armed() -> Result<()> {
        let (base, _requests) = serve_tool_call_then_reply(
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
                {"id":"call-shell","type":"function","function":{"name":"shell","arguments":"{\"command\":\"cat .env\"}"}}
            ]},"finish_reason":"tool_calls"}]}),
            final_reply(),
        )
        .await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(ShellStub(Arc::clone(&executions)));
        let mut agent = Agent::new(
            OpenRouterClient::new("local-test".into(), base),
            "test-model".into(),
            tools,
        );
        agent.messages = vec![Message::system("Test")];

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle(
                "show me the .env file".into(),
                None,
                &CancellationToken::new(),
                None,
            ),
        )
        .await??;

        assert!(reply.is_some());
        assert!(
            !agent.awaiting_approval(),
            "a refused credential read must not arm a confirmation"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            0,
            "a refused read must not run"
        );
        let result = agent
            .messages
            .iter()
            .find(|message| message.role == "tool")
            .expect("a tool result");
        assert!(
            result.text().contains("secret denylist"),
            "{}",
            result.text()
        );
        assert!(
            !result.text().contains(".env"),
            "the refusal must not echo the path: {}",
            result.text()
        );
        Ok(())
    }

    struct ComputerStub(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for ComputerStub {
        fn name(&self) -> &str {
            "computer"
        }

        fn description(&self) -> &str {
            "Test desktop input."
        }

        async fn execute(&self, _input: &str) -> Result<ToolResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success("moved"))
        }
    }

    /// Restore the shared capability record when a test ends, even on failure,
    /// so one test's denial cannot leak into another.
    struct CapabilityReset;

    impl Drop for CapabilityReset {
        fn drop(&mut self) {
            crate::tools::capabilities::reset_for_test();
        }
    }

    #[tokio::test]
    async fn a_denied_input_capability_fails_without_arming_a_card() -> Result<()> {
        crate::tools::capabilities::record_input(
            crate::tools::capabilities::Capability::Denied,
            "RemoteDesktop portal",
        );
        let _reset = CapabilityReset;

        let (base, _requests) = serve_tool_call_then_reply(
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
                {"id":"call-input","type":"function","function":{"name":"computer","arguments":"{\"action\":\"mouse_click\",\"x\":10,\"y\":20}"}}
            ]},"finish_reason":"tool_calls"}]}),
            final_reply(),
        )
        .await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(ComputerStub(Arc::clone(&executions)));
        let mut agent = Agent::new(
            OpenRouterClient::new("local-test".into(), base),
            "test-model".into(),
            tools,
        );
        agent.messages = vec![Message::system("Test")];

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle("click there".into(), None, &CancellationToken::new(), None),
        )
        .await??;

        assert!(reply.is_some());
        assert!(
            !agent.awaiting_approval(),
            "a denied capability must not arm an approval card"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            0,
            "the refused call must not run"
        );
        let result = agent
            .messages
            .iter()
            .find(|message| message.role == "tool")
            .expect("a tool result");
        assert!(
            result.text().contains("input capability denied"),
            "{}",
            result.text()
        );
        assert!(
            !result.text().contains("Pending confirmation"),
            "{}",
            result.text()
        );
        Ok(())
    }

    struct LeakyTool;

    #[async_trait]
    impl Tool for LeakyTool {
        fn name(&self) -> &str {
            "leaky"
        }

        fn description(&self) -> &str {
            "Returns a credential for the redaction test."
        }

        async fn execute(&self, _input: &str) -> Result<ToolResult> {
            Ok(ToolResult::success(
                "OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef\n\
                 -----BEGIN OPENSSH PRIVATE KEY-----\n\
                 b3BlbnNzaC1rZXktdjEAAAAA\n\
                 -----END OPENSSH PRIVATE KEY-----\n\
                 all done",
            ))
        }
    }

    #[tokio::test]
    async fn tool_output_is_redacted_before_it_becomes_a_result() -> Result<()> {
        let mut tools = ToolRegistry::new();
        tools.register(LeakyTool);

        let (result, timed_out) = execute_tool_with_deadline(
            &tools,
            "leaky",
            "{}",
            &CancellationToken::new(),
            Duration::from_secs(5),
        )
        .await?;

        assert!(!timed_out);
        assert!(result.success);
        assert!(
            !result.output.contains("sk-or-v1-0123456789abcdef"),
            "{}",
            result.output
        );
        assert!(
            !result.output.contains("b3BlbnNzaC1rZXktdjEAAAAA"),
            "{}",
            result.output
        );
        assert!(result.output.contains("OPENROUTER_API_KEY=[redacted]"));
        assert!(result.output.contains("[redacted private key]"));
        assert!(result.output.contains("all done"));
        assert!(
            result.output.contains("redacted 2 secret-shaped value(s)"),
            "{}",
            result.output
        );
        Ok(())
    }

    fn task_delegate_call(arguments: &str) -> Value {
        json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"call-task","type":"function","function":{"name":"task","arguments":arguments}}
        ]},"finish_reason":"tool_calls"}]})
    }

    fn final_reply() -> Value {
        json!({"choices":[{"message":{"role":"assistant","content":"{\"text\":\"Understood.\",\"needs_reply\":false,\"reply_type\":\"none\"}"},"finish_reason":"stop"}]})
    }

    async fn agent_with_task_tool(
        base: String,
    ) -> (Agent, tokio::sync::mpsc::Receiver<crate::types::HyuskEvent>) {
        use crate::{model::SharedActiveModel, tools::task::TaskTool};

        let (events, receiver) = tokio::sync::mpsc::channel(8);
        let mut tools = ToolRegistry::new();
        tools.register(TaskTool::new(
            events.clone(),
            SharedActiveModel::new(
                "openrouter",
                "fixture",
                OpenRouterClient::new("unused".into(), "http://127.0.0.1:1".into()),
            ),
        ));
        let mut agent = Agent::new(
            OpenRouterClient::new("local-test".into(), base),
            "test-model".into(),
            tools,
        );
        agent.messages = vec![Message::system("Test")];
        (agent, receiver)
    }

    #[tokio::test]
    async fn research_delegation_runs_without_an_approval_card() -> Result<()> {
        let (base, _requests) = serve_tool_call_then_reply(
            task_delegate_call(
                r#"{"action":"delegate","task":"summarise the notes","profile":"research"}"#,
            ),
            final_reply(),
        )
        .await?;
        let (mut agent, _events) = agent_with_task_tool(base).await;

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle(
                "research this".into(),
                None,
                &CancellationToken::new(),
                None,
            ),
        )
        .await??;

        assert!(reply.is_some());
        assert!(
            !agent.awaiting_approval(),
            "research delegation must not ask for confirmation"
        );
        assert!(
            agent
                .messages
                .iter()
                .any(|message| message.role == "tool" && message.text().contains("Delegated")),
            "the research worker should have started"
        );
        Ok(())
    }

    #[tokio::test]
    async fn unavailable_workspace_delegation_reports_instead_of_asking() -> Result<()> {
        let (base, _requests) = serve_tool_call_then_reply(
            task_delegate_call(
                r#"{"action":"delegate","task":"fix the build","profile":"workspace","workspace":"/nonexistent/hyusk-workspace-probe"}"#,
            ),
            final_reply(),
        )
        .await?;
        let (mut agent, _events) = agent_with_task_tool(base).await;

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle(
                "fix the build".into(),
                None,
                &CancellationToken::new(),
                None,
            ),
        )
        .await??;

        assert!(reply.is_some());
        assert!(
            !agent.awaiting_approval(),
            "an unavailable delegation must not publish a confirmation"
        );
        assert!(
            agent.messages.iter().any(|message| message.role == "tool"
                && message
                    .text()
                    .contains("Workspace delegation is unavailable")),
            "the failure must reach the model"
        );
        Ok(())
    }

    /// Serve canned chat-completions bodies in order, repeating the last one.
    /// Requests are recorded so a test can see what reached the model.
    async fn serve_scripted(responses: Vec<Value>) -> Result<(String, RequestLog)> {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let log: RequestLog = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&log);
        tokio::spawn(async move {
            let mut index = 0usize;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut bytes = Vec::new();
                let mut body_start = None;
                while bytes.len() < 1_000_000 {
                    let mut chunk = [0; 8192];
                    let Ok(read) = stream.read(&mut chunk).await else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                        let length: usize = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap_or("0")
                            .trim()
                            .parse()
                            .unwrap_or(0);
                        if bytes.len() >= index + 4 + length {
                            body_start = Some(index + 4);
                            break;
                        }
                    }
                }
                let Some(body_start) = body_start else {
                    continue;
                };
                let request: Value =
                    serde_json::from_slice(&bytes[body_start..]).unwrap_or(Value::Null);
                recorded.lock().unwrap().push(request.clone());
                let canned = responses
                    .get(index.min(responses.len().saturating_sub(1)))
                    .cloned()
                    .unwrap_or(Value::Null);
                index += 1;
                let body = canned.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        Ok((base, log))
    }

    /// A model reply that calls `shell` with `command`.
    fn shell_call(id: &str, command: &str) -> Value {
        json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":id,"type":"function","function":{"name":"shell","arguments":json!({"command": command}).to_string()}}
        ]},"finish_reason":"tool_calls"}]})
    }

    /// Arm a one-shot confirmation for a stored `shell` call, with the
    /// placeholder result the runner would have pushed.
    fn arm_shell_approval(agent: &mut Agent) -> usize {
        // The conversation must hold the assistant call the placeholder answers.
        agent.messages.push(
            serde_json::from_value(json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": "pending-call",
                    "type": "function",
                    "function": {"name": "shell", "arguments": "{\"command\":\"rm -rf build\"}"}
                }]
            }))
            .expect("assistant tool call"),
        );
        let index = agent.messages.len();
        agent.messages.push(Message::tool_result(
            "pending-call",
            "Pending confirmation for: shell command=rm -rf build",
        ));
        agent.control().set_pending(Some(PendingApproval {
            name: "shell".into(),
            arguments: "{\"command\":\"rm -rf build\"}".into(),
            preview: crate::tools::safety::preview("shell", "{\"command\":\"rm -rf build\"}"),
            call_id: "pending-call".into(),
            result_index: index,
            expires_at: Instant::now() + Duration::from_secs(300),
        }));
        index
    }

    fn shell_agent(base: String, executions: &Arc<AtomicUsize>) -> Agent {
        let mut tools = ToolRegistry::new();
        tools.register(ShellStub(Arc::clone(executions)));
        let mut agent = Agent::new(
            OpenRouterClient::new("local-test".into(), base),
            "test-model".into(),
            tools,
        );
        agent.messages = vec![Message::system("Test")];
        agent
    }

    /// The last user message the model actually received.
    fn last_user_text(requests: &RequestLog) -> String {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter_map(|request| request["messages"].as_array())
            .flatten()
            .filter(|message| message["role"] == "user")
            .filter_map(|message| message["content"].as_str())
            .next_back()
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn unrelated_request_drops_the_pending_action_and_runs_in_the_same_turn() -> Result<()> {
        let (base, requests) = serve_tool_call_then_reply(Value::Null, final_reply()).await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);
        let index = arm_shell_approval(&mut agent);
        let (sentences, mut speech) = tokio::sync::mpsc::unbounded_channel();

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle(
                "open Firefox".into(),
                None,
                &CancellationToken::new(),
                Some(&sentences),
            ),
        )
        .await??;

        assert_eq!(reply.as_deref(), Some("Understood."));
        assert!(
            !agent.awaiting_approval(),
            "the pending action must be gone"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            0,
            "the dropped action must not run"
        );
        let placeholder = agent.messages[index].text();
        assert!(
            placeholder.contains("dropped because the user asked something else"),
            "{placeholder}"
        );
        assert!(
            !placeholder.contains("Pending confirmation for:"),
            "no stale placeholder may remain: {placeholder}"
        );
        assert_eq!(
            speech.try_recv().ok().as_deref(),
            Some("I dropped that pending action because you asked something else."),
            "the user hears one short note"
        );
        assert_eq!(
            last_user_text(&requests),
            "open Firefox",
            "the new request runs in the same turn"
        );
        Ok(())
    }

    #[tokio::test]
    async fn denial_with_an_instruction_denies_and_answers_the_instruction() -> Result<()> {
        let (base, requests) = serve_tool_call_then_reply(Value::Null, final_reply()).await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);
        let index = arm_shell_approval(&mut agent);

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle(
                "no, open Firefox".into(),
                None,
                &CancellationToken::new(),
                None,
            ),
        )
        .await??;

        assert_eq!(reply.as_deref(), Some("Understood."));
        assert!(!agent.awaiting_approval());
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(agent.messages[index].text().contains("declined"));
        assert_eq!(
            last_user_text(&requests),
            "open Firefox",
            "only the instruction reaches the model"
        );
        Ok(())
    }

    #[tokio::test]
    async fn bare_denial_keeps_the_short_refusal_reply() -> Result<()> {
        let (base, requests) = serve_tool_call_then_reply(Value::Null, final_reply()).await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);
        let index = arm_shell_approval(&mut agent);

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle("no thanks".into(), None, &CancellationToken::new(), None),
        )
        .await??;

        assert_eq!(
            reply.as_deref(),
            Some("Understood. I did not perform that action.")
        );
        assert!(!agent.awaiting_approval());
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(agent.messages[index].text().contains("declined"));
        assert!(
            requests.lock().unwrap().is_empty(),
            "a bare refusal needs no model call"
        );
        Ok(())
    }

    #[tokio::test]
    async fn plain_yes_executes_the_stored_call_once() -> Result<()> {
        let (base, _requests) = serve_tool_call_then_reply(Value::Null, final_reply()).await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);
        let index = arm_shell_approval(&mut agent);

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle("yes".into(), None, &CancellationToken::new(), None),
        )
        .await??;

        assert_eq!(reply.as_deref(), Some("Understood."));
        assert!(!agent.awaiting_approval());
        assert_eq!(
            executions.load(Ordering::SeqCst),
            1,
            "the approved call runs exactly once"
        );
        assert!(agent.messages[index].text().contains("ran"));
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_turn_keeps_the_confirmation_armed() -> Result<()> {
        let (base, _requests) = serve_tool_call_then_reply(Value::Null, final_reply()).await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);
        arm_shell_approval(&mut agent);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle("open Firefox".into(), None, &cancel, None),
        )
        .await??;

        assert!(reply.unwrap().contains("clear yes or no"));
        assert!(
            agent.awaiting_approval(),
            "a cancelled turn must not silently drop the call"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn a_batch_keeps_its_skipped_call_after_the_cancellation() -> Result<()> {
        // Two calls in one batch: the first arms a confirmation, the second is
        // skipped. An unrelated request then drops the armed call; the skipped
        // record stays truthful instead of turning into a stale prompt.
        let (base, requests) = serve_tool_call_then_reply(
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
                {"id":"call-shell","type":"function","function":{"name":"shell","arguments":"{\"command\":\"rm -rf build\"}"}},
                {"id":"call-media","type":"function","function":{"name":"media","arguments":"{\"action\":\"play\"}"}}
            ]},"finish_reason":"tool_calls"}]}),
            final_reply(),
        )
        .await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle(
                "delete the build folder".into(),
                None,
                &CancellationToken::new(),
                None,
            ),
        )
        .await??;
        assert!(reply.is_some());
        assert!(agent.awaiting_approval());
        assert!(agent.messages.iter().any(|message| message.role == "tool"
            && message
                .text()
                .contains("Skipped because an earlier action in this batch")));

        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            agent.handle("open Firefox".into(), None, &CancellationToken::new(), None),
        )
        .await??;

        assert!(reply.is_some());
        assert!(!agent.awaiting_approval());
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(agent.messages.iter().any(|message| message.role == "tool"
            && message
                .text()
                .contains("dropped because the user asked something else")));
        assert!(
            !agent.messages.iter().any(|message| message.role == "tool"
                && message.text().contains("Pending confirmation for:")),
            "no stale confirmation placeholder after the drop"
        );
        assert_eq!(last_user_text(&requests), "open Firefox");
        Ok(())
    }

    #[tokio::test]
    async fn a_turn_grant_covers_matching_calls_and_expires_with_the_turn() -> Result<()> {
        let (base, _requests) = serve_scripted(vec![
            shell_call("call-a", "rm -rf build"),
            final_reply(),
            shell_call("call-b", "rm -rf other"),
            final_reply(),
            shell_call("call-c", "rm -rf third"),
            final_reply(),
        ])
        .await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);

        // Turn 1: the first shell call is held.
        agent
            .handle(
                "delete the build folder".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        assert!(agent.awaiting_approval());
        assert_eq!(executions.load(Ordering::SeqCst), 0);

        // Turn 2: "allow for this turn" runs the stored call and covers the
        // next call with the same shape.
        agent
            .handle(
                "allow for this turn".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        assert!(
            !agent.awaiting_approval(),
            "no second card for the same shape"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            2,
            "the approved call and the matching follow-up both ran"
        );
        assert!(
            agent.turn_grants.is_empty(),
            "the grant is dropped when the turn ends"
        );

        // Turn 3: the same shape asks again.
        agent
            .handle(
                "delete the third folder".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        assert!(
            agent.awaiting_approval(),
            "a new turn asks again for the same shape"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        Ok(())
    }

    struct BrowserStub(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for BrowserStub {
        fn name(&self) -> &str {
            "browser"
        }

        fn description(&self) -> &str {
            "Test browser mutation."
        }

        async fn execute(&self, _input: &str) -> Result<ToolResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success("clicked"))
        }
    }

    fn browser_click(id: &str, selector: &str) -> Value {
        json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":id,"type":"function","function":{"name":"browser","arguments":json!({"action":"click","target_id":"tab-1","selector":selector}).to_string()}}
        ]},"finish_reason":"tool_calls"}]})
    }

    #[tokio::test]
    async fn a_turn_grant_covers_repeated_browser_mutations() -> Result<()> {
        let (base, _requests) = serve_scripted(vec![
            browser_click("call-a", "#go"),
            final_reply(),
            browser_click("call-b", "#next"),
            final_reply(),
            browser_click("call-c", "#third"),
            final_reply(),
        ])
        .await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(BrowserStub(Arc::clone(&executions)));
        let mut agent = Agent::new(
            OpenRouterClient::new("local-test".into(), base),
            "test-model".into(),
            tools,
        );
        agent.messages = vec![Message::system("Test")];

        // Turn 1: a click is held and the preview names the action and the
        // target, so the user approves the mutation rather than a paraphrase.
        agent
            .handle(
                "click the Go button".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        assert!(agent.awaiting_approval());
        assert_eq!(
            agent.pending_approval_preview().as_deref(),
            Some("browser action=click target_id=tab-1 selector=#go")
        );
        assert_eq!(executions.load(Ordering::SeqCst), 0);

        // Turn 2: the grant covers the stored click and the next one, which is
        // a different selector but the same mutation.
        agent
            .handle(
                "allow for this turn".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        assert!(
            !agent.awaiting_approval(),
            "the click grant covers the next click"
        );
        assert_eq!(
            executions.load(Ordering::SeqCst),
            2,
            "the approved click and the matching follow-up both ran"
        );

        // Turn 3: a new turn asks again.
        agent
            .handle(
                "click the third link".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        assert!(
            agent.awaiting_approval(),
            "a new turn asks again for the same mutation"
        );
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[tokio::test]
    async fn a_turn_grant_does_not_cover_a_different_program() -> Result<()> {
        let (base, _requests) = serve_scripted(vec![
            shell_call("call-a", "rm -rf build"),
            final_reply(),
            shell_call("call-b", "python3 deploy.py"),
            final_reply(),
        ])
        .await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);

        agent
            .handle(
                "delete the build folder".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        agent
            .handle(
                "allow for this turn".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;

        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert!(
            agent.awaiting_approval(),
            "a different program still needs its own confirmation"
        );
        assert_eq!(
            agent.pending_approval_preview().as_deref(),
            Some("shell command=python3 deploy.py")
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_once_only_approval_grants_nothing() -> Result<()> {
        let (base, _requests) = serve_scripted(vec![
            shell_call("call-a", "rm -rf build"),
            final_reply(),
            shell_call("call-b", "rm -rf other"),
            final_reply(),
        ])
        .await?;
        let executions = Arc::new(AtomicUsize::new(0));
        let mut agent = shell_agent(base, &executions);

        agent
            .handle(
                "delete the build folder".into(),
                None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        agent
            .handle("yes".into(), None, &CancellationToken::new(), None)
            .await?;

        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert!(
            agent.awaiting_approval(),
            "a once-only approval must not cover the next call"
        );
        Ok(())
    }

    #[test]
    fn turn_grants_are_keyed_by_shape_and_cleared_by_the_turn_boundary() {
        let mut tools = ToolRegistry::new();
        tools.register(ShellStub(Arc::new(AtomicUsize::new(0))));
        let mut agent = Agent::new(
            OpenRouterClient::new("test-key".into(), "http://127.0.0.1".into()),
            "test-model".into(),
            tools,
        );
        agent.grant_for_turn("shell", r#"{"command":"git push origin main"}"#);
        agent.grant_for_turn("shell", r#"{"command":"rm -rf build"}"#);
        assert!(agent.turn_granted("shell", r#"{"command":"rm -rf other"}"#));
        assert!(agent.turn_granted("shell", r#"{"command":"git push --force"}"#));
        assert!(!agent.turn_granted("shell", r#"{"command":"cargo test"}"#));
        assert!(!agent.turn_granted("shell", r#"{"command":"ls -la"}"#));
        assert!(!agent.turn_granted("window", r#"{"action":"close"}"#));
        // The same shape is not duplicated.
        agent.grant_for_turn("shell", r#"{"command":"rm -rf again"}"#);
        assert_eq!(agent.turn_grants.len(), 2);
        // A call whose shape cannot be determined is never granted.
        agent.grant_for_turn("shell", r#"{"command":"rm -rf x && echo done"}"#);
        assert!(!agent.turn_granted("shell", r#"{"command":"rm -rf x && echo done"}"#));
        assert_eq!(agent.turn_grants.len(), 2);
    }

    #[test]
    fn tool_boundary_compaction_respects_a_pending_approval() {
        let mut agent = Agent::new(
            OpenRouterClient::new("test-key".into(), "http://127.0.0.1".into()),
            "test-model".into(),
            ToolRegistry::new(),
        );
        agent.messages = vec![Message::system("system")];
        for turn in 0..8 {
            agent.messages.push(Message::user(format!("task {turn}")));
            agent.messages.push(Message::assistant("z".repeat(20_000)));
        }
        agent.messages.push(Message::user("active task"));
        agent
            .messages
            .push(Message::tool_result("call-live", "w".repeat(20_000)));
        let before = agent.messages.len();

        agent.control().set_pending(Some(PendingApproval {
            name: "shell".into(),
            arguments: "{\"command\":\"dangerous\"}".into(),
            preview: crate::tools::safety::preview("shell", "{\"command\":\"dangerous\"}"),
            call_id: "pending-call".into(),
            result_index: 2,
            expires_at: std::time::Instant::now() + Duration::from_secs(60),
        }));
        assert!(!agent.compact_at_tool_boundary());
        assert_eq!(agent.messages.len(), before, "indices must stay stable");

        agent.control().set_pending(None);
        assert!(agent.compact_at_tool_boundary());
        assert!(agent.messages.len() < before);
        assert!(agent
            .messages
            .iter()
            .any(|message| message.text() == "active task"));
    }

    #[test]
    fn an_expired_confirmation_is_disarmed_and_recorded() {
        let mut agent = Agent::new(
            OpenRouterClient::new("test-key".into(), "http://127.0.0.1".into()),
            "test-model".into(),
            ToolRegistry::new(),
        );
        agent.messages = vec![
            Message::system("system"),
            Message::user("delete the build folder"),
            Message::tool_result("call-1", "This action needs explicit user confirmation."),
        ];
        agent.control().set_pending(Some(PendingApproval {
            name: "shell".into(),
            arguments: "{\"command\":\"rm -rf build\"}".into(),
            preview: crate::tools::safety::preview("shell", "{\"command\":\"rm -rf build\"}"),
            call_id: "call-1".into(),
            result_index: 2,
            expires_at: Instant::now() + Duration::from_secs(60),
        }));

        assert!(
            agent.expire_stale_approval().is_none(),
            "a live deadline keeps the confirmation armed"
        );
        assert!(agent.awaiting_approval());
        assert!(agent.messages[2]
            .text()
            .contains("needs explicit user confirmation"));

        let mut expired_pending = agent.control().take_pending().expect("armed");
        expired_pending.expires_at = Instant::now() - Duration::from_secs(1);
        agent.control().set_pending(Some(expired_pending));
        let message = agent.expire_stale_approval().expect("expired");
        assert!(message.contains("expired"));
        assert!(!agent.awaiting_approval());
        assert!(agent.approval_deadline().is_none());
        assert_eq!(agent.reply_needs_follow_up(), Some(false));

        // The placeholder result now records that nothing ran, keeping the
        // assistant tool-call batch complete.
        assert_eq!(agent.messages[2].tool_call_id.as_deref(), Some("call-1"));
        assert!(agent.messages[2].text().contains("expired"));
    }

    #[tokio::test]
    async fn expired_or_cancelled_tool_does_not_start_a_ready_side_effect() {
        let executions = Arc::new(AtomicUsize::new(0));
        let mut tools = ToolRegistry::new();
        tools.register(CountingTool(Arc::clone(&executions)));
        let cancel = CancellationToken::new();
        let (result, timed_out) =
            super::execute_tool_with_deadline(&tools, "media", "{}", &cancel, Duration::ZERO)
                .await
                .unwrap();
        assert!(!result.success);
        assert!(timed_out);
        cancel.cancel();
        let result = super::execute_tool(&tools, "media", "{}", &cancel)
            .await
            .unwrap();
        assert!(!result.success);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fast_command_cancellation_interrupts_action_and_marks_outcome_uncertain() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let mut tools = ToolRegistry::new();
        tools.register(HangingTool {
            calls: Arc::clone(&calls),
            started: Arc::clone(&started),
        });
        let mut agent = Agent::new(
            OpenRouterClient::new("test-key".into(), "http://127.0.0.1".into()),
            "test-model".into(),
            tools,
        );
        agent.loop_limits = AgentLoopLimits {
            max_rounds: 2,
            max_duration: Duration::from_secs(5),
            max_repeated_rounds: 2,
        };
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let result = agent.try_fast_command("volume up", &task_cancel).await;
            (agent, result)
        });

        started.notified().await;
        cancel.cancel();
        let (agent, result) = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.as_deref(), Some(""));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(agent.has_paused_task());
        assert_eq!(
            agent.action_journal.last().unwrap().status,
            "in_flight; outcome uncertain after interruption"
        );
    }

    #[tokio::test]
    async fn fast_command_deadline_stops_action_without_falling_back_to_model() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let mut tools = ToolRegistry::new();
        tools.register(HangingTool {
            calls: Arc::clone(&calls),
            started,
        });
        let mut agent = Agent::new(
            OpenRouterClient::new("test-key".into(), "http://127.0.0.1".into()),
            "test-model".into(),
            tools,
        );
        agent.loop_limits = AgentLoopLimits {
            max_rounds: 2,
            max_duration: Duration::from_millis(25),
            max_repeated_rounds: 2,
        };

        let result = agent
            .try_fast_command("volume up", &CancellationToken::new())
            .await;

        assert!(result
            .as_deref()
            .is_some_and(|text| text.contains("time limit")));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(agent.has_paused_task());
        assert_eq!(
            agent.action_journal.last().unwrap().status,
            "in_flight; outcome uncertain after interruption"
        );
    }

    #[test]
    fn splits_image_marker_from_output() {
        let (text, path) = split_image_marker("line one\nHYUSK_IMAGE:/tmp/a.png\nline two");

        assert_eq!(path.as_deref(), Some("/tmp/a.png"));
        assert!(text.contains("line one"));
        assert!(text.contains("line two"));
        assert!(!text.contains("HYUSK_IMAGE"));
    }

    #[test]
    fn confirmation_is_explicit_and_action_specific() {
        assert!(confirms("confirm"));
        assert!(confirms("yes"));
        assert!(confirms("yes."));
        assert!(confirms("okay, confirm. go ahead."));
        assert!(confirms("go ahead."));
        assert!(!confirms("yes, but not yet"));
        assert!(is_denial("No."));
    }

    #[test]
    fn voice_session_stop_is_exact_not_part_of_a_task() {
        assert!(is_voice_session_stop("Go away."));
        assert!(is_voice_session_stop("Hyusk, go away"));
        assert!(!is_voice_session_stop("Search for the song Go Away"));
    }

    #[test]
    fn parses_structured_reply_intent_and_strips_envelope() {
        let (text, needs_reply) = parse_structured_reply(
            r#"{"text":"Please confirm before I continue.","needs_reply":true,"reply_type":"confirmation"}"#,
        );

        assert_eq!(text, "Please confirm before I continue.");
        assert_eq!(needs_reply, Some(true));
    }

    #[test]
    fn falls_back_for_plain_provider_output() {
        let (text, needs_reply) = parse_structured_reply("Done.");

        assert_eq!(text, "Done.");
        assert_eq!(needs_reply, None);
    }

    #[test]
    fn strips_duplicate_prose_and_fenced_response_envelope() {
        let (text, needs_reply) = parse_structured_reply(
            "Would you like me to launch it?\n\n```json\n{\"text\":\"Would you like me to launch it?\",\"needs_reply\":true,\"reply_type\":\"confirmation\"}\n```",
        );
        assert_eq!(text, "Would you like me to launch it?");
        assert_eq!(needs_reply, Some(true));
    }

    #[test]
    fn preserves_ordinary_json_examples_in_prose() {
        let raw = "An example:\n```json\n{\"text\":\"example\"}\n```";
        assert_eq!(parse_structured_reply(raw), (raw.to_string(), None));
    }

    #[test]
    fn sonic_prompt_never_requests_spoken_json() {
        let voice = build_system_prompt("", "hello", true);
        let chat = build_system_prompt("", "hello", false);
        assert!(voice.contains("Speak only the natural answer"));
        assert!(!voice.contains("Response protocol (required)"));
        assert!(chat.contains("Response protocol (required)"));
    }

    #[test]
    fn repeated_tool_rounds_require_unchanged_output() {
        let mut progress = ToolProgress::default();
        let first = vec![("status".to_string(), "still running".to_string())];

        assert_eq!(progress.observe(first.clone()), 1);
        assert_eq!(progress.observe(first), 2);
        assert_eq!(
            progress.observe(vec![("status".to_string(), "finished".to_string())]),
            1
        );
    }

    #[test]
    fn tool_call_signature_normalizes_json_whitespace() {
        assert_eq!(
            tool_call_signature("status", &json!({"job": 7}), r#" { "job": 7 } "#),
            "status:{\"job\":7}"
        );
    }

    #[test]
    fn loop_limits_are_long_running_but_bounded() {
        let limits = AgentLoopLimits {
            max_rounds: 24,
            max_duration: Duration::from_secs(600),
            max_repeated_rounds: 3,
        };

        assert!(limits.max_rounds > limits.max_repeated_rounds);
        assert!(limits.max_duration > Duration::from_secs(60));
    }
}
