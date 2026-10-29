//! In-process record of the OS capabilities the desktop tools depend on (D3).
//!
//! GNOME gates synthetic input and screen capture behind portals, and the
//! accessibility tree depends on a working AT-SPI bridge. Those checks live in
//! the operating system, so an approved call can still fail after the user has
//! already confirmed it. This module records what each backend has told us, so
//! the runner returns a capability error instead of publishing a card for a
//! call that cannot run, and so the model and `gnome_doctor` see the same
//! one-line state.
//!
//! Only outcomes are recorded, never credentials or screen content. The record
//! is process-local and never persisted.

use serde_json::Value;

/// What a backend last reported about one capability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Capability {
    /// Nothing has been attempted yet; the call may try.
    #[default]
    Unknown,
    /// The OS granted the capability.
    Granted,
    /// The user or the OS refused it.
    Denied,
    /// This session has no working backend for it.
    Unsupported,
}

impl Capability {
    /// The lowercase name used in the one-line summary.
    pub fn label(self) -> &'static str {
        match self {
            Capability::Unknown => "unknown",
            Capability::Granted => "granted",
            Capability::Denied => "denied",
            Capability::Unsupported => "unsupported",
        }
    }
}

/// One capability and the backend that reported it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapabilityState {
    /// The last reported state.
    pub capability: Capability,
    /// The backend the state refers to, for example "RemoteDesktop portal".
    pub backend: Option<String>,
}

impl CapabilityState {
    fn record(&mut self, capability: Capability, backend: impl Into<String>) {
        self.capability = capability;
        self.backend = Some(backend.into());
    }

    fn render(&self) -> String {
        match self.backend.as_deref() {
            Some(backend) if !backend.is_empty() => {
                format!("{} ({backend})", self.capability.label())
            }
            _ => self.capability.label().to_string(),
        }
    }

    /// A refusal for a call that needs this capability, or `None` while the
    /// state is unknown or granted. Unknown allows the attempt, as recorded in
    /// the issue.
    fn refusal(&self, kind: &str) -> Option<String> {
        let backend = self.backend.as_deref().unwrap_or("this session");
        match self.capability {
            Capability::Denied => Some(format!(
                "{kind} capability denied ({backend}). The desktop refused it, so the call was \
                 not attempted and no approval card was armed. Ask the user to grant {kind} \
                 access, then retry."
            )),
            Capability::Unsupported => Some(format!(
                "{kind} capability unsupported ({backend}). This session has no working {kind} \
                 backend, so the call was not attempted and no approval card was armed."
            )),
            Capability::Granted | Capability::Unknown => None,
        }
    }
}

/// The capability record: one state per capability the desktop tools need.
#[derive(Clone, Debug, Default)]
pub struct CapabilityRegistry {
    input: CapabilityState,
    screenshot: CapabilityState,
    accessibility: CapabilityState,
}

impl CapabilityRegistry {
    pub fn record_input(&mut self, capability: Capability, backend: impl Into<String>) {
        self.input.record(capability, backend);
    }

    pub fn record_screenshot(&mut self, capability: Capability, backend: impl Into<String>) {
        self.screenshot.record(capability, backend);
    }

    pub fn record_accessibility(&mut self, capability: Capability, backend: impl Into<String>) {
        self.accessibility.record(capability, backend);
    }

    /// The one line the model prompt and `gnome_doctor` both show.
    pub fn line(&self) -> String {
        format!(
            "capabilities: input {}; screenshot {}; accessibility {}",
            self.input.render(),
            self.screenshot.render(),
            self.accessibility.render()
        )
    }

    /// True when a recorded capability is denied or unsupported.
    pub fn has_failure(&self) -> bool {
        [&self.input, &self.screenshot, &self.accessibility]
            .iter()
            .any(|state| {
                matches!(
                    state.capability,
                    Capability::Denied | Capability::Unsupported
                )
            })
    }

    /// Why this call cannot run, or `None` when it may proceed. Only a denied
    /// or unsupported capability refuses; an unknown one allows the attempt.
    pub fn refusal(&self, name: &str, arguments: &str) -> Option<String> {
        let Ok(args) = serde_json::from_str::<Value>(arguments) else {
            return None;
        };
        match name {
            "computer" => computer_refusal(self, &args),
            "accessibility" => self.accessibility.refusal("accessibility"),
            _ => None,
        }
    }
}

/// The capability a computer action needs first, if any. A batch is checked
/// step by step, so a denied capability cannot hide behind a free first step.
fn computer_refusal(registry: &CapabilityRegistry, args: &Value) -> Option<String> {
    match action(args) {
        "batch" => args
            .get("steps")
            .and_then(Value::as_array)
            .and_then(|steps| {
                steps
                    .iter()
                    .find_map(|step| computer_refusal(registry, step))
            }),
        "mouse_move" | "mouse_click" | "mouse_drag" | "scroll" | "type_text" | "key_press"
        | "key_combo" => registry.input.refusal("input"),
        "click_text" => registry
            .input
            .refusal("input")
            .or_else(|| registry.screenshot.refusal("screenshot")),
        "screenshot" | "ocr" | "find_text" | "app_state" => {
            registry.screenshot.refusal("screenshot")
        }
        _ => None,
    }
}

fn action(args: &Value) -> &str {
    args.get("action").and_then(Value::as_str).unwrap_or("")
}

/// The process-wide record, shared by the tools, the runner, and the doctor.
fn shared() -> &'static std::sync::Mutex<CapabilityRegistry> {
    static SHARED: std::sync::OnceLock<std::sync::Mutex<CapabilityRegistry>> =
        std::sync::OnceLock::new();
    SHARED.get_or_init(|| std::sync::Mutex::new(CapabilityRegistry::default()))
}

