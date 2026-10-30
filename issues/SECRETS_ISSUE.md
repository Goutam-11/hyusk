# Secrets and privacy issues

Tool output, screenshots, and fetched pages are sent to a remote provider.
Anything the agent can read, the provider can see; anything it writes to a
world-readable file can be read by another local process. These issues cover the
files and values that must not travel by accident.

## X1 — `.env` permissions

- Status: done (tooling) · 2026-10-29 · commit `921ff8a` · Phase 1 · Priority: high
- Owner: user applies the file-mode change in Phase 1; the installer, doctor,
  and docs work is the implementable part
- Depends on: —
- Evidence: `.env` on disk, `AGENTS.md` safety invariants

**Problem.** `.env` is mode `0644` and holds a live provider key. Any local
account can read it. `AGENTS.md` already says it should be `0600`, and the file
is git-ignored, so this is only a permissions and tooling gap.

**Decision (2026-10-29).** The user applies `chmod 600 .env` in Phase 1 himself;
this issue records that step and owns the surrounding tooling. The installer and
`install-autostart.sh` enforce `0600` when creating or touching `.env`. The doctor
warns when the mode is wider than `0600`. Document the requirement in
`README.md` setup.

**Scope.** In: file mode (user-applied), installer check, doctor warning, docs.
Out: moving keys to the keyring (already the primary path) and key rotation.

**Acceptance criteria**

- [ ] `.env` is `0600` on this machine (applied by the user in Phase 1). Still
      `0644` as of this commit, by the decision that assigns the change to the
      user. Run `chmod 600 .env` in the checkout; `gnome_doctor` now reports
      `needs_attention` until then.
- [x] The installer fixes a wider mode and reports it:
      `scripts/install-autostart.sh` chmods an existing `.env` to `600` and
      prints the previous mode. `scripts/setup-mobile-link.sh` already writes
      `.env` through a `0600` temporary file before moving it into place.
- [x] A doctor/check command flags `0644`: `gnome_doctor` gained an
      `.env permissions` check that passes at `0600`, warns at any group/other
      bit with the current mode, and stays quiet when the file is absent.
- [x] `README.md` and `docs/development.md` say `0600`, with the reason and the
      automatic enforcement.

**Tests**

- [x] Doctor warning unit test with a temporary file
      (`gnome_doctor::tests::env_file_mode_is_flagged_when_other_accounts_can_read_it`
      covers `0644` → warn with a `chmod 600` next step, `0600` → pass, and a
      missing file → info).
- [ ] Installer script check: the repository has no shell test harness (the
      `tests/` directory holds Python bridge/key checks), so this stays a
      documented manual check: run `install-autostart.sh` against a `0644`
      `.env` and confirm it prints the restriction and leaves mode `600`.

**Files.** `.env` (local, not committed), `scripts/install-autostart.sh`,
`src/tools/gnome_doctor.rs`, `README.md`, `docs/development.md`,
`docs/tools.md`.

**Notes (2026-10-29, implemented).** The doctor check is read-only and lives
with the other readiness checks because that is the command the user already
runs when something looks wrong; its name is `.env permissions` and its next
step is the exact command. The installer enforces the mode before it builds, so
a fresh install cannot leave the key readable. The mode change on this machine
is deliberately left to the user, as the decision records.

## X2 — Redact secret-shaped output

- Status: done · 2026-10-29 · commit `7af7130` · Phase 2 · Priority: medium
- Depends on: S2
- Evidence: `src/tools/shell.rs` (output path), `src/agent/runner/session.rs`

**Problem.** S2 stops the obvious files, but secrets also arrive from
environment dumps, logs, `git diff`, and web pages. Tool output is copied into
the session and the provider request; a token printed once stays in the
conversation forever.

**Decision (2026-10-29).** Apply a bounded redaction pass to tool output before
it becomes a tool result: private-key blocks, common token prefixes, bearer
headers, and long high-entropy assignments. Redact with a marker that keeps the
line shape, and record how many values were replaced so the model knows. Keep
the raw value out of session and logs.

**Scope.** In: pattern list, pass, marker, tests. Out: perfect secret
detection, which is not achievable.

**Acceptance criteria**

- [x] A printed `OPENROUTER_API_KEY=…` becomes `OPENROUTER_API_KEY=[redacted]`.
- [x] A PEM block is removed entirely (one `[redacted private key]` line
      replaces it, including when the block is unterminated).
