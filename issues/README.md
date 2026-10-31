# Hyusk work items

This folder is the tracker for the permission and desktop-control review of
2026-10-29. It replaces the bullet list that used to live in `AGENTS.md` under
"Current known issues". Each area has one file and stable IDs; work one issue
per session so a fresh agent can pick it up without prior context.

| File | IDs | Area |
| --- | --- | --- |
| [PERMISSION_ISSUE.md](PERMISSION_ISSUE.md) | A1–A10 | Approval policy, prompt/gate consistency, confirmation UX, panels |
| [SHELL_ISSUE.md](SHELL_ISSUE.md) | S1–S4 | Shell classification, project scope, read confinement, path spoofing |
| [DESKTOP_ISSUE.md](DESKTOP_ISSUE.md) | D1–D6 | Terminal typing, browser actions, portal capability, click labels |
| [SECRETS_ISSUE.md](SECRETS_ISSUE.md) | X1–X4 | `.env`, redaction, private runtime files, leakage audit |
| [AGENT_LOOP_ISSUE.md](AGENT_LOOP_ISSUE.md) | L1–L6 | Agent mutex, sync IO, stream timeout, timer clamp, mobile replay, restart |
| [ARCHITECTURE_ISSUE.md](ARCHITECTURE_ISSUE.md) | P1–P6, T1–T4, M1–M4, G1–G2 | Pruning, tool manifests, external providers, memory, preferences |

## Context recorded on 2026-10-29

These decisions shape every issue below. Change one only with a note in the
issue that depends on it.

- **Project root (S1/S2):** `HYUSK_WORKSPACE` when set, otherwise the directory
  the process was started in (the installer pins it to the repository clone).
- **Sessions:** GNOME Wayland and niri (Wayland). X11 and Hyprland are
  secondary; portal-only behaviour applies to GNOME.
- **Panels:** GNOME Shell indicator and Noctalia bar widget are both in daily
  use. The phone link is not used; keep it at once-only approvals.
- **Interaction:** voice hands-free with the wake word. Confirmation grammar
  must tolerate STT phrasing and paraphrases.
- **Stack:** OpenRouter and Bedrock Mantle text models. No Nova Sonic; the
  Sonic approval path stays correct but is low priority.
- **Formats:** session schema and the `hyusk-control.json` protocol may gain
  fields/actions; reads stay backward compatible.
- **Browser:** a dedicated automation profile with no personal logins counts
  as low risk, but click/fill/press_key are still gated; read/navigate stay
  free.
- **Terminal:** typing into a focused terminal prompts; TUI driving stays
  possible after approval.
- **Order:** Phase 1 = prompt/policy consistency and fewer prompts; Phase 2 =
  safety holes; Phase 3 = reliability.
- **`.env` mode (X1):** the user applies `chmod 600 .env` himself in Phase 1;
  the installer, doctor, and docs checks remain part of the issue.

## How to work one issue

1. Read `AGENTS.md`, then this file, then the issue in full.
2. Confirm the baseline: `cargo fmt --all -- --check`, `cargo check
   --all-targets --offline`, `cargo clippy --all-targets --offline`, and the
   isolated test command from `AGENTS.md`.
3. Implement only that issue. Add or update the tests named in the issue.
4. Update the issue status line, `CHANGELOG.md` (Unreleased), and `AGENTS.md`
   if the issue was listed in "Current known issues".
5. Commit as `fix(scope): … (A1)` or `feat(scope): … (A1)`; one issue per
   commit where practical.

Suggested session opener:

> Work issue A1 from `issues/PERMISSION_ISSUE.md`. Read `AGENTS.md` and the
> issue first. Implement only that issue, add the listed tests, run the checks
> from `AGENTS.md`, and update the status and changelog.

## Status legend

- `open` — not started.
- `in-progress` — claimed by a session; add the branch name.
- `blocked` — say why and by which issue.
- `done` — add the date and commit hash.

## Order

Phase 1 and Phase 2 are complete (2026-10-29); Phase 3 is next. Within a phase,
dependencies win over listed order.

