use std::fs;

/// Build a short, human-readable description of the current machine and
/// session for the model system prompt.
pub fn system_context() -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "Current date/time: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z")
    ));

    lines.push(format!(
        "Platform: {} / {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));

    if let Some(pretty) = os_release_value("PRETTY_NAME").or_else(|| os_release_value("NAME")) {
        lines.push(format!("Distribution: {pretty}"));
    }

    if let Ok(kernel) = fs::read_to_string("/proc/sys/kernel/osrelease") {
        lines.push(format!("Kernel: {}", kernel.trim()));
    }

    if let Ok(hostname) = fs::read_to_string("/etc/hostname") {
        let hostname = hostname.trim();

        if !hostname.is_empty() {
            lines.push(format!("Host: {hostname}"));
        }
    }

    for (label, variable) in [
        ("User", "USER"),
        ("Home", "HOME"),
        ("Shell", "SHELL"),
        ("Desktop", "XDG_CURRENT_DESKTOP"),
        ("Session", "XDG_SESSION_TYPE"),
        ("Wayland display", "WAYLAND_DISPLAY"),
        ("X11 display", "DISPLAY"),
    ] {
        if let Ok(value) = std::env::var(variable) {
            if !value.trim().is_empty() {
                lines.push(format!("{label}: {value}"));
            }
        }
    }

    if let Ok(directory) = std::env::current_dir() {
        lines.push(format!("Working directory: {}", directory.display()));
    }

    lines.join("\n")
}

fn os_release_value(key: &str) -> Option<String> {
    let contents = fs::read_to_string("/etc/os-release").ok()?;

    for line in contents.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };

        if name == key {
            return Some(value.trim_matches('"').to_string());
        }
    }

    None
}
