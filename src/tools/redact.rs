//! Bounded redaction of secret-shaped values in tool output (X2).
//!
//! Tool output is copied into the session, sent to the remote provider, and
//! shown in status text, so a token printed once stays in the conversation
//! forever. This pass replaces the shapes that are cheap to recognise - private
//! key blocks, common token prefixes, bearer credentials, and long values
//! assigned to secret-shaped names - with [`MARKER`], keeping the line shape
//! and counting the replacements so the model knows the output was edited.
//!
//! It is deliberately bounded: no pass can find every secret. Over-redaction is
//! preferred to leaking a value, and the raw value is never written to the
//! result, the session, or the journal.

/// The marker that replaces a secret value.
pub const MARKER: &str = "[redacted]";

/// The marker that replaces a private-key block; naming the kind tells the
/// model what was removed without keeping any key material.
pub const PRIVATE_KEY_MARKER: &str = "[redacted private key]";

/// Values shorter than this are not replaced by the assignment rule, so a
/// `TOKEN_COUNT=5` or a `key=true` flag stays readable.
const MIN_ASSIGNED_VALUE: usize = 12;

/// Token prefixes that only appear on credentials. The remainder must look like
/// a value, so an ordinary word that happens to start the same way is kept.
const TOKEN_PREFIXES: &[&str] = &[
    "sk-or-v1-",
    "sk-proj-",
    "sk-ant-",
    "sk_live_",
    "rk_live_",
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxr-",
    "AKIA",
    "ASIA",
    "AIza",
    "hf_",
    "glpat-",
    "npm_",
    "pypi-",
    "SG.",
    "dop_v1_",
    "shpat_",
    "eyJ",
];

/// Name segments that mark an assignment as a secret.
const SECRET_NAME_SEGMENTS: &[&str] = &[
    "apikey",
    "auth",
    "authorization",
    "cookie",
    "credential",
    "credentials",
    "key",
    "pass",
    "password",
    "passwd",
    "pat",
    "pwd",
    "secret",
    "session",
    "token",
];

/// Name suffixes that mark a camelCase assignment as a secret.
const SECRET_NAME_SUFFIXES: &[&str] = &["secret", "password", "token", "apikey", "credential"];

/// Redact secret-shaped values from tool output.
///
/// Returns the edited text and how many values were replaced. A `0` count means
/// the text is byte-identical to the input.
pub fn redact(input: &str) -> (String, usize) {
    if input.is_empty() {
        return (String::new(), 0);
    }
    let mut replaced = 0usize;
    let mut lines: Vec<String> = Vec::new();
    let mut in_key_block: Option<String> = None;

    for line in input.split('\n') {
        if let Some(end) = in_key_block.as_deref() {
            if line.contains(end) {
                in_key_block = None;
            }
            continue;
        }
        if let Some(end) = private_key_end(line) {
            // The whole block goes; only the marker is kept, so no key material
            // survives in the session or the provider request.
            lines.push(PRIVATE_KEY_MARKER.to_string());
            replaced += 1;
            in_key_block = Some(end);
            continue;
        }
        lines.push(redact_line(line, &mut replaced));
    }

    (lines.join("\n"), replaced)
}

/// The one-line note appended to a result whose output was edited.
pub fn note(replaced: usize) -> String {
    format!("\n[Hyusk redacted {replaced} secret-shaped value(s) from this output]")
}

/// The `-----END …-----` marker that closes a private-key block, when the line
/// opens one.
fn private_key_end(line: &str) -> Option<String> {
    let begin = line.find("-----BEGIN ")?;
    let rest = &line[begin + "-----BEGIN ".len()..];
    let label = rest.split("-----").next()?;
    if !label.contains("PRIVATE KEY") {
        return None;
    }
    Some(format!("-----END {label}-----"))
}

fn redact_line(line: &str, replaced: &mut usize) -> String {
    let line = redact_assignments(line, replaced);
    let line = redact_tokens(&line, replaced);
    redact_url_userinfo(&line, replaced)
}

