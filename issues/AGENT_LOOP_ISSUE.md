# Agent loop and runtime issues

The runtime is one event loop around `Arc<Mutex<Agent>>`. A turn locks the
agent, then awaits the model, tools, speech, and persistence while holding the
lock. Control state that the event loop needs mid-turn lives in the shared
`agent/control.rs` instead (L1). This file records the resulting
responsiveness and plumbing issues. None of them blocks the permission work,
but they explain why some controls feel unresponsive.

## L1 — Split the agent mutex

- Status: done · 2026-10-30 · commit `5e14377` · Phase 3 · Priority: high
- Depends on: —
- Evidence: `src/agent/runtime.rs:897-925` (the `StopRequested` arm),
  `src/agent/runtime.rs:956-1015` (`ModelSelected` and `ApiKeySaved`),
  `src/agent/runtime.rs:1588-1589` (the turn locking the agent)

**Problem.** A turn holds `agent.lock().await` across the whole model/tool loop.
`StopRequested` cancels without the lock, but `clear_pending_approval`,
`ModelSelected`, and `ApiKeySaved` all wait for it. During a slow model call,
the stop path cannot disarm the pending approval and a key save can stall the
event loop. This is a known issue and it will get worse with A6's turn-scoped
grants.

**Decision (2026-10-29, options to confirm).** Prefer an incremental split:
extract a small control struct (provider/model, pending approval, task-paused
flag, current turn id) behind its own mutex, and leave the turn-owned state
(messages, tool loop, journal) with the turn task. The event loop then responds
to control events without waiting for the model. A full actor refactor is the
alternative if the split proves invasive.

**Scope.** In: control state extraction, event handling, tests. Out: rewriting
the runner as a task.

**Acceptance criteria**

- [x] Emergency stop disarms an approval within a bounded time during a model
      call.
- [x] Saving a key or switching a model does not wait for a turn to finish.
- [x] No new lock-order deadlock; the turn observes a cancelled token.
- [x] Existing turn tests pass unchanged.

**Tests**

- [x] Concurrency test with a slow model future and an intervening control
      event:
      `agent::runtime::tests::a_slow_model_call_cannot_hold_the_stop_path_or_a_key_save`
      holds the agent mutex across a cancellable 30-second model future, then
      disarms an approval, installs a saved key, and cancels the turn, asserting
      each step is bounded and the turn observes its token.
- [x] Lock-order documentation test or comment: the rules are documented on
      `agent::control` and `Agent::control`, and four tests hold the agent mutex
      while reading the control
      (`a_running_turn_does_not_hide_an_armed_approval`,
      `a_stale_click_is_detected_while_a_turn_holds_the_agent`,
      `an_armed_confirmation_accepts_the_decision_as_turn_input`,
      `approval_expiry_sleeps_until_the_deadline_or_forever`).

