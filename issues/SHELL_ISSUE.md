# Shell policy issues

`shell` runs `sh -c <command>` with the user's privileges. `safety.rs`
classifies a command by parsing one simple command (no pipes, redirects,
chaining, substitutions) and checking the program name against a small
allowlist. Anything else asks for approval. This is why `mkdir`, `cp`, `touch`,
and `mv` prompt while `cat` and `rg` do not.

## S1 — Project-scoped shell writes

- Status: done · 2026-10-29 · commit `1c8ba6f` · Phase 1 · Priority: high
- Depends on: A8
- Evidence: `src/tools/safety.rs:60-154`

**Problem.** The allowlist is keyed by program only and has no notion of a
project. File operations that are routine while working on a project prompt
every time, which is the second reported pain. The same policy also lets
`cat`/`head`/`tail` read any file, so the gate is both too strict for writes and
too loose for reads (S2).

**Decision (2026-10-29).** The project root is `HYUSK_WORKSPACE` when set,
otherwise the process working directory, canonicalised once per decision. File
operations run free only when every path argument resolves inside that root.
Destructive commands always ask. Paths given as `~` are expanded for the check;
`$VAR` and command substitution already fail the simple-command parser and ask.

**Scope.** In: `mkdir`, `touch`, `cp`, `mv`, `ln`, `install` with all paths
inside the root; conservative argument parsing. Out: package managers, builds,
network fetches, VCS commands, and `rm` (all still ask). `sed` keeps its current
read-only rule.

**Acceptance criteria**

- [x] `mkdir -p build`, `cp a b`, `mv a b`, `touch x`, `ln -s a b` inside the
      root do not ask.
- [x] The same commands with a target outside the root ask.
- [x] `rm`, `rmdir`, `shred`, `truncate`, `chmod`, `chown` always ask.
- [x] `cp -t /outside a` and `cp --target-directory=/outside a` ask.
- [x] `cp ~/notes.txt .` is evaluated against the expanded home path and asks.
- [x] With `HYUSK_WORKSPACE` unset, the working directory is the root.

**Tests**

- [x] Temp-directory fixture with an explicit root
      (`safety::tests::project_file_operations_are_free_only_inside_the_root`
      builds a real root and passes both root and working directory
      explicitly, so no test mutates the environment).
- [x] Table of allow and ask cases, including `--`, `-t`, `~`, and `..`
      traversal.
- [x] Root selection precedence test
      (`the_project_root_is_hyusk_workspace_else_the_working_directory`).
- [x] `destructive_programs_always_ask`.

**Files.** `src/tools/safety.rs`, `docs/tools.md`.

**Notes (2026-10-29, implemented).** `PROJECT_FILE_OPERATIONS` lists the six
programs; `DESTRUCTIVE_PROGRAMS` names the always-ask set explicitly so the
catch-all is not the only thing refusing them. `file_operation_paths` parses
each program's known flags, treats `-t`/`--target-directory` and `touch -r` as
paths, consumes value options such as `-m`/`--mode` without treating them as
paths, and returns `None` for any option it does not know, which asks.

`path_inside_root` expands `~` and `~/…` (a bare `~user` is not expanded and
asks), makes the path absolute against the working directory, canonicalises the
deepest existing ancestor so symlinks and `..` are resolved, then normalises the
remaining components lexically and compares with the canonical root. A root
that cannot be canonicalised is not trusted, so the call asks.

Drift: the Evidence range (`safety.rs:60-154`) still described the pre-A8
classifier; the arm now lives in the `match program` inside
`shell_requires_approval`.

## S2 — Read confinement and secret denylist

- Status: done · 2026-10-29 · commit `PENDING` · Phase 2 · Priority: high
- Depends on: S1
- Evidence: `src/tools/safety.rs:78-79`, `AGENTS.md` known issues

**Problem.** `cat`, `head`, `tail`, `grep`, `rg`, `find`, `file`, `stat`,
`git show`, and `git log -p` can read arbitrary files and the output goes to the
remote provider. `.env`, SSH keys, keyrings, and browser profiles are all
reachable without approval.

**Decision (2026-10-29).** Reads are free inside the project root; reads outside
ask; secret-shaped paths are refused outright, not merely confirmed. Refusal
message names the rule, not the secret. The denylist is a compiled pattern list
so it can grow without touching the classifier.

**Scope.** In: path extraction for the read allowlist, root check, denylist,
tests. Out: output redaction (X2), which handles values that arrive from
elsewhere.

**Acceptance criteria**

