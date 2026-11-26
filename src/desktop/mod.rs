//! What desktop session is running, and which window adapter can serve it.
//!
//! Compositor knowledge used to be scattered: `window.rs` branched on
//! `NIRI_SOCKET`, `availability.rs` re-detected the desktop, and `computer.rs`
//! probed programs in its own order. Detection lives here once, as a pure
//! function over the environment ([`detect_from`]), so every consumer asks the
//! same question and the answer is testable without a live session.
//!
//! Detection is not a feature switch: it selects the adapter, and the adapter
//! must behave exactly like the code it replaced on niri and GNOME.

use std::sync::OnceLock;

mod gnome;
mod niri;

pub use gnome::GnomeBackend;
pub use niri::NiriBackend;

/// The compositor that is running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Compositor {
    Niri,
    Gnome,
    Sway,
    Hyprland,
    Kde,
    X11Ewmh,
    Unknown,
}

/// The display-server family, as far as the environment reports it.
///
/// `Unknown` means no usable display environment was detected; it is not a
/// guess between the other two.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionKind {
    Wayland,
    X11,
    Unknown,
}

impl Compositor {
    pub fn label(self) -> &'static str {
        match self {
            Compositor::Niri => "niri",
            Compositor::Gnome => "GNOME",
            Compositor::Sway => "sway",
            Compositor::Hyprland => "Hyprland",
            Compositor::Kde => "KDE",
            Compositor::X11Ewmh => "X11",
            Compositor::Unknown => "this desktop",
        }
    }
}

impl SessionKind {
    pub fn label(self) -> &'static str {
        match self {
            SessionKind::Wayland => "Wayland",
            SessionKind::X11 => "X11",
            SessionKind::Unknown => "unknown display server",
        }
    }
}

/// The environment variables detection reads, separated from the process so a
/// test can describe a session instead of mutating global state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Environment {
    pub niri_socket: Option<String>,
    pub desktop: Option<String>,
    pub session_type: Option<String>,
    pub wayland_display: Option<String>,
    pub display: Option<String>,
}

impl Environment {
    /// Read the live process environment once.
    pub fn current() -> Self {
        fn value(name: &str) -> Option<String> {
            std::env::var(name).ok().filter(|value| !value.is_empty())
        }

        Self {
            niri_socket: value("NIRI_SOCKET"),
            desktop: value("XDG_CURRENT_DESKTOP"),
            session_type: value("XDG_SESSION_TYPE"),
            wayland_display: value("WAYLAND_DISPLAY"),
            display: value("DISPLAY"),
        }
    }
}

/// One window, in the shape every consumer understands.
///
/// Absent fields stay `None` rather than defaulting, because the GNOME
/// extension omits some of them on a legacy build and the desktop snapshot must
/// not invent values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Window {
    pub id: Option<u64>,
    pub app: String,
    pub title: String,
    pub active: bool,
    pub workspace: Option<u64>,
    pub monitor: Option<i64>,
    pub minimized: Option<bool>,
    pub on_current_workspace: Option<bool>,
    pub fullscreen: Option<bool>,
    pub bounds: Option<Bounds>,
}

impl Window {
    /// The subset the bounded desktop snapshot renders.
    ///
    /// Keys are conditional exactly where the old per-compositor values were:
    /// `workspace` is always present (possibly null), while the GNOME-only
    /// flags and `monitor` appear only when the adapter reported them.
    pub fn to_desktop_value(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "id": self.id,
            "app": self.app,
            "title": self.title,
            "active": self.active,
            "workspace": self.workspace,
        });
        for (key, flag) in [
            ("minimized", self.minimized),
            ("on_current_workspace", self.on_current_workspace),
            ("fullscreen", self.fullscreen),
        ] {
            if let Some(flag) = flag {
                value[key] = serde_json::json!(flag);
            }
        }
        if let Some(monitor) = self.monitor {
            value["monitor"] = serde_json::json!(monitor);
        }
        value
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Bounds {
    pub x: i64,
    pub y: i64,
    pub width: u64,
    pub height: u64,
}

/// The windows an adapter reported, and the text the tool returns for them.
///
/// `rendered` is part of the adapter because the two compositors describe a
/// window differently (niri adds the workspace, GNOME adds bounds and
/// minimised state); `windows` is what the desktop snapshot and the later
/// navigation actions read.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WindowList {
    pub windows: Vec<Window>,
    pub rendered: String,
}

/// What one window adapter can do. A missing action is reported to the model
/// instead of failing inside the adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WindowCapabilities {
    pub list: bool,
    pub active: bool,
    pub activate: bool,
    pub close: bool,
}

