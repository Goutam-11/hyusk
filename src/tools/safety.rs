//! Central approval policy for tool calls that can change user data or commit
//! consequential actions. This runs before tool dispatch; unknown tools and
//! unknown shapes fail closed.
//!
//! One table is the source of truth for three consumers: the gate
//! ([`requires_approval`]), the model-facing approval guidance generated in
//! `agent::runner::prompts`, and the preview labels added by A3.

use serde_json::Value;

/// Every tool the composition root can register.
///
/// [`ToolName::policy`] matches this enum exhaustively with no wildcard, so a
/// new tool cannot compile without a decision. `main.rs` walks the registry it
/// actually builds, so a tool registered there without a variant here fails a
/// test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolName {
    Accessibility,
    Browser,
    Computer,
    GnomeDoctor,
    Media,
    Memory,
    Mobile,
    Process,
    Scheduler,
    Shell,
    Task,
    Timer,
    WebSearch,
    Window,
}

/// How the gate decides one call.
#[derive(Clone, Copy, Debug)]
pub enum Decision {
    /// Runs without asking.
    Allow,
    /// Decided from the call's JSON arguments by this rule. A tool that must
    /// always ask uses a rule that returns true.
    Classified(fn(&Value) -> bool),
    /// Decided from the call's arguments and the desktop focus context (D1).
    /// Without context the rule is asked with [`FocusContext::unknown`], so
    /// callers that cannot supply it fail closed.
    Contextual(fn(&Value, &FocusContext) -> bool),
}

/// What the runner knows about the focused window when it asks the gate (D1).
///
/// Typing and key input reach whatever has focus, so the decision depends on
/// it. Both fields are `None` when the focus could not be read, and such a call
/// asks rather than runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusContext {
    /// The focused application's name or app id.
    pub app: Option<String>,
    /// The focused window's title, kept for the approval preview.
    pub title: Option<String>,
}

impl FocusContext {
    /// No readable focus: context-sensitive calls ask.
    pub fn unknown() -> Self {
        Self::default()
    }

    /// The focus the runner read from the `window` tool. An empty application
    /// name is no focus at all, so the call asks.
    pub fn new(app: impl Into<String>, title: impl Into<String>) -> Self {
        let app = app.into();
        let app = (!app.trim().is_empty()).then_some(app);
        Self {
            app,
            title: Some(title.into()),
        }
    }
}

/// One row of the approval policy table.
#[derive(Clone, Copy, Debug)]
pub struct ToolPolicy {
    /// How each call is decided.
    pub decision: Decision,
    /// When this tool asks, written for the model. Generated guidance is
    /// derived from this text, so it must never contradict `decision`.
    pub guidance: &'static str,
    /// Argument keys that identify the call, most important first. The approval
    /// preview names these before any other argument, so the card shows what
    /// the call will do rather than the model's paraphrase.
    pub preview_keys: &'static [&'static str],
}

impl ToolName {
    /// Every tool the policy knows, in name order.
    pub const ALL: &'static [ToolName] = &[
        ToolName::Accessibility,
        ToolName::Browser,
        ToolName::Computer,
        ToolName::GnomeDoctor,
        ToolName::Media,
        ToolName::Memory,
        ToolName::Mobile,
        ToolName::Process,
        ToolName::Scheduler,
        ToolName::Shell,
        ToolName::Task,
        ToolName::Timer,
        ToolName::WebSearch,
        ToolName::Window,
    ];

    /// The name the registry and a model call use.
    pub fn name(self) -> &'static str {
        match self {
            ToolName::Accessibility => "accessibility",
            ToolName::Browser => "browser",
            ToolName::Computer => "computer",
            ToolName::GnomeDoctor => "gnome_doctor",
            ToolName::Media => "media",
            ToolName::Memory => "memory",
            ToolName::Mobile => "mobile",
            ToolName::Process => "process",
            ToolName::Scheduler => "scheduler",
            ToolName::Shell => "shell",
            ToolName::Task => "task",
            ToolName::Timer => "timer",
            ToolName::WebSearch => "web_search",
            ToolName::Window => "window",
        }
    }

    /// Resolve the tool a call names. `None` means the policy does not know it;
    /// callers fail closed.
    pub fn from_name(name: &str) -> Option<Self> {
        ToolName::ALL
            .iter()
            .copied()
            .find(|tool| tool.name() == name)
    }

    /// The explicit decision for this tool. Exhaustive with no wildcard, so a
    /// new variant does not compile until it has a policy.
    pub fn policy(self) -> ToolPolicy {
        match self {
            ToolName::Accessibility => ToolPolicy {
                decision: Decision::Classified(accessibility_requires_approval),
                guidance: "reading the screen, listing apps and windows, focusing, and \
                           setting text run freely; clicking a control whose label or \
                           action says send, delete, remove, pay, purchase, buy, submit, \
                           confirm, transfer, publish, post, share, install, uninstall, \
                           reset, or place an order asks, and an unrecognised action asks",
                preview_keys: &["action", "name", "text", "action_name"],
            },
            ToolName::Browser => ToolPolicy {
                decision: Decision::Classified(browser_requires_approval),
                guidance: "status, start, new_tab, tabs, read, and navigate run freely; \
                           click, fill, and press_key ask, and an unknown action asks too \
                           - the automation profile has its own sign-ins, but the page can \
                           still submit forms, post comments, or buy something",
                preview_keys: &["action", "target_id", "selector", "text", "key", "url"],
            },
            ToolName::Computer => ToolPolicy {
                decision: Decision::Contextual(computer_requires_approval_with_context),
                guidance: "screenshots, OCR, find, cursor, app state, mouse moves, clicks, \
                           scrolling, and setting clipboard text run freely; typing and \
                           single key presses also run freely, except when a terminal is \
                           focused or the focus cannot be read, because the keystrokes \
                           could run a shell command; reading or clearing the clipboard, \
                           click_text or an accessibility click on a consequential label \
                           (send, delete, pay, submit, and so on), and shortcuts that \
                           commit or quit (ctrl, alt, meta, or super with enter, delete, \
                           backspace, or F4; ctrl+q or ctrl+w) ask",
                preview_keys: &["action", "text", "keys", "key"],
            },
            ToolName::GnomeDoctor => ToolPolicy {
                decision: Decision::Allow,
                guidance: "read-only diagnostics never ask",
                preview_keys: &["action"],
            },
            ToolName::Media => ToolPolicy {
                decision: Decision::Allow,
                guidance: "playback control never asks",
                preview_keys: &["action"],
            },
            ToolName::Memory => ToolPolicy {
                decision: Decision::Classified(memory_requires_approval),
                guidance: "remembering, searching, and graph queries run freely; \
                           forgetting asks",
                preview_keys: &["action", "text"],
            },
            ToolName::Mobile => ToolPolicy {
                decision: Decision::Classified(mobile_requires_approval),
                guidance: "listing devices and the routine phone actions (apps, url, \
                           search, status, timers, reminders, ui, media, volume, system \
                           navigation, camera, dial, sms.compose, share, clipboard.write) \
                           run freely; invoking a consequential phone action (send, \
                           delete, install, call, clipboard read or clear, \
                           memory.forget, shell.execute) or an unknown action asks",
                preview_keys: &["action", "phone_action", "device_id"],
            },
            ToolName::Process => ToolPolicy {
                decision: Decision::Classified(process_requires_approval),
                guidance: "a bare launch of a configured app alias (firefox, chrome, \
                           brave, code, and so on) runs freely; extra arguments, an \
                           unknown program, or a raw executable path ask",
                preview_keys: &["program", "args", "action"],
            },
            ToolName::Scheduler => ToolPolicy {
                decision: Decision::Allow,
                guidance: "scheduling, listing, and cancelling a reminder, workflow, or \
                           agent task never asks",
                preview_keys: &["action", "type", "text", "workflow", "after_seconds"],
            },
            ToolName::Shell => ToolPolicy {
                decision: Decision::Classified(shell_requires_approval),
                guidance: "one simple read-only command runs freely only while every path it \
                           names stays inside the project root (pwd, ls, cat, head, tail, rg, \
                           sed -n, find without -delete or -exec, git status, diff, log, \
                           rev-parse, show or branch --list, sort, cargo test, check, build, \
                           fmt, clippy); a read outside the root asks, and a credential path \
                           (environment files, SSH keys, GnuPG, AWS, Kubernetes, netrc, the \
                           GitHub CLI config, keyrings, the automation browser profile) is \
                           refused outright rather than confirmed; a program named by a \
                           path (./ls, /tmp/x/ls) asks even when its bare name is allowlisted; \
                           chaining, pipes, redirection, expansion, in-place writes, and \
                           unknown programs ask, and a detached mode asks unless it launches an \
                           app",
                preview_keys: &["command", "mode"],
            },
            ToolName::Task => ToolPolicy {
                decision: Decision::Classified(task_requires_approval),
                guidance: "a research delegation runs immediately; a workspace \
                           delegation asks once, with its project root pinned; list and \
                           cancel run freely",
                preview_keys: &["action", "profile", "task", "workspace", "label", "id"],
            },
            ToolName::Timer => ToolPolicy {
                decision: Decision::Allow,
                guidance: "setting, listing, showing the time, and cancelling a timer \
                           never asks",
                preview_keys: &["action", "seconds", "label"],
            },
            ToolName::WebSearch => ToolPolicy {
                decision: Decision::Allow,
                guidance: "searching the web never asks",
                preview_keys: &["query", "count", "freshness"],
            },
            ToolName::Window => ToolPolicy {
                decision: Decision::Classified(window_requires_approval),
                guidance: "listing, reading the active window, and activating a window \
                           run freely; closing a window asks",
                preview_keys: &["action", "query", "id"],
            },
        }
    }
}

impl ToolPolicy {
    /// True when this call must be held for the user, given what the runner
    /// knows about the focused window (D1). Without context a contextual row
    /// asks, which is the fail-closed answer.
    pub fn asks_with_context(self, args: &Value, context: &FocusContext) -> bool {
        match self.decision {
            Decision::Allow => false,
            Decision::Classified(rule) => rule(args),
            Decision::Contextual(rule) => rule(args, context),
        }
    }
}

/// The model-facing approval guidance, rendered from the table so the prose
/// cannot drift from the gate: one line per tool, in the order of
/// [`ToolName::ALL`]. The system prompt embeds this text instead of a
/// hand-written list (A1).
pub fn approval_guidance() -> String {
    let mut guidance = String::new();
    for tool in ToolName::ALL {
        guidance.push_str(&format!("- {}: {}\n", tool.name(), tool.policy().guidance));
    }
    guidance
}

