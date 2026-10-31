# Architecture and platform issues

Design and decisions live in
[docs/architecture-roadmap.md](../docs/architecture-roadmap.md). This file holds
the work units: prune and feature-gate (`P`), tools (`T`), memory (`M`), and
growth (`G`). Waves order work inside the roadmap; they are independent of the
`Phase` labels in the permission tracker.

Start Wave 1 only after the permission Phase 1 work is complete: both tracks
touch `safety.rs` and `main.rs`, and `A8` (policy registry) is a prerequisite
for `T1` (tool manifests).

Open decisions carried from the 2026-10-29 discussion are listed in the
roadmap; the defaults assumed by these issues are:

- parked surfaces compile in a scheduled `--all-features` job (P5/P6);
- the MCP allowlist starts with read-only `git`, `filesystem`, and `fetch`
  servers (T3);
- preferences are saved automatically with a review list and an easy `forget`
  (G1).

Change a default only by editing this list and the issue that depends on it.

## P1 — Delete the shell-agent experiment

- Status: done 2026-10-31 · Wave 1 · Size: small
- Commit: `d3d6d63`
- Depends on: —
- Evidence: confirmed before the change at `Cargo.toml:17-19` (the `[[bin]]`
  entry), `src/bin/hyusk-shell-agent.rs` (262 lines) and `src/bin/types.rs`
  (4 lines, a `#[path]` re-export of `src/types/message.rs`)

**Problem.** `hyusk-shell-agent` is self-described as experimental and "not the
production Hyusk permission model". It duplicates the OpenRouter client through
a `#[path]` include and carries its own message types with `allow(dead_code)`.
It is a second, diverging execute-anything path.

**Decision (2026-10-29).** Delete the binary and its type module. Keep the
single OpenRouter client implementation. Remove doc references.

**Acceptance criteria**

- [x] The `[[bin]]` entry is gone; `cargo build` no longer produces the binary.
- [x] `rg hyusk-shell-agent` returns nothing outside the changelog history.
- [x] No duplicated message types remain for the removed binary.

**Tests**

- [x] `cargo check --all-targets` and the isolated test run pass.
- [x] A grep check: the tree has no hit outside the records (`CHANGELOG.md`,
  `issues/`, `memory/`), and
  `platform_tests::the_experimental_shell_agent_is_gone` keeps it that way.

**Files.** `Cargo.toml`, `src/bin/hyusk-shell-agent.rs` (deleted),
`src/bin/types.rs` (deleted), `src/main.rs`, `README.md`, `AGENTS.md`,
`docs/architecture-roadmap.md`.

**Notes (2026-10-31).** Deleted both files and the `[[bin]]` entry. Nothing
imported `src/bin/types.rs` except the removed binary, so `src/types/message.rs`
is untouched and the one `model/openrouter.rs` implementation is now the only
OpenRouter client. No script, systemd unit, CI job, desktop entry or `.env`
variable referenced the binary (`HYUSK_SHELL_AGENT_MODEL` existed only inside
its own help text). The grep criterion is held by a test that assembles the
removed name at run time, so the test file itself cannot satisfy or defeat it.

The suite dropped by 22 tests and gained three: the deleted binary compiled its
own fresh-history test plus the OpenRouter client, protocol, and stream suites
through the `#[path]` include, so those copies are gone. The new baseline is 408
passing, 7 ignored (main binary 403 passing, 6 ignored; the workflow editor
contributes five, one ignored), recorded in `AGENTS.md` and
`docs/development.md`.

## P2 — Remove macOS branches and declare Linux

- Status: done 2026-10-31 · Wave 1 · Size: small
- Commit: `d3d6d63`
- Depends on: —
- Evidence: confirmed before the change at `src/tools/computer.rs:348`
  (non-Linux `should_use_portal`), `1760/1767/1774` (`open_url` launchers),
  `328` (macOS screenshot backend), `1660/1662/1690/1692/1723/1725` (clipboard
  backends), `src/agent/runner/sonic.rs:139/359/647` (non-Linux audio buffer and
  TTS fallback), `src/speech/speech.rs:1573/1577/1590/1612/1632` (non-Linux
  `speak` and the `select!` wrapper) and `Cargo.toml:64-65` (the non-Linux
  `enigo` block). The issue named only the attribute form
  `cfg(not(target_os = "linux"))`; six of these are `cfg!(target_os = …)`
  expressions, and the dependency block was not mentioned.

