//! Local, LLM-free command router for the most common desktop actions.
//!
//! Voice control should feel instant for things like "open Firefox", "go to
//! YouTube", "switch to Code", "next song", or "volume up". Sending those
//! through the model costs seconds; this module recognizes them directly and
//! turns them into tool calls, falling back to the model for anything else.
//!
//! Aliases can be extended in `~/.config/hyusk/commands.json`:
//!
//! ```json
//! {
//!   "apps":  { "kitty": ["kitty"], "notes": ["flatpak", "run", "com.x.Notes"] },
//!   "sites": { "hn": "https://news.ycombinator.com" }
//! }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

/// One concrete action the router can take without the model.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Launch {
        program: String,
        args: Vec<String>,
        label: String,
    },
    OpenUrl {
        url: String,
        label: String,
    },
    SwitchWindow {
        query: String,
    },
    Media {
        action: String,
        level: Option<u32>,
    },
    Shell {
        command: String,
    },
    Screenshot,
    Remember {
        text: String,
    },
}

impl Action {
    /// The tool that executes this action.
    pub fn tool_name(&self) -> &'static str {
        match self {
            Action::Launch { .. } => "process",
            Action::OpenUrl { .. } => "computer",
            Action::SwitchWindow { .. } => "window",
            Action::Media { .. } => "media",
            Action::Shell { .. } => "shell",
            Action::Screenshot => "computer",
            Action::Remember { .. } => "memory",
        }
    }

    /// The tool input as a JSON string.
    pub fn tool_input(&self) -> String {
        match self {
            Action::Launch { program, args, .. } => serde_json::json!({
                "action": "launch",
                "program": program,
                "args": args,
            })
            .to_string(),

            Action::OpenUrl { url, .. } => serde_json::json!({
                "action": "open_url",
                "url": url,
            })
            .to_string(),

            Action::SwitchWindow { query } => serde_json::json!({
                "action": "activate",
                "query": query,
            })
            .to_string(),

            Action::Media { action, level } => {
                let mut value = serde_json::json!({ "action": action });

                if let Some(level) = level {
                    value["level"] = serde_json::json!(level);
                }

                value.to_string()
            }

            Action::Shell { command } => serde_json::json!({
                "command": command,
                "mode": "foreground",
            })
            .to_string(),

            Action::Screenshot => serde_json::json!({
                "action": "screenshot",
                "inline_image": false,
            })
            .to_string(),

            Action::Remember { text } => serde_json::json!({
                "action": "remember",
                "text": text,
            })
            .to_string(),
        }
    }

    /// Short, natural sentence spoken back to the user.
    fn spoken(&self) -> String {
        match self {
            Action::Launch { label, .. } => format!("Opening {label}."),
            Action::OpenUrl { label, .. } => format!("Opening {label}."),
            Action::SwitchWindow { query } => format!("Switching to {query}."),
            Action::Media { action, level } => match (action.as_str(), level) {
                ("play", _) => "Playing.".to_string(),
                ("pause", _) => "Paused.".to_string(),
                ("next", _) => "Next track.".to_string(),
                ("previous", _) => "Previous track.".to_string(),
                ("stop", _) => "Stopped.".to_string(),
                ("volume_up", _) => "Volume up.".to_string(),
                ("volume_down", _) => "Volume down.".to_string(),
                ("volume", Some(level)) => format!("Volume set to {level} percent."),
                _ => "Done.".to_string(),
            },
            Action::Shell { command } if command.contains("lock") => {
                "Locking the screen.".to_string()
            }
            Action::Shell { command } if command.contains("unmute") => "Unmuted.".to_string(),
            Action::Shell { command } if command.contains("mute") => "Muted.".to_string(),
            Action::Shell { .. } => "Done.".to_string(),
            Action::Screenshot => "Screenshot taken.".to_string(),
            Action::Remember { .. } => "Noted.".to_string(),
        }
    }
}