fn lock() -> std::sync::MutexGuard<'static, CapabilityRegistry> {
    shared()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Record the input state a backend just reported, for example "RemoteDesktop
/// portal". Called by the computer tool when it learns the state.
pub fn record_input(capability: Capability, backend: impl Into<String>) {
    lock().record_input(capability, backend);
}

/// Record the screenshot state a capture backend just reported.
pub fn record_screenshot(capability: Capability, backend: impl Into<String>) {
    lock().record_screenshot(capability, backend);
}

/// Record the accessibility state the AT-SPI bridge reported.
pub fn record_accessibility(capability: Capability, backend: impl Into<String>) {
    lock().record_accessibility(capability, backend);
}

/// Why this call cannot run right now, or `None` when it may proceed.
pub fn refusal(name: &str, arguments: &str) -> Option<String> {
    lock().refusal(name, arguments)
}

/// The one-line capability state for the prompt and `gnome_doctor`.
pub fn line() -> String {
    lock().line()
}

/// A copy of the process-wide record, for the doctor's report.
pub fn snapshot() -> CapabilityRegistry {
    lock().clone()
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    *lock() = CapabilityRegistry::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(action: &str) -> String {
        serde_json::json!({ "action": action }).to_string()
    }

    #[test]
    fn state_transitions_are_recorded_with_their_backend() {
        let mut registry = CapabilityRegistry::default();
        assert_eq!(
            registry.line(),
            "capabilities: input unknown; screenshot unknown; accessibility unknown"
        );
        // Unknown allows the attempt.
        assert!(registry.refusal("computer", &call("mouse_click")).is_none());

        registry.record_input(Capability::Granted, "RemoteDesktop portal");
        assert_eq!(registry.input.capability, Capability::Granted);
        assert_eq!(
            registry.input.backend.as_deref(),
            Some("RemoteDesktop portal")
        );
        assert!(registry.refusal("computer", &call("type_text")).is_none());

        registry.record_input(Capability::Denied, "RemoteDesktop portal");
        let refusal = registry
            .refusal("computer", &call("mouse_click"))
            .expect("a denied input must refuse");
        assert!(
            refusal.starts_with("input capability denied (RemoteDesktop portal)"),
            "{refusal}"
        );
        assert!(refusal.contains("no approval card"), "{refusal}");
        // Read-only calls and other tools are unaffected.
        assert!(registry.refusal("computer", &call("status")).is_none());
        assert!(registry.refusal("computer", &call("open_url")).is_none());
        assert!(registry.refusal("window", &call("active")).is_none());
        assert!(registry.refusal("shell", r#"{"command":"ls"}"#).is_none());

        registry.record_input(Capability::Unsupported, "native input");
        let refusal = registry
            .refusal("computer", &call("key_press"))
            .expect("an unsupported input must refuse");
        assert!(
            refusal.starts_with("input capability unsupported (native input)"),
            "{refusal}"
        );

        registry.record_input(Capability::Unknown, "no attempt yet");
        assert!(registry.refusal("computer", &call("scroll")).is_none());

        registry.record_screenshot(Capability::Granted, "grim");
        assert!(
            registry.line().contains("screenshot granted (grim)"),
            "{}",
            registry.line()
        );
    }

    #[test]
    fn screenshot_and_accessibility_refusals_cover_their_calls() {
        let mut registry = CapabilityRegistry::default();
        registry.record_screenshot(Capability::Denied, "desktop portal");
        for action in ["screenshot", "ocr", "find_text", "app_state"] {
            let refusal = registry
                .refusal("computer", &call(action))
                .unwrap_or_else(|| panic!("{action} must refuse"));
            assert!(
                refusal.starts_with("screenshot capability denied"),
                "{refusal}"
            );
        }
        // `click_text` needs both: the input denial is reported first.
        registry.record_input(Capability::Denied, "RemoteDesktop portal");
        let refusal = registry.refusal("computer", &call("click_text")).unwrap();
        assert!(refusal.starts_with("input capability denied"), "{refusal}");

        registry.record_accessibility(Capability::Unsupported, "AT-SPI bridge");
        let refusal = registry
            .refusal("accessibility", r#"{"action":"read"}"#)
            .expect("a missing bridge must refuse accessibility");
        assert!(
            refusal.starts_with("accessibility capability unsupported"),
            "{refusal}"
        );
        // Other tools are never refused by an accessibility state.
        assert!(registry
            .refusal("window", r#"{"action":"active"}"#)
            .is_none());
    }

    #[test]
    fn a_batch_cannot_hide_a_call_whose_capability_is_denied() {
        let mut registry = CapabilityRegistry::default();
        registry.record_input(Capability::Denied, "RemoteDesktop portal");
        let batch = serde_json::json!({
            "action": "batch",
            "steps": [
                {"action": "screenshot"},
                {"action": "mouse_click", "x": 10, "y": 20},
            ],
        })
        .to_string();
        let refusal = registry
            .refusal("computer", &batch)
            .expect("the click refuses");
        assert!(refusal.starts_with("input capability denied"), "{refusal}");

        // A batch of free steps is unaffected.
        let batch = serde_json::json!({
            "action": "batch",
            "steps": [{"action": "wait", "milliseconds": 10}],
        })
        .to_string();
        assert!(registry.refusal("computer", &batch).is_none());
        assert!(registry.refusal("computer", "not json").is_none());
    }
}
