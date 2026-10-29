//! Read-only diagnostics for GNOME desktop and computer-use prerequisites.

use std::{env, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Tool, ToolResult};

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

    fn next_step(&self) -> Option<&'static str> {
        if self.level == Level::Legacy {
            return Some(
                "reload the Hyusk GNOME Shell extension (or relog) so it returns window IDs",
            );
        }
        if self.level != Level::Warn {
            return None;
        }
        Some(match self.name {
            "GNOME session" => "run Hyusk from a GNOME desktop session",
            "display session type" => "run Hyusk in a supported GNOME Wayland or X11 session",
            "AT-SPI Python bindings" => {
                "install python3-pyatspi; import success does not prove an app exposes a live tree"
            }
            "GNOME toolkit accessibility" => {
                "enable toolkit accessibility for apps that need AT-SPI"
            }
            "RemoteDesktop portal" => {
                "check that xdg-desktop-portal and its desktop backend are running"
            }
            ".env permissions" => "run chmod 600 .env so only your account can read the key",
            "Desktop capabilities" => {
                "grant the refused permission in the system dialog or settings, then retry the action"
            }
            "Hyusk GNOME extension" => {
                "install and enable the Hyusk GNOME Shell extension, then log out and back in"
            }
            _ => "install or enable the reported prerequisite",
        })
    }
}

fn display_session_check(configured: Option<&str>, has_wayland: bool, has_x11: bool) -> Check {
    let inferred = if has_wayland {
        Some("Wayland")
    } else if has_x11 {
        Some("X11")
    } else {
        None
    };
    let (level, detail) = match configured {
        Some("wayland") => (Level::Pass, "Wayland".to_string()),
        Some("x11") => (Level::Pass, "X11".to_string()),
        Some(value) => (
            Level::Warn,
            format!("unsupported XDG_SESSION_TYPE={value}; expected wayland or x11"),
        ),
        None => match inferred {
            Some(kind) => (
                Level::Info,
                format!("XDG_SESSION_TYPE is unset; inferred {kind} from display environment"),
            ),
            None => (
                Level::Warn,
                "XDG_SESSION_TYPE is unset and display type is unknown".into(),
            ),
        },
    };
    Check {
        name: "display session type",
        level,
        detail,
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

fn gnome_session_check() -> Check {
    let desktop = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if desktop.to_ascii_lowercase().contains("gnome") {
        Check {
            name: "GNOME session",
            level: Level::Pass,
            detail: format!("{desktop} ({})", session_kind()),
        }
    } else {
        Check {
            name: "GNOME session",
            level: Level::Warn,
            detail: format!(
                "XDG_CURRENT_DESKTOP={}; session={}",
                if desktop.is_empty() {
                    "unset"
                } else {
                    &desktop
                },
                session_kind()
            ),
        }
    }
}

fn session_kind() -> &'static str {
    match env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => "Wayland",
        Ok("x11") => "X11",
        _ if env::var_os("WAYLAND_DISPLAY").is_some() => "Wayland",
        _ if env::var_os("DISPLAY").is_some() => "X11",
        _ => "unknown display server",
    }
}

fn executable_available(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

fn screenshot_check() -> Check {
    let candidates: &[&str] = match session_kind() {
        "Wayland" => &["grim"],
        "X11" => &["import", "scrot"],
        _ => &[],
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

async fn diagnostics() -> Vec<Check> {
    let mut checks = vec![gnome_session_check()];
    checks.push(display_session_check(
        env::var("XDG_SESSION_TYPE").ok().as_deref(),
        env::var_os("WAYLAND_DISPLAY").is_some(),
        env::var_os("DISPLAY").is_some(),
    ));
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
    checks.push(screenshot_check());
    checks.push(env_file_check(std::path::Path::new(".env")));
    checks.push(capability_check());

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
    checks.push(match toolkit_a11y {
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
    });

    checks.push(
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
        },
    );

    checks.push(extension_check().await);
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

fn report(checks: &[Check]) -> Value {
    let issues: Vec<_> = checks
        .iter()
        .filter(|check| matches!(check.level, Level::Warn | Level::Legacy))
        .map(|check| check.name)
        .collect();
    let next_steps: Vec<_> = checks
        .iter()
        .filter_map(|check| {
            check
                .next_step()
                .map(|action| json!({"capability": check.name, "action": action}))
        })
        .collect();
    json!({
        "summary": "partial; probes do not guarantee end-to-end computer use",
        "checks": checks.iter().map(|check| json!({
            "capability": check.name,
            "status": check.status(),
            "detail": check.detail,
            "next_step": check.next_step(),
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

pub struct GnomeDoctorTool;

impl GnomeDoctorTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GnomeDoctorTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GnomeDoctorTool {
    fn name(&self) -> &str {
        "gnome_doctor"
    }

    fn description(&self) -> &str {
        "Run read-only checks for GNOME computer-use readiness: session type, Python/AT-SPI, toolkit accessibility, the Hyusk shell extension, RemoteDesktop portal, optional Tesseract OCR, and the local .env file mode. Does not request permissions or change settings."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let args: Value = serde_json::from_str(input).context("GNOME doctor input must be JSON")?;
        if !args.is_object() {
            return Ok(ToolResult::failure("Input must be a JSON object."));
        }
        Ok(ToolResult::success(serde_json::to_string_pretty(&report(
            &diagnostics().await,
        ))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                .next_step()
                .is_some_and(|step| step.contains("chmod 600")));

            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("set mode");
            let check = env_file_check(&path);
            assert_eq!(check.level, Level::Pass, "{}", check.detail);
            assert!(check.next_step().is_none());
        }

        // A missing file is not a problem: the keyring is the primary path.
        let missing = env_file_check(&directory.join("absent.env"));
        assert_eq!(missing.level, Level::Info);

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn renders_each_status_and_check_name() {
        let value = report(&[
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
        ]);
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
    fn display_session_check_does_not_mark_unsupported_values_available() {
        let check = display_session_check(Some("tty"), false, false);
        assert_eq!(check.level, Level::Warn);
        assert!(check.detail.contains("expected wayland or x11"));

        let inferred = display_session_check(None, true, false);
        assert_eq!(inferred.level, Level::Info);
        assert!(inferred.detail.contains("inferred Wayland"));
    }

    #[test]
    fn marks_legacy_extension_payload_as_an_issue() {
        let value = report(&[Check {
            name: "Hyusk GNOME extension",
            level: Level::Legacy,
            detail: "listed windows lack stable IDs".into(),
        }]);
        assert_eq!(value["checks"][0]["status"], "responding_legacy");
        assert_eq!(value["issues"][0], "Hyusk GNOME extension");
        assert!(value["checks"][0]["next_step"]
            .as_str()
            .unwrap()
            .contains("relog"));
    }
}