| ID | Title | Phase | Pri | Depends on |
| --- | --- | --- | --- | --- |
| A8 | Explicit policy registry with exhaustive coverage | 1 | high | — |
| A1 | One source of truth for approval guidance | 1 | high | A8 |
| A2 | Research delegation runs without approval | 1 | high | A8 |
| A3 | Approval shows the exact call | 1 | high | A8 |
| A4 | Confirmation grammar for voice | 1 | medium | — |
| A5 | Unrelated speech cancels the pending action and proceeds | 1 | high | A4 |
| S1 | Project-scoped shell writes | 1 | high | A8 |
| A7 | One approval card kind; stale approve handling | 1 | high | — |
| A6 | Allow for this turn | 1 | high | A3, A7 |
| X1 | `.env` permissions | 1 | high | — |
| A9 | Fast-command lane uses the same policy | 2 | medium | A8 |
| S2 | Read confinement and secret denylist | 2 | high | S1 |
| S3 | Reject programs containing a slash | 2 | medium | S1 |
| D1 | Prompt for typing into a focused terminal | 2 | high | A8 |
| D2 | Classify browser mutations | 2 | medium | A8 |
| D3 | Capability registry for portal and AT-SPI | 2 | medium | — |
| D4 | Refine consequential click labels | 2 | medium | A8 |
| X2 | Redact secret-shaped output | 2 | medium | S2 |
| D5 | Consistent keyboard risk model | 3 | low | D1 |
| D6 | Single-path invariant for typing versus shell | 3 | medium | D1, D2 |
| S4 | Launch parity between `process` and `shell` | 3 | low | — |
| A10 | Approval audit log | 3 | low | A3 |
| L1 | Split the agent mutex | 3 | high | — |
| L2 | Async session and status writes | 3 | medium | — |
| L3 | Streaming idle timeout | 3 | medium | — |
| L4 | Clamp timer and scheduler durations | 3 | low | — |
| L5 | Mobile-link replay across reconnects | 3 | low | — |
| L6 | Approval state across restart | 3 | medium | A3, A6 |
| X3 | Private runtime files | 3 | medium | — |
| X4 | Leakage audit of logs, status, and memory | 3 | low | X2 |

## Architecture work

Designed in [architecture-roadmap.md](../docs/architecture-roadmap.md) and
tracked in [ARCHITECTURE_ISSUE.md](ARCHITECTURE_ISSUE.md). Waves order work
inside that track and are independent of the phases above. Wave 1 was completed
on 2026-10-31: `P1`, `P2`, `P3`, and `T1` are closed. Wave 2 is under way:
`M1` (memory schema v2), `T2` (the external tool host), `M2` (hybrid
retrieval), `G1` (preference notes), `M3` (consolidation), and `M4` (explain and
export) are closed, and `P5` (the mobile feature) remains; `M2`, `G1`, `M3`, and
`M4` landed ahead of `P5`/`T2` because they depended only on the memory work.
`P4` (the orb overlay) is deferred because this installation uses the GNOME
extension and the Noctalia widget, and `P5`'s dependency on it was only the
shared `Cargo.toml` area. The remaining items are `P5`, `P6` (which needs
`P5`), `T3` and `T4` (which build on `T2`), and `G2`.

| ID | Title | Wave | Depends on |
| --- | --- | --- | --- |
| P1 | Delete the shell-agent experiment | 1 | — |
| P2 | Remove macOS branches | 1 | — |
| P3 | Feature-gate the GTK workflow editor | 1 | — |
| T1 | Tool manifests | 1 | A8, P3 |
| P4 | Feature-gate the orb overlay | 2 | P3 |
| P5 | Feature-gate the mobile link | 2 | P4 |
| M1 | Memory schema v2 | 2 | — |
| T2 | External tool host | 2 | T1 |
| P6 | Feature-gate Nova Sonic | 3 | P5 |
| T3 | MCP provider | 3 | T2 |
| T4 | Trust model and contract tests | 3 | T3 |
| M2 | Hybrid retrieval | 3 | M1 |
| M3 | Consolidation | 3 | M1 |
| M4 | Explain and Obsidian export | 3 | M1 |
| G1 | Preference notes | 4 | M1 |
| G2 | Propose-diff loop | 4 | — |

## Definition of done

- Behaviour is observable and matches the acceptance criteria.
- The named tests exist and fail before the fix.
- `cargo fmt`, `cargo clippy` (no new warnings), and the isolated test command
  pass.
- `CHANGELOG.md` and the issue status are updated; docs that describe the old
  behaviour are corrected.

Last reviewed: 2026-10-31.
