//! Local, LLM-free command router for the most common desktop actions.
//!
//! Voice control should feel instant for things like "open Firefox", "go to
//! YouTube", "switch to Code", "next song", or "volume up". Sending those
//! through the model costs seconds; this module recognizes them directly and
//! turns them into tool calls, falling back to the model for anything else.
//!
//! Aliases and native workflows can be extended in `~/.config/hyusk/commands.json`:
//!
//! ```json
//! {
//!   "apps":  { "kitty": ["kitty"], "notes": ["flatpak", "run", "com.x.Notes"] },
//!   "sites": { "hn": "https://news.ycombinator.com" }
//! }
//! ```
//!
//! A workflow is a named sequence of the same deterministic commands:
//!
//! ```json
//! {
//!   "workflows": [{
//!     "name": "start work",
//!     "phrases": ["start my workday", "begin work"],
//!     "steps": ["open code", "open slack", "set timer for 25 minutes"]
//!   }]
//! }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

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
    Timer {
        action: String,
        seconds: Option<u64>,
    },
    Schedule {
        kind: String,
        value: String,
        seconds: u64,
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
            Action::Timer { .. } => "timer",
            Action::Schedule { .. } => "scheduler",
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

            Action::Timer { action, seconds } => serde_json::json!({
                "action": action,
                "seconds": seconds,
            })
            .to_string(),

            Action::Schedule {
                kind,
                value,
                seconds,
            } => {
                let mut input = serde_json::json!({
                    "action": "schedule",
                    "type": kind,
                    "after_seconds": seconds,
                });
                input[if kind == "workflow" {
                    "workflow"
                } else {
                    "text"
                }] = serde_json::json!(value);
                input.to_string()
            }
        }
    }

    /// Short, natural sentence spoken back to the user.
    fn spoken(&self) -> String {
        match self {
            Action::Launch { program, label, .. } => {
                if program == "mpv" {
                    format!("Playing {label}.")
                } else {
                    format!("Opening {label}.")
                }
            }
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
            Action::Timer {
                action,
                seconds: Some(seconds),
            } if action == "set" => format!("Timer set for {seconds} seconds."),
            Action::Timer { action, .. } if action == "show_time" => {
                "Showing the time.".to_string()
            }
            Action::Timer { .. } => "Done.".to_string(),
            Action::Schedule {
                kind,
                value,
                seconds,
            } if kind == "workflow" => {
                format!("Scheduled {value} in {}.", spoken_duration(*seconds))
            }
            Action::Schedule { value, seconds, .. } => {
                format!(
                    "I’ll remind you to {value} in {}.",
                    spoken_duration(*seconds)
                )
            }
        }
    }
}