/// One compositor's window protocol.
///
/// Implemented only inside `src/desktop/`: adding an adapter must not need a
/// change to the window tool or to any other module.
#[async_trait::async_trait]
pub trait WindowsBackend: Send + Sync {
    /// The observation source named in the desktop snapshot.
    fn source(&self) -> &'static str;

    fn capabilities(&self) -> WindowCapabilities;

    /// The open windows, plus the list text the tool returns.
    async fn list(&self) -> anyhow::Result<WindowList>;

    /// The focused window, in this compositor's reply shape.
    async fn active(&self) -> anyhow::Result<String>;

    /// Raise the window a query matches; returns its title.
    async fn activate(&self, query: &str) -> anyhow::Result<String>;

    /// Close the window a query matches; returns its title.
    async fn close(&self, query: &str) -> anyhow::Result<String>;
}

/// The detected session.
#[derive(Debug, Clone)]
pub struct Session {
    compositor: Compositor,
    kind: SessionKind,
    desktop: String,
    niri_socket: bool,
}

impl Session {
    pub fn compositor(&self) -> Compositor {
        self.compositor
    }

    pub fn kind(&self) -> SessionKind {
        self.kind
    }

    /// The window adapter this session can use, if any.
    pub fn windows(&self) -> Option<&'static dyn WindowsBackend> {
        match (self.compositor, self.niri_socket) {
            (Compositor::Niri, true) => Some(&NIRI),
            (Compositor::Gnome, _) => Some(&GNOME),
            _ => None,
        }
    }

    /// What the prompt and the gate can rely on for this session.
    pub fn capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            windows: self.windows().map(|backend| backend.capabilities()),
        }
    }

    /// Why the window tool cannot work here, or `None` when an adapter exists.
    pub fn window_unavailable_reason(&self) -> Option<String> {
        if self.windows().is_some() {
            return None;
        }
        Some(format!(
            "no window backend on {} ({}): NIRI_SOCKET is unset and the desktop is {:?}",
            self.compositor().label(),
            self.kind().label(),
            self.desktop
        ))
    }

    /// The ordered focus fallback `computer focus_window` may try.
    ///
    /// Only the commands that can work on this compositor are listed: there is
    /// no blind probe of every program the agent knows about.
    pub fn focus_commands(&self, title: &str) -> Vec<FocusCommand> {
        let mut commands = Vec::new();

        if self.compositor == Compositor::Hyprland {
            commands.push(FocusCommand {
                program: "hyprctl",
                args: vec![
                    "dispatch".to_string(),
                    "focuswindow".to_string(),
                    format!("title:{title}"),
                ],
            });
        }

        if self.kind == SessionKind::X11 {
            commands.push(FocusCommand {
                program: "wmctrl",
                args: vec!["-a".to_string(), title.to_string()],
            });
            commands.push(FocusCommand {
                program: "xdotool",
                args: vec![
                    "search".to_string(),
                    "--name".to_string(),
                    title.to_string(),
                    "windowactivate".to_string(),
                ],
            });
        }

        commands
    }

    /// What to tell the user when no fallback worked.
    pub fn focus_unavailable_hint(&self) -> String {
        match (self.compositor, self.kind) {
            (Compositor::Niri, _) => {
                "niri focuses windows through the `window` tool's `activate` action".to_string()
            }
            (Compositor::Gnome, SessionKind::Wayland) => {
                "GNOME Wayland does not expose a generic window-focus API; use the `window` tool"
                    .to_string()
            }
            (_, SessionKind::X11) => "No X11 window manager accepted the request.".to_string(),
            (compositor, _) => format!(
                "{} exposes no generic window-focus command; use the `window` tool when its compositor is supported",
                compositor.label()
            ),
        }
    }
}

/// One focus attempt `computer focus_window` may make.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusCommand {
    pub program: &'static str,
    pub args: Vec<String>,
}

/// The facts a session exposes to the rest of the agent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionCapabilities {
    pub windows: Option<WindowCapabilities>,
}

/// Detect the session from `env`.
///
/// `NIRI_SOCKET` is the most specific signal, then the desktop name, then the
/// display-server family. A declared `XDG_SESSION_TYPE` wins over the display
/// variables when it names a supported family, so a stale `WAYLAND_DISPLAY`
/// cannot mislabel an X11 session.
pub fn detect_from(env: &Environment) -> Session {
    let kind = match env.session_type.as_deref() {
        Some(value) if value.eq_ignore_ascii_case("wayland") => SessionKind::Wayland,
        Some(value) if value.eq_ignore_ascii_case("x11") => SessionKind::X11,
        _ => inferred_kind(env),
    };
    let desktop = env.desktop.clone().unwrap_or_default();
    let names = desktop.to_ascii_lowercase();
    let niri_socket = env.niri_socket.is_some();

    let compositor = if niri_socket || names.contains("niri") {
        Compositor::Niri
    } else if names.contains("gnome") {
        Compositor::Gnome
    } else if names.contains("sway") {
        Compositor::Sway
    } else if names.contains("hyprland") {
        Compositor::Hyprland
    } else if names.contains("kde") || names.contains("plasma") {
        Compositor::Kde
    } else if kind == SessionKind::X11 {
        Compositor::X11Ewmh
    } else {
        Compositor::Unknown
    };

    Session {
        compositor,
        kind,
        desktop,
        niri_socket,
    }
}