- [x] The session file and journal do not contain the original value. The
      journal stores tool arguments, not output, so only redacted output can
      reach it; the session is covered by a save-and-read round-trip test.
- [x] Redaction is documented in `docs/tools.md` (security model).

**Tests**

- [x] Pattern table with positive and negative cases
      (`tools::redact::tests`: assignments in env/JSON/URL forms, token
      prefixes, bearer values, PEM blocks, ordinary text that must stay).
- [x] Session round-trip test
      (`a_redacted_tool_result_round_trips_without_the_secret`), plus the
      execution choke point
      (`tool_output_is_redacted_before_it_becomes_a_result`).

**Files.** `src/tools/redact.rs` (new), `src/tools/mod.rs`,
`src/agent/runner.rs`, `src/agent/runner/session.rs`, `docs/tools.md`,
`AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-29, implemented).** `tools::redact::redact` walks the output
once: it removes `-----BEGIN … PRIVATE KEY-----` blocks up to their matching
`-----END …-----` marker, replaces credential token prefixes (OpenRouter/OpenAI,
GitHub, Slack, AWS, Google, Hugging Face, GitLab, npm, PyPI, SendGrid,
DigitalOcean, Shopify, JWTs) when the remainder is long enough that an ordinary
word cannot match, replaces the value after a `Bearer` scheme, and replaces
values of 12 or more characters assigned to a secret-shaped name. The last rule
reads `=`, `:`, and quoted JSON keys, so env dumps, `"api_secret": "…"`, and
`?access_token=…` are all covered, while `TOKEN_COUNT=5`, `key=true`, and a
`monkey` stay readable. The line shape is preserved and one note
(`[Hyusk redacted N secret-shaped value(s) from this output]`) is appended so
the model knows the output was edited.

The pass runs once in `execute_tool`, which is the choke point for the text
runner, the Nova Sonic loop, and the instant command lane. Everything
downstream - the tool result message, the session save, the status card text,
the spoken summary in the fast lane, and the provider request - therefore sees
the redacted text. The journal stores tool arguments rather than output, so it
cannot carry a redacted value by itself.

Over-redaction is deliberate and documented: `[redacted]` inside a value,
`PASSWORD_POLICY="must be 12 chars"`, and a long value under a `key` field are
all possible false positives, and a false redaction costs one confused line
whereas a missed secret is permanent. The pass cannot find an unrecognised
shape, which the docs say plainly.

Drift: the Evidence named `shell.rs` as the output path, but the single place
every lane shares is `runner::execute_tool`; redacting there is what makes the
session, the provider request, and the fast lane consistent. The session file
is a sink, not the source, and is covered by its own round-trip test.

## X3 — Private runtime files

- Status: done · 2026-10-29 · commit `39b4fbc` · Phase 3 · Priority: medium
- Depends on: —
- Evidence: `src/speech/speech.rs:251-257` (`next_piper_output_path`),
  `src/tools/computer.rs:380-455` (`screenshot_path` and
  `cleanup_screenshots`), `src/model/codex.rs:62-70` (the temporary
  `--output-last-message` file)

**Problem.** TTS WAV files, screenshots, and Codex output land in the shared
temporary directory with predictable names and default modes. Another local
process can read them; screenshots can contain private windows.

**Decision (2026-10-29).** Use a per-user runtime directory
(`$XDG_RUNTIME_DIR/hyusk/` or the platform equivalent), mode `0700`, files
created with `0600` and unique names, and the existing cleanup policy applied
there. Fall back to the temporary directory only when the runtime directory is
unavailable.

**Scope.** In: path helper, creation mode, cleanup, tests. Out: encryption at
rest.

**Acceptance criteria**

- [x] New files live under the private directory with `0600`.
- [x] Directory mode is `0700`.
- [x] Cleanup works in the new location.
- [x] A test asserts mode and path.

**Tests**

- [x] Helper unit tests with a temporary runtime directory
      (`paths::tests`: `runtime_directory_is_a_private_child_of_the_runtime_base`,
      `private_files_are_owner_only_and_unique`,
      `restrict_file_tightens_a_world_readable_file`,
      `generated_paths_keep_their_prefix_and_extension`), plus the call-site
      tests `speech::speech::tests::piper_output_paths_are_unique_and_owner_only`,
      `tools::computer::tests::screenshot_placeholders_are_owner_only_and_private`,
      `tools::computer::tests::cleanup_keeps_the_newest_screenshots_and_drops_expired_ones`,
      and `model::codex::tests::temporary_output_is_owner_only_and_removed_on_drop`.

**Files.** `src/paths.rs` (new), `src/main.rs` (`mod paths`),
`src/speech/speech.rs`, `src/tools/computer.rs`, `src/model/codex.rs`,
`docs/tools.md`, `docs/architecture.md`, `AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-29, implemented).** `src/paths.rs` is the single helper.
`runtime_dir` returns `$XDG_RUNTIME_DIR/hyusk`, creating it with `0700` and
tightening a wider mode on every call; `private_dir` returns it, or the shared
temporary directory when the variable is unset or the directory cannot be
created. `private_file(prefix, extension)` creates a unique empty file with
`0600` there (`create_new`, so a name collision can never truncate an existing
file) and falls back to the temporary directory with the same owner-only mode.
`private_file_or_path` is the best-effort form for the two callers that cannot
report an error, and `restrict_file` re-applies `0600` to a file an external
program replaced.

