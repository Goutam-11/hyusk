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
- [ ] Manual voice check: "delete the build folder" asks once. Still not run:
      it needs a live microphone, STT, and a provider. On 2026-10-30 the agent's
      environment was checked again - the desktop session is niri (Wayland) and
      `/dev/snd` is not present inside the agent sandbox, so the voice path
      cannot be exercised from the agent's shell at all. This one needs the user
      at the microphone with the service running.

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

- Status: done · 2026-10-29 · commit `e3d85de` · Phase 1 · Priority: high
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

- [x] Card and tool result show the same tool and arguments. Both are built
      from one function, `safety::pending_text(preview)`; the text card uses it
      in `runtime::card_text`, the Sonic card uses it directly, and the tool
      result prefixes the same sentence.
- [x] Long commands truncate predictably with an ellipsis:
      `safety::PREVIEW_LIMIT` is 120 characters including the ellipsis, counted
      in characters rather than bytes.
- [x] The stored preview is what the user approves; argument substitution after
      approval is impossible. The preview is rendered from
      `approval_arguments`, which are the arguments stored for execution, so the
      previewed call and the approved call are the same JSON.
- [x] Speech names the tool and the first few argument words: the tool result
      and the generated prompt block both require the model to repeat
      "Pending confirmation for: …". A live voice check is still outstanding,
      as in A1.

**Tests**

- [x] Preview formatting for `shell`, `process`, `task`, `mobile`, `window`
      (`safety::tests::approval_preview_names_the_tool_and_its_key_arguments`).
- [x] Truncation and empty-argument cases
      (`approval_preview_is_one_line_and_bounded`), plus
      `pending_text_is_the_one_sentence_both_surfaces_use`.
- [x] Runtime card text uses the preview
      (`runtime::tests::approval_card_shows_the_exact_call_not_the_models_paraphrase`).
- [x] End to end:
      `runner::tests::guarded_call_is_held_once_and_the_pending_call_names_it`
      drives a guarded `shell` call and checks the stored preview, the tool
      result, and that nothing ran.

**Files.** `src/tools/safety.rs`, `src/agent/runner.rs`,
`src/agent/runner/sonic.rs`, `src/agent/runtime.rs`,
`src/agent/runner/prompts.rs`, `docs/tools.md`, `docs/voice-and-ui.md`.

**Notes (2026-10-29, implemented).** The A8 policy rows gained `preview_keys`,
so each tool names its identifying arguments in a stable order (the policy
order first, then any remaining arguments alphabetically). The preview is one
line: whitespace collapses, arrays render as `[a, b]`, empty and null values are
skipped, and arguments that are not a JSON object render as the tool name alone
- such a call cannot run, and the gate still holds it.

The Sonic card keeps its existing `confirmation` kind; A7 owns the card-kind
change, so only the text moved here.

## A4 — Confirmation grammar for voice

- Status: done · 2026-10-29 · commit `a170a53` · Phase 1 · Priority: medium
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

- [x] Affirmatives: yes, yeah, yep, okay, ok, sure, go ahead, do it, please do,
      proceed, confirm - and the retired exact sentences still approve.
- [x] Negatives: no, nope, don't, do not, cancel, stop, deny; including "don't
      do that" and "no thanks". Apostrophes are removed before matching, so
      both "don't" and "don’t" classify instead of normalising to "don t".
- [x] "is it done?" and "what did you delete?" are not approvals, with or
      without the question mark.
- [x] Pure courtesy phrases do not approve when a question is attached: "okay"
      approves, "okay, but what does it do?" does not.

**Tests**

- [x] Table-driven cases in `reply.rs`, including the deny-and-instruct case
      (`confirmation_grammar_accepts_documented_affirmatives`,
      `confirmation_grammar_rejects_denials`,
      `denial_wins_over_a_leading_approval`,
      `questions_and_courtesy_are_never_approvals`,
      `stopping_listening_is_a_denial_and_still_ends_the_session`).
- [x] The pre-existing `confirmation_is_explicit_and_action_specific` and
      `voice_session_stop_is_exact_not_part_of_a_task` still pass unchanged.

**Files.** `src/agent/runner/reply.rs`, `docs/voice-and-ui.md`.