**Files.** `src/agent/control.rs` (new), `src/agent/mod.rs`,
`src/agent/runtime.rs`, `src/agent/runner.rs`, `src/agent/runner/sonic.rs`,
`docs/architecture.md`, `AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-30, implemented).** New `src/agent/control.rs` owns the state
the event loop needs mid-turn behind a `std::sync::Mutex`:
`AgentControl { provider, model, pending approval, task_paused, text client,
openrouter/openai/bedrock clients, sonic client, active API model, codex
client }`. `Agent` keeps the turn-owned state (messages, tool grants, journal,
loaded identity, follow-up flag, loop limits) and holds an `Arc` of the
control; `Agent::control_handle()` gives the runtime the handle *before* it
locks the agent, so no control event needs that lock.

The event loop now disarms a stopped approval, installs a saved key, reads the
armed approval, decides a panel click, and arms the expiry timer through the
control. Three behaviours changed for the better, and each has a test:
a click that arrives while a Sonic session (or any turn) holds the agent is
now classified from the current state instead of being forwarded blindly after
a half-second wait; a stale click reports expiry even while a turn runs; a
running turn no longer makes an unarmed confirmation look armed to the
scheduled-input gate. The expiry timer also fires during a Sonic session now,
and retries every 100 ms instead of spinning when the deadline is due but the
turn still holds the agent for the message rewrite.

Decisions: the guard is a leaf - held for a statement, never across an
`.await`, never while waiting for the agent mutex - and the lock order is agent
then control, documented in both places. A poisoned control lock is recovered
rather than propagated, because losing it would leave the loop unable to
disarm an approval. `expire_stale_approval` takes, tests, and puts back the
approval under one guard so the event loop never sees it momentarily unarmed.
`pending_approval_preview` returns an owned string because the preview now
comes from behind a guard; the post-turn card derives the card kind from that
same read so the kind and the text cannot disagree.

Two decision details were changed, recorded here: the **current turn id** was
left in the runtime, which already tracks `next_turn_id`/`ActiveTurn.id` and
cannot act on a second copy without the agent lock, so mirroring it would only
create a value that could drift; and the **provider clients** had to join the
control although the decision named only provider/model, because a key save
installs a client and that is the acceptance criterion ("saving a key does not
wait for a turn to finish"). A model switch still interrupts the turn and then
takes the agent lock, because it rebuilds the system prompt and resets the
conversation - turn-owned state - but it does not wait for the turn to finish.

Runner and Sonic tests changed only mechanically (field writes go through the
control, preview comparisons use the owned string). Three runtime tests changed
meaningfully because they asserted the removed fallbacks: "a busy lock means
defer" and "give up when the lock is held" are now "a running turn does not
hide an armed approval" and "a stale click is detected while a turn holds the
agent".

Live re-check (2026-10-30): the packaged service was not running in the current
niri session (no `hyusk-*` files in `$XDG_RUNTIME_DIR`, `hyusk.service`
inactive), so the manual "stop during a slow model call" run could not be
reproduced against a real provider. The runtime path is covered by the
concurrency test above; the remaining live half needs the service started and
the GNOME stop menu, which is the same surface as A6/A7.

Drift: the Evidence line pointed at `runtime.rs:1514-1575` and `877-995`, which
were the turn lock and the control arms before later sessions shifted them; the
ranges above are current. `src/types/event.rs` needed no change: the events
already carried only what the control needs.

## L2 — Async session and status writes

- Status: done · 2026-10-30 · commit `415c28e` · Phase 3 · Priority: medium
- Depends on: —
- Evidence: `src/persist.rs` (new writer), `src/agent/runner/session.rs`
  (`save_session`, `SessionSnapshot::capture`), `src/status.rs` (`save`),
  `src/main.rs` (`persist::start`, shutdown flush)

**Problem.** `persist_session` runs after every message and tool result, and
status cards write a JSON file on each update, all synchronously on async
workers. Writes are small and atomic, so the practical impact is latency
jitter, not corruption, but it is avoidable blocking.

**Decision (2026-10-29).** Add a single writer task with coalescing: at most one
pending snapshot per file, atomic replace, failures logged with a category.
Keep the synchronous path for shutdown and tests.

**Acceptance criteria**

- [x] No blocking file IO in the hot path.
- [x] At most one queued write per file.
- [x] Shutdown flushes the latest snapshot.
- [x] Failure to write is logged and does not abort a turn.

**Tests**

- [x] Coalescing test with a fake writer
      (`coalescing_keeps_one_pending_write_per_file`,
      `a_superseded_snapshot_is_not_even_built`).
- [x] Shutdown flush test
      (`enqueue_does_no_file_io_and_flush_writes_the_latest_snapshot`).
- [x] Writer behaviour
      (`a_snapshot_that_cannot_be_built_is_dropped`,
      `a_failed_write_is_reported_and_keeps_the_previous_file`,
      `write_atomically_leaves_a_private_file_and_no_temp_files`,
      `a_failed_rename_keeps_the_previous_file_and_cleans_the_temp`,
      `an_older_snapshot_never_overwrites_a_newer_one`,
      `categories_name_themselves`).

**Files.** `src/persist.rs`, `src/main.rs`, `src/status.rs`,
`src/agent/runner/session.rs`, `docs/architecture.md`, `docs/development.md`,
`AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-30, implemented).** `src/persist.rs` owns the writer. A caller
hands over a `Builder` (`FnOnce() -> Option<Vec<u8>>`) and returns: `enqueue`
puts at most one entry per path into a small queue, replacing the older builder
so a superseded snapshot is never serialised, and notifies the task. The task
drains the queue and replaces each file atomically (temporary file mode `0600`
in the same directory, rename, then `paths::restrict_file`) with failures logged
as `[Writer] Could not write <category> <path>: <error>`.

