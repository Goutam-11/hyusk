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
            "AT-SPI Python bindings" => {
                "install python3-pyatspi; import success does not prove an app exposes a live tree"
            }
            "GNOME toolkit accessibility" => {
                "enable toolkit accessibility for apps that need AT-SPI"
            }
            "RemoteDesktop portal" => {
                "check that xdg-desktop-portal and its desktop backend are running"
            }
            "Hyusk GNOME extension" => {
                "install and enable the Hyusk GNOME Shell extension, then log out and back in"
            }
            _ => "install or enable the reported prerequisite",
        })
    }
}

fn environment_check(name: &'static str, var: &str) -> Check {
    match env::var(var).ok().filter(|value| !value.is_empty()) {
        Some(value) => Check {
            name,
            level: Level::Pass,
            detail: value,
        },
        None => Check {
            name,
            level: if session_kind() == "unknown display server" {
                Level::Warn
            } else {
                Level::Info
            },
            detail: if session_kind() == "unknown display server" {
                format!("{var} is not set; display type is unknown")
            } else {
                format!(
                    "{var} is not set; inferred {} from display environment",
                    session_kind()
                )
            },
        },
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
                format!("none of {} found; GNOME Shell D-Bus or screenshot portal may still be available", candidates.join(", "))
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
        tokio::process::Command::new(command).args(args).output(),
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

async fn diagnostics() -> Vec<Check> {
    let mut checks = vec![gnome_session_check()];
    checks.push(environment_check(
        "display session type",
        "XDG_SESSION_TYPE",
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

    let toolkit_a11y = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("gsettings")
            .args([
                "get",
                "org.gnome.desktop.interface",
                "toolkit-accessibility",
            ])
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
            "No portal permission dialog is opened by this diagnostic."
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
        "Run read-only checks for GNOME computer-use readiness: session type, Python/AT-SPI, toolkit accessibility, the Hyusk shell extension, RemoteDesktop portal, and optional Tesseract OCR. Does not request permissions or change settings."
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
