//! Read-only diagnostics for the detected desktop session and computer-use
//! prerequisites.

use std::{env, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Tool, ToolResult};
use crate::desktop;
use crate::tools::manifest::{RiskClass, Schema, Source, ToolManifest, CORE_VERSION};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Level {
    Pass,
    Warn,
    Info,
    Legacy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Check {
    name: &'static str,
    level: Level,
    detail: String,
}

impl Check {
    fn status(&self) -> &'static str {
        match (self.name, self.level) {
            ("Hyusk GNOME extension", Level::Legacy) => "responding_legacy",
            ("Hyusk GNOME extension", Level::Pass) => "responding",
            ("Hyusk GNOME extension", Level::Info) => "responding_unverified",
            (_, Level::Pass) => "available_unverified",
            (_, Level::Warn) => "needs_attention",
            (_, Level::Info) => "unknown",
            (_, Level::Legacy) => "legacy",
        }
    }

    fn next_step(&self, session: &desktop::Session) -> Option<String> {
        let gnome = session.compositor() == desktop::Compositor::Gnome;

        if self.level == Level::Legacy {
            if !gnome {
                return None;
            }
            return Some(
                "reload the Hyusk GNOME Shell extension (or relog) so it returns window IDs"
                    .to_string(),
            );
        }
        if self.level != Level::Warn {
            return None;
        }

        Some(match self.name {
            "Desktop session" => {
                "start Hyusk from a supported desktop session (niri, GNOME, sway, \
                 Hyprland, KDE, or X11)"
                    .to_string()
            }
            "Window backend" => match session.compositor() {
                desktop::Compositor::Niri => {
                    "start Hyusk from the niri session so NIRI_SOCKET is set, or use \
                     `accessibility` on this session"
                        .to_string()
                }
                desktop::Compositor::Gnome => {
                    "install and enable the Hyusk GNOME Shell extension, then log out and \
                     back in"
                        .to_string()
                }
                compositor => format!(
                    "there is no window adapter for {} ({}); use `accessibility` and \
                     `computer` for the actions this session supports",
                    compositor.label(),
                    session.kind().label()
                ),
            },
            "AT-SPI Python bindings" => {
                "install python3-pyatspi; import success does not prove an app exposes a \
                 live tree"
                    .to_string()
            }
            "GNOME toolkit accessibility" if gnome => {
                "enable toolkit accessibility for apps that need AT-SPI".to_string()
            }
            "RemoteDesktop portal" if gnome => {
                "check that xdg-desktop-portal and its desktop backend are running".to_string()
            }
            "RemoteDesktop portal" => format!(
                "the RemoteDesktop portal is unavailable on {} ({}); use `accessibility` \
                 and `window` for the actions this session supports",
                session.compositor().label(),
                session.kind().label()
            ),
            ".env permissions" => {
                "run chmod 600 .env so only your account can read the key".to_string()
            }
            "Desktop capabilities" => {
                "grant the refused permission in the system dialog or settings, then retry \
                 the action"
                    .to_string()
            }
            "Hyusk GNOME extension" if gnome => {
                "install and enable the Hyusk GNOME Shell extension, then log out and back \
                 in"
                .to_string()
            }
            // A GNOME-only prerequisite must never produce advice off GNOME.
            "GNOME toolkit accessibility" | "Hyusk GNOME extension" => return None,
            _ => "install or enable the reported prerequisite".to_string(),
        })
    }
}

/// `.env` is git-ignored and normally holds a live provider key, so its mode
/// must not be readable by other local accounts.
fn env_file_check(path: &std::path::Path) -> Check {
    let name = ".env permissions";
    let Ok(metadata) = std::fs::metadata(path) else {
        return Check {
            name,
            level: Level::Info,
            detail: "no .env file; the keyring or the environment is used".into(),
        };
    };
    if !metadata.is_file() {
        return Check {
            name,
            level: Level::Warn,
            detail: format!("{} is not a regular file", path.display()),
        };
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 == 0 {
            Check {
                name,
                level: Level::Pass,
                detail: format!("{:04o}", mode),
            }
        } else {
            Check {
                name,
                level: Level::Warn,
                detail: format!(
                    "{:04o}; the provider key is readable by other accounts",
                    mode
                ),
            }
        }
    }
    #[cfg(not(unix))]
    {
        Check {
            name,
            level: Level::Info,
            detail: "file modes are not checked on this platform".into(),
        }
    }
}