**Notes (2026-10-29, implemented).** `confirms` now reads the leading phrase
against a documented affirmative table, and `is_denial` does the same against a
negation table; `normalized_decision_phrase` strips apostrophes before splitting
so contractions match. A leading approval is still refused when any later token
negates it ("yes, but not yet", "okay, wait") or when the transcript carries a
question mark, because acting on an ambiguous reply is worse than re-asking.
Each check is a separate case of one table-driven classifier rather than an
exact-sentence match.

`stop listening` now also reads as a denial; the runtime still ends the voice
session before the turn runs, so the approval branch only sees it if the
session is already finished. The Sonic loop uses the same shared grammar, so it
improves too; A7 still owns moving the decision off the transcript there.

Evidence drift: the old nine-sentence match and the seven-phrase denial match
were at `reply.rs:122-142` when this was written; the tables now start at
`reply.rs:123` and the classifiers at 224 and 240.

## A5 — Unrelated speech cancels the pending action and proceeds

- Status: done · 2026-10-29 · commit `13d955d` · Phase 1 · Priority: high
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

- [x] Saying "open Firefox" while an approval is pending opens Firefox and
      cancels the action, with a one-line note. The placeholder records the
      drop, "I dropped that pending action because you asked something else."
      reaches the speech sink, and the model receives "open Firefox".
- [x] "yes" and "no" keep their current behaviour: "yes" runs the stored call
      once, a bare "no" answers "Understood. I did not perform that action.",
      and neither reaches the model more than the turn needs.
- [x] No stale placeholder is left saying "needs confirmation": the stored tool
      result is replaced in place.
- [x] The new request runs in the same turn: the drop branch falls through to
      the normal model loop instead of returning early.

**Tests**

- [x] Armed-approval runner tests:
      `unrelated_request_drops_the_pending_action_and_runs_in_the_same_turn`,
      `denial_with_an_instruction_denies_and_answers_the_instruction`,
      `bare_denial_keeps_the_short_refusal_reply`,
      `plain_yes_executes_the_stored_call_once`, and
      `cancelled_turn_keeps_the_confirmation_armed`.
- [x] A batch with a skipped call after the cancellation
      (`a_batch_keeps_its_skipped_call_after_the_cancellation`).
- [x] `reply::tests::a_denial_can_carry_an_instruction` covers the split itself.
- [x] The unrelated and batch tests fail when the drop branch is disabled.

**Files.** `src/agent/runner.rs`, `src/agent/runner/reply.rs`,
`docs/voice-and-ui.md`, `docs/tools.md`.

**Notes (2026-10-29, implemented).** `reply::denial_instruction` splits
"no, open Firefox" at a clause boundary (or strips the leading denial phrase
when there is no punctuation) and ignores filler such as "no, thanks", so the
runner can tell a bare refusal from a refusal plus a request. The denial branch
records the refusal, then continues the turn with the instruction as the new
input, so the model answers "open Firefox" without the leading "no" confusing
it.

The drop branch replaces the placeholder with "Not confirmed: this action was
dropped because the user asked something else. Do not execute it; answer the new
request instead.", sends one short note to the speech sink, and falls through to
the model loop with the user's new request.

Decisions beyond the letter of the issue:

- A cancelled turn keeps the confirmation armed (the old re-ask path). The user
  never answered, so dropping the call there would lose an approved action to a
  barge-in.
- The note goes to the speech sink rather than the conversation, so the model
  does not see a second assistant message before the user's request; the model
  learns about the drop from the tool result.

Evidence drift: the old `if let Some(pending)` block sat at `runner.rs:718-773`;
`handle` now starts at 715 and the branch spans roughly 733-830.

**Manual check outstanding.** The voice path was exercised through the speech
sink in tests, but no live microphone/STT/provider session was run.

## A6 — Allow for this turn

- Status: done · 2026-10-29 · commit `d6b4cab` · Phase 1 · Priority: high
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

- [x] After "allow for this turn", a second matching shell call runs without a
      card
      (`runner::tests::a_turn_grant_covers_matching_calls_and_expires_with_the_turn`
      drives a real two-round turn).
- [x] A different program still prompts
      (`a_turn_grant_does_not_cover_a_different_program`).
- [x] The next turn prompts again: the grant vector is cleared at turn start and
      turn end, asserted in the same lifecycle test.
