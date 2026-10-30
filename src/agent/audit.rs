//! Approval audit log (A10).
//!
//! The action journal records what the agent did; this file records what the
//! user decided. One JSON line is appended per decision - approved, denied,
//! dropped, expired, or a click that arrived after expiry - so the sequence of
//! confirmations can be reviewed afterwards.
//!
//! A decision is recorded at the single point that resolves the armed call, so
//! a repeated event cannot double it. The preview is redacted on the way out
//! (X2 values, then X4 credential paths), because the log outlives the turn and
//! must never become a secret store. The file is `approvals.jsonl` in the data
//! directory (`$XDG_DATA_HOME/hyusk`, directory `0700`, file `0600`) and is
//! rotated by size into one previous copy, so at most two bounded files exist.
//!
//! Rotation and the append are small, synchronous file operations: a decision
//! happens once per card, not per tool call, and nothing waits on the log. A
//! failed write is reported and dropped rather than failing the turn.

use std::{io::Write, path::PathBuf};

use crate::types::event::ApprovalScope;

/// The live audit file.
pub const LOG_NAME: &str = "approvals.jsonl";

/// The rotated copy. The live file is moved here before an append would exceed
/// [`MAX_BYTES`], replacing the previous copy, so storage stays bounded.
pub const MAX_BYTES: u64 = 256 * 1024;

/// What happened to an armed call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The user allowed the stored call.
    Approved,
    /// The user refused it.
    Denied,
    /// The user asked for something else, so the call was dropped unrun.
    Dropped,
    /// The confirmation timed out.
    Expired,
    /// A panel click arrived with nothing armed.
    Stale,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Approved => "approved",
            Outcome::Denied => "denied",
            Outcome::Dropped => "dropped",
            Outcome::Expired => "expired",
            Outcome::Stale => "stale",
        }
    }
}

fn scope_name(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::Once => "once",
        ApprovalScope::Turn => "turn",
    }
}

/// The data directory: `$XDG_DATA_HOME/hyusk`, else `~/.local/share/hyusk`.
#[cfg(not(test))]
fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("hyusk")
}

/// A test's scratch log, so no test writes the real data directory.
#[cfg(test)]
static TEST_LOG: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Point every later record at a scratch file and return its path.
///
/// Process state rather than an environment variable: tests run in parallel
/// threads, and mutating `XDG_DATA_HOME` would move the session, memory, and
/// status files other tests use.
#[cfg(test)]
pub(crate) fn use_scratch_log() -> PathBuf {
    TEST_LOG
        .get_or_init(|| {
            let directory =
                std::env::temp_dir().join(format!("hyusk-audit-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&directory);
            directory.join(LOG_NAME)
        })
        .clone()
}

/// The audit log the runtime writes.
#[cfg(not(test))]
pub fn default_path() -> PathBuf {
    data_dir().join(LOG_NAME)
}

/// In a test build every decision goes to the scratch log, so no test can write
/// the real data directory whichever test runs first.
#[cfg(test)]
pub fn default_path() -> PathBuf {
    use_scratch_log()
}

/// One JSON line for a decision.
///
/// `tool` and `preview` are absent when nothing was armed (a stale click); the
/// `turn` is the turn whose processing resolved the call, and absent when the
/// event loop resolved it outside a turn.
pub fn line(
    outcome: Outcome,
    scope: Option<ApprovalScope>,
    tool: Option<&str>,
    preview: Option<&str>,
    turn: Option<u64>,
) -> String {
    let preview = preview.map(scrub);
    let value = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "tool": tool,
        "preview": preview,
        "decision": outcome.as_str(),
        "scope": scope.map(scope_name),
        "turn": turn,
    });
    value.to_string()
}

/// Remove secret-shaped values (X2) and credential paths (X4) from text the log
/// keeps beyond the turn.
fn scrub(text: &str) -> String {
    let (text, _) = crate::tools::redact::redact(text);
    let (text, _) = crate::tools::safety::redact_secret_paths(&text);
    text
}

/// Append one decision to the audit log. A failure is reported, never fatal.
pub fn record(
    outcome: Outcome,
    scope: Option<ApprovalScope>,
    tool: Option<&str>,
    preview: Option<&str>,
    turn: Option<u64>,
) {
    let path = default_path();
    let entry = line(outcome, scope, tool, preview, turn);
    if let Err(error) = record_at(&path, &entry) {
        eprintln!("[Audit] Could not write the approval log: {error}");
    }
}

/// Append one line to `path`, creating the private directory and file.
pub fn record_at(path: &std::path::Path, entry: &str) -> std::io::Result<()> {
    record_at_with_limit(path, entry, MAX_BYTES)
}

fn record_at_with_limit(path: &std::path::Path, entry: &str, limit: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        crate::paths::ensure_private_dir(parent)?;
    }
    rotate_if_needed(path, limit)?;

    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{entry}")?;
    drop(file);
    // An existing file keeps its old mode; the log holds previews, so make it
    // owner-only again after every write.
    crate::paths::restrict_file(path);
    Ok(())
}

/// Move the live file aside once it has reached `limit`, keeping one copy.
fn rotate_if_needed(path: &std::path::Path, limit: u64) -> std::io::Result<()> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() < limit {
        return Ok(());
    }
    let previous = previous_path(path);
    std::fs::rename(path, previous)?;
    Ok(())
}

