//! The `hyusk-control.json` integration contract.
//!
//! Bars, keybinds, scripts, and the bundled indicators all drive the running
//! agent through one small runtime file. This module is the single parser and
//! writer for that file: the agent consumes it, `hyusk_agent ctl` writes it,
//! and `docs/integration.md` documents the same shapes.
//!
//! A command is single-use: it carries a wall-clock `revision`, and the agent
//! only accepts a revision newer than the last one it processed. Writers must
//! therefore advance the revision, which [`next_revision`] does.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;

/// The only control-file schema this build understands.
pub const SCHEMA: u32 = 1;

/// One command a client may send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    StartListening,
    StopTask,
    StopListening,
    SetModel {
        provider: Option<String>,
        model: Option<String>,
    },
    SetApiKey {
        provider: Option<String>,
        key: Option<String>,
        /// Older writers put the key in `text`.
        text: Option<String>,
    },
    RefreshModels,
    PairPhone,
    DismissCard,
    Approve,
    ApproveTurn,
    Deny,
    RunWorkflow {
        workflow: Option<String>,
        /// Older writers put the workflow name in `text`.
        text: Option<String>,
    },
    ReloadWorkflows,
    ScheduleWorkflow {
        text: Option<String>,
    },
    ScheduleReminder {
        text: Option<String>,
    },
    /// The agent's own scrub marker after it reads a key. Accepted so a
    /// restart can read the revision instead of replaying the previous action.
    KeyReceived,
}

impl Action {
    /// The action word written in the file.
    pub fn name(&self) -> &'static str {
        match self {
            Action::StartListening => "start_listening",
            Action::StopTask => "stop_task",
            Action::StopListening => "stop_listening",
            Action::SetModel { .. } => "set_model",
            Action::SetApiKey { .. } => "set_api_key",
            Action::RefreshModels => "refresh_models",
            Action::PairPhone => "pair_phone",
            Action::DismissCard => "dismiss_card",
            Action::Approve => "approve",
            Action::ApproveTurn => "approve_turn",
            Action::Deny => "deny",
            Action::RunWorkflow { .. } => "run_workflow",
            Action::ReloadWorkflows => "reload_workflows",
            Action::ScheduleWorkflow { .. } => "schedule_workflow",
            Action::ScheduleReminder { .. } => "schedule_reminder",
            Action::KeyReceived => "key_received",
        }
    }

    /// The workflow name a `run_workflow` command names, if any.
    pub fn workflow_name(&self) -> Option<&str> {
        let Action::RunWorkflow { workflow, text } = self else {
            return None;
        };
        workflow
            .as_deref()
            .or(text.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
}

/// A validated control command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub revision: u64,
    pub action: Action,
}

/// The embedded payload of `schedule_workflow`/`schedule_reminder` (`text`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ScheduleControl {
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub delay_seconds: u64,
}

