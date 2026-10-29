# Documentation issues

Tracked documentation that does **not** match the current code. Created
2026-10-29 at commit `4e28322` (`hyusk_agent` is now a standalone Cargo
workspace). Read
[`DSH_SESSION_NOTES.md`](../memory/DSH_SESSION_NOTES.md) first for build,
environment, and tooling context.

Status: **open** — only D7 (mobile `pairing` secret) needs a decision. All
docs-only issues (D1, D4, D5, D6, D8) are resolved, and D2/D3 are fixed in
code; build/tests are green.

## Verified baseline

| Check | Result |
| --- | --- |
| `cargo check --all-targets --offline` | 0 errors, 0 warnings |
| `cargo test --all-targets --offline -- --test-threads=4` | 283 passed, 0 failed, 7 ignored |
| `cargo clippy --all-targets --offline` | 0 errors; 3 pre-existing `result_large_err` warnings in `src/link/server.rs` |
| `cargo fmt --all -- --check` | clean |

Live microphone/STT/TTS, portal input, GNOME/niri window control, live CDP,
and real provider calls were **not** exercised. See
`memory/DSH_SESSION_NOTES.md` for the isolation commands used to run the tests
without touching real user data.

## Decisions needed first

One issue remains and involves behaviour, not just prose. Decide before editing
docs: either change the code to match the docs or correct the docs to match the
code.

| ID | Decision | Recommended |
| --- | --- | --- |
| D2 | Add runtime approval for `task` (especially `workspace`) **or** soften the docs | **Resolved 2026-10-29:** approval arm added for both profiles |
| D3 | Make wake defaults `false` in code **or** change docs to `true` | **Resolved 2026-10-29:** code now defaults to `false` |
| D7 | Remove the `mobile pairing` tool action **or** document and gate it | Remove it from the model-facing schema; keep the pairing payload for the indicators |

D1 has a code option too (stop restarting the service from the GNOME menu), but
a docs-only correction is acceptable.

---

## D1 — API-key flow claims are false (P1) — resolved 2026-10-29