/// Replace the password half of a URL's userinfo, as in
/// `https://oauth2:token@example.test/repo`: a credential in a URL is visible
/// wherever the URL is shown or stored.
fn redact_url_userinfo(line: &str, replaced: &mut usize) -> String {
    let mut output = String::with_capacity(line.len());
    let mut remaining = line;

    while let Some(position) = remaining.find("://") {
        let after_scheme = position + 3;
        output.push_str(&remaining[..after_scheme]);
        let authority = &remaining[after_scheme..];
        let end = authority
            .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace())
            .unwrap_or(authority.len());
        let (authority, tail) = authority.split_at(end);

        match authority.split_once('@') {
            Some((userinfo, host))
                if userinfo
                    .split_once(':')
                    .is_some_and(|(_, password)| !password.is_empty()) =>
            {
                let (user, _password) = userinfo.split_once(':').expect("checked above");
                output.push_str(user);
                output.push(':');
                output.push_str(MARKER);
                output.push('@');
                output.push_str(host);
                *replaced += 1;
            }
            _ => output.push_str(authority),
        }

        remaining = tail;
    }

    output.push_str(remaining);
    output
}

/// Replace a long value assigned to a secret-shaped name, for example
/// `OPENROUTER_API_KEY=…`, `"api_key": "…"`, or `?access_token=…`.
fn redact_assignments(line: &str, replaced: &mut usize) -> String {
    let bytes = line.as_bytes();
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        let separator = bytes[index];
        if separator != b'=' && separator != b':' {
            index += 1;
            continue;
        }

        // A quoted key (`"api_key": …`) ends just before the quote.
        let mut name_end = index;
        if name_end > 0 && (bytes[name_end - 1] == b'"' || bytes[name_end - 1] == b'\'') {
            name_end -= 1;
        }
        let mut start = name_end;
        while start > 0 && is_name_byte(bytes[start - 1]) {
            start -= 1;
        }
        let name = &line[start..name_end];

        let mut value_start = index + 1;
        while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
            value_start += 1;
        }
        let quoted = value_start < bytes.len()
            && (bytes[value_start] == b'"' || bytes[value_start] == b'\'');
        let value_begin = if quoted { value_start + 1 } else { value_start };
        let mut value_end = value_begin;
        while value_end < bytes.len() && !is_value_end(bytes[value_end]) {
            value_end += 1;
        }
        let value = &line[value_begin..value_end];

        if is_secret_name(name) && value.len() >= MIN_ASSIGNED_VALUE && !value.starts_with('$') {
            output.push_str(&line[cursor..value_begin]);
            output.push_str(MARKER);
            cursor = value_end;
            *replaced += 1;
        }
        index = value_end.max(index + 1);
    }

    output.push_str(&line[cursor..]);
    output
}

/// Replace standalone credential tokens and the value after a `Bearer` scheme.
fn redact_tokens(line: &str, replaced: &mut usize) -> String {
    let mut output = String::with_capacity(line.len());
    let mut token = String::new();
    let mut awaiting_bearer_value = false;

    let flush =
        |token: &mut String, output: &mut String, replaced: &mut usize, awaiting: &mut bool| {
            if token.is_empty() {
                return;
            }
            let redact = *awaiting && token.len() >= 8 || has_secret_prefix(token);
            if redact {
                output.push_str(MARKER);
                *replaced += 1;
            } else {
                output.push_str(token);
            }
            *awaiting = token.eq_ignore_ascii_case("bearer");
            token.clear();
        };

    for character in line.chars() {
        if is_token_delimiter(character) {
            flush(
                &mut token,
                &mut output,
                replaced,
                &mut awaiting_bearer_value,
            );
            output.push(character);
        } else {
            token.push(character);
        }
    }
    flush(
        &mut token,
        &mut output,
        replaced,
        &mut awaiting_bearer_value,
    );

    output
}

fn has_secret_prefix(token: &str) -> bool {
    TOKEN_PREFIXES.iter().any(|prefix| {
        let required = if *prefix == "sk-" { 12 } else { 8 };
        token.starts_with(prefix) && token.len() >= prefix.len() + required
    })
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

fn is_value_end(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b',' | b';' | b'"' | b'\'' | b')' | b']' | b'}' | b'<' | b'>' | b'&' | b'`'
        )
}

fn is_token_delimiter(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ',' | ';'
                | '"'
                | '\''
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | '&'
                | '`'
                | '\\'
                // `=` splits a rendered tool call (`command=API_KEY=sk-…`) so a
                // credential after it is still seen as its own token, even
                // though the outer name is not secret-shaped.
                | '='
        )
}

