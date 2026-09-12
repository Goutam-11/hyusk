use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Tool, ToolResult};

const MAX_ENTRIES: usize = 5_000;
const MAX_GRAPH: usize = 5_000;
const DEFAULT_LIMIT: usize = 8;

/// One remembered fact or note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: u64,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_unix: u64,
}

/// A knowledge-graph triple.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphTriple {
    pub subject: String,
    pub relation: String,
    pub object: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryStore {
    #[serde(default)]
    pub entries: Vec<MemoryEntry>,
    #[serde(default)]
    pub graph: Vec<GraphTriple>,
    #[serde(default)]
    pub next_id: u64,
}

pub fn memory_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|_| PathBuf::from("/tmp"));

    base.join("hyusk")
}

fn memory_json_path() -> PathBuf {
    memory_dir().join("memory.json")
}

fn memory_md_path() -> PathBuf {
    memory_dir().join("memory.md")
}

pub fn load_store() -> MemoryStore {
    let path = memory_json_path();

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(store) => store,

            Err(error) => {
                eprintln!("[Memory] Could not parse {}: {error}", path.display());
                MemoryStore::default()
            }
        },

        Err(_) => MemoryStore::default(),
    }
}

fn save_store(store: &MemoryStore) -> Result<()> {
    let directory = memory_dir();
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("Failed to create {}", directory.display()))?;

    let json = serde_json::to_string_pretty(store).context("Failed to serialize memory store")?;

    std::fs::write(memory_json_path(), json).context("Failed to write memory.json")?;
    std::fs::write(memory_md_path(), render_markdown(store))
        .context("Failed to write memory.md")?;

    Ok(())
}

fn render_markdown(store: &MemoryStore) -> String {
    let mut output = String::from("# Hyusk memory\n\n");

    output.push_str("## Notes\n\n");

    if store.entries.is_empty() {
        output.push_str("_No notes yet._\n");
    } else {
        for entry in &store.entries {
            let tags = if entry.tags.is_empty() {
                String::new()
            } else {
                format!(" `{}`", entry.tags.join("`, `"))
            };

            output.push_str(&format!("- [#{}]{} {}\n", entry.id, tags, entry.text));
        }
    }

    output.push_str("\n## Knowledge graph\n\n");

    if store.graph.is_empty() {
        output.push_str("_No triples yet._\n");
    } else {
        for triple in &store.graph {
            output.push_str(&format!(
                "- {} **{}** {}\n",
                triple.subject, triple.relation, triple.object
            ));
        }
    }

    output
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.len() >= 2)
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

/// Rank entries against a query with a small tf-idf style score.
///
/// Terms that are rare across the store weigh more, tag matches count extra,
/// and long notes are length-normalized so they cannot win just by being long.
/// This is deliberately local and dependency-free; it scales to the small
/// personal store this tool keeps.
fn rank_entries<'a>(
    entries: &'a [MemoryEntry],
    query_terms: &[String],
    limit: usize,
) -> Vec<(&'a MemoryEntry, f32)> {
    if entries.is_empty() || query_terms.is_empty() {
        return Vec::new();
    }

    let mut terms = query_terms.to_vec();
    terms.sort();
    terms.dedup();

    // Tokenize each entry once: note text and tags are searched separately so
    // a tag match can be weighted more heavily.
    let tokenized: Vec<(Vec<String>, Vec<String>)> = entries
        .iter()
        .map(|entry| {
            (
                tokenize(&entry.text),
                entry.tags.iter().flat_map(|tag| tokenize(tag)).collect(),
            )
        })
        .collect();

    let total = entries.len() as f32;
    let mut document_frequency: HashMap<&str, f32> = HashMap::new();

    for term in &terms {
        let mut count = 0.0f32;

        for (text, tags) in &tokenized {
            if text.contains(term) || tags.contains(term) {
                count += 1.0;
            }
        }

        document_frequency.insert(term.as_str(), count.max(1.0));
    }

    let mut scored: Vec<(&MemoryEntry, f32)> = Vec::new();

    for (entry, (text, tags)) in entries.iter().zip(tokenized.iter()) {
        let mut score = 0.0f32;

        for term in &terms {
            let in_text = text.iter().filter(|token| *token == term).count() as f32;
            let in_tags = tags.iter().filter(|token| *token == term).count() as f32;

            if in_text == 0.0 && in_tags == 0.0 {
                continue;
            }

            let idf = (total / document_frequency[term.as_str()]).ln_1p();
            score += idf * (1.0 + in_text + 2.0 * in_tags);
        }

        if score > 0.0 {
            let length = text.len().max(1) as f32;
            scored.push((entry, score / length.sqrt()));
        }
    }

    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    scored.truncate(limit);

    scored
}

