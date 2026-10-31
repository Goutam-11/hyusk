# Changelog

## Unreleased

### Added

- Every tool now declares a manifest (T1): name, version, description, JSON
  schema, risk classes, capability needs, and source (`core` for the built-in
  tools). `Tool::manifest` has no default, so a tool without one does not
  compile; `ToolRegistry::register` validates the declaration at startup and
  panics with one message on an empty name, a version that is not
  `major.minor.patch`, an empty or oversized description, a schema that is not an
  object schema, a `required` entry `properties` does not declare, no risk
  class, a name that disagrees with the tool, a duplicate, or an external
  source. The model-facing specs are generated from the manifests, and
  `conformance()` ties them to the approval policy table: every registered tool
  needs a policy row, and the `input` capability must match the row's declared
  input paths. A tool can self-test its declaration with
  `manifest().conformance()`. The per-action gate is unchanged.
- The agent declares Linux as its only supported target. `src/main.rs` fails any
  other build with one explicit `compile_error!` instead of a pile of missing
  symbols, and the macOS and Windows branches are gone: the `open`/`cmd`
  launchers in `open_url`, the `screencapture`, `pbcopy`/`clip`, `pbpaste`, and
  PowerShell clipboard backends, the non-Linux screenshot backend list, the
  non-Linux Nova Sonic audio buffer, and the macOS `say`/Windows PowerShell TTS
  fallback. The non-Linux `enigo` dependency block is removed, and behaviour on
  Linux is unchanged. `platform_tests` in `src/main.rs` fail if a macOS or
  Windows branch is reintroduced or the target gate is dropped (P2).
- Approval decisions are recorded in an audit log. Every resolved confirmation
  appends one JSON line to `approvals.jsonl` in the data directory - approved
  (with the `once` or `turn` scope), denied, dropped because the user asked for
  something else, expired, or a panel click that arrived after expiry - with the
  timestamp, tool, preview and turn. The line is written once, where the stored
  call is resolved, so a repeated event cannot double it, and the preview is
  redacted on the way out (secret-shaped values and credential paths), because
  the file outlives the turn. The log is `approvals.jsonl` (`0600`) in a `0700`
  directory and rotates into one previous copy at 256 KiB, so two bounded files
  exist. The redaction pass also learned to split token candidates on `=`, so a
  credential inside a rendered call such as `command=API_KEY=sk-…` is caught.
  Tests cover the line shape and redaction, the scope and turn fields, a stale
  click, private modes, and rotation; the runner records exactly one line per
  denial, approval and expiry.
- `docs/architecture-roadmap.md` and `issues/ARCHITECTURE_ISSUE.md`: the
  2026-10-29 architecture decisions — park Android/link and Nova Sonic behind
  features, delete the experimental shell agent, remove macOS branches,
  feature-gate the GTK editor and the orb, keep one crate; in-process core with
  one MCP-compatible tool boundary; memory v2 with provenance and
  consolidation; preference-only learning; human-reviewed upgrades.
- `issues/` work-item tracker from the 2026-10-29 permission and desktop-control
  review: five area files with stable IDs (`A` approval policy, `S` shell, `D`
  desktop control, `X` secrets, `L` agent loop), each with evidence, a recorded
  decision, acceptance criteria, and tests. `AGENTS.md` now points at the
  tracker instead of listing known issues inline.
- Noctalia v5 bar widget (`noctalia-plugin/`) for niri, Hyprland, and Sway:
  state glyph, tooltip card, and a settings panel that switches models and
  saves provider API keys. Install with `scripts/install-noctalia-plugin.sh`.
- Model and credential management across OpenRouter, the OpenAI API, Amazon
  Bedrock Mantle, Nova Sonic, and Codex. Catalogs refresh at runtime, a saved
  key registers its provider immediately, and the selection persists in
  `~/.config/hyusk/config.json`. Keys are stored in the login keyring
  (`credentials.rs`). The GNOME menu writes the key with `secret-tool` and
  restarts `hyusk.service`; the Noctalia panel hands it over through
  `hyusk-control.json`, which the agent scrubs as soon as it has read the key
  and then stores and refreshes the catalog. Keys are not persisted in
  `commands.json` or the session snapshot.
- `hyusk-control.json` channel with monotonic revisions for the indicators:
  `set_model`, `set_api_key`, `refresh_models`, workflow/stop actions, and
  immediate scrubbing of any key payload the agent has consumed.
- `browser` tool that drives a separate Brave/Chromium/Chrome profile over CDP
  (`status`, `start`, `new_tab`, `tabs`, `read`, `navigate`, `click`, `fill`,
  `press_key`), plus `scripts/start-browser-control.sh` and
  `docs/browser-control.md`.
- Android companion: Material 3 command deck, accessibility-driven phone
  actions with verification, foreground task service, direct Nova Sonic live
  voice, workflow execution, encrypted local settings/cache, and a new local
  model client protocol with matching unit tests.
- Nova Sonic speech-to-speech on the desktop and phone, including tool
  dispatch, barge-in, and stream-expiry reporting; optional PipeWire
  echo-cancel service installed by `scripts/install-autostart.sh --voice-audio`.
- Bounded safety approvals (`tools/safety.rs`): consequential actions arm a
  one-shot confirmation card or phone prompt that expires unanswered instead of
  running.
