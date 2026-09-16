use super::HyuskState;

#[derive(Debug, Clone)]
pub enum HyuskEvent {
    // Wake system
    WakeWordDetected,

    // User interaction
    UserInput(String),

    // Continue a just-finished voice conversation without another wake word.
    ContinueListening,
    StopListening,

    // UI state
    StateChanged(HyuskState),

    // Agent/tool activity
    ToolStarted {
        name: String,
    },

    ToolFinished {
        name: String,
        success: bool,
    },

    // Agent output
    Response(String),

    // A configured background subagent finished work. This is kept separate
    // from UserInput so it never interrupts the user's current request.
    SubtaskFinished {
        id: u64,
        label: String,
        success: bool,
        summary: String,
    },

    // A persisted schedule became due. Unlike direct user input, this is
    // queued behind an active foreground turn instead of interrupting it.
    ScheduledWorkflow {
        id: u64,
        name: String,
    },
    ScheduledAgentTask {
        id: u64,
        prompt: String,
    },

    TurnFinished {
        id: u64,
    },

    // Requested by the GNOME indicator's emergency-stop menu item.
    StopRequested,

    ModelSelected {
        provider: String,
        model: String,
    },

    // System
    Shutdown,
}
