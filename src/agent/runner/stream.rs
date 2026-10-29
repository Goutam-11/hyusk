//! Incremental decoding of streamed model replies into speakable sentences.
//!
//! The text agent answers with a structured envelope
//! (`{"text":"...","needs_reply":false}`) or, before a tool call, with plain
//! prose. This module recovers the speakable text from either form while it is
//! still arriving, so speech can start before the model finishes.
//!
//! [`decode`] is prefix-stable: a longer input always decodes to a string that
//! starts with the earlier result, apart from an incomplete trailing escape,
//! which is dropped. [`ReplyStream`] relies on that property to hand out each
//! new suffix exactly once and to flush complete sentences as they form.

const FENCE: &str = "```";
const ENVELOPE_KEY: &str = "\"text\"";
/// How much undecided `{...` input to tolerate before treating the reply as
/// plain prose. A real envelope reveals its key almost immediately.
const UNDECIDED_LIMIT: usize = 200;
/// Longest unpunctuated run spoken as one chunk, so a wall of text without
/// sentence breaks cannot delay the first words indefinitely.
const MAX_PENDING: usize = 400;

/// Speakable text decoded from the reply so far.
pub(super) fn decode(raw: &str) -> String {
    match classify(raw) {
        Shape::Pending => String::new(),
        Shape::Prose(text) => text.to_string(),
        Shape::Envelope { prose, body } => {
            let mut decoded = String::new();
            if !prose.trim().is_empty() {
                decoded.push_str(prose);
            }
            if let Some(text) = envelope_text(body) {
                decoded.push_str(echo_remainder(prose, &text));
            }
            decoded
        }
    }
}

/// The part of an envelope's answer that the prose has not said already.
///
/// Models routinely write the answer as prose and then echo it inside the
/// envelope, so appending it whole would speak and display the same sentence
/// twice. A text that is still only an echo adds nothing, and one that runs
/// past the prose adds just its remainder.
///
/// The result only ever grows as more envelope text arrives, which
/// [`ReplyStream`] relies on to hand out each new suffix exactly once.
fn echo_remainder<'a>(prose: &str, text: &'a str) -> &'a str {
    let prose = prose.trim();
    if prose.is_empty() {
        return text;
    }
    if prose.starts_with(text) {
        return "";
    }
    match text.strip_prefix(prose) {
        Some(remainder) => remainder.trim_start(),
        None => text,
    }
}

/// What the reply received so far looks like.
enum Shape<'a> {
    /// Nothing speakable yet; an envelope key may still arrive.
    Pending,
    /// Plain prose, spoken as written.
    Prose(&'a str),
    /// A spoken preamble followed by a structured envelope.
    Envelope { prose: &'a str, body: &'a str },
}

fn classify(raw: &str) -> Shape<'_> {
    let trimmed = raw.trim_start();
    if trimmed.is_empty() {
        return Shape::Pending;
    }

    if trimmed.starts_with('{') {
        if trimmed.contains(ENVELOPE_KEY) {
            let leading = raw.len() - trimmed.len();
            return Shape::Envelope {
                prose: &raw[..leading],
                body: &raw[leading..],
            };
        }
        return if trimmed.len() >= UNDECIDED_LIMIT {
            Shape::Prose(raw)
        } else {
            Shape::Pending
        };
    }

    if let Some(fence) = raw.find(FENCE) {
        let rest = &raw[fence + FENCE.len()..];
        match rest.find('\n') {
            Some(newline) => {
                let body_start = fence + FENCE.len() + newline + 1;
                let body = raw[body_start..].trim_start();
                if body.starts_with('{') || body.is_empty() {
                    return Shape::Envelope {
                        prose: &raw[..fence],
                        body: &raw[body_start..],
                    };
                }
                // A prose code block is not an envelope.
            }
            // The fence line itself is still arriving; hold speech.
            None => {
                return Shape::Envelope {
                    prose: &raw[..fence],
                    body: "",
                };
            }
        }
    }

    if let Some(marker) = raw.find("{\"text\"") {
        return Shape::Envelope {
            prose: &raw[..marker],
            body: &raw[marker..],
        };
    }

    // Hold back a trailing marker so a reply that turns out to be an envelope
    // does not change text that was already spoken.
    match raw.rfind('{') {
        Some(index) if raw[index + 1..].trim().is_empty() => {
            Shape::Prose(raw[..index].trim_end_matches('`'))
        }
        _ => Shape::Prose(raw.trim_end_matches('`')),
    }
}

