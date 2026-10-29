# Permission and approval issues

The approval path today: the model calls a tool, the runner asks
`safety::requires_approval(name, raw_arguments)`, and if it is true the runner
stores the exact call in `PendingApproval` and returns a failed tool result
telling the model to ask the user. The next user utterance is interpreted by
`confirms`/`is_denial`; a confirmation executes the stored call, a denial drops
it, anything else re-asks. Panels publish a card with kind `approval`; the
Approve/Deny buttons write `approve`/`deny` into `hyusk-control.json`, which
`main.rs` turns into `UserInput("yes")`/`UserInput("no")`.

Reported pains on 2026-10-29, in the user's order: model asks then runtime asks
again; asked for every small shell/file action; multi-step tasks need repeated
approvals; approval card or buttons missing/inconsistent; pending approval
swallows the next request.

## A1 — One source of truth for approval guidance

- Status: done · 2026-10-29 · commit `b4b2725` · Phase 1 · Priority: high
- Depends on: A8
- Evidence: `src/agent/runner/prompts.rs:131-142`, `src/tools/safety.rs:8-24`

**Problem.** The hand-written prompt contradicts `requires_approval`. It says
"workspace tasks do not need confirmation" while every `task` delegation is
gated (`safety.rs:49-54`), and it lists only destructive shell commands, window
closing, clipboard, and memory forgetting while the gate also covers
`process` launches with arguments, unknown mobile actions, and flag-level
checks. It also tells the model to ask the user before destructive actions
(`prompts.rs:131-133`); when the call then arrives, the runtime asks again. That
is the double confirmation the user ranked first.

**Decision (2026-10-29).** The runtime is the only component that asks. The
model must call the tool and let the runtime hold it. Guidance is generated from
the same policy table that classifies tools (A8), so prose cannot drift.

**Scope.** In: replace the contradictory block with generated guidance; add
"do not ask before calling a tool; the runtime holds guarded calls and returns
the pending call"; tests. Out: any policy change (S1, A2) and panel work (A6,
A7).

**Acceptance criteria**

- [x] `build_system_prompt` contains one guidance block derived from the policy
      table, not a hand-maintained list. `prompts::approval_guidance_block()`
      renders `tools::safety::approval_guidance()`, one row per registered tool.
- [x] No prompt text claims an ungated class or omits a gated one. The block and
      `user_profile_context` no longer pre-ask; the per-turn note in the runner
      now repeats the "never ask first" rule.
- [x] On a guarded request the model calls the tool once, without a spoken
      "should I?", and repeats the runtime's pending-call text (A3). The prompt
      now says the runtime is the only component that asks and describes what to
      say when a result says the action needs confirmation.
- [x] Routine work (`rg`, `sed -n`, `git status`, `cargo test`) is named as free,
      matching the classifier, in the generated shell row.

**Tests**

- [x] Prompt contains every policy row from the A8 table
      (`prompts::tests::approval_block_is_generated_with_every_policy_row_once`).
- [x] The retired phrases are absent
      (`retired_guidance_phrases_are_absent`).
- [x] `prompt_names_routine_work_as_free`,
      `prompt_tells_the_model_never_to_pre_ask`, and
      `generated_rows_never_contradict_their_decision`.
- [ ] Manual voice check: "delete the build folder" asks once. Not run in this
      session: it needs a live microphone, STT, and a provider.

**Files.** `src/agent/runner/prompts.rs`, `src/tools/safety.rs`,
`src/agent/runner.rs`.

**Notes (2026-10-29, implemented).** The retired block claimed that ordinary
app launches, navigation, timers, and workspace tasks needed no confirmation,
listed only part of the gated classes, and told the model to ask before
destructive actions. All three sentences are gone. The replacement is generated
from the A8 table, so every gated class is named, and the prompt states that the
runtime is the only component that asks, that a guarded call should be made once,
and that the model repeats the pending action instead of retrying it.

Evidence drift: the retired block now sits at
`src/agent/runner/prompts.rs:119-141` before the change (the file grew by the
`build_tool_specs`/`append_desktop_context` helpers). `safety.rs:8-24` was still
the gate when the review was written; A8 moved it.