/// The key a turn-scoped "allow for this turn" grant applies to: the tool plus
/// the shape of the call (A6).
///
/// The shell shape is the program as written plus its first subcommand, so
/// "git push" and "git status" are different shapes while two `rm` commands
/// share one. A program named by a path keeps the path in its shape, so
/// approving `/tmp/x/ls` never covers `ls` or another path form (S3). Other
/// tools use their `action`. `None` means the shape cannot be determined, and
/// such a call is never covered by a grant.
pub fn grant_key(name: &str, arguments: &str) -> Option<(String, String)> {
    let args = serde_json::from_str::<Value>(arguments).ok()?;
    let shape = match name {
        "shell" => {
            let command = args.get("command").and_then(Value::as_str)?;
            let mut words = simple_command_words(command)?;
            if words.is_empty() {
                return None;
            }
            // The program as written: `/tmp/x/ls` and `ls` are different
            // shapes, so approving one path form never covers another (S3).
            let program = words.remove(0);
            match words.first() {
                Some(subcommand) if !subcommand.starts_with('-') => {
                    format!("{program} {subcommand}")
                }
                _ => program,
            }
        }
        "task" => args
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or("research")
            .to_string(),
        _ => action(&args).to_string(),
    };
    if shape.is_empty() {
        return None;
    }
    Some((name.to_string(), shape))
}

/// The longest approval preview, including the tool name and the ellipsis.
pub const PREVIEW_LIMIT: usize = 120;

/// The exact call the user is asked to approve: the tool name plus its key
/// arguments, normalised to one line and truncated with an ellipsis. The card
/// and the tool result both carry this text, so neither can show the model's
/// paraphrase instead of the call.
///
/// Arguments that are not a JSON object render as the tool name alone; such a
/// call cannot run, and the gate still holds it.
pub fn preview(name: &str, arguments: &str) -> String {
    truncate_preview(&render_preview(name, arguments))
}

/// The preview plus the focused terminal when that is why the gate asks, so a
/// card about typing says where the keystrokes will land (D1).
pub fn preview_with_context(name: &str, arguments: &str, context: &FocusContext) -> String {
    let mut text = render_preview(name, arguments);
    if requires_focus_context(name, arguments) {
        if let Some(app) = context.app.as_deref().map(str::trim) {
            if is_terminal_app(app) {
                text.push_str(&format!(" (focused: {app})"));
            }
        }
    }
    truncate_preview(&text)
}

fn render_preview(name: &str, arguments: &str) -> String {
    let mut text = name.to_string();
    let args = serde_json::from_str::<Value>(arguments).unwrap_or(Value::Null);
    for (key, value) in preview_fields(name, &args) {
        text.push(' ');
        text.push_str(&key);
        text.push('=');
        text.push_str(&value);
    }
    text
}

/// The one sentence that names an armed call. The tool result and every status
/// card use this function, so they always show the same text.
pub fn pending_text(preview: &str) -> String {
    format!("Pending confirmation for: {preview}")
}

fn preview_fields(name: &str, args: &Value) -> Vec<(String, String)> {
    let Value::Object(map) = args else {
        return Vec::new();
    };
    let mut fields = Vec::new();
    let mut rendered = Vec::new();
    let ordered = ToolName::from_name(name)
        .map(|tool| tool.policy().preview_keys)
        .unwrap_or(&[]);
    for key in ordered.iter().copied() {
        if let Some(value) = map.get(key).and_then(preview_value) {
            fields.push((key.to_string(), value));
            rendered.push(key);
        }
    }
    // Remaining arguments are alphabetical, so the text is stable across runs.
    let mut rest: Vec<&String> = map
        .keys()
        .filter(|key| !rendered.contains(&key.as_str()))
        .collect();
    rest.sort();
    for key in rest {
        if let Some(value) = map.get(key).and_then(preview_value) {
            fields.push((key.clone(), value));
        }
    }
    fields
}

fn preview_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        // One line only: newlines and tabs would break the card layout.
        Value::String(text) => {
            let normalised = text.split_whitespace().collect::<Vec<_>>().join(" ");
            (!normalised.is_empty()).then_some(normalised)
        }
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Array(items) => {
            let rendered = items.iter().filter_map(preview_value).collect::<Vec<_>>();
            (!rendered.is_empty()).then(|| format!("[{}]", rendered.join(", ")))
        }
        Value::Object(_) => serde_json::to_string(value).ok(),
    }
}

fn truncate_preview(text: &str) -> String {
    if text.chars().count() <= PREVIEW_LIMIT {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(PREVIEW_LIMIT - 1).collect();
    truncated.push('…');
    truncated
}

/// The policy for a registered tool, or `None` when the name is unknown.
pub fn policy_for(name: &str) -> Option<ToolPolicy> {
    ToolName::from_name(name).map(ToolName::policy)
}

/// Return true when this tool call should be held for user approval.
///
/// Callers that can read the desktop should use
/// [`requires_approval_with_context`]; this wrapper asks for every call whose
/// decision depends on the focused window, which is the fail-closed answer.
pub fn requires_approval(name: &str, arguments: &str) -> bool {
    requires_approval_with_context(name, arguments, &FocusContext::unknown())
}

/// Return true when this tool call should be held for user approval, given the
/// focused window the runner read (D1).
pub fn requires_approval_with_context(name: &str, arguments: &str, context: &FocusContext) -> bool {
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return true;
    };
    // An unknown tool name fails closed: a tool registered without a policy
    // entry must ask, so a new tool can never quietly skip the gate.
    match policy_for(name) {
        Some(policy) => policy.asks_with_context(&args, context),
        None => true,
    }
}

/// True when the decision for this call depends on which window has focus, so
/// the runner reads it only for these calls (D1).
pub fn requires_focus_context(name: &str, arguments: &str) -> bool {
    if name != "computer" {
        return false;
    }
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return false;
    };
    computer_call_uses_focus(&args)
}

fn computer_call_uses_focus(args: &Value) -> bool {
    match action(args) {
        "type_text" | "key_press" | "key_combo" => true,
        "batch" => args
            .get("steps")
            .and_then(Value::as_array)
            .is_some_and(|steps| steps.iter().any(computer_call_uses_focus)),
        _ => false,
    }
}

/// Rewrite the arguments stored for a pending confirmation so that the call
/// the user approves is exactly the call that will run. Task delegation pins
/// the canonical workspace root here; every other tool keeps its arguments.
pub fn approval_arguments(name: &str, arguments: &str) -> String {
    if name != "task" {
        return arguments.to_string();
    }
    let Ok(mut args) = serde_json::from_str::<Value>(arguments) else {
        return arguments.to_string();
    };
    if args.get("action").and_then(Value::as_str) != Some("delegate")
        || args.get("profile").and_then(Value::as_str) != Some("workspace")
    {
        return arguments.to_string();
    }
    // Pin the root the delegation will actually use, whether the model named it
    // or it came from HYUSK_WORKSPACE.
    let Ok(canonical) = crate::tools::task::canonical_workspace(
        args.get("workspace").and_then(Value::as_str),
        crate::tools::task::configured_root().as_deref(),
    ) else {
        return arguments.to_string();
    };
    args["workspace"] = Value::String(canonical.to_string_lossy().into_owned());
    serde_json::to_string(&args).unwrap_or_else(|_| arguments.to_string())
}

fn task_requires_approval(args: &Value) -> bool {
    if action(args) != "delegate" {
        return false;
    }
    // Research is one read-only `client.chat` call: no tools, no filesystem
    // access, no credentials, so it runs immediately. Workspace delegation
    // starts Codex CLI against a project root and asks once. An absent profile
    // is research (the tool's default); an unknown profile fails closed.
    match args.get("profile") {
        None => false,
        Some(value) => value.as_str() != Some("research"),
    }
}

fn memory_requires_approval(args: &Value) -> bool {
    action(args) == "forget"
}

fn window_requires_approval(args: &Value) -> bool {
    action(args) == "close"
}

/// Read-only and navigation actions stay free; every action that mutates the
/// page or types into it asks. The match is a whitelist, so an action the table
/// does not know asks rather than runs (D2).
fn browser_requires_approval(args: &Value) -> bool {
    !matches!(
        action(args),
        "status" | "start" | "new_tab" | "tabs" | "read" | "navigate"
    )
}

fn action(args: &Value) -> &str {
    args.get("action").and_then(Value::as_str).unwrap_or("")
}

/// The project root a shell command may write into: `HYUSK_WORKSPACE` when set,
/// otherwise the working directory, canonicalised once per decision.
pub fn project_root_from(
    configured: Option<&std::ffi::OsStr>,
    working_directory: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let candidate = match configured {
        Some(path) if !path.is_empty() => std::path::PathBuf::from(path),
        _ => working_directory.to_path_buf(),
    };
    candidate.canonicalize().ok()
}

fn project_root(working_directory: &std::path::Path) -> Option<std::path::PathBuf> {
    project_root_from(
        std::env::var_os("HYUSK_WORKSPACE").as_deref(),
        working_directory,
    )
}

/// Resolve an argument the way `sh -c` would see it, without touching the file.
///
/// `~` and `~/…` expand to the home directory; `~user` is not expanded and
/// therefore never trusted. Variables and command substitution already fail
/// [`simple_command_words`], so a path here is literal.
fn absolute_argument(
    argument: &str,
    working_directory: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if argument == "~" || argument.starts_with("~/") {
        let home = std::env::var_os("HOME")?;
        let mut expanded = std::path::PathBuf::from(home);
        if let Some(rest) = argument.strip_prefix("~/") {
            expanded.push(rest);
        }
        return Some(expanded);
    }
    if argument.starts_with('~') {
        return None;
    }
    let path = std::path::Path::new(argument);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    })
}

/// Canonicalise the part of the path that exists, then normalise the rest, so
/// neither `..` nor a symlink can hide a target outside the root.
fn resolve_for_root_check(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;

    let mut existing = path.to_path_buf();
    let mut trailing: Vec<std::ffi::OsString> = Vec::new();
    let mut resolved = loop {
        match existing.canonicalize() {
            Ok(canonical) => break canonical,
            Err(_) => {
                let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
                    return path.to_path_buf();
                };
                trailing.push(name);
                if !existing.pop() {
                    return path.to_path_buf();
                }
            }
        }
    };
    for part in trailing.into_iter().rev() {
        resolved.push(part);
    }
    let mut normalised = std::path::PathBuf::new();
    for component in resolved.components() {
        match component {
            Component::ParentDir => {
                normalised.pop();
            }
            Component::CurDir => {}
            other => normalised.push(other),
        }
    }
    normalised
}

/// True when every path the command names stays inside `root`.
fn path_inside_root(
    argument: &str,
    root: &std::path::Path,
    working_directory: &std::path::Path,
) -> bool {
    let Some(absolute) = absolute_argument(argument, working_directory) else {
        return false;
    };
    resolve_for_root_check(&absolute).starts_with(root)
}

/// Programs whose output is file content or file metadata. They are free only
/// while every path they name stays inside the project root, and the secret
/// denylist refuses known credential locations outright (S2).
const READ_PROGRAMS: &[&str] = &[
    "cat", "head", "tail", "wc", "file", "stat", "ls", "du", "df", "grep", "rg", "sed", "find",
    "sort", "uniq", "cut", "tr",
];

