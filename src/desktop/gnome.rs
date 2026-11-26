//! The GNOME adapter: the Hyusk GNOME Shell extension's D-Bus interface.
//!
//! Moved out of `window.rs` unchanged. [`ShellApi`] is the one seam: a recorded
//! payload can exercise parsing, rendering, and action replies without a live
//! extension.

use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use super::{Bounds, Window, WindowCapabilities, WindowList, WindowsBackend};

const SERVICE: &str = "org.hyusk.Shell";
const PATH: &str = "/org/hyusk/Shell";
const INTERFACE: &str = "org.hyusk.Shell";
const DBUS_CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// The three D-Bus methods the adapter calls, with their one optional string
/// argument.
#[async_trait]
pub trait ShellApi: Send + Sync {
    async fn call(&self, method: &str, argument: Option<&str>) -> Result<String>;
}

/// The production transport: one session-bus method call.
pub struct DbusShell;

#[async_trait]
impl ShellApi for DbusShell {
    async fn call(&self, method: &str, argument: Option<&str>) -> Result<String> {
        let result = tokio::time::timeout(DBUS_CALL_TIMEOUT, async {
            let connection = zbus::Connection::session()
                .await
                .context("failed to connect to the session D-Bus")?;
            match argument {
                Some(argument) => {
                    connection
                        .call_method(
                            Some(SERVICE),
                            PATH,
                            Some(INTERFACE),
                            method,
                            &argument,
                        )
                        .await
                }
                None => {
                    connection
                        .call_method(Some(SERVICE), PATH, Some(INTERFACE), method, &())
                        .await
                }
            }
            .context("D-Bus method call failed")
        })
        .await
        .with_context(|| {
            if matches!(method, "ActivateWindow" | "CloseWindow") {
                format!(
                    "{SERVICE}.{method} timed out after {} seconds; the window action outcome is unknown, inspect the window list before retrying",
                    DBUS_CALL_TIMEOUT.as_secs()
                )
            } else {
                format!(
                    "{SERVICE}.{method} timed out after {} seconds",
                    DBUS_CALL_TIMEOUT.as_secs()
                )
            }
        })?
        .with_context(|| {
            format!(
                "{SERVICE}.{method} failed. Is the Hyusk GNOME extension \
                 (hyusk@hyusk.local) installed and enabled? On Wayland it is \
                 the only reliable way to switch windows."
            )
        });
        let reply = result?;

        reply
            .body()
            .deserialize::<String>()
            .context("unexpected reply from the Hyusk shell extension")
    }
}

pub struct GnomeBackend<S> {
    shell: S,
}

impl<S> GnomeBackend<S> {
    pub const fn new(shell: S) -> Self {
        Self { shell }
    }
}

impl From<&Value> for Window {
    fn from(value: &Value) -> Self {
        let bounds = value.get("bounds").and_then(|bounds| {
            Some(Bounds {
                x: bounds.get("x")?.as_i64()?,
                y: bounds.get("y")?.as_i64()?,
                width: bounds.get("width")?.as_u64()?,
                height: bounds.get("height")?.as_u64()?,
            })
        });

        Self {
            id: value.get("id").and_then(Value::as_u64),
            app: value
                .get("app")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            active: value
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            workspace: value.get("workspace").and_then(Value::as_u64),
            monitor: value.get("monitor").and_then(Value::as_i64),
            minimized: value.get("minimized").and_then(Value::as_bool),
            on_current_workspace: value.get("on_current_workspace").and_then(Value::as_bool),
            fullscreen: value.get("fullscreen").and_then(Value::as_bool),
            bounds,
        }
    }
}

fn parse_windows(raw: &str) -> Result<Vec<Value>> {
    serde_json::from_str(raw).context("invalid window list reply from the Hyusk shell extension")
}

