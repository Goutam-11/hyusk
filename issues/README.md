# Work record

The permission and desktop-control review of 2026-10-29 and the architecture
track of 2026-10-31 are finished. This file is the one-page record of what they
changed, what is still open, and where the detail lives. Open a new file per
area when the next batch of work starts.

The six trackers were removed on 2026-11-01, once every item was closed,
deferred, or carried over to "Still open" below. Their full text - evidence,
decisions, acceptance criteria, named tests, and the review that found F1-F3 -
stays in git history:

```bash
git show 8daa174:issues/ARCHITECTURE_ISSUE.md
git log --oneline -- issues/ARCHITECTURE_ISSUE.md
```

`8daa174` is the last commit that carried the architecture tracker, including
the 2026-11-01 review section; the other five files are unchanged since
`2cdc1d8`.

What still binds later work is not only in history: each decision that governs
the code today is a safety invariant in [`AGENTS.md`](../AGENTS.md), a section
in [`docs/tools.md`](../docs/tools.md), or an entry in
[`CHANGELOG.md`](../CHANGELOG.md). Those are the places to read first.

## What shipped

| ID | Change | Closed |
| --- | --- | --- |
| A1 | The model-facing approval guidance is rendered from the policy table | 2026-10-29 |
| A2 | A research delegation runs free; a workspace delegation asks once | 2026-10-29 |
| A3 | The card shows the exact call, and that stored call is what runs | 2026-10-29 |
| A4 | Spoken confirmations are classified by their leading phrase | 2026-10-29 |
| A5 | Unrelated speech drops the pending call and answers the new request | 2026-10-29 |
| A6 | "Allow for this turn" is keyed by tool and call shape, and expires | 2026-10-29 |
| A7 | One approval card kind; a stale panel click is answered | 2026-10-29 |
| A8 | Explicit policy registry with exhaustive coverage | 2026-10-29 |
| A9 | The instant command lane uses the same policy | 2026-10-29 |
| A10 | Every resolved confirmation is appended to `approvals.jsonl` | 2026-10-30 |
| S1 | Shell writes are scoped to the project root | 2026-10-29 |
| S2 | Shell reads are confined, and credential paths are refused outright | 2026-10-29 |
| S3 | A program named by a path never inherits the bare name's decision | 2026-10-29 |
| S4 | `process` and `shell` classify one browser launch the same way | 2026-10-30 |
| D1 | Typing or pressing keys into a terminal asks | 2026-10-29 |
| D2 | Browser `click`/`fill`/`press_key` ask; read and navigate stay free | 2026-10-29 |
| D3 | A capability registry preflights portal and AT-SPI state | 2026-10-29 |
| D4 | Clicks ask only when the label commits something | 2026-10-29 |
| D5 | Keyboard input is classed by what a key does | 2026-10-30 |
| D6 | Every typing path is decided by one shared rule | 2026-10-30 |
| X1 | `.env` is mode `0600`; the doctor and installers report it | 2026-10-29 |
| X2 | Secret-shaped tool output is redacted before it is stored or sent | 2026-10-29 |
| X3 | Audio, screenshots, and model output are owner-only runtime files | 2026-10-29 |
| X4 | Journal, session, cards, memory, and logs pass the redaction sinks | 2026-10-30 |
| L1 | Runtime control state moved out of the turn's agent lock | 2026-10-30 |
| L2 | Session and status writes go through one coalescing writer | 2026-10-30 |
| L3 | Streams are bounded by an idle window, not the request timeout | 2026-10-30 |
| L4 | Timer and scheduler waits are clamped at 30 days | 2026-10-30 |
| L5 | A replayed mobile request is refused after a reconnect | 2026-10-30 |
| L6 | An unanswered confirmation is settled when a session loads | 2026-10-30 |
| P1 | The experimental shell agent is deleted | 2026-10-31 |
| P2 | macOS and Windows branches are gone; Linux is declared once | 2026-10-31 |
| P3 | The GTK workflow editor is the `workflow-editor` feature | 2026-10-31 |
| P4 | The orb overlay was deferred, not closed - see below | deferred |
| P5 | The mobile link is the `mobile` feature, off by default | 2026-10-31 |
| P6 | Nova Sonic is the `sonic` feature, off by default | 2026-10-31 |
| T1 | Every tool declares a validated manifest, and the registry enforces it | 2026-10-31 |
| T2 | External servers are hosted over JSON-RPC stdio under one always-ask row | 2026-10-31 |
| T3 | MCP is a provider over that host, with a curated description and an env allowlist | 2026-10-31 |
| T4 | The provider trust model, kill switch, and fixture contract tests | 2026-10-31 |
| M1 | The memory schema is versioned and migrated additively | 2026-10-31 |
| M2 | Recall is ranked from FTS, local embeddings, confidence, and recency | 2026-10-31 |
| M3 | A bounded pass merges duplicates and decays stale entries | 2026-10-31 |
| M4 | `why` explains an entry; `export` writes the Obsidian vault | 2026-10-31 |
| G1 | A correction sticks as a scoped, superseding preference | 2026-10-31 |
| G2 | The propose-diff loop is written down in `docs/self-upgrade.md` | 2026-10-31 |

