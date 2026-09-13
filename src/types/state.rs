#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyuskState {
    Hidden,
    Waking,
    Listening,
    Thinking,
    Working,
    Speaking,
}

impl HyuskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "Hidden",
            Self::Waking => "Waking",
            Self::Listening => "Listening",
            Self::Thinking => "Thinking",
            Self::Working => "Working",
            Self::Speaking => "Speaking",
        }
    }

    /// Publish the state for external status indicators.
    ///
    /// The GNOME Shell indicator watches `$XDG_RUNTIME_DIR/hyusk-state` and
    /// updates its color/label when this changes.
    pub fn publish(self) {
        let directory = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let path = std::path::Path::new(&directory).join("hyusk-state");

        let _ = std::fs::write(path, self.as_str());
        crate::status::state(self.as_str());
    }
}
