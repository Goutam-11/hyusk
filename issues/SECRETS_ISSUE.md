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

- Status: open · Phase 2 · Priority: medium
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

- [ ] A printed `OPENROUTER_API_KEY=…` becomes `OPENROUTER_API_KEY=[redacted]`.
- [ ] A PEM block is removed entirely.
- [ ] The session file and journal do not contain the original value.
- [ ] Redaction is documented in `docs/tools.md`.

**Tests**

- [ ] Pattern table with positive and negative cases.
- [ ] Session round-trip test.

**Files.** a small `src/tools/redact.rs` or helper in `safety.rs`,
`src/agent/runner.rs`, `src/agent/runner/session.rs`.

## X3 — Private runtime files

- Status: open · Phase 3 · Priority: medium
- Depends on: —
- Evidence: `src/speech/speech.rs:256`, `src/tools/computer.rs:386-397`,
  `src/model/codex.rs:67-68`

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

- [ ] New files live under the private directory with `0600`.
- [ ] Directory mode is `0700`.
- [ ] Cleanup works in the new location.
- [ ] A test asserts mode and path.

**Tests**

- [ ] Helper unit tests with a temporary `XDG_RUNTIME_DIR`.

**Files.** `src/speech/speech.rs`, `src/tools/computer.rs`,
`src/model/codex.rs`, a shared `paths` helper.

## X4 — Leakage audit of logs, status, and memory

- Status: open · Phase 3 · Priority: low
- Depends on: X2
- Evidence: `src/status.rs`, `src/agent/runner/session.rs`,
  `src/tools/memory.rs`

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

- [ ] No sink stores a value that X2 would redact.
- [ ] Status cards never show a URL with a token or a full path to a secret.
- [ ] The memory database is `0600` and inside a private directory.
- [ ] The audit result is written down.

**Tests**

- [ ] Round-trip checks per sink with a synthetic secret.

**Files.** `src/status.rs`, `src/agent/runner/session.rs`,
`src/tools/memory.rs`, docs.
