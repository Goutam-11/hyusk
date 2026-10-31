# The propose-diff loop

This document is the written rule for how the agent may change the product it
runs on. It implements `G2` (recorded in
[issues/README.md](../issues/README.md)) and the
"Upgrade" decision in
[docs/architecture-roadmap.md](architecture-roadmap.md): **human-reviewed
releases; the agent proposes diffs, it does not merge or install them.**

The running `hyusk.service` is the user's production assistant. A change that
ships unreviewed can alter what the agent may approve, where it may write, or
which binary starts at login. The loop keeps those decisions with a human even
when the agent does the work.

## Roles

| Role | Who | May |
| --- | --- | --- |
| Proposer | The agent | Read, edit a branch, run checks, commit, and present the diff and evidence. |
| Reviewer | A human | Read the diff, ask for changes, approve or reject. |
| Merger | A human | Merge to `main` (or squash), tag a release if the repository does. |
| Supervisor | systemd or a human | Restart the user service after the merge. `systemd` runs `Restart=on-failure`; a person decides when to restart a healthy service. |
| Installer | A human | Run `scripts/install-autostart.sh` (rebuild and install) or roll back. |

The agent must never be the reviewer, merger, installer, or the thing that
decides a restart. It may *ask* for one.

## The loop

1. **Claim one item.** Read `AGENTS.md`, then the issue in full (the closed
   2026 tracks are summarised in `issues/README.md`; new work gets its own
   `issues/<AREA>_ISSUE.md`). Work one item per branch, as `AGENTS.md`
   requires. If the issue needs a human-only file (below), stop at step 6 and
   propose rather than edit.
2. **Branch.** From `main`, create a topic branch, for example
   `git switch -c fix/shell-path-escape`. Never commit to `main` directly and
   never rewrite `main`.
3. **Change the smallest thing.** Implement only that issue. Add or update the
   tests it names, confirm the evidence references against the code, and record
   line drift in the issue.
4. **Verify** with the commands in [development.md](development.md):
   - `cargo fmt --all -- --check`;
   - `cargo check --all-targets --offline`;
   - `cargo clippy --all-targets --offline` (no new warnings);
   - the isolated test command from `AGENTS.md` (never a bare `cargo test`);
   - every feature build the change touches: `--features mobile`,
     `--features sonic`, `--features workflow-editor`, and `--all-features`
     when the change is cross-cutting.
   A documentation-only change runs fmt plus the review; it still names the
   test counts it did not touch.
5. **Commit for review.** Use the log's style: `feat(scope): ... (ID)`,
   `fix(scope): ... (ID)`, `chore(build): ... (ID)`, or `docs: ...`, then a
   small `docs(issues): record the <ID> closing commit` with the real hash.
   Do not tag, do not merge, and do not push to `main`.
6. **Propose.** Present the branch or patch with a short message:
   - the issue and what changed, in one paragraph;
   - the files and any deviation from the issue;
   - the checks run with their counts, and any fail-before-fix probe;
   - the manual checks not run and why;
   - the risk and the rollback.
7. **Review.** The human reads the diff, checks the invariants in `AGENTS.md`,
   and either asks for changes or approves. CI is another check, not the
   authority: a green run does not replace the review.
8. **Merge.** A human merges. The agent does not run `git merge`, `git push`
   to `main`, or `gh pr merge`, even when it has the credentials.
9. **Restart under supervision.** After the merge, a human or the supervisor
   rebuilds if needed and restarts:
   `systemctl --user restart hyusk.service` (or
   `scripts/install-autostart.sh`, which builds and restarts). The agent does
   not run either.
10. **Check and roll back if needed.** Confirm the service is up and the new
    build answers. If it regresses, revert the merge commit and restart. The
    memory store keeps its own backups; the session file is rebuilt from the
    previous snapshot.

## Human-only files

These files are the boundary the loop protects. The agent does not change them
as part of an issue. If a change is genuinely required there, the agent stops
and writes the proposal; a human reviews and commits it, or explicitly
authorises a prepared diff in the issue. Even with that authorisation, the
agent does not merge, install, or restart.

**The approval model**

- `src/tools/safety.rs` - the policy table, every classifier, the consent
  words, and the preview. A change here could make a consequential call free.
- `src/agent/control.rs` - the armed confirmation, `pending`, and the state the
  runtime shares while a turn holds the agent mutex.
- `src/agent/runner.rs` - the approval flow: when a call is armed, how the
  stored arguments are executed, and how a grant is dropped.
- `src/agent/runner/reply.rs` - the confirmation and denial grammar and the
  "allow for this turn" grant text.
- `src/agent/runner/session.rs` - the restart settlement of an interrupted
  confirmation (`Pending confirmation for:` placeholders).
- `src/agent/audit.rs` - the approval audit log and its redaction.

**The update path**

- `scripts/install-autostart.sh` and `scripts/uninstall-autostart.sh` - the
  build/install/service wiring.
- `systemd/` - the user unit and its drop-ins.
- `.github/workflows/` - what CI builds and publishes.
- Release or profile changes in `Cargo.toml` that alter how the shipped binary
  is built. Adding a normal feature or dependency is ordinary work; changing
  the build/install/update shape is not.

**Secrets and policy**

- `src/credentials.rs`, the keyring paths, `.env`, and anything that moves a
  provider key.
- The safety invariants in `AGENTS.md` and this document. The agent may update
  an issue's status as part of its work; it does not weaken the rules it works
  under.

Why these and not others: a change here could let the agent approve its own
actions, silently widen a gate, hide a decision from the audit, or install
unreviewed code. Those are exactly the outcomes the loop exists to prevent.

## What the agent may change

Ordinary source, tests, fixtures, documentation, `CHANGELOG.md`, issues and
their trackers, and tool modules - the work the closed `issues/` trackers
described. It may add a
feature (with its `dep:` entries and optional dependencies), add a tool with
its manifest and policy row, or change a non-installer script such as a wake
trainer or a browser helper. It may not delete or skip a test to make a build
pass, and it may not turn off a gate.

## A proposal template

```text
G2 proposal: <issue ID> - <one-line title>
Branch: <branch name>
Changed: <files or areas>
Behaviour: <what the user or agent observes differently>
Evidence: <the issue's evidence lines, with drift noted>
Checks: fmt/check/clippy; <feature> suite N passing, M ignored;
        probe <what it proved>; manual: <not run, why>
Risk: <what could regress>; rollback: revert <hash> and restart.
```

## Review checklist

- The diff is limited to the issue; no silent extras.
- No human-only file is touched, or the human authored that part.
- Every new tool has a manifest and a policy row; every external one stays in
  `ext.`; no gate got weaker.
- The tests named in the issue exist, and any fail-before-fix probe is real.
- The counts in `AGENTS.md`, `docs/development.md`, and the issue are updated.
- Redaction, the audit, and the private-file rules still hold for any new sink.
- The restart story is clear: what an in-flight approval does (it is dropped
  and settled), and what the user must redo.

## What this is not

This is not a sandbox or a restriction on the agent's shell and computer tools;
their approval policy still applies to every call. It is also not a claim that
the agent cannot edit its own source in a workspace - it may prepare a diff -
but it does not review, merge, install, or restart it. A human owns those
steps, and the binary that starts at login is the one a human shipped.

## Related

- [`AGENTS.md`](../AGENTS.md) - the invariants and the checks named here.
- [`docs/development.md`](development.md) - the exact commands.
- [`issues/README.md`](../issues/README.md) - `G2` and the closed-work
  record.
- [`docs/architecture-roadmap.md`](architecture-roadmap.md) - the "Upgrade"
  decision and the deferred list.
