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
      GNOME Wayland and a niri session; see the notes.) 2026-10-30: the niri
      half of the *read* is verified live - `window::focused_window()` returned
      `("brave-browser", ...)` and `desktop_context` reported `source: niri IPC`
      and `session_type: wayland` on the running session. Still to do: focus a
      terminal (Alacritty was open but unfocused) and ask the agent to type, so
      the card names it; and repeat on a GNOME Wayland login. Typing into the
      user's live session is not something an agent session may do unattended.

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
      this session. 2026-10-30: `capabilities::line()` reports
      `input unknown; screenshot unknown; accessibility unknown` (nothing
      attempted in that process), and the session bus exposes
      `org.freedesktop.portal.Desktop` with the `gnome` and `gtk` portal
      implementations, which is the backend mismatch the notes describe on
      niri. The remaining half is a real portal dialog: deny it once and watch
      the capability turn `denied`.

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

- Status: done · 2026-10-29 · commit `0df4d8e` · Phase 2 · Priority: medium
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

- Status: done · 2026-10-29 · commit `d8f0d1d` · Phase 2 · Priority: medium
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

- [x] "Submit search" and "Send feedback" do not ask.
- [x] "Send message" and "Post comment" ask.
- [x] "Delete account" and "Place order" ask.
- [x] The rule is documented in `docs/tools.md`.

**Tests**

- [x] Label table with true and false positives
      (`consequential_labels_ask_only_without_a_benign_qualifier`), including
      both routes (`accessibility` click and `computer` `click_text`), the
      action name, and a high-risk word that a benign qualifier must not soften.

**Files.** `src/tools/safety.rs`, `docs/tools.md`, `README.md`, `AGENTS.md`,
`CHANGELOG.md`.

**Notes (2026-10-29, implemented).** `is_consequential_text` is now driven by
three explicit tables: `HIGH_RISK_LABEL_WORDS` (buy, delete, pay, purchase,
remove, reset, transfer, uninstall) always asks; `SUBMISSION_LABEL_WORDS`
(confirm, install, post, publish, send, share, submit) asks unless
`BENIGN_LABEL_WORDS` (calculator, feedback, filter, link, query, search) also
appears in the label; the "place order" pair and the existing "call" rules are
unchanged. The same function serves `computer` `click_text` and `accessibility`
clicks, including the control's action name, and the two policy rows' guidance
was rewritten to match the table.

Two judgement calls, both recorded here rather than left implicit:

- The decision lists `comment` as a possible benign qualifier but the
  acceptance says "Post comment" must ask. The acceptance was treated as
  authoritative, so `comment` is not a qualifier. Adding it later would make
  "Post comment" free, which the acceptance forbids.
- `confirm` and `publish` were in the old flat list but are not named in the
  decision's keep/conditional lists. They were kept as submission words
  (conditional), which preserves the old behaviour except next to a benign
  qualifier.

Drift: the Evidence range (`safety.rs:573-608`) now points at the mobile phone
action helper; the label table lives in `is_consequential_text` further down the
file.

## D5 — Consistent keyboard risk model

- Status: done · 2026-10-30 · commit `589794e` · Phase 3 · Priority: low
- Depends on: D1
- Evidence: `src/tools/safety.rs:1559-1790` (`computer_requires_approval_with_context`,
  `KeyClass`, `DESTRUCTIVE_APPS`, `key_press_asks`, `key_combo_asks`,
  `focus_is_the_reason`)

**Problem.** `key_press` is always free, including Delete, while `key_combo`
with a modifier and Enter/Delete/Backspace/F4 asks, and Ctrl+Q/W asks. Pressing
Delete in a file manager is destructive; pressing Escape is not. There is no
stated model.

**Decision (2026-10-29, tentative).** Define three classes: navigation keys
(free), editing keys (free outside destructive apps, gated inside),
and commit keys (Enter, modifiers, F4, gated). Reuse the D1 context for the
destructive-app list.

**Acceptance criteria**