- [x] Both panels expose the button and the same decision path: the GNOME card
      and the Noctalia Agent tab send `approve_turn`, which `main.rs` maps to
      `ApprovalDecision { approved: true, scope: Turn }`; the runtime turns that
      into the same spoken phrase the microphone path uses.
- [x] A grant never survives a restart: it is in-memory only, cleared at the
      turn boundary, and never passed to `save_session`.

**Tests**

- [x] Grant lifecycle unit tests: the two-arm tests above,
      `a_once_only_approval_grants_nothing`, and
      `turn_grants_are_keyed_by_shape_and_cleared_by_the_turn_boundary`.
- [x] Shape rules: `safety::tests::a_turn_grant_is_keyed_by_tool_and_shape`.
- [x] Control mapping test in `main.rs`
      (`approve_and_deny_map_to_an_approval_decision_not_a_user_message` now
      covers `approve_turn` → `ApprovalScope::Turn`).
- [x] Spoken grammar: `reply::tests::turn_approvals_cover_the_rest_of_the_turn`.
- [ ] Manual panel check on GNOME and Noctalia: not run; it needs a live Shell
      extension, the widget, and a desktop session. Checked on 2026-10-30: the
      extension is installed (`~/.local/share/gnome-shell/extensions/hyusk@hyusk.local`)
      but the current session is niri, so neither `org.gnome.Shell` nor
      `org.hyusk.Shell` is on the session bus, and Noctalia is not installed
      (`~/.config/noctalia` is absent). The check needs a GNOME login with the
      extension enabled, or a Noctalia bar session.

**Files.** `src/agent/runner.rs`, `src/agent/runner/reply.rs`,
`src/tools/safety.rs`, `src/types/event.rs`, `src/main.rs`,
`src/agent/runtime.rs`, GNOME `responseCard.js`, Noctalia `settings.luau`,
`docs/noctalia-indicator.md`, `docs/gnome-indicator.md`,
`docs/voice-and-ui.md`, `docs/tools.md`.

**Notes (2026-10-29, implemented).** The grant is a `Vec<(tool, shape)>` on the
agent, keyed by `safety::grant_key`. The shell shape is the program plus its
first subcommand ("git push", "rm"), program paths are normalised, other tools
use their `action`, and `task` uses its profile. A call whose shape cannot be
determined (unparseable arguments, chained shell) is never granted, and
`grant_for_turn` also publishes a card naming what was allowed.

`handle` is now the turn boundary: it clears the grants before the input is
interpreted and after the turn returns, so a grant cannot leak into the next
turn. The runner consults the grant in the tool loop, so a matching call skips
the card without a second confirmation, and `runtime::apply_approval_decision`
maps the panel's `Turn` scope onto the same phrase the voice path uses
(`runner::ALLOW_FOR_THIS_TURN`), keeping one decision path for both.

Drift found:

- The issue lists Noctalia `hyusk.luau`; the panel's Approve/Deny buttons live
  in `noctalia-plugin/settings.luau` (the bar widget has no approval controls).
  The new button went there, and the bar widget needs no change.
- The phone link keeps `ApprovalScope::Once` explicitly, as the decision says.

## A7 — One approval card kind; stale approve handling

- Status: done · 2026-10-29 · commit `7efd464` · Phase 1 · Priority: high
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

- [x] Every mode that publishes an approval uses the same kind:
      `status::APPROVAL_CARD_KIND` is used by the text turn card, the Sonic
      approval card, the Sonic post-turn card, and the Sonic listen-completion
      card. The three Sonic sites previously published `confirmation`.
- [x] Buttons appear in GNOME and Noctalia for text and Sonic approvals: both
      panels already key on `approval`, so the kind fix is what restores them.
      The manual panel check below is still outstanding.
- [x] A click after expiry leaves the conversation untouched: a panel decision
      is validated against the armed confirmation, and a stale one publishes an
      expiry card and starts no turn.
- [x] The user is told the confirmation expired: `STALE_APPROVAL_REPLY` reaches
      both the card and the spoken response.

**Tests**

- [x] Runtime event test for a decision without a pending approval
      (`runtime::approval_decision_tests::a_decision_without_a_pending_approval_reports_expiry`,
      plus `an_armed_confirmation_accepts_the_decision_as_turn_input`).