fn spoken_duration(seconds: u64) -> String {
    if seconds.is_multiple_of(3600) {
        let hours = seconds / 3600;
        format!("{hours} hour{}", if hours == 1 { "" } else { "s" })
    } else if seconds.is_multiple_of(60) {
        let minutes = seconds / 60;
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    } else {
        format!("{seconds} second{}", if seconds == 1 { "" } else { "s" })
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

#[derive(Debug, Default)]
struct Config {
    apps: HashMap<String, Vec<String>>,
    sites: HashMap<String, String>,
    workflows: Vec<Workflow>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Workflow {
    name: String,
    #[serde(default)]
    phrases: Vec<String>,
    steps: Vec<String>,
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|_| PathBuf::from("/tmp"));

    base.join("hyusk").join("commands.json")
}

fn load_config() -> Config {
    let Ok(contents) = std::fs::read_to_string(config_path()) else {
        return Config::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&contents) else {
        return Config::default();
    };
    let Some(object) = value.as_object() else {
        return Config::default();
    };

    // Parse each top-level section independently. One malformed workflow must
    // not hide otherwise valid app/site aliases (or other workflows).
    let apps = object
        .get("apps")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let sites = object
        .get("sites")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let mut workflows = Vec::new();
    if let Some(entries) = object.get("workflows").and_then(Value::as_array) {
        for workflow in entries.iter().filter_map(parse_workflow) {
            // Ignore exact duplicate records while preserving distinct
            // workflows that happen to share a trigger (the first valid one
            // wins at execution time).
            if !workflows
                .iter()
                .any(|existing: &Workflow| existing == &workflow)
            {
                workflows.push(workflow);
            }
        }
    }

    Config {
        apps,
        sites,
        workflows,
    }
}

fn parse_workflow(value: &Value) -> Option<Workflow> {
    let object = value.as_object()?;
    let name = object.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }

    let steps = object
        .get("steps")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|step| !step.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut phrases = Vec::new();
    if let Some(values) = object.get("phrases").and_then(Value::as_array) {
        for phrase in values.iter().filter_map(Value::as_str) {
            let phrase = clean(phrase);
            if !phrase.is_empty() && !phrases.contains(&phrase) {
                phrases.push(phrase);
            }
        }
    }

    Some(Workflow {
        name: name.to_string(),
        phrases,
        steps,
    })
}

fn workflow_matches(text: &str, workflow: &Workflow) -> bool {
    let mut invocation = clean(text);
    for prefix in [
        "run workflow ",
        "run shortcut ",
        "execute workflow ",
        "start workflow ",
        "run ",
        "execute ",
        "start ",
        "shortcut ",
    ] {
        if let Some(rest) = invocation.strip_prefix(prefix) {
            invocation = rest.trim().to_string();
            break;
        }
    }
    if let Some(rest) = invocation.strip_prefix("my ") {
        invocation = rest.trim().to_string();
    }
    for suffix in [" workflow", " shortcut"] {
        if let Some(rest) = invocation.strip_suffix(suffix) {
            invocation = rest.trim().to_string();
            break;
        }
    }

    let candidates = [
        text,
        text.strip_prefix("run ").unwrap_or(text),
        text.strip_prefix("execute ").unwrap_or(text),
        text.strip_prefix("start ").unwrap_or(text),
        text.strip_prefix("shortcut ").unwrap_or(text),
    ];

    std::iter::once(workflow.name.as_str())
        .chain(workflow.phrases.iter().map(String::as_str))
        .map(clean)
        .any(|name| invocation == name || candidates.iter().any(|candidate| *candidate == name))
}

#[cfg(test)]
fn workflow_for<'a>(text: &str, config: &'a Config) -> Option<&'a Workflow> {
    config
        .workflows
        .iter()
        .find(|workflow| workflow_matches(text, workflow))
}

fn workflow_actions(workflow: &Workflow, config: &Config) -> Result<Vec<Action>, String> {
    let clauses = workflow
        .steps
        .iter()
        .flat_map(|step| split_commands(&clean(step)))
        .collect::<Vec<_>>();
    let actions = parse_actions(clauses, config)?;

    if actions.is_empty() {
        return Err("it has no steps".to_string());
    }

    Ok(actions)
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

    if crate::apps::resolves(&key) {
        let (program, args) = crate::apps::resolve(&key);

        return Some((program, args, key));
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

/// Play `query` for real when a native player is available, otherwise open a
/// YouTube Music search.
///
/// `mpv` + `yt-dlp` start playback immediately and need no clicks, which is
/// what "play X" should mean. Without them the browser can only show results,
/// so the confirmation says so.
fn music_action(query: &str) -> Action {
    if on_path("mpv") && on_path("yt-dlp") {
        return Action::Launch {
            program: "mpv".to_string(),
            args: vec![
                "--no-video".to_string(),
                "--really-quiet".to_string(),
                format!("ytdl://ytsearch1:{query}"),
            ],
            label: query.to_string(),
        };
    }

    Action::OpenUrl {
        url: format!(
            "https://music.youtube.com/search?q={}",
            percent_encode(query)
        ),
        label: if query == "music" {
            "YouTube Music".to_string()
        } else {
            format!("YouTube Music for {query}")
        },
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|directory| directory.join(program).is_file())
        })
        .unwrap_or(false)
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
        "brightness",
        "increase brightness",
        "decrease brightness",
        "brighter",
        "dimmer",
        "what time",
        "tell me the time",
        "set a timer",
        "set timer",
        "remind me ",
        "schedule workflow ",
        "screenshot",
        "take a note",
        "note that",
        "remember ",
    ];

    STARTS.iter().any(|start| text.starts_with(start))
}

