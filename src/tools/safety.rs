//! Central approval policy for tool calls that can change user data or commit
//! consequential actions. This runs before tool dispatch; unknown shapes fail
//! closed.

use serde_json::Value;

/// Return true when this tool call should be held for user approval.
pub fn requires_approval(name: &str, arguments: &str) -> bool {
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return true;
    };
    match name {
        "shell" => shell_requires_approval(&args),
        "process" => process_requires_approval(&args),
        "computer" => computer_requires_approval(&args),
        "accessibility" => accessibility_requires_approval(&args),
        "window" => args.get("action").and_then(Value::as_str) == Some("close"),
        "mobile" => mobile_requires_approval(&args),
        "memory" => args.get("action").and_then(Value::as_str) == Some("forget"),
        // For any other tool, preserve the existing tool-specific policy.
        _ => false,
    }
}

fn action(args: &Value) -> &str {
    args.get("action").and_then(Value::as_str).unwrap_or("")
}

fn shell_requires_approval(args: &Value) -> bool {
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return true;
    };
    let command = command.trim();
    // Avoid shell grammar entirely in the no-approval path. Quoting, pipes,
    // substitutions, redirection, chaining, and escapes all require approval.
    if command.is_empty()
        || command.chars().any(|c| {
            matches!(
                c,
                ';' | '|' | '&' | '>' | '<' | '$' | '`' | '\\' | '\n' | '\r' | '\'' | '"'
            )
        })
    {
        return true;
    }
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return true;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let rest: Vec<&str> = words.collect();
    let safe = match program {
        "pwd" | "whoami" | "date" | "uname" | "ls" | "cat" | "head" | "tail" | "wc" | "file"
        | "stat" | "ps" | "df" | "du" | "grep" | "which" | "type" => true,
        "git" => rest
            .first()
            .is_some_and(|sub| matches!(*sub, "status" | "log" | "rev-parse")),
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
            program,
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

fn process_requires_approval(args: &Value) -> bool {
    let Some(program) = args.get("program").and_then(Value::as_str) else {
        return true;
    };
    // The process tool can otherwise launch any executable, including a
    // shell or a file-changing utility. Known app aliases are the ordinary
    // no-approval path; an unknown executable needs a user decision.
    if !crate::apps::resolves(program) {
        return true;
    }
    let Some(arguments) = args.get("args").and_then(Value::as_array) else {
        return false;
    };
    let browser = matches!(
        program.trim().to_ascii_lowercase().as_str(),
        "firefox" | "brave" | "brave browser" | "chrome" | "google chrome" | "chromium"
    );
    !arguments.is_empty()
        && !(browser
            && arguments.iter().all(|arg| {
                arg.as_str().is_some_and(|value| {
                    value.starts_with("https://") || value.starts_with("http://")
                })
            }))
}

fn computer_requires_approval(args: &Value) -> bool {
    match action(args) {
        "batch" => args
            .get("steps")
            .and_then(Value::as_array)
            .map(|steps| steps.iter().any(computer_requires_approval))
            .unwrap_or(true),
        "clipboard_set" | "clipboard_clear" | "clipboard_get" => true,
        "click_text" => {
            is_consequential_text(args.get("text").and_then(Value::as_str).unwrap_or(""))
        }
        // Single keys are core navigation and editing. Labeled commits and
        // consequential keyboard shortcuts are handled separately.
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
    let lowered = phone_action.to_ascii_lowercase();
    if [
        "delete",
        "remove",
        "uninstall",
        "install",
        "purchase",
        "payment",
        "pay",
        "send",
        "message",
        "share",
        "upload",
        "call",
        "sms",
        "clipboard",
        "reset",
        "shell",
    ]
    .iter()
    .any(|term| lowered.contains(term))
    {
        return true;
    }
    !matches!(
        lowered.as_str(),
        "apps.launch"
            | "url.open"
            | "device.status"
            | "timer.create"
            | "timer.cancel"
            | "ui.read"
            | "ui.screenshot"
            | "ui.find"
    )
}

fn is_consequential_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
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
        "call",
        "place order",
    ]
    .iter()
    .any(|term| text.contains(term))
}

#[cfg(test)]
mod tests {
    use super::requires_approval as check;

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
        assert!(!check(
            "computer",
            r#"{"action":"type_text","text":"hello"}"#
        ));
        assert!(!check(
            "computer",
            r#"{"action":"key_press","key":"enter"}"#
        ));
        assert!(!check("computer", r#"{"action":"screenshot"}"#));
        assert!(!check("computer", r#"{"action":"app_state"}"#));
        assert!(!check(
            "computer",
            r#"{"action":"click_text","text":"Open settings"}"#
        ));
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
        assert!(check(
            "mobile",
            r#"{"action":"invoke","phone_action":"messages.send"}"#
        ));
        assert!(!check(
            "mobile",
            r#"{"action":"invoke","phone_action":"url.open"}"#
        ));
        assert!(check("window", r#"{"action":"close"}"#));
        assert!(check("memory", r#"{"action":"forget"}"#));
        assert!(check("computer", r#"{"action":"clipboard_get"}"#));
        assert!(check("computer", r#"{"action":"clipboard_clear"}"#));
        assert!(!check(
            "process",
            r#"{"action":"launch","program":"firefox"}"#
        ));
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
