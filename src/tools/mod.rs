#[cfg(target_os = "linux")]
pub mod accessibility;
pub mod computer;
pub mod media;
pub mod memory;
#[cfg(target_os = "linux")]
pub mod portal;
pub mod process;
pub mod registry;
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
