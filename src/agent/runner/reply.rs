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
        if let Some(reply) = as_structured_reply(json) {
            return (reply.text, Some(reply.needs_reply));
        }
    }

    // The same duplication also arrives without a fence: prose, a blank line,
    // then the envelope. Leaving it in the reply would show the answer twice
    // and the raw JSON beside it on every card that displays it.
    for (index, _) in trimmed.match_indices('{') {
        if index == 0 {
            break;
        }
        if let Some(reply) = as_structured_reply(&trimmed[index..]) {
            return (reply.text, Some(reply.needs_reply));
        }
    }

    (raw.to_string(), None)
}

/// A parsed reply envelope, only when the JSON is exactly a terminal envelope:
/// a whole object carrying the explicit boolean flag and some text.
fn as_structured_reply(json: &str) -> Option<StructuredReply> {
    let value: Value = serde_json::from_str(json.trim()).ok()?;
    if !value.get("needs_reply").is_some_and(Value::is_boolean) {
        return None;
    }
    serde_json::from_value::<StructuredReply>(value)
        .ok()
        .filter(|reply| !reply.text.trim().is_empty())
}

/// Spoken approvals. Speech-to-text varies, so the grammar starts with the
/// leading phrase instead of requiring one exact sentence; the table stays
/// narrow because a confirmation authorizes exactly one stored tool call.
const AFFIRMATIVES: &[&str] = &[
    "yes",
    "yeah",
    "yep",
    "yup",
    "ok",
    "okay",
    "sure",
    "certainly",
    "absolutely",
    "definitely",
    "confirm",
    "confirmed",
    "proceed",
    "go ahead",
    "go for it",
    "do it",
    "do that",
    "please do",
    "please proceed",
    "sounds good",
    "of course",
    "affirmative",
    "approve",
    "agreed",
    "correct",
    "fine",
    "alright",
    "all right",
];

/// Leading phrases that refuse the pending action.
const NEGATIONS: &[&str] = &[
    "no",
    "nope",
    "nah",
    "nay",
    "dont",
    "do not",
    "cancel",
    "stop",
    "deny",
    "decline",
    "negative",
    "abort",
    "never",
    "skip",
    "not now",
    "leave it",
    "forget it",
    "no thanks",
];

/// A negation anywhere refuses the action: "yes, but not yet" is not an
/// approval, so denial wins over a leading affirmative.
const NEGATION_TOKENS: &[&str] = &[
    "no", "not", "never", "dont", "wont", "cant", "cannot", "cancel", "stop", "wait", "later",
    "instead", "deny", "nope", "nah", "without",
];