- `session::save_session` now clones the snapshot (`SessionSnapshot::capture`)
  and hands the writer a builder that sanitises, redacts (X4) and serialises at
  write time. `session::save_session_to_path` stays as the synchronous shape for
  tests, and `save_bytes_atomically{,_with}` moved into the writer with its two
  failure tests.
- `status::save` builds the JSON in the writer too, so a card update costs the
  store lock and a queue push. The file is now replaced atomically, which also
  matters because the GNOME extension polls it.
- `main` calls `persist::start()` at the top and `persist::flush()` before
  `process::exit`, printing how many writes it flushed.

Five judgement calls, recorded because the decision left them open:

- **The builder, not the bytes.** Coalescing at the byte level would still
  serialise (and redact) every snapshot before dropping it; handing over a
  builder means only the snapshot that actually reaches the disk is built. This
  is what makes the change win beyond moving the syscall.
- **A caller without a started writer writes synchronously.** Tests, the
  standalone workflow binary, and anything before startup keep the old
  behaviour instead of queueing into a task that will never run. The
  composition root is the only place that starts the task.
- **Newest wins, across writers.** Each queued entry carries a generation and a
  per-path "last written" map; the writer and `flush` share one lock across the
  write, so a slow writer can never put an older snapshot back over a newer one.
- **`flush` writes the queue synchronously** instead of waking the task and
  waiting: shutdown runs inside the runtime that is about to stop.
- **Status and session stay separate entries in one queue.** They are different
  files with different owners; "one pending write per file" is the invariant,
  not "one write in total".

**Not covered by a test:** the claim that `main` starts the writer, because that
is composition-root wiring; the queue behaviour it relies on is unit-tested,
and the synchronous fallback is exercised by every existing status test.

Drift: the Evidence range (`session.rs:84-190`, `status.rs:80-105`) still
pointed at the right functions but not at the writer, because the writer did not
exist; the issue now names the functions. The session's atomic-write tests moved
to `persist.rs` with the primitive.


## L3 — Streaming idle timeout

- Status: done · 2026-10-30 · commit `884b0f1` · Phase 3 · Priority: medium
- Depends on: —
- Evidence: `src/model/openrouter.rs:84-88` (was 85-86: the client builder's
  `connect_timeout` and `timeout`)

**Problem.** The client sets a 150-second total request timeout, which also
caps streams. A long answer that keeps producing tokens is cut mid-reply; the
runner then reports a failed stream after part of it was spoken.

**Decision (2026-10-29).** Use an idle timeout between chunks (for example 45
seconds without data) plus a configurable total cap. Preserve the existing
connect timeout and retry rules.

**Acceptance criteria**

- [x] A stream longer than 150 seconds succeeds while chunks keep arriving.
- [x] A stalled stream fails after the idle window.
- [x] The total cap is configurable and documented in `.env.example`.
- [x] The partial-speech rule is unchanged.

**Tests**

- [x] Local stream fixture with slow chunks
      (`model::openrouter::stream::tests::slow_chunks_keep_a_reply_alive_past_the_idle_window`,
      `model::openrouter::stream_timeout_tests::streamed_reply_succeeds_while_chunks_arrive_within_the_idle_window`).
- [x] Stalled fixture
      (`model::openrouter::stream::tests::stalled_stream_fails_after_the_idle_window`),
      plus the ceiling cases `total_limit_ends_a_stream_that_keeps_sending` and
      `capped_stream_fails_even_while_chunks_keep_arriving` and the policy table
      `stream_limits_use_defaults_and_treat_zero_total_as_unlimited`.

