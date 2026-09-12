use super::HyuskState;

#[derive(Debug, Clone)]
pub enum HyuskEvent {
    // Wake system
    WakeWordDetected,

    // User interaction
    UserInput(String),

    // UI state
    StateChanged(HyuskState),

    // Agent/tool activity
    ToolStarted { name: String },

    ToolFinished { name: String, success: bool },

    // Agent output
    Response(String),

    // System
    Shutdown,
}