/// One denylist rule. The label is what a refusal names; it never includes the
/// path that matched, so the message cannot echo a credential location.
struct SecretRule {
    label: &'static str,
    matches: fn(&std::path::Path) -> bool,
}

fn path_component_text(path: &std::path::Path) -> Vec<String> {
    use std::path::Component;
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn has_component(parts: &[String], name: &str) -> bool {
    parts.iter().any(|part| part == name)
}

fn matches_environment_file(path: &std::path::Path) -> bool {
    path_component_text(path)
        .iter()
        .any(|part| part.starts_with(".env"))
}

fn matches_ssh_key(path: &std::path::Path) -> bool {
    let parts = path_component_text(path);
    has_component(&parts, ".ssh")
        || parts.last().is_some_and(|name| {
            matches!(
                name.as_str(),
                "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519" | "id_rsa_sk" | "id_ed25519_sk"
            )
        })
}

fn matches_gnupg_home(path: &std::path::Path) -> bool {
    has_component(&path_component_text(path), ".gnupg")
}

fn matches_aws_credentials(path: &std::path::Path) -> bool {
    has_component(&path_component_text(path), ".aws")
}

fn matches_kube_config(path: &std::path::Path) -> bool {
    has_component(&path_component_text(path), ".kube")
}

fn matches_netrc(path: &std::path::Path) -> bool {
    let parts = path_component_text(path);
    has_component(&parts, ".netrc") || has_component(&parts, "_netrc")
}

fn matches_github_config(path: &std::path::Path) -> bool {
    path_component_text(path)
        .windows(2)
        .any(|pair| pair == [".config", "gh"])
}

fn matches_keyring(path: &std::path::Path) -> bool {
    let parts = path_component_text(path);
    has_component(&parts, "keyrings")
        || parts.last().is_some_and(|name| {
            [".keyring", ".keychain", ".kdbx"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        })
}

fn matches_browser_profile(path: &std::path::Path) -> bool {
    let parts = path_component_text(path);
    has_component(&parts, "hyusk-browser-control-profile")
        || has_component(&parts, "browser-control-profile")
}

/// The secret denylist: compiled once as a table so a new credential location
/// is one row, not a change to the classifier (S2). Each rule matches a path as
/// written, so `~` and relative forms are covered without touching `HOME`.
const SECRET_PATH_RULES: &[SecretRule] = &[
    SecretRule {
        label: "environment files",
        matches: matches_environment_file,
    },
    SecretRule {
        label: "SSH keys",
        matches: matches_ssh_key,
    },
    SecretRule {
        label: "GnuPG home",
        matches: matches_gnupg_home,
    },
    SecretRule {
        label: "AWS credentials",
        matches: matches_aws_credentials,
    },
    SecretRule {
        label: "Kubernetes config",
        matches: matches_kube_config,
    },
    SecretRule {
        label: "netrc credentials",
        matches: matches_netrc,
    },
    SecretRule {
        label: "GitHub CLI config",
        matches: matches_github_config,
    },
    SecretRule {
        label: "keyrings",
        matches: matches_keyring,
    },
    SecretRule {
        label: "automation browser profile",
        matches: matches_browser_profile,
    },
];

/// The denylist rule an argument names, if any. A `REV:path` form is checked
/// too, because `git show HEAD:.env` reads a credential from history.
fn secret_rule_for_argument(argument: &str) -> Option<&'static str> {
    let matches = |candidate: &str| {
        SECRET_PATH_RULES
            .iter()
            .find(|rule| (rule.matches)(std::path::Path::new(candidate)))
            .map(|rule| rule.label)
    };
    matches(argument).or_else(|| {
        argument
            .split_once(':')
            .and_then(|(_, path)| (!path.is_empty()).then_some(path))
            .and_then(matches)
    })
}

/// How many leading positionals a program spends on a pattern, a script, or
/// another non-path argument before its file operands begin.
fn read_program_skips(program: &str, explicit_pattern: bool) -> usize {
    if !explicit_pattern && matches!(program, "grep" | "rg" | "sed") {
        1
    } else {
        0
    }
}

/// True when the arguments name a pattern or script explicitly, so no leading
/// positional is a pattern.
fn has_explicit_pattern(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| {
        matches!(
            argument.as_str(),
            "-e" | "--regexp" | "-f" | "--file" | "--expression"
        ) || argument.starts_with("--regexp=")
            || argument.starts_with("--file=")
            || argument.starts_with("--expression=")
            || (argument.starts_with("-e") && argument.len() > 2)
    })
}

/// Every path argument a read command names, or `None` when its shape cannot be
/// classified (which asks). Option values are path candidates too, so a path
/// cannot hide behind `--option=value`.
fn read_command_paths(program: &str, arguments: &[String]) -> Option<Vec<String>> {
    if program == "git" {
        return Some(git_read_paths(arguments));
    }
    let mut positionals = Vec::new();
    let mut option_values = Vec::new();
    let mut options_ended = false;
    for argument in arguments {
        if !options_ended && argument == "--" {
            options_ended = true;
            continue;
        }
        if !options_ended && argument.starts_with('-') && argument.len() > 1 {
            if let Some((_, value)) = argument.split_once('=') {
                option_values.push(value.to_string());
            }
            continue;
        }
        positionals.push(argument.clone());
    }
    let skip = read_program_skips(program, has_explicit_pattern(arguments));
    let mut paths: Vec<String> = positionals.into_iter().skip(skip).collect();
    paths.extend(option_values);
    Some(paths)
}

/// The paths a read-only git subcommand names: everything after `--`, plus the
/// path half of a `REV:path` object name.
fn git_read_paths(arguments: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut after_separator = false;
    for argument in arguments.iter().skip(1) {
        if after_separator {
            paths.push(argument.clone());
            continue;
        }
        if argument == "--" {
            after_separator = true;
            continue;
        }
        if let Some((revision, path)) = argument.split_once(':') {
            if !revision.is_empty() && !path.is_empty() {
                paths.push(path.to_string());
            }
        }
    }
    paths
}

/// True when every path a read command names stays inside the project root.
fn read_arguments_are_free(
    program: &str,
    arguments: &[String],
    root: &std::path::Path,
    working_directory: &std::path::Path,
) -> bool {
    let Some(paths) = read_command_paths(program, arguments) else {
        return false;
    };
    paths
        .iter()
        .all(|path| path_inside_root(path, root, working_directory))
}

/// The project root and working directory a decision is made against.
fn project_and_working_directory() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    Some((
        project_root(std::path::Path::new("."))?,
        std::env::current_dir().ok()?.canonicalize().ok()?,
    ))
}

/// The refusal for a shell call that names a credential path, or `None` when the
/// call names none. A refused read never runs and never asks: the message names
/// the denylist rule, not the path or any value, so the policy cannot leak what
/// it refused (S2).
pub fn secret_refusal(name: &str, arguments: &str) -> Option<String> {
    if name != "shell" {
        return None;
    }
    let args: Value = serde_json::from_str(arguments).ok()?;
    let command = args.get("command").and_then(Value::as_str)?;
    let label = command_secret_rule(command)?;
    Some(format!(
        "Refused by the secret denylist (rule: {label}). Hyusk does not read credential \
         files or their folders; ask the user to open it themselves."
    ))
}

/// The denylist rule a simple read command names, if any.
fn command_secret_rule(command: &str) -> Option<&'static str> {
    let mut words = simple_command_words(command)?;
    if words.is_empty() {
        return None;
    }
    let program = words.remove(0);
    let program = program.rsplit('/').next().unwrap_or(&program).to_string();
    if !READ_PROGRAMS.contains(&program.as_str()) && program != "git" {
        return None;
    }
    let candidates = read_command_paths(&program, &words).unwrap_or_else(|| {
        words
            .iter()
            .filter(|word| !word.starts_with('-'))
            .cloned()
            .collect()
    });
    candidates
        .iter()
        .find_map(|candidate| secret_rule_for_argument(candidate))
}

/// Programs that write files: free only when every path stays in the project.
const PROJECT_FILE_OPERATIONS: &[&str] = &["mkdir", "touch", "cp", "mv", "ln", "install"];

/// Destructive or ownership-changing programs always ask, whatever the paths.
const DESTRUCTIVE_PROGRAMS: &[&str] = &[
    "rm", "rmdir", "shred", "truncate", "chmod", "chown", "chgrp", "dd", "mkfs", "wipefs",
];

/// Every path argument a file operation names, or `None` when an option is not
/// understood (an unknown option asks rather than guessing).
fn file_operation_paths(program: &str, arguments: &[String]) -> Option<Vec<String>> {
    let mut paths = Vec::new();
    let mut index = 0;
    let mut options_ended = false;
    while index < arguments.len() {
        let argument = &arguments[index];
        index += 1;
        if !options_ended && argument == "--" {
            options_ended = true;
            continue;
        }
        if !options_ended && argument.starts_with("--") {
            let (name, inline) = match argument.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (argument.as_str(), None),
            };
            let takes_value = matches!(
                name,
                "--target-directory" | "--mode" | "--reference" | "--date"
            );
            let value = if takes_value {
                match inline {
                    Some(value) => value,
                    None => {
                        let value = arguments.get(index)?.clone();
                        index += 1;
                        value
                    }
                }
            } else {
                if inline.is_some() {
                    return None;
                }
                String::new()
            };
            match name {
                // A value that names a path must also stay inside the root.
                "--target-directory" | "--reference" => paths.push(value),
                // Values that are not paths.
                "--mode" | "--date" => {}
                // Flags that change no path.
                "--parents"
                | "--verbose"
                | "--force"
                | "--interactive"
                | "--no-clobber"
                | "--recursive"
                | "--archive"
                | "--symbolic-link"
                | "--symbolic"
                | "--preserve"
                | "--no-target-directory"
                | "--no-dereference"
                | "--relative"
                | "--logical"
                | "--physical"
                | "--directory"
                | "--no-create"
                | "--preserve-timestamps" => {}
                _ => return None,
            }
            continue;
        }
        if !options_ended && argument.starts_with('-') && argument.len() > 1 {
            let mut cluster = argument[1..].chars().peekable();
            while let Some(flag) = cluster.next() {
                let value_flag = matches!(flag, 't' | 'm' | 'r' | 'd' | 'S' | 'o' | 'g')
                    && matches!(
                        (program, flag),
                        ("mkdir", 'm')
                            | ("install", 'm')
                            | ("install", 'o')
                            | ("install", 'g')
                            | ("cp", 't')
                            | ("mv", 't')
                            | ("ln", 't')
                            | ("install", 't')
                            | ("touch", 'r')
                            | ("touch", 'd')
                            | ("touch", 't')
                    );
                if value_flag {
                    let rest: String = cluster.collect();
                    let value = if rest.is_empty() {
                        let value = arguments.get(index)?.clone();
                        index += 1;
                        value
                    } else {
                        rest
                    };
                    // `touch -r` names a reference file and `-t` on the file
                    // operations names a target directory; the rest are values.
                    let names_a_path =
                        (program == "touch" && flag == 'r') || (program != "touch" && flag == 't');
                    if names_a_path {
                        paths.push(value);
                    }
                    break;
                }
                let allowed = matches!(
                    (program, flag),
                    ("mkdir", 'p')
                        | ("mkdir", 'v')
                        | ("touch", 'a')
                        | ("touch", 'c')
                        | ("touch", 'm')
                        | ("touch", 'h')
                        | ("cp", 'r')
                        | ("cp", 'R')
                        | ("cp", 'a')
                        | ("cp", 'f')
                        | ("cp", 'i')
                        | ("cp", 'n')
                        | ("cp", 'v')
                        | ("cp", 's')
                        | ("cp", 'L')
                        | ("cp", 'P')
                        | ("mv", 'f')
                        | ("mv", 'i')
                        | ("mv", 'n')
                        | ("mv", 'v')
                        | ("ln", 's')
                        | ("ln", 'f')
                        | ("ln", 'i')
                        | ("ln", 'n')
                        | ("ln", 'v')
                        | ("ln", 'r')
                        | ("ln", 'L')
                        | ("ln", 'P')
                        | ("ln", 'T')
                        | ("install", 'd')
                        | ("install", 'v')
                        | ("install", 'p')
                );
                if !allowed {
                    return None;
                }
            }
            continue;
        }
        paths.push(argument.clone());
    }
    Some(paths)
}