fn session_check(session: &desktop::Session) -> Check {
    let known = session.compositor() != desktop::Compositor::Unknown;
    Check {
        name: "Desktop session",
        level: if known { Level::Pass } else { Level::Warn },
        detail: if known {
            format!(
                "{} ({})",
                session.compositor().label(),
                session.kind().label()
            )
        } else {
            "no display session detected (XDG_CURRENT_DESKTOP, XDG_SESSION_TYPE, WAYLAND_DISPLAY and DISPLAY are unset)"
                .to_string()
        },
    }
}

fn window_backend_check(session: &desktop::Session) -> Check {
    let Some(backend) = session.windows() else {
        return Check {
            name: "Window backend",
            level: Level::Warn,
            detail: session
                .window_unavailable_reason()
                .unwrap_or_else(|| "no window adapter for this session".to_string()),
        };
    };

    let capabilities = backend.capabilities();
    let mut actions = Vec::new();
    for (available, action) in [
        (capabilities.list, "list"),
        (capabilities.active, "active"),
        (capabilities.activate, "activate"),
        (capabilities.last, "last"),
        (capabilities.workspace, "workspace"),
        (capabilities.output, "output"),
        (capabilities.close, "close"),
    ] {
        if available {
            actions.push(action);
        }
    }

    Check {
        name: "Window backend",
        level: Level::Pass,
        detail: format!("{}; actions: {}", backend.source(), actions.join(", ")),
    }
}

fn executable_available(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

fn screenshot_check(session: &desktop::Session) -> Check {
    let candidates: &[&str] = match session.kind() {
        desktop::SessionKind::Wayland => &["grim"],
        desktop::SessionKind::X11 => &["import", "scrot"],
        desktop::SessionKind::Unknown => &[],
    };
    let found: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|name| executable_available(name))
        .collect();
    if found.is_empty() {
        Check {
            name: "screenshot executable",
            level: Level::Info,
            detail: if candidates.is_empty() {
                "no executable backend checked for this session; GNOME Shell D-Bus or the screenshot portal may still be available".into()
            } else {
                format!(
                    "none of {} found; GNOME Shell D-Bus or screenshot portal may still be available",
                    candidates.join(", ")
                )
            },
        }
    } else {
        Check {
            name: "screenshot executable",
            level: Level::Pass,
            detail: format!(
                "{} found (availability only; capture not tested)",
                found.join(", ")
            ),
        }
    }
}

async fn command_check(name: &'static str, command: &str, args: &[&str], optional: bool) -> Check {
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(command)
            .args(args)
            // A timed-out probe must not keep running in the background.
            .kill_on_drop(true)
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output)) if output.status.success() => Check {
            name,
            level: Level::Pass,
            detail: String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("available")
                .trim()
                .to_string(),
        },
        _ if optional => Check {
            name,
            level: Level::Info,
            detail: format!("{command} unavailable (optional)"),
        },
        _ => Check {
            name,
            level: Level::Warn,
            detail: format!("{command} unavailable or failed"),
        },
    }
}

/// The same one-line capability state the model sees, from the process that
/// has actually attempted the actions (D3).
fn capability_check() -> Check {
    let registry = crate::tools::capabilities::snapshot();
    Check {
        name: "Desktop capabilities",
        level: if registry.has_failure() {
            Level::Warn
        } else {
            Level::Info
        },
        detail: registry.line(),
    }
}

/// A read-only probe of the automation browser's CDP endpoint.
fn browser_check() -> Check {
    match crate::tools::availability::reason("browser") {
        None => Check {
            name: "Automation browser (CDP)",
            level: Level::Pass,
            detail: "the CDP endpoint answers; capture is not tested".into(),
        },
        Some(reason) => Check {
            name: "Automation browser (CDP)",
            level: Level::Info,
            detail: reason.to_string(),
        },
    }
}

async fn remote_desktop_check() -> Check {
    match tokio::time::timeout(
        Duration::from_secs(3),
        crate::tools::portal::portal_version(),
    )
    .await
    {
        Ok(Ok(version)) => Check {
            name: "RemoteDesktop portal",
            level: Level::Pass,
            detail: format!("service available (version {version}; input permission is untested and may require approval)"),
        },
        Ok(Err(error)) => Check {
            name: "RemoteDesktop portal",
            level: Level::Warn,
            detail: format!("unavailable: {error}"),
        },
        Err(_) => Check {
            name: "RemoteDesktop portal",
            level: Level::Warn,
            detail: "did not respond within 3 seconds".into(),
        },
    }
}