**Problem.** A handful of non-Linux branches and a non-Linux `enigo` dependency
suggest cross-platform support that is never built or tested here.

**Decision (2026-10-29).** Target Linux. Remove the fallback branches and the
non-Linux dependency, and emit one `compile_error!` on other targets in
`main.rs` so the failure is explicit rather than a pile of missing symbols.

**Acceptance criteria**

- [x] A non-Linux build fails with one clear message.
- [x] The Linux build is unchanged.
- [x] The remaining `cfg(unix)` uses are Linux-relevant only.

**Tests**

- [x] `cargo check --target x86_64-unknown-linux-gnu` passes.
- [ ] Manual: note the expected failure on another target. Not run: only
  `x86_64-unknown-linux-gnu` is installed here (`rustup target list
  --installed`), so no other target can be built without adding a toolchain.
  `platform_tests::the_linux_only_target_is_declared_once` asserts the gate
  instead.

**Files.** `Cargo.toml`, `src/main.rs`, `src/tools/computer.rs`,
`src/speech/speech.rs`, `src/agent/runner/sonic.rs`, `docs/tools.md`,
`docs/voice-and-ui.md`, `docs/architecture.md`, `docs/development.md`,
`docs/architecture-roadmap.md`, `AGENTS.md`.

**Notes (2026-10-31).** `src/main.rs` now opens with one
`#[cfg(not(target_os = "linux"))] compile_error!`, so a wrong host fails with
one sentence instead of a pile of missing symbols. Every other branch was
deleted rather than left dead: the `open`/`cmd` launchers, the macOS screenshot
backend, the `pbcopy`/`clip`/`pbpaste`/PowerShell clipboard paths, the non-Linux
Nova Sonic audio buffer and its `TextToSpeech` fallback, and the non-Linux TTS
`speak` with the `select!` wrapper around it. The Linux chain is now the only
`TextToSpeech::speak_cancellable` body.

Two decisions the issue did not settle:

- **The removed fallbacks could never have compiled.** `speech.rs` called
  `Command::new("say")` and `Command::new("powershell")` without importing
  `Command` (the file imports only `Stdio`), and `sonic.rs` called
  `TextToSpeech::new()` without importing it. This is the strongest evidence
  that the branches were never built; it is recorded here rather than repaired,
  because P2 deletes them.
- **`cfg(not(unix))` stubs were kept.** `task::is_executable`,
  `gnome_doctor`'s `.env` mode check and `paths::restrict_mode` have a
  `cfg(not(unix))` alternative. They are unreachable now that the crate refuses
  non-Linux builds, but removing them would edit three files outside this
  issue and change nothing on Linux, so they stay and the `cfg(unix)` audit
  requirement is met: every `cfg(unix)` site is a mode, chmod, or symlink call
  that is true on Linux.

Positive `cfg(target_os = "linux")` guards stay where they mark a Linux-only
section (the portal helpers, `accessibility`, `window`, `gnome_doctor`, and the
tests that need a session). They are true on the only supported target.

`platform_tests::no_source_file_keeps_a_macos_or_windows_branch` walks `src/`
and fails if a macOS or Windows branch returns; it is the regression guard the
grep criterion of P1 and the "Linux is unchanged" criterion of P2 asked for.
Until P3 feature-gates the GTK editor, `hyusk-workflows` still fails on a
non-Linux host through its `gtk4` dependency before reaching the gate.

## P3 — Feature-gate the GTK workflow editor

- Status: open · Wave 1 · Size: small
- Depends on: —
- Evidence: `src/bin/hyusk-workflows.rs` (1,829 lines), `Cargo.toml`
  (gtk4 and libadwaita)

**Problem.** The editor is a separate binary but shares the package's
dependencies, so gtk4 and libadwaita are built for every agent build. That is
the largest avoidable build cost in the repository.

**Decision (2026-10-29).** Add a `workflow-editor` feature, make gtk4 and
libadwaita optional, and give the bin `required-features = ["workflow-editor"]`.
Keep one crate, as agreed. The installer builds with the feature.

**Acceptance criteria**

- [ ] Default `cargo build` does not compile gtk4 or libadwaita.
- [ ] `cargo build --features workflow-editor --bin hyusk-workflows` builds.
- [ ] `scripts/install-workflow-app.sh` installs the feature-enabled build.
- [ ] README and the desktop entry are unchanged for users who install it.

**Tests**

- [ ] CI: default build plus one feature-enabled build.