/// Lowercase, punctuation-free form for the confirmation grammar.
///
/// Apostrophes are removed before splitting, so "don't" becomes "dont" and is
/// classified instead of being read as "don t". Other punctuation becomes a
/// space, which also drops a trailing full stop or comma.
fn normalized_decision_phrase(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for character in text.chars() {
        if character == '\'' || character == '’' {
            continue;
        }
        if character.is_ascii_punctuation() {
            cleaned.push(' ');
        } else {
            cleaned.extend(character.to_lowercase());
        }
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn leads_with_phrase(normalized: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|phrase| {
        normalized == *phrase
            || normalized
                .strip_prefix(phrase)
                .is_some_and(|rest| rest.starts_with(' '))
    })
}

fn contains_negation(normalized: &str) -> bool {
    normalized
        .split_whitespace()
        .any(|word| NEGATION_TOKENS.contains(&word))
}

/// A spoken approval.
///
/// The leading phrase decides, so "okay, go ahead" and "sure, please do"
/// approve while "what did you delete?" cannot. A denial anywhere in the
/// utterance wins, and a question is never an approval: both keep the stored
/// call armed and the runtime re-asks instead of acting on an ambiguous reply.
pub(super) fn confirms(text: &str) -> bool {
    let normalized = normalized_decision_phrase(text);
    if normalized.is_empty() {
        return false;
    }
    if is_denial(text) || contains_negation(&normalized) || text.contains('?') {
        return false;
    }
    leads_with_phrase(&normalized, AFFIRMATIVES)
}

/// A refusal of the pending action, decided by the leading phrase.
///
/// "don't do that", "no thanks", and "not now" deny; "no, open Firefox" is a
/// denial followed by an instruction, which the caller handles as a denial and
/// a new request.
pub(super) fn is_denial(text: &str) -> bool {
    leads_with_phrase(&normalized_decision_phrase(text), NEGATIONS)
}

#[cfg(test)]
mod tests {
    use super::{
        confirms, is_denial, is_voice_session_stop, parse_structured_reply, tool_call_preamble,
    };

    /// The documented affirmative set, plus the exact sentences the old
    /// grammar accepted, which must keep working.
    #[test]
    fn confirmation_grammar_accepts_documented_affirmatives() {
        for phrase in [
            "yes",
            "yeah",
            "yep",
            "okay",
            "ok",
            "sure",
            "go ahead",
            "do it",
            "please do",
            "proceed",
            "confirm",
            // Real STT phrasing around those words.
            "Yes.",
            "OK!",
            "okay, go ahead.",
            "yeah, go ahead",
            "sure, please do",
            "yep, do it",
            "of course",
            "sounds good",
            // The retired exact matches stay approving.
            "yes confirm",
            "yes please",
            "yes go ahead",
            "okay confirm",
            "okay confirm go ahead",
            "please proceed",
        ] {
            assert!(confirms(phrase), "should approve: {phrase}");
        }
    }

    #[test]
    fn confirmation_grammar_rejects_denials() {
        for phrase in [
            "no",
            "nope",
            "don't",
            "do not",
            "cancel",
            "stop",
            "deny",
            "no thanks",
            // The case that used to normalise to "don t" and slip through.
            "don't do that",
            "don’t do that",
            "Don't do that.",
            "not now",
            "leave it",
            "forget it",
        ] {
            assert!(is_denial(phrase), "should deny: {phrase}");
            assert!(!confirms(phrase), "a denial must not approve: {phrase}");
        }
    }

    #[test]
    fn denial_wins_over_a_leading_approval() {
        // A refusal later in the utterance still refuses.
        for phrase in [
            "yes, but not yet",
            "yes, don't do that",
            "sure, no thanks",
            "okay, wait",
            "yeah, not that one",
        ] {
            assert!(!confirms(phrase), "must not approve: {phrase}");
        }
        // A denial followed by an instruction is still a denial; the caller
        // handles the instruction separately.
        for phrase in ["no, open Firefox", "don't do that, just tell me"] {
            assert!(is_denial(phrase), "should deny: {phrase}");
            assert!(!confirms(phrase), "must not approve: {phrase}");
        }
    }

    #[test]
    fn questions_and_courtesy_are_never_approvals() {
        for phrase in [
            "is it done?",
            "what did you delete?",
            "sure?",
            "okay, will it delete my files?",
            "yes, is that safe?",
        ] {
            assert!(!confirms(phrase), "a question must not approve: {phrase}");
        }
        // A bare courtesy phrase approves, but not when a question rides along.
        assert!(confirms("okay"));
        assert!(!confirms("okay, but what does it do?"));
        // A statement without a question mark is still not an approval.
        assert!(!confirms("is it done"));
        assert!(!confirms("what did you delete"));
    }

    #[test]
    fn stopping_listening_is_a_denial_and_still_ends_the_session() {
        assert!(is_voice_session_stop("stop listening"));
        assert!(is_denial("stop listening"));
        assert!(!confirms("stop listening"));
    }

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

    #[test]
    fn strips_an_unfenced_envelope_after_duplicate_prose() {
        let (text, needs_reply) = parse_structured_reply(
            "I've set a timer for 10 minutes.\n\n{\"text\": \"I've set a timer for 10 minutes.\", \"needs_reply\": false, \"reply_type\": \"none\"}",
        );

        assert_eq!(text, "I've set a timer for 10 minutes.");
        assert_eq!(needs_reply, Some(false));
    }

    #[test]
    fn keeps_prose_that_merely_mentions_the_flag() {
        let raw = "Set needs_reply to {\"needs_reply\": true} only for a question.";

        assert_eq!(parse_structured_reply(raw), (raw.to_string(), None));
    }
}