## A2 — Research delegation runs without approval

- Status: done · 2026-10-29 · commit `d31228a` · Phase 1 · Priority: high
- Depends on: A1
- Evidence: `src/tools/safety.rs:49-54`, `src/tools/task.rs` (research worker)

**Problem.** `task_requires_approval` returns true for every `delegate`,
including `profile: "research"`, which is one `client.chat` call with no tools
and no filesystem access. Every voice request to research something costs a
confirmation. Workspace delegation needs Codex CLI; when it is missing the user
is asked first and then told the launch failed.

**Decision (2026-10-29).** Research is free. Workspace asks once, with the
canonical root pinned by `approval_arguments`. Availability is preflighted
before arming so the user is never asked for a delegation that cannot run.

**Scope.** In: policy change, preflight, tests. Out: changing the delegation
sandbox or Codex integration.

**Acceptance criteria**

- [x] `task` with `profile: "research"` runs immediately: the policy row allows
      it, and `runner::tests::research_delegation_runs_without_an_approval_card`
      drives a full turn with no pending approval.
- [x] `task` with `profile: "workspace"` still asks once and pins the root.
      `task_requires_approval` asks for every profile other than an explicit
      research, so an unknown profile fails closed. `approval_arguments` stores
      the canonical root, from the call or from `HYUSK_WORKSPACE`.
- [x] Missing Codex or `HYUSK_WORKSPACE` returns a clear failure and no approval
      card is published: `task::delegation_preflight` runs in the text and Sonic
      paths before arming, and
      `runner::tests::unavailable_workspace_delegation_reports_instead_of_asking`
      covers the card-free path.

**Tests**

- [x] Safety table cases for both profiles
      (`research_delegation_is_free_and_workspace_delegation_asks_once`,
      `workspace_delegation_pins_the_canonical_root`).
- [x] Task preflight failure paths:
      `workspace_preflight_reports_a_missing_project_root`,
      `..._a_workspace_that_does_not_exist`, `..._a_missing_codex_cli`, and
      `codex_cli_discovery_looks_only_for_an_executable_on_path`.
- [x] `workspace_delegation_uses_the_configured_root` and
      `research_delegation_is_never_blocked_by_availability`.

**Files.** `src/tools/safety.rs`, `src/tools/task.rs`, `src/agent/runner.rs`,
`src/agent/runner/sonic.rs`, `src/agent/runner/prompts.rs`, `README.md`,
`docs/tools.md`, `AGENTS.md`.

