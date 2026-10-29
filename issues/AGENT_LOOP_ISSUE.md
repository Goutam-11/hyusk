# Agent loop and runtime issues

The runtime is one event loop around `Arc<Mutex<Agent>>`. A turn locks the
agent, then awaits the model, tools, speech, and persistence while holding the
lock. Control state that the event loop needs mid-turn lives in the shared
`agent/control.rs` instead (L1). This file records the resulting
responsiveness and plumbing issues. None of them blocks the permission work,
but they explain why some controls feel unresponsive.

## L1 — Split the agent mutex

- Status: done · 2026-10-30 · commit `L1PENDING` · Phase 3 · Priority: high
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

Drift: the Evidence line pointed at `runtime.rs:1514-1575` and `877-995`, which
were the turn lock and the control arms before later sessions shifted them; the
ranges above are current. `src/types/event.rs` needed no change: the events
already carried only what the control needs.

## L2 — Async session and status writes

- Status: open · Phase 3 · Priority: medium
- Depends on: —
- Evidence: `src/agent/runner/session.rs:84-190`, `src/status.rs:80-105`

**Problem.** `persist_session` runs after every message and tool result, and
status cards write a JSON file on each update, all synchronously on async
workers. Writes are small and atomic, so the practical impact is latency
jitter, not corruption, but it is avoidable blocking.

**Decision (2026-10-29).** Add a single writer task with coalescing: at most one
pending snapshot per file, atomic replace, failures logged with a category.
Keep the synchronous path for shutdown and tests.

**Acceptance criteria**

- [ ] No blocking file IO in the hot path.
- [ ] At most one queued write per file.
- [ ] Shutdown flushes the latest snapshot.
- [ ] Failure to write is logged and does not abort a turn.

**Tests**

- [ ] Coalescing test with a fake writer.
- [ ] Shutdown flush test.

**Files.** `src/agent/runner/session.rs`, `src/status.rs`.

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

Limits of the test fixtures: the slow-chunk and stall windows are scaled down
(tens of milliseconds) so the suite stays fast, and the absence of the old
150-second body cap is verified by construction — the client builder has no
total timeout — rather than by waiting 150 seconds. The client-level tests
inject `StreamLimits` through the client's own field, which is how the wiring
from env to the parser is covered.

## L4 — Clamp timer and scheduler durations

- Status: open · Phase 3 · Priority: low
- Depends on: —
- Evidence: `src/tools/timer.rs:96-104`, `src/tools/scheduler.rs:130-140`

**Problem.** `seconds` is a `u64` straight into `Duration::from_secs` and
`tokio::time::sleep`. A huge value can overflow the deadline computation and
panic the worker, and the schema only enforces a minimum.

**Decision (2026-10-29).** Clamp to a documented maximum (30 days) and return a
clear failure above it. Apply the same rule to schedules and reminders.

**Acceptance criteria**

- [ ] `u64::MAX` seconds returns a failure, not a panic.
- [ ] The maximum is documented in the tool schema.
- [ ] Existing timer tests pass.

**Tests**

- [ ] Boundary table around the maximum.

**Files.** `src/tools/timer.rs`, `src/tools/scheduler.rs`.

## L5 — Mobile-link replay across reconnects

- Status: open · Phase 3 · Priority: low
- Depends on: —
- Evidence: `src/link/server.rs`, `docs/mobile-link-protocol.md`

**Problem.** Replay protection is per connection, so a client that reconnects
starts its sequence again. A captured command can be replayed by a new
connection.

**Decision (2026-10-29, tentative).** Persist the highest accepted sequence per
device and reject older ones after reconnect. Confirm the protocol version
impact before implementing; the phone is not in daily use here, so this stays
low priority.

**Acceptance criteria**

- [ ] A reconnect with an older sequence is rejected.
- [ ] The protocol document states the rule.
- [ ] Existing pairing and command tests pass.

**Tests**

- [ ] Reconnect fixture.

**Files.** `src/link/server.rs`, `src/link/protocol.rs`,
`docs/mobile-link-protocol.md`.

## L6 — Approval state across restart

- Status: open · Phase 3 · Priority: medium
- Depends on: A3, A6
- Evidence: `src/agent/runner.rs:718-773`, `src/agent/runner/session.rs`

**Problem.** A pending approval is in-memory only, but its placeholder tool
result is saved in the session. After a restart the model sees "needs
confirmation" with no pending state, may ask again, and may misinterpret a
later "yes".

**Decision (2026-10-29).** On load, rewrite any pending-confirmation placeholder
to "not confirmed: the service restarted before the user answered" and clear
any grant state. Do not re-arm silently.

**Acceptance criteria**

- [ ] Restarting mid-approval leaves a truthful tool result.
- [ ] The next "yes" is an ordinary utterance, not an approval.
- [ ] A test covers load with a pending placeholder.

**Tests**

- [ ] Session load fixture.

**Files.** `src/agent/runner/session.rs`, `src/agent/runner.rs`.