/// A project file operation is free only when every path it names resolves
/// inside the root; anything conservative asks.
fn project_file_operation_is_free(
    program: &str,
    arguments: &[String],
    root: &std::path::Path,
    working_directory: &std::path::Path,
) -> bool {
    let Some(paths) = file_operation_paths(program, arguments) else {
        return false;
    };
    if paths.is_empty() {
        return false;
    }
    paths
        .iter()
        .all(|path| path_inside_root(path, root, working_directory))
}

fn shell_requires_approval(args: &Value) -> bool {
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return true;
    };
    // A credential path is refused outright rather than confirmed (S2).
    if command_secret_rule(command).is_some() {
        return true;
    }
    // Parse a single simple command. Quotes are allowed for ordinary search
    // terms and paths; shell expansion, escapes, chaining, pipes and redirects
    // remain approval-gated. This avoids a prompt for common read-only commands
    // like `rg -n "text to find" src` without trying to interpret shell syntax.
    let Some(mut words) = simple_command_words(command) else {
        return true;
    };
    if words.is_empty() {
        return true;
    }
    let program = words.remove(0);
    // A program containing a path separator never inherits an allowlisted
    // decision: `/tmp/x/ls` is not `ls`, and `./ls` is whatever sits in the
    // project directory. Allowlisted names are resolved through `PATH`, so a
    // relative or absolute path to a trusted binary asks too (S3).
    if program.contains('/') {
        return true;
    }
    let rest = words;
    // Path checks are made against the project root and the working directory
    // once per decision; a root that cannot be canonicalised asks (S1, S2).
    let read_free = |arguments: &[String]| match project_and_working_directory() {
        Some((root, cwd)) => read_arguments_are_free(&program, arguments, &root, &cwd),
        None => false,
    };
    let safe = match program.as_str() {
        "pwd" | "whoami" | "date" | "uname" | "ps" | "which" | "type" => true,
        "rg" => {
            // `--pre` runs a command for every file, so it never runs free.
            !rest.iter().any(|arg| {
                matches!(arg.as_str(), "--pre" | "--pre-glob")
                    || arg.starts_with("--pre=")
                    || arg.starts_with("--pre-glob=")
            }) && read_free(&rest)
        }
        "sed" => !sed_requires_approval(&rest) && read_free(&rest),
        "find" => {
            !rest.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "-delete"
                        | "-exec"
                        | "-execdir"
                        | "-ok"
                        | "-okdir"
                        | "-fprint"
                        | "-fprint0"
                        | "-fprintf"
                        | "-fls"
                )
            }) && read_free(&rest)
        }
        "git" => {
            let read_only = rest
                .first()
                .is_some_and(|sub| matches!(sub.as_str(), "status" | "log" | "rev-parse" | "show"))
                || (rest.first().is_some_and(|sub| sub == "diff")
                    && !rest.iter().any(|arg| {
                        arg == "-o" || arg.starts_with("--output") || arg == "--ext-diff"
                    }))
                || (rest.first().is_some_and(|sub| sub == "branch")
                    && rest.iter().skip(1).all(|arg| {
                        matches!(
                            arg.as_str(),
                            "--list"
                                | "-l"
                                | "--all"
                                | "-a"
                                | "--remotes"
                                | "-r"
                                | "--show-current"
                        )
                    }));
            read_only && read_free(&rest)
        }
        "sort" => {
            !rest.iter().any(|arg| {
                long_option_abbreviation(arg, "output")
                    || long_option_abbreviation(arg, "compress-program")
                    || (arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains('o'))
            }) && read_free(&rest)
        }
        "cargo" => {
            rest.first().is_some_and(|sub| {
                matches!(sub.as_str(), "test" | "check" | "build" | "fmt" | "clippy")
            }) && read_free(&rest)
        }
        // Other read-only programs are free only while every path they name
        // stays inside the project root (S2).
        program if READ_PROGRAMS.contains(&program) => read_free(&rest),
        // File operations are free only when every path they name stays inside
        // the project root (S1).
        program if PROJECT_FILE_OPERATIONS.contains(&program) => {
            match (
                project_root(std::path::Path::new(".")),
                std::env::current_dir()
                    .ok()
                    .and_then(|cwd| cwd.canonicalize().ok()),
            ) {
                (Some(root), Some(cwd)) => {
                    project_file_operation_is_free(program, &rest, &root, &cwd)
                }
                _ => false,
            }
        }
        // Destructive and ownership-changing programs always ask.
        program if DESTRUCTIVE_PROGRAMS.contains(&program) => false,
        // Permit ordinary bare GUI launches through shell, while arguments
        // and detached modes remain subject to the conservative checks below.
        "firefox" | "chromium" | "chromium-browser" | "google-chrome" | "spotify" | "nautilus"
        | "gedit" | "code" => rest.is_empty(),
        _ => false,
    };
    if !safe {
        return true;
    }
    let mode = args
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("foreground");
    mode == "detached"
        && !matches!(
            program.as_str(),
            "firefox"
                | "chromium"
                | "chromium-browser"
                | "google-chrome"
                | "spotify"
                | "nautilus"
                | "gedit"
                | "code"
        )
}

fn simple_command_words(command: &str) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut quote = Quote::None;
    let mut token = String::new();
    let mut words = Vec::new();
    for character in command.trim().chars() {
        match quote {
            Quote::None => match character {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\n' | '\r' => return None,
                c if c.is_whitespace() => {
                    if !token.is_empty() {
                        words.push(std::mem::take(&mut token));
                    }
                }
                ';' | '|' | '&' | '>' | '<' | '$' | '`' | '\\' | '(' | ')' => return None,
                c => token.push(c),
            },
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::None
                } else {
                    token.push(character)
                }
            }
            Quote::Double => match character {
                '"' => quote = Quote::None,
                '$' | '`' | '\\' => return None,
                c => token.push(c),
            },
        }
    }
    if quote != Quote::None {
        return None;
    }
    if !token.is_empty() {
        words.push(token);
    }
    Some(words)
}

fn sed_requires_approval(args: &[String]) -> bool {
    let mut scripts = Vec::new();
    let mut explicit_expression = false;
    let mut index = 0;
    let mut options_terminated = false;

    while index < args.len() {
        let arg = &args[index];
        if arg == "-i"
            || arg.starts_with("-i")
            || arg == "--in-place"
            || arg.starts_with("--in-place=")
        {
            return true;
        }
        if arg == "-f"
            || arg == "--file"
            || arg.starts_with("--file=")
            || (arg.starts_with("-f") && arg.len() > 2)
        {
            // The script file's contents are not available to this policy.
            return true;
        }
        if arg == "-e" || arg == "--expression" {
            let Some(script) = args.get(index + 1) else {
                return true;
            };
            scripts.push(script.as_str());
            explicit_expression = true;
            index += 2;
            continue;
        }
        if let Some(script) = arg.strip_prefix("--expression=") {
            scripts.push(script);
            explicit_expression = true;
            index += 1;
            continue;
        }
        if let Some(script) = arg.strip_prefix("-e").filter(|script| !script.is_empty()) {
            scripts.push(script);
            explicit_expression = true;
            index += 1;
            continue;
        }
        if matches!(
            arg.as_str(),
            "-n" | "--quiet"
                | "--silent"
                | "-E"
                | "-r"
                | "--regexp-extended"
                | "-s"
                | "--separate"
                | "-u"
                | "--unbuffered"
                | "-z"
                | "--null-data"
        ) {
            index += 1;
            continue;
        }
        if arg == "--" {
            index += 1;
            options_terminated = true;
            break;
        }
        if arg.starts_with('-') {
            return true;
        }
        break;
    }

    // Without -e, the first positional argument after options is the script.
    // With -e, all positional arguments are input paths, so they aren't parsed
    // as script text and filenames containing `w` or `W` remain prompt-free.
    if index < args.len() && !explicit_expression {
        scripts.push(args[index].as_str());
    }
    if !options_terminated {
        // GNU sed accepts options after the positional script. Require `--`
        // before filenames beginning with `-`, and catch write/external-script
        // switches even when they follow an ordinary input path.
        let first_input = index + usize::from(!explicit_expression && index < args.len());
        let suffix = &args[first_input..];
        let option_end = suffix
            .iter()
            .position(|arg| arg == "--")
            .unwrap_or(suffix.len());
        if suffix[..option_end].iter().any(|arg| arg.starts_with('-')) {
            return true;
        }
    }
    !scripts
        .iter()
        .all(|script| read_only_sed_print_script(script))
}

fn long_option_abbreviation(argument: &str, option: &str) -> bool {
    let Some(name) = argument.strip_prefix("--") else {
        return false;
    };
    let name = name.split('=').next().unwrap_or(name);
    !name.is_empty() && option.starts_with(name)
}

fn read_only_sed_print_script(script: &str) -> bool {
    if script == "p" || script == "=" {
        return true;
    }
    let Some(addresses) = script.strip_suffix('p') else {
        return false;
    };
    let is_address = |address: &str| {
        address == "$"
            || (!address.is_empty() && address.chars().all(|character| character.is_ascii_digit()))
            || is_simple_sed_regex_address(address)
    };
    if is_address(addresses) {
        return true;
    }
    addresses
        .split_once(',')
        .is_some_and(|(start, end)| is_address(start) && is_address(end))
}