/// A parsed plan of one or more instant actions.
#[derive(Debug, Clone)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub spoken: String,
}

impl Plan {
    /// Tools that must be registered for this plan to run.
    pub fn tools(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.actions.iter().map(Action::tool_name).collect();
        names.sort();
        names.dedup();
        names
    }
}

#[derive(Debug, Default, Deserialize)]
struct Config {
    #[serde(default)]
    apps: HashMap<String, Vec<String>>,
    #[serde(default)]
    sites: HashMap<String, String>,
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|_| PathBuf::from("/tmp"));

    base.join("hyusk").join("commands.json")
}

fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

fn builtin_apps() -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::from([
        ("firefox", vec!["firefox"]),
        ("brave", vec!["flatpak", "run", "com.brave.Browser"]),
        ("brave browser", vec!["flatpak", "run", "com.brave.Browser"]),
        ("chrome", vec!["flatpak", "run", "com.google.Chrome"]),
        ("google chrome", vec!["flatpak", "run", "com.google.Chrome"]),
        ("chromium", vec!["chromium"]),
        ("terminal", vec!["ptyxis"]),
        ("console", vec!["ptyxis"]),
        ("ptyxis", vec!["ptyxis"]),
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

fn builtin_sites() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("youtube", "https://www.youtube.com"),
        ("youtube music", "https://music.youtube.com"),
        ("music", "https://music.youtube.com"),
        ("gmail", "https://mail.google.com"),
        ("google", "https://www.google.com"),
        ("google maps", "https://maps.google.com"),
        ("maps", "https://maps.google.com"),
        ("github", "https://github.com"),
        ("google drive", "https://drive.google.com"),
        ("drive", "https://drive.google.com"),
        ("google calendar", "https://calendar.google.com"),
        ("calendar", "https://calendar.google.com"),
        ("chatgpt", "https://chatgpt.com"),
        ("whatsapp", "https://web.whatsapp.com"),
        ("reddit", "https://www.reddit.com"),
        ("twitter", "https://x.com"),
        ("x", "https://x.com"),
        ("instagram", "https://www.instagram.com"),
        ("linkedin", "https://www.linkedin.com"),
        ("amazon", "https://www.amazon.com"),
        ("netflix", "https://www.netflix.com"),
        ("hacker news", "https://news.ycombinator.com"),
        ("hn", "https://news.ycombinator.com"),
        ("openai", "https://openai.com"),
        ("openrouter", "https://openrouter.ai"),
    ])
}

/// Search-URL prefixes for `<engine>` in "search X on <engine>".
fn search_prefix(engine: &str) -> Option<&'static str> {
    match engine {
        "google" => Some("https://www.google.com/search?q="),
        "youtube" => Some("https://www.youtube.com/results?search_query="),
        "youtube music" | "music" => Some("https://music.youtube.com/search?q="),
        "github" => Some("https://github.com/search?q="),
        "amazon" => Some("https://www.amazon.com/s?k="),
        "wikipedia" => Some("https://en.wikipedia.org/w/index.php?search="),
        "reddit" => Some("https://www.reddit.com/search/?q="),
        "maps" | "google maps" => Some("https://www.google.com/maps/search/"),
        _ => None,
    }
}

fn percent_encode(query: &str) -> String {
    let mut output = String::new();

    for byte in query.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char)
            }
            b' ' => output.push('+'),
            other => output.push_str(&format!("%{other:02X}")),
        }
    }

    output
}

fn resolve_site(target: &str, config: &Config) -> Option<String> {
    let key = target.trim().to_lowercase();

    if let Some(url) = config.sites.get(&key) {
        return Some(url.clone());
    }

    if let Some(url) = builtin_sites().get(key.as_str()) {
        return Some((*url).to_string());
    }

    if looks_like_url(&key) {
        return Some(ensure_scheme(&key));
    }

    None
}

