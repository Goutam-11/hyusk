//! The niri adapter: niri's own IPC, through `niri msg`.
//!
//! Moved out of `window.rs` unchanged. The only seam added for tests is
//! [`NiriRunner`], which lets a recorded `niri msg --json windows` payload and
//! a recorded action reply exercise the adapter without a compositor.

use std::collections::BTreeMap;
use std::process::Output;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{MoveDestination, Window, WindowCapabilities, WindowList, WindowsBackend};

/// The one process call the adapter makes.
#[async_trait]
pub trait NiriRunner: Send + Sync {
    async fn run(&self, arguments: &[String]) -> std::io::Result<Output>;
}

/// The production runner: one `niri` child per call.
pub struct CommandRunner;

#[async_trait]
impl NiriRunner for CommandRunner {
    async fn run(&self, arguments: &[String]) -> std::io::Result<Output> {
        tokio::process::Command::new("niri")
            .args(arguments)
            .output()
            .await
    }
}

pub struct NiriBackend<R> {
    runner: R,
}

impl<R> NiriBackend<R> {
    pub const fn new(runner: R) -> Self {
        Self { runner }
    }
}

/// One window as `niri msg --json windows` reports it.
#[derive(Debug, Deserialize)]
struct NiriWindow {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    app_id: String,
    #[serde(default)]
    workspace_id: Option<u64>,
    #[serde(default)]
    is_focused: bool,
    #[serde(default)]
    focus_timestamp: Option<NiriTimestamp>,
}