#[derive(Deserialize)]
struct RawControl {
    #[serde(default)]
    schema: Option<u32>,
    #[serde(default)]
    revision: u64,
    action: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    workflow: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

/// Parse and validate one control file.
///
/// A malformed file, an unknown action, revision zero, or an unknown schema is
/// rejected. Field-level checks that the runtime turns into an error card
/// (an empty key, workflow, or schedule) stay in the runtime so its guidance is
/// unchanged.
pub fn parse(raw: &str) -> Result<Control, String> {
    let raw: RawControl =
        serde_json::from_str(raw).map_err(|error| format!("not valid JSON: {error}"))?;

    if let Some(schema) = raw.schema {
        if schema != SCHEMA {
            return Err(format!(
                "unsupported control schema {schema}; this build expects {SCHEMA}"
            ));
        }
    }
    if raw.revision == 0 {
        return Err("revision must be a positive wall-clock value".to_string());
    }

    let action = match raw.action.as_str() {
        "start_listening" => Action::StartListening,
        "stop_task" => Action::StopTask,
        "stop_listening" => Action::StopListening,
        "refresh_models" => Action::RefreshModels,
        "pair_phone" => Action::PairPhone,
        "dismiss_card" => Action::DismissCard,
        "approve" => Action::Approve,
        "approve_turn" => Action::ApproveTurn,
        "deny" => Action::Deny,
        "reload_workflows" => Action::ReloadWorkflows,
        "key_received" => Action::KeyReceived,
        "set_model" => Action::SetModel {
            provider: raw.provider,
            model: raw.model,
        },
        "set_api_key" => Action::SetApiKey {
            provider: raw.provider,
            key: raw.key,
            text: raw.text,
        },
        "run_workflow" => Action::RunWorkflow {
            workflow: raw.workflow,
            text: raw.text,
        },
        "schedule_workflow" => Action::ScheduleWorkflow { text: raw.text },
        "schedule_reminder" => Action::ScheduleReminder { text: raw.text },
        other => return Err(format!("unknown control action {other:?}")),
    };

    Ok(Control {
        revision: raw.revision,
        action,
    })
}

/// Encode one command for the file. The result always carries `schema` and a
/// positive revision.
pub fn encode(action: &Action, revision: u64) -> String {
    let mut value = json!({
        "schema": SCHEMA,
        "revision": revision,
        "action": action.name(),
    });

    match action {
        Action::SetModel { provider, model } => {
            if let Some(provider) = provider {
                value["provider"] = json!(provider);
            }
            if let Some(model) = model {
                value["model"] = json!(model);
            }
        }
        Action::SetApiKey {
            provider,
            key,
            text,
        } => {
            if let Some(provider) = provider {
                value["provider"] = json!(provider);
            }
            if let Some(key) = key {
                value["key"] = json!(key);
            }
            if let Some(text) = text {
                value["text"] = json!(text);
            }
        }
        Action::RunWorkflow { workflow, text } => {
            if let Some(workflow) = workflow {
                value["workflow"] = json!(workflow);
            }
            if let Some(text) = text {
                value["text"] = json!(text);
            }
        }
        Action::ScheduleWorkflow { text } | Action::ScheduleReminder { text } => {
            if let Some(text) = text {
                value["text"] = json!(text);
            }
        }
        _ => {}
    }

    value.to_string()
}

/// Decode the embedded schedule payload.
pub fn control_schedule(text: Option<&str>) -> Option<ScheduleControl> {
    let parsed: ScheduleControl = serde_json::from_str(text?).ok()?;
    (parsed.delay_seconds > 0).then_some(parsed)
}

/// The runtime control file path.
pub fn path() -> PathBuf {
    let directory = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    Path::new(&directory).join("hyusk-control.json")
}

/// The revision the current file carries, if it parses.
pub fn read_revision(path: &Path) -> Option<u64> {
    let contents = std::fs::read_to_string(path).ok()?;
    parse(&contents).ok().map(|control| control.revision)
}

/// A revision strictly newer than `current` and at least `now_ms`, so a
/// wall-clock writer and an older file can never replay a command.
pub fn next_revision(current: Option<u64>, now_ms: u64) -> u64 {
    now_ms.max(current.map_or(0, |revision| revision.saturating_add(1)))
}

/// Replace the control file atomically with an owner-only file.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the control path has no parent directory",
        )
    })?;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(
        ".hyusk-control-{}-{unique}.tmp",
        std::process::id()
    ));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(&temporary)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);

    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn with(action: &str) -> String {
        format!(r#"{{"schema":1,"revision":42,"action":{action}}}"#)
    }

    #[test]
    fn parses_every_known_command() {
        let fixtures = [
            (with(r#""start_listening""#), Action::StartListening),
            (with(r#""stop_task""#), Action::StopTask),
            (with(r#""stop_listening""#), Action::StopListening),
            (with(r#""refresh_models""#), Action::RefreshModels),
            (with(r#""pair_phone""#), Action::PairPhone),
            (with(r#""dismiss_card""#), Action::DismissCard),
            (with(r#""approve""#), Action::Approve),
            (with(r#""approve_turn""#), Action::ApproveTurn),
            (with(r#""deny""#), Action::Deny),
            (with(r#""reload_workflows""#), Action::ReloadWorkflows),
            (with(r#""key_received""#), Action::KeyReceived),
            (
                with(r#""set_model","provider":"openrouter","model":"openai/gpt-4o""#),
                Action::SetModel {
                    provider: Some("openrouter".to_string()),
                    model: Some("openai/gpt-4o".to_string()),
                },
            ),
            (
                with(r#""set_api_key","provider":"openrouter","key":"sk-test""#),
                Action::SetApiKey {
                    provider: Some("openrouter".to_string()),
                    key: Some("sk-test".to_string()),
                    text: None,
                },
            ),
            (
                with(r#""set_api_key","provider":"openrouter","text":"sk-legacy""#),
                Action::SetApiKey {
                    provider: Some("openrouter".to_string()),
                    key: None,
                    text: Some("sk-legacy".to_string()),
                },
            ),
            (
                with(r#""run_workflow","workflow":"Focus""#),
                Action::RunWorkflow {
                    workflow: Some("Focus".to_string()),
                    text: None,
                },
            ),
            (
                with(
                    r#""schedule_workflow","text":"{\"workflow\":\"Focus\",\"delay_seconds\":60}""#,
                ),
                Action::ScheduleWorkflow {
                    text: Some(r#"{"workflow":"Focus","delay_seconds":60}"#.to_string()),
                },
            ),
            (
                with(r#""schedule_reminder","text":"{\"text\":\"tea\",\"delay_seconds\":300}""#),
                Action::ScheduleReminder {
                    text: Some(r#"{"text":"tea","delay_seconds":300}"#.to_string()),
                },
            ),
        ];

        for (raw, expected) in fixtures {
            let control = parse(&raw).unwrap_or_else(|error| panic!("{raw}: {error}"));
            assert_eq!(control.revision, 42, "{raw}");
            assert_eq!(control.action, expected, "{raw}");
        }
    }

    #[test]
    fn accepts_a_file_without_a_schema_as_version_one() {
        let control = parse(r#"{"revision":9,"action":"stop_task"}"#).expect("legacy control");
        assert_eq!(control.revision, 9);
        assert_eq!(control.action, Action::StopTask);
    }

    #[test]
    fn rejects_malformed_input_and_unknown_versions() {
        for (raw, reason) in [
            ("not json", "valid JSON"),
            (r#"{"revision":1,"action":"launch_rockets"}"#, "unknown"),
            (r#"{"revision":0,"action":"stop_task"}"#, "revision"),
            (
                r#"{"schema":2,"revision":1,"action":"stop_task"}"#,
                "schema",
            ),
            (r#"{"revision":1}"#, "action"),
        ] {
            let error = parse(raw).expect_err("malformed input must be refused");
            assert!(error.contains(reason), "{raw}: {error}");
        }
    }

    #[test]
    fn workflow_name_prefers_the_standalone_field_and_ignores_blanks() {
        let control = parse(&with(
            r#""run_workflow","workflow":" Focus ","text":"legacy""#,
        ))
        .expect("workflow control");
        assert_eq!(control.action.workflow_name(), Some("Focus"));

        let legacy =
            parse(&with(r#""run_workflow","text":"  Focus  ""#)).expect("legacy workflow control");
        assert_eq!(legacy.action.workflow_name(), Some("Focus"));

        let blank =
            parse(&with(r#""run_workflow","workflow":"  ","text":"""#)).expect("blank control");
        assert_eq!(blank.action.workflow_name(), None);
    }

    #[test]
    fn schedule_payload_is_decoded_and_requires_a_positive_delay() {
        let control = parse(&with(
            r#""schedule_workflow","text":"{\"workflow\":\"Focus\",\"delay_seconds\":900}""#,
        ))
        .expect("schedule control");
        let Action::ScheduleWorkflow { text } = control.action else {
            panic!("expected a schedule action");
        };
        let schedule = control_schedule(text.as_deref()).expect("decoded schedule");
        assert_eq!(schedule.workflow.as_deref(), Some("Focus"));
        assert_eq!(schedule.delay_seconds, 900);

        assert!(control_schedule(Some(r#"{"workflow":"Focus","delay_seconds":0}"#)).is_none());
        assert!(control_schedule(Some("not json")).is_none());
        assert!(control_schedule(None).is_none());
    }

    #[test]
    fn encodes_a_parseable_command_with_schema_and_revision() {
        for action in [
            Action::StopListening,
            Action::StartListening,
            Action::StopTask,
            Action::Approve,
            Action::ApproveTurn,
            Action::Deny,
            Action::DismissCard,
        ] {
            let encoded = encode(&action, 1234);
            let value: Value = serde_json::from_str(&encoded).expect("encoded control is JSON");
            assert_eq!(value["schema"], 1);
            assert_eq!(value["revision"], 1234);
            assert_eq!(value["action"], action.name());
            assert_eq!(
                parse(&encoded).expect("encoded control parses").action,
                action
            );
        }
    }

    #[test]
    fn a_new_revision_always_advances_and_never_goes_back() {
        assert_eq!(next_revision(None, 1000), 1000);
        assert_eq!(next_revision(Some(1000), 1000), 1001);
        assert_eq!(next_revision(Some(999), 1000), 1000);
        assert_eq!(next_revision(Some(u64::MAX), 1000), u64::MAX);
    }

    #[test]
    fn write_atomic_replaces_the_file_owner_only() {
        let directory = std::env::temp_dir().join(format!(
            "hyusk-control-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        let path = directory.join("hyusk-control.json");

        let first = encode(&Action::StopTask, 10);
        write_atomic(&path, &first).expect("first write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), first);
        assert_eq!(read_revision(&path), Some(10));

        let second = encode(&Action::StartListening, 11);
        write_atomic(&path, &second).expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), second);
        assert_eq!(read_revision(&path), Some(11));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the control file must be owner-only");
        }

        let files: Vec<_> = std::fs::read_dir(&directory)
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(files.len(), 1, "no temporary file may remain: {files:?}");

        let _ = std::fs::remove_dir_all(&directory);
    }
}
