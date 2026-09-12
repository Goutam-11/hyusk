use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::{
    model::openrouter::OpenRouterClient,
    tools::{ToolRegistry, ToolResult},
    types::{HyuskEvent, HyuskState, Message},
};

pub struct Agent {
    client: OpenRouterClient,
    model: String,
    tools: ToolRegistry,
    tool_specs: Vec<Value>,
    tool_guide: String,
    messages: Vec<Message>,
    vision: bool,
}

impl Agent {
    pub fn new(client: OpenRouterClient, model: String, tools: ToolRegistry) -> Self {
        let tool_specs = build_tool_specs(&tools);
        let tool_guide = build_tool_guide(&tools);
        let system_prompt = build_system_prompt(&tool_guide, "");
        let vision = std::env::var("MODEL_VISION")
            .map(|value| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(true);

        Self {
            client,
            model,
            tools,
            tool_specs,
            tool_guide,
            messages: vec![Message::system(system_prompt)],
            vision,
        }
    }

    /// Run one turn for `user_input`.
    ///
    /// Cancelling `cancel` aborts the model request or tool call in progress
    /// and rolls the conversation back to the state before this turn, so a
    /// replacement request never sees half-finished tool messages.
    ///
    /// Returns `Ok(None)` when the turn was cancelled.
    pub async fn handle(
        &mut self,
        user_input: String,
        ui_tx: Option<&Sender<HyuskEvent>>,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        /*
         * The system prompt contains the wall-clock time and retrieved
         * memories; both were frozen at process start. Rebuild it every turn
         * -- and retrieve memories relevant to this message -- so a
         * long-running session never acts on a stale date or misses something
         * the user told us earlier.
         */
        self.messages[0] = Message::system(build_system_prompt(&self.tool_guide, &user_input));

        let turn_start = self.messages.len();

        self.messages.push(Message::user(user_input));

        loop {
            let response = match tokio::select! {
                _ = cancel.cancelled() => None,
                response = self.client.chat(
                    &self.model,
                    &self.messages,
                    &self.tool_specs,
                ) => Some(response),
            } {
                None => {
                    self.messages.truncate(turn_start);
                    return Ok(None);
                }

                Some(Ok(response)) => response,

                Some(Err(error)) => {
                    self.messages.truncate(turn_start);
                    return Err(error);
                }
            };

            if let Some(calls) = response.tool_calls.clone() {
                if !calls.is_empty() {
                    self.messages.push(response.clone());

                    for call in calls {
                        if cancel.is_cancelled() {
                            self.messages.truncate(turn_start);
                            return Ok(None);
                        }

                        let name = call.function.name.clone();

                        let args_value: Value =
                            serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);

                        let args_str = call.function.arguments.clone();

                        println!("\n[Hyusk] Running {}: {}\n", name, args_value);

                        if let Some(tx) = ui_tx {
                            HyuskState::Working.publish();

                            // try_send: the UI channel must never block the
                            // agent while it holds the agent lock (in
                            // indicator mode nothing consumes this channel).
                            let _ = tx.try_send(HyuskEvent::StateChanged(HyuskState::Working));

                            let _ = tx.try_send(HyuskEvent::ToolStarted { name: name.clone() });
                        }

                        let result = if let Some(tool) = self.tools.get(&name) {
                            match tokio::select! {
                                _ = cancel.cancelled() => None,
                                result = tool.execute(&args_str) => Some(result),
                            } {
                                None => {
                                    self.messages.truncate(turn_start);
                                    return Ok(None);
                                }

                                Some(Ok(result)) => result,

                                Some(Err(error)) => {
                                    ToolResult::failure(format!("Tool execution error: {}", error))
                                }
                            }
                        } else {
                            ToolResult::failure(format!("Tool '{}' does not exist.", name))
                        };

                        let success = result.success;

                        let (tool_output, image_path) = if self.vision && name == "computer" {
                            split_image_marker(&result.output)
                        } else {
                            (result.output.clone(), None)
                        };

                        let tool_result = ToolResult {
                            success,
                            output: tool_output,
                            error: result.error.clone(),
                        };

                        self.messages.push(Message::tool_result(
                            call.id.clone(),
                            tool_result.as_agent_message(),
                        ));

                        if let Some(image_path) = image_path {
                            match load_image_data_url(&image_path).await {
                                Some(data_url) => {
                                    self.messages.push(Message::user_with_image(
                                        format!("Screenshot at {image_path}"),
                                        data_url,
                                    ));
                                }

                                None => {
                                    eprintln!("[Agent] Could not read screenshot {image_path}")
                                }
                            }
                        }

                        if let Some(tx) = ui_tx {
                            let _ = tx.try_send(HyuskEvent::ToolFinished { name, success });
                        }
                    }

                    continue;
                }
            }

            self.messages.push(response.clone());

            return Ok(Some(response.text()));
        }
    }
}

fn build_tool_specs(tools: &ToolRegistry) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name(),
                    "description": tool.description(),
                    "parameters": tool.input_schema(),
                }
            })
        })
        .collect()
}

/// Names and one-line purposes of every registered tool, for the prompt.
fn build_tool_guide(tools: &ToolRegistry) -> String {
    let mut tools: Vec<_> = tools.iter().collect();

    tools.sort_by_key(|tool| tool.name().to_string());

    let mut guide = String::new();

    for tool in tools {
        let description = tool.description();
        let summary = description.split(". ").next().unwrap_or(description).trim();

        guide.push_str(&format!("- {}: {}\n", tool.name(), summary));
    }

    guide
}

