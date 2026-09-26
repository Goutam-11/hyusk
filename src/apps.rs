//! Friendly application names -> launch commands.
//!
//! Shared by the instant command router and the `process` tool so both know
//! that, for example, `brave` here means `flatpak run com.brave.Browser`.
//! Keeping one table avoids the agent trying `brave`, `google-chrome`, and
//! `firefox` in turn when the first two are Flatpaks.

use std::collections::HashMap;

fn table() -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::from([
        ("firefox", vec!["firefox"]),
        ("brave", vec!["flatpak", "run", "com.brave.Browser"]),
        ("brave browser", vec!["flatpak", "run", "com.brave.Browser"]),
        (
            "youtube music",
            vec![
                "flatpak",
                "run",
                "com.brave.Browser",
                "https://music.youtube.com",
            ],
        ),
        (
            "youtube-music",
            vec![
                "flatpak",
                "run",
                "com.brave.Browser",
                "https://music.youtube.com",
            ],
        ),
        ("chrome", vec!["flatpak", "run", "com.google.Chrome"]),
        ("google chrome", vec!["flatpak", "run", "com.google.Chrome"]),
        ("chromium", vec!["chromium"]),
        ("terminal", vec!["ptyxis", "--new-window"]),
        ("console", vec!["ptyxis", "--new-window"]),
        ("ptyxis", vec!["ptyxis", "--new-window"]),
        ("gnome terminal", vec!["gnome-terminal"]),
        ("files", vec!["nautilus"]),
        ("file manager", vec!["nautilus"]),
        ("nautilus", vec!["nautilus"]),
        ("calculator", vec!["gnome-calculator"]),
        ("calc", vec!["gnome-calculator"]),
        ("settings", vec!["gnome-control-center"]),
        ("system settings", vec!["gnome-control-center"]),
        ("control center", vec!["gnome-control-center"]),
        ("text editor", vec!["gnome-text-editor"]),
        ("editor", vec!["gnome-text-editor"]),
        ("code", vec!["code"]),
        ("vs code", vec!["code"]),
        ("vscode", vec!["code"]),
        ("visual studio code", vec!["code"]),
        ("spotify", vec!["spotify"]),
        ("discord", vec!["discord"]),
        ("slack", vec!["slack"]),
        ("vlc", vec!["vlc"]),
        ("thunderbird", vec!["thunderbird"]),
        ("camera", vec!["cheese"]),
        ("libreoffice", vec!["libreoffice"]),
        ("office", vec!["libreoffice"]),
        (
            "podman desktop",
            vec!["flatpak", "run", "io.podman_desktop.PodmanDesktop"],
        ),
        (
            "gradia",
            vec!["flatpak", "run", "be.alexandervanhee.gradia"],
        ),
        (
            "exhibit",
            vec!["flatpak", "run", "io.github.nokse22.Exhibit"],
        ),
    ])
}

/// Resolve a friendly or raw program name to `(program, args)`.
///
/// A name that is not in the table resolves to itself with no extra args, so
/// callers can always use the result.
pub fn resolve(name: &str) -> (String, Vec<String>) {
    let key = name.trim().to_lowercase();

    if let Some(parts) = table().get(key.as_str()) {
        let mut parts = parts.iter().map(|part| part.to_string());
        let program = parts.next().unwrap_or_else(|| name.to_string());

        return (program, parts.collect());
    }

    (name.to_string(), Vec::new())
}

/// Whether `name` resolves to a known application alias.
pub fn resolves(name: &str) -> bool {
    table().contains_key(name.trim().to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_flatpak_browser() {
        let (program, args) = resolve("brave");

        assert_eq!(program, "flatpak");
        assert_eq!(
            args,
            vec!["run".to_string(), "com.brave.Browser".to_string()]
        );
    }

    #[test]
    fn resolves_simple_app() {
        let (program, args) = resolve("Firefox");

        assert_eq!(program, "firefox");
        assert!(args.is_empty());
    }

    #[test]
    fn unknown_passes_through() {
        let (program, args) = resolve("/usr/bin/something");

        assert_eq!(program, "/usr/bin/something");
        assert!(args.is_empty());
    }

    #[test]
    fn youtube_music_opens_in_brave() {
        let (program, args) = resolve("youtube-music");
        assert_eq!(program, "flatpak");
        assert_eq!(
            args,
            vec!["run", "com.brave.Browser", "https://music.youtube.com"]
        );
        assert!(resolves("youtube-music"));
    }
}