Where a decision is still load-bearing, the invariant in `AGENTS.md` names the
ID, so the table above is also the ID index: `grep -n "A8\|T1\|M2" AGENTS.md`.

### Review follow-ups (2026-11-01)

A fresh review of the closed items found three defects and fixed them; the
probes are recorded in the removed `ARCHITECTURE_ISSUE.md` and in the commits
themselves.

| ID | Defect | Commit |
| --- | --- | --- |
| F1 | An external tool whose arguments named `action` could be granted for a turn | `b474d60` |
| F2 | The next memory write reverted an M3 consolidation (whole-store saves by two writers) | `78805e4` |
| F3 | The standing preference block ignored `HYUSK_MEMORY_PROMPT_TOKENS` | `78805e4` |

F1 touches `src/tools/safety.rs`, which is human-only: it is prepared on a
branch and merged by a person, never by an agent.

## Still open

### R1 - the memory path opens the database twice per turn

Low priority, small. `preferences_for_prompt` and `context_for` each call
`load_store`, and `run_db_at` spawns a thread, builds a runtime and pool, and
re-runs the schema and migration checks on every open before `.join()`ing on
the caller. Measured 2026-11-01: 5.83 ms per open with 200 entries, so about
12 ms of blocking work on a tokio worker per turn.

The fix is a kept pool or connection rather than the `spawn_blocking` shape L2
moved away from, which changes error and lifetime handling in a path that
currently fails soft (a read error logs and returns an empty store). It is left
open deliberately rather than hidden inside F2.

Acceptance criteria: one database open per turn at most, measured; the budget
and consistency tests still green; a read failure still returning an empty
store instead of failing the turn. Tests to add with the change.

### P4 - the orb overlay is deferred

Not rejected. eframe and egui stay unconditional until someone takes it,
because this installation is driven from the GNOME top-bar extension and the
Noctalia bar widget, so an `orb` feature would buy build time and nothing the
user sees. Whoever takes it needs:

- the default build to stop compiling eframe or egui;
- the orb to behave as today with the feature, including `HYUSK_ORB=0`;
- `docs/voice-and-ui.md` to state the feature requirement;
- both build modes checked, plus a manual orb check.

### Manual checks that never ran

Automated tests cover the code; these need a real session, a real account, or a
real device, and none was available when the items were closed.

- P2: a non-Linux build fails with the one `compile_error!` (only
  `x86_64-unknown-linux-gnu` is installed here).
- P3: `scripts/install-workflow-app.sh` installs the feature-enabled editor.
- T2/T3/T4: a real external server and a real MCP server (the fixtures are the
  only ones the tests ran), and a live kill-switch run against the service.
- P5: a live Android reconnect against a `--features mobile` build.
- P6: a live `bedrock-sonic` call and a live microphone round trip.
- A1/A5/A6/A7/D1/D3: the GNOME and Noctalia panels - card text, the "allow for
  this turn" button, a stale click - and the spoken confirmations.
- S2/D1/D3: GNOME Wayland and niri portal behaviour (the four portal smokes are
  `#[ignore]`d for the same reason).
- X1: the installer `.env` mode check (the repository has no shell test
  harness).
- D2: the live CDP browser test, which needs the dedicated profile and
  `HYUSK_BROWSER_LIVE_TEST=1`.

## Context decisions that still hold

- **Project root:** `HYUSK_WORKSPACE` when set, otherwise the directory the
  process was started in. Shell policy and project preferences both use it.
- **Sessions:** GNOME Wayland and niri are the daily drivers; X11 and Hyprland
  are secondary. Portal-only behaviour applies to GNOME.
- **Panels:** the GNOME Shell indicator and the Noctalia bar widget are both in
  daily use; the phone link is not, and stays at once-only approvals.
- **Interaction:** voice hands-free with the wake word, so confirmation grammar
  has to tolerate STT phrasing and paraphrases.
- **Stack:** OpenRouter and Bedrock Mantle text models. Nova Sonic is compiled
  only under `--features sonic` and is not the daily path.
- **Browser:** a dedicated automation profile with no personal logins counts as
  low risk, but click, fill, and press key still ask; read and navigate stay
  free.
- **Formats:** the session schema and the `hyusk-control.json` protocol may gain
  fields and actions; reads stay backward compatible.

## Starting the next batch

1. Open `issues/<AREA>_ISSUE.md` for the area you are about to touch, with one
   section per item and a stable ID.
2. Record, per item: the evidence that the problem exists, the decision, the
   acceptance criteria, the tests that must exist, and the files the change may
   touch.
3. Work it through the propose-diff loop in
   [`docs/self-upgrade.md`](../docs/self-upgrade.md): a branch, the checks in
   `AGENTS.md`, a human review and merge, and a supervised restart.

Prefixes already used by the closed tracks are `A` (approval), `S` (shell), `D`
(desktop), `X` (secrets), `L` (loop), `P` (pruning), `T` (tools), `M` (memory),
`G` (growth), `R` (review follow-ups), and `F` (review findings). Pick a fresh
prefix or continue an area's sequence; do not reuse a closed ID.

Last reviewed: 2026-11-01.
