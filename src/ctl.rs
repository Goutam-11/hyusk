//! `hyusk_agent ctl ...`: write one validated control command.
//!
//! A keybind or script can pause, resume, approve, or deny without a panel and
//! without hand-writing JSON. The command goes through the same parser and
//! revision rule the agent uses for every other control writer.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::control_file::{self, Action};

pub const USAGE: &str =
    "usage: hyusk_agent ctl <pause|resume|stop|approve|approve-turn|deny|dismiss>";

/// Map the keybind verbs to control actions.
pub fn action_from_args(args: &[String]) -> Result<Action, String> {
    let Some(command) = args.first() else {
        return Err(format!("missing command; {USAGE}"));
    };
    if let Some(extra) = args.get(1) {
        return Err(format!("unexpected argument {extra:?}; {USAGE}"));
    }

    match command.as_str() {
        "pause" => Ok(Action::StopListening),
        "resume" => Ok(Action::StartListening),
        "stop" => Ok(Action::StopTask),
        "approve" => Ok(Action::Approve),
        "approve-turn" => Ok(Action::ApproveTurn),
        "deny" => Ok(Action::Deny),
        "dismiss" => Ok(Action::DismissCard),
        other => Err(format!("unknown command {other:?}; {USAGE}")),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Write one command atomically with a revision newer than both the clock and
/// the revision already on disk. Returns the revision it wrote.
pub fn write_command(path: &Path, action: &Action, now_ms: u64) -> Result<u64, String> {
    let revision = control_file::next_revision(control_file::read_revision(path), now_ms);
    let contents = control_file::encode(action, revision);
    control_file::write_atomic(path, &contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(revision)
}

fn run_with_path(args: &[String], path: &Path, now_ms: u64) -> Result<String, String> {
    let action = action_from_args(args)?;
    let revision = write_command(path, &action, now_ms)?;
    Ok(format!(
        "Sent {} (revision {revision}) to {}.",
        action.name(),
        path.display()
    ))
}

/// Entry point for `hyusk_agent ctl ...`.
pub fn run(args: &[String]) -> Result<String, String> {
    run_with_path(args, &control_file::path(), now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "hyusk-ctl-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory.join("hyusk-control.json")
    }

    #[test]
    fn maps_each_keybind_verb_to_its_control_action() {
        for (verb, expected) in [
            ("pause", Action::StopListening),
            ("resume", Action::StartListening),
            ("stop", Action::StopTask),
            ("approve", Action::Approve),
            ("approve-turn", Action::ApproveTurn),
            ("deny", Action::Deny),
            ("dismiss", Action::DismissCard),
        ] {
            assert_eq!(
                action_from_args(&args(&[verb])).expect(verb),
                expected,
                "{verb}"
            );
        }
    }

    #[test]
    fn rejects_missing_or_unknown_commands_with_usage() {
        let error = action_from_args(&[]).expect_err("a verb is required");
        assert!(error.contains("usage"), "{error}");

        let error = action_from_args(&args(&["launch-rockets"])).expect_err("unknown verb");
        assert!(error.contains("launch-rockets"), "{error}");
        assert!(error.contains("usage"), "{error}");
    }

    #[test]
    fn writes_a_parseable_command_and_advances_the_revision() {
        let path = scratch("write");

        let first = write_command(&path, &Action::StopListening, 1_000).expect("first command");
        assert_eq!(first, 1_000);
        let control = control_file::parse(&std::fs::read_to_string(&path).expect("read"))
            .expect("written command parses");
        assert_eq!(control.revision, 1_000);
        assert_eq!(control.action, Action::StopListening);

        // The same clock must not replay the previous command.
        let second = write_command(&path, &Action::StartListening, 1_000).expect("second command");
        assert_eq!(second, 1_001);
        assert_eq!(control_file::read_revision(&path), Some(1_001));

        // A hand-written file with a future revision is still respected.
        let handwritten = r#"{"schema":1,"revision":5000,"action":"dismiss_card"}"#;
        control_file::write_atomic(&path, handwritten).expect("hand-written command");
        let third = write_command(&path, &Action::Deny, 1_000).expect("third command");
        assert_eq!(third, 5_001);
        let control = control_file::parse(&std::fs::read_to_string(&path).expect("read"))
            .expect("third command parses");
        assert_eq!(control.action, Action::Deny);

        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn run_reports_the_command_and_revision() {
        let path = scratch("run");
        let message = run_with_path(&args(&["pause"]), &path, 42).expect("pause");
        assert!(message.contains("stop_listening"), "{message}");
        assert!(message.contains("revision 42"), "{message}");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }
}