async fn gnome_toolkit_check() -> Check {
    let toolkit_a11y = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("gsettings")
            .args([
                "get",
                "org.gnome.desktop.interface",
                "toolkit-accessibility",
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await;
    match toolkit_a11y {
        Ok(Ok(output)) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if value == "true" {
                Check {
                    name: "GNOME toolkit accessibility",
                    level: Level::Pass,
                    detail: "enabled".into(),
                }
            } else {
                Check {
                    name: "GNOME toolkit accessibility",
                    level: Level::Warn,
                    detail: format!("{value}; some GTK apps may expose no AT-SPI tree"),
                }
            }
        }
        _ => Check {
            name: "GNOME toolkit accessibility",
            level: Level::Info,
            detail: "gsettings key unavailable; not a GNOME session?".into(),
        },
    }
}

async fn diagnostics_for(session: &desktop::Session) -> Vec<Check> {
    let mut checks = vec![session_check(session), window_backend_check(session)];
    checks.push(command_check("Python 3", "python3", &["--version"], false).await);
    checks.push(
        command_check(
            "AT-SPI Python bindings",
            "python3",
            &["-c", "import pyatspi"],
            false,
        )
        .await,
    );
    checks.push(command_check("Tesseract OCR", "tesseract", &["--version"], true).await);
    checks.push(screenshot_check(session));
    checks.push(env_file_check(std::path::Path::new(".env")));
    checks.push(capability_check());
    checks.push(browser_check());
    checks.push(remote_desktop_check().await);

    // GNOME-only prerequisites are omitted on other sessions, so a niri user
    // is never told to install or configure a GNOME component.
    if session.compositor() == desktop::Compositor::Gnome {
        checks.push(gnome_toolkit_check().await);
        checks.push(extension_check().await);
    }

    checks
}

async fn extension_check() -> Check {
    let outcome = tokio::time::timeout(Duration::from_secs(3), async {
        let connection = zbus::Connection::session()
            .await
            .context("session D-Bus unavailable")?;
        let reply = connection
            .call_method(
                Some("org.hyusk.Shell"),
                "/org/hyusk/Shell",
                Some("org.hyusk.Shell"),
                "ListWindows",
                &(),
            )
            .await
            .context("Hyusk GNOME Shell extension is not responding")?;
        let raw: String = reply
            .body()
            .deserialize()
            .context("extension returned an unexpected response")?;
        let windows: Vec<Value> =
            serde_json::from_str(&raw).context("extension returned invalid window-list JSON")?;
        anyhow::Ok(windows)
    })
    .await;
    match outcome {
        Ok(Ok(windows)) if windows.is_empty() => Check {
            name: "Hyusk GNOME extension",
            level: Level::Info,
            detail: "D-Bus responds, but an empty window list cannot verify the loaded API version".into(),
        },
        Ok(Ok(windows)) if windows.iter().any(|window| window.get("id").is_none()) => Check {
            name: "Hyusk GNOME extension",
            level: Level::Legacy,
            detail: "D-Bus responds, but listed windows lack stable IDs (loaded extension appears outdated)".into(),
        },
        Ok(Ok(_)) => Check {
            name: "Hyusk GNOME extension",
            level: Level::Pass,
            detail: "D-Bus responds and listed windows include stable IDs; activation was not tested".into(),
        },
        Ok(Err(error)) => Check {
            name: "Hyusk GNOME extension",
            level: Level::Warn,
            detail: error.to_string(),
        },
        Err(_) => Check {
            name: "Hyusk GNOME extension",
            level: Level::Warn,
            detail: "D-Bus request timed out".into(),
        },
    }
}

/// GNOME-only prerequisites are irrelevant on another session and must not
/// appear as issues, let alone advice.
fn applies_to_session(check: &Check, session: &desktop::Session) -> bool {
    let gnome_only = matches!(
        check.name,
        "Hyusk GNOME extension" | "GNOME toolkit accessibility"
    );
    !gnome_only || session.compositor() == desktop::Compositor::Gnome
}

fn report(checks: &[Check], session: &desktop::Session) -> Value {
    let checks: Vec<&Check> = checks
        .iter()
        .filter(|check| applies_to_session(check, session))
        .collect();
    let issues: Vec<_> = checks
        .iter()
        .filter(|check| matches!(check.level, Level::Warn | Level::Legacy))
        .map(|check| check.name)
        .collect();
    let next_steps: Vec<_> = checks
        .iter()
        .filter_map(|check| {
            check
                .next_step(session)
                .map(|action| json!({"capability": check.name, "action": action}))
        })
        .collect();
    json!({
        "summary": "partial; probes do not guarantee end-to-end computer use",
        "checks": checks.iter().map(|check| json!({
            "capability": check.name,
            "status": check.status(),
            "detail": check.detail,
            "next_step": check.next_step(session),
        })).collect::<Vec<_>>(),
        "issues": issues,
        "next_steps": next_steps,
        "limitations": [
            "AT-SPI import does not prove an application exposes a live accessibility tree.",
            "RemoteDesktop portal presence does not prove input permission has been granted.",
            "Screenshot executable presence is not a screenshot capture test.",
            "No portal permission dialog is opened by this diagnostic.",
            "Desktop capabilities are only what earlier attempts reported; a granted state is recorded when a backend succeeds."
        ]
    })
}

pub struct DesktopDoctorTool;

impl DesktopDoctorTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DesktopDoctorTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DesktopDoctorTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "desktop_doctor",
            version: CORE_VERSION,
            description: "Run read-only checks for the detected desktop session: compositor and session type, window backend and its actions, Python/AT-SPI, RemoteDesktop portal input, automation browser CDP, optional Tesseract OCR, screenshot executables, recorded desktop capability state, and the local .env file mode. On GNOME it also checks toolkit accessibility and the Hyusk shell extension; other sessions omit those checks and their advice. Does not request permissions or change settings.",
            summary: "Report which desktop capabilities are missing and what to try next.",
            schema: Schema::Fn(desktop_doctor_schema),
            actions: None,
            risk: &[RiskClass::Read],
            capabilities: &[],
            source: Source::Core,
        }
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let args: Value = serde_json::from_str(input).context("GNOME doctor input must be JSON")?;
        if !args.is_object() {
            return Ok(ToolResult::failure("Input must be a JSON object."));
        }
        let session = desktop::session();
        Ok(ToolResult::success(serde_json::to_string_pretty(&report(
            &diagnostics_for(session).await,
            session,
        ))?))
    }
}

