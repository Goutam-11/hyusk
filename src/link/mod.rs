//! The laptop/mobile link protocol and its TLS WebSocket transport.
//!
//! The module is intentionally independent from `main.rs`.  The application can
//! create the channels in [`LinkChannels`], call [`start_link_server`], and map
//! the typed [`LinkCommand`] values into the agent/runtime when it is ready.

mod auth;
mod protocol;
mod server;

pub use protocol::*;
pub use server::{start_link_server, LinkServer};

use crate::types::HyuskEvent;
use tokio::sync::{broadcast, mpsc};

/// Commands which need an application/runtime decision rather than a direct
/// `HyuskEvent` mapping.
#[derive(Debug, Clone)]
pub enum LinkCommand {
    Invoke(InvokeRequest),
    InvocationResult(InvocationResult),
    Approval(ApprovalResponse),
    MemorySync(MemorySyncRequest),
    WorkflowSync(WorkflowSyncRequest),
}

/// Channels used by the transport.  `outbound` is a broadcast sender so each
/// connected phone receives laptop notifications without a per-client router.
pub struct LinkChannels {
    pub events: mpsc::Sender<HyuskEvent>,
    pub commands: mpsc::Sender<LinkCommand>,
    pub outbound: broadcast::Sender<OutboundNotification>,
}

impl LinkChannels {
    pub fn new(
        events: mpsc::Sender<HyuskEvent>,
        commands: mpsc::Sender<LinkCommand>,
        outbound: broadcast::Sender<OutboundNotification>,
    ) -> Self {
        Self {
            events,
            commands,
            outbound,
        }
    }
}