#[derive(Debug, Deserialize)]
struct NiriWorkspace {
    id: u64,
    idx: u64,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NiriOutput {
    name: String,
    #[serde(default)]
    logical: Option<NiriLogical>,
}

#[derive(Debug, Deserialize)]
struct NiriLogical {
    width: u64,
    height: u64,
}

#[derive(Debug, Deserialize)]
struct NiriTimestamp {
    secs: u64,
    #[serde(default)]
    nanos: u64,
}

impl From<&NiriWindow> for Window {
    fn from(window: &NiriWindow) -> Self {
        Self {
            id: Some(window.id),
            app: window.app_id.clone(),
            title: window.title.clone(),
            active: window.is_focused,
            workspace: window.workspace_id,
            ..Self::default()
        }
    }
}

impl<R: NiriRunner + Send + Sync> NiriBackend<R> {
    async fn windows(&self) -> Result<Vec<NiriWindow>> {
        let arguments = vec![
            "msg".to_string(),
            "--json".to_string(),
            "windows".to_string(),
        ];
        let output = self
            .runner
            .run(&arguments)
            .await
            .context("could not run `niri msg`")?;
        if !output.status.success() {
            anyhow::bail!(
                "`niri msg --json windows` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        serde_json::from_slice(&output.stdout).context("invalid window list from niri")
    }

    async fn workspaces(&self) -> Result<Vec<NiriWorkspace>> {
        let arguments = vec![
            "msg".to_string(),
            "--json".to_string(),
            "workspaces".to_string(),
        ];
        let output = self
            .runner
            .run(&arguments)
            .await
            .context("could not run `niri msg`")?;
        if !output.status.success() {
            anyhow::bail!(
                "`niri msg --json workspaces` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        serde_json::from_slice(&output.stdout).context("invalid workspace list from niri")
    }

    async fn output_map(&self) -> Result<BTreeMap<String, NiriOutput>> {
        let arguments = vec![
            "msg".to_string(),
            "--json".to_string(),
            "outputs".to_string(),
        ];
        let output = self
            .runner
            .run(&arguments)
            .await
            .context("could not run `niri msg`")?;
        if !output.status.success() {
            anyhow::bail!(
                "`niri msg --json outputs` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        serde_json::from_slice(&output.stdout).context("invalid output list from niri")
    }

    async fn action(&self, arguments: &[&str]) -> Result<()> {
        let mut full = vec!["msg".to_string(), "action".to_string()];
        full.extend(arguments.iter().map(|argument| argument.to_string()));
        let output = self
            .runner
            .run(&full)
            .await
            .context("could not run `niri msg action`")?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            anyhow::bail!(
                "niri rejected `{}`: {}",
                arguments.join(" "),
                if detail.is_empty() {
                    "no detail"
                } else {
                    &detail
                }
            );
        }
        Ok(())
    }
}

/// Pick the window a query means.
///
/// `id:<n>` addresses one window exactly. Everything else is a case-insensitive
/// substring of the app id or the title, with an exact app-id match first and
/// the most recently focused window breaking ties, so "alacritty" prefers the
/// window the user was last working in rather than an arbitrary one.
fn select_window<'a>(windows: &'a [NiriWindow], query: &str) -> Option<&'a NiriWindow> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return windows.iter().find(|window| window.is_focused);
    }
    if let Some(id) = query.strip_prefix("id:") {
        let id: u64 = id.trim().parse().ok()?;
        return windows.iter().find(|window| window.id == id);
    }
    let mut matches: Vec<&NiriWindow> = windows
        .iter()
        .filter(|window| {
            window.app_id.to_ascii_lowercase().contains(&query)
                || window.title.to_ascii_lowercase().contains(&query)
        })
        .collect();
    matches.sort_by_key(|window| {
        (
            window.app_id.to_ascii_lowercase() != query,
            std::cmp::Reverse(window.focus_timestamp.as_ref().map_or(0, |ts| ts.secs)),
        )
    });
    matches.first().copied()
}

fn render_list(windows: &[NiriWindow]) -> String {
    if windows.is_empty() {
        return "No windows reported.".to_string();
    }
    let mut lines = vec![format!("Windows ({}):", windows.len())];
    for window in windows {
        lines.push(format!(
            "{}[id:{}] {} — {}{}",
            if window.is_focused { "* " } else { "  " },
            window.id,
            window.app_id,
            window.title,
            window
                .workspace_id
                .map(|workspace| format!(" (workspace {workspace})"))
                .unwrap_or_default(),
        ));
    }
    lines.join("\n")
}

#[async_trait]
impl<R: NiriRunner + Send + Sync> WindowsBackend for NiriBackend<R> {
    fn source(&self) -> &'static str {
        "niri IPC"
    }

    fn capabilities(&self) -> WindowCapabilities {
        WindowCapabilities {
            list: true,
            active: true,
            activate: true,
            close: true,
            last: true,
            workspace: true,
            output: true,
        }
    }

    async fn list(&self, workspace: Option<u64>) -> Result<WindowList> {
        let windows = self.windows().await?;
        let windows: Vec<NiriWindow> = match workspace {
            Some(id) => windows
                .into_iter()
                .filter(|window| window.workspace_id == Some(id))
                .collect(),
            None => windows,
        };
        let rendered = if windows.is_empty() {
            match workspace {
                Some(id) => format!("No windows on workspace {id}."),
                None => "No windows reported.".to_string(),
            }
        } else {
            render_list(&windows)
        };
        Ok(WindowList {
            rendered,
            windows: windows.iter().map(Window::from).collect(),
        })
    }

    async fn active(&self) -> Result<String> {
        let windows = self.windows().await?;
        let window = windows
            .iter()
            .find(|window| window.is_focused)
            .ok_or_else(|| anyhow::anyhow!("No window is focused."))?;
        Ok(json!({
            "id": window.id,
            "app": window.app_id,
            "title": window.title,
            "workspace": window.workspace_id,
        })
        .to_string())
    }

    async fn activate(&self, query: &str) -> Result<String> {
        let windows = self.windows().await?;
        let window = select_window(&windows, query)
            .ok_or_else(|| anyhow::anyhow!("No window matched '{query}'."))?;
        let id = window.id.to_string();
        self.action(&["focus-window", "--id", &id]).await?;
        Ok(window.title.clone())
    }

    async fn close(&self, query: &str) -> Result<String> {
        let windows = self.windows().await?;
        let window = select_window(&windows, query)
            .ok_or_else(|| anyhow::anyhow!("No window matched '{query}'."))?;
        let id = window.id.to_string();
        self.action(&["close-window", "--id", &id]).await?;
        Ok(window.title.clone())
    }

    async fn last(&self) -> Result<String> {
        let windows = self.windows().await?;
        let previous = windows
            .iter()
            .filter(|window| !window.is_focused)
            .max_by_key(|window| {
                window
                    .focus_timestamp
                    .as_ref()
                    .map_or((0, 0), |timestamp| (timestamp.secs, timestamp.nanos))
            });
        let window = previous.ok_or_else(|| anyhow::anyhow!("No previous window."))?;
        let id = window.id.to_string();
        self.action(&["focus-window", "--id", &id]).await?;
        Ok(window.title.clone())
    }

    async fn move_to(&self, query: &str, destination: &MoveDestination) -> Result<String> {
        let windows = self.windows().await?;
        let window = select_window(&windows, query)
            .ok_or_else(|| anyhow::anyhow!("No window matched '{query}'."))?;
        let id = window.id.to_string();

        match destination {
            MoveDestination::Workspace(workspace_id) => {
                let workspaces = self.workspaces().await?;
                let workspace = workspaces
                    .iter()
                    .find(|workspace| workspace.id == *workspace_id)
                    .ok_or_else(|| anyhow::anyhow!("No workspace with id {workspace_id}."))?;
                let reference = workspace
                    .name
                    .clone()
                    .unwrap_or_else(|| workspace.idx.to_string());
                self.action(&["move-window-to-workspace", "--window-id", &id, &reference])
                    .await?;
            }
            MoveDestination::Output(name) => {
                let outputs = self.output_map().await?;
                if !outputs.contains_key(name) {
                    anyhow::bail!("No output named '{name}'.");
                }
                self.action(&["move-window-to-monitor", "--id", &id, name])
                    .await?;
            }
        }

        Ok(window.title.clone())
    }

    async fn outputs(&self) -> Result<String> {
        let outputs = self.output_map().await?;
        if outputs.is_empty() {
            return Ok("No outputs reported.".to_string());
        }
        let mut entries: Vec<(&String, &NiriOutput)> = outputs.iter().collect();
        entries.sort_by_key(|(name, _)| name.to_ascii_lowercase());
        let mut lines = vec![format!("Outputs ({}):", entries.len())];
        for (_, output) in entries {
            let size = output
                .logical
                .as_ref()
                .map(|logical| format!(" — {}x{}", logical.width, logical.height))
                .unwrap_or_default();
            lines.push(format!("  {}{size}", output.name));
        }
        Ok(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::{Arc, Mutex};

    const RECORDED_WINDOWS: &str = r#"[
        {"id":9,"title":"~","app_id":"Alacritty","workspace_id":1,"is_focused":true,"focus_timestamp":{"secs":200}},
        {"id":5,"title":"Docs - Brave","app_id":"brave-browser","workspace_id":2,"is_focused":false,"focus_timestamp":{"secs":100}}
    ]"#;

    fn output(stdout: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn failure(stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[derive(Default)]
    struct Inner {
        responses: Mutex<VecDeque<std::io::Result<Output>>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    #[derive(Clone, Default)]
    struct Recorded {
        inner: Arc<Inner>,
    }

    impl Recorded {
        fn with(responses: Vec<std::io::Result<Output>>) -> (Self, Self) {
            let recorded = Self::default();
            recorded.inner.responses.lock().unwrap().extend(responses);
            (recorded.clone(), recorded)
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.inner.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl NiriRunner for Recorded {
        async fn run(&self, arguments: &[String]) -> std::io::Result<Output> {
            self.inner.calls.lock().unwrap().push(arguments.to_vec());
            self.inner
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected niri call: {arguments:?}"))
        }
    }

    #[tokio::test]
    async fn list_renders_the_recorded_payload_exactly_as_before() {
        let (runner, recorded) = Recorded::with(vec![Ok(output(RECORDED_WINDOWS))]);
        let backend = NiriBackend::new(runner);

        let list = backend.list(None).await;
        assert!(
            list.is_ok(),
            "the recorded window list must parse: {:?}",
            list.err()
        );
        let list = list.unwrap();

        assert_eq!(
            list.rendered,
            "Windows (2):\n\
             * [id:9] Alacritty — ~ (workspace 1)\n  \
             [id:5] brave-browser — Docs - Brave (workspace 2)"
        );
        assert_eq!(list.windows.len(), 2);
        assert_eq!(list.windows[0].id, Some(9));
        assert_eq!(list.windows[0].app, "Alacritty");
        assert_eq!(list.windows[0].title, "~");
        assert_eq!(list.windows[0].workspace, Some(1));
        assert!(list.windows[0].active);
        assert_eq!(recorded.calls(), vec![vec!["msg", "--json", "windows"]]);
    }

    #[tokio::test]
    async fn list_filters_to_one_workspace_and_names_an_empty_result() {
        let (runner, recorded) = Recorded::with(vec![
            Ok(output(RECORDED_WINDOWS)),
            Ok(output(RECORDED_WINDOWS)),
        ]);
        let backend = NiriBackend::new(runner);

        let list = backend.list(Some(2)).await;
        assert!(
            list.is_ok(),
            "the recorded list must parse: {:?}",
            list.err()
        );
        let list = list.unwrap();
        assert_eq!(list.windows.len(), 1);
        assert_eq!(list.windows[0].id, Some(5));
        assert_eq!(
            list.rendered,
            "Windows (1):\n  [id:5] brave-browser — Docs - Brave (workspace 2)"
        );

        let empty = backend.list(Some(99)).await;
        assert!(empty.is_ok());
        let empty = empty.unwrap();
        assert!(empty.windows.is_empty());
        assert_eq!(empty.rendered, "No windows on workspace 99.");

        assert_eq!(
            recorded.calls(),
            vec![
                vec!["msg", "--json", "windows"],
                vec!["msg", "--json", "windows"],
            ]
        );
    }

    #[tokio::test]
    async fn activate_selects_the_best_match_and_focuses_its_id() {
        let (runner, recorded) = Recorded::with(vec![
            Ok(output(RECORDED_WINDOWS)),
            Ok(output("")),
            Ok(output(RECORDED_WINDOWS)),
            Ok(output("")),
        ]);
        let backend = NiriBackend::new(runner);

        let title = backend.activate("id:9").await;
        assert!(title.is_ok(), "an exact id must match: {:?}", title.err());
        assert_eq!(title.unwrap(), "~");

        // The app-name match prefers the more recently focused window, as the
        // old selector did, and still addresses it by stable id.
        let title = backend.activate("alacritty").await;
        assert!(title.is_ok(), "an app name must match: {:?}", title.err());
        assert_eq!(title.unwrap(), "~");

        assert_eq!(
            recorded.calls(),
            vec![
                vec!["msg", "--json", "windows"],
                vec!["msg", "action", "focus-window", "--id", "9"],
                vec!["msg", "--json", "windows"],
                vec!["msg", "action", "focus-window", "--id", "9"],
            ]
        );
    }

    #[tokio::test]
    async fn activate_with_no_match_reports_the_same_failure_without_acting() {
        let (runner, recorded) = Recorded::with(vec![Ok(output(RECORDED_WINDOWS))]);
        let backend = NiriBackend::new(runner);

        let result = backend.activate("nothing-here").await;

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "No window matched 'nothing-here'."
        );
        assert_eq!(
            recorded.calls(),
            vec![vec!["msg", "--json", "windows"]],
            "a failed match must not issue a compositor action"
        );
    }

    fn niri_window(id: u64, app: &str, title: &str, focused: bool, focused_at: u64) -> NiriWindow {
        NiriWindow {
            id,
            title: title.to_string(),
            app_id: app.to_string(),
            workspace_id: Some(1),
            is_focused: focused,
            focus_timestamp: Some(NiriTimestamp {
                secs: focused_at,
                nanos: 0,
            }),
        }
    }

    /// Guard (ported from the old `window` test when the selector moved): the
    /// id, title, exact-app, recency, and empty-query rules are unchanged.
    #[test]
    fn selection_preserves_ids_exact_apps_and_recent_focus() {
        let windows = vec![
            niri_window(3, "Alacritty", "Freebuff", false, 100),
            niri_window(9, "Alacritty", "~", true, 200),
            niri_window(5, "brave-browser", "Docs - Brave", false, 300),
        ];

        assert_eq!(select_window(&windows, "id:3").unwrap().id, 3);
        assert!(select_window(&windows, "id:404").is_none());
        assert_eq!(select_window(&windows, "alacritty").unwrap().id, 9);
        assert_eq!(select_window(&windows, "brave").unwrap().id, 5);
        assert_eq!(select_window(&windows, "FREEBUFF").unwrap().id, 3);
        assert_eq!(select_window(&windows, "").unwrap().id, 9);
        assert!(select_window(&windows, "nothing-here").is_none());
    }

    #[tokio::test]
    async fn last_focuses_the_most_recent_window_other_than_the_focused_one() {
        let payload = r#"[
            {"id":9,"title":"~","app_id":"Alacritty","workspace_id":1,"is_focused":true,"focus_timestamp":{"secs":300,"nanos":0}},
            {"id":5,"title":"Docs","app_id":"brave-browser","workspace_id":2,"is_focused":false,"focus_timestamp":{"secs":400,"nanos":100}},
            {"id":7,"title":"Notes","app_id":"gedit","workspace_id":1,"is_focused":false,"focus_timestamp":{"secs":400,"nanos":200}}
        ]"#;
        let (runner, recorded) = Recorded::with(vec![Ok(output(payload)), Ok(output(""))]);

        let title = NiriBackend::new(runner).last().await;

        assert!(
            title.is_ok(),
            "a recorded focus history must read: {:?}",
            title.err()
        );
        assert_eq!(title.unwrap(), "Notes");
        assert_eq!(
            recorded.calls(),
            vec![
                vec!["msg", "--json", "windows"],
                vec!["msg", "action", "focus-window", "--id", "7"],
            ]
        );

        // No window other than the focused one: report that, and do not
        // touch the compositor.
        let one = r#"[{"id":1,"title":"only","app_id":"app","is_focused":true,"focus_timestamp":{"secs":9}}]"#;
        let (runner, recorded) = Recorded::with(vec![Ok(output(one))]);
        let result = NiriBackend::new(runner).last().await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "No previous window.");
        assert_eq!(recorded.calls().len(), 1);
    }

    const RECORDED_OUTPUTS: &str = r#"{
        "eDP-1":{"name":"eDP-1","make":"Chimei Innolux Corporation","model":"0x1521","logical":{"x":0,"y":0,"width":1920,"height":1080,"scale":1.0}},
        "HDMI-A-1":{"name":"HDMI-A-1","make":"Dell","model":"U2720Q","logical":{"x":1920,"y":0,"width":2560,"height":1440,"scale":1.0}}
    }"#;

    #[tokio::test]
    async fn output_lists_the_recorded_outputs_with_their_logical_size() {
        let (runner, recorded) = Recorded::with(vec![Ok(output(RECORDED_OUTPUTS))]);

        let outputs = NiriBackend::new(runner).outputs().await;

        assert!(
            outputs.is_ok(),
            "the recorded outputs must parse: {:?}",
            outputs.err()
        );
        assert_eq!(
            outputs.unwrap(),
            "Outputs (2):\n  eDP-1 — 1920x1080\n  HDMI-A-1 — 2560x1440"
        );
        assert_eq!(recorded.calls(), vec![vec!["msg", "--json", "outputs"]]);
    }

    const RECORDED_WORKSPACES: &str = r#"[
        {"id":1,"idx":1,"name":null,"output":"eDP-1","is_active":true,"is_focused":true},
        {"id":2,"idx":2,"name":null,"output":"eDP-1","is_active":false,"is_focused":false}
    ]"#;

    #[tokio::test]
    async fn move_sends_the_matched_window_to_the_workspace_id() {
        let (runner, recorded) = Recorded::with(vec![
            Ok(output(RECORDED_WINDOWS)),
            Ok(output(RECORDED_WORKSPACES)),
            Ok(output("")),
        ]);
        let backend = NiriBackend::new(runner);

        let title = backend
            .move_to("brave", &MoveDestination::Workspace(2))
            .await;

        assert!(
            title.is_ok(),
            "a recorded workspace must move: {:?}",
            title.err()
        );
        assert_eq!(title.unwrap(), "Docs - Brave");
        assert_eq!(
            recorded.calls(),
            vec![
                vec!["msg", "--json", "windows"],
                vec!["msg", "--json", "workspaces"],
                vec![
                    "msg",
                    "action",
                    "move-window-to-workspace",
                    "--window-id",
                    "5",
                    "2"
                ],
            ]
        );
    }

    #[tokio::test]
    async fn move_to_an_unknown_workspace_reports_it_without_acting() {
        let (runner, recorded) = Recorded::with(vec![
            Ok(output(RECORDED_WINDOWS)),
            Ok(output(RECORDED_WORKSPACES)),
        ]);

        let result = NiriBackend::new(runner)
            .move_to("brave", &MoveDestination::Workspace(99))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "No workspace with id 99.");
        assert_eq!(
            recorded.calls().len(),
            2,
            "an unknown workspace must not reach the compositor action"
        );
    }

    #[tokio::test]
    async fn move_sends_the_matched_window_to_a_named_output() {
        let (runner, recorded) = Recorded::with(vec![
            Ok(output(RECORDED_WINDOWS)),
            Ok(output(RECORDED_OUTPUTS)),
            Ok(output("")),
        ]);

        let title = NiriBackend::new(runner)
            .move_to("brave", &MoveDestination::Output("HDMI-A-1".to_string()))
            .await;

        assert!(
            title.is_ok(),
            "a recorded output must move: {:?}",
            title.err()
        );
        assert_eq!(title.unwrap(), "Docs - Brave");
        assert_eq!(
            recorded.calls(),
            vec![
                vec!["msg", "--json", "windows"],
                vec!["msg", "--json", "outputs"],
                vec![
                    "msg",
                    "action",
                    "move-window-to-monitor",
                    "--id",
                    "5",
                    "HDMI-A-1"
                ],
            ]
        );
    }

    #[tokio::test]
    async fn move_to_an_unknown_output_reports_it_without_acting() {
        let (runner, recorded) = Recorded::with(vec![
            Ok(output(RECORDED_WINDOWS)),
            Ok(output(RECORDED_OUTPUTS)),
        ]);

        let result = NiriBackend::new(runner)
            .move_to("brave", &MoveDestination::Output("DP-9".to_string()))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "No output named 'DP-9'.");
        assert_eq!(
            recorded.calls().len(),
            2,
            "an unknown output must not reach the compositor action"
        );
    }