**Files.** `Cargo.toml`, `src/bin/hyusk-workflows.rs`,
`scripts/install-workflow-app.sh`, `README.md`.

## P4 — Feature-gate the orb overlay

- Status: open · Wave 2 · Size: small
- Depends on: P3 (same `Cargo.toml` area)
- Evidence: `src/ui/`, `Cargo.toml` (eframe, egui), `docs/voice-and-ui.md`

**Problem.** eframe and egui are built for every run. The panels already render
reply text; the orb is a status visual, and this installation uses GNOME and
Noctalia.

**Decision (2026-10-29).** Add an `orb` feature, make eframe and egui optional,
and start the orb only when the feature is enabled and `HYUSK_ORB` is not
disabled.

**Acceptance criteria**

- [ ] Default build does not compile eframe or egui.
- [ ] With the feature, the orb behaves as today, including `HYUSK_ORB=0`.
- [ ] `docs/voice-and-ui.md` states the feature requirement.

**Tests**

- [ ] Both build modes; manual orb check.

**Files.** `Cargo.toml`, `src/main.rs`, `src/ui/`, `.env.example`,
`docs/voice-and-ui.md`.

## P5 — Feature-gate the mobile link

- Status: open · Wave 2 · Size: medium
- Depends on: P4
- Evidence: `src/link/`, `src/tools/mobile.rs`, `src/main.rs` (link startup)

**Problem.** The link server, pairing file, mobile tool, and TLS/WebSocket stack
are compiled and polled in every run even when no phone is paired. The
memory-sync and workflow-sync branches are stubs that log "merge support is
pending".

**Decision (2026-10-29).** Add a `mobile` feature, default off. Gate the link
startup, the mobile tool registration, pairing publication, and the control
actions. Keep the Android app in the repository; it is not built by default.
Decide separately whether the two sync stubs are implemented or removed.

**Acceptance criteria**

- [ ] Default build excludes the link server and the model-facing mobile tool.
- [ ] `--features mobile` restores current behaviour.
- [ ] The daemon does not poll for a pairing file when the feature is off.
- [ ] Docs say how to build with the feature.

**Tests**

- [ ] Link and mobile tests run under the feature.
- [ ] Default build check.

**Files.** `Cargo.toml`, `src/main.rs`, `src/link/`, `src/tools/mobile.rs`,
`docs/mobile-link-protocol.md`, `README.md`.

## P6 — Feature-gate Nova Sonic

- Status: open · Wave 3 · Size: large
- Depends on: P5
- Evidence: `src/model/bedrock_sonic.rs`, `src/agent/runner/sonic.rs`,
  `src/agent/runner.rs`, `src/agent/runtime.rs`, `src/types/event.rs`

**Problem.** Sonic is a second agent loop with its own approval handling, card
kind, and audio lifecycle. It is not used on this installation, and it already
diverges from the text path.

**Decision (2026-10-29).** Add a `sonic` feature, default off. Gate the client,
the voice loop, the related events, and the `uses_bedrock_sonic` branches.
Before gating, confirm which parts of the divergence are fixed by A7 and keep
them shared.

**Acceptance criteria**

- [ ] Default build excludes Sonic code and its AWS live-audio path.
- [ ] `--features sonic` builds and the existing Sonic tests run.
- [ ] No `cfg` leaves a dangling event or enum variant.

**Tests**

- [ ] Both build modes; Sonic tests under the feature.

**Files.** `Cargo.toml`, `src/model/bedrock_sonic.rs`,
`src/agent/runner/sonic.rs`, `src/agent/runner.rs`, `src/agent/runtime.rs`,
`src/types/event.rs`, `docs/bedrock-nova-sonic.md`.

## T1 — Tool manifests

- Status: open · Wave 1 (after A8) · Size: medium
- Depends on: A8, P3
- Evidence: `src/tools/tool.rs`, `src/tools/registry.rs`,
  `src/agent/runner/prompts.rs`

**Problem.** The `Tool` trait exposes a name, description, and schema. Version,
risk class, capability needs, and origin are implicit or absent, so the policy
table (A8) and the model-facing specs can drift from the implementation.

**Decision (2026-10-29).** Add a manifest to every tool: name, version,
description, JSON schema, risk classes, capability needs, and source (core or
external). The registry validates manifests at startup, generates the model
specs, and feeds the policy table. A conformance check is exposed so a tool can
self-test its schema.

**Acceptance criteria**

