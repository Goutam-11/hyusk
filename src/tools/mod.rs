#[cfg(target_os = "linux")]
pub mod accessibility;
pub mod browser;
pub mod capabilities;
pub mod computer;
pub mod external;
#[cfg(target_os = "linux")]
pub mod gnome_doctor;
pub mod manifest;
pub mod mcp;
pub mod media;
pub mod memory;
#[cfg(feature = "mobile")]
pub mod mobile;
#[cfg(target_os = "linux")]
pub mod portal;
pub mod process;
pub mod redact;
pub mod registry;
pub mod safety;
pub mod scheduler;
pub mod shell;
pub mod task;
pub mod timer;
pub mod tool;
pub mod web_search;
#[cfg(target_os = "linux")]
pub mod window;

pub use registry::ToolRegistry;
pub use tool::{Tool, ToolResult};

/// The longest wait a model may ask for, in seconds (L4).
///
/// Timers and schedules both sleep for a model-supplied duration, and the
/// deadline conversion overflows - and panics the worker - long before
/// `u64::MAX`. The bound is one documented value: the tool schema advertises
/// it, the tools reject anything longer with a clear failure, and the scheduler
/// never sleeps past it even for a schedule loaded from an older file.
pub const MAX_WAIT_SECONDS: u64 = 30 * 24 * 60 * 60;

/// A human-readable form of [`MAX_WAIT_SECONDS`] for tool results.
pub const MAX_WAIT_LABEL: &str = "30 days";
