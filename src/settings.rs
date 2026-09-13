use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub struct ModelSelection {
    pub provider: String,
    pub model: String,
}

fn path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join("hyusk").join("config.json")
}

pub fn load() -> Option<ModelSelection> {
    serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()
}
pub fn save(selection: &ModelSelection) {
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec_pretty(selection) {
        let _ = std::fs::write(path, json);
    }
}