The three sinks now use it: `next_piper_output_path` (Piper writes its WAV
there, and `TemporaryAudioFile` still removes it on drop), `screenshot_path`
(the placeholder is created `0600` before the backend writes to it, and
`capture_screenshot` re-applies `0600` after a backend succeeds because the
portal copy and a GNOME Shell write can replace the file with the default
mode), and the Codex `--output-last-message` file. A failed capture removes its
own empty placeholder instead of leaving it for retention.

`cleanup_screenshots` keeps the existing retention and cap policy in
`cleanup_screenshots_in(directory, retention, max_files)`; it runs on the
private directory and, when that differs, on the shared temporary directory, so
screenshots written by an earlier version are still removed. The indicator files
(`hyusk-state`, `hyusk-status.json`, `hyusk-control.json`,
`hyusk-pairing.json`) deliberately stay directly in `$XDG_RUNTIME_DIR`: the GNOME
and Noctalia panels read that protocol, and moving it would break them.

Drift: the Evidence range `computer.rs:386-397` mixed the end of
`screenshot_path` (then 380-390) with the start of `cleanup_screenshots` (then
396-455); `codex.rs:67-68` is the start of a construction that spans 62-70
(`stamp`/`sequence` were only there for the name). Both are recorded with the
ranges above. `speech.rs:256` was exact. The other `temp_dir` uses in the tree
are test scratch directories (`memory.rs`, `scheduler.rs`, `safety.rs`,
`gnome_doctor.rs`, `link/auth.rs`, `task.rs`) and stay as they are.

Verification: fail-before-fix probes are recorded in the session log — pointing
`private_file` at the shared temporary directory failed the piper, screenshot,
and Codex path assertions; dropping `mode(0o600)` failed the mode assertions;
dropping `restrict_file` failed the tightening test.

## X4 — Leakage audit of logs, status, and memory

- Status: done · 2026-10-30 · commit `X4PENDING` · Phase 3 · Priority: low
- Depends on: X2
- Evidence: `src/status.rs` (`card`, `save`, `path`),
  `src/agent/runner/session.rs` (`save_session_to_path`,
  `sanitize_session_messages`), `src/tools/memory.rs` (`memory_dir`, `run_db`,
  the `remember` action), `src/agent/runner.rs` (journal creation and the
  `[Hyusk] Running …` line)

**Problem.** Several sinks can retain text the user did not intend to keep: the
journal, the session file, status cards (visible in the panel and on screen),
and the memory database. None has been audited for keys, paths, or personal
content after the redaction pass.

**Decision (2026-10-29).** Audit each sink, then decide per sink: redact,
truncate, or exclude. Record the decision in the issue and in
`docs/development.md`. Check the memory database file mode as part of the audit.

**Scope.** In: audit and fixes for the four sinks. Out: changing the session
schema (that is A3/A6).

**Acceptance criteria**

- [x] No sink stores a value that X2 would redact.
- [x] Status cards never show a URL with a token or a full path to a secret.
- [x] The memory database is `0600` and inside a private directory.
- [x] The audit result is written down.

**Tests**