- [ ] Registering a tool without a manifest fails the build or a test.
- [ ] Model specs and the policy table are generated from manifests.
- [ ] A manifest version is visible in the registry.
- [ ] External tools later use the same manifest shape.

**Tests**

- [ ] Registry coverage test tied to A8.
- [ ] Conformance check for the built-in tools.

**Files.** `src/tools/tool.rs`, `src/tools/registry.rs`,
`src/tools/safety.rs`, `src/main.rs`, `src/agent/runner/prompts.rs`.

## T2 — External tool host

- Status: open · Wave 2 · Size: large
- Depends on: T1
- Evidence: new module; `src/tools/registry.rs`, `src/main.rs`

**Problem.** Every tool is Rust and built into the binary. Adding a capability
means editing the crate, recompiling, and shipping a new binary.

**Decision (2026-10-29).** One external host over JSON-RPC 2.0 on stdio: spawn,
handshake with a manifest, register namespaced tools (`ext.<server>.<tool>`),
apply per-call timeouts, restart a crashed server, and kill children on
shutdown. Arguments are validated against the declared schema; results are
truncated and redacted like core tool output.

**Acceptance criteria**

- [ ] A fixture server exposes a tool that the model can call.
- [ ] A crashing server does not take down the agent.
- [ ] Every external call passes through the host approval policy.
- [ ] Shutdown leaves no orphan processes.

**Tests**

- [ ] Fixture server: happy path, crash, timeout, oversized output.

**Files.** `src/tools/external.rs` (new), `src/tools/registry.rs`,
`src/main.rs`, `src/tools/safety.rs`.

## T3 — MCP provider

- Status: open · Wave 3 · Size: large
- Depends on: T2
- Evidence: new module; `~/.config/hyusk/mcp.json` (new)

**Problem.** MCP gives access to a growing ecosystem, but its descriptions and
results are untrusted, servers run with user privileges, and approval semantics
are not uniform.

**Decision (2026-10-29).** MCP is one external-server kind. Configuration lists
allowed servers (command, args, environment allowlist) and per-server capability
grants. The host replaces raw server descriptions with curated summaries and
applies its own policy to every call. Seed with read-only `git`, `filesystem`,
and `fetch`. Never auto-approve based on server hints.

**Acceptance criteria**

- [ ] A configured server's tools appear with curated descriptions.
- [ ] Removing a server from the config removes its tools without a restart.
- [ ] A destructive tool is gated by the host, regardless of the server's claim.
- [ ] `HYUSK_MCP=0` disables the provider.

**Tests**

- [ ] Fake MCP server fixture covering discovery, call, and refusal.

**Files.** `src/tools/mcp.rs` (new), config loader, `docs/tools.md`.

## T4 — Trust model, contract tests, kill switch

- Status: open · Wave 3 · Size: medium
- Depends on: T3
- Evidence: `docs/tools.md`

**Problem.** External providers add a trust boundary that the current docs do
not describe.

**Decision (2026-10-29).** Document the boundary: who writes descriptions, what
the host sanitises, what a server can and cannot do, how to disable the
provider, and what an operator must review before adding a server. Add contract
tests for providers.

**Acceptance criteria**

- [ ] `docs/tools.md` has an "External providers" section with the threat model.
- [ ] The kill switch is documented and tested.
- [ ] Contract tests run against the fixture servers.

**Tests**

- [ ] Kill-switch test; fixture contract tests.

**Files.** `docs/tools.md`, `src/tools/external.rs`, tests.

## M1 — Memory schema v2

- Status: open · Wave 2 · Size: medium
- Depends on: —
- Evidence: `src/tools/memory.rs` (schema, `MemoryEntry`)

**Problem.** Entries carry text, tags, and a timestamp. There is no provenance,
confidence, last-used time, supersession, or entry kind, so stale facts cannot
be resolved and recall cannot be scored.

**Decision (2026-10-29).** Additive schema: `source`, `session_id`, `kind`
(note, fact, preference, episode), `confidence`, `last_used_unix`,
`superseded_by`, `created_unix`, `updated_unix`. Add an embedding table. Keep v1
databases readable; backfill unknown provenance. Migration is idempotent.

**Acceptance criteria**

- [ ] A v1 database opens, reads, and writes without data loss.
- [ ] New fields have safe defaults.
- [ ] `memory.json` and `memory.md` export still work.

**Tests**

- [ ] Migration on a v1 fixture; roundtrip; export.

**Files.** `src/tools/memory.rs`.

## M2 — Hybrid retrieval