**Files.** `src/model/openrouter.rs`, `src/model/openrouter/stream.rs`,
`.env.example`, `README.md`, `docs/development.md`, `CHANGELOG.md`.

**Notes (2026-10-30, implemented).** The client no longer carries a total
timeout: it keeps `connect_timeout(15s)` and nothing else. Every non-streaming
request (`list_models`, `send_once`) sets `REQUEST_TIMEOUT` (150 s) per request,
so only streams changed behaviour. A streamed request now has two bounds, held
in `stream::StreamLimits` and applied inside `parse_sse_stream`:
`HYUSK_STREAM_IDLE_TIMEOUT_SECS` (default 45, clamped to 3600) fails the reply
with `Model stream stalled: no data arrived for 45s` when no chunk arrives, and
`HYUSK_STREAM_TOTAL_TIMEOUT_SECS` (default 600, clamped to 86400, `0` disables)
ends a reply that has streamed for that long with
`Model stream exceeded the 600s total limit`. The wait for the next chunk is
`min(idle, remaining total)`, so the reply ends on whichever bound is reached
first, and a total of `0` leaves the idle window as the only bound.

Connecting and waiting for response headers stays bounded by the 150-second
request timeout through `open_stream`; an overrun becomes a network error, so
the existing retry loop treats it exactly like a connect failure. The retry
rules, the `[DONE]`/finish-reason completion rule, the 4 MiB stream cap, and the
runner's partial-speech rule (a stream that fails after part of the reply was
spoken is reported, never replayed as a plain request) are unchanged.

Drift: the Evidence line `openrouter.rs:85-86` pointed at the two builder calls,
which is exact for the pre-change file; after the change the builder is at
92-97 and the policy lives in `src/model/openrouter/stream.rs:15-63`.

Live re-check (2026-10-30): the real `OpenRouterClient::chat_stream` path was
run once against a local fixture that sent a chunk every 100 ms for 1,650
chunks - 167.1 s of streaming, 6,600 bytes, well past the old 150-second cap -
and completed successfully with all deltas delivered. That is the end-to-end
proof of the fix; it took the place of a provider run because `~/.env` holds a
billed key and nobody should spend tokens to prove a timeout. The fixture was
temporary and is not part of the suite.

Limits of the test fixtures: the slow-chunk and stall windows are scaled down
(tens of milliseconds) so the suite stays fast, and the absence of the old
150-second body cap is verified by construction — the client builder has no
total timeout — rather than by waiting 150 seconds. The client-level tests
inject `StreamLimits` through the client's own field, which is how the wiring
from env to the parser is covered.

## L4 — Clamp timer and scheduler durations

- Status: done · 2026-10-30 · commit `15b9253` · Phase 3 · Priority: low
- Depends on: —
- Evidence: `src/tools/timer.rs` (`set`), `src/tools/scheduler.rs`
  (`execute` schedule arm, `bounded_sleep_secs`), `src/tools/mod.rs`
  (`MAX_WAIT_SECONDS`)

**Problem.** `seconds` is a `u64` straight into `Duration::from_secs` and
`tokio::time::sleep`. A huge value can overflow the deadline computation and
panic the worker, and the schema only enforces a minimum.

**Decision (2026-10-29).** Clamp to a documented maximum (30 days) and return a
clear failure above it. Apply the same rule to schedules and reminders.

**Acceptance criteria**

- [x] `u64::MAX` seconds returns a failure, not a panic.
- [x] The maximum is documented in the tool schema.
- [x] Existing timer tests pass.

**Tests**

- [x] Boundary table around the maximum
      (`a_timer_is_bounded_by_the_documented_maximum`,
      `a_schedule_is_bounded_by_the_documented_maximum`,
      `the_worker_sleep_is_bounded`,
      `a_persisted_schedule_past_the_maximum_does_not_panic`).