- Restart-safe sessions and bounded history compaction
  (`agent/runner/session.rs`), streamed `ResponseDelta` speech while the reply
  is still generating, and repair of interrupted tool batches.
- `scheduler`, `task`, `timer`, and `web_search` tool documentation and their
  GTK workflow-editor integration (control-deck library, quick-run menu,
  scheduling, and run-without-saving fixes).
- `CI` workflow (`.github/workflows/ci.yml`) that checks formatting, runs
  clippy, and runs the test suite on pushes to `main` and on pull requests.

### Changed

- The GTK workflow editor is behind a `workflow-editor` feature. `gtk4` and
  `libadwaita` are optional dependencies owned by that feature, the
  `hyusk-workflows` bin carries `required-features = ["workflow-editor"]`, and
  `scripts/install-workflow-app.sh` passes the feature, so a plain
  `cargo build` or `cargo test --all-targets` no longer compiles the toolkit —
  the largest avoidable build cost in the repository. CI keeps one
  feature-enabled build (`cargo check --all-targets --features
  workflow-editor` plus the editor's tests) next to the default job, and
  `platform_tests` in `src/main.rs` fails if the feature wiring is dropped. The
  README, the desktop entry, and the installed application are unchanged for
  users who run the installer (P3).
- The mobile link no longer accepts a replayed request after a reconnect. Replay
  protection used to be per connection: each new connection started its
  sequence at zero, so a captured request could be replayed by opening a new
  one. The laptop now keeps the highest request sequence it has accepted for
  each paired device in `link-devices.json` (mode `0600`, written with the
  device map), rejects anything at or below it with `invalid or replayed
  request`, and checks the MAC before the sequence so an unauthenticated frame
  cannot advance the counter. `device.hello` returns the accepted value as
  `result.sequence`, and the Android client seeds its counter from it, so a
  reconnect (or a fresh app process) continues above the mark instead of
  replaying. The wire format is unchanged; `docs/mobile-link-protocol.md`
  states the rule.
- One launch is classified the same way through `process` and `shell`.
  `process` already let a browser alias run with `http(s)` URLs, but
  `shell firefox https://…` asked while `shell firefox` was free. Both tools now
  share one browser list and one URL rule: a bare GUI launch is free, a browser
  may also be given `http(s)` URLs and nothing else, and any other argument
  asks in both. A browser named by a path (`./firefox https://…`) still asks
  (S3), and a non-browser launcher with a URL still asks (`spotify https://…`).
  The model-facing guidance for both rows and `docs/tools.md` state the rule.
- Timers and schedules are bounded at 30 days. Both tools passed a model-supplied
  `u64` straight into a sleep, so a huge value (`u64::MAX` seconds, or an
  `at_unix` far in the future) overflowed the deadline conversion and panicked
  the worker; the schema only enforced a minimum. `tools::MAX_WAIT_SECONDS`
  (2592000) is now the one bound: `timer` `set` refuses a longer `seconds` and
  the scheduler refuses an `after_seconds`/`at_unix` further out, each with a
  failure that names the limit, and both schemas advertise it. The schedule
  worker also caps its own sleep at the bound, so a `schedules.json` written
  before the clamp (or by hand) wakes and recomputes instead of panicking.
- The session snapshot and the status file are written by one coalescing
  writer instead of on the async worker that runs a turn. `persist_session`
  runs after every message and tool result, and each status card rewrote
  `hyusk-status.json`; both serialised their snapshot and did their file IO
  inline. A caller now hands the writer a builder and returns: the queue keeps
  at most one entry per file (a superseded snapshot is dropped without being
  serialised), a single writer task replaces the file atomically with mode
  `0600`, and a snapshot that cannot be built or written is logged with its
  category instead of failing the turn. The writer starts in the composition
  root; without it (tests, the standalone workflow binary, early startup) a
  write still happens synchronously, and shutdown flushes whatever is queued.
  A stale snapshot can never overwrite a newer one that already landed.
- A restart no longer leaves a confirmation looking unanswered. The armed call
  lives only in memory, but its placeholder tool result was saved in the
  session, so after a restart the model saw "Pending confirmation for: …" with
  nothing armed - it could repeat the question, and a later "yes" could be read
  as an answer to a card that no longer existed. Loading a session now rewrites
  every such placeholder to "Not confirmed: the service restarted before the
  user answered", so the conversation agrees with the prompt's "no pending
  approval" sentence and the next "yes" is an ordinary utterance that reaches
  the model. Nothing is re-armed, and no grant survives: turn grants live only
  in memory and were never persisted. The rewrite happens on load only, so a
  live process still saves the armed placeholder it is showing.
- Typing is one capability with one decision, whichever tool names the path.
  `accessibility set_text` used to run free in every application, so a command
  could be typed into a terminal through it while the equivalent `computer
  type_text` asked. Every typing and key action is now declared in its tool's
  policy row (`computer` `type_text`/`key_press`/`key_combo`, `accessibility`
  `set_text`, `browser` `fill`/`press_key`), the shared rule decides them before
  the tool's own rules, and the runner reads the focused window for exactly
  those actions. Text entry or a key press therefore asks when the focused
  application is a terminal or the focus cannot be read, and the card names the
  terminal. The browser's `fill` and `press_key` stay a stricter class that
  always asks, and a tool that cannot reach the keyboard is listed explicitly,
  with a test walking the table so a new tool or path has to be classified.
- Keyboard input outside a terminal now follows one stated model. Every key is
  classed as navigation, editing, or commit (D5). Navigation keys (arrows, Home,
  End, Page Up/Down, Tab, Escape, Space, Print Screen, the modifier keys, and
  F1-F12 except F4) run freely. Editing keys (Delete, Backspace, Insert) run
  freely in an editor, a browser field, or a chat window, and ask in a file
  manager, where they delete a file; the card names the application. Commit keys
  (Enter/Return, F4, and any shortcut containing one such as ctrl+enter or
  alt+F4) ask everywhere, because the agent cannot undo a commit. A modified
  editing key still asks as a bulk delete, ctrl+q and ctrl+w still ask, and a
  terminal or an unreadable focus still asks for every key. "Allow for this
  turn" covers the rest of the turn's key presses, so TUI driving still needs
  one approval.
- Nothing the agent stores is a secret store. The four sinks were audited (X4):
  tool-call arguments reached the journal and the saved session unredacted,
  status card text could show a credential or a path to one, and the memory
  database was created with the default mode. Arguments are now redacted where
  a journal entry is created and again when the session snapshot is saved
  (arguments, text, and refusals), card text is scrubbed before it is stored,
  written, or shown (secret values, plus `[redacted path]` for credential
  paths), and remembered notes and graph facts are redacted on write with the
  database at `0600` inside an owner-only directory. The redaction pass also
  now covers the password in a URL's userinfo, `[Hyusk] Running …` log lines
  print redacted arguments, and `hyusk-status.json` is chmodded `0600` because
  its fallback location is `/tmp`. The live audit also found the schedule file
  at `0644` in a `0755` directory: `schedules.json` is now written `0600`
  through a private temporary file in a `0700` directory and restricted after
  the rename. The per-sink table is in `docs/development.md`.
- Runtime controls no longer wait for a turn. Approval, model-selection, and
  paused-task state moved out of the agent into a small control struct
  (`src/agent/control.rs`) behind its own lock, which the event loop takes
  without the agent mutex. An emergency stop now disarms a stored confirmation
  immediately instead of after the turn is interrupted, and a saved API key is
  installed at once instead of waiting for a running turn to finish. A panel
  decision is classified from the current state even while a turn or a Nova
  Sonic session holds the agent: a stale click now reports expiry instead of
  being forwarded, and an expired confirmation can be resolved during a Sonic
  session. The agent keeps the turn-owned state (messages, tool grants, journal)
  and is still locked for a whole turn, so nothing about turns, approvals, or
  the tool loop changed. Lock order is agent then control, documented in the new
  module.
- Streamed replies are no longer cut by the whole-request timeout. The client
  carried a single 150-second limit that also capped the response body, so a
  long answer that kept producing tokens was truncated mid-reply and the turn
  reported a failed stream after part of it had been spoken. A stream is now
  bounded by an idle window (`HYUSK_STREAM_IDLE_TIMEOUT_SECS`, default 45
  seconds without data) and a configurable whole-reply ceiling
  (`HYUSK_STREAM_TOTAL_TIMEOUT_SECS`, default 600; `0` disables it). A provider
  that stops sending fails with `Model stream stalled` instead of holding the
  turn open, and a reply past the ceiling fails with the total-limit message.
  Connecting and waiting for response headers, and every non-streaming request,
  keep the 150-second request timeout, and the retry rules are unchanged. A
  stream that fails after part of the reply was spoken is still reported rather
  than replayed as a plain request. Both variables are documented in
  `.env.example`, `README.md`, and `docs/development.md`.
- Files Hyusk generates are now private. TTS audio, screenshots, and Codex
  output are created with mode `0600` and unique names under
  `$XDG_RUNTIME_DIR/hyusk` (directory mode `0700`) through a new `src/paths.rs`
  helper, instead of the shared temporary directory with the default mode and
  a guessable name. The temporary directory is used only when the runtime
  directory is unavailable, and even there the files are owner-only. Screenshot
  retention and the file cap keep working in the new location and also sweep
  the temporary directory, so screenshots written there by an earlier version
  are removed; a capture backend that replaces the file has the owner-only mode
  re-applied to the result.
- Tool output is redacted before it becomes a tool result. A bounded pass
  replaces private-key blocks, common credential token prefixes (OpenRouter,
  OpenAI, GitHub, Slack, AWS, Google, Hugging Face, GitLab, npm, PyPI, JWTs),
  values after a `Bearer` scheme, and long values assigned to secret-shaped
  names (`OPENROUTER_API_KEY=…`, `"api_secret": "…"`, `?access_token=…`) with
  `[redacted]` or `[redacted private key]`. The result keeps its line shape and
  ends with a note naming how many values were replaced, so the model knows the
  output was edited. The raw value no longer reaches the session file, the
  provider request, or status text. Redaction is deliberately bounded and
  over-redacts rather than leaking; the rule is documented in `docs/tools.md`.
- Click labels are classified more narrowly. Words that commit a payment,
  deletion, transfer, removal, reset, or uninstall still ask whatever else the
  label says, and "place order" asks; a submission word (`submit`, `send`,
  `post`, `share`, `install`, `confirm`, `publish`) asks only when the same
  label does not name a benign qualifier such as `search`, `filter`, `query`,
  `calculator`, `feedback`, or `link`. "Submit search", "Send feedback", and
  "Share link" no longer ask, while "Send message", "Post comment", "Delete
  account", and "Place order" still do. The same table serves `computer`
  `click_text` and `accessibility` clicks.
- Desktop capability state is recorded and used before asking. The computer and
  accessibility tools now record what the OS last reported for input,
  screenshot, and accessibility (`granted`, `denied`, `unsupported`, or
  `unknown`), with the backend that reported it: the RemoteDesktop portal, the
  native input backend, a screenshot backend, or the AT-SPI bridge. When input,
  screenshot, or accessibility is denied or unsupported, the matching call
  returns a capability error instead of publishing an approval card the user
  cannot satisfy; an unknown state still allows the attempt. The model sees one
  `capabilities: …` line in its per-turn desktop context, and `gnome_doctor`
  reports the same line as a `Desktop capabilities` check.
- Browser mutations now ask for confirmation. `browser click`, `fill`, and
  `press_key` arm the usual card, while `status`, `start`, `new_tab`, `tabs`,
  `read`, and `navigate` stay free; an action the policy does not know asks
  rather than runs. The preview names the action, target id, and selector or
  text, and "Allow for this turn" covers later clicks in the same turn, because
  a browser grant is keyed by the mutation.
- Typing and key input now ask for confirmation while a terminal is focused.
  `computer type_text`, `key_press`, and `key_combo` can run a shell command
  through a terminal window, so the gate reads the focused window from the
  `window` tool and asks when it is a terminal, naming it in the approval
  preview; outside a terminal they stay free, so TUI driving needs one approval
  rather than one per keystroke. If the focus cannot be read the action asks,
  which fails closed. The same decision applies in the text runner and the Nova
  Sonic loop. The terminal list is built in and can be extended with
  `HYUSK_TERMINAL_APPS` (comma-separated application names).
- The instant command lane now obeys the approval policy. If any step of a
  parsed command would ask — `mute` and `brightness up`/`down` run `pactl` or
  `brightnessctl`, and `lock the screen` runs `loginctl` — nothing executes and
  the utterance goes to the model path, where the runtime arms the usual
  confirmation. A chain that mixes a free step with a guarded one defers as a
  whole, so no part runs before the decision. Workflows already stopped before
  a gated step; the model, instant, and workflow lanes now agree.
- `shell` no longer gives an allowlisted decision to a program named by a path.
  The classifier used to take the last path component, so a binary at
  `/tmp/x/ls` or `./ls` inherited `ls`'s free pass; such a call now asks, and an
  allowlisted name must be resolved through `PATH`. A turn grant keeps the path
  in its shape, so approving one path form never covers another. A credential
  path is still refused outright rather than merely asked.
- `shell` reads are confined to the project root, and credential paths are
  refused outright. File-reading programs (`cat`, `head`, `tail`, `wc`, `file`,
  `stat`, `ls`, `du`, `df`, `grep`, `rg`, `sed`, `find`, `sort`, `git show`,
  `git log`, `cargo test`/`check`/`build`/`fmt`/`clippy`) run freely only while
  every path they name stays inside the project root; a read outside it asks,
  `~` is expanded and `..` resolved before the check, and a path that names
  `.env*`, `.ssh`, `.gnupg`, `.aws`, `.kube`, `.netrc`, `.config/gh`, a keyring,
  or the managed automation browser profile is refused before any card is
  armed. The refusal names the denylist rule, never the path, and a
  `git show REV:.env` blob read is refused the same way. A search pattern is
  not treated as a path, so searching for the text `.env` still works.
- Routine project file work no longer prompts: `mkdir`, `touch`, `cp`, `mv`,
  `ln`, and `install` run without approval when every path they name stays
  inside the project root (`HYUSK_WORKSPACE`, else the working directory). `~`
  is expanded and `..` resolved before the check, `-t`/`--target-directory` are
  treated as paths, unknown options ask, and `rm`, `rmdir`, `shred`,
  `truncate`, `chmod`, and `chown` always ask.
- The `.env` file mode is enforced and reported. `install-autostart.sh` tightens
  an existing `.env` to `0600` and prints the previous mode (the mobile-link
  script already wrote through a `0600` temporary file), `gnome_doctor` reports
  `.env permissions` with `needs_attention` and a `chmod 600 .env` next step
  while other accounts can read it, and the setup docs tell the user to
  `chmod 600 .env`.
- A confirmation can now be granted for the rest of the turn. "Allow for this
  turn" — spoken as "allow for this turn" or "yes, for the rest of the task" —
  and the new **Allow for this turn** button in the GNOME card and the Noctalia
  Agent tab record a grant keyed by the tool and the call's shape (the shell
  program plus its first subcommand). Later matching calls run without a card, a
  different program or a new turn asks again, and the grant lives only in
  memory: it is cleared at turn start and turn end and never written to a
  session. The phone link stays once-only.
