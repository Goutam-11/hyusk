//! Which desktop tools can actually work on this machine (W5).
//!
//! A tool that cannot serve a call should not be offered: the model plans
//! around it, spends a round trip finding out, and the D3 refusal only fires
//! after the call. Each probe here is cheap (a loopback connect or an
//! environment check) and answers "why not" as well as "not", so the prompt can
//! say what is missing instead of silently shrinking the tool list.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::OnceLock;
use std::time::Duration;

/// The longest a loopback connection attempt may take.
const PROBE_TIMEOUT: Duration = Duration::from_millis(150);

/// Why `browser` is unavailable, or `None` when CDP answers.
///
/// The endpoint is the one the tool itself would use; a listener that refuses
/// the connection means the automation browser is not running.
pub fn browser_reason(endpoint: &str) -> Option<String> {
    let addresses = match endpoint.to_socket_addrs() {
        Ok(addresses) => addresses.collect::<Vec<_>>(),
        Err(error) => {
            return Some(format!(
                "the CDP endpoint {endpoint} is not an address: {error}"
            ));
        }
    };

    for address in addresses {
        if TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok() {
            return None;
        }
    }

    Some(format!(
        "no CDP listener on {endpoint}; start the automation browser with its script"
    ))
}

/// Why `accessibility` is unavailable, or `None` when it can be used.
pub fn accessibility_reason(bridge_exists: bool, has_session: bool) -> Option<String> {
    if !bridge_exists {
        return Some("the AT-SPI bridge script is missing".to_string());
    }

    if !has_session {
        return Some("no display session (DISPLAY and WAYLAND_DISPLAY are unset)".to_string());
    }

    None
}

/// The whole "why not" set, as it was probed at startup.
fn snapshot() -> &'static Vec<(&'static str, String)> {
    static SNAPSHOT: OnceLock<Vec<(&'static str, String)>> = OnceLock::new();

    SNAPSHOT.get_or_init(|| {
        let mut reasons = Vec::new();

        let endpoint = browser_endpoint();
        if let Some(reason) = browser_reason(&endpoint) {
            reasons.push(("browser", reason));
        }

        let bridge = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts/atspi_bridge.py")
            .is_file();
        let session =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        if let Some(reason) = accessibility_reason(bridge, session) {
            reasons.push(("accessibility", reason));
        }

        if let Some(reason) = crate::desktop::session().window_unavailable_reason() {
            reasons.push(("window", reason));
        }

        reasons
    })
}

/// The absent set, for the prompt line.
pub fn absent_snapshot() -> Vec<(&'static str, String)> {
    snapshot().clone()
}

fn browser_endpoint() -> String {
    let raw = std::env::var("HYUSK_BROWSER_CDP_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9222".to_string());

    // The probe only needs host:port; the scheme and any path are the client's
    // concern, and adding the `url` crate for that would be a dependency for
    // one string operation.
    let without_scheme = raw.split_once("://").map_or(raw.as_str(), |(_, rest)| rest);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .to_string()
}

pub fn reason(tool: &str) -> Option<&'static str> {
    snapshot()
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, reason)| reason.as_str())
}

/// One prompt line naming every absent tool and its reason.
pub fn format_absent(reasons: &[(&str, String)]) -> String {
    reasons
        .iter()
        .map(|(tool, reason)| format!("{tool} ({reason})"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn browser_reason_reports_no_listener_on_the_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let open = format!("127.0.0.1:{port}");

        assert_eq!(browser_reason(&open), None, "a listening port is reachable");

        drop(listener);

        let reason = browser_reason(&open).expect("a closed port must be reported");
        assert!(reason.contains(&port.to_string()), "{reason}");
    }

    #[test]
    fn accessibility_reason_names_the_missing_piece() {
        assert!(accessibility_reason(false, true).is_some());
        assert!(accessibility_reason(true, false).is_some());
        assert_eq!(accessibility_reason(true, true), None);
    }

    /// Guard: the prompt line is the only place a user learns a tool is gone.
    #[test]
    fn absent_line_names_each_tool_and_reason() {
        let line = format_absent(&[
            ("browser", "no CDP listener".to_string()),
            ("window", "no desktop".to_string()),
        ]);

        assert!(line.contains("browser (no CDP listener)"), "{line}");
        assert!(line.contains("window (no desktop)"), "{line}");
        assert!(format_absent(&[]).is_empty());
    }
}