**Files.** `src/tools/timer.rs`, `src/tools/scheduler.rs`, `src/tools/mod.rs`,
`docs/tools.md`, `AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-30, implemented).** `tools::MAX_WAIT_SECONDS` (30 days, 2592000
seconds) is the one bound, with `MAX_WAIT_LABEL` for messages. `timer` `set`
refuses a longer `seconds` with "A timer may run for at most 2592000 seconds (30
days).", and the scheduler refuses an `after_seconds`/`at_unix` further out with
the same shape of message; both schemas carry `maximum` (and a description) so
the model sees the limit before it calls. The boundary is inclusive: exactly
2592000 works, one second more is refused.

Three judgement calls, recorded because the decision left them open:

- **One constant, two tools.** The number is shared rather than written twice,
  because the clamp, the message, and both schemas have to agree and a drifting
  copy would let one path through.
- **The scheduler enforces it on the resolved date, not on the field.** Checking
  `run_at_unix - now` covers `after_seconds` and `at_unix` with one rule and
  catches any future arithmetic change in between.
- **The worker caps its own sleep.** A `schedules.json` written before this
  change (or edited by hand) can hold a date further out than the maximum, and
  the rearm path sleeps for it; `bounded_sleep_secs` wakes at the bound and
  recomputes, so the panic is unreachable from the file as well as from a call.

Drift: the Evidence ranges (`timer.rs:96-104`, `scheduler.rs:130-140`) still
pointed at the right functions; `bounded_sleep_secs` and the shared constant are
new, so the issue now names the functions rather than ranges. Not covered by a
test: that the old code actually panicked rather than freezing; the failure mode
is `tokio::time::sleep`'s deadline overflow, and the boundary table is the
regression guard.


## L5 — Mobile-link replay across reconnects

- Status: done · 2026-10-30 · commit `47bdaf0` · Phase 3 · Priority: low
- Depends on: —
- Evidence: `src/link/auth.rs` (`StoredDevice::last_sequence`,
  `DeviceStore::accept_sequence`), `src/link/server.rs` (hello reply and the
  request loop), `src/link/protocol.rs` (`is_fresh_sequence`),
  `docs/mobile-link-protocol.md`

**Problem.** Replay protection is per connection, so a client that reconnects
starts its sequence again. A captured command can be replayed by a new
connection.

**Decision (2026-10-29, tentative).** Persist the highest accepted sequence per
device and reject older ones after reconnect. Confirm the protocol version
impact before implementing; the phone is not in daily use here, so this stays
low priority.

**Acceptance criteria**

- [x] A reconnect with an older sequence is rejected.
- [x] The protocol document states the rule.
- [x] Existing pairing and command tests pass.

**Tests**

- [x] Reconnect fixture
      (`a_reconnect_cannot_replay_an_accepted_sequence`).
- [x] Rule table (`a_sequence_is_fresh_only_when_it_advances`).

**Files.** `src/link/auth.rs`, `src/link/server.rs`, `src/link/protocol.rs`,
`android/app/src/main/java/io/github/hyusk/mobile/network/HyuskRpcClient.kt`,
`docs/mobile-link-protocol.md`, `CHANGELOG.md`.

**Notes (2026-10-30, implemented).** `StoredDevice` gained `last_sequence`
(`#[serde(default)]`, so an existing `link-devices.json` loads unchanged), and
`DeviceStore::accept_sequence` advances it only when the request's sequence is
strictly greater, writing the device map each time. The connection loop now
checks the MAC first and the per-device freshness second, and the hello reply
returns the accepted value as `result.sequence`. `protocol::is_fresh_sequence`
is the rule itself, so the protocol module states it and the store applies it.

The protocol-version question the decision raised: the wire format is unchanged,
so `hyusk.link.v1` stays. The behaviour is a rule the laptop enforces and the
phone must honour, which is why the in-repo Android client was updated in the
same commit: on the hello reply it seeds its counter from `result.sequence`
instead of relying on a fresh zero. Without that, the client's existing reset in
`closeSocket` would have made every request after a reconnect a replay. The
client change is three lines and could not be compiled here (no Android SDK in
the container); it is recorded as a manual check rather than claimed as verified.

