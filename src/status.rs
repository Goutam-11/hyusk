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
        let _ = std::fs::write(path(), json);
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
pub fn card(kind: &str, text: impl Into<String>, persistent: bool) {
    if let Ok(mut s) = store().lock() {
        s.latest = Some(Card {
            kind: kind.to_string(),
            text: text.into(),
            persistent,
        });
        save(&mut s);
    }
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
