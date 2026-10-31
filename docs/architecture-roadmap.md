# Architecture roadmap

Status: agreed on 2026-10-29. Wave 1 (P1, P2, P3, T1) was completed on
2026-10-31, Wave 2 began with M1 (memory schema v2), and M2 (hybrid retrieval)
from Wave 3 and G1 (preference notes) from Wave 4 followed because they depended
only on the memory work; P4 is deferred. Work is tracked in
[issues/ARCHITECTURE_ISSUE.md](../issues/ARCHITECTURE_ISSUE.md); this document
records the direction and the reasoning, not individual tasks.

This roadmap starts after the permission Phase 1 work in
[issues/README.md](../issues/README.md). It does not change the permission
policy; it changes what the repository compiles, how tools arrive, and how
memory and preferences behave.

## Where the repository is today

About 32.4k lines of Rust in one package, two binaries (`hyusk_agent` and the
feature-gated `hyusk-workflows` editor), and five surfaces
(GNOME extension, Noctalia widget, orb, Android app plus link, CLI). The
load-bearing path is: wake word, Whisper STT, runtime events, one agent turn,
the model, the tool registry, the approval gate, session and status files, and
the panels.

The instability is not feature count. It is parallel implementations of the
same three concerns:

- two agent loops (`runner.rs` and `runner/sonic.rs`), each with its own
  approval handling;
- three approval call sites (runner, Sonic loop, workflow queue) plus a fast
  command lane that never consults the gate;
- two speech stacks (local Whisper/Piper and Nova Sonic live audio);
- several UI contracts (`hyusk-status.json`, `hyusk-control.json`,
  `hyusk-pairing.json`, orb events);
- overlapping model clients (OpenRouter, OpenAI, Bedrock Mantle, Codex).

Surfaces this installation does not use: the Android app and mobile link, Nova
Sonic, the GTK workflow editor, and the orb overlay. Roughly 5.8k of the 32.4k
Rust lines and 8.8k Kotlin lines support paths that are not exercised here. Wave
1 has started the pruning: the experimental shell-only agent was deleted and the
macOS branches removed (P1, P2), Linux is the only declared target, and the
editor now sits behind the `workflow-editor` feature so an agent build no longer
compiles GTK4 or libadwaita (P3).

## Decisions

| Area | Decision |
| --- | --- |
| Pruning | Park Android plus link and Nova Sonic behind features; delete the experimental shell-only agent; remove macOS branches; make the orb optional; feature-gate the GTK editor with `required-features`. |
| Tool architecture | Hybrid: OS control and approval stay in-process; external tools arrive through one MCP-compatible boundary. |
| First boundary work | MCP client with an explicit allowlist. |
| Memory | Hybrid retrieval with provenance and consolidation. |
| Learning | Preference learning only for now. Workflow capture, allowlist promotion, and evaluation suites are deferred. |
| Upgrade | Human-reviewed releases; the agent proposes diffs, it does not merge or install them. |
| Crates | Keep one crate and feature-gate optional surfaces. |

## Target shape

Principles:

1. The host owns the policy table, the model-visible tool descriptions, and the
   approval gate. External providers are untrusted by construction.
2. Anything that touches the desktop session, secrets, the approval state, or
   the memory store stays in-process.
3. Every tool declares a manifest: name, version, description, JSON schema, risk
   classes, capability needs, and source (core or external).
4. Optional surfaces are features, off by default, and still compile in a
   scheduled all-features job so parked code does not rot.

```
GNOME / Noctalia  <->  status.json / control.json
                              |
wake -> STT -> runtime -> Agent turn -> ToolRegistry
                              |            |
                     host policy      +----+----------------------+
                     (single table)   | core tools (in-process)   |
                                      | shell, computer, window,  |
                                      | accessibility, browser,   |
                                      | memory, timer, media      |
                                      +----+----------------------+
                                           |
                                   external host (JSON-RPC)
                                           |-- MCP servers (allowlisted)
                                           +-- local plugins (later)
                              |
                 session JSON · memory v2 · schedules
```

