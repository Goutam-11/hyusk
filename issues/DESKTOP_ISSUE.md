# Desktop control issues

`computer` moves the real pointer, clicks, types, and presses keys. `accessibility`
reads and acts on AT-SPI elements. Both bypass the shell classifier, which is
why a guarded shell command is easier to trigger through a terminal window than
through the tool that was designed to be safe.

Target sessions: GNOME Wayland and niri. GNOME input injection goes through the
RemoteDesktop portal; niri exposes virtual input differently. Window focus is
available from the `window` tool on both.

## D1 — Prompt for typing into a focused terminal

- Status: done · 2026-10-29 · commit `1242402` · Phase 2 · Priority: high
- Depends on: A8
- Evidence: `src/tools/safety.rs:410-458`, `src/tools/window.rs`, `scripts/`

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

- [x] `computer type_text` with a terminal focused asks, and the card names the
      terminal.
- [x] `computer key_press`/`key_combo` with a terminal focused asks.
- [x] Typing into a text editor, browser field, or chat window stays free.
- [x] If focus cannot be determined, the action asks (fail closed).
- [ ] The check works on GNOME and niri. (Manual check not run: it needs a live
      GNOME Wayland and a niri session; see the notes.)

**Tests**

- [x] Pure `is_terminal_app` table
      (`terminal_applications_are_recognised_by_name`,
      `terminal_list_can_be_extended_without_a_code_change`).
- [x] Context-aware decision tests
      (`typing_and_key_input_ask_in_a_terminal_and_fail_closed`,
      `only_input_calls_read_the_focused_window`,
      `an_approval_for_typing_names_the_focused_terminal`,
      `focused_window_is_read_for_the_approval_gate_or_reports_none`).
- [ ] Manual check on both sessions. Not run: no live GNOME or niri control in
      this session.

**Files.** `src/tools/safety.rs`, `src/agent/runner.rs`,
`src/agent/runner/sonic.rs`, `src/tools/window.rs`, `src/tools/computer.rs`,
`docs/tools.md`, `README.md`, `.env.example`, `AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-29, implemented).** `safety::FocusContext` carries the focused
application and window title. `Decision` gained a `Contextual` variant, so the
`computer` row is classified from the arguments *and* that context while every
other row keeps its old decision. `requires_approval_with_context` is the new
entry point; `requires_approval` stays as the wrapper and is deliberately
fail-closed, so a caller without context asks for typing rather than running it.

The runner reads focus only for calls whose decision needs it
(`requires_focus_context`: `type_text`, `key_press`, `key_combo`, or a `batch`
containing one) through the new `window::focused_window`, which parses the
`focused_window` field of the existing bounded desktop snapshot. An unreadable
focus, a missing field, an empty application name, or a timeout all yield
`FocusContext::unknown`, which asks. The same wiring is in the Nova Sonic loop,
because it shares the gate.

`is_terminal_app` matches the lower-cased application name exactly against the
built-in list plus `HYUSK_TERMINAL_APPS` (comma-separated, parsed by
`parse_terminal_extension` so the extension is testable without mutating the
environment). Matching is on the application, never the window title, so a title
that merely says "terminal" does not gate typing. The approval preview appends
` (focused: <app>)` when the terminal is the reason for asking, and stays within
the 120-character limit.

Not covered by an automated test: the runner-to-window wiring end to end. A test
would read the live session, which is flaky and would touch the user's desktop,
so the pieces (decision, preview, snapshot parse) are unit-tested and the two
lane changes were reviewed by hand.

Drift: the Evidence range (`safety.rs:410-458`) pointed at the task/window
classifier helpers; the computer classifier and its new context live further
down the file. `accessibility focus` stays free as the issue's scope says: it
only changes focus, and `accessibility set_text` is D6's aliasing problem, not a
key-input path that can press Enter.

## D2 — Classify browser mutations

- Status: done · 2026-10-29 · commit `542e099` · Phase 2 · Priority: medium
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

- [x] `navigate` and `read` stay free.
- [x] `click`, `fill`, `press_key` ask and are covered by an
      "allow for this turn" grant.
- [x] The approval preview names the action, target id, and selector or text.

**Tests**

- [x] Policy table cases for every action: six free
      (`status`, `start`, `new_tab`, `tabs`, `read`, `navigate`), three gated
      (`click`, `fill`, `press_key`), plus an unknown action and malformed
      arguments (`browser_navigation_stays_free_and_mutations_ask`).
- [x] Preview cases for `fill` and `press_key`
      (`approval_preview_names_the_tool_and_its_key_arguments`).
- [x] Grant coverage end to end
      (`a_turn_grant_covers_repeated_browser_mutations`) and the grant shape
      (`a_turn_grant_is_keyed_by_tool_and_shape`).

**Files.** `src/tools/safety.rs`, `src/agent/runner.rs`, `docs/tools.md`,
`README.md`, `AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-29, implemented).** The `browser` row is
`Decision::Classified(browser_requires_approval)`. The classifier is a
whitelist of the six read/navigation actions, so `click`, `fill`, and
`press_key` ask and any future action asks until it is named as free. The
preview keys are now `action`, `target_id`, `selector`, `text`, `key`, `url`, so
a card names the mutation, the tab, and the selector or text.