fn desktop_doctor_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gnome_session() -> desktop::Session {
        session_fixture(false, "GNOME", "wayland")
    }

    fn niri_session() -> desktop::Session {
        session_fixture(true, "niri", "wayland")
    }

    fn session_fixture(niri: bool, desktop: &str, session_type: &str) -> desktop::Session {
        desktop::detect_from(&desktop::Environment {
            niri_socket: niri.then(|| "/run/user/1000/niri.wayland-1.0.sock".to_string()),
            desktop: Some(desktop.to_string()),
            session_type: Some(session_type.to_string()),
            wayland_display: (session_type == "wayland").then(|| "wayland-1".to_string()),
            ..desktop::Environment::default()
        })
    }

    #[test]
    fn session_check_reports_the_compositor_kind_and_window_backend() {
        let niri = session_fixture(true, "niri", "wayland");
        let check = session_check(&niri);
        assert_eq!(check.name, "Desktop session");
        assert_eq!(check.level, Level::Pass);
        assert!(check.detail.contains("niri"), "{}", check.detail);
        assert!(check.detail.contains("Wayland"), "{}", check.detail);

        let backend = window_backend_check(&niri);
        assert_eq!(backend.name, "Window backend");
        assert_eq!(backend.level, Level::Pass);
        assert!(backend.detail.contains("niri IPC"), "{}", backend.detail);
        for action in ["last", "workspace", "output"] {
            assert!(backend.detail.contains(action), "{}", backend.detail);
        }

        let gnome = session_fixture(false, "GNOME", "wayland");
        assert!(session_check(&gnome).detail.contains("GNOME"));
        let backend = window_backend_check(&gnome);
        assert_eq!(backend.level, Level::Pass);
        assert!(
            backend.detail.contains("GNOME Shell window metadata"),
            "{}",
            backend.detail
        );
        assert!(
            !backend.detail.contains("last"),
            "GNOME must not advertise niri navigation: {}",
            backend.detail
        );

        let sway = session_fixture(false, "sway", "wayland");
        assert_eq!(session_check(&sway).level, Level::Pass);
        let backend = window_backend_check(&sway);
        assert_eq!(backend.level, Level::Warn);
        assert!(backend.detail.contains("NIRI_SOCKET"), "{}", backend.detail);

        let unknown = desktop::detect_from(&desktop::Environment::default());
        let check = session_check(&unknown);
        assert_eq!(check.level, Level::Warn);
        assert!(check.detail.contains("no display"), "{}", check.detail);
    }

    #[test]
    fn advice_is_session_relative_and_never_offers_gnome_advice_on_niri() {
        let niri = niri_session();
        let portal = Check {
            name: "RemoteDesktop portal",
            level: Level::Warn,
            detail: "unavailable: no portal".into(),
        };
        let advice = portal
            .next_step(&niri)
            .expect("an unavailable capability needs a next step");
        assert!(advice.contains("niri"), "{advice}");
        assert!(!advice.contains("GNOME"), "{advice}");

        // Even if a GNOME-only check leaked into the list, its advice is
        // suppressed off GNOME.
        let extension = Check {
            name: "Hyusk GNOME extension",
            level: Level::Warn,
            detail: "unavailable".into(),
        };
        assert_eq!(extension.next_step(&niri), None);

        let sway = session_fixture(false, "sway", "wayland");
        let backend = window_backend_check(&sway);
        let advice = backend
            .next_step(&sway)
            .expect("an unavailable backend needs a next step");
        assert!(advice.contains("sway"), "{advice}");
        assert!(!advice.contains("GNOME"), "{advice}");

        let gnome = gnome_session();
        let advice = extension
            .next_step(&gnome)
            .expect("GNOME gets the install step");
        assert!(advice.contains("install"), "{advice}");

        let report = report(&[portal, extension], &niri);
        let encoded = report.to_string();
        assert!(
            !encoded.contains("GNOME extension"),
            "a niri report must not recommend a GNOME extension: {encoded}"
        );
    }

    #[test]
    fn capability_state_uses_the_same_one_line_as_the_prompt() {
        let check = capability_check();
        assert_eq!(check.name, "Desktop capabilities");
        assert!(
            check.detail.starts_with("capabilities: input "),
            "{}",
            check.detail
        );
        assert!(check.detail.contains("; screenshot "), "{}", check.detail);
        assert!(
            check.detail.contains("; accessibility "),
            "{}",
            check.detail
        );
        // A clean record is informational, not a warning.
        crate::tools::capabilities::reset_for_test();
        assert_eq!(capability_check().level, Level::Info);
    }

    #[test]
    fn env_file_mode_is_flagged_when_other_accounts_can_read_it() {
        let directory = std::env::temp_dir().join(format!(
            "hyusk-env-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        let path = directory.join(".env");
        std::fs::write(&path, "OPENROUTER_API_KEY=test\n").expect("write .env");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .expect("set mode");
            let check = env_file_check(&path);
            assert_eq!(check.level, Level::Warn, "{}", check.detail);
            assert!(check.detail.contains("0644"), "{}", check.detail);
            assert!(check
                .next_step(&gnome_session())
                .is_some_and(|step| step.contains("chmod 600")));

            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("set mode");
            let check = env_file_check(&path);
            assert_eq!(check.level, Level::Pass, "{}", check.detail);
            assert!(check.next_step(&gnome_session()).is_none());
        }

        // A missing file is not a problem: the keyring is the primary path.
        let missing = env_file_check(&directory.join("absent.env"));
        assert_eq!(missing.level, Level::Info);

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn renders_each_status_and_check_name() {
        let value = report(
            &[
                Check {
                    name: "AT-SPI",
                    level: Level::Pass,
                    detail: "available".into(),
                },
                Check {
                    name: "RemoteDesktop portal",
                    level: Level::Warn,
                    detail: "unavailable".into(),
                },
                Check {
                    name: "OCR",
                    level: Level::Info,
                    detail: "optional".into(),
                },
            ],
            &gnome_session(),
        );
        assert_eq!(
            value["summary"],
            "partial; probes do not guarantee end-to-end computer use"
        );
        assert_eq!(value["checks"][0]["status"], "available_unverified");
        assert_eq!(value["checks"][1]["status"], "needs_attention");
        assert_eq!(
            value["checks"][1]["next_step"],
            "check that xdg-desktop-portal and its desktop backend are running"
        );
        assert_eq!(value["checks"][2]["status"], "unknown");
        assert!(value["limitations"].as_array().unwrap().len() >= 3);
    }

    #[test]
    fn marks_legacy_extension_payload_as_an_issue() {
        let value = report(
            &[Check {
                name: "Hyusk GNOME extension",
                level: Level::Legacy,
                detail: "listed windows lack stable IDs".into(),
            }],
            &gnome_session(),
        );
        assert_eq!(value["checks"][0]["status"], "responding_legacy");
        assert_eq!(value["issues"][0], "Hyusk GNOME extension");
        assert!(value["checks"][0]["next_step"]
            .as_str()
            .unwrap()
            .contains("relog"));
    }
}
