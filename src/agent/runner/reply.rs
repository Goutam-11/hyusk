//! Voice reply envelopes, spoken preambles, and exact conversation controls.

use super::session::truncate_bytes;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct StructuredReply {
    text: String,
    #[serde(default)]
    needs_reply: bool,
    #[serde(default)]
    #[allow(dead_code)]
    reply_type: Option<String>,
}

pub(super) fn tool_call_preamble(raw: &str) -> Option<String> {
    // Provider control tokens are not spoken content. Some models append
    // DSML/tool-call tags even when also returning real structured tool calls.
    let raw = ["<｜DSML｜", "<|tool_call", "<tool_call"]
        .iter()
        .filter_map(|marker| raw.find(marker))
        .min()
        .map_or(raw, |end| &raw[..end]);
    let raw = raw.trim();
    if raw.is_empty()
        || raw.starts_with('{')
        || raw.starts_with("```json")
        || parse_structured_reply(raw).1.is_some()
    {
        return None;
    }

    Some(truncate_bytes(raw, 280))
}

fn normalized_spoken_phrase(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_punctuation() {
                ' '
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn is_voice_session_stop(text: &str) -> bool {
    let normalized = normalized_spoken_phrase(text);
    matches!(
        normalized.as_str(),
        "go away" | "go away hyusk" | "hyusk go away" | "stop listening"
    )
}

pub(super) fn parse_structured_reply(raw: &str) -> (String, Option<bool>) {
    let candidate = raw
        .trim()
        .strip_prefix("```json")
        .and_then(|text| text.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or_else(|| raw.trim());

    if let Some(reply) = serde_json::from_str::<StructuredReply>(candidate)
        .ok()
        .filter(|reply| !reply.text.trim().is_empty())
    {
        return (reply.text, Some(reply.needs_reply));
    }

    // Some providers duplicate the natural answer before a fenced envelope.
    // Decode only a terminal response envelope with an explicit boolean flag,
    // not arbitrary JSON examples embedded in an otherwise ordinary answer.
    let trimmed = raw.trim();
    if let Some((_, fenced)) = trimmed
        .strip_suffix("```")
        .and_then(|before_close| before_close.rsplit_once("```"))
    {
        let json = fenced
            .trim()
            .strip_prefix("json")
            .unwrap_or(fenced.trim())
            .trim();
        if let Some(reply) = serde_json::from_str::<Value>(json)
            .ok()
            .filter(|value| value.get("needs_reply").is_some_and(Value::is_boolean))
            .and_then(|value| serde_json::from_value::<StructuredReply>(value).ok())
            .filter(|reply| !reply.text.trim().is_empty())
        {
            return (reply.text, Some(reply.needs_reply));
        }
    }
    (raw.to_string(), None)
}

/// Confirmation is intentionally narrow: a confirmation authorizes exactly
/// one matching tool call, never a later or different action.
pub(super) fn confirms(text: &str) -> bool {
    matches!(
        normalized_spoken_phrase(text).as_str(),
        "confirm"
            | "yes"
            | "yes confirm"
            | "yes please"
            | "go ahead"
            | "yes go ahead"
            | "okay confirm"
            | "okay confirm go ahead"
            | "please proceed"
    )
}

pub(super) fn is_denial(text: &str) -> bool {
    matches!(
        normalized_spoken_phrase(text).as_str(),
        "no" | "no thanks" | "don t" | "do not" | "cancel" | "deny" | "stop"
    )
}

#[cfg(test)]
mod tests {
    use super::tool_call_preamble;

    #[test]
    fn preamble_drops_provider_control_tokens() {
        assert_eq!(tool_call_preamble("\n<｜DSML｜function_calls"), None);
        assert_eq!(
            tool_call_preamble("I’ll check it.\n<｜DSML｜function_calls"),
            Some("I’ll check it.".into())
        );
    }

    #[test]
    fn preamble_does_not_speak_duplicate_response_json() {
        assert_eq!(
            tool_call_preamble("Done.\n```json\n{\"text\":\"Done.\",\"needs_reply\":false}\n```"),
            None
        );
    }
}