- Every mode publishes the same approval card kind (`status::APPROVAL_CARD_KIND`,
  `approval`), so Approve/Deny now appear for Nova Sonic confirmations too; the
  Sonic paths previously used `confirmation`, which both panels ignore. Panel
  decisions arrive as an `ApprovalDecision` event instead of the words
  "yes"/"no": the runtime validates them against the armed confirmation, and a
  click after expiry says so and leaves the conversation untouched. The expiry
  notice is now a response card, so no dead Approve/Deny buttons stay on
  screen.
- A pending confirmation no longer hijacks the conversation. Unrelated speech
  drops the stored call - the placeholder says it was dropped because the user
  asked something else, one short spoken note says so, and the new request runs
  in the same turn - while "no, open Firefox" records the refusal and runs the
  instruction. Only a cancelled turn keeps the call armed.
- Spoken confirmations are classified by their leading phrase instead of nine
  exact sentences, so "okay", "sure", "yeah", "do it", and "please do" approve
  and "don't do that" denies. A denial anywhere in the utterance wins over a
  leading approval, a question is never an approval, and a refusal followed by
  an instruction is a denial plus a new request.
- A confirmation now names the exact call. The gate renders a preview from the
  stored arguments - tool plus key arguments, one line, truncated at about 120
  characters with an ellipsis - stores it with the pending call, and both the
  tool result (`Pending confirmation for: …`) and the status card use that same
  sentence, so the user approves the command rather than the model's
  paraphrase. The prompt tells the model to repeat the previewed call in one
  short line.