fn resolve_app(target: &str, config: &Config) -> Option<(String, Vec<String>, String)> {
    let key = target.trim().to_lowercase();

    if let Some(parts) = config.apps.get(&key) {
        if let Some((program, args)) = parts.split_first() {
            return Some((program.clone(), args.to_vec(), key));
        }
    }

    let builtins = builtin_apps();

    if let Some(parts) = builtins.get(key.as_str()) {
        let mut parts = parts.iter().map(|part| part.to_string());
        let program = parts.next()?;
        return Some((program, parts.collect(), key));
    }

    None
}

fn looks_like_url(token: &str) -> bool {
    !token.contains(' ')
        && (token.starts_with("http://")
            || token.starts_with("https://")
            || (token.contains('.') && !token.ends_with('.')))
}

fn ensure_scheme(token: &str) -> String {
    if token.starts_with("http://") || token.starts_with("https://") {
        token.to_string()
    } else {
        format!("https://{token}")
    }
}

fn strip_prefix_any<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for prefix in prefixes {
        if let Some(rest) = text.strip_prefix(prefix) {
            return Some(rest.trim());
        }
    }

    None
}

fn clean(raw: &str) -> String {
    let mut text = raw.trim().to_lowercase();

    for filler in [
        "hey hyusk ",
        "hey hyusk, ",
        "hyusk ",
        "hyusk, ",
        "please ",
        "can you ",
        "could you ",
        "would you ",
    ] {
        if let Some(rest) = text.strip_prefix(filler) {
            text = rest.to_string();
        }
    }

    for suffix in [" please", " for me", " now", " thanks"] {
        if let Some(rest) = text.strip_suffix(suffix) {
            text = rest.to_string();
        }
    }

    text.trim_matches(|character: char| {
        character.is_whitespace() || matches!(character, '.' | '!' | '?')
    })
    .to_string()
}

fn looks_like_action_start(text: &str) -> bool {
    const STARTS: &[&str] = &[
        "open ",
        "launch ",
        "start ",
        "run ",
        "go to ",
        "goto ",
        "navigate to ",
        "visit ",
        "switch ",
        "show ",
        "focus ",
        "bring ",
        "play",
        "pause",
        "resume",
        "continue",
        "next",
        "previous",
        "skip",
        "stop",
        "volume",
        "mute",
        "unmute",
        "search ",
        "lock",
        "screenshot",
        "take a note",
        "note that",
        "remember ",
    ];

    STARTS.iter().any(|start| text.starts_with(start))
}

fn split_commands(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = vec![text.to_string()];

    for separator in [" and then ", " then ", " and "] {
        let mut next = Vec::new();

        for part in parts {
            if separator == " and " {
                let pieces: Vec<&str> = part.split(separator).collect();
                let splittable = pieces.len() > 1
                    && pieces
                        .iter()
                        .all(|piece| looks_like_action_start(piece.trim()));

                if splittable {
                    next.extend(pieces.into_iter().map(|piece| piece.to_string()));
                    continue;
                }
            } else {
                next.extend(part.split(separator).map(|piece| piece.to_string()));
                continue;
            }

            next.push(part);
        }

        parts = next;
    }

    parts
}

