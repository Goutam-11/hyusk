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

- Status: open · Phase 1 · Priority: high
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

- [ ] `build_system_prompt` contains one guidance block derived from the policy
      table, not a hand-maintained list.
- [ ] No prompt text claims an ungated class or omits a gated one.
- [ ] On a guarded request the model calls the tool once, without a spoken
      "should I?", and repeats the runtime's pending-call text (A3).
- [ ] Routine work (`rg`, `sed -n`, `git status`, `cargo test`) is named as free,
      matching the classifier.

**Tests**

- [ ] Prompt contains every policy row from the A8 table.
- [ ] The retired phrases are absent.
- [ ] Manual voice check: "delete the build folder" asks once.

**Files.** `src/agent/runner/prompts.rs`, `src/tools/safety.rs`,
`src/agent/runner.rs`.

## A2 — Research delegation runs without approval

- Status: open · Phase 1 · Priority: high
- Depends on: A8
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

- [ ] `task` with `profile: "research"` runs immediately.
- [ ] `task` with `profile: "workspace"` still asks once and pins the root.
- [ ] Missing Codex or `HYUSK_WORKSPACE` returns a clear failure and no approval
      card is published.

**Tests**

- [ ] Safety table cases for both profiles.
- [ ] Task preflight failure path (bogus binary or missing workspace).

**Files.** `src/tools/safety.rs`, `src/tools/task.rs`, `src/agent/runner.rs`.

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

- Status: open · Phase 1 · Priority: high · Do this first
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

- [ ] Adding a tool without a policy fails a test.
- [ ] An unknown tool name asks.
- [ ] Every tool registered in `main.rs` has an explicit decision.
- [ ] The table feeds A1's generated guidance and A3's preview labels.

**Tests**

- [ ] Registry coverage test.
- [ ] Unknown-name case.

**Files.** `src/tools/safety.rs`, `src/main.rs`, `src/tools/registry.rs`.

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