fn parse_action_reply(raw: &str, action: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(raw).with_context(|| {
        format!(
            "invalid {action} reply from the Hyusk shell extension; the action may already have been applied, inspect the window list before retrying"
        )
    })?;

    if !value.is_object() || value.get("ok").and_then(Value::as_bool).is_none() {
        anyhow::bail!(
            "malformed {action} reply from the Hyusk shell extension; the action may already have been applied, inspect the window list before retrying"
        );
    }

    Ok(value)
}

fn render_list(windows: &[Window]) -> String {
    if windows.is_empty() {
        return "No windows reported.".to_string();
    }
    let mut lines = vec![format!("Windows ({}):", windows.len())];
    for window in windows {
        let id = window
            .id
            .map_or_else(|| "?".to_string(), |id| id.to_string());
        let bounds = window
            .bounds
            .map(|bounds| {
                format!(
                    " at {},{} {}x{}",
                    bounds.x, bounds.y, bounds.width, bounds.height
                )
            })
            .unwrap_or_default();
        lines.push(format!(
            "{}[id:{}] {} — {}{}{}",
            if window.active { "* " } else { "  " },
            id,
            window.app,
            window.title,
            bounds,
            if window.minimized == Some(true) {
                " (minimized)"
            } else {
                ""
            }
        ));
    }
    lines.join("\n")
}