fn inferred_kind(env: &Environment) -> SessionKind {
    if env.wayland_display.is_some() {
        SessionKind::Wayland
    } else if env.display.is_some() {
        SessionKind::X11
    } else {
        SessionKind::Unknown
    }
}

/// The two adapters the series ships so far. Each is stateless, so one
/// process-wide instance is enough.
static NIRI: NiriBackend<niri::CommandRunner> = NiriBackend::new(niri::CommandRunner);
static GNOME: GnomeBackend<gnome::DbusShell> = GnomeBackend::new(gnome::DbusShell);

/// The process session, detected once.
pub fn session() -> &'static Session {
    static SESSION: OnceLock<Session> = OnceLock::new();
    SESSION.get_or_init(|| detect_from(&Environment::current()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        niri_socket: Option<&str>,
        desktop: Option<&str>,
        session_type: Option<&str>,
        wayland_display: Option<&str>,
        display: Option<&str>,
    ) -> Environment {
        Environment {
            niri_socket: niri_socket.map(str::to_string),
            desktop: desktop.map(str::to_string),
            session_type: session_type.map(str::to_string),
            wayland_display: wayland_display.map(str::to_string),
            display: display.map(str::to_string),
        }
    }

    #[test]
    fn detection_maps_each_environment_to_one_compositor_and_kind() {
        let cases = [
            (
                "niri",
                fixture(
                    Some("/run/user/1000/niri.wayland-1.0.sock"),
                    Some("niri"),
                    Some("wayland"),
                    Some("wayland-1"),
                    None,
                ),
                Compositor::Niri,
                SessionKind::Wayland,
            ),
            (
                "GNOME on Wayland",
                fixture(
                    None,
                    Some("ubuntu:GNOME"),
                    Some("wayland"),
                    Some("wayland-0"),
                    None,
                ),
                Compositor::Gnome,
                SessionKind::Wayland,
            ),
            (
                "sway",
                fixture(None, Some("sway"), Some("wayland"), Some("wayland-1"), None),
                Compositor::Sway,
                SessionKind::Wayland,
            ),
            (
                "X11",
                fixture(None, Some("X-Cinnamon"), Some("x11"), None, Some(":0")),
                Compositor::X11Ewmh,
                SessionKind::X11,
            ),
            (
                "Hyprland",
                fixture(
                    None,
                    Some("Hyprland"),
                    Some("wayland"),
                    Some("wayland-1"),
                    None,
                ),
                Compositor::Hyprland,
                SessionKind::Wayland,
            ),
            (
                "KDE",
                fixture(None, Some("KDE"), Some("wayland"), Some("wayland-0"), None),
                Compositor::Kde,
                SessionKind::Wayland,
            ),
            (
                "unknown",
                fixture(None, None, None, None, None),
                Compositor::Unknown,
                SessionKind::Unknown,
            ),
        ];

        for (name, environment, expected_compositor, expected_kind) in cases {
            let session = detect_from(&environment);
            assert_eq!(
                session.compositor(),
                expected_compositor,
                "compositor: {name}"
            );
            assert_eq!(session.kind(), expected_kind, "kind: {name}");
        }
    }

    #[test]
    fn a_session_selects_the_backend_it_can_serve_or_explains_the_gap() {
        let niri = detect_from(&fixture(
            Some("/run/user/1000/niri.wayland-1.0.sock"),
            Some("niri"),
            Some("wayland"),
            Some("wayland-1"),
            None,
        ));
        assert!(niri.windows().is_some(), "niri provides a window adapter");
        assert_eq!(niri.window_unavailable_reason(), None);
        assert_eq!(
            niri.capabilities().windows,
            niri.windows().map(|backend| backend.capabilities())
        );

        let gnome = detect_from(&fixture(
            None,
            Some("GNOME"),
            Some("wayland"),
            Some("wayland-0"),
            None,
        ));
        assert!(
            gnome.windows().is_some(),
            "GNOME uses the extension adapter"
        );

        // A compositor the series has not reached yet reports the gap instead
        // of pretending a backend exists.
        let sway = detect_from(&fixture(
            None,
            Some("sway"),
            Some("wayland"),
            Some("wayland-1"),
            None,
        ));
        assert!(sway.windows().is_none());
        let reason = sway
            .window_unavailable_reason()
            .expect("sway must explain the missing backend");
        assert!(reason.contains("NIRI_SOCKET"), "{reason}");
        assert!(reason.contains("sway"), "{reason}");
        assert_eq!(sway.capabilities().windows, None);

        // Naming niri without its socket is not enough to run `niri msg`.
        let niri_without_socket = detect_from(&fixture(
            None,
            Some("niri"),
            Some("wayland"),
            Some("wayland-1"),
            None,
        ));
        assert!(niri_without_socket.windows().is_none());
        assert!(niri_without_socket
            .window_unavailable_reason()
            .is_some_and(|reason| reason.contains("NIRI_SOCKET")));
    }

    #[test]
    fn a_window_becomes_the_desktop_snapshot_shape_without_inventing_fields() {
        let niri = Window {
            id: Some(9),
            app: "Alacritty".to_string(),
            title: "~".to_string(),
            active: true,
            workspace: Some(1),
            ..Window::default()
        };
        let value = niri.to_desktop_value();
        assert_eq!(value["id"], 9);
        assert_eq!(value["app"], "Alacritty");
        assert_eq!(value["title"], "~");
        assert_eq!(value["active"], true);
        assert_eq!(value["workspace"], 1);
        assert!(
            value.get("minimized").is_none(),
            "niri did not report minimised state: {value}"
        );

        let gnome = Window {
            id: Some(5),
            app: "brave-browser".to_string(),
            title: "Docs".to_string(),
            active: false,
            workspace: Some(0),
            monitor: Some(0),
            minimized: Some(true),
            on_current_workspace: Some(false),
            fullscreen: Some(false),
            bounds: Some(Bounds {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            }),
        };
        let value = gnome.to_desktop_value();
        assert_eq!(value["minimized"], true);
        assert_eq!(value["on_current_workspace"], false);
        assert_eq!(value["fullscreen"], false);
        assert_eq!(value["monitor"], 0);
        assert!(
            value.get("bounds").is_none(),
            "the bounded snapshot never rendered bounds: {value}"
        );
    }

    #[test]
    fn focus_fallbacks_are_chosen_by_the_session_not_probed_blindly() {
        let hyprland = detect_from(&fixture(
            None,
            Some("Hyprland"),
            Some("wayland"),
            Some("wayland-1"),
            None,
        ));
        let commands = hyprland.focus_commands("Docs - Brave");
        assert_eq!(commands.len(), 1, "only hyprctl can focus on Hyprland");
        assert_eq!(commands[0].program, "hyprctl");
        assert_eq!(
            commands[0].args,
            vec!["dispatch", "focuswindow", "title:Docs - Brave"]
        );

        let x11 = detect_from(&fixture(None, Some("XFCE"), Some("x11"), None, Some(":0")));
        let commands = x11.focus_commands("Docs");
        assert_eq!(
            commands
                .iter()
                .map(|command| command.program)
                .collect::<Vec<_>>(),
            vec!["wmctrl", "xdotool"]
        );
        assert_eq!(commands[0].args, vec!["-a", "Docs"]);

        // These sessions have no generic focus command: niri is served by the
        // window tool, and GNOME Wayland does not expose one at all.
        let niri = detect_from(&fixture(
            Some("/run/user/1000/niri.wayland-1.0.sock"),
            Some("niri"),
            Some("wayland"),
            Some("wayland-1"),
            None,
        ));
        assert!(niri.focus_commands("Docs").is_empty());
        assert!(
            niri.focus_unavailable_hint().contains("window"),
            "{}",
            niri.focus_unavailable_hint()
        );

        let gnome_wayland = detect_from(&fixture(
            None,
            Some("GNOME"),
            Some("wayland"),
            Some("wayland-0"),
            None,
        ));
        assert!(gnome_wayland.focus_commands("Docs").is_empty());
        assert!(gnome_wayland
            .focus_unavailable_hint()
            .contains("GNOME Wayland"));
    }

    #[test]
    fn detection_infers_the_kind_from_the_display_environment_when_the_type_is_unset() {
        let wayland = fixture(None, Some("sway"), None, Some("wayland-1"), None);
        assert_eq!(detect_from(&wayland).kind(), SessionKind::Wayland);

        let x11 = fixture(None, Some("GNOME"), None, None, Some(":0"));
        assert_eq!(detect_from(&x11).kind(), SessionKind::X11);

        // XDG_SESSION_TYPE wins over the display variables when it names a
        // supported family, so a stale WAYLAND_DISPLAY cannot mislabel X11.
        let declared = fixture(
            None,
            Some("GNOME"),
            Some("x11"),
            Some("wayland-1"),
            Some(":0"),
        );
        assert_eq!(detect_from(&declared).kind(), SessionKind::X11);
    }
}