- [x] The classes are documented.
- [x] Delete in a file manager asks; in a text editor it does not.
- [x] Enter after typing in a terminal is covered by D1.

**Tests**

- [x] Key table tests (`key_classes_name_the_keys_the_tool_accepts`,
      `commit_keys_ask_and_navigation_keys_run_freely`,
      `delete_asks_in_a_file_manager_and_stays_free_in_an_editor`,
      `file_managers_are_recognised_by_name`,
      `an_approval_for_a_destructive_edit_names_the_application`).

**Files.** `src/tools/safety.rs`, `docs/tools.md`, `README.md`, `AGENTS.md`,
`CHANGELOG.md`.

**Notes (2026-10-30, implemented).** `KeyClass` names the three classes and
`key_class` maps every spelling `computer::parse_keysym` accepts onto one of
them: Navigation covers arrows, Home/End, Page Up/Down, Tab, Escape, Space,
Print Screen, Caps Lock, the modifier keys, and F1-F12 except F4; Editing
covers Delete, Backspace, and Insert; Commit covers Enter/Return and F4. A
single character, an empty name, and an unsupported name have no class: the
first is text input, and the tool rejects the other two at execution.

`key_press_asks` and `key_combo_asks` are the only key gates, so the model,
instant, and workflow lanes cannot disagree. A commit key asks in every
application. An editing key asks only while `DESTRUCTIVE_APPS` (Nautilus,
Dolphin, Thunar, Nemo, Caja, PCManFM, Krusader, Double Commander, SpaceFM) is
focused, where Delete removes a file; a terminal is deliberately absent because
D1 already asks for every key there. `focus_is_the_reason` decides the card's
`(focused: …)` suffix, so a card about Delete names the file manager while a
card about Enter does not claim the application is why it asks.

Four judgement calls, recorded because the decision was tentative:

- **Enter is gated everywhere.** "Commit keys … gated" was taken literally, so
  `key_press enter` now asks even in a text editor. That tightens the case D1's
  test asserted was free, and the assertion was moved into this issue's table
  (typing itself stays free, which is what D1's acceptance says). The reason is
  consistency: `browser press_key` asks for every key (D2), so the same
  keystroke through `computer` could not stay free without leaving the union
  weaker than its strongest path. A6's grant is keyed by the `computer` action,
  so it costs one card per turn rather than one per keystroke.
- **"Modifiers" means modified commit keys, not every modifier shortcut.**
  Reading it as every ctrl/alt/super combination would make ctrl+c, ctrl+t, and
  the already-free `shift+tab` ask. A commit key in a combination asks whatever
  the modifiers are, and a *modified editing* key (ctrl+delete) stays gated as a
  bulk delete, which is the pre-D5 rule kept rather than relaxed.
- **Shift+Delete is newly gated in a file manager.** Shift is not in the strong
  modifier list, so the old rule left the permanent-delete shortcut free; the
  editing-key rule now asks there. In an editor it stays free.
- **Malformed combos ask.** An empty `keys` array or a non-string entry asks
  instead of running; the old rule treated an empty array as free.

Batch behaviour is unchanged: each step is classified on its own, so the
container cannot hide a gated key. `docs/tools.md` gained the class table
(under `## Computer`), `README.md` the user-facing summary, and `AGENTS.md` a
D5 invariant next to D1's.

Drift: the Evidence range (`safety.rs:426-449`) pointed at `truncate_preview`
and `policy_for`, not the keyboard rule, which lived in
`computer_requires_approval_with_context`; it now lives in the functions above.

Not covered by an automated test: a live Delete in a file manager. It would
delete a real file in the user's session, so the unit tests stand in for it;
see D1's note about not driving the live desktop unattended.

## D6 — Single-path invariant for typing versus shell

- Status: done · 2026-10-30 · commit `7c345f2` · Phase 3 · Priority: medium
- Depends on: D1, D2
- Evidence: `src/tools/safety.rs:82-97` (`ToolPolicy::input_paths`),
  `src/tools/safety.rs:533-551` (`reaches_input`, `input_asks`),
  `src/tools/safety.rs:1927` (`accessibility_requires_approval_with_context`)

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

- [x] The invariant is written in `AGENTS.md`.
- [x] A test covers typing, `set_text`, and browser `fill` for a terminal or
      form target.
- [x] Adding a new input path without a policy entry fails the test.

**Tests**

- [x] Path-equivalence table
      (`every_input_path_is_decided_by_the_same_capability_rule`,
      `every_tool_declares_whether_it_can_type_or_press_keys`,
      `only_input_calls_read_the_focused_window`,
      `an_approval_for_typing_names_the_focused_terminal`,
      `accessible_and_mobile_actions_are_classified`).

**Files.** `AGENTS.md`, `src/tools/safety.rs`, `docs/tools.md`, `README.md`,
`CHANGELOG.md`, tests.

**Notes (2026-10-30, implemented).** The policy row gained
`ToolPolicy::input_paths`, the actions that type text or press keys:
`computer` `type_text`/`key_press`/`key_combo`, `accessibility` `set_text`, and
`browser` `fill`/`press_key` (declared in `COMPUTER_INPUT_PATHS`,
`ACCESSIBILITY_INPUT_PATHS`, `BROWSER_INPUT_PATHS`). One shared rule,
`input_asks`, decides every declared path before the tool's own rules: text
entry or a key press asks when the focused application is a terminal or the
focus cannot be read. `reaches_input` walks a batch's steps so a container
cannot hide a path, and `requires_focus_context` now reads the focus for exactly
the declared paths on a contextual row, which is why the rule reaches the
accessibility tool at all. `preview_with_context` names the terminal for any
input path, not only for `computer`.

The behaviour change is `accessibility set_text`, which used to run free
everywhere and now asks in a terminal (the real hole: a command could be typed
into a terminal through `set_text` while the equivalent `computer type_text`
asked). In an ordinary field it stays free, so the tool remains the fast path
for forms and dialogs.

Five judgement calls, recorded because the decision was tentative:

- **The equivalence is per target class, not global.** A terminal (gated), an
  ordinary field (free), and the automation browser profile (gated by D2) are
  different classes. So the browser's `fill`/`press_key` stay their own,
  stricter class that asks for every call, while `computer type_text` in an
  ordinary browser field stays free. The invariant forbids a path being weaker
  than another path to the *same* class, not every path being equal.
- **`shell` is not an input path.** It is the capability the terminal rule
  protects; its command classifier stays in charge, and the test asserts that a
  command `shell` would ask for asks when typed into a terminal too.
- **`mobile` is out of scope.** It drives a separate device with its own action
  rows and no desktop focus; the issue's list of paths is about this machine.
- **Reading, listing, finding, focusing, `get_text`, and `click` stay free.**
  They do not reach the keyboard, so they read no focus and ask nothing; only
  `set_text` is an input path.
- **The failure the classification test catches is a new tool, not a new
  action.** `TOOLS_WITHOUT_INPUT_PATHS` names every tool that cannot reach the
  keyboard, and the test asserts `input_paths.is_empty()` matches membership, so
  a tool added with no declaration fails with `classify these tools for D6`.
  An action added to an existing input tool still has to be added to that tool's
  list by hand; nothing in the schema says which actions type text, so the table
  stays the declaration and the issue records the limit rather than pretending
  the test introspects the tools.

The second half of the same test covers the other direction: a declared path on
a row that is not contextual must ask anyway (the browser case), so a path can
never be declared and left free.

Drift: the Evidence range (`safety.rs:450-457`) pointed at
`requires_approval_with_context`; the aliasing paths lived in the three
classifiers. The declaration is now `ToolPolicy::input_paths`
(`safety.rs:82-97`) with its lists at 103-105, `TOOLS_WITHOUT_INPUT_PATHS` at
115, the shared rule in `reaches_input`/`input_asks` at 533-551, and the
accessibility classifier at 1927.