fn is_simple_sed_regex_address(address: &str) -> bool {
    let Some(body) = address
        .strip_prefix('/')
        .and_then(|value| value.strip_suffix('/'))
    else {
        return false;
    };
    let mut escaped = false;
    for character in body.chars() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '/' {
            // An unescaped delimiter here ends the address and could begin a
            // write or execute command before the final slash.
            return false;
        }
    }
    !escaped
}

fn process_requires_approval(args: &Value) -> bool {
    let Some(program) = args.get("program").and_then(Value::as_str) else {
        return true;
    };
    let arguments = match args.get("args") {
        None => Vec::new(),
        Some(value) => {
            let Some(arguments) = value.as_array() else {
                return true;
            };
            let Some(arguments) = arguments
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()
            else {
                return true;
            };
            arguments
        }
    };
    // The process tool can otherwise launch any executable, including a
    // shell or a file-changing utility. Workflow parsing may already have
    // expanded a friendly alias (e.g. brave -> flatpak run ...), so recognize
    // only an exact built-in expansion before rejecting unknown executables.
    let (alias, consumed) = if crate::apps::resolves(program) {
        (program, 0)
    } else if let Some((alias, consumed)) = crate::apps::resolved_alias(
        program,
        &arguments
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>(),
    ) {
        (alias, consumed)
    } else {
        return true;
    };
    let extra_arguments = &arguments[consumed..];
    let browser = matches!(
        alias.trim().to_ascii_lowercase().as_str(),
        "firefox"
            | "brave"
            | "brave browser"
            | "chrome"
            | "google chrome"
            | "google-chrome"
            | "google-chrome-stable"
            | "chromium"
            | "chromium-browser"
            | "youtube music"
            | "youtube-music"
    );
    !extra_arguments.is_empty()
        && !(browser
            && extra_arguments
                .iter()
                .all(|value| value.starts_with("https://") || value.starts_with("http://")))
}

fn computer_requires_approval_with_context(args: &Value, context: &FocusContext) -> bool {
    // Typing and key input reach whatever has focus. In a terminal the
    // keystrokes can run a shell command, and an unreadable focus could be a
    // terminal, so both ask (D1).
    if computer_call_uses_focus(args) && !focus_is_safe(context) {
        return true;
    }
    match action(args) {
        "batch" => args
            .get("steps")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .any(|step| computer_requires_approval_with_context(step, context))
            })
            .unwrap_or(true),
        // Setting clipboard text is a routine, reversible local operation.
        // Reading or clearing it can expose/remove unrelated user data.
        "clipboard_set" => false,
        "clipboard_clear" | "clipboard_get" => true,
        "click_text" => {
            is_consequential_text(args.get("text").and_then(Value::as_str).unwrap_or(""))
        }
        // Outside a terminal, single keys are core navigation and editing.
        // Labeled commits and consequential keyboard shortcuts still ask.
        "key_press" => false,
        "key_combo" => {
            args.get("keys")
                .and_then(Value::as_array)
                .map(|keys| {
                    let keys = keys
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_ascii_lowercase)
                        .collect::<Vec<_>>();
                    (keys.iter().any(|k| {
                        matches!(k.as_str(), "ctrl" | "control" | "alt" | "meta" | "super")
                    }) && keys.iter().any(|k| {
                        matches!(
                            k.as_str(),
                            "enter" | "return" | "delete" | "backspace" | "f4"
                        )
                    })) || (keys
                        .iter()
                        .any(|k| matches!(k.as_str(), "ctrl" | "control"))
                        && keys.iter().any(|k| matches!(k.as_str(), "q" | "w")))
                })
                .unwrap_or(true)
        }
        // Raw coordinates and text entry cannot be classified without screen
        // context. Keep ordinary navigation usable; blind raw clicks remain a risk.
        "mouse_click" | "type_text" | "mouse_drag" | "scroll" | "mouse_move" | "focus_window"
        | "open_url" | "wait" | "screenshot" | "ocr" | "find_text" | "cursor" | "screens"
        | "status" => false,
        "app_state" => false,
        _ => true,
    }
}

/// True when focus was read and the focused application is not a terminal.
fn focus_is_safe(context: &FocusContext) -> bool {
    context
        .app
        .as_deref()
        .map(str::trim)
        .is_some_and(|app| !app.is_empty() && !is_terminal_app(app))
}

/// Terminal application names, lower-case app ids and window application names
/// as GNOME and niri report them. Extend with `HYUSK_TERMINAL_APPS` rather than
/// a code change; new terminals appear regularly (D1).
const TERMINAL_APPS: &[&str] = &[
    "alacritty",
    "com.mitchellh.ghostty",
    "console",
    "contour",
    "cool-retro-term",
    "eterm",
    "foot",
    "ghostty",
    "gnome-terminal",
    "guake",
    "io.elementary.terminal",
    "kgx",
    "kitty",
    "konsole",
    "lxterminal",
    "org.gnome.console",
    "org.gnome.terminal",
    "org.kde.konsole",
    "org.wezfurlong.wezterm",
    "qterminal",
    "rxvt",
    "sakura",
    "st",
    "tabby",
    "terminator",
    "terminology",
    "termite",
    "tilix",
    "urxvt",
    "wezterm",
    "xfce4-terminal",
    "xterm",
    "yakuake",
];

/// The built-in terminals plus `HYUSK_TERMINAL_APPS`, a comma-separated list of
/// additional application names.
fn configured_terminals() -> Vec<String> {
    let mut terminals: Vec<String> = TERMINAL_APPS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    if let Ok(value) = std::env::var("HYUSK_TERMINAL_APPS") {
        terminals.extend(parse_terminal_extension(&value));
    }
    terminals
}

fn parse_terminal_extension(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
}

/// True when the focused application is a terminal, so typing into it can run a
/// shell command.
pub fn is_terminal_app(app: &str) -> bool {
    let app = app.trim().to_ascii_lowercase();
    !app.is_empty() && configured_terminals().contains(&app)
}