- [x] Round-trip checks per sink with a synthetic secret:
      the session file
      (`agent::runner::session::tests::the_saved_session_redacts_call_arguments_and_journal_values`),
      the journal
      (`agent::runner::tests::journal_arguments_lose_a_secret_before_the_entry_is_stored`),
      status cards (`status::tests::the_stored_card_and_its_json_lose_the_secret`
      and `card_text_loses_secret_values_and_credential_paths`), the memory
      database
      (`tools::memory::tests::the_database_is_private_and_a_secret_never_round_trips`),
      and the new URL-userinfo rule
      (`tools::redact::tests::url_userinfo_passwords_are_redacted`).

**Files.** `src/tools/redact.rs`, `src/tools/safety.rs`, `src/status.rs`,
`src/agent/runner.rs`, `src/agent/runner/session.rs`, `src/tools/memory.rs`,
`docs/development.md`, `docs/tools.md`, `docs/architecture.md`, `AGENTS.md`,
`CHANGELOG.md`.

**Notes (2026-10-30, implemented).** Audit result, sink by sink:

- **Journal** (`action_journal`, persisted in the session snapshot). It stores
  tool *arguments*, which never pass through X2's output choke point, so a
  credential in `curl -H 'Authorization: Bearer …'` or a `browser fill` was
  written verbatim and reused by later prompts. Now `journal_arguments` redacts
  at each of the three creation sites (fast lane, approved call, model tool
  round), and the save path redacts the entries again so a journal loaded from
  an older file is cleaned.
- **Session file.** Tool results were already redacted (X2), but assistant
  tool-call arguments and the user's own text were not. `redact_for_storage`
  runs on the snapshot after sanitisation and redacts message text (including
  multimodal text parts), tool-call arguments, and refusals. Only the on-disk
  copy is edited: the live conversation keeps the user's words, and a resumed
  turn never reloads a credential from the file. The file is still written
  through a `0600` temporary file and renamed, so the schema is unchanged.
- **Status card.** Card text is scrubbed in `status::card` before it reaches
  the store, the file, and the panel subscribers. Two passes: the X2 value pass,
  and the new `safety::redact_secret_paths`, because a credential *path* is not
  a secret-shaped value (the denylist is the same table S2 uses). The pass is
  silent; a count note belongs on tool output, not on a card the user is
  reading. `hyusk-status.json` is also chmodded `0600`, because when
  `XDG_RUNTIME_DIR` is unset the fallback is `/tmp`.
- **Memory database.** The directory was created with the default mode and the
  database with SQLite's. `run_db` now creates the directory with `0700`
  (`paths::ensure_private_dir`) and the database is `0600`
  (`paths::restrict_file`) right after it opens; `memory.md` is restricted the
  same way. Remembered notes and graph triples go through `stored_text`, the
  same bounded pass, so the database and the markdown export never keep a
  credential-shaped value.
- **Logs** (the issue title names them). `[Hyusk] Running <tool>: <arguments>`
  and the fast-lane line printed arguments; they now print the redacted form.
  `[User] …` keeps the user's own words, which are already in the session.

Decisions taken: over-redaction is preferred to leaking, and the pass stays
bounded, so an unrecognised shape can still get through - the docs say so. A
credential *path* is handled separately from a *value*, because the value pass
would not match `/home/me/.ssh/id_ed25519`. The memory `remember` action does not
refuse a secret; it stores the redacted text, so the user's note keeps its shape
and loses only the credential. Reviewed and left alone: `hyusk-state` (a state
word), `hyusk-control.json` (the agent scrubs the key payload as soon as it
reads it, and the file lives in a `0700` directory), `hyusk-pairing.json`
(already `0600`), and the legacy `memory.json` (read once by the migration, then
unused; it is the user's file and the audit does not rewrite it).

Discovery beyond the issue: the URL-userinfo password (`https://user:secret@host`)
was not covered by any X2 rule, which the "URL with a token" criterion asks for;
`redact_line` now redacts it, and the rule applies to tool output too.

Drift: the Evidence ranges were module-level references, so they were replaced
with the current function names rather than line numbers.

Verification: fail-before-fix probes are recorded in the session log - storing
the card unscrubbed, skipping the snapshot pass, dropping the directory and file
modes, making `journal_arguments` an identity function, and removing the
userinfo rule each failed its test.