- Research delegation runs immediately again: `task` with `profile: "research"`
  is one read-only model call, so it no longer costs a confirmation. Workspace
  delegation still asks once, with the canonical project root pinned in the
  approved call, and the root now falls back to `HYUSK_WORKSPACE` when the model
  names none. Availability is preflighted before a card is published: a missing
  Codex CLI, a missing root, or a root that does not exist reports a clear
  message and asks the user nothing.
- The system prompt's approval guidance is now generated from the policy table
  instead of a hand-written list, so the prose cannot contradict the gate. The
  retired block claimed that `task` delegation and other gated classes needed
  no confirmation and told the model to ask the user before destructive
  actions; the prompt now says the runtime is the only component that asks, so
  the model calls a guarded tool once and repeats the runtime's pending action.
- The approval gate is now one explicit policy table (`tools/safety.rs`) with a
  row per registered tool: an allow decision or a classifier over the call's
  arguments, plus the guidance the system prompt renders. A tool name the table
  does not know asks instead of running, a new tool cannot be added without a
  decision, and a registry coverage test fails when `main.rs` registers a tool
  that has no row. Behaviour is unchanged: every current decision, including
  `browser` remaining ungated, is recorded rather than reinterpreted. An
  unknown tool name now reports a missing tool instead of arming a confirmation
  that could never run.