/// Retrieval context injected into the system prompt for one user message.
///
/// Ranks stored notes against the message so the assistant sees the memories
/// that matter for *this* turn instead of an arbitrary "most recent" list. When
/// nothing matches, the most recent notes are used so there is still some
/// continuity. Stable facts from the knowledge graph are always included.
pub fn context_for(query: &str, limit: usize) -> String {
    let store = load_store();
    let mut output = String::new();

    let terms = tokenize(query);

    let mut chosen: Vec<&MemoryEntry> = rank_entries(&store.entries, &terms, limit)
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();

    if chosen.is_empty() {
        chosen = store.entries.iter().rev().take(limit).collect();
        chosen.reverse();
    }

    if !chosen.is_empty() {
        output.push_str("Relevant memories:\n");

        for entry in chosen {
            output.push_str(&format!("- [#{}] {}\n", entry.id, entry.text));
        }
    }

    let facts: Vec<&GraphTriple> = store.graph.iter().rev().take(limit).collect();

    if !facts.is_empty() {
        output.push_str("Known facts:\n");

        for triple in facts.into_iter().rev() {
            output.push_str(&format!(
                "- {} {} {}\n",
                triple.subject, triple.relation, triple.object
            ));
        }
    }

    output
}

pub struct MemoryTool {
    state: tokio::sync::Mutex<MemoryStore>,
}

impl MemoryTool {
    pub fn new() -> Self {
        Self {
            state: tokio::sync::Mutex::new(load_store()),
        }
    }
}

impl Default for MemoryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum MemoryAction {
    Remember {
        text: String,
        #[serde(default)]
        tags: Vec<String>,
    },

    Search {
        query: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },

    GraphAdd {
        subject: String,
        relation: String,
        object: String,
    },

    GraphQuery {
        #[serde(default)]
        subject: Option<String>,
        #[serde(default)]
        relation: Option<String>,
        #[serde(default)]
        object: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },

    List {
        #[serde(default = "default_limit")]
        limit: usize,
    },

