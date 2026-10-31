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

- Status: done 2026-10-31 · Wave 1 · Size: small
- Commit: `d167b63`
- Depends on: —
- Evidence: confirmed exactly before the change: `src/bin/hyusk-workflows.rs` is
  1,829 lines, and `Cargo.toml:61-62` declared `gtk4` and `libadwaita` as
  unconditional Linux dependencies for the whole package

**Problem.** The editor is a separate binary but shares the package's
dependencies, so gtk4 and libadwaita are built for every agent build. That is
the largest avoidable build cost in the repository.

**Decision (2026-10-29).** Add a `workflow-editor` feature, make gtk4 and
libadwaita optional, and give the bin `required-features = ["workflow-editor"]`.
Keep one crate, as agreed. The installer builds with the feature.

**Acceptance criteria**

- [x] Default `cargo build` does not compile gtk4 or libadwaita.
- [x] `cargo build --features workflow-editor --bin hyusk-workflows` builds.
- [ ] `scripts/install-workflow-app.sh` installs the feature-enabled build. The
  script now builds with `--features workflow-editor` and passes `bash -n`; the
  install itself writes into `~/.local/bin` and `~/.local/share/applications`,
  so it was not run from this session.
- [x] README and the desktop entry are unchanged for users who install it.

**Tests**

- [x] CI: default build plus one feature-enabled build. `.github/workflows/ci.yml`
  now runs `cargo check --all-targets --features workflow-editor` and
  `cargo test --bin hyusk-workflows --features workflow-editor` after the default
  clippy and test steps; both commands were run locally, GitHub CI itself has not
  executed this workflow yet.

**Files.** `Cargo.toml`, `src/main.rs` (guard test),
`scripts/install-workflow-app.sh`, `.github/workflows/ci.yml`, `README.md`,
`docs/development.md`, `docs/architecture-roadmap.md`, `AGENTS.md`,
`CHANGELOG.md`.

**Notes (2026-10-31).** `[features]` now holds `default = []` and
`workflow-editor = ["dep:gtk4", "dep:libadwaita"]`; `gtk4` and `libadwaita`
became `optional = true` in the Linux dependency table, and the
`hyusk-workflows` bin carries `required-features = ["workflow-editor"]`. Using
`dep:` keeps one public feature instead of two implicit per-dependency features,
so a typo in a feature name cannot silently enable half the toolkit.

Verified rather than assumed: `cargo tree -e normal` has no `gtk4` or
`libadwaita` line by default and both appear with `--features workflow-editor`;
a default `cargo build` compiles only `hyusk_agent`; the feature build and the
editor's five tests pass.

Decisions the issue did not settle:

- **The editor's tests move with the feature.** `cargo test --all-targets`
  skips the bin, so the default suite is 404 passing, 6 ignored and the
  feature-enabled suite is 409 passing, 7 ignored. Both numbers are recorded in
  `AGENTS.md` and `docs/development.md` rather than pretending one is the whole
  truth.
- **One CI job, extra steps.** The feature build is two steps in the existing
  job instead of a second job, because the native packages and the cache are
  already there; a second job would rebuild GTK4 from scratch.
- **The guard is a manifest test.** `platform_tests::the_workflow_editor_is_behind_its_feature`
  in `src/main.rs` asserts the feature, the two `dep:` entries, the bin's
  `required-features`, and `optional = true`. It cannot prove the compiler skips
  a crate; `cargo tree` does that, and the test keeps the wiring from drifting.

Drift: the issue's `Files` list did not mention `.github/workflows/ci.yml`,
`docs/development.md`, `AGENTS.md`, or the guard test in `src/main.rs`. Nothing
else in the repository referenced the bin's build: `data/io.github.hyusk.Workflows.desktop`
runs the installed `hyusk-workflows`, which the installer still provides.

## P4 — Feature-gate the orb overlay

- Status: deferred 2026-10-31 (see the note below) · Wave 2 · Size: small
- Depends on: P3 (same `Cargo.toml` area)
- Evidence: `src/ui/`, `Cargo.toml` (eframe, egui), `docs/voice-and-ui.md`

