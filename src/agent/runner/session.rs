use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::types::Message;

/// Bytes per token assumed when converting a model's context window into a
/// serialized-history budget. English prose and JSON tool output sit near
/// this ratio; the conversion stays deliberately conservative.
const BYTES_PER_TOKEN: usize = 4;
/// Context kept free for the system prompt, tool schemas, the desktop
/// observation, and the reply itself.
const MIN_RESERVED_TOKENS: usize = 2_000;
const MAX_RESERVED_TOKENS: usize = 8_000;
/// Window assumed when the model id matches no known family.
const DEFAULT_CONTEXT_TOKENS: usize = 32_768;
/// Budget bounds: a tiny window still keeps the latest exchange useful, and a
/// huge window still bounds the cost of one request.
const MIN_HISTORY_BYTES: usize = 16_000;
const MAX_HISTORY_BYTES: usize = 120_000;
const MIN_RECENT_TURNS: usize = 6;
const COMPACT_TOOL_OUTPUT_BYTES: usize = 4_000;
const COMPACT_USER_INPUT_BYTES: usize = 8_000;
const COMPACT_ASSISTANT_OUTPUT_BYTES: usize = 8_000;
const PERSISTED_SESSION_MESSAGES: usize = 80;

#[derive(Debug, Serialize, Deserialize)]
struct SessionSnapshot {
    version: u8,
    messages: Vec<Message>,
    #[serde(default)]
    journal: Vec<ActionJournalEntry>,
    #[serde(default)]
    task_paused: bool,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
}

pub(super) type LoadedSession = (
    Vec<Message>,
    Vec<ActionJournalEntry>,
    bool,
    Option<(String, String)>,
);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ActionJournalEntry {
    pub(super) tool: String,
    pub(super) arguments: String,
    pub(super) status: String,
}

pub(super) fn load_session() -> LoadedSession {
    let path = session_path();
    let snapshot = std::fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str::<SessionSnapshot>(&contents).ok());
    match snapshot.filter(|value| value.version == 1 || value.version == 2) {
        Some(value) => {
            let uncertain = value
                .journal
                .iter()
                .filter(|entry| entry.status.starts_with("in_flight"))
                .map(|entry| format!("{}: {}", entry.tool, entry.status))
                .collect::<Vec<_>>();
            let mut messages = sanitize_session_messages(value.messages);
            if !uncertain.is_empty() {
                messages.push(Message::assistant(format!("An earlier action may have completed before interruption and needs verification before retry: {}", uncertain.join("; "))));
            }
            let identity = (!value.provider.is_empty() && !value.model.is_empty())
                .then_some((value.provider, value.model));
            (messages, value.journal, value.task_paused, identity)
        }
        None => (Vec::new(), Vec::new(), false, None),
    }
}

pub(super) fn save_session(
    messages: &[Message],
    journal: &[ActionJournalEntry],
    task_paused: bool,
    provider: &str,
    model: &str,
) {
    save_session_to_path(
        &session_path(),
        messages,
        journal,
        task_paused,
        provider,
        model,
    );
}

fn save_session_to_path(
    path: &Path,
    messages: &[Message],
    journal: &[ActionJournalEntry],
    task_paused: bool,
    provider: &str,
    model: &str,
) {
    let snapshot = SessionSnapshot {
        version: 2,
        messages: sanitize_session_messages(messages.to_vec()),
        journal: journal
            .iter()
            .rev()
            .take(32)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
        task_paused,
        provider: provider.to_string(),
        model: model.to_string(),
    };
    let bytes = match serde_json::to_vec(&snapshot) {
        Ok(bytes) => bytes,
        Err(_) => {
            eprintln!("Could not serialize agent session");
            return;
        }
    };
    if let Err(error) = save_bytes_atomically(path, &bytes) {
        eprintln!("Could not save agent session ({})", error.kind());
    }
}

fn save_bytes_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    save_bytes_atomically_with(
        path,
        bytes,
        |file, bytes| file.write_all(bytes),
        |from, to| fs::rename(from, to),
    )
}