fn split_commands(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = vec![text.to_string()];

    for separator in [";", " and then ", " then ", " and "] {
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

    // Music.
    //   "play <query> on <engine>" -> search that site
    //   "play <query>"             -> play it directly if a native player
    //                                 exists, else search YouTube Music
    //   "play music/something"     -> play/serve generic music
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

        let query = rest.trim();

        if matches!(
            query,
            "something"
                | "some music"
                | "music"
                | "some songs"
                | "songs"
                | "a song"
                | "song"
                | "some tunes"
                | "tunes"
        ) {
            return Some(music_action("music"));
        }

        if !query.is_empty() {
            return Some(music_action(query));
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

        // Keep browser choice inside the deterministic command path. This is
        // especially useful in workflows: `open Brave; go to example.com`
        // should not silently fall back to xdg-open's default browser.
        if let Some((url_target, browser_target)) = target.rsplit_once(" in ") {
            let url_target = url_target.trim();
            let browser_target = browser_target.trim();
            if let Some(url) = resolve_site(url_target, config) {
                if let Some((program, mut args, label)) = resolve_app(browser_target, config) {
                    args.push(url);
                    return Some(Action::Launch {
                        program,
                        args,
                        label: format!("{} in {}", title_case(url_target), title_case(&label)),
                    });
                }
            }
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

fn browser_label(label: &str) -> bool {
    matches!(
        label.trim().to_ascii_lowercase().as_str(),
        "brave" | "chrome" | "chromium" | "firefox" | "edge" | "browser"
    )
}

fn parse_actions(
    clauses: impl IntoIterator<Item = String>,
    config: &Config,
) -> Result<Vec<Action>, String> {
    let mut actions = Vec::new();
    let mut browser: Option<(String, Vec<String>, String)> = None;

    for clause in clauses {
        let clause = clause.trim();
        if clause.is_empty() {
            continue;
        }
        let mut action =
            parse_clause(clause, config).ok_or_else(|| format!("unsupported step \"{clause}\""))?;

        if let (
            Some((program, args, label)),
            Action::OpenUrl {
                url,
                label: url_label,
            },
        ) = (browser.as_ref(), &action)
        {
            let mut browser_args = args.clone();
            browser_args.push(url.clone());
            action = Action::Launch {
                program: program.clone(),
                args: browser_args,
                label: format!("{} in {}", url_label, title_case(label)),
            };
        }

        browser = match &action {
            Action::Launch {
                program,
                args,
                label,
            } if browser_label(label) => Some((program.clone(), args.clone(), label.clone())),
            Action::OpenUrl { .. } => browser,
            _ => None,
        };
        actions.push(action);
    }

    Ok(actions)
}

fn parse_media(clause: &str) -> Option<Action> {
    let media = |action: &str| {
        Some(Action::Media {
            action: action.to_string(),
            level: None,
        })
    };

    match clause {
        "play" | "resume" | "resume music" | "continue" | "continue music" => return media("play"),

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
    if let Some(rest) = clause.strip_prefix("remind me in ") {
        if let Some((duration, text)) = rest.split_once(" to ") {
            if let Some(seconds) = parse_duration(duration) {
                if !text.trim().is_empty() {
                    return Some(Action::Schedule {
                        kind: "reminder".to_string(),
                        value: text.trim().to_string(),
                        seconds,
                    });
                }
            }
        }
    }

    if let Some(rest) = clause.strip_prefix("remind me to ") {
        if let Some((text, duration)) = rest.rsplit_once(" in ") {
            if let Some(seconds) = parse_duration(duration) {
                if !text.trim().is_empty() {
                    return Some(Action::Schedule {
                        kind: "reminder".to_string(),
                        value: text.trim().to_string(),
                        seconds,
                    });
                }
            }
        }
    }

    if let Some(rest) = clause.strip_prefix("schedule workflow ") {
        if let Some((name, duration)) = rest.rsplit_once(" in ") {
            if let Some(seconds) = parse_duration(duration) {
                if !name.trim().is_empty() {
                    return Some(Action::Schedule {
                        kind: "workflow".to_string(),
                        value: name.trim().to_string(),
                        seconds,
                    });
                }
            }
        }
    }

    if matches!(
        clause,
        "what time is it" | "tell me the time" | "show the time" | "show time"
    ) {
        return Some(Action::Timer {
            action: "show_time".to_string(),
            seconds: None,
        });
    }

    if let Some(rest) = clause
        .strip_prefix("set a timer for ")
        .or_else(|| clause.strip_prefix("set timer for "))
    {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if let Some(number) = parts.first().and_then(|value| value.parse::<u64>().ok()) {
            let seconds = if parts.get(1).is_some_and(|unit| unit.starts_with("hour")) {
                number.saturating_mul(3600)
            } else if parts
                .get(1)
                .is_some_and(|unit| unit.starts_with("minute") || *unit == "min")
            {
                number.saturating_mul(60)
            } else {
                number
            };
            return Some(Action::Timer {
                action: "set".to_string(),
                seconds: Some(seconds),
            });
        }
    }

    if matches!(clause, "brightness up" | "increase brightness" | "brighter") {
        return Some(Action::Shell {
            command: "brightnessctl set +5%".to_string(),
        });
    }
    if matches!(clause, "brightness down" | "decrease brightness" | "dimmer") {
        return Some(Action::Shell {
            command: "brightnessctl set 5%-".to_string(),
        });
    }
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

fn parse_duration(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace();
    let amount = match parts.next()? {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "ten" => 10,
        "fifteen" => 15,
        "twenty" => 20,
        "thirty" => 30,
        value => value.parse::<u64>().ok()?,
    };
    let multiplier = match parts.next().unwrap_or("seconds") {
        unit if unit.starts_with("hour") => 3_600,
        unit if unit.starts_with("minute") || unit == "min" || unit == "mins" => 60,
        unit if unit.starts_with("second") || unit == "sec" || unit == "secs" => 1,
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(amount.saturating_mul(multiplier))
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

    let matching_workflows: Vec<_> = config
        .workflows
        .iter()
        .filter(|workflow| workflow_matches(&cleaned, workflow))
        .collect();

    if !matching_workflows.is_empty() {
        let mut error = "it has no valid steps".to_string();

        // If duplicate entries share a trigger, use the first complete one;
        // a stale/broken duplicate must not prevent a valid one from running.
        for workflow in matching_workflows {
            match workflow_actions(workflow, &config) {
                Ok(actions) => {
                    let spoken = format!(
                        "Running {}. {}",
                        title_case(&workflow.name),
                        actions
                            .iter()
                            .map(Action::spoken)
                            .collect::<Vec<_>>()
                            .join(" ")
                    );

                    return Some(Plan { actions, spoken });
                }
                Err(reason) => error = reason,
            }
        }

        let workflow = config
            .workflows
            .iter()
            .find(|workflow| workflow_matches(&cleaned, workflow))
            .expect("matching_workflows is not empty");
        return Some(Plan {
            actions: Vec::new(),
            spoken: format!(
                "I couldn't run {} because {error}. Edit the workflow and try again.",
                title_case(&workflow.name)
            ),
        });
    }

    let actions = parse_actions(split_commands(&cleaned), &config).ok()?;
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
    fn matches_named_workflow_and_alias_phrase() {
        let config = Config {
            apps: HashMap::new(),
            sites: HashMap::new(),
            workflows: vec![Workflow {
                name: "focus mode".to_string(),
                phrases: vec!["start my focus session".to_string()],
                steps: vec!["mute".to_string()],
            }],
        };

        assert_eq!(
            workflow_for("start my focus session", &config).map(|workflow| workflow.name.as_str()),
            Some("focus mode")
        );
        assert_eq!(
            workflow_for("run focus mode", &config).map(|workflow| workflow.name.as_str()),
            Some("focus mode")
        );
        assert_eq!(
            workflow_for("run my focus mode workflow", &config)
                .map(|workflow| workflow.name.as_str()),
            Some("focus mode")
        );
    }

    #[test]
    fn malformed_workflow_entries_are_ignored_individually() {
        let malformed = serde_json::json!({
            "name": "",
            "steps": []
        });
        let valid = serde_json::json!({
            "name": "focus mode",
            "phrases": ["focus", 42, "focus"],
            "steps": ["mute", "", 42]
        });

        assert!(parse_workflow(&malformed).is_none());
        assert_eq!(
            parse_workflow(&valid),
            Some(Workflow {
                name: "focus mode".to_string(),
                phrases: vec!["focus".to_string()],
                steps: vec!["mute".to_string()],
            })
        );

        let empty_named = serde_json::json!({
            "name": "empty workflow",
            "steps": []
        });
        let workflow = parse_workflow(&empty_named).expect("named empty workflow is retained");
        assert_eq!(
            workflow_actions(&workflow, &Config::default()),
            Err("it has no steps".to_string())
        );
    }

    #[test]
    fn workflow_steps_are_atomic_and_support_semicolon_chains() {
        let config = Config::default();
        let workflow = Workflow {
            name: "morning".to_string(),
            phrases: Vec::new(),
            steps: vec!["open terminal; set timer for 25 minutes".to_string()],
        };
        let actions = workflow_actions(&workflow, &config).expect("valid workflow");
        assert_eq!(actions.len(), 2);
        assert!(matches!(actions[0], Action::Launch { .. }));
        assert!(matches!(actions[1], Action::Timer { .. }));

        let broken = Workflow {
            steps: vec!["mute".to_string(), "do something unknown".to_string()],
            ..workflow
        };
        assert_eq!(
            workflow_actions(&broken, &config),
            Err("unsupported step \"do something unknown\"".to_string())
        );
    }

    #[test]
    fn duplicate_trigger_can_use_the_first_valid_workflow() {
        let config = Config {
            workflows: vec![
                Workflow {
                    name: "broken focus".to_string(),
                    phrases: vec!["focus".to_string()],
                    steps: vec!["not a command".to_string()],
                },
                Workflow {
                    name: "working focus".to_string(),
                    phrases: vec!["focus".to_string()],
                    steps: vec!["mute".to_string()],
                },
            ],
            ..Config::default()
        };
        let matching: Vec<_> = config
            .workflows
            .iter()
            .filter(|workflow| workflow_matches("focus", workflow))
            .collect();
        let selected = matching
            .into_iter()
            .find_map(|workflow| workflow_actions(workflow, &config).ok())
            .expect("second duplicate is valid");
        assert_eq!(selected.len(), 1);
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
    fn schedules_relative_reminders_without_a_model() {
        let plan = parse("remind me in ten minutes to stretch").expect("parses");

        assert_eq!(plan.actions.len(), 1);
        assert_eq!(tool(&plan.actions[0]), "scheduler");
        assert!(matches!(
            &plan.actions[0],
            Action::Schedule { kind, value, seconds }
                if kind == "reminder" && value == "stretch" && *seconds == 600
        ));

        let alternate = parse("remind me to drink water in 30 minutes").expect("parses");
        assert!(matches!(
            &alternate.actions[0],
            Action::Schedule { value, seconds, .. }
                if value == "drink water" && *seconds == 1_800
        ));
    }

    #[test]
    fn schedules_named_workflows_without_a_model() {
        let plan = parse("schedule workflow focus mode in an hour").expect("parses");

        assert!(matches!(
            &plan.actions[0],
            Action::Schedule { kind, value, seconds }
                if kind == "workflow" && value == "focus mode" && *seconds == 3_600
        ));
    }

    #[test]
    fn opens_url_in_requested_browser() {
        let plan = parse("open https://example.com in brave").expect("parses");

        match &plan.actions[0] {
            Action::Launch { program, args, .. } => {
                assert_eq!(program, "flatpak");
                assert_eq!(args.last().map(String::as_str), Some("https://example.com"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn chained_url_uses_the_previous_browser() {
        let plan = parse("open brave and go to example.com").expect("parses");

        assert_eq!(plan.actions.len(), 2);
        match &plan.actions[1] {
            Action::Launch { program, args, .. } => {
                assert_eq!(program, "flatpak");
                assert_eq!(args.last().map(String::as_str), Some("https://example.com"));
            }
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
    fn plays_music_on_youtube_music() {
        let plan = parse("play some music on youtube music").expect("parses");

        match &plan.actions[0] {
            Action::OpenUrl { url, .. } => {
                assert!(url.contains("music.youtube.com/search"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn play_something_is_handled_without_the_model() {
        let plan = parse("play something").expect("parses");

        assert!(matches!(
            plan.actions[0],
            Action::OpenUrl { .. } | Action::Launch { .. }
        ));
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
        assert!(matches!(plan.actions[1], Action::Launch { .. }));
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