#[async_trait]
impl<S: ShellApi + Send + Sync> WindowsBackend for GnomeBackend<S> {
    fn source(&self) -> &'static str {
        "GNOME Shell window metadata"
    }

    fn capabilities(&self) -> WindowCapabilities {
        WindowCapabilities {
            list: true,
            active: true,
            activate: true,
            close: true,
        }
    }

    async fn list(&self) -> Result<WindowList> {
        let raw = self.shell.call("ListWindows", None).await?;
        let windows: Vec<Window> = parse_windows(&raw)?.iter().map(Window::from).collect();
        Ok(WindowList {
            rendered: render_list(&windows),
            windows,
        })
    }

    async fn active(&self) -> Result<String> {
        self.shell.call("ActiveWindow", None).await
    }

    async fn activate(&self, query: &str) -> Result<String> {
        let raw = self.shell.call("ActivateWindow", Some(query)).await?;
        let value = parse_action_reply(&raw, "activate")?;

        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            let title = value.get("title").and_then(Value::as_str).unwrap_or(query);
            Ok(title.to_string())
        } else {
            anyhow::bail!(
                "{}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("no window matched the query")
            )
        }
    }

    async fn close(&self, query: &str) -> Result<String> {
        let raw = self.shell.call("CloseWindow", Some(query)).await?;
        let value = parse_action_reply(&raw, "close")?;

        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            let title = value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("window");
            Ok(title.to_string())
        } else {
            anyhow::bail!(
                "{}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("no window matched the query")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    const RECORDED_WINDOWS: &str = r#"[
        {"id":9,"title":"~","app":"Alacritty","pid":123,"bounds":{"x":0,"y":0,"width":800,"height":600},"minimized":false,"active":true,"workspace":0,"on_current_workspace":true,"monitor":0,"fullscreen":false},
        {"id":5,"title":"Docs - Brave","app":"brave-browser","pid":124,"bounds":{"x":100,"y":50,"width":1200,"height":800},"minimized":true,"active":false,"workspace":1,"on_current_workspace":false,"monitor":0,"fullscreen":false}
    ]"#;

    #[derive(Default)]
    struct Inner {
        responses: Mutex<VecDeque<Result<String>>>,
        calls: Mutex<Vec<(String, Option<String>)>>,
    }

    #[derive(Clone, Default)]
    struct Recorded {
        inner: Arc<Inner>,
    }

    impl Recorded {
        fn with(responses: Vec<Result<String>>) -> (Self, Self) {
            let recorded = Self::default();
            recorded.inner.responses.lock().unwrap().extend(responses);
            (recorded.clone(), recorded)
        }

        fn calls(&self) -> Vec<(String, Option<String>)> {
            self.inner.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ShellApi for Recorded {
        async fn call(&self, method: &str, argument: Option<&str>) -> Result<String> {
            self.inner
                .calls
                .lock()
                .unwrap()
                .push((method.to_string(), argument.map(str::to_string)));
            self.inner
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected D-Bus call: {method}"))
        }
    }

    #[tokio::test]
    async fn list_renders_the_recorded_extension_payload_exactly_as_before() {
        let (shell, recorded) = Recorded::with(vec![Ok(RECORDED_WINDOWS.to_string())]);
        let backend = GnomeBackend::new(shell);

        let list = backend.list().await;
        assert!(
            list.is_ok(),
            "the recorded window list must parse: {:?}",
            list.err()
        );
        let list = list.unwrap();

        assert_eq!(
            list.rendered,
            "Windows (2):\n\
             * [id:9] Alacritty — ~ at 0,0 800x600\n  \
             [id:5] brave-browser — Docs - Brave at 100,50 1200x800 (minimized)"
        );
        assert_eq!(list.windows.len(), 2);
        assert_eq!(list.windows[0].workspace, Some(0));
        assert_eq!(list.windows[0].bounds.unwrap().width, 800);
        assert_eq!(list.windows[1].minimized, Some(true));
        assert_eq!(
            recorded.calls(),
            vec![("ListWindows".to_string(), None)],
            "list reads one snapshot from the extension"
        );
    }

    #[tokio::test]
    async fn activate_and_close_return_the_extension_title() {
        let (shell, recorded) = Recorded::with(vec![
            Ok(r#"{"ok":true,"id":9,"title":"~","app":"Alacritty"}"#.to_string()),
            Ok(r#"{"ok":true,"id":5,"title":"Docs - Brave","app":"brave-browser"}"#.to_string()),
        ]);
        let backend = GnomeBackend::new(shell);

        let activated = backend.activate("alacritty").await;
        assert!(activated.is_ok(), "activate reply: {:?}", activated.err());
        assert_eq!(activated.unwrap(), "~");

        let closed = backend.close("brave").await;
        assert!(closed.is_ok(), "close reply: {:?}", closed.err());
        assert_eq!(closed.unwrap(), "Docs - Brave");

        assert_eq!(
            recorded.calls(),
            vec![
                ("ActivateWindow".to_string(), Some("alacritty".to_string())),
                ("CloseWindow".to_string(), Some("brave".to_string())),
            ]
        );
    }

    #[tokio::test]
    async fn a_false_ok_reply_surfaces_the_extension_error() {
        let (shell, _) = Recorded::with(vec![Ok(
            r#"{"ok":false,"error":"no window matching 'ghost'"}"#.to_string(),
        )]);

        let result = GnomeBackend::new(shell).activate("ghost").await;

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "no window matching 'ghost'"
        );
    }

    #[tokio::test]
    async fn active_returns_the_extension_reply_unchanged() {
        let reply = r#"{"id":9,"title":"~","app":"Alacritty","workspace":0}"#;
        let (shell, recorded) = Recorded::with(vec![Ok(reply.to_string())]);

        let active = GnomeBackend::new(shell).active().await;

        assert!(active.is_ok(), "active reply: {:?}", active.err());
        assert_eq!(active.unwrap(), reply);
        assert_eq!(recorded.calls(), vec![("ActiveWindow".to_string(), None)]);
    }

    #[tokio::test]
    async fn a_malformed_action_reply_warns_against_unsafe_retries() {
        let (shell, _) = Recorded::with(vec![Ok("not json".to_string())]);
        let error = GnomeBackend::new(shell).close("id:9").await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("inspect the window list before retrying"),
            "{error}"
        );

        let (shell, _) = Recorded::with(vec![Ok("[]".to_string())]);
        assert!(GnomeBackend::new(shell).close("id:9").await.is_err());
    }
}