fn save_bytes_atomically_with(
    path: &Path,
    bytes: &[u8],
    write_contents: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    rename_file: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "session path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;

    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "session path has no file name")
    })?;
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let (temporary, mut file) = loop {
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
        let temporary = parent.join(temporary_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };

    let write_result = write_contents(&mut file, bytes);
    drop(file);
    let result = write_result.and_then(|()| rename_file(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn session_path() -> PathBuf {
    std::env::var_os("HYUSK_SESSION_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::tools::memory::memory_dir().join("session.json"))
}

pub(super) fn sanitize_session_messages(messages: Vec<Message>) -> Vec<Message> {
    let mut safe = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        if message.role == "user" && !is_screenshot_context(message) {
            safe.push(Message::user(truncate_bytes(
                &message.text(),
                COMPACT_USER_INPUT_BYTES,
            )));
            index += 1;
            continue;
        }
        if message.role == "assistant" {
            if let Some(calls) = &message.tool_calls {
                let mut existing = std::collections::HashMap::new();
                let mut consumed = 0;
                while let Some(result) = messages.get(index + 1 + consumed) {
                    if result.role == "tool" {
                        if let Some(id) = &result.tool_call_id {
                            existing.insert(id.clone(), result.clone());
                        }
                    } else if !is_screenshot_context(result) {
                        break;
                    }
                    consumed += 1;
                }
                safe.push(message.clone());
                for call in calls {
                    safe.push(existing.remove(&call.id).unwrap_or_else(|| Message::tool_result(
                        call.id.clone(), "The action outcome is uncertain after interruption. Verify the external state before retrying it.",
                    )));
                }
                index += consumed + 1;
                continue;
            } else if !message.text().trim().is_empty() {
                safe.push(Message::assistant(truncate_bytes(
                    &message.text(),
                    COMPACT_ASSISTANT_OUTPUT_BYTES,
                )));
            }
        }
        index += 1;
    }
    trim_session_messages(&mut safe, PERSISTED_SESSION_MESSAGES);
    while safe.first().is_some_and(|message| message.role != "user") {
        safe.remove(0);
    }
    safe
}

/// Bound persisted history by dropping complete old user turns. Keeping an
/// oversized single turn is safer than splitting an assistant tool-call batch
/// from the tool results required to complete its protocol sequence.
pub(super) fn trim_session_messages(messages: &mut Vec<Message>, max_messages: usize) {
    while messages.len() > max_messages {
        let Some(next_turn) = messages
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, message)| {
                (message.role == "user" && !is_screenshot_context(message)).then_some(index)
            })
        else {
            break;
        };
        messages.drain(..next_turn);
    }
}

/// Serialized-history budget for the selected model.
///
/// The window comes from [`context_window_tokens`];
/// `HYUSK_AGENT_MAX_HISTORY_BYTES` overrides it when the table guesses wrong,
/// for example for a locally hosted model with a tuned context size.
pub(super) fn history_budget(model: &str) -> usize {
    configured_history_budget()
        .unwrap_or_else(|| derived_history_budget(context_window_tokens(model)))
}

fn configured_history_budget() -> Option<usize> {
    std::env::var("HYUSK_AGENT_MAX_HISTORY_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value >= MIN_HISTORY_BYTES)
}

/// Convert a context window into a history budget, leaving room for the
/// request scaffolding and the reply.
fn derived_history_budget(context_tokens: usize) -> usize {
    let usable = context_tokens.saturating_sub(reserved_tokens(context_tokens));
    usable
        .saturating_mul(BYTES_PER_TOKEN)
        .clamp(MIN_HISTORY_BYTES, MAX_HISTORY_BYTES)
}

fn reserved_tokens(context_tokens: usize) -> usize {
    (context_tokens / 4).clamp(MIN_RESERVED_TOKENS, MAX_RESERVED_TOKENS)
}

/// Context windows of the model families this agent is pointed at.
///
/// Provider catalogs do not expose this consistently (OpenAI's `/models`
/// omits it), so derive a conservative window from the identifier and let
/// the environment override anything that does not match.
fn context_window_tokens(model: &str) -> usize {
    let model = model.to_ascii_lowercase();
    const WINDOWS: &[(&str, usize)] = &[
        ("gemini", 1_048_576),
        ("claude", 200_000),
        ("gpt-oss", 131_072),
        ("gpt-4o", 128_000),
        ("gpt-4.1", 128_000),
        ("gpt-5", 128_000),
        ("nova", 128_000),
        ("llama", 128_000),
        ("deepseek", 64_000),
        ("qwen", 32_768),
        ("mistral", 32_768),
        ("mixtral", 32_768),
        ("gemma", 32_768),
    ];
    WINDOWS
        .iter()
        .find(|(family, _)| model.contains(family))
        .map_or(DEFAULT_CONTEXT_TOKENS, |(_, tokens)| *tokens)
}

/// Compact `messages` until they fit `budget` bytes.
///
/// Complete old turns are dropped first; only then are oversized individual
/// messages clipped. The active turn is never dropped, and a single turn that
/// still exceeds the budget is left protocol-complete rather than split.
/// Returns whether anything changed.
pub(super) fn compact_history(messages: &mut Vec<Message>, budget: usize) -> bool {
    let original_bytes = history_bytes(messages);
    if original_bytes <= budget {
        return false;
    }
    let original_len = messages.len();

    while history_bytes(messages) > budget && turn_starts(messages).len() > MIN_RECENT_TURNS {
        let starts = turn_starts(messages);
        messages.drain(starts[0]..starts[1]);
    }
    if history_bytes(messages) > budget {
        let mut index = 1;
        while index < messages.len() && history_bytes(messages) > budget {
            if messages[index].role == "user" && is_screenshot_context(&messages[index]) {
                messages.remove(index);
            } else {
                index += 1;
            }
        }
    }
    for message in messages.iter_mut().skip(1) {
        let limit = if message.role == "tool" {
            COMPACT_TOOL_OUTPUT_BYTES
        } else if message.role == "user" {
            COMPACT_USER_INPUT_BYTES
        } else if message.role == "assistant" && message.tool_calls.is_none() {
            COMPACT_ASSISTANT_OUTPUT_BYTES
        } else {
            continue;
        };
        let text = message.text();
        if text.len() <= limit {
            continue;
        }
        let clipped = truncate_bytes(&text, limit);
        if message.role == "tool" {
            let id = message.tool_call_id.clone().unwrap_or_default();
            *message = Message::tool_result(id, format!("{clipped}\n[history output compacted]"));
        } else if message.role == "user" {
            *message = Message::user(format!("{clipped}\n[history input compacted]"));
        } else {
            *message = Message::assistant(format!("{clipped}\n[history output compacted]"));
        }
    }
    while history_bytes(messages) > budget && turn_starts(messages).len() > 2 {
        let starts = turn_starts(messages);
        messages.drain(starts[0]..starts[1]);
    }

    messages.len() < original_len || history_bytes(messages) < original_bytes
}

fn history_bytes(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| {
            serde_json::to_vec(message)
                .map(|value| value.len())
                .unwrap_or(0)
        })
        .sum()
}

fn is_screenshot_context(message: &Message) -> bool {
    message.text().starts_with("Screenshot at ")
}

fn turn_starts(messages: &[Message]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, message)| {
            (message.role == "user" && !is_screenshot_context(message)).then_some(index)
        })
        .collect()
}