- Status: open · Wave 3 · Size: large
- Depends on: M1
- Evidence: `src/tools/memory.rs` (`search_entries`, `context_for`)

**Problem.** Retrieval is token overlap over FTS. A paraphrased question misses
relevant notes, and injection is a fixed top-k with no token budget.

**Decision (2026-10-29).** Score with FTS rank, local embedding similarity,
recency, and importance; exclude superseded entries; inject within a token
budget. Embeddings are computed locally when a model file is present; without
one, fall back to FTS.

**Acceptance criteria**

- [ ] A paraphrased query retrieves the expected note in a fixture.
- [ ] The prompt stays inside the configured budget.
- [ ] No network call is made for retrieval.

**Tests**

- [ ] Ranking fixtures; budget test; no-model fallback.

**Files.** `src/tools/memory.rs`, `models/` (ignored), `docs/development.md`.

## M3 — Consolidation

- Status: open · Wave 3 · Size: medium
- Depends on: M1
- Evidence: `src/tools/memory.rs`

**Problem.** Duplicates accumulate, stale entries never decay, and there is no
episodic layer above individual notes.

**Decision (2026-10-29).** A bounded background job merges near-duplicates,
marks stale entries for decay, and summarises completed episodes with the
selected model. It never deletes without a tombstone and a backup, and it is
cancellable.

**Acceptance criteria**

- [ ] Duplicates merge without losing the newer text.
- [ ] Superseded entries stop appearing in retrieval.
- [ ] A backup is written before any removal.
- [ ] The job respects cancellation and a time budget.

**Tests**

- [ ] Fixture with duplicates, supersession, and cancellation.

**Files.** `src/tools/memory.rs`, `src/agent/runtime.rs` (scheduling).

## M4 — Explain and Obsidian export

- Status: open · Wave 3 · Size: small
- Depends on: M1
- Evidence: `src/tools/memory.rs` (markdown mirror)

**Problem.** The user cannot ask why a memory exists, and the human-curated
Obsidian vault used by the harness is separate from the agent's memory.

**Decision (2026-10-29).** `memory why <id>` returns provenance, confidence,
and the supersession chain. Add a one-way export to a configured vault path:
one note per fact with frontmatter, links for relations, and no secrets.

**Acceptance criteria**

- [ ] `why` explains a note without exposing secrets.
- [ ] Export writes valid Markdown notes with unique names.
- [ ] Re-export updates rather than duplicating.

**Tests**

- [ ] Explain output; export idempotence; redaction.

**Files.** `src/tools/memory.rs`, config, docs.

## G1 — Preference notes

- Status: open · Wave 4 · Size: medium
- Depends on: M1
- Evidence: `src/tools/memory.rs`, `src/agent/runner/prompts.rs`

**Problem.** Corrections do not stick. The model re-asks about the same tool,
format, or default in every session.

**Decision (2026-10-29).** A preference is a memory entry of kind
`preference`, scoped global or project. It is injected as a separate block,
deduplicated, and resolved against newer preferences. Low-risk preferences are
saved automatically; privacy, spending, and cross-app behaviour ask first.
`forget` works immediately.

**Acceptance criteria**

- [ ] "No, use pnpm" becomes a project preference and is honoured next turn.
- [ ] A newer preference supersedes an older one.
- [ ] The review list shows preferences with their scope and age.
- [ ] Forgetting a preference removes it from the prompt at once.

**Tests**

- [ ] Save, retrieve, supersede, forget; scope isolation.

**Files.** `src/tools/memory.rs`, `src/agent/runner/prompts.rs`,
`src/agent/runner.rs`.

## G2 — Propose-diff loop

- Status: open · Wave 4 · Size: small
- Depends on: —
- Evidence: `docs/architecture-roadmap.md`, `AGENTS.md`

**Problem.** There is no written rule for how the agent may change the product
it runs on.

**Decision (2026-10-29).** Document the process: the agent works on a branch,
runs fmt, clippy, and the isolated tests, and presents a diff for human review.
A human merges; a supervisor restarts the service. The agent never merges,
never installs, and never changes `safety.rs`, the approval model, or the update
path itself. This is a process document, not new product code.

**Acceptance criteria**

- [ ] `docs/development.md` or a new `docs/self-upgrade.md` states the loop.
- [ ] `AGENTS.md` references it.
- [ ] The safety files are named as human-only.

**Tests**

- [ ] Documentation review only.

**Files.** `docs/self-upgrade.md` (new) or `docs/development.md`, `AGENTS.md`.