/// True when an assignment name looks like it holds a credential.
fn is_secret_name(name: &str) -> bool {
    let name = name.trim_matches(|character: char| !character.is_ascii_alphanumeric());
    if name.is_empty() {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    if lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .any(|segment| SECRET_NAME_SEGMENTS.contains(&segment))
    {
        return true;
    }
    SECRET_NAME_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use super::{note, redact, MARKER, PRIVATE_KEY_MARKER};

    #[test]
    fn assignments_with_secret_names_are_redacted_in_place() {
        // A rendered call wraps the command in `command=…`, so the outer
        // assignment is not secret-shaped; `=` still splits the inner token.
        let (text, count) =
            redact("shell command=OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef done");
        assert_eq!(count, 1);
        assert!(
            !text.contains("sk-or-v1-0123456789abcdef"),
            "a token after an inner `=` is redacted: {text}"
        );
        assert!(text.contains("[redacted]"), "{text}");

        let (text, count) = redact("OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef\nnormal=value");
        assert_eq!(count, 1);
        assert_eq!(text, format!("OPENROUTER_API_KEY={MARKER}\nnormal=value"));
        // JSON and URL forms keep the line shape too.
        let (text, count) = redact(
            r#"{"client_secret": "averylongsecretvalue1234"} and ?access_token=abcdef1234567890"#,
        );
        assert_eq!(count, 2, "{text}");
        assert!(!text.contains("averylongsecretvalue1234"), "{text}");
        assert!(!text.contains("abcdef1234567890"), "{text}");
        assert!(text.contains(MARKER));
    }

    #[test]
    fn url_userinfo_passwords_are_redacted() {
        let (text, count) =
            redact("clone https://oauth2:ghp_0123456789abcdef@github.com/hyusk/hyusk.git now");
        assert_eq!(count, 1, "{text}");
        assert_eq!(
            text,
            format!("clone https://oauth2:{MARKER}@github.com/hyusk/hyusk.git now")
        );

        // A URL without userinfo, and one with only a user, stay readable.
        for text in [
            "https://example.com/repo.git",
            "https://user@example.com/path",
        ] {
            let (output, count) = redact(text);
            assert_eq!(count, 0, "{text} became {output}");
            assert_eq!(output, text);
        }
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for text in [
            "ls: cannot access 'src/nope': No such file or directory",
            "TOKEN_COUNT=5",
            "key=true",
            "the monkey ate a banana",
            r#"{"key": "ctrl+enter"}"#,
            "note: see the documentation",
            "https://example.com/search?q=weather",
            "PORT=8080",
        ] {
            let (output, count) = redact(text);
            assert_eq!(count, 0, "{text} was edited to {output}");
            assert_eq!(output, text);
        }
    }

    #[test]
    fn token_prefixes_and_bearer_values_are_redacted() {
        let (text, count) = redact("token ghp_0123456789abcdef published");
        assert_eq!(count, 1);
        assert_eq!(text, format!("token {MARKER} published"));

        let (text, count) = redact("Authorization: Bearer abcdefghijklmnop");
        assert_eq!(count, 1, "{text}");
        assert!(!text.contains("abcdefghijklmnop"));
        assert!(text.contains("Bearer"), "{text}");

        // A word that only starts like a prefix stays.
        let (text, count) = redact("sk-learn is a library");
        assert_eq!(count, 0, "{text}");
    }

    #[test]
    fn private_key_blocks_are_removed_entirely() {
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
        let (text, count) = redact(&format!("before\n{key}\nafter"));
        assert_eq!(count, 1);
        assert_eq!(text, format!("before\n{PRIVATE_KEY_MARKER}\nafter"));
        assert!(!text.contains("b3BlbnNzaC1rZXktdjEAAAAA"));

        // An unterminated block is still removed.
        let (text, count) = redact("-----BEGIN RSA PRIVATE KEY-----\nMIIEow");
        assert_eq!(count, 1);
        assert_eq!(text, PRIVATE_KEY_MARKER);
    }

    #[test]
    fn the_note_counts_the_replacements() {
        assert_eq!(
            note(2),
            "\n[Hyusk redacted 2 secret-shaped value(s) from this output]"
        );
    }
}