/// Decode the `text` field of an envelope body that may still be incomplete.
fn envelope_text(envelope: &str) -> Option<String> {
    let key = envelope.find(ENVELOPE_KEY)?;
    let after_key = &envelope[key + ENVELOPE_KEY.len()..];
    let colon = after_key.find(':')?;
    let after_colon = &after_key[colon + 1..];
    let quote = after_colon.find('"')?;
    Some(decode_json_string(&after_colon[quote + 1..]))
}

/// Decode a JSON string body up to its closing quote.
///
/// An incomplete trailing escape is dropped rather than guessed, so the result
/// never changes once more input arrives.
fn decode_json_string(value: &str) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => break,
            '\\' => {
                let Some(escaped) = chars.next() else {
                    break;
                };
                match escaped {
                    'n' | 'r' | 't' => decoded.push(' '),
                    'u' => {
                        let mut hex = String::new();
                        for _ in 0..4 {
                            match chars.next() {
                                Some(digit) if digit.is_ascii_hexdigit() => hex.push(digit),
                                _ => break,
                            }
                        }
                        if hex.len() == 4 {
                            if let Some(character) =
                                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                            {
                                decoded.push(character);
                            }
                        }
                    }
                    other => decoded.push(other),
                }
            }
            other => decoded.push(other),
        }
    }
    decoded
}

/// The reply text delivered so far, and the text not yet spoken.
#[derive(Default)]
pub(super) struct ReplyStream {
    raw: String,
    decoded: String,
    pending: String,
    spoken: String,
}

impl ReplyStream {
    /// Feed one streamed delta, forwarding every sentence that is now complete.
    pub(super) fn push(&mut self, delta: &str, emit: &mut impl FnMut(&str)) {
        self.raw.push_str(delta);
        let decoded = decode(&self.raw);
        self.absorb(&decoded, emit);
    }

    /// Finish the stream with the assembled reply text.
    ///
    /// `final_text` is authoritative: it covers deltas that never reached
    /// [`Self::push`]. Anything already spoken is never repeated.
    pub(super) fn finish(&mut self, final_text: &str, emit: &mut impl FnMut(&str)) {
        let decoded = decode(final_text);
        self.absorb(&decoded, emit);
        let rest = self.pending.trim().to_string();
        self.pending.clear();
        if !rest.is_empty() {
            self.deliver(&rest, emit);
        }
    }

    /// Text already handed to the speech and UI sinks.
    pub(super) fn spoken(&self) -> &str {
        self.spoken.trim_end()
    }

    pub(super) fn delivered(&self) -> bool {
        !self.spoken.trim().is_empty()
    }

    fn absorb(&mut self, decoded: &str, emit: &mut impl FnMut(&str)) {
        if !decoded.starts_with(&self.decoded) {
            // The reply diverged from what was already spoken; keep it.
            return;
        }
        if decoded.len() > self.decoded.len() {
            self.pending.push_str(&decoded[self.decoded.len()..]);
            self.decoded = decoded.to_string();
        }
        while let Some(sentence) = take_sentence(&mut self.pending) {
            self.deliver(&sentence, emit);
        }
        while self.pending.len() > MAX_PENDING {
            // Prefer a word boundary so speech does not split a word in half.
            let mut cut = self.pending[..MAX_PENDING]
                .rfind(' ')
                .filter(|cut| *cut > 0)
                .unwrap_or(MAX_PENDING);
            while !self.pending.is_char_boundary(cut) {
                cut -= 1;
            }
            let chunk = self.pending[..cut].to_string();
            self.pending.drain(..cut);
            self.deliver(&chunk, emit);
        }
    }