- `hyusk_agent` is a standalone Cargo workspace: the crate owns `Cargo.lock`,
  `target/`, and build profiles, the parent workspace excludes the directory,
  and the installer and docs use the local target directory.
- Hand-clap wake and wake-word RNNoise preprocessing now default to off when
  `WAKE_CLAP_ENABLED` and `WAKE_WORD_DENOISE` are unset, matching the documented
  defaults and `.env.example`; `WAKE_TEST` uses the same defaults. The README
  configuration table now lists `WAKE_WORD_STEP_MS`.
- `task` delegation now arms a one-shot confirmation for both the research and
  workspace profiles before a worker starts, and the approved call pins the
  canonical workspace root.
- Workflow execution now names the tool in its approval-gated step error and
  points at the assistant; the README states that `task` delegation is not a
  workflow step.
- The Noctalia widget names the state and a running timer's countdown on the
  bar, and its panel carries the GNOME indicator's menu as Agent, Models,
  Workflows, and Device tabs (transport keys, reply card with approve/deny,
  workflow quick-run, phone pairing, automation browser), with the API key form
  pinned below them. The installer also copies the two companion scripts the
  Device tab runs.
- API keys are resolved keyring-first and then from the environment, so saving
  a key applies without restarting the user service.
- The GNOME indicator is split into `lib/` modules with a searchable model
  picker, quick-run menu, API-key submenu, and reorganization into Workspace,
  Model, Device, and Settings sections.
- The agent runner is split into `prompts`, `reply`, `session`, `sonic`,
  `stream`, and `transport_tests` modules; tool dispatch reports partial
  failures instead of aborting a turn.
- Shell foreground commands are bounded at 120 s with truncated output.
- The crate declares its package metadata and its `sqlx` and
  `tokio-tungstenite` requirements directly instead of inheriting them from the
  parent workspace, so a fresh clone builds and tests without that workspace.

### Fixed

- The `mobile` tool no longer offers a `pairing` action, so the live pairing
  secret cannot appear in a model tool result or session history. Pairing codes
  remain available from the GNOME and Noctalia indicators.
- Streamed replies no longer speak the same text twice when a provider repeats
  the preceding prose in an unfenced response envelope; the envelope is stripped
  and only the unsaid remainder is spoken (`agent/runner/reply.rs`).
- Android guided actions verify the resulting screen instead of assuming a
  dispatch succeeded, and Nova stream expiry now surfaces instead of wedging
  the live session.
- Wake probes no longer cancel an active task unless a spoken instruction is
  actually transcribed, and stale listen results cannot update a newer turn.

### Removed

- The experimental `hyusk-shell-agent` binary and its duplicated
  `src/bin/types.rs` message module. It described itself as "not the production
  Hyusk permission model" and was a second, diverging execute-anything path
  through a `#[path]`-included OpenRouter client; the one client implementation
  stays. `platform_tests` in `src/main.rs` fail if the binary or the duplicated
  types return (P1).

## 2026-09 — voice, workflows, and desktop control

### Added

- `HYUSK_WAKE_TEST=1` diagnostics mode: runs only the wake detector with live
  level/gate/score output and prints every detection event, so microphone and
  classifier problems are visible without starting the agent.