**Status:** resolved docs-only. The three statements now describe each client:
the GNOME menu writes the key with `secret-tool` and restarts
`hyusk.service`; the Noctalia panel queues the key in `hyusk-control.json`, and
the agent scrubs the file as soon as it reads the key, stores it in the
keyring, and refreshes the catalog. The
absolute "never stored in the runtime control file" wording is replaced by "not
persisted", and the Noctalia handoff is described as transient. Two more files
repeated the same claim and were corrected:
[`docs/gnome-indicator.md`](../docs/gnome-indicator.md#L38-L41) (said "no
service restart") and [`docs/tools.md`](../docs/tools.md#L528-L531).

**Files:** [`README.md`](../README.md#L158), [`README.md`](../README.md#L411-L419),
[`CHANGELOG.md`](../CHANGELOG.md) (Unreleased "Added"), [`docs/noctalia-indicator.md`](../docs/noctalia-indicator.md#L85-L90)

**Claims:**

- Both GNOME and Noctalia save keys "and apply without a restart".
- "The agent writes it to the login keyring … without restarting the service".
- "the key is never stored in `commands.json`, the runtime control file, or the
  session snapshot."

**Reality:**

- GNOME's API-key form stores via `secret-tool` and then runs
  `systemctl --user restart hyusk.service`
  ([`credentials.js:91`](../gnome-extension/hyusk@hyusk.local/lib/menu/credentials.js#L91)).
  The agent does not write the key in that path.
- The Noctalia panel writes the plaintext key into `hyusk-control.json`
  ([`settings.luau:1067`](../noctalia-plugin/settings.luau#L1067)); the agent's
  `set_api_key` consumer stores it in the keyring and immediately overwrites
  the file ([`main.rs:987-L1026`](../src/main.rs#L987-L1026)).
- [`docs/noctalia-indicator.md`](../docs/noctalia-indicator.md#L85-L90) already
  documents the transient control-file handoff, so README/CHANGELOG contradict
  the project's own docs.

**Fix applied (docs-only):** each client is now described accurately. Examples:

- GNOME: key goes to the login keyring and the service restarts.
- Noctalia: key is queued in `hyusk-control.json`, scrubbed as soon as the
  agent consumes it, stored in the keyring, applied without a restart.
- Remove the absolute "never stored in the runtime control file" wording;
  qualify it as "not persisted" instead.

**Optional code fix:** make the GNOME form send `set_api_key` through the
control file too, so both clients behave identically without a restart.

**Done when:** no claim says the key never touches the control file, and the
restart/no-restart behavior is stated per client.

---

## D2 — `task` confirmation claim has no implementation (P1) — resolved 2026-10-29

**Status:** resolved in code (recommended path). `tools::safety::requires_approval`
now arms a one-shot confirmation for `action: "delegate"` on both profiles, the
runner and Sonic paths store the canonicalised workspace via
`tools::safety::approval_arguments`, and a regression test covers workspace,
research, list, cancel, and the pinned root. Verified with fmt, check, clippy
(the same 3 pre-existing warnings), and the full suite (283 passed, 7 ignored).

**Files:** [`README.md`](../README.md#L244-L250), [`docs/tools.md`](../docs/tools.md#L444-L448)

**Claim:** starting either `research` or `workspace` requires "explicit user
confirmation", and the workspace is "one explicitly approved project root".

**Reality:**

- [`safety.rs:12-L22`](../src/tools/safety.rs#L12-L22) has no `"task"` arm, so
  delegation never arms an approval.
- [`task.rs:126-L315`](../src/tools/task.rs#L126-L315) starts a worker
  immediately.
- [`codex.rs:46-L54`](../src/model/codex.rs#L46-L54) accepts any existing
  directory the model passes and runs Codex with `--approve-for-me` in
  `workspace-write` mode ([`codex.rs:73-L90`](../src/model/codex.rs#L73-L90)).
  A model can therefore edit files anywhere it can name.

**Fix applied:** `task_requires_approval` arms a one-shot confirmation for
`action: "delegate"` on both profiles, and `approval_arguments` rewrites the
stored call with the canonical workspace path. The runner's generic
pending-approval path executes the stored call on "yes", and the Sonic path does
the same. A regression test covers `workspace`, `research`, `list`, `cancel`,
and the pinned root.

**Alternative (not taken):** correct the docs to say no runtime approval is
required and the model chooses the workspace root. That would remove a safety
property the rest of the docs advertise.

**Done:** the tool call is confirmed before a worker starts, and the docs
statement is true.

---

## D3 — Wake defaults differ from the documented defaults (P1/P2) — resolved 2026-10-29

**Status:** resolved in code (recommended path). `WAKE_CLAP_ENABLED` and
`WAKE_WORD_DENOISE` now default to `false` in normal startup, `WAKE_TEST` matches
those defaults, the `WakeWordDetector` constructor default is `false`, and the
README configuration table lists `WAKE_WORD_STEP_MS`. `.env.example` is
unchanged. Verified with `cargo fmt`, `cargo check --all-targets --offline`,
`cargo clippy` (only the 3 pre-existing `result_large_err` warnings), and
`cargo test --all-targets --offline` (283 passed, 7 ignored).

**Files:** [`README.md`](../README.md#L137-L150),
[`docs/voice-and-ui.md`](../docs/voice-and-ui.md#L69),
[`src/main.rs`](../src/main.rs#L1179), [`src/main.rs`](../src/main.rs#L1241),
[`src/wake/detector.rs`](../src/wake/detector.rs#L206)

**Claims:** `WAKE_CLAP_ENABLED` defaults to `0`; `WAKE_WORD_DENOISE` defaults to
`0`.

**Reality:**

- Clap wake: [`main.rs:1179`](../src/main.rs#L1179) uses
  `.unwrap_or(true)`, so clap detection is **on** when the variable is unset.
- Denoise: [`main.rs:1241`](../src/main.rs#L1241) and
  [`detector.rs:206`](../src/wake/detector.rs#L206) default to `true`.
- `WAKE_TEST` mode is inconsistent with normal startup: denoise defaults true
  but clap defaults false ([`main.rs:357-L375`](../src/main.rs#L357-L375)).
- `.env.example` sets both to `0`, which masks the mismatch for users who copy
  it; a user who runs the binary without `.env` gets clap + denoise enabled.
- `WAKE_WORD_STEP_MS` is documented in `voice-and-ui.md` but missing from the
  README configuration table.

**Fix applied:** both `unwrap_or(true)` defaults are now `false` in normal
startup, the `WAKE_TEST` path matches, the detector constructor default is
`false`, and the README table documents `WAKE_WORD_STEP_MS`. `.env.example` is
unchanged.

**Done:** running without `.env` behaves as documented, and both docs tables
list every supported wake setting.

---

## D4 — `voice-and-ui.md` contains stale implementation details (P2) — resolved 2026-10-29

**Status:** resolved. The flow diagram now shows `transcribe_from_microphone(8.0)`
and `speak_cancellable`; the removed `speak_async` sentence was deleted; the
fixed-recording-window limitation was replaced with the real onset/8-second cap
limitation; the cancellation and text-display bullets now distinguish the
`eframe` orb from the GNOME card and Noctalia tooltip; and the integration
checklist drops the completed VAD and barge-in items and narrows the
text-display item to the orb.

**File:** [`docs/voice-and-ui.md`](../docs/voice-and-ui.md)

| Line | Text | Reality |
| --- | --- | --- |
| [21](../docs/voice-and-ui.md#L21) | `transcribe_from_microphone(6.0)` | The runtime passes `8.0` ([`runtime.rs:327`](../src/agent/runtime.rs#L327), [`runtime.rs:1402`](../src/agent/runtime.rs#L1402)). |
| [191](../docs/voice-and-ui.md#L191) | "The older `TextToSpeech::speak_async` helper remains" | No such symbol exists; CHANGELOG says it was removed. |
| [220](../docs/voice-and-ui.md#L220) | "STT uses a fixed recording window" | Adaptive endpointing is implemented; the same document says so at lines 102–112. |
| [228](../docs/voice-and-ui.md#L228) | "Tool calls and STT have no user-facing cancellation path" | Stop / Stop listening exist and are documented at line 96 and in the GNOME/Noctalia menus. |
| [227](../docs/voice-and-ui.md#L227) | "The UI does not display transcripts or response text" | True only for the `eframe` orb; the GNOME card and Noctalia tooltip show text. Clarify the subject. |

**Fix applied:** those five statements were updated; the "Remaining
limitations" section still covers echo cancellation, portal approval, and orb
placement.

**Done:** the file no longer references removed symbols or contradicts its own
endpointing section.

---

## D5 — Architecture statements about wake and optional tools (P2) — resolved 2026-10-29

**Status:** resolved. The wake sentence now distinguishes a typed message
(immediate cancellation) from a wake word during a running turn (speech probe
first; only a non-empty transcript replaces the turn). Tool registration is
described as `browser` when CDP configuration loads and Linux
`accessibility`/`window`/`gnome_doctor` unconditionally. The full candidate
order is listed in both files. The same stale claims were corrected in
[`README.md`](../README.md#L40) (status row, startup list, interruption prose,
and wake-model fallback) and the echo-cancellation bullet in
[`docs/voice-and-ui.md`](../docs/voice-and-ui.md#L213-L231).

**Files:** [`ARCHITECTURE.md`](../ARCHITECTURE.md#L44-L48),
[`docs/architecture.md`](../docs/architecture.md#L54-L68)

| Statement | Reality |
| --- | --- |
| [ARCHITECTURE.md:44](../ARCHITECTURE.md#L44) "A new typed message or wake word cancels the active turn" | A wake word during an active turn starts a speech probe; the turn is cancelled only if a non-empty transcript arrives ([`runtime.rs:303-L355`](../src/agent/runtime.rs#L303-L355), [`voice-and-ui.md:92-L95`](../docs/voice-and-ui.md#L92-L95), CHANGELOG fix). |
| [docs/architecture.md:54](../docs/architecture.md#L54) optional tools registered "when their backends are available" | `accessibility`, `window`, and `gnome_doctor` are registered unconditionally on Linux ([`main.rs:672-L679`](../src/main.rs#L672-L679)); they report unavailability at call time. |
| [docs/architecture.md:68](../docs/architecture.md#L68) candidate list `hey_hyusk`, `hey_livekit`, `nihao_livekit` | Auto-discovery also includes `alexa`, `hey_jarvis`, `hey_mycroft`, `hey_rhasspy` ([`main.rs:74-L82`](../src/main.rs#L74-L82)). README's status table likewise lists only two models. |

Do **not** change [`docs/architecture.md:202`](../docs/architecture.md#L202)
("A new `UserInput` … cancels the active turn"); that one is correct.

**Fix applied:** the wake sentence now says a wake word starts a speech probe and
only a non-empty transcribed instruction replaces the turn; the registration
sentence is corrected; and the real auto-discovery candidates are listed. The
same corrections were applied to `README.md`.

---

## D6 — Mobile-link sync claims are ahead of the implementation (P2) — resolved 2026-10-29

**Status:** resolved. [`docs/mobile-link-protocol.md`](../docs/mobile-link-protocol.md)
now marks `memory.sync` and `workflow.sync` as reserved in v1, describes the
acknowledge-and-drop behaviour (`{"accepted": true}`), documents the real v1
payload shapes, and keeps the revision/tombstone/conflict semantics as the
target contract. It also notes that `turn.submit` accepts but does not enforce
`source_device`, `target_device`, `timeout_ms`, `idempotency_key`, and
`workflow`. `README.md` carried no sync claim and
[`docs/android-agent.md`](../docs/android-agent.md#L30-L33) was already correct,
so neither needed a change.

**Files:** [`docs/mobile-link-protocol.md`](../docs/mobile-link-protocol.md#L32-L49),
[`README.md`](../README.md#L53), [`docs/android-agent.md`](../docs/android-agent.md#L30-L33)

**Claim:** `memory.sync` and `workflow.sync` exchange revisioned, encrypted
records with base revisions, tombstones, and conflict handling.

**Reality:** the laptop accepts both methods and logs that merge support is
pending, then drops the payloads
([`main.rs:793-L803`](../src/main.rs#L793-L803)). `android-agent.md` correctly
says automatic merge is disabled; the protocol document presents it as
implemented.

**Fix applied:** the two methods are marked **reserved / accepted but not
merged** in protocol v1, the current no-op behaviour is described, and the
target semantics remain a future contract. `turn.submit` context fields are
noted as accepted but not yet enforced.

---

## D7 — `mobile pairing` is undocumented and exposes the pairing secret (P1 security)

**Files:** [`src/tools/mobile.rs`](../src/tools/mobile.rs#L49),
[`src/tools/mobile.rs`](../src/tools/mobile.rs#L65-L67),
[`docs/tools.md`](../docs/tools.md) (Mobile section)

**Reality:** the model-facing schema includes `action: "pairing"`, which
returns the `PairingPayload` — endpoint, TLS fingerprint, and the live
single-use pairing secret — as a tool result. That result goes to the remote
provider and into session history. It has no approval arm.

**Fix (recommended):** remove `pairing` from the tool's action enum/schema; the
GNOME/Noctalia indicators already have a direct path to the payload. If it
must stay, gate it with an approval and avoid returning the secret verbatim.

**Done when:** no model-reachable tool result contains the pairing secret.

---

## D8 — Minor consistency items (P3) — resolved 2026-10-29

- **CHANGELOG:** added entries for `4e28322` (streamed-reply dedupe, Fixed) and
  `8ee8c37` (standalone workspace, Changed). D3/D7 entries remain pending those
  decisions.
- **README project layout** ([`README.md`](../README.md#L700-L752)): `Cargo.lock`
  is now listed as the committed lockfile for this workspace.
- **README status table** ([`README.md:46`](../README.md#L46)): the
  `hey_hyusk`/`hey_livekit`-only wording omits the auto-discovered
  `alexa`/`hey_jarvis`/`hey_mycroft`/`hey_rhasspy` classifiers — fixed with D5.
- **CHANGELOG "Added"** (Unreleased, key-management bullet) repeated the
  "keys never enter … the runtime control file" claim — fixed with D1.
- **`docs/development.md:5-L8`:** the historical reference to
  `../crates/hyusk-cli` is fine in the parent checkout but not in a standalone
  clone; reworded to "the older `hyusk` CLI workspace".

---

## Suggested next-session order

1. Decide D7 (behaviour), remove the `pairing` action from the model-facing
   `mobile` schema, and add the matching CHANGELOG entry.
2. Re-run:
   - `cargo fmt --all -- --check`
   - `cargo check --all-targets --offline`
   - `cargo test --all-targets --offline` (isolate XDG paths; see `memory/DSH_SESSION_NOTES.md`)
   - `cargo clippy --all-targets --offline`
   - relative-link check for `README.md`, `ARCHITECTURE.md`, `docs/*.md`
3. Commit the doc/code changes with a message referencing this file.

## Resolved

- **D2 (`task` confirmation)** was fixed in code on 2026-10-29: delegation now
  arms a one-shot confirmation for both profiles, the approved call pins the
  canonical workspace root, and a regression test covers the arm and
  non-delegate actions. Suite: 283 passed, 7 ignored.
- **D3 (wake defaults)** was fixed in code on 2026-10-29: clap wake and
  wake-word denoise default to `false` in normal startup and `WAKE_TEST`, the
  detector constructor default is `false`, and the README table lists
  `WAKE_WORD_STEP_MS`. `.env.example` is unchanged.
- **D8 (minor consistency items)** was corrected on 2026-10-29: CHANGELOG
  entries for the streamed-reply dedupe and standalone workspace, `Cargo.lock`
  in the README layout, and the `docs/development.md` historical path. Only the
  D3/D7 changelog entries remain pending their decisions.
- **D6 (mobile-link sync)** was corrected on 2026-10-29: the protocol document
  now marks `memory.sync`/`workflow.sync` reserved in v1, documents the
  acknowledge-and-drop behaviour and v1 payloads, and keeps conflict/tombstone
  semantics as a target contract; `turn.submit` context fields are noted as
  unenforced.
- **D5 (architecture statements)** was corrected on 2026-10-29: the wake
  probe/no-cancel distinction, unconditional Linux tool registration, and the
  full wake-classifier candidate order now match `runtime.rs` and `main.rs`; the
  same claims were fixed in `README.md` and `docs/voice-and-ui.md`.
- **D4 (voice-and-ui.md)** was corrected on 2026-10-29: the stale recording
  duration, removed `speak_async` reference, fixed-window limitation,
  cancellation claim, and text-display claim now match the implementation.
- **D1 (API-key flow)** was corrected on 2026-10-29 in `README.md`,
  `CHANGELOG.md`, `docs/gnome-indicator.md`, and `docs/tools.md`: the GNOME
  restart and the transient Noctalia control-file handoff are now stated per
  client, and no claim says the key never touches the control file.
- Test counts and clippy description in
  [`docs/development.md`](../docs/development.md#L116-L128) were corrected on
  2026-10-29 to the measured 282/7 and the real `LinkServerError` warnings.
- Standalone-workspace statements and commands in README/development docs were
  updated and verified by an actual build/test run.