    fn deliver(&mut self, chunk: &str, emit: &mut impl FnMut(&str)) {
        let chunk = chunk.trim();
        if chunk.is_empty() || carries_protocol_text(chunk) {
            return;
        }
        self.spoken.push_str(chunk);
        self.spoken.push(' ');
        emit(chunk);
    }
}

/// Whether a chunk is the model narrating its own reply protocol.
///
/// A model that ignores the envelope and thinks out loud ends up describing the
/// schema ("I should return JSON with needs_reply…"). Speaking that is worse
/// than silence: the microphone picks the agent's own voice up as a new
/// command, so the narration feeds straight back into another turn. These
/// fragments never occur in a real spoken answer, so dropping them is safe.
fn carries_protocol_text(chunk: &str) -> bool {
    const ARTIFACTS: &[&str] = &[
        "needs_reply",
        "needsreply",
        "reply_type",
        "replytype",
        "response protocol",
        "final json",
        "{\"text\"",
    ];
    let lowered = chunk.to_ascii_lowercase();
    ARTIFACTS.iter().any(|artifact| lowered.contains(artifact))
}

/// Remove the first complete sentence from `buffer`.
///
/// A terminator only ends a sentence when whitespace follows it, so decimals
/// and version numbers stay intact.
fn take_sentence(buffer: &mut String) -> Option<String> {
    loop {
        let mut split = None;
        for (index, character) in buffer.char_indices() {
            if !matches!(character, '.' | '!' | '?' | '\n') {
                continue;
            }
            let end = index + character.len_utf8();
            let next = buffer[end..].chars().next();
            if character == '\n' || next.is_some_and(char::is_whitespace) {
                split = Some(end);
                break;
            }
        }
        let end = split?;
        let sentence = buffer[..end].trim().to_string();
        buffer.drain(..end);
        if !sentence.is_empty() {
            return Some(sentence);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, ReplyStream};

    fn collect(stream: &mut ReplyStream, deltas: &[&str], final_text: &str) -> Vec<String> {
        let mut spoken = Vec::new();
        for delta in deltas {
            stream.push(delta, &mut |chunk| spoken.push(chunk.to_string()));
        }
        stream.finish(final_text, &mut |chunk| spoken.push(chunk.to_string()));
        spoken
    }

    #[test]
    fn prose_is_spoken_as_sentences_before_the_stream_finishes() {
        let mut stream = ReplyStream::default();
        let mut spoken = Vec::new();
        stream.push("I'll open Firefox", &mut |chunk| {
            spoken.push(chunk.to_string())
        });
        assert!(spoken.is_empty(), "no sentence yet");
        stream.push(". Then I'll search.", &mut |chunk| {
            spoken.push(chunk.to_string())
        });
        assert_eq!(spoken, vec!["I'll open Firefox."]);
    }

    #[test]
    fn envelope_text_streams_without_speaking_json() {
        let mut stream = ReplyStream::default();
        let mut spoken = Vec::new();
        stream.push("{\"text\":\"Drafted the", &mut |chunk| {
            spoken.push(chunk.to_string())
        });
        stream.push(" reply. Sending it", &mut |chunk| {
            spoken.push(chunk.to_string())
        });
        assert_eq!(spoken, vec!["Drafted the reply."]);

        stream.finish(
            "{\"text\":\"Drafted the reply. Sending it now.\",\"needs_reply\":false}",
            &mut |chunk| spoken.push(chunk.to_string()),
        );
        assert_eq!(spoken, vec!["Drafted the reply.", "Sending it now."]);
        assert_eq!(stream.spoken(), "Drafted the reply. Sending it now.");
    }

    #[test]
    fn partial_envelope_holds_text_until_its_key_arrives() {
        let mut stream = ReplyStream::default();
        let mut spoken = Vec::new();
        stream.push("{\"te", &mut |chunk| spoken.push(chunk.to_string()));
        assert!(spoken.is_empty());
        assert!(stream.spoken().is_empty());

        stream.push("xt\":\"Hi.\"}", &mut |chunk| spoken.push(chunk.to_string()));
        stream.finish("{\"text\":\"Hi.\"}", &mut |chunk| {
            spoken.push(chunk.to_string())
        });
        assert_eq!(spoken, vec!["Hi."]);
    }

    #[test]
    fn escapes_are_decoded_and_never_split() {
        assert_eq!(
            decode("{\"text\":\"say \\\"hi\\\" now\"}"),
            "say \"hi\" now"
        );
        assert_eq!(decode("{\"text\":\"line\\nbreak\"}"), "line break");
        assert_eq!(decode("{\"text\":\"caf\\u00e9\"}"), "café");
        // An incomplete escape decodes to nothing and waits for its digits.
        assert_eq!(decode("{\"text\":\"caf\\u00"), "caf");
        assert_eq!(decode("{\"text\":\"caf\\u00e9\"}"), "café");
    }

    #[test]
    fn fenced_envelope_keeps_the_spoken_preamble() {
        assert_eq!(
            decode("Done.\n```json\n{\"text\":\"All set.\",\"needs_reply\":false}\n```"),
            "Done.\nAll set."
        );
    }

    #[test]
    fn plain_reply_is_flushed_at_finish() {
        let mut stream = ReplyStream::default();
        let spoken = collect(&mut stream, &["All done"], "All done");
        assert_eq!(spoken, vec!["All done"]);
    }

    #[test]
    fn unfinished_envelope_speaks_nothing() {
        let mut stream = ReplyStream::default();
        let spoken = collect(&mut stream, &["{\"text\":\""], "{\"text\":\"");
        assert!(spoken.is_empty());
        assert!(!stream.delivered());
    }

    #[test]
    fn long_unpunctuated_text_is_chunked() {
        let mut stream = ReplyStream::default();
        let text = "word ".repeat(200);
        let spoken = collect(&mut stream, &[&text], &text);
        assert!(spoken.len() > 1);
        assert!(spoken.iter().all(|chunk| chunk.len() <= 400));
    }

    #[test]
    fn narrated_protocol_is_never_spoken() {
        /*
         * A model that ignores the envelope narrates the schema instead. That
         * narration used to be synthesized, and the microphone then heard the
         * agent's own voice as the next command.
         */
        let narration = "Per the response protocol I should return JSON with \
                         text, needs_reply and reply_type. Final JSON coming up:";
        let mut stream = ReplyStream::default();
        let spoken = collect(&mut stream, &[narration], narration);
        assert!(spoken.is_empty(), "narration was spoken: {spoken:?}");
        assert!(!stream.delivered());
    }

    #[test]
    fn an_echoed_envelope_is_spoken_once() {
        let mut stream = ReplyStream::default();
        let spoken = collect(
            &mut stream,
            &["Timer set.\n\n{\"text\": \"Timer set."],
            "Timer set.\n\n{\"text\": \"Timer set.\", \"needs_reply\": false}",
        );

        assert_eq!(spoken, vec!["Timer set."]);
        assert_eq!(stream.spoken(), "Timer set.");
    }

    #[test]
    fn an_envelope_that_extends_the_prose_speaks_the_remainder() {
        let mut stream = ReplyStream::default();
        let spoken = collect(
            &mut stream,
            &["Timer set.\n\n{\"text\": \"Timer set. Anything else?"],
            "Timer set.\n\n{\"text\": \"Timer set. Anything else?\", \"needs_reply\": true}",
        );

        assert_eq!(spoken, vec!["Timer set.", "Anything else?"]);
    }

    #[test]
    fn a_normal_envelope_still_speaks_after_the_guard() {
        let mut stream = ReplyStream::default();
        let text = "{\"text\":\"The timer is set. Anything else?\",\"needs_reply\":true}";
        let spoken = collect(&mut stream, &[text], text);
        assert_eq!(spoken, vec!["The timer is set.", "Anything else?"]);
    }
}