    Forget {
        id: u64,
    },
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Persistent local memory. Use `remember` to save a note or preference, \
         `search` to find relevant notes, `graph_add`/`graph_query` for \
         subject-relation-object facts, `list` for recent notes, and `forget` \
         to delete a note by id. The store survives restarts; data is written \
         to Markdown and JSON under the user's data directory."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["remember", "search", "graph_add", "graph_query", "list", "forget"],
                    "description": "Memory operation."
                },
                "text": {
                    "type": "string",
                    "description": "Note text for `remember`."
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags for `remember`."
                },
                "query": {
                    "type": "string",
                    "description": "Search query for `search`."
                },
                "subject": { "type": "string", "description": "Graph subject." },
                "relation": { "type": "string", "description": "Graph relation." },
                "object": { "type": "string", "description": "Graph object." },
                "id": { "type": "integer", "description": "Entry id for `forget`." },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "description": "Maximum results. Defaults to 8."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let action: MemoryAction = serde_json::from_str(input)
            .context("Memory tool input must be JSON with an `action` field")?;

        let mut store = self.state.lock().await;

        match action {
            MemoryAction::Remember { text, tags } => {
                let text = text.trim().to_string();

                if text.is_empty() {
                    return Ok(ToolResult::failure("Memory text must not be empty."));
                }

                // Do not store the same note twice; repeated tool calls are
                // common when the model "remembers" a fact it already saved.
                if let Some(existing) = store
                    .entries
                    .iter()
                    .find(|entry| entry.text.eq_ignore_ascii_case(&text))
                {
                    return Ok(ToolResult::success(format!(
                        "Already remembered as #{}: {}",
                        existing.id, existing.text
                    )));
                }

                store.next_id += 1;

                let entry = MemoryEntry {
                    id: store.next_id,
                    text,
                    tags: tags
                        .into_iter()
                        .map(|tag| tag.trim().to_string())
                        .filter(|tag| !tag.is_empty())
                        .take(8)
                        .collect(),
                    created_unix: now_unix(),
                };

                store.entries.push(entry.clone());

                if store.entries.len() > MAX_ENTRIES {
                    let excess = store.entries.len() - MAX_ENTRIES;
                    store.entries.drain(0..excess);
                }

                save_store(&store)?;

                Ok(ToolResult::success(format!(
                    "Remembered as #{}: {}",
                    entry.id, entry.text
                )))
            }

            MemoryAction::Search { query, limit } => {
                let limit = limit.clamp(1, 50);
                let terms = tokenize(&query);

                let scored = rank_entries(&store.entries, &terms, limit);

                if scored.is_empty() {
                    return Ok(ToolResult::success(format!(
                        "No memories matched '{query}'."
                    )));
                }

                let mut lines = vec![format!("Memory matches for '{query}':")];

                for (entry, score) in scored {
                    lines.push(format!(
                        "- [#{}] (score {:.2}) {}",
                        entry.id, score, entry.text
                    ));
                }

                Ok(ToolResult::success(lines.join("\n")))
            }

            MemoryAction::GraphAdd {
                subject,
                relation,
                object,
            } => {
                let subject = subject.trim().to_string();
                let relation = relation.trim().to_string();
                let object = object.trim().to_string();

                if subject.is_empty() || relation.is_empty() || object.is_empty() {
                    return Ok(ToolResult::failure(
                        "Graph triples require non-empty subject, relation, and object.",
                    ));
                }

                let already_known = store.graph.iter().any(|triple| {
                    triple.subject.eq_ignore_ascii_case(&subject)
                        && triple.relation.eq_ignore_ascii_case(&relation)
                        && triple.object.eq_ignore_ascii_case(&object)
                });

                if already_known {
                    return Ok(ToolResult::success(format!(
                        "Already known: {subject} {relation} {object}"
                    )));
                }

                store.graph.push(GraphTriple {
                    subject: subject.clone(),
                    relation: relation.clone(),
                    object: object.clone(),
                });

                if store.graph.len() > MAX_GRAPH {
                    let excess = store.graph.len() - MAX_GRAPH;
                    store.graph.drain(0..excess);
                }

                save_store(&store)?;

                Ok(ToolResult::success(format!(
                    "Added fact: {subject} {relation} {object}"
                )))
            }

            MemoryAction::GraphQuery {
                subject,
                relation,
                object,
                limit,
            } => {
                let limit = limit.clamp(1, 50);

                let matches: Vec<&GraphTriple> = store
                    .graph
                    .iter()
                    .filter(|triple| {
                        subject
                            .as_ref()
                            .map(|value| {
                                triple
                                    .subject
                                    .to_lowercase()
                                    .contains(&value.to_lowercase())
                            })
                            .unwrap_or(true)
                            && relation
                                .as_ref()
                                .map(|value| {
                                    triple
                                        .relation
                                        .to_lowercase()
                                        .contains(&value.to_lowercase())
                                })
                                .unwrap_or(true)
                            && object
                                .as_ref()
                                .map(|value| {
                                    triple.object.to_lowercase().contains(&value.to_lowercase())
                                })
                                .unwrap_or(true)
                    })
                    .collect();

                if matches.is_empty() {
                    return Ok(ToolResult::success("No graph facts matched."));
                }

                let mut lines = vec![format!("Graph matches ({}):", matches.len())];

                for triple in matches.into_iter().take(limit) {
                    lines.push(format!(
                        "- {} {} {}",
                        triple.subject, triple.relation, triple.object
                    ));
                }

                Ok(ToolResult::success(lines.join("\n")))
            }

            MemoryAction::List { limit } => {
                let limit = limit.clamp(1, 50);
                let recent: Vec<&MemoryEntry> = store.entries.iter().rev().take(limit).collect();

                if recent.is_empty() {
                    return Ok(ToolResult::success("No memories stored yet."));
                }

                let mut lines = vec![format!("Recent memories ({}):", recent.len())];

                for entry in recent.into_iter().rev() {
                    lines.push(format!("- [#{}] {}", entry.id, entry.text));
                }

                Ok(ToolResult::success(lines.join("\n")))
            }

            MemoryAction::Forget { id } => {
                let before = store.entries.len();
                store.entries.retain(|entry| entry.id != id);

                if store.entries.len() == before {
                    return Ok(ToolResult::failure(format!("No memory with id #{id}.")));
                }

                save_store(&store)?;

                Ok(ToolResult::success(format!("Forgot memory #{id}.")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_and_scores() {
        let entries = vec![
            MemoryEntry {
                id: 1,
                text: "The user prefers dark mode".to_string(),
                tags: vec!["ui".to_string()],
                created_unix: 0,
            },
            MemoryEntry {
                id: 2,
                text: "unrelated fan speed".to_string(),
                tags: vec![],
                created_unix: 0,
            },
        ];

        let query = tokenize("user prefers dark mode");
        let ranked = rank_entries(&entries, &query, 5);

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].0.id, 1);
    }

    #[test]
    fn tags_boost_matches() {
        let entries = vec![
            MemoryEntry {
                id: 1,
                text: "the meeting moved".to_string(),
                tags: vec!["calendar".to_string()],
                created_unix: 0,
            },
            MemoryEntry {
                id: 2,
                text: "calendar notes".to_string(),
                tags: vec![],
                created_unix: 0,
            },
        ];

        let query = tokenize("calendar");
        let ranked = rank_entries(&entries, &query, 5);

        assert_eq!(ranked[0].0.id, 1);
    }

    #[test]
    fn renders_markdown() {
        let store = MemoryStore {
            entries: vec![MemoryEntry {
                id: 1,
                text: "hello".to_string(),
                tags: vec!["tag".to_string()],
                created_unix: 0,
            }],
            graph: vec![GraphTriple {
                subject: "a".to_string(),
                relation: "b".to_string(),
                object: "c".to_string(),
            }],
            next_id: 1,
        };

        let markdown = render_markdown(&store);

        assert!(markdown.contains("#1"));
        assert!(markdown.contains("a **b** c"));
    }
}
