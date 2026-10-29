# Agent loop and runtime issues

The runtime is one event loop around `Arc<Mutex<Agent>>`. A turn locks the
agent, then awaits the model, tools, speech, and persistence while holding the
lock. Control events (stop, model switch, key save) need the same lock, so they
wait for the turn to end. This file records the resulting responsiveness and
plumbing issues. None of them blocks the permission work, but they explain why
some controls feel unresponsive.

## L1 — Split the agent mutex

- Status: open · Phase 3 · Priority: high
- Depends on: —
- Evidence: `src/agent/runtime.rs:1514-1575`, `src/agent/runtime.rs:877-995`

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

- [ ] Emergency stop disarms an approval within a bounded time during a model
      call.
- [ ] Saving a key or switching a model does not wait for a turn to finish.
- [ ] No new lock-order deadlock; the turn observes a cancelled token.
- [ ] Existing turn tests pass unchanged.

**Tests**

- [ ] Concurrency test with a slow model future and an intervening control
      event.
- [ ] Lock-order documentation test or comment.

**Files.** `src/agent/runtime.rs`, `src/agent/runner.rs`,
`src/types/event.rs`.

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

- Status: open · Phase 3 · Priority: medium
- Depends on: —
- Evidence: `src/model/openrouter.rs:85-86`

**Problem.** The client sets a 150-second total request timeout, which also
caps streams. A long answer that keeps producing tokens is cut mid-reply; the
runner then reports a failed stream after part of it was spoken.

**Decision (2026-10-29).** Use an idle timeout between chunks (for example 45
seconds without data) plus a configurable total cap. Preserve the existing
connect timeout and retry rules.

**Acceptance criteria**

- [ ] A stream longer than 150 seconds succeeds while chunks keep arriving.
- [ ] A stalled stream fails after the idle window.
- [ ] The total cap is configurable and documented in `.env.example`.
- [ ] The partial-speech rule is unchanged.

**Tests**

- [ ] Local stream fixture with slow chunks.
- [ ] Stalled fixture.

**Files.** `src/model/openrouter.rs`, `src/model/openrouter/stream.rs`,
`.env.example`, `docs/development.md`.

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