- [x] Control mapping test
      (`control_tests::approve_and_deny_map_to_an_approval_decision_not_a_user_message`),
      plus `the_approval_card_kind_matches_what_both_panels_expect`.
- [ ] Manual panel check on GNOME and Noctalia: not run in this session; it
      needs a live Shell extension, the Noctalia widget, and a desktop session.
      Rechecked on 2026-10-30: extension installed, session is niri, no Shell
      name on the bus, Noctalia absent - see the A6 note above.

**Files.** `src/types/event.rs`, `src/status.rs`, `src/agent/runtime.rs`,
`src/main.rs`, `src/agent/runner/sonic.rs`, both panel sources.

**Notes (2026-10-29, implemented).** `HyuskEvent::ApprovalDecision { approved }`
replaces the "yes"/"no" `UserInput` conversion in the control-file consumer and
in the phone-link path, so a click can no longer be mistaken for speech. The
runtime validates the decision with a short bounded wait for the agent lock,
because the lock is held for a whole turn: if it stays busy (a Nova Sonic
session holds it for the whole interaction) the decision falls back to the
pre-A7 forwarding, which A7's scope leaves to the Sonic loop.

`scope` is not in the event yet: the only scope today is once-only, and A6
introduces the turn-scoped grant that needs it. Adding an unused field now would
have been dead weight.

The expiry notice is now a `response` card rather than an `approval` card, so
the panels stop offering Approve/Deny for a decision that no longer exists.

Evidence drift: the Sonic card moved from `sonic.rs:402-406` to 410-420, the
control mapping from `main.rs:1044-1053` to 1093-1100, and the panel checks are
unchanged at `responseCard.js:199` and `hyusk.luau:112` (now with a comment
naming the shared tag).

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

- Status: done · 2026-10-29 · commit `2ee83a6` · Phase 2 · Priority: medium
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

- [x] A plan containing a guarded action executes nothing and defers to the
      model path.
- [x] A three-lane parity test covers an identical request
      (`the_model_fast_and_workflow_lanes_agree_on_a_guarded_request`).

**Tests**

- [x] Fast-path test with a guarded action
      (`fast_command_defers_a_guarded_plan_before_the_first_side_effect`:
      `mute`, and a chain that mixes `mute` with the free `volume up`, run
      nothing).
- [x] Workflow lane comparison in the parity test, which also asserts the
      model lane's gate asks for the same call.