fn build_system_prompt(tool_guide: &str, query: &str) -> String {
    let mut prompt = r#"
You are Hyusk, a warm, quick-witted local AI assistant that lives on the
user's computer. You are talking with the user out loud through a
text-to-speech voice, so everything you write is heard, not read.

How you speak:
- Write plain, natural prose. Never use Markdown: no bold or italic markers,
  no headings, no bullet or numbered lists, no tables, no code fences, and no
  emoji. A speech engine reads those out literally and sounds broken.
- Keep replies short and conversational: usually one to three sentences.
  Go longer only when the user asks for detail.
- Be friendly and a little playful. Light humour and personality are welcome;
  rambling is not.
- Write numbers, symbols, and units the way a person says them, for example
  "twenty percent" instead of "20%".
- Never narrate stage directions or sound effects (no *laughs*, no [pause]).
- When you act on the computer, say what you did in one short, natural
  sentence instead of naming functions. Never read tool names, JSON, or raw
  tool output aloud.

Working with tools:
- Tools perform real actions on the user's computer. When the user asks you to
  do something, do it with a tool instead of explaining how.
- Work step by step: call one tool, read its result, then decide the next
  call. Chain tools for multi-step tasks (launch an app, find a control, click
  it). Do not announce a plan longer than the work itself.
- Prefer the fastest, most reliable path on this Linux desktop:
  1. `accessibility` for native apps -- list windows/apps, read the screen as
     text, find and act on elements. It needs no screenshots.
  2. `computer` for what accessibility cannot do (pixel-precise drag, scroll,
     hover) and for keyboard shortcuts via `key_combo`.
  3. `computer screenshot` for vision-capable models when accessibility gives
     nothing.
  4. `computer ocr` / `find_text` / `click_text` for text-only models.
- If a tool fails, read the error and try a different approach or tool. Do not
  repeat a failing call unchanged. Never claim something worked unless the
  tool result says it did.
- Before anything destructive or hard to undo -- deleting, overwriting,
  sending messages, purchases, quitting an app with unsaved work -- ask the
  user to confirm in one short spoken sentence first.

On Linux, drive the desktop natively with `accessibility` first:
1. `accessibility active` (optionally `read: true`) to see the focused app,
   window, and element plus the visible text. This is the fastest way to
   orient yourself before acting.
2. `accessibility windows` for every open window (with active state and
   titles), or `accessibility apps` for running applications (each lists its
   window titles and whether it is active).
3. `accessibility read` with the app index to read an app's screen as text
   (the native replacement for a screenshot plus OCR).
4. `accessibility find` (match by `name`, `role`, and/or `text`, optionally
   scoped with `app`) to locate a control, then act on the returned path:
   `click` (optionally with `action_name`), `focus`, or `set_text` /
   `get_text`.
Blind spots: apps that do not publish an accessibility tree (Chromium and
Electron apps unless `ACCESSIBILITY_ENABLED=1`, or any app while
toolkit-accessibility is off) return no nodes and a `warnings` field. When that
happens, or for canvas and pixel-only UIs, fall back to `computer` screenshot
with vision (or `find_text`/`click_text` for text-only models). Never conclude
an app is not open from accessibility alone -- take a screenshot to be sure.
Launch applications with the `process` tool (or `shell`) and control playback
with `media`.

Memory:
- You have a persistent memory that survives restarts. Relevant memories are
  provided below automatically each turn; you can also search with the
  `memory` tool. Search before assuming something about the user.
- Save durable things about the user proactively, without being asked: their
  name, preferences, routines, people, ongoing projects, decisions, and
  corrections they make. Use short, self-contained sentences via `remember`.
- Use `graph_add` for stable facts as subject-relation-object triples (for
  example: user / prefers / dark mode) and `graph_query` to look them up.
- Do not store transient chatter, one-off commands, or secrets. If the user
  asks you to forget something, use `forget`.

Available tools:
"#
    .to_string();

    prompt.push_str(tool_guide);

    prompt.push_str(
        "\nIf the user sends a new message while you are still working, treat it \
         as a replacement request and answer the newest message.\n",
    );

    prompt.push_str("\nSystem context:\n");
    prompt.push_str(&crate::system_info::system_context());

    prompt.push_str("\n\nRemembered context (retrieved for this message):\n");

    let memory = crate::tools::memory::context_for(query, 12);

    if memory.trim().is_empty() {
        prompt.push_str("(no memories stored yet)\n");
    } else {
        prompt.push_str(&memory);
    }

    prompt
}

/// Extract `HYUSK_IMAGE:<path>` marker lines from a tool output.
///
/// Returns the cleaned text and the last image path found.
fn split_image_marker(output: &str) -> (String, Option<String>) {
    const PREFIX: &str = "HYUSK_IMAGE:";

    let mut cleaned = Vec::new();
    let mut path = None;

    for line in output.lines() {
        if let Some(rest) = line.trim_start().strip_prefix(PREFIX) {
            path = Some(rest.trim().to_string());
        } else {
            cleaned.push(line);
        }
    }

    (cleaned.join("\n"), path)
}

async fn load_image_data_url(path: &str) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let encoded = STANDARD.encode(bytes);

    Some(format!("data:image/png;base64,{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::split_image_marker;

    #[test]
    fn splits_image_marker_from_output() {
        let (text, path) = split_image_marker("line one\nHYUSK_IMAGE:/tmp/a.png\nline two");

        assert_eq!(path.as_deref(), Some("/tmp/a.png"));
        assert!(text.contains("line one"));
        assert!(text.contains("line two"));
        assert!(!text.contains("HYUSK_IMAGE"));
    }
}
