# Documentation issues

Tracked documentation that does **not** match the current code. Created
2026-10-29 at commit `4e28322` (`hyusk_agent` is now a standalone Cargo
workspace). Read
[`DSH_SESSION_NOTES.md`](../memory/DSH_SESSION_NOTES.md) first for build,
environment, and tooling context.

Status: **open**. Build/tests are green; these are accuracy issues, not build
failures. Nothing here has been fixed yet except the items marked
**resolved**.

## Verified baseline

| Check | Result |
| --- | --- |
| `cargo check --all-targets --offline` | 0 errors, 0 warnings |
| `cargo test --all-targets --offline -- --test-threads=4` | 282 passed, 0 failed, 7 ignored |
| `cargo clippy --all-targets --offline` | 0 errors; 3 pre-existing `result_large_err` warnings in `src/link/server.rs` |
| `cargo fmt --all -- --check` | clean |

Live microphone/STT/TTS, portal input, GNOME/niri window control, live CDP,
and real provider calls were **not** exercised. See
`memory/DSH_SESSION_NOTES.md` for the isolation commands used to run the tests
without touching real user data.

## Decisions needed first

Three issues involve behavior, not just prose. Decide before editing docs:
either change the code to match the docs or correct the docs to match the code.

| ID | Decision | Recommended |
| --- | --- | --- |
| D2 | Add runtime approval for `task` (especially `workspace`) **or** soften the docs | Add approval; it is the documented safety contract |
| D3 | Make wake defaults `false` in code **or** change docs to `true` | Make code `false`; docs/CHANGELOG deliberately describe clap/denoise as opt-in |
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

## D2 — `task` confirmation claim has no implementation (P1)

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

**Fix (recommended):** add a `task_requires_approval` arm that arms a one-shot
confirmation for `action: "delegate"` (at minimum `profile: "workspace"`), and
record the canonical workspace path in the approval arguments. The runner's
generic pending-approval path already stores the call and executes it on a
"yes". Add a regression test that asserts the arm fires for `workspace` and
(for the docs' "either profile" wording) for `research`.

**Alternative:** correct the docs to say no runtime approval is required and
the model chooses the workspace root. Not recommended: it removes a safety
property the rest of the docs advertise.

**Done when:** the tool call is confirmed before a worker starts, and the docs
statement is true.

---

## D3 — Wake defaults differ from the documented defaults (P1/P2)

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

**Fix (recommended):** change both `unwrap_or(true)` defaults to `false` in
normal startup, align the `WAKE_TEST` path, and document `WAKE_WORD_STEP_MS` in
the README table. Keep `.env.example` unchanged.

**Done when:** running without `.env` behaves as documented, and both docs
tables list every supported wake setting.

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

## D5 — Architecture statements about wake and optional tools (P2)

**Files:** [`ARCHITECTURE.md`](../ARCHITECTURE.md#L44-L48),
[`docs/architecture.md`](../docs/architecture.md#L54-L68)

| Statement | Reality |
| --- | --- |
| [ARCHITECTURE.md:44](../ARCHITECTURE.md#L44) "A new typed message or wake word cancels the active turn" | A wake word during an active turn starts a speech probe; the turn is cancelled only if a non-empty transcript arrives ([`runtime.rs:303-L355`](../src/agent/runtime.rs#L303-L355), [`voice-and-ui.md:92-L95`](../docs/voice-and-ui.md#L92-L95), CHANGELOG fix). |
| [docs/architecture.md:54](../docs/architecture.md#L54) optional tools registered "when their backends are available" | `accessibility`, `window`, and `gnome_doctor` are registered unconditionally on Linux ([`main.rs:672-L679`](../src/main.rs#L672-L679)); they report unavailability at call time. |
| [docs/architecture.md:68](../docs/architecture.md#L68) candidate list `hey_hyusk`, `hey_livekit`, `nihao_livekit` | Auto-discovery also includes `alexa`, `hey_jarvis`, `hey_mycroft`, `hey_rhasspy` ([`main.rs:74-L82`](../src/main.rs#L74-L82)). README's status table likewise lists only two models. |

Do **not** change [`docs/architecture.md:202`](../docs/architecture.md#L202)
("A new `UserInput` … cancels the active turn"); that one is correct.

**Fix:** reword the wake sentence to "a wake word starts a speech probe; a
non-empty transcribed instruction replaces the turn", update the registration
sentence, and list the real auto-discovery candidates.

---

## D6 — Mobile-link sync claims are ahead of the implementation (P2)

**Files:** [`docs/mobile-link-protocol.md`](../docs/mobile-link-protocol.md#L32-L49),
[`README.md`](../README.md#L53), [`docs/android-agent.md`](../docs/android-agent.md#L30-L33)

**Claim:** `memory.sync` and `workflow.sync` exchange revisioned, encrypted
records with base revisions, tombstones, and conflict handling.

**Reality:** the laptop accepts both methods and logs that merge support is
pending, then drops the payloads
([`main.rs:793-L803`](../src/main.rs#L793-L803)). `android-agent.md` correctly
says automatic merge is disabled; the protocol document presents it as
implemented.

**Fix:** mark the two methods as **reserved / accepted but not merged** in
protocol v1, describe the current no-op behavior, and keep the target semantics
as a future contract. Optionally note that `turn.submit` context fields
(`source_device`, `target_device`, `timeout_ms`, `idempotency_key`) are
accepted but not yet enforced.

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

## D8 — Minor consistency items (P3)

- **CHANGELOG:** add entries for `4e28322` (streamed-reply dedupe) and
  `8ee8c37` (standalone workspace), plus the D3/D7 changes once made.
- **README project layout** ([`README.md:687-L733`](../README.md#L687-L733)):
  add `Cargo.lock` now that it is committed and owned by this crate.
- **README status table** ([`README.md:46`](../README.md#L46)): "uses
  `hey_hyusk.onnx` or `hey_livekit.onnx`" omits the auto-discovered
  `alexa`/`hey_jarvis`/`hey_mycroft`/`hey_rhasspy` classifiers.
- **CHANGELOG "Added"** (Unreleased, key-management bullet) repeated the
  "keys never enter … the runtime control file" claim — fixed with D1.
- **`docs/development.md:5-L8`:** the historical reference to
  `../crates/hyusk-cli` is fine in the parent checkout but not in a standalone
  clone; optional reword to "the older `hyusk` CLI workspace".

---

## Suggested next-session order

1. Decide D2, D3, D7 (behavior).
2. Implement the code changes (approval arm, defaults, remove pairing action).
3. Apply the doc-only fixes: D1, D4, D5, D6, D8.
4. Re-run:
   - `cargo fmt --all -- --check`
   - `cargo check --all-targets --offline`
   - `cargo test --all-targets --offline` (isolate XDG paths; see `memory/DSH_SESSION_NOTES.md`)
   - `cargo clippy --all-targets --offline`
   - relative-link check for `README.md`, `ARCHITECTURE.md`, `docs/*.md`
5. Commit the doc/code changes with a message referencing this file.

## Resolved

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