**Files.** `src/agent/runner.rs`, `src/agent/runtime.rs`, `README.md`,
`AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-29, implemented).** `try_fast_command` checks every action of
the parsed plan with `safety::requires_approval(action.tool_name(),
&action.tool_input())` after the "tools are registered" check and before the
cancellation, journal, or execution loops. A single guarded step defers the
whole plan: `None` tells the runtime to run the utterance through the model
path, where the usual card is armed, so no part of a mixed chain runs before the
decision. The check is a plain `any`, which keeps it cheap and side-effect free.

The commands that now defer are the `shell` ones in the vocabulary: `mute` and
`unmute` (`pactl`), `brightness up`/`down` (`brightnessctl`), and `lock the
screen` (`loginctl`). Everything else in the vocabulary is already free under
the table. No vocabulary was changed, so `commands.rs` needed no code change.

Drift found in the docs while verifying the lanes: README listed `lock the
screen` as an instant command and claimed workflows can change the brightness
and lock the session, while the shell policy asks for both and a workflow stops
before a gated step. Those three claims were corrected. Evidence line drift:
`try_fast_command` now starts at `runner.rs:496`, not `438`.

## A10 — Approval audit log

- Status: done · 2026-10-30 · commit `88132e7` · Phase 3 · Priority: low
- Depends on: A3
- Evidence: `src/agent/audit.rs` (new writer), `src/agent/runner.rs`
  (`run_turn`, `expire_stale_approval`), `src/agent/runtime.rs`
  (`apply_approval_decision`)

**Problem.** The action journal records in-flight and completed calls, not the
decisions the user made. There is no way to review what was approved and when.

**Decision (2026-10-29).** Append one JSON line per decision to a file in the
data directory: timestamp, tool, preview, decision, scope, turn id. No secrets;
the preview follows X2's redaction. Rotate by size.

**Acceptance criteria**

- [x] Every approval, denial, expiry, and stale click is recorded once.
- [x] The log contains no secret-shaped values.
- [x] Rotation is bounded.

**Tests**

- [x] Writer unit tests with a temporary directory
      (`a_decision_is_one_private_json_line_without_secrets`,
      `an_approved_call_records_its_scope_and_turn`,
      `a_stale_click_names_no_call_and_no_turn`,
      `rotation_keeps_the_live_file_and_one_previous_copy`,
      `a_missing_parent_directory_is_created_private`,
      `every_outcome_and_scope_has_a_name`).
- [x] Redaction case (in the first writer test and in
      `tools::redact::tests::assignments_with_secret_names_are_redacted_in_place`).
- [x] Decisions reach the file from the runtime
      (`agent::runner::tests::every_approval_decision_is_audited_once`,
      `agent::runtime::approval_decision_tests::a_stale_click_is_audited_without_a_call_or_a_turn`).

**Notes (2026-10-30, implemented).** `src/agent/audit.rs` writes one JSON line per
decision to `approvals.jsonl` in the data directory (`$XDG_DATA_HOME/hyusk`,
directory `0700`, file `0600`, restricted after every append). The line carries
`timestamp`, `tool`, `preview`, `decision`, `scope`, and `turn`; a field is
`null` when it does not apply. The writer creates the directory private, rotates
when the live file reaches `MAX_BYTES` (256 KiB) by moving it to
`approvals.jsonl.1` and replacing any previous copy, and reports a failed write
as `[Audit] …` on stderr without failing the turn.

Recording points, each at the one place that resolves the stored call:

- `run_turn` records `denied` and `dropped` (the user asked for something else)
  and `approved` with the scope the answer grants (`once` or `turn`).
- `expire_stale_approval` records `expired`, whether a turn or the expiry timer
  noticed it.
- `runtime::apply_approval_decision` records `stale` when nothing was armed, with
  the click's scope and no call or turn.

Five judgement calls, recorded because the decision left them open:

- **`dropped` is a fifth outcome.** The decision names approval, denial, expiry
  and a stale click; "the user asked something else, so the call was dropped
  unrun" is a distinct decision the audit would otherwise hide. It is recorded
  with no scope.
- **A turn id needs a label, not a second counter.** The runtime still owns
  `next_turn_id`; it now sets `AgentControl.turn` for the duration of a turn
  through a small RAII guard (`TurnLabel`), so every early return of the turn
  task clears it and a decision the event loop resolves afterwards is recorded
  without a turn. A panel click is recorded with no turn even when a turn is
  running, because it is not that turn's decision.
- **The preview is scrubbed on the way out, not at the source.** Values go
  through X2's pass and credential paths through `redact_secret_paths`, the same
  pair `status::card` uses, because the log outlives the turn.
- **The write is synchronous.** It is one small append per card, not per tool
  call; L2's coalescing writer can absorb it later, and nothing waits on it.
- **Test builds never touch the data directory.** `default_path()` is compiled to
  the scratch file under `cfg(test)`, so a parallel test cannot write the real
  log whichever test runs first; the writer tests pass explicit paths.

Discovery beyond the issue: the acceptance exposed a gap in X2's pass. A
rendered call wraps its arguments as `key=value`, so `shell
command=OPENROUTER_API_KEY=sk-…` defeated both rules - the assignment matcher
saw the non-secret name `command`, and the token scanner saw one token starting
with the key name. `=` is now a token delimiter, so a credential after any inner
`=` becomes its own token and is redacted. That also closes the same gap for
status cards (X4's sink) and tool output; the redaction test covers the shape.

Drift: the Evidence was `src/status.rs` and "`src/agent/runner.rs` (action
journal)", module references rather than a range; the writer and the four
recording points are named above instead. Not covered by an automated test: the
non-test `default_path()` (`$XDG_DATA_HOME/hyusk/approvals.jsonl`), because the
test build resolves to the scratch log by construction.

**Files.** `src/agent/audit.rs`, `src/agent/mod.rs`, `src/agent/runner.rs`,
`src/agent/runtime.rs`, `src/agent/control.rs`, `src/tools/redact.rs`, docs.