A6 needed no change: `grant_key` already keys non-shell tools by their action,
so one "allow for this turn" covers subsequent clicks (a different selector is
the same shape) while a `fill` or `press_key` in the same turn asks again. The
new runner test drives that end to end with a browser stub.

Drift: the Tests line asked for "policy table cases for all six gated actions",
which contradicts the Decision's three gated actions. The table was treated as
the decision and the tests cover all nine actions; recorded here rather than
redesigning the policy. Evidence drift: `safety.rs:8-24` now points at the
`ToolName`/`Decision` definitions, and the `browser` row lives in `policy()`.

## D3 — Capability registry for portal and AT-SPI

- Status: done · 2026-10-29 · commit `D3PENDING` · Phase 2 · Priority: medium
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

- [x] On GNOME, a denied portal input is recorded and the next input action
      fails with "input capability denied" without asking for approval. (The
      denial is simulated with a fake backend in the tests; the live portal
      dialog was not denied by hand.)
- [x] On niri, the registry reports which backend is in use or unsupported: the
      Wayland input path records the `RemoteDesktop portal` backend, and the
      native backend records itself when it is used. Not exercised on a live
      niri session.
- [x] The model sees one line of capability state per turn.
- [x] `gnome_doctor` prints the same state.

**Tests**

- [x] State transitions with a fake backend
      (`state_transitions_are_recorded_with_their_backend`,
      `screenshot_and_accessibility_refusals_cover_their_calls`,
      `a_batch_cannot_hide_a_call_whose_capability_is_denied`).
- [x] Error path does not create a pending approval
      (`a_denied_input_capability_fails_without_arming_a_card`).
- [x] The prompt line and the doctor line
      (`the_per_turn_context_ends_with_one_capability_line`,
      `capability_state_uses_the_same_one_line_as_the_prompt`).

**Files.** `src/tools/capabilities.rs` (new), `src/tools/computer.rs`,
`src/tools/accessibility.rs`, `src/tools/portal.rs`,
`src/tools/gnome_doctor.rs`, `src/agent/runner.rs`,
`src/agent/runner/sonic.rs`, `src/agent/runner/prompts.rs`,
`src/agent/runtime.rs`, `docs/tools.md`, `README.md`, `AGENTS.md`,
`CHANGELOG.md`.

**Notes (2026-10-29, implemented).** `tools::capabilities` holds a
process-local registry with three states (`input`, `screenshot`,
`accessibility`), each carrying the backend that reported it. The state is
recorded where the OS answers: the portal connect path records granted, denied,
or unsupported for the RemoteDesktop portal (a missing portal service is
unsupported; a refusal or timeout is denied), the native input path records
granted or unsupported, each successful screenshot backend records itself and a
portal refusal records denied, and the AT-SPI bridge records granted or
unsupported when it starts.

`capabilities::refusal(name, arguments)` maps a call to the capability it needs
(`computer` input actions, capture actions, `click_text` needing both, batches
recursively, `accessibility` needing AT-SPI) and returns a message starting
`input capability denied …`, `screenshot capability denied …`, or
`accessibility capability unsupported …` when that state is denied or
unsupported. The text runner, the Nova Sonic loop, and the workflow executor
check it before arming anything, so a refused capability produces a tool result
and never a card. An `unknown` state returns `None`, which allows the attempt.

The per-turn context appends `capabilities: input …; screenshot …;
accessibility …` after the desktop snapshot (`append_desktop_context_with` is
the tested seam), and `gnome_doctor` adds a `Desktop capabilities` check that
prints the same line and warns once a state is denied or unsupported.

Known limits: the state is only what earlier attempts reported, so a capability
that was never tried stays `unknown` and still prompts; a granted state is
recorded on the next success, not cleared on failure unless the failure is
recognised. On Wayland (GNOME and niri) input still goes through the portal
first; the doctor's existing `enigo fallback` line remains the place to see
native availability, and choosing another backend when the portal is absent was
left as the pre-existing behaviour (it is a backend-selection change, not a
capability one).

Drift: the Evidence ranges pointed at the portal session setup, the doctor's
portal check, and the computer status function; the recording hooks are in the
same functions but at different lines.

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
