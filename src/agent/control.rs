//! State the event loop must read or change while a turn is running.
//!
//! A turn owns the agent mutex for its whole model/tool loop
//! (`runtime::run_turn` locks it and calls `Agent::handle`), so anything the
//! runtime needs in the middle of a turn lives here instead, behind its own
//! short-lived lock: the armed confirmation, the selected provider and clients,
//! and the paused-task flag. Messages, the tool loop, and the journal stay with
//! the turn.
//!
//! # Lock order
//!
//! The agent mutex may be held while this lock is taken - a turn reads and
//! writes its control state in small scopes - but this lock is never held
//! across an `.await` and never while waiting for the agent mutex. Every scope
//! here is a single statement or a small block, so the event loop can always
//! take it immediately. A guard that escaped into an `await` would reintroduce
//! the stall this split removes.

use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use crate::model::{
    bedrock_sonic::BedrockSonicClient, codex::CodexClient, openrouter::OpenRouterClient,
    SharedActiveModel,
};

/// A confirmation waiting for the user, with the exact call that will run.
#[derive(Debug, Clone)]
pub(crate) struct PendingApproval {
    pub(crate) name: String,
    pub(crate) arguments: String,
    /// The exact call as the user sees it. Derived from `arguments`, which are
    /// the stored arguments, so the approved call is the previewed call.
    pub(crate) preview: String,
    pub(crate) call_id: String,
    pub(crate) result_index: usize,
    pub(crate) expires_at: Instant,
}

/// Approval, model-selection, and paused-task state shared with the runtime.
pub(crate) struct AgentControl {
    provider: String,
    model: String,
    pending: Option<PendingApproval>,
    task_paused: bool,
    /// The client the next text request uses. `None` only before a client is
    /// selected, which `select_model` refuses to leave in place.
    text_client: Option<OpenRouterClient>,
    openrouter: Option<OpenRouterClient>,
    openai: Option<OpenRouterClient>,
    bedrock: Option<OpenRouterClient>,
    sonic: Option<Arc<BedrockSonicClient>>,
    active_api_model: Option<SharedActiveModel>,
    codex: Option<CodexClient>,
}

/// Shareable handle used by the runtime alongside the agent mutex.
pub(crate) type ControlHandle = Arc<Mutex<AgentControl>>;

pub(crate) fn new_control(
    provider: impl Into<String>,
    model: impl Into<String>,
    text_client: Option<OpenRouterClient>,
    task_paused: bool,
) -> ControlHandle {
    Arc::new(Mutex::new(AgentControl {
        provider: provider.into(),
        model: model.into(),
        pending: None,
        task_paused,
        text_client,
        openrouter: None,
        openai: None,
        bedrock: None,
        sonic: None,
        active_api_model: None,
        codex: None,
    }))
}

