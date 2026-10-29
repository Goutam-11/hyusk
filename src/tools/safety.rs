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
        "task" => task_requires_approval(&args),
        // For any other tool, preserve the existing tool-specific policy.
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
    if args.get("action").and_then(Value::as_str) != Some("delegate") {
        return arguments.to_string();
    }
    let Some(workspace) = args.get("workspace").and_then(Value::as_str) else {
        return arguments.to_string();
    };
    let Ok(canonical) = std::fs::canonicalize(workspace) else {
        return arguments.to_string();
    };
    args["workspace"] = Value::String(canonical.to_string_lossy().into_owned());
    serde_json::to_string(&args).unwrap_or_else(|_| arguments.to_string())
}

fn task_requires_approval(args: &Value) -> bool {
    // Starting a worker is the documented safety boundary for both profiles.
    // Research is read-only but still runs a second model; workspace can edit
    // any project root the model names, so it must never start unconfirmed.
    action(args) == "delegate"
}

fn action(args: &Value) -> &str {
    args.get("action").and_then(Value::as_str).unwrap_or("")
}

fn shell_requires_approval(args: &Value) -> bool {
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return true;
    };
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
    let program = program.rsplit('/').next().unwrap_or(&program);
    let rest = words;
    let safe = match program {
        "pwd" | "whoami" | "date" | "uname" | "ls" | "cat" | "head" | "tail" | "wc" | "file"
        | "stat" | "ps" | "df" | "du" | "grep" | "which" | "type" | "uniq" | "cut" | "tr" => true,
        "rg" => !rest.iter().any(|arg| {
            matches!(arg.as_str(), "--pre" | "--pre-glob")
                || arg.starts_with("--pre=")
                || arg.starts_with("--pre-glob=")
        }),
        "sed" => !sed_requires_approval(&rest),
        "find" => !rest.iter().any(|arg| {
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
        }),
        "git" => {
            rest.first()
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
                    }))
        }
        "sort" => !rest.iter().any(|arg| {
            long_option_abbreviation(arg, "output")
                || long_option_abbreviation(arg, "compress-program")
                || (arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains('o'))
        }),
        "cargo" => rest.first().is_some_and(|sub| {
            matches!(sub.as_str(), "test" | "check" | "build" | "fmt" | "clippy")
        }),
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

fn computer_requires_approval(args: &Value) -> bool {
    match action(args) {
        "batch" => args
            .get("steps")
            .and_then(Value::as_array)
            .map(|steps| steps.iter().any(computer_requires_approval))
            .unwrap_or(true),
        // Setting clipboard text is a routine, reversible local operation.
        // Reading or clearing it can expose/remove unrelated user data.
        "clipboard_set" => false,
        "clipboard_clear" | "clipboard_get" => true,
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
    use super::{approval_arguments, requires_approval as check};
    use serde_json::Value;

    #[test]
    fn task_delegation_requires_confirmation_and_pins_the_workspace_root() {
        assert!(check(
            "task",
            r#"{"action":"delegate","task":"fix the build","profile":"workspace","workspace":"src"}"#
        ));
        assert!(check("task", r#"{"action":"delegate","task":"summarise"}"#));
        assert!(check(
            "task",
            r#"{"action":"delegate","task":"summarise","profile":"research"}"#
        ));
        assert!(!check("task", r#"{"action":"list"}"#));
        assert!(!check("task", r#"{"action":"cancel","id":3}"#));

        let stored = approval_arguments(
            "task",
            r#"{"action":"delegate","task":"fix","profile":"workspace","workspace":"src/."}"#,
        );
        let stored: Value = serde_json::from_str(&stored).unwrap();
        let canonical = std::fs::canonicalize("src").unwrap();
        assert_eq!(stored["workspace"], canonical.to_string_lossy().as_ref());
        assert_eq!(
            approval_arguments("shell", r#"{"command":"ls"}"#),
            r#"{"command":"ls"}"#
        );
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
