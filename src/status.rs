//! Small runtime JSON bridge consumed by the GNOME Shell extension.

use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize)]
pub struct Status {
    revision: u64,
    state: String,
    active_provider: String,
    active_model: String,
    awaiting_reply: bool,
    latest: Option<Card>,
    catalogs: Vec<Catalog>,
    timer: Option<Timer>,
}

#[derive(Clone, Serialize)]
struct Card {
    id: u64,
    kind: String,
    text: String,
    persistent: bool,
}

#[derive(Clone, Serialize)]
struct Timer {
    label: String,
    ends_at_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Catalog {
    pub provider: String,
    pub models: Vec<Model>,
    pub fresh: bool,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Model {
    pub id: String,
    pub label: String,
}

fn store() -> &'static Mutex<Status> {
    static STORE: OnceLock<Mutex<Status>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Status::default()))
}

fn publisher() -> &'static tokio::sync::broadcast::Sender<Status> {
    static PUBLISHER: OnceLock<tokio::sync::broadcast::Sender<Status>> = OnceLock::new();
    PUBLISHER.get_or_init(|| tokio::sync::broadcast::channel(32).0)
}

pub fn subscribe() -> tokio::sync::broadcast::Receiver<Status> {
    publisher().subscribe()
}
pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string()))
        .join("hyusk-status.json")
}

fn save(status: &mut Status) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    status.revision = status.revision.max(now).saturating_add(1);
    if let Ok(json) = serde_json::to_vec(status) {
        let path = path();
        let _ = std::fs::write(&path, json);
        // The fallback location is /tmp, which other accounts can read (X4).
        crate::paths::restrict_file(&path);
    }
    let _ = publisher().send(status.clone());
}
pub fn state(value: &str) {
    if let Ok(mut s) = store().lock() {
        s.state = value.to_string();
        save(&mut s);
    }
}
pub fn model(provider: &str, model: &str) {
    if let Ok(mut s) = store().lock() {
        s.active_provider = provider.to_string();
        s.active_model = model.to_string();
        save(&mut s);
    }
}
/// The card kind every mode publishes while a tool confirmation is armed.
///
/// The GNOME response card and the Noctalia tooltip show Approve/Deny for this
/// kind only, so a mode that invents another tag silently loses the buttons.
pub const APPROVAL_CARD_KIND: &str = "approval";

pub fn card(kind: &str, text: impl Into<String>, persistent: bool) {
    if let Ok(mut s) = store().lock() {
        s.latest = Some(Card {
            id: s.revision.saturating_add(1),
            kind: kind.to_string(),
            text: scrub_card_text(text.into()),
            persistent,
        });
        save(&mut s);
    }
}

/// Card text is shown on screen and written to `hyusk-status.json`, so it never
/// carries a secret-shaped value or a full path to a credential (X4).
///
/// The pass is silent: a count note belongs on tool output, not on a card the
/// user is reading. It runs before the card reaches the store, the file, and
/// the panel subscribers, so none of them can see the unedited text.
fn scrub_card_text(text: String) -> String {
    let (text, _) = crate::tools::redact::redact(&text);
    let (text, _) = crate::tools::safety::redact_secret_paths(&text);
    text
}
pub fn awaiting_reply(value: bool) {
    if let Ok(mut s) = store().lock() {
        s.awaiting_reply = value;
        save(&mut s);
    }
}
pub fn dismiss() {
    if let Ok(mut s) = store().lock() {
        s.latest = None;
        save(&mut s);
    }
}
pub fn catalogs(catalogs: Vec<Catalog>) {
    if let Ok(mut s) = store().lock() {
        s.catalogs = catalogs;
        save(&mut s);
    }
}
pub fn timer(label: impl Into<String>, ends_at_ms: u64) {
    if let Ok(mut s) = store().lock() {
        s.timer = Some(Timer {
            label: label.into(),
            ends_at_ms,
        });
        save(&mut s);
    }
}
pub fn clear_timer() {
    if let Ok(mut s) = store().lock() {
        s.timer = None;
        save(&mut s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_text_loses_secret_values_and_credential_paths() {
        let scrubbed = scrub_card_text(
            "ran OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef and read /home/me/.ssh/id_ed25519."
                .to_string(),
        );

        assert!(
            !scrubbed.contains("sk-or-v1-0123456789abcdef"),
            "{scrubbed}"
        );
        assert!(!scrubbed.contains("/home/me/.ssh/id_ed25519"), "{scrubbed}");
        assert!(scrubbed.contains("[redacted]"), "{scrubbed}");
        assert!(scrubbed.contains("[redacted path]"), "{scrubbed}");
        // The rest of the sentence, including the final period, is untouched.
        assert!(scrubbed.starts_with("ran "), "{scrubbed}");
        assert!(scrubbed.ends_with("."), "{scrubbed}");

        // A URL that carries a credential in its userinfo is scrubbed too.
        let scrubbed = scrub_card_text(
            "filling https://oauth2:ghp_0123456789abcdef@example.test/login".to_string(),
        );
        assert!(!scrubbed.contains("ghp_0123456789abcdef"), "{scrubbed}");
        assert!(scrubbed.contains("oauth2:[redacted]@"), "{scrubbed}");
    }

    #[test]
    fn the_stored_card_and_its_json_lose_the_secret() {
        // The store is process-global and other tests publish cards, so the
        // card is read back through the same lock the writer holds.
        card(
            "response",
            "OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef for /home/me/.env",
            false,
        );

        let stored = {
            let status = store().lock().expect("status store");
            let card = status.latest.as_ref().expect("a card is stored");
            assert!(
                !card.text.contains("sk-or-v1-0123456789abcdef"),
                "{}",
                card.text
            );
            assert!(!card.text.contains("/home/me/.env"), "{}", card.text);
            assert!(card.text.contains("[redacted]"), "{}", card.text);
            assert!(card.text.contains("[redacted path]"), "{}", card.text);
            serde_json::to_string(&*status).expect("status serializes")
        };
        assert!(!stored.contains("sk-or-v1-0123456789abcdef"), "{stored}");

        dismiss();
    }
}