fn previous_path(path: &std::path::Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".1");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn scratch(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "hyusk-audit-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    #[test]
    fn every_outcome_and_scope_has_a_name() {
        for (outcome, name) in [
            (Outcome::Approved, "approved"),
            (Outcome::Denied, "denied"),
            (Outcome::Dropped, "dropped"),
            (Outcome::Expired, "expired"),
            (Outcome::Stale, "stale"),
        ] {
            assert_eq!(outcome.as_str(), name);
        }
        assert_eq!(scope_name(ApprovalScope::Once), "once");
        assert_eq!(scope_name(ApprovalScope::Turn), "turn");
    }

    #[test]
    fn a_decision_is_one_private_json_line_without_secrets() {
        let directory = scratch("line");
        let path = directory.join(LOG_NAME);
        let preview = "shell command=OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef \
                       cat ~/.ssh/id_ed25519 build";
        record_at(
            &path,
            &line(Outcome::Denied, None, Some("shell"), Some(preview), Some(7)),
        )
        .expect("record");

        let text = std::fs::read_to_string(&path).expect("read log");
        assert_eq!(text.lines().count(), 1, "one decision is one line");

        let value: Value = serde_json::from_str(text.trim()).expect("valid JSON");
        assert_eq!(value["decision"], "denied");
        assert_eq!(value["tool"], "shell");
        assert_eq!(value["turn"], 7);
        assert!(value["scope"].is_null(), "a denial grants no scope");
        assert!(value["timestamp"]
            .as_str()
            .is_some_and(|time| time.ends_with('Z') && time.contains('T')));

        // The preview is kept for review, minus the secret and the credential
        // path: an audit line is kept longer than the turn.
        let recorded = value["preview"].as_str().expect("preview");
        assert!(recorded.contains("shell command="), "{recorded}");
        assert!(
            !recorded.contains("sk-or-v1-0123456789abcdef"),
            "{recorded}"
        );
        assert!(!recorded.contains("id_ed25519"), "{recorded}");
        assert!(recorded.contains("[redacted"), "{recorded}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the log is owner-only");
            let mode = std::fs::metadata(&directory)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "the directory is owner-only");
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn an_approved_call_records_its_scope_and_turn() {
        let directory = scratch("scope");
        let path = directory.join(LOG_NAME);
        record_at(
            &path,
            &line(
                Outcome::Approved,
                Some(ApprovalScope::Turn),
                Some("shell"),
                Some("shell command=rm -rf build"),
                Some(12),
            ),
        )
        .expect("record");
        let text = std::fs::read_to_string(&path).expect("read log");
        let value: Value = serde_json::from_str(text.trim()).expect("valid JSON");
        assert_eq!(value["decision"], "approved");
        assert_eq!(value["scope"], "turn");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_stale_click_names_no_call_and_no_turn() {
        let directory = scratch("stale");
        let path = directory.join(LOG_NAME);
        record_at(
            &path,
            &line(Outcome::Stale, Some(ApprovalScope::Once), None, None, None),
        )
        .expect("record");
        let text = std::fs::read_to_string(&path).expect("read log");
        let value: Value = serde_json::from_str(text.trim()).expect("valid JSON");
        assert_eq!(value["decision"], "stale");
        assert_eq!(value["scope"], "once", "the click's own scope is kept");
        assert!(value["tool"].is_null());
        assert!(value["preview"].is_null());
        assert!(value["turn"].is_null());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn rotation_keeps_the_live_file_and_one_previous_copy() {
        let directory = scratch("rotate");
        let path = directory.join(LOG_NAME);
        let previous = previous_path(&path);
        let limit = 180u64;
        for turn in 0..12 {
            let entry = line(
                Outcome::Approved,
                Some(ApprovalScope::Once),
                Some("shell"),
                Some("shell command=rm -rf build"),
                Some(turn),
            );
            record_at_with_limit(&path, &entry, limit).expect("record");
        }

        assert!(
            previous.exists(),
            "a bounded log rotates instead of growing"
        );
        let live = std::fs::metadata(&path).expect("live").len();
        let older = std::fs::metadata(&previous).expect("previous").len();
        // A file rotates only once it has reached the limit, so the live file
        // holds at most one line past it and the pair stays bounded.
        assert!(
            live < limit + 200,
            "the live file holds at most one line past the limit: {live}"
        );
        assert!(
            older >= limit,
            "the previous copy is the one that filled up"
        );
        assert!(live + older <= 2 * (limit + 200), "storage stays bounded");
        // Both files hold whole JSON lines, so a reader never sees a fragment.
        for file in [&path, &previous] {
            let text = std::fs::read_to_string(file).expect("read");
            for line in text.lines() {
                let value: Value = serde_json::from_str(line).expect("valid JSON");
                assert_eq!(value["decision"], "approved");
            }
        }

        // The next rotation replaces the previous copy rather than adding one.
        for turn in 0..12 {
            let entry = line(
                Outcome::Approved,
                Some(ApprovalScope::Once),
                Some("shell"),
                Some("shell command=rm -rf build"),
                Some(turn),
            );
            record_at_with_limit(&path, &entry, limit).expect("record");
        }
        let entries: usize = std::fs::read_dir(&directory)
            .expect("list")
            .filter_map(Result::ok)
            .count();
        assert_eq!(entries, 2, "only the live file and one previous copy exist");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_missing_parent_directory_is_created_private() {
        let directory = scratch("parent");
        let path = directory.join("nested").join(LOG_NAME);
        record_at(&path, &line(Outcome::Denied, None, None, None, None)).expect("record");
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path.parent().expect("parent"))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        let _ = std::fs::remove_dir_all(&directory);
    }
}