**Notes (2026-10-29, implemented).** The gate now treats an absent profile as
research (the tool's own default) and asks for anything else, so a new profile
cannot slip through free. The project root resolves through one helper
(`task::canonical_workspace`) used by the gate's pinning, the preflight, and the
worker, so the approved root is the executed root.

Decisions beyond the letter of the issue:

- The root falls back to `HYUSK_WORKSPACE` when a workspace delegation names
  none. Without that, the acceptance line about a missing `HYUSK_WORKSPACE`
  could not be reached, and the model would have to reproduce the installer's
  pinned path.
- The preflight lives in `task.rs` and is called from `runner.rs` and
  `sonic.rs`; it checks the `codex` executable on `PATH` and canonicalises the
  root without running anything, so a card is only published for work that can
  start.
- Evidence drift: `safety.rs:49-54` was the old two-line `task` arm; A8 moved it
  into the policy table.

**Not done.** The manual voice check from A1 is still outstanding: it needs a
live microphone, STT, and a provider.

## A3 — Approval shows the exact call

- Status: open · Phase 1 · Priority: high
- Depends on: A8
- Evidence: `src/agent/runner.rs:941-955`, `src/agent/runtime.rs:1575-1600`,
  `src/agent/runner/sonic.rs:392-409`

**Problem.** The card shows the model's paraphrase, and the stored arguments are
never surfaced. The user approves a sentence such as "I'll clean up the build
output" without seeing `shell: rm -rf target`.

**Decision (2026-10-29).** The policy decision produces a `preview` string: tool
name plus normalised key arguments, truncated to about 120 characters. The
runner stores it in `PendingApproval`, includes it in the tool result as
"Pending confirmation for: <preview>", and the status card shows the same text.
The model speaks a short form derived from it.

**Scope.** In: preview generation, storage, tool result, card text, Sonic card
text. Out: redaction of secret-shaped values (X2) and the consent UI (A6).

**Acceptance criteria**

- [ ] Card and tool result show the same tool and arguments.
- [ ] Long commands truncate predictably with an ellipsis.
- [ ] The stored preview is what the user approves; argument substitution after
      approval is impossible.
- [ ] Speech names the tool and the first few argument words.

**Tests**

- [ ] Preview formatting for `shell`, `process`, `task`, `mobile`, `window`.
- [ ] Truncation and empty-argument cases.
- [ ] Runtime card text uses the preview.

**Files.** `src/tools/safety.rs`, `src/agent/runner.rs`,
`src/agent/runner/sonic.rs`, `src/agent/runtime.rs`.

## A4 — Confirmation grammar for voice

- Status: open · Phase 1 · Priority: medium
- Depends on: —
- Evidence: `src/agent/runner/reply.rs:122-142`

**Problem.** `confirms` accepts nine exact phrases. "okay", "sure", "yeah",
"do it", and "please do" are rejected, while "don't do that" is not recognised
as a denial because it normalises to "don t do that". With hands-free voice,
STT phrasing varies, so a correct answer can still be re-asked.

**Decision (2026-10-29).** Classify on the leading phrase, not the whole
utterance. Accept a documented affirmative set and a negation set; denial wins
over approval; a question is never an approval; keep exact matching as one case
of the new table.

**Scope.** In: classification helper and table tests. Out: the "unrelated
input" behaviour (A5) and Sonic's transfer of the decision (A7).

**Acceptance criteria**

- [ ] Affirmatives: yes, yeah, yep, okay, ok, sure, go ahead, do it, please do,
      proceed, confirm.
- [ ] Negatives: no, nope, don't, do not, cancel, stop, deny; including "don't
      do that" and "no thanks".
- [ ] "is it done?" and "what did you delete?" are not approvals.
- [ ] Pure courtesy phrases do not approve when a question is attached.

**Tests**

- [ ] Table-driven cases in `reply.rs`, including the deny-and-instruct case.

**Files.** `src/agent/runner/reply.rs`, `src/agent/runner/sonic.rs`.

## A5 — Unrelated speech cancels the pending action and proceeds

- Status: open · Phase 1 · Priority: high
- Depends on: A4
- Evidence: `src/agent/runner.rs:718-773`

**Problem.** With a pending approval, any utterance that is neither a clear
approval nor a denial returns "I still need a clear yes or no", and the user's
request is dropped. The user ranked this among the top pains: a pending
approval hijacks the conversation.

**Decision (2026-10-29).** Classify first (A4). If the utterance is unrelated:
replace the stored result with "not confirmed; dropped because the user asked
something else", clear the pending state, emit one short note, and continue the
turn with the new input. If the utterance denies and then gives an instruction
("no, open Firefox"), record the denial and handle the instruction.

**Scope.** In: runner branch, note text, tests. Out: scheduled-input deferral
(already correct) and turn-scoped grants (A6).

**Acceptance criteria**

- [ ] Saying "open Firefox" while an approval is pending opens Firefox and
      cancels the action, with a one-line note.
- [ ] "yes" and "no" keep their current behaviour.
- [ ] No stale placeholder is left saying "needs confirmation".
- [ ] The new request runs in the same turn.

**Tests**

- [ ] Armed-approval runner tests for unrelated, denial-plus-instruction, and
      pure yes/no inputs.
- [ ] A batch with a skipped call after the cancellation.

**Files.** `src/agent/runner.rs`, `src/agent/runner/reply.rs`.

## A6 — Allow for this turn

- Status: open · Phase 1 · Priority: high
- Depends on: A3, A7
- Evidence: `src/agent/runner.rs:933-955`, `gnome-extension/hyusk@hyusk.local/lib/responseCard.js`,
  `noctalia-plugin/hyusk.luau`

**Problem.** Every guarded step re-prompts. A task that makes a directory, copies
files, and delegates a workspace task needs three confirmations. The panels only
offer Approve and Deny.

**Decision (2026-10-29).** Add a turn-scoped grant. The card gains "Allow for
this turn"; spoken forms are "allow for this turn" and "yes, for the rest of the
task". A new control action `approve_turn` carries it from GNOME and Noctalia;
the phone link stays once-only. A grant is keyed by `(tool, shape)`, where the
shell shape is the program plus the first subcommand; it is cleared at turn
start and turn end and is never persisted. The card shows what was granted.

**Scope.** In: grant state, control action, GNOME/Noctalia buttons, tests.
Out: session-wide grants, a permissions config file, phone-link protocol change.

**Acceptance criteria**

- [ ] After "allow for this turn", a second matching shell call runs without a
      card.
- [ ] A different program still prompts.
- [ ] The next turn prompts again.
- [ ] Both panels expose the button and the same decision path.
- [ ] A grant never survives a restart.

**Tests**

- [ ] Grant lifecycle unit tests.
- [ ] Control mapping test in `main.rs`.
- [ ] Manual panel check on GNOME and Noctalia.

**Files.** `src/agent/runner.rs`, `src/types/event.rs`, `src/main.rs`,
`src/agent/runtime.rs`, GNOME `responseCard.js`/`indicator.js`, Noctalia
`hyusk.luau`, `docs/noctalia-indicator.md`, `docs/gnome-indicator.md`.

## A7 — One approval card kind; stale approve handling

- Status: open · Phase 1 · Priority: high
- Depends on: —
- Evidence: `src/agent/runner/sonic.rs:402-406`,
  `gnome-extension/hyusk@hyusk.local/lib/responseCard.js:199-203`,
  `noctalia-plugin/hyusk.luau:112`, `src/main.rs:1044-1053`

**Problem.** Sonic publishes card kind `confirmation`; both panels only show
Approve/Deny for `approval`, so the buttons silently disappear. Approve/Deny are
forwarded as the words "yes"/"no", so a click after expiry becomes an ordinary
user message and can produce a new turn or a false answer.

**Decision (2026-10-29).** One shared constant for the approval card kind. The
panel decision is validated against the pending state: a stale click reports
"that confirmation expired" and does not start a turn. Prefer an explicit
`ApprovalDecision { approved, scope }` event over `UserInput("yes")`.

**Scope.** In: constant, event, runtime handling, panel check. Out: the Sonic
voice loop, which only needs the kind change.

**Acceptance criteria**

- [ ] Every mode that publishes an approval uses the same kind.
- [ ] Buttons appear in GNOME and Noctalia for text and Sonic approvals.
- [ ] A click after expiry leaves the conversation untouched.
- [ ] The user is told the confirmation expired.

**Tests**

- [ ] Runtime event test for a decision without a pending approval.
- [ ] Control mapping test.
- [ ] Manual panel check.

**Files.** `src/types/event.rs`, `src/agent/runtime.rs`, `src/main.rs`,
`src/agent/runner/sonic.rs`, both panel sources.

## A8 — Explicit policy registry with exhaustive coverage

- Status: done · 2026-10-29 · commit `38a4ce4` · Phase 1 · Priority: high
- Depends on: —
- Evidence: `src/tools/safety.rs:8-24`, `src/tools/registry.rs`

**Problem.** `requires_approval` returns false for any tool name it does not
know, so a newly registered tool is silently ungated. `browser` is such a tool
today. Policy is spread across per-tool functions with no test that every
registered tool was considered.

**Decision (2026-10-29).** One exhaustive match over tool names, no wildcard;
unknown names fail closed. A test iterates the registry built in `main.rs` and
fails when a tool has no explicit policy. `browser` is classified by D2.

**Scope.** In: table, exhaustive match, coverage test, browser entry. Out:
changing individual decisions (each has its own issue).

**Acceptance criteria**

- [x] Adding a tool without a policy fails a test. `main::build_tools` is the
      only place tools are registered, and
      `tool_policy_tests::every_registered_tool_has_an_explicit_policy` walks
      that registry. Verified by temporarily registering a probe tool: the test
      failed with `tools registered without an approval policy: ["ghost"]`.
- [x] An unknown tool name asks. `requires_approval` returns true when the name
      is not in `ToolName`, and when the arguments are not JSON.
- [x] Every tool registered in `main.rs` has an explicit decision.
- [x] The table feeds A1's generated guidance and A3's preview labels.
      `safety::approval_guidance()` renders one line per row (A1 consumes it);
      A3 derives its preview label from the same row.

**Tests**

- [x] Registry coverage test (`src/main.rs`, `tool_policy_tests`).
- [x] Unknown-name case (`safety::tests::unknown_tool_names_and_bad_arguments_fail_closed`).
- [x] `policy_table_names_every_tool_once_and_matches_its_decision` and
      `generated_guidance_lists_every_policy_row_once`.
- [x] `runner::tests::unknown_tool_name_reports_a_missing_tool_instead_of_arming_an_approval`.

**Files.** `src/tools/safety.rs`, `src/main.rs`, `src/agent/runner.rs`,
`src/agent/runner/sonic.rs`.

**Notes (2026-10-29, implemented).** `requires_approval` is now
`policy_for(name)` plus a per-row classifier; `ToolName` is an enum with
`ALL`, `name()`, `from_name()` and an exhaustive `policy()` match with no
wildcard, so a new tool cannot compile without a decision. Each row carries the
model-facing guidance so prose and gate cannot drift. `main.rs` grew
`ToolSetup`/`build_tools` so a test can walk the registry the composition root
actually builds.

Drift found while implementing:

- The Evidence range (`safety.rs:8-24`) was exact; the file has moved on and the
  gate now sits near `safety.rs:215`.
- `browser` is recorded as `Decision::Allow`. The Problem and Decision text say
  D2 classifies it while Scope says individual decisions are out of scope; the
  row makes today's effective decision explicit and leaves the classification to
  D2. Nothing about browser calls changed.
- Making unknown names ask would have let a hallucinated tool name arm a
  confirmation for a call that cannot run. The text and Sonic paths now report
  "Tool 'x' does not exist." before arming; the gate itself still asks for the
  unknown name.
- `mobile` is registered only while the opt-in link server runs, so the coverage
  test cannot construct it; its row is asserted directly in
  `tool_policy_tests`.

## A9 — Fast-command lane uses the same policy

- Status: open · Phase 2 · Priority: medium
- Depends on: A8
- Evidence: `src/agent/runner.rs:438-571`

**Problem.** `try_fast_command` executes plan actions without consulting the
decision function. Today the vocabulary is limited to fixed strings and
configured apps, but any future action, or a fuzzy workflow match, inherits the
bypass. The model, fast, and workflow lanes should not disagree.

**Decision (2026-10-29).** Route each action through the decision before
execution. If any step asks, stop before the first side effect and hand the
utterance to the model path so the runtime can hold and confirm it.

**Acceptance criteria**

- [ ] A plan containing a guarded action executes nothing and defers to the
      model path.
- [ ] A three-lane parity test covers an identical request.

**Tests**

- [ ] Fast-path test with a guarded action.
- [ ] Workflow lane comparison.

**Files.** `src/agent/runner.rs`, `src/agent/runtime.rs`,
`src/agent/commands.rs`.

## A10 — Approval audit log

- Status: open · Phase 3 · Priority: low
- Depends on: A3
- Evidence: `src/status.rs`, `src/agent/runner.rs` (action journal)

**Problem.** The action journal records in-flight and completed calls, not the
decisions the user made. There is no way to review what was approved and when.

**Decision (2026-10-29).** Append one JSON line per decision to a file in the
data directory: timestamp, tool, preview, decision, scope, turn id. No secrets;
the preview follows X2's redaction. Rotate by size.

**Acceptance criteria**

- [ ] Every approval, denial, expiry, and stale click is recorded once.
- [ ] The log contains no secret-shaped values.
- [ ] Rotation is bounded.

**Tests**

- [ ] Writer unit tests with a temporary directory.
- [ ] Redaction case.

**Files.** `src/agent/runner.rs`, `src/agent/runtime.rs`, a small writer module.