    #[tokio::test]
    async fn close_reports_the_matched_title() {
        let (runner, recorded) = Recorded::with(vec![Ok(output(RECORDED_WINDOWS)), Ok(output(""))]);
        let backend = NiriBackend::new(runner);

        let title = backend.close("brave").await;
        assert!(
            title.is_ok(),
            "a title substring must match: {:?}",
            title.err()
        );
        assert_eq!(title.unwrap(), "Docs - Brave");
        assert_eq!(
            recorded.calls(),
            vec![
                vec!["msg", "--json", "windows"],
                vec!["msg", "action", "close-window", "--id", "5"],
            ]
        );
    }

    #[tokio::test]
    async fn a_rejected_action_keeps_the_compositor_error() {
        let (runner, _) = Recorded::with(vec![
            Ok(output(RECORDED_WINDOWS)),
            Ok(failure("no focused output")),
        ]);
        let backend = NiriBackend::new(runner);

        let result = backend.close("id:9").await;

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "niri rejected `close-window --id 9`: no focused output"
        );
    }

    #[tokio::test]
    async fn active_reports_the_focused_window_in_the_same_shape() {
        let (runner, _) = Recorded::with(vec![Ok(output(RECORDED_WINDOWS))]);
        let backend = NiriBackend::new(runner);

        let active = backend.active().await;
        assert!(
            active.is_ok(),
            "a recorded focus must read: {:?}",
            active.err()
        );
        let value: Value = serde_json::from_str(&active.unwrap()).expect("active is JSON");
        assert_eq!(value["id"], 9);
        assert_eq!(value["app"], "Alacritty");
        assert_eq!(value["title"], "~");
        assert_eq!(value["workspace"], 1);

        let (runner, _) = Recorded::with(vec![Ok(output(
            r#"[{"id":1,"title":"x","app_id":"y","is_focused":false}]"#,
        ))]);
        let no_focus = NiriBackend::new(runner).active().await;
        assert!(no_focus.is_err());
        assert_eq!(no_focus.unwrap_err().to_string(), "No window is focused.");
    }
}