/// Lock the control state.
///
/// A poisoned lock is recovered instead of propagated: a panic while holding a
/// short-lived scope must not make every later control read fail, because that
/// would leave the event loop unable to disarm an approval.
pub(crate) fn lock(control: &ControlHandle) -> MutexGuard<'_, AgentControl> {
    control
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AgentControl {
    pub(crate) fn provider(&self) -> String {
        self.provider.clone()
    }

    pub(crate) fn model(&self) -> String {
        self.model.clone()
    }

    pub(crate) fn uses_sonic(&self) -> bool {
        self.provider == "bedrock-sonic"
    }

    /// Record the selected provider and model without touching the clients.
    pub(crate) fn set_identity(&mut self, provider: &str, model: &str) {
        self.provider = provider.to_string();
        self.model = model.to_string();
    }

    pub(crate) fn text_client(&self) -> Option<OpenRouterClient> {
        self.text_client.clone()
    }

    pub(crate) fn set_text_client(&mut self, client: Option<OpenRouterClient>) {
        self.text_client = client;
    }

    /// The client configured for a named provider, if any.
    pub(crate) fn provider_client(&self, provider: &str) -> Option<OpenRouterClient> {
        match provider {
            "openrouter" => self.openrouter.clone(),
            "openai" => self.openai.clone(),
            "bedrock" => self.bedrock.clone(),
            _ => None,
        }
    }

    /// Register a provider client created after startup, so a key saved from
    /// the indicator is usable without restarting the service.
    pub(crate) fn set_provider_client(&mut self, provider: &str, client: Option<OpenRouterClient>) {
        match provider {
            "openrouter" => self.openrouter = client,
            "openai" => self.openai = client,
            "bedrock" => self.bedrock = client,
            _ => {}
        }
    }

    pub(crate) fn sonic_client(&self) -> Option<Arc<BedrockSonicClient>> {
        self.sonic.clone()
    }

    pub(crate) fn set_sonic_client(&mut self, client: Option<Arc<BedrockSonicClient>>) {
        self.sonic = client;
    }

    pub(crate) fn codex_client(&self) -> Option<CodexClient> {
        self.codex.clone()
    }

    pub(crate) fn has_codex_client(&self) -> bool {
        self.codex.is_some()
    }

    pub(crate) fn set_codex_client(&mut self, client: Option<CodexClient>) {
        self.codex = client;
    }

    pub(crate) fn set_active_api_model(&mut self, active: SharedActiveModel) {
        self.active_api_model = Some(active);
    }

    pub(crate) fn select_api_model(
        &self,
        provider: &str,
        model: &str,
        client: &OpenRouterClient,
    ) -> anyhow::Result<()> {
        if let Some(active) = &self.active_api_model {
            active.select(provider, model, client.clone())?;
        }
        Ok(())
    }

    pub(crate) fn armed(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn pending_cloned(&self) -> Option<PendingApproval> {
        self.pending.clone()
    }

    pub(crate) fn take_pending(&mut self) -> Option<PendingApproval> {
        self.pending.take()
    }

    pub(crate) fn set_pending(&mut self, pending: Option<PendingApproval>) {
        self.pending = pending;
    }

    /// The preview text of the armed confirmation, for cards and tests.
    pub(crate) fn preview(&self) -> Option<String> {
        self.pending.as_ref().map(|pending| pending.preview.clone())
    }

    /// The deadline of the armed confirmation, if any.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.expires_at)
    }

    pub(crate) fn clear_pending_approval(&mut self) -> bool {
        self.pending.take().is_some()
    }

    pub(crate) fn task_paused(&self) -> bool {
        self.task_paused
    }

    pub(crate) fn set_task_paused(&mut self, paused: bool) {
        self.task_paused = paused;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn approval() -> PendingApproval {
        PendingApproval {
            name: "shell".into(),
            arguments: "{\"command\":\"rm -rf build\"}".into(),
            preview: "shell rm -rf build".into(),
            call_id: "call-1".into(),
            result_index: 0,
            expires_at: Instant::now() + Duration::from_secs(60),
        }
    }

    #[test]
    fn arming_and_disarming_an_approval_is_one_short_step() {
        let control = new_control("openrouter", "model", None, false);

        assert!(!lock(&control).armed());
        lock(&control).set_pending(Some(approval()));
        {
            let guard = lock(&control);
            assert!(guard.armed());
            assert_eq!(guard.preview().as_deref(), Some("shell rm -rf build"));
            assert!(guard.deadline().is_some());
        }

        assert!(lock(&control).clear_pending_approval());
        assert!(!lock(&control).armed());
        assert!(!lock(&control).clear_pending_approval());
    }

    #[test]
    fn a_saved_provider_client_is_visible_without_the_agent() {
        let control = new_control("openrouter", "model", None, false);
        assert!(lock(&control).provider_client("openai").is_none());

        lock(&control).set_provider_client(
            "openai",
            Some(OpenRouterClient::new(
                "saved-later".into(),
                "https://example.test/v1".into(),
            )),
        );

        assert!(lock(&control).provider_client("openai").is_some());
        assert!(lock(&control).provider_client("openrouter").is_none());
        assert_eq!(lock(&control).provider(), "openrouter");
        assert!(!lock(&control).uses_sonic());
    }

    #[test]
    fn a_poisoned_control_still_answers_the_event_loop() {
        let control = new_control("openrouter", "model", None, false);
        lock(&control).set_pending(Some(approval()));

        let poison = Arc::clone(&control);
        let _ = std::thread::spawn(move || {
            let _guard = lock(&poison);
            panic!("poison the control lock");
        })
        .join();

        // The event loop can still disarm the approval after a panic.
        assert!(lock(&control).clear_pending_approval());
    }
}