fn accessibility_requires_approval(args: &Value) -> bool {
    match action(args) {
        "click" => {
            let target = format!(
                "{} {}",
                args.get("name").and_then(Value::as_str).unwrap_or(""),
                args.get("text").and_then(Value::as_str).unwrap_or("")
            );
            let action_name = args
                .get("action_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            is_consequential_text(&target) || is_consequential_text(action_name)
        }
        "set_text" => false,
        "active" | "apps" | "windows" | "tree" | "find" | "read" | "focus" | "get_text" => false,
        _ => true,
    }
}

fn mobile_requires_approval(args: &Value) -> bool {
    if action(args) != "invoke" {
        return false;
    }
    let Some(phone_action) = args.get("phone_action").and_then(Value::as_str) else {
        return true;
    };
    if phone_action_requires_confirmation(phone_action) {
        return true;
    }
    !matches!(
        phone_action.to_ascii_lowercase().as_str(),
        "apps.list"
            | "apps.launch"
            | "url.open"
            | "web.search"
            | "device.status"
            | "timer.create"
            | "timer.cancel"
            | "reminder.create"
            | "ui.read"
            | "ui.screenshot"
            | "ui.find"
            | "ui.snapshot"
            | "ui.click"
            | "ui.long_click"
            | "ui.set_text"
            | "ui.scroll"
            | "ui.tap"
            | "ui.swipe"
            | "ui.press_enter"
            | "media.play"
            | "media.pause"
            | "media.play_pause"
            | "media.next"
            | "media.previous"
            | "media.status"
            | "volume.up"
            | "volume.down"
            | "system.back"
            | "system.home"
            | "system.recents"
            | "system.notifications"
            | "system.quick_settings"
            | "system.settings"
            | "camera.open"
            | "phone.dial"
            | "sms.compose"
            | "share.text"
            | "clipboard.write"
            | "memory.remember"
    )
}

pub(crate) fn phone_action_requires_confirmation(action: &str) -> bool {
    let action = action.to_ascii_lowercase();
    let words = action
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    [
        "delete",
        "remove",
        "uninstall",
        "install",
        "purchase",
        "payment",
        "pay",
        "buy",
        "send",
        "transfer",
        "publish",
        "post",
        "upload",
        "reset",
        "reboot",
        "shutdown",
    ]
    .iter()
    .any(|term| words.iter().any(|word| word == term))
        || matches!(
            action.as_str(),
            "call"
                | "call.place"
                | "phone.call"
                | "clipboard.read"
                | "clipboard.clear"
                | "memory.forget"
                | "shell.execute"
        )
        || action.starts_with("call.")
}

fn is_consequential_text(text: &str) -> bool {
    let words = text
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    [
        "send",
        "delete",
        "remove",
        "pay",
        "purchase",
        "buy",
        "submit",
        "confirm",
        "transfer",
        "publish",
        "post",
        "share",
        "install",
        "uninstall",
        "reset",
    ]
    .iter()
    .any(|term| words.iter().any(|word| word == term))
        || words.windows(2).any(|pair| pair == ["place", "order"])
        || matches!(words.as_slice(), [call] if call == "call")
        || matches!(words.as_slice(), [call, now] if call == "call" && now == "now")
        || matches!(words.as_slice(), [place, call] if place == "place" && call == "call")
        || (words.len() == 2
            && words[0] == "call"
            && !matches!(
                words[1].as_str(),
                "history" | "logs" | "settings" | "contacts"
            ))
}

#[cfg(test)]
mod tests {
    use super::{
        approval_arguments, grant_key, is_terminal_app, pending_text, policy_for, preview,
        preview_with_context, requires_approval as check,
        requires_approval_with_context as check_with, requires_focus_context, secret_refusal,
        Decision, FocusContext, ToolName, PREVIEW_LIMIT,
    };
    use serde_json::Value;

    /// A temp project root with a file and a directory inside it.
    fn project_fixture(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "hyusk-s1-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("sub")).expect("fixture directory");
        std::fs::write(root.join("a.txt"), "hello").expect("fixture file");
        root.canonicalize().expect("canonical fixture root")
    }

    fn free(program: &str, arguments: &[&str], root: &std::path::Path) -> bool {
        let arguments = arguments
            .iter()
            .map(|argument| (*argument).to_string())
            .collect::<Vec<_>>();
        super::project_file_operation_is_free(program, &arguments, root, root)
    }

    /// A read command's decision against an explicit root, so no test mutates
    /// the environment another test thread shares.
    fn read_is_free(program: &str, arguments: &[&str], root: &std::path::Path) -> bool {
        let arguments = arguments
            .iter()
            .map(|argument| (*argument).to_string())
            .collect::<Vec<_>>();
        super::read_arguments_are_free(program, &arguments, root, root)
    }

    /// The command an approval check would see for one shell call.
    fn shell_call(command: &str) -> String {
        serde_json::json!({ "command": command }).to_string()
    }

    #[test]
    fn project_file_operations_are_free_only_inside_the_root() {
        let root = project_fixture("inside");
        for (program, arguments) in [
            ("mkdir", vec!["-p", "build", "build/sub"]),
            ("mkdir", vec!["build"]),
            ("touch", vec!["x.txt"]),
            ("cp", vec!["a.txt", "b.txt"]),
            ("mv", vec!["a.txt", "sub/moved.txt"]),
            ("ln", vec!["-s", "a.txt", "link.txt"]),
            ("cp", vec!["-r", "sub", "sub-copy"]),
            // Option handling: `--`, `-t`, and inline long options.
            ("mkdir", vec!["--", "build"]),
            ("cp", vec!["-t", "sub", "a.txt"]),
            ("cp", vec!["--target-directory=sub", "a.txt"]),
            ("mv", vec!["-t", "sub", "a.txt"]),
            // `.` and `..` inside the root are fine once resolved.
            ("cp", vec!["a.txt", "sub/../b.txt"]),
            ("cp", vec![".", "sub/."]),
        ] {
            assert!(
                free(program, &arguments, &root),
                "{program} {arguments:?} should run free inside the root"
            );
        }

        let outside = std::env::temp_dir().join("hyusk-s1-outside-target.txt");
        for (program, arguments) in [
            ("cp", vec!["a.txt", "../escaped.txt"]),
            ("mkdir", vec!["-p", "../escaped"]),
            ("mv", vec!["a.txt", outside.to_str().unwrap()]),
            ("cp", vec!["-t", "/outside", "a.txt"]),
            ("cp", vec!["--target-directory=/outside", "a.txt"]),
            ("touch", vec!["~/notes.txt"]),
            ("cp", vec!["~/notes.txt", "."]),
            ("ln", vec!["-s", "/etc/passwd", "link"]),
            // Unknown options are not guessed at.
            ("cp", vec!["-z", "a.txt", "b.txt"]),
            ("cp", vec!["--reflink=auto", "a.txt", "b.txt"]),
            // No path at all is not a project write.
            ("mkdir", vec![]),
        ] {
            assert!(
                !free(program, &arguments, &root),
                "{program} {arguments:?} should ask"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_are_free_only_inside_the_root() {
        let root = project_fixture("read-root");
        for (program, arguments) in [
            ("cat", vec!["a.txt"]),
            ("cat", vec!["sub"]),
            ("head", vec!["-n", "5", "a.txt"]),
            ("tail", vec!["a.txt"]),
            ("wc", vec!["-l", "a.txt"]),
            ("file", vec!["a.txt"]),
            ("stat", vec!["a.txt"]),
            ("ls", vec!["-la"]),
            ("du", vec!["-sh", "."]),
            ("grep", vec!["-rn", "hello", "sub"]),
            ("rg", vec!["-n", "tool", "sub"]),
            ("find", vec!["sub", "-name", "*.rs"]),
            ("sed", vec!["-n", "1,20p", "a.txt"]),
            ("sort", vec!["a.txt"]),
            ("cut", vec!["-d:", "-f1", "a.txt"]),
            ("git", vec!["show", "HEAD:a.txt"]),
            ("git", vec!["log", "-p", "--", "a.txt"]),
            ("cargo", vec!["test", "--lib"]),
            // A relative path that leaves and returns stays inside the root.
            ("cat", vec!["sub/../a.txt"]),
            ("cat", vec!["./sub/./a.txt"]),
            // No path operand reads stdin or the working directory.
            ("cat", vec![]),
            ("ls", vec![]),
        ] {
            assert!(
                read_is_free(program, &arguments, &root),
                "{program} {arguments:?} should read freely inside the root"
            );
        }

        for (program, arguments) in [
            ("cat", vec!["../escaped.txt"]),
            ("cat", vec!["/etc/passwd"]),
            ("head", vec!["-n", "5", "../escaped.txt"]),
            ("tail", vec!["/var/log/syslog"]),
            ("rg", vec!["token", ".."]),
            ("grep", vec!["-rn", "token", "/etc"]),
            ("find", vec!["/", "-name", "*.pem"]),
            ("ls", vec!["/"]),
            ("sed", vec!["-n", "1p", "../escaped.txt"]),
            ("sort", vec!["../escaped.txt"]),
            ("git", vec!["log", "-p", "--", "../escaped.txt"]),
            // `..` is resolved before the check, including after a real
            // directory inside the root.
            ("cat", vec!["sub/../../escaped.txt"]),
        ] {
            assert!(
                !read_is_free(program, &arguments, &root),
                "{program} {arguments:?} should ask outside the root"
            );
        }

        // A symlink inside the root that points outside it is resolved first.
        let outside = root.parent().unwrap().join(format!(
            "hyusk-s2-outside-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&outside, "secret").expect("outside file");
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");
        assert!(
            !read_is_free("cat", &["link.txt"], &root),
            "a symlink out of the root must ask"
        );
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The focused window a decision is made against.
    fn focus(app: &str) -> FocusContext {
        FocusContext::new(app, "a window title")
    }

    /// True when the gate asks for a `computer` call in a focused app.
    fn computer_check(app: &str, input: &str) -> bool {
        check_with("computer", input, &focus(app))
    }

    #[test]
    fn terminal_applications_are_recognised_by_name() {
        for app in [
            "kitty",
            "Kitty",
            "Alacritty",
            "org.gnome.Terminal",
            "gnome-terminal",
            "org.gnome.Console",
            "Console",
            "foot",
            "org.wezfurlong.wezterm",
            "wezterm",
            "com.mitchellh.ghostty",
            "xterm",
            "konsole",
            "tilix",
            "xfce4-terminal",
        ] {
            assert!(is_terminal_app(app), "{app} is a terminal");
        }
        for app in [
            "code",
            "org.gnome.gedit",
            "firefox",
            "brave-browser",
            "org.gnome.Nautilus",
            "slack",
            "spotify",
            "",
            "   ",
            "terminal notes", // a title, not an application
        ] {
            assert!(!is_terminal_app(app), "{app:?} is not a terminal");
        }
    }

    #[test]
    fn terminal_list_can_be_extended_without_a_code_change() {
        // The extension list is a pure parse so a test never mutates the
        // environment another thread reads.
        assert_eq!(
            super::parse_terminal_extension("MyTerm, ghostty ,, "),
            vec!["myterm".to_string(), "ghostty".to_string()]
        );
        assert!(super::parse_terminal_extension("").is_empty());
        // An unset or empty extension keeps the built-in list.
        assert!(super::configured_terminals()
            .iter()
            .any(|name| name == "kitty"));
    }

    #[test]
    fn typing_and_key_input_ask_in_a_terminal_and_fail_closed() {
        for input in [
            r#"{"action":"type_text","text":"rm -rf build"}"#,
            r#"{"action":"type_text","text":"ls"}"#,
            r#"{"action":"key_press","key":"enter"}"#,
            r#"{"action":"key_combo","keys":["ctrl","c"]}"#,
            r#"{"action":"batch","steps":[{"action":"type_text","text":"rm -rf /"},{"action":"key_press","key":"enter"}]}"#,
        ] {
            assert!(
                computer_check("kitty", input),
                "typing in a terminal must ask: {input}"
            );
            assert!(
                computer_check("org.gnome.Terminal", input),
                "the terminal check covers gnome-terminal too: {input}"
            );
            assert!(
                check_with("computer", input, &FocusContext::unknown()),
                "an unreadable focus must ask: {input}"
            );
            assert!(
                check("computer", input),
                "the context-free wrapper must fail closed: {input}"
            );
        }

        // Outside a terminal, typing and navigation keys stay free.
        for input in [
            r#"{"action":"type_text","text":"hello"}"#,
            r#"{"action":"key_press","key":"enter"}"#,
            r#"{"action":"key_press","key":"escape"}"#,
            r#"{"action":"key_combo","keys":["shift","tab"]}"#,
            r#"{"action":"batch","steps":[{"action":"type_text","text":"thanks"},{"action":"key_press","key":"enter"}]}"#,
            r#"{"action":"screenshot"}"#,
            r#"{"action":"click_text","text":"Open settings"}"#,
        ] {
            assert!(
                !computer_check("code", input),
                "typing outside a terminal stays free: {input}"
            );
            assert!(
                !computer_check("firefox", input),
                "a browser field stays free: {input}"
            );
        }

        // A consequential shortcut still asks outside a terminal.
        assert!(computer_check(
            "code",
            r#"{"action":"key_combo","keys":["ctrl","enter"]}"#
        ));
        // An empty application name is no focus at all.
        assert!(check_with(
            "computer",
            r#"{"action":"type_text","text":"ls"}"#,
            &focus("")
        ));
        // Other tools do not depend on focus.
        assert!(!check_with(
            "window",
            r#"{"action":"activate","query":"kitty"}"#,
            &focus("kitty")
        ));
        assert!(check_with(
            "shell",
            r#"{"command":"rm -rf build"}"#,
            &focus("kitty")
        ));
    }

    #[test]
    fn only_input_calls_read_the_focused_window() {
        for input in [
            r#"{"action":"type_text","text":"ls"}"#,
            r#"{"action":"key_press","key":"enter"}"#,
            r#"{"action":"key_combo","keys":["ctrl","c"]}"#,
            r#"{"action":"batch","steps":[{"action":"screenshot"},{"action":"type_text","text":"ls"}]}"#,
        ] {
            assert!(requires_focus_context("computer", input), "{input}");
        }
        for input in [
            r#"{"action":"screenshot"}"#,
            r#"{"action":"click_text","text":"Send"}"#,
            r#"{"action":"open_url","url":"https://example.com"}"#,
            r#"{"action":"batch","steps":[{"action":"screenshot"}]}"#,
            "not json",
        ] {
            assert!(!requires_focus_context("computer", input), "{input}");
        }
        assert!(!requires_focus_context("shell", r#"{"command":"ls"}"#));
        assert!(!requires_focus_context("window", r#"{"action":"active"}"#));
    }

    #[test]
    fn an_approval_for_typing_names_the_focused_terminal() {
        assert_eq!(
            preview_with_context(
                "computer",
                r#"{"action":"type_text","text":"npm install"}"#,
                &focus("kitty")
            ),
            "computer action=type_text text=npm install (focused: kitty)"
        );
        // Outside a terminal the card has no focus suffix.
        assert_eq!(
            preview_with_context(
                "computer",
                r#"{"action":"type_text","text":"hello"}"#,
                &focus("code")
            ),
            "computer action=type_text text=hello"
        );
        // An unknown focus cannot name the terminal, so the card is honest.
        assert_eq!(
            preview_with_context(
                "computer",
                r#"{"action":"type_text","text":"hello"}"#,
                &FocusContext::unknown()
            ),
            "computer action=type_text text=hello"
        );
        // The suffix still respects the preview limit.
        let long = preview_with_context(
            "computer",
            &serde_json::json!({"action": "type_text", "text": "x".repeat(400)}).to_string(),
            &focus("kitty"),
        );
        assert_eq!(long.chars().count(), PREVIEW_LIMIT);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn secret_paths_are_refused_without_echoing_them() {
        for (command, private) in [
            ("cat .env", ".env"),
            ("cat .env.local", ".env.local"),
            ("cat ~/.ssh/id_ed25519", "id_ed25519"),
            ("ls ~/.ssh", ".ssh"),
            ("rg token ~/.config/gh", ".config/gh"),
            ("cat ~/.gnupg/pubring.kbx", ".gnupg"),
            ("cat ~/.aws/credentials", ".aws"),
            ("cat ~/.kube/config", ".kube"),
            ("cat ~/.netrc", ".netrc"),
            (
                "head ~/.local/share/keyrings/login.keyring",
                "login.keyring",
            ),
            (
                "cat ~/.local/share/hyusk/browser-control-profile/Cookies",
                "browser-control-profile",
            ),
            // A blob read from history names a credential just as plainly.
            ("git show HEAD:.env", ".env"),
        ] {
            let input = shell_call(command);
            let message = secret_refusal("shell", &input).expect(command);
            assert!(
                message.contains("secret denylist") && message.contains("rule:"),
                "the refusal must name the rule: {message}"
            );
            assert!(
                !message.contains(private),
                "the refusal must not echo {private}: {message}"
            );
            assert!(
                check("shell", &input),
                "a refused read still fails closed in the gate: {command}"
            );
        }

        // Ordinary reads, including a search for the text ".env", are not
        // refused: the pattern is not a path.
        for command in [
            "cat src/main.rs",
            r#"rg "tool" src"#,
            r#"rg -n '\.env' src"#,
            "git status --short",
            "git log -p",
            "find src -name '*.rs'",
        ] {
            assert!(
                secret_refusal("shell", &shell_call(command)).is_none(),
                "{command} should not be refused"
            );
        }
    }

    #[test]
    fn repository_reads_stay_free_and_escapes_ask() {
        // The acceptance cases, run against this checkout as the project root.
        for command in [
            "cat src/main.rs",
            r#"rg "tool" src"#,
            "head -n 5 Cargo.toml",
            "find src -name '*.rs'",
            "sed -n '1,20p' src/tools/safety.rs",
            "git status --short",
            "cargo test --lib",
        ] {
            assert!(
                !check("shell", &shell_call(command)),
                "unexpected approval for: {command}"
            );
        }
        assert!(
            check("shell", &shell_call("cat ../other-project/file")),
            "a read outside the project root must ask"
        );
        assert!(
            check("shell", &shell_call("find / -name '*.pem'")),
            "a whole-filesystem search must ask before any output"
        );
    }

    #[test]
    fn destructive_programs_always_ask() {
        for command in [
            "rm -rf build",
            "rmdir build",
            "shred a.txt",
            "truncate -s 0 a.txt",
            "chmod 600 a.txt",
            "chown root a.txt",
        ] {
            let input = serde_json::json!({ "command": command }).to_string();
            assert!(check("shell", &input), "{command} must always ask");
        }
    }

    #[test]
    fn the_project_root_is_hyusk_workspace_else_the_working_directory() {
        let root = project_fixture("root-selection");
        let configured = Some(root.as_os_str());
        assert_eq!(
            super::project_root_from(configured, &root),
            Some(root.clone())
        );
        assert_eq!(
            super::project_root_from(None, &root),
            Some(root.clone()),
            "an unset HYUSK_WORKSPACE falls back to the working directory"
        );
        assert_eq!(
            super::project_root_from(Some(std::ffi::OsStr::new("")), &root),
            Some(root.clone()),
            "an empty value is treated as unset"
        );
        assert_eq!(
            super::project_root_from(Some(std::ffi::OsStr::new("/nonexistent/hyusk")), &root),
            None,
            "a root that does not exist cannot be trusted"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn approval_preview_names_the_tool_and_its_key_arguments() {
        assert_eq!(
            preview("shell", r#"{"command":"rm -rf build"}"#),
            "shell command=rm -rf build"
        );
        // The policy row's key order wins over the JSON key order.
        assert_eq!(
            preview("shell", r#"{"mode":"detached","command":"firefox"}"#),
            "shell command=firefox mode=detached"
        );
        assert_eq!(
            preview(
                "process",
                r#"{"action":"launch","program":"code","args":["src/main.rs","Cargo.toml"]}"#
            ),
            "process program=code args=[src/main.rs, Cargo.toml] action=launch"
        );
        assert_eq!(
            preview(
                "task",
                r#"{"action":"delegate","profile":"workspace","task":"fix the build","workspace":"/home/user/project"}"#
            ),
            "task action=delegate profile=workspace task=fix the build workspace=/home/user/project"
        );
        assert_eq!(
            preview(
                "mobile",
                r#"{"action":"invoke","phone_action":"messages.send"}"#
            ),
            "mobile action=invoke phone_action=messages.send"
        );
        assert_eq!(
            preview("window", r#"{"action":"close"}"#),
            "window action=close"
        );
        // A browser mutation names the action, target, and selector or text.
        assert_eq!(
            preview(
                "browser",
                r##"{"action":"fill","target_id":"tab-1","selector":"#q","text":"hello"}"##
            ),
            "browser action=fill target_id=tab-1 selector=#q text=hello"
        );
        assert_eq!(
            preview(
                "browser",
                r#"{"action":"press_key","target_id":"tab-1","key":"Enter"}"#
            ),
            "browser action=press_key target_id=tab-1 key=Enter"
        );
    }

    #[test]
    fn approval_preview_is_one_line_and_bounded() {
        // Empty or non-object arguments name the tool alone.
        assert_eq!(preview("browser", "{}"), "browser");
        assert_eq!(preview("shell", ""), "shell");
        assert_eq!(preview("shell", "[1,2]"), "shell");
        // Whitespace, tabs, and newlines collapse onto one line.
        assert_eq!(
            preview(
                "memory",
                r#"{"action":"remember","text":"line one\n\tline   two"}"#
            ),
            "memory action=remember text=line one line two"
        );
        // A long command truncates to the limit with a visible ellipsis.
        let long = preview(
            "shell",
            &serde_json::json!({"command": format!("echo {}", "x".repeat(400))}).to_string(),
        );
        assert_eq!(long.chars().count(), PREVIEW_LIMIT);
        assert!(long.ends_with('…'), "{long}");
        assert!(long.starts_with("shell command=echo xxx"), "{long}");
        // A call that fits exactly is not truncated.
        let head = "shell command=";
        let exact_command = "y".repeat(PREVIEW_LIMIT - head.chars().count());
        let exact = preview(
            "shell",
            &serde_json::json!({"command": exact_command}).to_string(),
        );
        assert_eq!(exact.chars().count(), PREVIEW_LIMIT);
        assert!(!exact.ends_with('…'));
    }

    #[test]
    fn a_turn_grant_is_keyed_by_tool_and_shape() {
        let key = |name: &str, arguments: &str| grant_key(name, arguments);
        // The shell shape is the program plus its first subcommand.
        assert_eq!(
            key("shell", r#"{"command":"git push origin main"}"#),
            Some(("shell".into(), "git push".into()))
        );
        assert_eq!(
            key("shell", r#"{"command":"rm -rf build"}"#),
            Some(("shell".into(), "rm".into()))
        );
        // A path form is its own shape, so approving one never covers a bare
        // allowlisted name or another path (S3).
        assert_eq!(
            key("shell", r#"{"command":"/usr/bin/rm -rf other"}"#),
            Some(("shell".into(), "/usr/bin/rm".into()))
        );
        assert_eq!(
            key("shell", r#"{"command":"./ls -la"}"#),
            Some(("shell".into(), "./ls".into()))
        );
        assert_eq!(
            key("shell", r#"{"command":"cargo test --lib"}"#),
            Some(("shell".into(), "cargo test".into()))
        );
        // Other tools use their action; task uses its profile.
        assert_eq!(
            key("window", r#"{"action":"close"}"#),
            Some(("window".into(), "close".into()))
        );
        assert_eq!(
            key("task", r#"{"action":"delegate","profile":"workspace"}"#),
            Some(("task".into(), "workspace".into()))
        );
        // A browser grant is keyed by the mutation, so one approval covers the
        // rest of the turn's clicks but never a different mutation.
        assert_eq!(
            key(
                "browser",
                r##"{"action":"click","target_id":"tab-1","selector":"#go"}"##
            ),
            Some(("browser".into(), "click".into()))
        );
        assert_ne!(
            key("browser", r#"{"action":"click","target_id":"tab-1"}"#),
            key("browser", r#"{"action":"fill","target_id":"tab-1"}"#)
        );
        // A shape that cannot be determined is never granted.
        assert_eq!(key("shell", r#"{"command":"rm -rf x && echo done"}"#), None);
        assert_eq!(key("shell", "{}"), None);
        assert_eq!(key("shell", "not json"), None);
        assert_eq!(key("computer", r#"{"action":""}"#), None);
    }

    #[test]
    fn pending_text_is_the_one_sentence_both_surfaces_use() {
        assert_eq!(
            pending_text("shell command=rm -rf build"),
            "Pending confirmation for: shell command=rm -rf build"
        );
    }

    #[test]
    fn policy_table_names_every_tool_once_and_matches_its_decision() {
        let mut names = Vec::new();
        for tool in ToolName::ALL {
            let name = tool.name();
            assert!(!name.is_empty());
            assert_eq!(
                ToolName::from_name(name),
                Some(*tool),
                "{name} must resolve to its own variant"
            );
            assert!(!names.contains(&name), "duplicate policy name: {name}");
            names.push(name);

            let policy = tool.policy();
            assert!(
                !policy.guidance.trim().is_empty(),
                "missing guidance for {name}"
            );
            assert!(
                !policy.guidance.contains('\n'),
                "guidance for {name} must be one line"
            );
            // Guidance is generated into one prompt block, so no row may claim
            // a decision its classifier does not make.
            if matches!(policy.decision, Decision::Allow) {
                assert!(
                    !policy.asks_with_context(&Value::Null, &super::FocusContext::unknown()),
                    "{name} is Allow but asks"
                );
            }
        }
        assert_eq!(names.len(), ToolName::ALL.len());
    }

    #[test]
    fn generated_guidance_lists_every_policy_row_once() {
        let guidance = super::approval_guidance();
        let lines: Vec<&str> = guidance.lines().collect();
        assert_eq!(lines.len(), ToolName::ALL.len());
        for (line, tool) in lines.iter().zip(ToolName::ALL) {
            assert!(
                line.starts_with(&format!("- {}: ", tool.name())),
                "guidance line for {} is {line}",
                tool.name()
            );
            assert!(!line.trim_end().ends_with(':'), "empty guidance in {line}");
        }
        for tool in ToolName::ALL {
            assert_eq!(
                guidance.matches(&format!("- {}: ", tool.name())).count(),
                1,
                "{} must appear once in the guidance",
                tool.name()
            );
        }
    }

    #[test]
    fn unknown_tool_names_and_bad_arguments_fail_closed() {
        assert!(check("mystery_tool", "{}"));
        assert!(check("shell_typo", r#"{"command":"ls"}"#));
        assert!(check("Shell", r#"{"command":"ls"}"#));
        assert!(check("shell", "not json"));
        assert!(check("browser", "not json"));
        assert!(policy_for("mystery_tool").is_none());
        assert!(policy_for("shell").is_some());
        assert!(!check("browser", r#"{"action":"read","target_id":"1"}"#));
    }

    #[test]
    fn research_delegation_is_free_and_workspace_delegation_asks_once() {
        // Research is one read-only model call: no tools, no filesystem.
        assert!(!check(
            "task",
            r#"{"action":"delegate","task":"summarise","profile":"research"}"#
        ));
        // An absent profile is research, which is the tool's default.
        assert!(!check(
            "task",
            r#"{"action":"delegate","task":"summarise"}"#
        ));
        // Workspace delegation starts Codex against a project root.
        assert!(check(
            "task",
            r#"{"action":"delegate","task":"fix the build","profile":"workspace","workspace":"src"}"#
        ));
        // An unknown profile fails closed rather than being treated as research.
        assert!(check(
            "task",
            r#"{"action":"delegate","task":"fix","profile":"coding"}"#
        ));
        assert!(!check("task", r#"{"action":"list"}"#));
        assert!(!check("task", r#"{"action":"cancel","id":3}"#));
    }

    #[test]
    fn workspace_delegation_pins_the_canonical_root() {
        let stored = approval_arguments(
            "task",
            r#"{"action":"delegate","task":"fix","profile":"workspace","workspace":"src/."}"#,
        );
        let stored: Value = serde_json::from_str(&stored).unwrap();
        let canonical = std::fs::canonicalize("src").unwrap();
        assert_eq!(stored["workspace"], canonical.to_string_lossy().as_ref());
        // Research keeps its arguments: no root to pin.
        assert_eq!(
            approval_arguments(
                "task",
                r#"{"action":"delegate","task":"summarise","profile":"research"}"#
            ),
            r#"{"action":"delegate","task":"summarise","profile":"research"}"#
        );
        assert_eq!(
            approval_arguments("shell", r#"{"command":"ls"}"#),
            r#"{"command":"ls"}"#
        );
    }

    #[test]
    fn programs_named_by_a_path_always_ask() {
        // The classifier used to take the last path component, so any binary
        // planted at such a path inherited the allowlisted name's decision.
        // A path form asks now; the bare name is resolved through PATH (S3).
        for command in [
            "/tmp/x/ls",
            "./ls",
            "./ls -la",
            "../bin/wc",
            "/usr/bin/cat src/main.rs",
            "/usr/bin/rg tool src",
            "/bin/rm -rf build",
            "~/scripts/ls",
        ] {
            assert!(
                check("shell", &shell_call(command)),
                "{command} must ask: a path form is not the allowlisted name"
            );
        }
        for command in [
            "ls",
            "ls -la",
            "cat src/main.rs",
            "rg tool src",
            "git status",
        ] {
            assert!(
                !check("shell", &shell_call(command)),
                "{command} must stay free: the bare name resolves through PATH"
            );
        }
    }

    #[test]
    fn shell_defaults_closed_and_detects_command_injection() {
        assert!(check("shell", r#"{"command":"python -c print(1)"}"#));
        assert!(check("shell", r#"{"command":"ls; rm -rf /tmp/x"}"#));
        assert!(check("shell", r#"{"command":"cat $(touch /tmp/x)"}"#));
        assert!(check("shell", r#"{"command":"find . -delete"}"#));
        assert!(check("shell", r#"{"command":"rg --pre cat pattern"}"#));
        assert!(check("shell", r#"{"command":"git branch -D main"}"#));
        assert!(check(
            "shell",
            r#"{"command":"git diff --output=/tmp/changed"}"#
        ));
        assert!(!check("shell", r#"{"command":"ls -la"}"#));
        assert!(!check("shell", r#"{"command":"git status --short"}"#));
    }

    #[test]
    fn common_read_only_and_build_commands_do_not_prompt() {
        for command in [
            r#"rg -n "tool result" src"#,
            r#"sed -n '1,20p' src/tools/safety.rs"#,
            r#"sed -n '1,20p' workflow.rs"#,
            r#"sed -n '1,20p' -- -input-with-w.rs"#,
            r#"sed -n '/word/p' new.rs"#,
            r#"find . -name '*.rs'"#,
            "git diff --stat",
            "git show --stat",
            "sort -r input",
            "cargo test --lib",
            "cargo check --workspace",
        ] {
            let input = serde_json::json!({"command": command}).to_string();
            assert!(
                !check("shell", &input),
                "unexpected approval for: {command}"
            );
        }
    }

    #[test]
    fn read_command_allowlist_still_blocks_mutating_flags_and_chaining() {
        for command in [
            "find . -delete",
            "find . -exec rm {} ;",
            "sed -i s/a/b/ file",
            "sed -n '1,20w /tmp/out' file",
            "sed -n '/foo/w/bar/p' file",
            "sed 's/a/b/w /tmp/out' file",
            "sed -f /tmp/script.sed workflow.rs",
            "sed -n '1,20p' workflow.rs -i",
            "sed -n '1,20p' workflow.rs -f /tmp/write.sed",
            "rg --pre=cat pattern",
            "sort -o /tmp/out input",
            "sort -uo/tmp/out input",
            "sort --output=/tmp/out input",
            "sort --out=/tmp/out input",
            "sort --compress-program=sh input",
            "git branch -D main",
            "git diff -o /tmp/out",
            "git diff --ext-diff",
            "cargo test; rm -rf target",
            "rg \"$(touch /tmp/x)\" src",
        ] {
            let input = serde_json::json!({"command": command}).to_string();
            assert!(check("shell", &input), "missing approval for: {command}");
        }
    }

    #[test]
    fn recursive_computer_batch_cannot_hide_dangerous_action() {
        assert!(check(
            "computer",
            r#"{"action":"batch","steps":[{"action":"screenshot"},{"action":"batch","steps":[{"action":"click_text","text":"Send message"}]}]}"#
        ));
        assert!(check(
            "computer",
            r#"{"action":"click_text","text":"Send message"}"#
        ));
        assert!(!check(
            "computer",
            r#"{"action":"mouse_click","x":10,"y":20}"#
        ));
        // Typing is free only when the focused window is known and is not a
        // terminal; the context-free wrapper asks (D1).
        assert!(!computer_check(
            "code",
            r#"{"action":"type_text","text":"hello"}"#
        ));
        assert!(!computer_check(
            "code",
            r#"{"action":"key_press","key":"enter"}"#
        ));
        assert!(check(
            "computer",
            r#"{"action":"type_text","text":"hello"}"#
        ));
        assert!(check("computer", r#"{"action":"key_press","key":"enter"}"#));
        assert!(!check("computer", r#"{"action":"screenshot"}"#));
        assert!(!check("computer", r#"{"action":"app_state"}"#));
        assert!(!check(
            "computer",
            r#"{"action":"click_text","text":"Open settings"}"#
        ));
    }

    #[test]
    fn browser_navigation_stays_free_and_mutations_ask() {
        for input in [
            r#"{"action":"status"}"#,
            r#"{"action":"start"}"#,
            r#"{"action":"new_tab"}"#,
            r#"{"action":"tabs"}"#,
            r#"{"action":"read","target_id":"tab-1"}"#,
            r#"{"action":"navigate","target_id":"tab-1","url":"https://example.com"}"#,
        ] {
            assert!(
                !check("browser", input),
                "a read or navigation action must stay free: {input}"
            );
        }
        for input in [
            r##"{"action":"click","target_id":"tab-1","selector":"#go"}"##,
            r##"{"action":"fill","target_id":"tab-1","selector":"#q","text":"hello"}"##,
            r#"{"action":"press_key","target_id":"tab-1","key":"Enter"}"#,
            // An action the table does not know fails closed.
            r#"{"action":"download","target_id":"tab-1"}"#,
            r#"{"action":"click"}"#,
            "{}",
            "not json",
        ] {
            assert!(
                check("browser", input),
                "a browser mutation must ask: {input}"
            );
        }
    }

    #[test]
    fn accessible_and_mobile_actions_are_classified() {
        assert!(!check(
            "accessibility",
            r#"{"action":"click","path":[1,2]}"#
        ));
        assert!(check(
            "accessibility",
            r#"{"action":"click","name":"Delete account"}"#
        ));
        assert!(!check("accessibility", r#"{"action":"read"}"#));
        assert!(!check(
            "accessibility",
            r#"{"action":"set_text","text":"hello"}"#
        ));
        assert!(!check(
            "accessibility",
            r#"{"action":"click","name":"Open callback settings"}"#
        ));
        assert!(!check(
            "accessibility",
            r#"{"action":"click","name":"Call history"}"#
        ));
        assert!(check(
            "accessibility",
            r#"{"action":"click","name":"Call Goutam"}"#
        ));
        assert!(check(
            "mobile",
            r#"{"action":"invoke","phone_action":"messages.send"}"#
        ));
        assert!(!check(
            "mobile",
            r#"{"action":"invoke","phone_action":"url.open"}"#
        ));
        for phone_action in [
            "system.back",
            "ui.click",
            "media.play",
            "media.next",
            "sms.compose",
            "clipboard.write",
        ] {
            let input =
                serde_json::json!({"action":"invoke", "phone_action":phone_action}).to_string();
            assert!(
                !check("mobile", &input),
                "unexpected approval for {phone_action}"
            );
        }
        assert!(check(
            "mobile",
            r#"{"action":"invoke","phone_action":"unsupported.future_action"}"#
        ));
        assert!(check("window", r#"{"action":"close"}"#));
        assert!(check("memory", r#"{"action":"forget"}"#));
        assert!(check("computer", r#"{"action":"clipboard_get"}"#));
        assert!(check("computer", r#"{"action":"clipboard_clear"}"#));
        assert!(!check(
            "computer",
            r#"{"action":"clipboard_set","text":"hello"}"#
        ));
        assert!(check("computer", r#"{"action":"clipboard_clear"}"#));
        assert!(!check(
            "process",
            r#"{"action":"launch","program":"firefox"}"#
        ));
        for program in ["google-chrome", "google-chrome-stable", "chromium-browser"] {
            assert!(!check(
                "process",
                &serde_json::json!({"action":"launch", "program":program, "args":["https://example.com"]}).to_string()
            ));
            assert!(check(
                "process",
                &serde_json::json!({"action":"launch", "program":program, "args":["--no-sandbox"]})
                    .to_string()
            ));
        }
        assert!(check(
            "process",
            r#"{"action":"launch","program":"python3"}"#
        ));
        assert!(check(
            "process",
            r#"{"action":"launch","program":"/usr/bin/mv","args":["a","b"]}"#
        ));
    }
}