**Problem.** eframe and egui are built for every run. The panels already render
reply text; the orb is a status visual, and this installation uses GNOME and
Noctalia.

**Decision (2026-10-29).** Add an `orb` feature, make eframe and egui optional,
and start the orb only when the feature is enabled and `HYUSK_ORB` is not
disabled.

**Deferred (2026-10-31).** The user does not use the orb: this installation is
driven from the GNOME top-bar extension and the Noctalia bar widget, so an orb
feature would buy build time and nothing they would see. The work is not
rejected - the decision above and the acceptance criteria stay valid for anyone
who wants it - but it is out of the queue. `P5`'s dependency on `P4` was only
"the same `Cargo.toml` area", so `P5` and `P6` can land without it, and eframe
and egui stay unconditional until someone takes this item.

**Acceptance criteria** (for whoever takes it)

- [ ] Default build does not compile eframe or egui.
- [ ] With the feature, the orb behaves as today, including `HYUSK_ORB=0`.
- [ ] `docs/voice-and-ui.md` states the feature requirement.

**Tests** (for whoever takes it)

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

- Status: done 2026-10-31 · Wave 1 (after A8) · Size: medium
- Commit: `f6f574b`
- Depends on: A8, P3
- Evidence: confirmed before the change at `src/tools/tool.rs:42-54` (the
  three-method trait), `src/tools/registry.rs:6-36` (a `HashMap` of
  `Arc<dyn Tool>`, no validation), `src/agent/runner/prompts.rs:6-20` (specs
  built from the tool's own methods), and `src/tools/safety.rs:37-200`
  (the A8 policy table)

**Problem.** The `Tool` trait exposes a name, description, and schema. Version,
risk class, capability needs, and origin are implicit or absent, so the policy
table (A8) and the model-facing specs can drift from the implementation.

**Decision (2026-10-29).** Add a manifest to every tool: name, version,
description, JSON schema, risk classes, capability needs, and source (core or
external). The registry validates manifests at startup, generates the model
specs, and feeds the policy table. A conformance check is exposed so a tool can
self-test its schema.

**Acceptance criteria**

- [x] Registering a tool without a manifest fails the build or a test.
- [x] Model specs and the policy table are generated from manifests.
- [x] A manifest version is visible in the registry.
- [x] External tools later use the same manifest shape.

**Tests**

- [x] Registry coverage test tied to A8
  (`tool_policy_tests::every_registered_tool_declares_a_conformant_manifest`).
- [x] Conformance check for the built-in tools (the same test, plus five
  `manifest` and eight `registry` unit tests).

**Files.** `src/tools/manifest.rs` (new), `src/tools/tool.rs`,
`src/tools/registry.rs`, `src/tools/safety.rs`, `src/main.rs`,
`src/agent/runner/prompts.rs`, all fourteen tool modules, `docs/tools.md`,
`docs/architecture.md`.

**Notes (2026-10-31).** `Tool::manifest` is now the only required declaration
and `name`, `description`, and the schema read back from it, so a tool cannot
compile without one. `ToolManifest` carries `name`, `version` (`CORE_VERSION`,
`1.0.0`), `description`, `schema: fn() -> Value`, `risk` (read, mutate, input,
execute, network, system - at least one), `capabilities` (input, screenshot,
accessibility, from D3), and `source` (core or external).

The registry keeps each tool with its validated manifest, exposes
`manifests()`, and generates `specs()` from those manifests; `prompts.rs` now
calls `specs()` instead of rebuilding the JSON itself. `register` validates and
panics with one message on a programmer error, and `build_tools` runs
`conformance()` before handing the registry on, so a bad set fails at startup,
not at the first call.

`conformance()` is the A8/D6 tie: a registered tool without a policy row fails,
and the `input` capability must be present exactly when the row declares input
paths. That last check caught a real asymmetry while it was written: `computer`
declares input paths, so its manifest must carry `Capability::Input`.

Deviations from the decision, recorded so the wording is not mistaken for the
implementation:

- **The per-action decisions did not move into manifests.** A risk class cannot
  express "this command stays inside the project root" or "this click label
  names delete", and moving the decisions out of `safety.rs` would move the
  approval review surface away from the file the invariant names. Manifests
  feed the *identity* checks (policy row coverage, input capability versus
  input paths) and the model specs; the table and its rules stay in
  `safety.rs`, and a doc note there points at the tie.
- **`Source::External` exists but cannot register.** It has no policy row until
  T2's host supplies the approval decisions, so `register` refuses it with a
  message naming T2. The shape is ready; the host is not.
- **`input_schema()` was removed from the trait.** Nothing needed a second way
  to reach the schema; the four tool tests that used it call their own schema
  function now, which also keeps the schema beside its tool.
- **Descriptions were reflowed once.** The move into the manifest removed the
  collapsed runs of spaces left by an earlier reformat, so the model-facing text
  is the same words with normal spacing.

**Verification.** `cargo fmt --all -- --check`, `cargo check --all-targets
--offline`, and `cargo clippy --all-targets --offline` are clean apart from the
three pre-existing `result_large_err` warnings. The default suite is 418 passing,
6 ignored (404 before this item; T1 adds fourteen tests), and the feature suite
423 passing, 7 ignored. Fail-before-fix: the trait change itself stopped eight
test doubles from compiling until each declared a manifest, which is the
"cannot compile without a manifest" guarantee in the small; two registry tests
then failed for real reasons (the name check is unobservable through the default
`name()`, and the conformance message had gained an article) and were corrected
before the commit. A probe then removed `Capability::Input` from the browser
manifest and both coverage tests failed with `browser declares the input
capability false but its policy row declares input paths true`; the file was
restored in the same call (marker re-checked, fmt clean).

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

- Status: done 2026-10-31 · Wave 2 · Size: medium
- Commit: `098e5c4`
- Depends on: —
- Evidence: confirmed before the change at `src/tools/memory.rs:22-29`
  (`MemoryEntry`: id, text, tags, created_unix), `70-93` (`SCHEMA`: the v1
  table), `283-330` (`read_store`), `362-435` (`save_store`)

**Problem.** Entries carry text, tags, and a timestamp. There is no provenance,
confidence, last-used time, supersession, or entry kind, so stale facts cannot
be resolved and recall cannot be scored.

**Decision (2026-10-29).** Additive schema: `source`, `session_id`, `kind`
(note, fact, preference, episode), `confidence`, `last_used_unix`,
`superseded_by`, `created_unix`, `updated_unix`. Add an embedding table. Keep v1
databases readable; backfill unknown provenance. Migration is idempotent.

**Acceptance criteria**

- [x] A v1 database opens, reads, and writes without data loss.
- [x] New fields have safe defaults.
- [x] `memory.json` and `memory.md` export still work. `memory.md` is the
  export and now also shows an entry's kind and a superseded marker;
  `memory.json` is an **import** format, not an export - see the drift note.

**Tests**

- [x] Migration on a v1 fixture; roundtrip; export
  (`a_v1_database_opens_with_backfilled_provenance`,
  `the_schema_migration_is_idempotent`,
  `the_new_fields_round_trip_through_the_database`,
  `the_markdown_mirror_shows_kind_and_supersession`,
  `a_legacy_memory_json_still_imports`,
  `forgetting_an_entry_drops_its_embedding`).

**Files.** `src/tools/memory.rs`, `docs/tools.md`, `docs/architecture.md`,
`AGENTS.md`.

**Notes (2026-10-31).** `MemoryEntry` gained `source`, `session_id`, `kind`
(`MemoryKind`: note, fact, preference, episode), `confidence`,
`superseded_by`, `updated_unix`, and `last_used_unix`, all with serde and
database defaults, plus `MemoryKind::as_str`/`from_str` (`from_str` maps an
unknown stored value to `note`, so a row from a newer build still reads). The
schema gained a `memory_embeddings` table (`entry_id`, `model`, `dims`, `vector`,
`created_unix`), and `save_store` drops an embedding whose entry is gone.

`migrate_schema` runs on every open after `create_schema`: it reads
`PRAGMA table_info`, adds each missing column from a fixed list with
`ALTER TABLE ... ADD COLUMN`, and backfills `updated_unix`/`last_used_unix` from
`created_unix` and `source = 'import'` where they are still 0/empty, then records
`schema_version` in `memory_meta`. Re-running finds every column present and
every backfill already satisfied, so it is a no-op; the v1 fixture test opens the
same file twice.

Decisions the issue did not settle:

- **`source` values.** A note the agent saves is `assistant` with confidence
  `1.0`; a v1 row is `import` with `BACKFILLED_CONFIDENCE` (0.5). `session_id`
  stays `None` until the memory tool is given a session handle (G1), because
  inventing one here would be false provenance.
- **No model-facing change.** `remember` still takes text and tags; kind,
  confidence, and supersession are schema fields that G1 and M3 will write.
  These are tool-manifest-compatible: the schema is unchanged, so the manifest
  version stays `1.0.0`.
- **Retrieval is untouched.** `search_entries` still scores with FTS, tags and
  recency; `last_used_unix` and `superseded_by` are written but not read yet.
  Wiring them in is M2/M3, and doing it here would have changed ranking without
  the fixtures those issues name.
- **The store paths are parameterised for tests.** `load_store_at` and
  `save_store_at` take the database path and derive the `memory.md` mirror from
  it, so the new tests never write the real data directory (the X4 lesson); the
  public `load_store`/`save_store` keep their signatures.

Drift found: the acceptance named `memory.json` an export, but this repository
only *reads* it (a one-shot legacy import after migration, retained as a
recovery backup). Adding a writer would create a second, stale copy of the
store, so the criterion is recorded as met by the import still working and the
Markdown export; the issue now says so.

**Verification.** `cargo fmt --all -- --check`, `cargo check --all-targets
--offline`, and `cargo clippy --all-targets --offline` are clean apart from the
three pre-existing `result_large_err` warnings. The default suite is 424 passing,
6 ignored (418 before; M1 adds six tests), and the feature suite 429 passing, 7
ignored. Two probes, each backed up with `cp` and restored in the same bash call
(marker re-checked, fmt re-checked): dropping `migrate_schema` from
`initialize_db` failed both v1 tests with `no such column: kind`; dropping the
orphan-embedding cleanup failed `the forgotten entry's embedding is gone`. The
first attempt at the second probe matched nothing because rustfmt had reflowed
the statement - the same trap the L4 session hit; it was re-applied against the
formatted text.

## M2 — Hybrid retrieval

- Status: done 2026-10-31 · Wave 3 · Size: large
- Commit: `321a0c6`
- Depends on: M1
- Evidence: confirmed before the change at `src/tools/memory.rs:620`
  (`search_entries`: FTS + tag + recency only) and `921` (`context_for`: a fixed
  `limit`, no token budget)

**Problem.** Retrieval is token overlap over FTS. A paraphrased question misses
relevant notes, and injection is a fixed top-k with no token budget.

**Decision (2026-10-29).** Score with FTS rank, local embedding similarity,
recency, and importance; exclude superseded entries; inject within a token
budget. Embeddings are computed locally when a model file is present; without
one, fall back to FTS.

**Acceptance criteria**

- [x] A paraphrased query retrieves the expected note in a fixture
  (`a_paraphrased_query_finds_the_note_through_the_embedding`, which also shows
  the same query finds nothing without the embedder).
- [x] The prompt stays inside the configured budget
  (`the_context_stays_inside_the_prompt_budget` at 80, 140, and 400 tokens).
- [x] No network call is made for retrieval. The recall path has no HTTP client:
  `LocalEmbedder` hashes in-process, and the database is local.

**Tests**

- [x] Ranking fixtures (`a_paraphrased_query_finds_the_note_through_the_embedding`,
  `without_embeddings_only_lexical_matches_are_retrieved`,
  `superseded_entries_are_never_retrieved`,
  `confidence_and_recency_break_equal_lexical_scores`); budget test
  (`the_context_stays_inside_the_prompt_budget`); no-model fallback
  (`without_embeddings_only_lexical_matches_are_retrieved`, plus
  `the_local_embedder_is_deterministic_and_places_related_words_closer` and
  `saving_the_store_caches_local_embeddings`).

**Files.** `src/tools/memory.rs`, `.env.example`, `README.md`, `docs/tools.md`,
`docs/development.md`, `AGENTS.md`.

**Notes (2026-10-31).** `hybrid_entries` scores four signals - the FTS rank
(normalised to the best candidate), the cosine similarity of the local vectors,
`confidence` as importance, and recency (`exp(-age/90d)`) - weighted
0.55 / 0.25 / 0.12 / 0.08 so the lexical term stays dominant. It skips a
superseded entry outright, and it ignores an entry FTS did not match unless its
similarity clears `MIN_EMBEDDING_SIMILARITY` (0.30), so the hashed vectors
cannot fill the prompt with noise. `search_entries` fills the lexical map from
FTS and keeps the old tag/recency ranker as the fallback when FTS returns
nothing and no embedder is available; the `memory` tool's `search` action uses
the same hybrid ranker.

`context_from_store` chooses the notes and graph facts, then `render_context`
charges the header and footer first and adds each line in rank order until the
next one would not fit - a note is included whole or dropped, never cut in half.
The budget is `HYUSK_MEMORY_PROMPT_TOKENS` (default 600 estimated tokens at
four characters per token, clamped to 4000, `0` disables injection), and the
sum of rounded-up line costs never underestimates the whole.

Deviations from the decision, recorded so the sketch is not mistaken for the
code:

- **There is no model file, so the embedder is a built-in feature hash.** The
  decision assumed "computed locally when a model file is present". This
  repository ships none, and a transformer would also need a tokenizer
  dependency and its vocab; instead `LocalEmbedder` hashes word tokens and
  character trigrams into 256 signed dimensions, which makes plurals, typos, and
  compounds (`npm`/`pnpm`) land near each other with no file, no C++ runtime,
  and no network. `HYUSK_MEMORY_EMBEDDINGS=0` is the lexical-only fallback the
  decision asked for.
- **The paraphrase test uses a fixture embedder.** With no trained model there
  is nothing to paraphrase with, so that test injects a deterministic embedder
  whose vectors put "compiler" near "borrow checker" and asserts the note is
  found by the vector alone (and that the same query finds nothing without one).
  The built-in embedder's own behaviour is tested separately for related-word
  closeness and determinism.
- **The vectors are cached on the write path.** `save_store` stores one vector
  per entry in `memory_embeddings` (the M1 table) under the label
  `local-hash-v1`, replaces rows from another model, and drops orphans; an entry
  without a cached vector (a v1 database, or one written before M2) is embedded
  during recall instead. The M1 embedding test was rewritten for this: saving
  the store now creates the cache, so the fixture no longer inserts rows by
  hand.
- **`MemoryStore` carries the cache.** `embeddings: HashMap<u64, Vec<f32>>` is
  `#[serde(skip)]`, so the legacy JSON mirror is unchanged.

Drift found: none in the Evidence lines; the acceptance's "no network call" is
a property of the code path rather than something a unit test can prove, and it
is recorded that way.

**Verification.** `cargo fmt --all -- --check`, `cargo check --all-targets
--offline`, and `cargo clippy --all-targets --offline` are clean apart from the
three pre-existing `result_large_err` warnings. The default suite is 431 passing,
6 ignored (424 before; M2 adds seven tests), and the feature suite 436 passing, 7
ignored. Probes, each backed up with `cp` and restored in the same bash call
(markers re-checked, fmt re-checked): forcing the similarity input to `None` in
`hybrid_entries` failed the paraphrase test at `only the paraphrase is similar
enough`; removing the entry budget check failed the budget test with `budget 80
produced 531 tokens`.

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

- Status: done 2026-10-31 · Wave 4 · Size: medium
- Commit: `41ff434`
- Depends on: M1 (planned M1 only; the work also used M2's `superseded_by` and
  `confidence` fields)
- Evidence: confirmed before the change at `src/tools/memory.rs` (the `remember`
  arm built every entry as `kind: Note`, with `scope: Global` implied) and
  `src/agent/runner/prompts.rs:252` (one retrieved-memory block, no preference
  block)

**Problem.** Corrections do not stick. The model re-asks about the same tool,
format, or default in every session.

**Decision (2026-10-29).** A preference is a memory entry of kind
`preference`, scoped global or project. It is injected as a separate block,
deduplicated, and resolved against newer preferences. Low-risk preferences are
saved automatically; privacy, spending, and cross-app behaviour ask first.
`forget` works immediately.

**Acceptance criteria**

- [x] "No, use pnpm" becomes a project preference and is honoured next turn
  (`a_newer_preference_supersedes_the_older_one` and
  `the_preference_block_isolates_projects`; the block is added to the system
  prompt for every turn).
- [x] A newer preference supersedes an older one (same topic and scope sets
  `superseded_by`; only the newest is injected).
- [x] The review list shows preferences with their scope and age
  (`the_review_list_reports_scope_and_age`).
- [x] Forgetting a preference removes it from the prompt at once
  (`forgetting_a_preference_removes_it_from_the_prompt`).

**Tests**

- [x] Save, retrieve, supersede, forget; scope isolation (five memory tests
  plus the consent rule in `safety`:
  `a_sensitive_preference_asks_before_it_is_saved`).

**Files.** `src/tools/memory.rs`, `src/agent/runner/prompts.rs`,
`src/tools/safety.rs`, `docs/tools.md`, `AGENTS.md`.

**Notes (2026-10-31).** A preference is a `preference`-kind entry with three new
fields, all additive in schema version 3: `scope` (`global`/`project`), the
canonical `project` root a project preference belongs to, and a `topic` label.
`remember` accepts `kind`, `scope`, and `topic`; a non-preference is always
global. Supersession is topic-based: a new preference marks every active
preference with the same topic and scope `superseded_by`, so the injected block
carries one statement per topic and a correction replaces what it corrects.
Without a topic, only an identical repeated text is deduplicated, because two
paraphrases cannot be told apart safely. `preferences_for_prompt` loads the store,
keeps the global ones and the project ones whose recorded root is the current one
(the same root the shell policy uses), and renders them as their own block in the
system prompt, before the retrieved-memory block. `forget` already removed an
entry; the block is rebuilt from the store, so it disappears on the next turn.
The new `preferences` action is the review list with scope, age in days, topic,
and id.

Decisions and deviations:

- **Consent is a strict word list, not a model judgement.** The decision says
  privacy, spending, and cross-app behaviour ask first; the gate is
  `PREFERENCE_CONSENT_WORDS` in `safety.rs`, matched against the preference text
  and topic, and the row's classifier reads the stored `kind`, so an ordinary
  note that happens to mention a budget is not gated. Over-asking is the safe
  direction; the list is documented in `docs/tools.md`.
- **Project scope uses the shell policy's root.** `safety::current_project_root`
  was made public rather than inventing a second notion of "the project", so a
  preference saved in one workspace cannot leak into another.
- **No automatic "low-risk" classification.** Everything that is not on the
  consent list is low-risk by construction; adding a separate classifier would
  have been a second, unreviewed gate.
- **The review list is a new action** rather than extra columns on `list`,
  because `list` is the model's recent-notes view and the review list is a
  human-facing one with scope and age.

Drift: the issue's dependency line said M1 only; the implementation also relies
on M2's supersession and confidence fields. The `Files` list did not mention
`safety.rs`, `docs/tools.md`, or `AGENTS.md`.

**Verification.** `cargo fmt --all -- --check`, `cargo check --all-targets
--offline`, and `cargo clippy --all-targets --offline` are clean apart from the
three pre-existing `result_large_err` warnings. The default suite is 437 passing,
6 ignored (431 before; six new tests), and the feature suite 442 passing, 7
ignored. Probes, each backed up with `cp` and restored in the same bash call
(markers re-checked, fmt re-checked): disabling the supersession loop failed
`a_newer_preference_supersedes_the_older_one`; accepting every scope failed
`the_preference_block_isolates_projects`; dropping the consent rule failed
`a_sensitive_preference_asks_before_it_is_saved` at `remember my bank details
must ask`.

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