- [x] `cat src/main.rs` and `rg "tool" src` are free.
- [x] `cat ../other-project/file` asks.
- [x] `cat .env`, `cat ~/.ssh/id_ed25519`, `rg token ~/.config/gh` are refused
      with a policy message that does not echo a secret.
- [x] `find / -name '*.pem'` is refused or asks before output.
- [x] The denylist covers `.env*`, `.ssh`, `.gnupg`, `.aws`, `.kube`, `.netrc`,
      `.config/gh`, keyrings, and the managed browser profile directory.

**Tests**

- [x] Denylist and outside-root tables in a temp fixture
      (`reads_are_free_only_inside_the_root`,
      `secret_paths_are_refused_without_echoing_them`,
      `repository_reads_stay_free_and_escapes_ask`).
- [x] A path-traversal case (`..`) is resolved before the check, including a
      symlink out of the root.
- [x] End-to-end: `agent::runner::tests::
      credential_read_is_refused_instead_of_armed` and
      `agent::runtime::tests::
      workflow_refuses_a_credential_read_before_holding_it_for_approval`.

**Files.** `src/tools/safety.rs`, `src/agent/runner.rs`,
`src/agent/runner/sonic.rs`, `src/agent/runtime.rs`, `docs/tools.md`,
`AGENTS.md`, `CHANGELOG.md`.

**Notes (2026-10-29, implemented).** `READ_PROGRAMS` lists the file-reading
programs. `read_command_paths` extracts their operands: `grep` and `rg` skip the
pattern (every positional is a path once `-e`/`--file` names one), `sed` skips
its script unless `-e`/`--expression` is used, `git` reports the operands after
`--` plus the path half of a `REV:path` object name, and the remaining programs
treat every non-option token and every `--option=value` as a path candidate.
`read_arguments_are_free` then requires every candidate to resolve inside the
root through S1's `path_inside_root`, so `~` is expanded and `..` and symlinks
are resolved before the comparison; a path that cannot be resolved asks.

`SECRET_PATH_RULES` is the compiled denylist table: one row per credential
location, with a label and a matcher, so a new location is a row rather than a
classifier change. `secret_refusal` renders the one refusal sentence from the
matched label and never includes the path or a value. The refusal is checked
before the approval gate in the text runner, the Nova Sonic loop, and workflow
steps, so a refused read neither arms a card nor runs and a turn grant cannot
cover it. The gate itself also asks for those calls, so a caller that skipped
the check fails closed with a prompt rather than a leak. A search pattern is not
a path, which keeps `rg -n '\.env' src` free.

Drift: the Evidence range (`safety.rs:78-79`) pointed at the policy `name()`
accessor; the classifier is `shell_requires_approval` and the read helper block
now sits just above the file-operation constants. Adjacent gap found and left
for a follow-up issue: `git log --output=…` writes a file, and the git arm only
refuses `--output` for `diff`.

## S3 — Reject programs containing a slash

- Status: open · Phase 2 · Priority: medium
- Depends on: S1
- Evidence: `src/tools/safety.rs:74-76`

**Problem.** The classifier takes the last path component, so `/tmp/x/ls` or
`./wc` is treated as the allowlisted `ls`/`wc` and runs without approval. Any
executable placed at such a path is a bypass.

**Decision (2026-10-29).** A program containing `/` never inherits an
allowlisted decision; it asks. Allowlisted names must be resolved through
`PATH`. A relative or absolute path to a trusted binary still asks, which is
acceptable friction.

**Acceptance criteria**

- [ ] `/tmp/x/ls`, `./ls`, and `../bin/wc` ask.
- [ ] Bare `ls` is still free.
- [ ] A test documents why a path form asks.

**Tests**

- [ ] Program-form table.

**Files.** `src/tools/safety.rs`.

## S4 — Launch parity between `process` and `shell`

- Status: open · Phase 3 · Priority: low
- Depends on: —
- Evidence: `src/tools/safety.rs:129-154`, `src/tools/safety.rs:351-408`

**Problem.** `process` may launch a browser alias with an `https://` argument
without approval, but `shell firefox https://…` asks, and `shell firefox` alone
is free. Two tools give different answers for the same user-visible action.

**Decision (2026-10-29, tentative).** Pick one rule. Preferred: browser aliases
with only `http(s)` arguments are free in both tools; every other argument set
asks. Confirm the rule in the issue before implementing.

**Acceptance criteria**

- [ ] The same launch is classified identically through both tools.
- [ ] A non-URL argument still asks in both.

**Tests**

- [ ] Cross-tool table.

**Files.** `src/tools/safety.rs`.