fn parse_clause(clause: &str, config: &Config) -> Option<Action> {
    let clause = clause.trim();

    if clause.is_empty() {
        return None;
    }

    // Search engine: "search X on youtube", or "search X" (default Google).
    if let Some(rest) = strip_prefix_any(clause, &["search ", "google ", "look up "]) {
        if let Some((query, engine)) = rest.rsplit_once(" on ") {
            if let Some(prefix) = search_prefix(engine.trim()) {
                let query = query.trim();
                return Some(Action::OpenUrl {
                    url: format!("{prefix}{}", percent_encode(query)),
                    label: format!("{} for {query}", title_case(engine.trim())),
                });
            }
        }

        if !rest.is_empty() {
            return Some(Action::OpenUrl {
                url: format!(
                    "https://www.google.com/search?q={}",
                    percent_encode(rest.trim())
                ),
                label: format!("Google for {}", rest.trim()),
            });
        }
    }

    // "play <query> on youtube" is a search, not media control.
    if let Some(rest) = strip_prefix_any(clause, &["play ", "put on "]) {
        if let Some((query, engine)) = rest.rsplit_once(" on ") {
            if let Some(prefix) = search_prefix(engine.trim()) {
                let query = query.trim();
                return Some(Action::OpenUrl {
                    url: format!("{prefix}{}", percent_encode(query)),
                    label: format!("{} for {query}", title_case(engine.trim())),
                });
            }
        }
    }

    if let Some(action) = parse_media(clause) {
        return Some(action);
    }

    if let Some(action) = parse_shell(clause) {
        return Some(action);
    }

    if let Some(query) = strip_prefix_any(
        clause,
        &[
            "switch to ",
            "switch ",
            "show me ",
            "show ",
            "focus ",
            "bring up ",
            "bring ",
        ],
    ) {
        if !query.is_empty() {
            return Some(Action::SwitchWindow {
                query: query.to_string(),
            });
        }
    }

    if let Some(target) = strip_prefix_any(
        clause,
        &[
            "open ",
            "launch ",
            "start ",
            "run ",
            "go to ",
            "goto ",
            "navigate to ",
            "visit ",
        ],
    ) {
        if target.is_empty() {
            return None;
        }

        if let Some(url) = resolve_site(target, config) {
            return Some(Action::OpenUrl {
                url,
                label: title_case(target),
            });
        }

        if let Some((program, args, label)) = resolve_app(target, config) {
            return Some(Action::Launch {
                program,
                args,
                label: title_case(&label),
            });
        }

        return None;
    }

    None
}

fn parse_media(clause: &str) -> Option<Action> {
    let media = |action: &str| {
        Some(Action::Media {
            action: action.to_string(),
            level: None,
        })
    };

    match clause {
        "play" | "play music" | "play some music" | "resume" | "resume music" | "continue"
        | "continue music" | "play the music" => return media("play"),

        "pause" | "pause music" | "pause the music" | "stop the music" => return media("pause"),

        "next" | "next song" | "next track" | "skip" | "skip song" | "skip track" => {
            return media("next")
        }

        "previous" | "previous song" | "previous track" | "last song" | "go back a song" => {
            return media("previous")
        }

        "stop" | "stop playback" => return media("stop"),

        "volume up" | "turn it up" | "turn the volume up" | "louder" => return media("volume_up"),

        "volume down" | "turn it down" | "turn the volume down" | "quieter" => {
            return media("volume_down")
        }

        "mute" | "mute the volume" | "mute the sound" => {
            return Some(Action::Shell {
                command: "pactl set-sink-mute @DEFAULT_SINK@ 1".to_string(),
            })
        }

        "unmute" | "unmute the volume" | "unmute the sound" => {
            return Some(Action::Shell {
                command: "pactl set-sink-mute @DEFAULT_SINK@ 0".to_string(),
            })
        }

        _ => {}
    }

    // "set volume to 30", "volume 30", "set the volume to 30 percent"
    let volume = clause
        .strip_prefix("set volume to ")
        .or_else(|| clause.strip_prefix("set the volume to "))
        .or_else(|| clause.strip_prefix("volume "));

    if let Some(rest) = volume {
        let digits: String = rest
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();

        if let Ok(level) = digits.parse::<u32>() {
            return Some(Action::Media {
                action: "volume".to_string(),
                level: Some(level.min(100)),
            });
        }
    }

    None
}