Three judgement calls, recorded because the decision left them open:

- **The MAC is checked before the sequence.** An unauthenticated frame must
  never be able to advance the device's counter.
- **The counter is persisted with the device map on every accepted request.**
  The link is low-traffic (a heartbeat plus commands), and the write is the same
  atomic `0600` replace the store already used. A failed write is logged and the
  request still runs, because the in-memory mark already enforces the rule for
  the running process and the next write catches up.
- **The hello reply carries the mark.** It is only disclosed to a device that
  has just proved possession of its secret, and it lets a client with no local
  state (a fresh app process) continue the sequence instead of needing its own
  persistence.

Drift: the Evidence was a module reference (`src/link/server.rs`) and the doc; it
now names the functions. Not covered by an automated test: a full WebSocket
reconnect (the suite has no TLS client fixture); the store-level fixture models
the same transition, including a reload that stands in for a laptop restart.


## L6 — Approval state across restart

- Status: done · 2026-10-30 · commit `f2aa2d8` · Phase 3 · Priority: medium
- Depends on: A3, A6
- Evidence: `src/agent/runner/session.rs` (`load_session_from`,
  `settle_interrupted_approvals`), `src/tools/safety.rs` (`PENDING_PREFIX`)

**Problem.** A pending approval is in-memory only, but its placeholder tool
result is saved in the session. After a restart the model sees "needs
confirmation" with no pending state, may ask again, and may misinterpret a
later "yes".

**Decision (2026-10-29).** On load, rewrite any pending-confirmation placeholder
to "not confirmed: the service restarted before the user answered" and clear
any grant state. Do not re-arm silently.

**Acceptance criteria**

- [x] Restarting mid-approval leaves a truthful tool result.
- [x] The next "yes" is an ordinary utterance, not an approval.
- [x] A test covers load with a pending placeholder.

**Tests**

- [x] Session load fixture
      (`a_restart_settles_a_pending_confirmation_placeholder`,
      `an_armed_placeholder_is_still_saved_while_the_approval_is_pending`,
      `settling_leaves_unrelated_tool_results_alone`).
- [x] The next "yes" reaches the model
      (`agent::runner::tests::a_yes_after_a_restart_is_an_ordinary_utterance`).

**Files.** `src/agent/runner/session.rs`, `src/agent/runner.rs`,
`src/tools/safety.rs`, `AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-30, implemented).** `load_session` now delegates to
`load_session_from(path)`, and after the usual sanitisation it calls
`settle_interrupted_approvals`, which rewrites every tool result that starts
with `safety::PENDING_PREFIX` to `RESTARTED_BEFORE_ANSWER`: "Not confirmed: the
service restarted before the user answered. This action was not executed. A
later \"yes\" does not approve it; if the user still wants it, call the tool
again." The prefix moved into `safety::PENDING_PREFIX` so the writer and the
repair cannot drift apart. The count is printed once at load
(`[Agent] N unconfirmed action(s) were settled by the restart`).

Four judgement calls, recorded because the decision left them open:

- **Only a load settles the placeholder.** The live process still has the card
  and the saved copy must keep the sentence its tool result will be replaced by,
  so the repair is not part of saving; a test pins that.
- **Grants need no clearing.** A6's turn grants live in `turn_grants`, are
  cleared at turn start and end, and are never written to the session, so the
  only trace of a lost approval is the placeholder.
- **Nothing is re-armed from the session**, as the decision says: the armed call
  is not persisted at all, and the repair only replaces text.
- **The rewritten result names the tool call id it answers**, so the assistant
  tool-call / tool-result pairing and the provider protocol stay valid.

The prompt already carried "There is currently no pending tool approval.
Earlier confirmation questions in conversation history are inactive"; this makes
the conversation agree with it, which is what the acceptance asks for.

Drift: the Evidence range (`runner.rs:718-773`) pointed at the old turn start in
a file that has shifted several times; the issue now names the functions
instead. Not covered by an automated test: a real process restart between an
armed approval and the next start, because the suite drives the load path
directly with a fixture.

