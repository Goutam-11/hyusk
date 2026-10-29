# Desktop control issues

`computer` moves the real pointer, clicks, types, and presses keys. `accessibility`
reads and acts on AT-SPI elements. Both bypass the shell classifier, which is
why a guarded shell command is easier to trigger through a terminal window than
through the tool that was designed to be safe.

Target sessions: GNOME Wayland and niri. GNOME input injection goes through the
RemoteDesktop portal; niri exposes virtual input differently. Window focus is
available from the `window` tool on both.

## D1 — Prompt for typing into a focused terminal

- Status: open · Phase 2 · Priority: high
- Depends on: A8
- Evidence: `src/tools/safety.rs:410-458`, `src/tools/window.rs`,
  `scripts/`

**Problem.** `type_text`, `key_press`, and `key_combo` are free, and
`accessibility focus` is free. Focusing a terminal, typing a command, and
pressing Enter therefore executes arbitrary shell commands with no approval,
while the equivalent `shell` call asks. This is the largest inconsistency in the
policy and the most likely explanation for "it can do dangerous things without
asking but asks for mkdir".

**Decision (2026-10-29).** Typing and key input prompt when the focused
application is a terminal; other applications stay free. TUI driving remains
possible after one approval. The terminal app list is explicit and can be
extended by environment variable, because new terminals appear regularly.

**Design note.** `requires_approval` is synchronous and context-free. Add a
small `Context` value (focused app, current window title) filled by the runner
from the `window` tool, and a context-aware entry point; keep the existing
signature as a wrapper for callers that cannot supply context.

**Scope.** In: context type, focus lookup, terminal list, policy branch, tests.
Out: refusing terminal typing entirely (the user chose to keep TUIs), and
per-keystroke approval.

**Acceptance criteria**

- [ ] `computer type_text` with a terminal focused asks, and the card names the
      terminal.
- [ ] `computer key_press`/`key_combo` with a terminal focused asks.
- [ ] Typing into a text editor, browser field, or chat window stays free.
- [ ] If focus cannot be determined, the action asks (fail closed).
- [ ] The check works on GNOME and niri.

**Tests**

- [ ] Pure `is_terminal_app` table.
- [ ] Context-aware decision tests.
- [ ] Manual check on both sessions.

**Files.** `src/tools/safety.rs`, `src/agent/runner.rs`, `src/tools/window.rs`,
`src/tools/computer.rs`.

## D2 — Classify browser mutations

- Status: open · Phase 2 · Priority: medium
- Depends on: A8
- Evidence: `src/tools/safety.rs:8-24`, `src/tools/browser.rs:471-480`

**Problem.** `browser` is absent from the classifier, so `click`, `fill`, and
`press_key` never ask. The same action through `accessibility` or `computer`
(for example a click labelled "Send") does. The browser tool drives a separate
profile, but it can still submit forms, post comments, and trigger purchases.

**Decision (2026-10-29).** Gate `click`, `fill`, and `press_key`; keep `status`,
`start`, `new_tab`, `tabs`, `read`, and `navigate` free. Pair multi-step flows
with A6 so one approval covers the rest of the turn.

**Scope.** In: policy entry, tests, docs table. Out: page-content inspection to
decide whether a form is sensitive.

**Acceptance criteria**

- [ ] `navigate` and `read` stay free.
- [ ] `click`, `fill`, `press_key` ask and are covered by an
      "allow for this turn" grant.
- [ ] The approval preview names the action, target id, and selector or text.

**Tests**

- [ ] Policy table cases for all six gated actions.

**Files.** `src/tools/safety.rs`, `src/tools/browser.rs`, `docs/tools.md`,
`README.md`.

## D3 — Capability registry for portal and AT-SPI

- Status: open · Phase 2 · Priority: medium
- Depends on: —
- Evidence: `src/tools/portal.rs:28-120`, `src/tools/gnome_doctor.rs:270-290`,
  `src/tools/computer.rs:1325-1390`

**Problem.** The OS has its own permission layer: the RemoteDesktop portal
dialog for input, the Screenshot portal for capture, and AT-SPI availability.
The agent does not track any of it. It can ask for action approval, the user
confirms, and execution still fails because the OS capability is missing or was
denied. `gnome_doctor` reports input permission as "untested".