fn parse_shell(clause: &str) -> Option<Action> {
    if matches!(
        clause,
        "lock" | "lock screen" | "lock the screen" | "lock session" | "lock the session"
    ) {
        return Some(Action::Shell {
            command: "loginctl lock-session".to_string(),
        });
    }

    if clause == "screenshot"
        || clause == "take a screenshot"
        || clause == "take a screenshot of the screen"
    {
        return Some(Action::Screenshot);
    }

    for prefix in [
        "take a note ",
        "note that ",
        "remember that ",
        "make a note ",
    ] {
        if let Some(rest) = clause.strip_prefix(prefix) {
            let text = rest.trim();

            if !text.is_empty() {
                return Some(Action::Remember {
                    text: text.to_string(),
                });
            }
        }
    }

    None
}

fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut characters = word.chars();

            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse an utterance into instant actions, or `None` to use the model.
pub fn parse(raw: &str) -> Option<Plan> {
    let cleaned = clean(raw);

    if cleaned.is_empty() {
        return None;
    }

    let config = load_config();
    let mut actions = Vec::new();

    for clause in split_commands(&cleaned) {
        let clause = clause.trim();

        if clause.is_empty() {
            continue;
        }

        actions.push(parse_clause(clause, &config)?);
    }
    if actions.is_empty() {
        return None;
    }

    let spoken = actions
        .iter()
        .map(Action::spoken)
        .collect::<Vec<_>>()
        .join(" ");

    Some(Plan { actions, spoken })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(action: &Action) -> &'static str {
        action.tool_name()
    }

    #[test]
    fn opens_known_site_before_app() {
        let plan = parse("open youtube").expect("parses");

        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(plan.actions[0], Action::OpenUrl { .. }));
        assert_eq!(tool(&plan.actions[0]), "computer");
    }

    #[test]
    fn opens_app() {
        let plan = parse("open firefox").expect("parses");

        match &plan.actions[0] {
            Action::Launch { program, .. } => assert_eq!(program, "firefox"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn opens_flatpak_browser() {
        let plan = parse("open brave").expect("parses");

        match &plan.actions[0] {
            Action::Launch { program, args, .. } => {
                assert_eq!(program, "flatpak");
                assert_eq!(
                    args,
                    &vec!["run".to_string(), "com.brave.Browser".to_string()]
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn opens_domain() {
        let plan = parse("go to github.com").expect("parses");

        match &plan.actions[0] {
            Action::OpenUrl { url, .. } => assert_eq!(url, "https://github.com"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn searches_on_engine() {
        let plan = parse("search lofi beats on youtube").expect("parses");

        match &plan.actions[0] {
            Action::OpenUrl { url, .. } => {
                assert!(url.starts_with("https://www.youtube.com/results?search_query="));
                assert!(url.contains("lofi+beats"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn switches_window() {
        let plan = parse("switch to code").expect("parses");

        assert_eq!(tool(&plan.actions[0]), "window");
        match &plan.actions[0] {
            Action::SwitchWindow { query } => assert_eq!(query, "code"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn controls_media_and_volume() {
        assert!(matches!(parse("play"), Some(Plan { .. })));
        assert!(matches!(parse("next song"), Some(Plan { .. })));
        assert!(matches!(parse("volume up"), Some(Plan { .. })));

        let plan = parse("set volume to 30").expect("parses");
        match &plan.actions[0] {
            Action::Media { action, level } => {
                assert_eq!(action, "volume");
                assert_eq!(*level, Some(30));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn multi_step_command() {
        let plan = parse("open brave and go to youtube").expect("parses");

        assert_eq!(plan.actions.len(), 2);
        assert!(matches!(plan.actions[0], Action::Launch { .. }));
        assert!(matches!(plan.actions[1], Action::OpenUrl { .. }));
    }

    #[test]
    fn remembers_note() {
        let plan = parse("take a note buy milk").expect("parses");

        match &plan.actions[0] {
            Action::Remember { text } => assert_eq!(text, "buy milk"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn falls_back_for_freeform() {
        assert!(parse("what is the weather like today").is_none());
        assert!(parse("summarize this article").is_none());
        assert!(parse("open").is_none());
    }
}