pub(super) fn truncate_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::{
        compact_history, context_window_tokens, derived_history_budget, history_bytes,
        sanitize_session_messages, save_bytes_atomically_with, save_session_to_path,
        trim_session_messages, ActionJournalEntry, SessionSnapshot, COMPACT_TOOL_OUTPUT_BYTES,
        MAX_HISTORY_BYTES, MIN_HISTORY_BYTES, PERSISTED_SESSION_MESSAGES,
    };
    use crate::types::Message;
    use serde_json::json;
    use std::{
        fs,
        io::{self, Write},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    fn test_session_path(label: &str) -> PathBuf {
        static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir()
            .join(format!(
                "hyusk-session-{label}-{}-{}",
                std::process::id(),
                TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
            ))
            .join("session.json")
    }

    fn assert_no_temp_files(path: &std::path::Path) {
        let parent = path.parent().unwrap();
        assert!(fs::read_dir(parent).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")
        }));
    }

    #[test]
    fn a_redacted_tool_result_round_trips_without_the_secret() {
        let path = test_session_path("redacted");
        let (output, replaced) =
            crate::tools::redact::redact("OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef");
        assert_eq!(replaced, 1);
        let output = format!("{output}{}", crate::tools::redact::note(replaced));
        // A tool result is kept only after the assistant call it answers.
        let call: Message = serde_json::from_value(json!({
            "role": "assistant",
            "tool_calls": [{
                "id": "call-1",
                "type": "function",
                "function": {
                    "name": "shell",
                    "arguments": "{\"command\":\"printenv OPENROUTER_API_KEY\"}"
                }
            }]
        }))
        .expect("assistant tool call");
        let messages = vec![
            Message::user("check the key"),
            call,
            Message::tool_result("call-1", output),
        ];

        save_session_to_path(&path, &messages, &[], false, "openrouter", "model-x");

        let text = String::from_utf8(fs::read(&path).expect("session file")).expect("utf-8");
        assert!(
            !text.contains("sk-or-v1-0123456789abcdef"),
            "the session kept the secret: {text}"
        );
        assert!(text.contains("[redacted]"), "{text}");
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn session_save_roundtrips_and_sets_private_file_mode() {
        let path = test_session_path("roundtrip");
        let messages = vec![Message::user("hello"), Message::assistant("world")];
        let journal = vec![ActionJournalEntry {
            tool: "shell".into(),
            arguments: "{}".into(),
            status: "completed".into(),
        }];
        save_session_to_path(&path, &messages, &journal, true, "openrouter", "model-x");

        let bytes = fs::read(&path).expect("session file written");
        let snapshot: SessionSnapshot = serde_json::from_slice(&bytes).expect("session roundtrip");
        assert_eq!(snapshot.version, 2);
        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(snapshot.journal.len(), 1);
        assert!(snapshot.task_paused);
        assert_eq!(snapshot.provider, "openrouter");
        assert_eq!(snapshot.model, "model-x");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_no_temp_files(&path);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_temp_write_preserves_previous_snapshot_and_cleans_own_temp() {
        let path = test_session_path("write-failure");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let previous = b"previous session";
        fs::write(&path, previous).unwrap();

        let result = save_bytes_atomically_with(
            &path,
            b"replacement",
            |file, bytes| {
                file.write_all(&bytes[..3])?;
                Err(io::Error::other("injected write failure"))
            },
            |from, to| fs::rename(from, to),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert_no_temp_files(&path);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_rename_preserves_previous_snapshot_and_cleans_own_temp() {
        let path = test_session_path("rename-failure");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let previous = b"previous session";
        fs::write(&path, previous).unwrap();

        let result = save_bytes_atomically_with(
            &path,
            b"replacement",
            |file, bytes| file.write_all(bytes),
            |_temporary, _destination| Err(io::Error::other("injected rename failure")),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert_no_temp_files(&path);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn persisted_session_keeps_bounded_plain_conversation() {
        let mut messages = vec![Message::system("system"), Message::assistant("orphan")];
        for index in 0..(PERSISTED_SESSION_MESSAGES + 10) {
            messages.push(if index % 2 == 0 {
                Message::user(format!("user {index}"))
            } else {
                Message::assistant(format!("assistant {index}"))
            });
        }
        messages.push(Message::user_with_image(
            "Screenshot at /tmp/private.png",
            "data:image/png;base64,AAAA",
        ));
        let saved = sanitize_session_messages(messages);
        assert!(saved.len() <= PERSISTED_SESSION_MESSAGES);
        assert_eq!(
            saved.first().map(|message| message.role.as_str()),
            Some("user")
        );
        assert!(saved
            .iter()
            .all(|message| message.role == "user" || message.role == "assistant"));
        assert!(saved
            .iter()
            .all(|message| !message.text().starts_with("Screenshot at ")));
    }

    #[test]
    fn persisted_session_repairs_incomplete_tool_batch() {
        let assistant: Message = serde_json::from_value(json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"shell","arguments":"{}"}},{"id":"call-2","type":"function","function":{"name":"computer","arguments":"{}"}}]})).unwrap();
        let saved = sanitize_session_messages(vec![
            Message::user("do task"),
            assistant,
            Message::tool_result("call-1", "done"),
        ]);
        assert_eq!(saved.len(), 4);
        assert_eq!(saved[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(saved[3].tool_call_id.as_deref(), Some("call-2"));
        assert!(saved[3].text().contains("uncertain"));
    }

    #[test]
    fn persisted_session_keeps_results_after_legacy_interleaved_screenshot() {
        let assistant: Message = serde_json::from_value(json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"computer","arguments":"{}"}},{"id":"call-2","type":"function","function":{"name":"computer","arguments":"{}"}}]})).unwrap();
        let saved = sanitize_session_messages(vec![
            Message::user("do task"),
            assistant,
            Message::tool_result("call-1", "first done"),
            Message::user("Screenshot at /tmp/old.png"),
            Message::tool_result("call-2", "second done"),
        ]);
        assert_eq!(saved.len(), 4);
        assert_eq!(saved[3].tool_call_id.as_deref(), Some("call-2"));
        assert!(saved[3].text().contains("second done"));
    }

    #[test]
    fn persisted_history_trims_only_complete_turns() {
        let assistant: Message = serde_json::from_value(json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"shell","arguments":"{}"}},{"id":"call-2","type":"function","function":{"name":"computer","arguments":"{}"}}]})).unwrap();
        let mut messages = vec![
            Message::user("old task"),
            assistant.clone(),
            Message::tool_result("call-1", "first done"),
            Message::tool_result("call-2", "second done"),
            Message::assistant("old final"),
            Message::user("new task"),
            assistant,
            Message::tool_result("call-1", "new first done"),
            Message::tool_result("call-2", "new second done"),
        ];
        trim_session_messages(&mut messages, 5);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].tool_calls.as_ref().map(Vec::len), Some(2));
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(messages[3].tool_call_id.as_deref(), Some("call-2"));
    }

    #[test]
    fn context_windows_cover_known_model_families() {
        assert_eq!(context_window_tokens("openai/gpt-4o-mini"), 128_000);
        assert_eq!(context_window_tokens("anthropic/claude-sonnet-4"), 200_000);
        assert_eq!(context_window_tokens("google/gemini-2.5-pro"), 1_048_576);
        assert_eq!(context_window_tokens("qwen/qwen2.5-7b-instruct"), 32_768);
        assert_eq!(context_window_tokens("local-custom"), 32_768);
    }

    #[test]
    fn budget_shrinks_for_small_windows_and_stays_bounded() {
        assert_eq!(derived_history_budget(128_000), MAX_HISTORY_BYTES);
        assert_eq!(derived_history_budget(1_048_576), MAX_HISTORY_BYTES);
        assert_eq!(derived_history_budget(4_096), MIN_HISTORY_BYTES);

        let small = derived_history_budget(32_768);
        assert!(small < MAX_HISTORY_BYTES, "{small}");
        assert!(small > MIN_HISTORY_BYTES, "{small}");
    }

    #[test]
    fn compaction_clips_large_tool_output_and_keeps_result_ids() {
        let mut messages = vec![Message::system("system"), Message::user("long task")];
        for index in 0..40 {
            messages.push(Message::tool_result(
                format!("call-{index}"),
                "x".repeat(20_000),
            ));
        }
        let before = history_bytes(&messages);

        assert!(compact_history(&mut messages, 30_000));
        assert!(before > history_bytes(&messages));

        let results: Vec<_> = messages
            .iter()
            .filter(|message| message.role == "tool")
            .collect();
        assert_eq!(results.len(), 40, "every call keeps its result");
        for (index, result) in results.iter().enumerate() {
            let expected = format!("call-{index}");
            assert_eq!(result.tool_call_id.as_deref(), Some(expected.as_str()));
            assert!(result.text().len() <= COMPACT_TOOL_OUTPUT_BYTES + 64);
            assert!(result.text().contains("compacted"));
        }
    }

    #[test]
    fn compaction_drops_complete_turns_before_clipping_the_active_turn() {
        let mut messages = vec![Message::system("system")];
        for turn in 0..8 {
            messages.push(Message::user(format!("old task {turn}")));
            messages.push(Message::assistant("ok"));
        }
        messages.push(Message::user("active task"));
        messages.push(Message::tool_result("call-live", "y".repeat(20_000)));

        assert!(compact_history(&mut messages, 20_000));
        assert_eq!(messages[0].role, "system");
        assert!(messages
            .iter()
            .any(|message| message.text() == "active task"));
        assert!(
            !messages
                .iter()
                .any(|message| message.text() == "old task 0"),
            "the oldest complete turn is dropped first"
        );

        let live = messages
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("call-live"))
            .expect("the active result is kept");
        assert!(live.text().len() <= COMPACT_TOOL_OUTPUT_BYTES + 64);
    }

    #[test]
    fn compaction_is_a_no_op_within_budget() {
        let mut messages = vec![Message::user("hello"), Message::assistant("hi")];
        let before = messages.len();
        assert!(!compact_history(&mut messages, MAX_HISTORY_BYTES));
        assert_eq!(messages.len(), before);
    }

    #[test]
    fn oversized_single_turn_is_kept_protocol_complete() {
        let assistant: Message = serde_json::from_value(json!({"role":"assistant","content":null,"tool_calls":[{"id":"call-1","type":"function","function":{"name":"shell","arguments":"{}"}}]})).unwrap();
        let mut messages = vec![
            Message::user("long task"),
            assistant,
            Message::tool_result("call-1", "done"),
        ];
        trim_session_messages(&mut messages, 2);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].tool_calls.as_ref().map(Vec::len), Some(1));
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("call-1"));
    }
}