MCP is a provider kind on the external boundary, not the architecture. Tool
descriptions from a server are untrusted text: the host shows the model a
curated summary and applies the same approval policy to every call.

## Tracks

Issue IDs live in `issues/ARCHITECTURE_ISSUE.md`. Waves order the work within
this roadmap; they are independent of the Phase labels in the permission
tracker.

### P — prune and feature-gate

| ID | Work |
| --- | --- |
| P1 | Delete the experimental shell-only agent and the duplicated type module. |
| P2 | Remove macOS branches; declare Linux as the supported target. |
| P3 | `workflow-editor` feature; gtk4 and libadwaita optional; `required-features` on the bin. |
| P4 | `orb` feature; eframe and egui optional. Deferred on 2026-10-31: the installation uses the GNOME extension and the Noctalia widget, not the orb. |
| P5 | `mobile` feature around the link server, the mobile tool, and pairing. |
| P6 | `sonic` feature around the second loop and its client. |

### T — tools

| ID | Work |
| --- | --- |
| T1 | Tool manifests: version, risk classes, capabilities, source; registry generates specs and the policy table. |
| T2 | One external host: JSON-RPC over stdio, handshake, timeouts, restart, namespaced names. |
| T3 | MCP client as one provider kind: allowlist, curated descriptions, capability grants, host approval. |
| T4 | Trust model, contract tests, and a kill switch. |

### M — memory

| ID | Work |
| --- | --- |
| M1 | Schema v2: provenance, confidence, last used, supersession, entry kind; v1 stays readable. |
| M2 | Hybrid retrieval: FTS5 plus local embeddings, recency and importance, token budget. |
| M3 | Consolidation: duplicates merged, stale entries decayed, episodes summarised, bounded. |
| M4 | `memory why <id>` and a one-way Obsidian export for human curation. |

### G — growth and upgrade

| ID | Work |
| --- | --- |
| G1 | Preference notes: scoped, deduplicated, injected as a separate block, forgettable. |
| G2 | Propose-diff loop: branch, tests, human review, merge, supervised restart. |

## Sequencing

- **Wave 1 — build and foundation:** P1, P2, P3, then T1 after the permission
  `A8` registry lands.
- **Wave 2 — boundary and memory core:** P4, P5, M1, T2.
- **Wave 3 — ecosystem and recall:** P6, T3, T4, M2, M3, M4.
- **Wave 4 — learning and process:** G1, G2.

Waves can overlap when the files do not. P1 and P2 touch nothing the permission
work touches; P3 changes the build graph and should land before the larger
additions.

## Risks

- **Parked code rots.** A feature-flagged surface that is never compiled is
  broken warehouse stock. The recommended mitigation is a scheduled
  `--all-features` job; this is an open decision, not a settled one.
- **MCP is a prompt-injection surface.** Server descriptions and results are
  external text. Curated summaries, an allowlist, and host-side policy are
  required, and the host must never trust a server's "read only" claim.
- **Memory migration is the least reversible change.** v1 must stay readable,
  consolidation must never delete without a tombstone or backup, and the
  Obsidian export must not carry secrets.
- **Preference drift.** A wrong preference saved silently is worse than none.
  The review path must be one action, and `forget` must be immediate.
- **Pruning is product-visible.** Android and Sonic are shipped features in
  `README.md` and the changelog. Parking is reversible; deleting them is not.

## Deferred, deliberately

Workflow capture from successful sequences, allowlist promotion from repeated
approvals, a failure/evaluation suite, agents writing their own tools, and
autonomous updates. The first is small to add later because the deterministic
workflow vocabulary already exists.

## Starting point

Read [issues/ARCHITECTURE_ISSUE.md](../issues/ARCHITECTURE_ISSUE.md), then
`AGENTS.md`, then the issue. Do not start Wave 1 before the permission Phase 1
work is complete: both touch `safety.rs` and `main.rs`, and the registry change
in `A8` is the foundation for tool manifests in `T1`.