**Decision (2026-10-29).** Keep a small in-process capability record:
`input: granted|denied|unsupported|unknown`, `screenshot: …`, `atspi: …`,
per backend. Show it in `gnome_doctor` and in the system prompt context so the
model can plan. When a capability is denied or unsupported, return a capability
error without publishing an approval card; when unknown, allow the attempt.

**Scope.** In: state, doctor output, prompt line, computer/portal error paths.
Out: retrying or repairing the OS permission automatically.

**Acceptance criteria**

- [ ] On GNOME, a denied portal input is recorded and the next input action
      fails with "input capability denied" without asking for approval.
- [ ] On niri, the registry reports which backend is in use or unsupported.
- [ ] The model sees one line of capability state per turn.
- [ ] `gnome_doctor` prints the same state.

**Tests**

- [ ] State transitions with a fake backend.
- [ ] Error path does not create a pending approval.

**Files.** `src/tools/portal.rs`, `src/tools/computer.rs`,
`src/tools/gnome_doctor.rs`, `src/agent/runner/prompts.rs`.

## D4 — Refine consequential click labels

- Status: open · Phase 2 · Priority: medium
- Depends on: A8
- Evidence: `src/tools/safety.rs:573-608`

**Problem.** The word list matches `submit`, `send`, `post`, `share`, and
`install` anywhere in a label, so "Submit search", "Send feedback", and "Share
link" ask. That trains the user to approve without reading, which weakens every
other prompt.

**Decision (2026-10-29).** Keep the high-risk words (`pay`, `purchase`, `buy`,
`delete`, `remove`, `transfer`, `place order`, `uninstall`, `reset`). Treat
generic submission words as consequential only when no benign qualifier is
present (`search`, `filter`, `query`, `calculator`, `feedback`, `link`,
`comment`?). Build a table from real labels and keep it explicit.

**Scope.** In: rule table and tests. Out: page or app context.

**Acceptance criteria**

- [ ] "Submit search" and "Send feedback" do not ask.
- [ ] "Send message" and "Post comment" ask.
- [ ] "Delete account" and "Place order" ask.
- [ ] The rule is documented in `docs/tools.md`.

**Tests**

- [ ] Label table with true and false positives.

**Files.** `src/tools/safety.rs`, `docs/tools.md`.

## D5 — Consistent keyboard risk model

- Status: open · Phase 3 · Priority: low
- Depends on: D1
- Evidence: `src/tools/safety.rs:426-449`

**Problem.** `key_press` is always free, including Delete, while `key_combo`
with a modifier and Enter/Delete/Backspace/F4 asks, and Ctrl+Q/W asks. Pressing
Delete in a file manager is destructive; pressing Escape is not. There is no
stated model.

**Decision (2026-10-29, tentative).** Define three classes: navigation keys
(free), editing keys (free outside destructive apps, gated inside),
and commit keys (Enter, modifiers, F4, gated). Reuse the D1 context for the
destructive-app list.

**Acceptance criteria**

- [ ] The classes are documented.
- [ ] Delete in a file manager asks; in a text editor it does not.
- [ ] Enter after typing in a terminal is covered by D1.

**Tests**

- [ ] Key table tests.

**Files.** `src/tools/safety.rs`, docs.

## D6 — Single-path invariant for typing versus shell

- Status: open · Phase 3 · Priority: medium
- Depends on: D1, D2
- Evidence: `src/tools/safety.rs:450-457`

**Problem.** The registry has several ways to reach the same capability
(typing, accessibility `set_text`, browser `fill`, shell). Each has its own
policy, and the union is weaker than the strongest. New tools can add another
path without anyone noticing.

**Decision (2026-10-29, tentative).** State the invariant: one capability gets
one execution path per risk class, and every aliasing path routes through the
same policy decision. Encode it as a test that enumerates input paths and
asserts the same decision for the same effective action, and document it in
`AGENTS.md` safety invariants.

**Acceptance criteria**

- [ ] The invariant is written in `AGENTS.md`.
- [ ] A test covers typing, `set_text`, and browser `fill` for a terminal or
      form target.
- [ ] Adding a new input path without a policy entry fails the test.

**Tests**

- [ ] Path-equivalence table.

**Files.** `AGENTS.md`, `src/tools/safety.rs`, tests.