- `HYUSK_WAKE_CAPTURE=<path>` records the exact 16 kHz mono stream the
  classifier consumes, and the `wake_score_probe` test scores it offline
  (sliding windows) for tuning thresholds against a real voice.
- Accessibility `read` action: dumps the visible text content of an app
  natively (the fast replacement for screenshot + OCR), plus a `windows`
  action listing open windows with active state, richer element descriptions
  with embedded text, an optional `action_name` for `click`, and a
  native-first desktop-control recipe in the system prompt.

### Fixed

- Launched GUI apps no longer spill their own warnings into the agent console.
  The `process` tool and `shell ... detached` now detach child stdio, so noise
  like Brave/Chromium's harmless Puffin "Failed to create a temporary file for
  memory-mapping" line (a Flatpak sandbox restriction in its component
  updater) stays out of the agent's output.
- Wake-word classifier never fired from an exactly 2-second buffer: the
  classifier consumes the last 16 embedding windows, which a 32000-sample
  buffer cannot produce. The inference buffer is now 2.2 seconds.
- Hand-clap detector rejected almost every real clap: the onset check
  required the preceding 10 ms window to be under 20% of the clap peak,
  which real claps (with spread-out attacks) fail. The ratio is now 45%
  with a matching decay check, and the background level also adapts to
  sustained loud noise (music, TV) so clap sensitivity survives playback.
- Accessibility bridge reported success even when `doAction`, `grabFocus`,
  or `setTextContents` threw; failures are now returned as errors, and a
  click on an element without an action interface returns its screen
  bounds so the computer tool can click there instead.
- Portal clicks frequently landed on the old pointer position or were
  dropped; move-to-press and press-to-release now settle briefly, and
  drags move through intermediate points so compositors register motion.
- Shell foreground commands had no timeout and could wedge a turn for
  minutes; they are now bounded at 120 s with truncated output.
- Media volume levels 1-9 set 10x the requested volume (`0.1` instead of
  `0.01`).
- The wake detector's microphone thread could hang forever if the runtime
  died while it was paused; every resume wait is now bounded (120 s).
- Chunk-wise microphone resampling restarted its phase every callback,
  adding timing jitter on non-48 kHz microphones; the resampler is now
  phase-continuous across chunks (unit-tested against whole-buffer
  resampling).
- RNNoise state was recreated (plus an 8-frame silence warmup) on every
  wake inference; the denoiser state now persists for the session.
- Classifier input was peak-normalized with up to 8x gain, amplifying
  denoiser residue; the cap is now 2x, and the energy gate is softer so
  quiet or far speech still reaches the model.
- System prompt (wall-clock time, memory summary) was frozen at process
  start; it is rebuilt every turn.
- Wake-word inference almost never ran in time: dependency code (the ONNX
  feature extraction in `livekit-wakeword`) was compiled unoptimized in debug
  builds, taking seconds per prediction, so the 2.2 s audio window slid past
  the wake word before it was ever scored. Dependencies are now optimized even
  in dev builds, dropping inference to tens of milliseconds.
- Hand-clap detection silently stopped after startup: the scanner used a
  buffer-relative cursor that stopped advancing once the 2.5 s sliding audio
  buffer reached its cap. It now tracks absolute stream positions, so claps
  are detected for the whole session.
- The detector stopped scoring as soon as the audio went quiet, skipping the
  single best-aligned window (the wake word ending at the buffer end). A grace
  period now keeps scoring through the trailing silence.
- Microphone captures dominated by sub-100 Hz energy (DC offset and rumble
  from high hardware gain) masked speech; a 90 Hz high-pass now runs before
  denoising and clap detection.
- A loud pop when the capture stream opened could poison the clap scanner's
  background estimate and spuriously fire; the first second of audio is now
  ignored.
- PipeWire/ALSA UCM often restores the microphone capture path to maximum
  (+30 dB capture, +30 dB mic boost), clipping the ADC beyond recovery; the
  detector now warns when the input is clipping.
- Speech-to-text discarded valid commands as "background noise": its silence
  gate used fixed thresholds tuned for the old maximum-gain default, so a
  correctly calibrated (quieter) microphone failed it while loud fan noise
  could pass. The gate is now adaptive -- it compares the loud part of the
  clip against its own background -- and the captured level is printed.
- Speech-to-text fed low-frequency rumble straight to Whisper, which
  hallucinated sentences from noise; the recording is now high-passed (the
  same 90 Hz filter the wake detector uses).
- Speech-to-text now stops recording when the user stops talking. End of
  speech is detected on 20 ms frames against an adaptive, high-passed noise
  floor (the capture-start click is ignored), so it no longer runs to the full
  timeout or cuts off early on a fixed level. Tail silence defaults to 900 ms
  (tunable with `STT_SILENCE_MS`) and the hard cap is 8 s, so longer commands
  and natural pauses are not cut off (`STT_NO_SPEECH_MS` for the no-speech
  give-up).
- One shared app-alias table (`src/apps.rs`) is used by both the instant
  command router and the `process` tool, so `brave`/`chrome` resolve to their
  Flatpak commands instead of the agent trying several wrong binaries.
- "Play" commands are handled directly: `play <query> on <site>` searches that
  site, `play something`/`play music` starts music, and `play <query>` plays
  through `mpv` + `yt-dlp` when installed (instant) or otherwise opens a
  YouTube Music search.
- The wake detector kept its microphone stream open while speech-to-text
  recorded the command, opening a second capture stream. It now pauses the
  stream for the recording and resumes afterwards.
- The wake detector heard the agent's own text-to-speech and re-triggered in a
  loop (a clap on the speech onset). Hand-clap wake is now off by default, and
  while the agent is working the clap scanner is disabled and the wake-word
  threshold is raised slightly. The wake word stays live, so saying "alexa"
  during a turn interrupts it and starts listening again (barge-in), but the
  agent's own voice does not trigger it.

### Changed

- Voice turns are much faster. Common commands ("open Firefox", "go to
  YouTube", "switch to Code", "next song", "volume up") are recognized by a
  local router and executed without a model call. Longer replies stream from
  the model and are spoken sentence-by-sentence while generation continues,
  instead of waiting for the whole answer. Recording now stops 700 ms after
  speech ends (was about a second), recordings are capped at 4 s, and Whisper
  uses all CPU cores instead of a hard-coded four.
- The default model is now a fast paid one (`openai/gpt-4o-mini`); free tiers
  are frequently queued and add seconds to every turn.
- Hand-clap wake is disabled by default: background noise (doors, coughs,
  desk bumps) triggered it often enough to be a nuisance. Set
  `WAKE_CLAP_ENABLED=1` to opt back in; `alexa` remains the wake word.
- The system prompt now lists the registered tools and teaches a concrete
  strategy: work one tool at a time, prefer native accessibility, fall back to
  the `computer` tool, recover from errors with a different approach, and
  confirm destructive actions before running them.
- Memory retrieval is query-aware. Each turn ranks stored notes against the
  user's message with a small tf-idf score (tag matches weighted higher) and
  injects those, instead of only the most recently created notes.
- The accessibility bridge can now tell where the user is: a new `active`
  action reports the focused application, window, and element (and the visible
  text with `read`). `apps` includes window titles and an active flag,
  `windows` includes title, bounds, and active state, and `find`/`click`/
  `focus` can match an element by its visible `text` as well as name/role.
  Responses carry a `warnings` field when the environment limits visibility.
- The system prompt now tells the agent to start with `accessibility active`,
  and that apps publishing no tree (Chromium/Electron without
  `ACCESSIBILITY_ENABLED=1`, or any app while toolkit-accessibility is off) are
  a blind spot to fall back from to screenshots.
- Voice wake flow no longer blocks the agent event loop: recording and
  transcription run as their own cancellable task, a new wake or typed
  message interrupts an in-flight listening session, and the wake detector
  is always resumed (drop guard) even when that task is cancelled.
- Whisper inference runs on the blocking pool instead of a runtime worker.
- After a wake, recording stops shortly after speech ends instead of
  always waiting the full fixed duration, cutting wake-to-command latency.
- `WAKE_DEBUG=1` prints live level/gate/score diagnostics for tuning.
- Lowered default clap sensitivity/min-peak and wake threshold in `.env`.

### Added

- Event-driven agent runtime using Tokio MPSC channels.
- Local instant command router (`src/agent/commands.rs`) with built-in app and
  site aliases, search-on-engine support, media/volume control, window
  switching, lock/screenshot, and "take a note". Extend it with
  `~/.config/hyusk/commands.json`.
- `window` tool backed by a new `org.hyusk.Shell` D-Bus interface in the GNOME
  extension: `list`, `active`, and `activate <query>` for native window
  switching on Wayland (install with `scripts/install-gnome-indicator.sh`).
- `HYUSK_TIMING=1` prints per-stage latency (record/transcribe, model, TTS).
- `scripts/setup-accessibility.sh` enables GNOME toolkit accessibility and sets
  `ACCESSIBILITY_ENABLED=1` for Chromium/Electron apps (including Flatpak
  browsers) so more applications publish an accessibility tree; `--check` shows
  what AT-SPI currently sees.
- Memory deduplication: identical notes and graph triples are not stored twice.
- The prompt now tells the assistant to save durable facts about the user
  proactively (preferences, routines, projects, decisions, corrections).
- `HYUSK_WAKE_TEST=<seconds>` standalone microphone/classifier diagnostic
  (`HYUSK_WAKE_TEST=30 cargo run`) that prints live level/gate/score
  diagnostics and every detection.
- `HYUSK_WAKE_CAPTURE=<path>` records the exact 16 kHz stream the classifier
  consumes for offline analysis.
- `HYUSK_STT_TEST=1` records from the default microphone without a wake word
  and prints the captured level and transcription.
- `scripts/set-mic-gain.sh` sets a sane capture level and mic-boost value for
  the default microphone so the ADC does not clip.
- Replies are passed through a speech sanitizer before text-to-speech: Markdown
  markers, code fences, emoji, tables, and links are removed and symbols like
  `%` and `+` are spoken as words, so the voice no longer reads formatting
  aloud.
- The system prompt now defines Hyusk as a friendly, concise, spoken persona
  that answers in plain prose without Markdown.
- Animated transparent `eframe` status overlay.
- `computer` tool for desktop GUI control: status, screenshots, OCR, mouse
  movement/click/drag, scrolling, typing, key presses, key combos, and waits.
- Linux XDG RemoteDesktop portal backend for GNOME/Wayland input and Screenshot
  portal fallback for capture, both with one-time user approval.
- Interruptible async agent turns: a new typed message or wake word cancels the
  in-flight model request, tool call, and TTS, and rolls the conversation back
  to the previous complete state.
- Cancellable Linux TTS using `tokio::process` children with `kill_on_drop`.
- Circular black always-on-top orb with opt-in keyboard focus and an optional
  GNOME top-bar indicator mode.
- Absolute pointer movement through the portal's combined ScreenCast stream.
- Keyboard input through portal evdev keycodes with keysym fallback; the orb no
  longer steals keyboard focus by default (`ORB_STEAL_FOCUS`).
- GNOME Shell top-bar indicator (`gnome-extension/hyusk@hyusk.local`) with a
  color-coded butterfly, plus `scripts/install-gnome-indicator.sh` and the
  `$XDG_RUNTIME_DIR/hyusk-state` runtime state file.
- `batch` action for multi-step desktop workflows in one tool call.
- OCR bounding-box targeting with `find_text`/`click_text` and a `cursor`
  position action for more precise GUI control.
- Automatic temporary-screenshot cleanup via
  `HYUSK_SCREENSHOT_RETENTION_SECS`/`HYUSK_SCREENSHOT_MAX_FILES`.
- Persistent local memory tool with Markdown notes, JSON storage,
  knowledge-graph triples, and sparse bag-of-words search; recent memories are
  injected into the system prompt at startup.
- System context (OS, distribution, kernel, host, desktop/session, date/time)
  in the model system prompt.
- Linux AT-SPI2 accessibility tool (`accessibility`) backed by a Python
  `pyatspi` bridge for fast app control without screenshots.
- Multi-phrase wake support: the temporary `hey_livekit.onnx` and `alexa.onnx`
  classifiers are auto-discovered and scored together.
- Fixed `install-gnome-indicator.sh` so the temporary zip bundle is created
  correctly.
- Clipboard set/get/clear, `open_url`, and best-effort `focus_window` actions.
- Multi-model wake detection (`WAKE_WORD_MODELS`), adaptive noise-floor
  tracking, peak normalization, `WAKE_WORD_MIN_RMS`, and
  `WAKE_WORD_COOLDOWN_MS`.
- RNNoise denoising (`WAKE_WORD_DENOISE`) for fan/hiss noise before wake
  scoring.
- Multimodal screenshot support (`MODEL_VISION`): screenshots are attached to
  the next model turn as image content for vision-capable models instead of
  requiring OCR.
- `scripts/download-wake-word.sh` and automatic `models/hey_livekit.onnx`
  fallback with the `WAKE_WORD_THRESHOLD` setting.
- Wake-word detector based on CPAL and `livekit-wakeword`, with an optional
  `WakeResume` pause/resume handshake for post-wake STT.
- Whisper-based speech-to-text wired into the wake flow, plus platform-specific
  text-to-speech adapters whose completion is now awaited.
- `media` tool for MPRIS playback control through `playerctl`.
- Structured OpenAI-compatible function calling with per-tool JSON schemas.
- Retry handling for transient model requests, including `Retry-After`.
- Environment switches for `WAKE_WORD_ENABLED`, `WAKE_WORD_MODEL`, and
  `STT_MODEL`.
- Terminal stdin input path so typed messages always reach the agent, even when
  wake detection or STT is unavailable.

### Changed

- Replaced the original REPL composition with a desktop/voice-oriented startup
  sequence.
- Tool results now use `role: "tool"` messages with matching `tool_call_id`
  values.
- Tool discovery now uses `ToolRegistry::iter()` and each tool's schema instead
  of a prose description list.
- The agent keeps an in-memory conversation history across tool-call rounds.
- Wake detection is optional: a missing `WAKE_WORD_MODEL` disables detection
  instead of stopping startup.
- `WakeWordDetected` is now followed by STT transcription and a `UserInput`
  event, and the runtime resumes the detector after transcription.
- The runtime awaits TTS completion before returning the UI to `Hidden`.

### Removed

- Unused `hound`, `rodio`, and `async-std` dependencies.
- Unused `WakeWordDetector::new` single-model helper and
  `TextToSpeech::speak_async`.
- Duplicate non-cancellable Linux Piper/espeak TTS path; Linux now uses only
  the cancellable implementation.

### Known limitations at this release

- `models/hey_hyusk.onnx` is not bundled. The temporary
  `models/hey_livekit.onnx` classifier is downloaded by
  `scripts/download-wake-word.sh` and selected automatically when the custom
  model is absent.
- Wake detection and TTS are sequential. Echo cancellation depends on the
  optional PipeWire/PulseAudio module, so without it the detector relies on a
  raised threshold and a disabled clap scanner while the agent speaks.
- The `eframe` orb renders activity states only, not reply text.
- GNOME Wayland may ignore the orb's top-position and focus requests.
- The GNOME indicator extension is only scanned at session start, so the first
  install requires a logout/login.
- `focus_window` has no generic GNOME Wayland backend and can fail there; use
  the `window` tool instead.
- Approved actions are not sandboxed, and there is no process supervision
  beyond the turn-level cancellation path.

Current gaps are tracked in [docs/development.md](docs/development.md).

## v0.1 - initial agent

- REPL that loaded `.env`, registered `ShellTool` and `ProcessTool`, and parsed
  JSON tool requests from model text.
- OpenRouter-compatible chat client and in-memory agent loop.
