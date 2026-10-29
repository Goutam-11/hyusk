# Noctalia bar widget (niri, Hyprland, Sway)

`noctalia-plugin/` is the Noctalia v5 counterpart of the
[GNOME top-bar indicator](gnome-indicator.md). GNOME Shell extensions cannot
load under niri, so this is a bar widget plus a panel instead: the bar carries
what the GNOME panel button carries (state glyph, state name, a running
timer's countdown) and the panel behind the gear carries the rest of the GNOME
menu in four tabs.

## Install

```bash
./scripts/install-noctalia-plugin.sh
```

The installer copies the widget, the panel, and the two companion scripts the
Device tab runs (`setup-mobile-link.sh`, `start-browser-control.sh`) into the
plugin directory.

Then add the widget to a bar once, in **Settings → Bar → Bar Widgets → Hyusk**
(or `noctalia msg settings-open bar`). Noctalia stores the placement in
`$XDG_STATE_HOME/noctalia/settings.toml`, which is why the script leaves that
one step to the GUI.

## Updating

Re-run `./scripts/install-noctalia-plugin.sh` after changing the plugin. It
copies the new files and enables `hyusk/indicator`; if the running shell keeps
serving the old code, toggle the plugin once so its entry scripts reload:

```bash
noctalia msg plugins disable hyusk/indicator
noctalia msg plugins enable hyusk/indicator
```

A widget already placed on a bar keeps its placement.

## Usage

| Gesture | Result |
| --- | --- |
| Left click | Start listening, exactly as after the wake word (stop while busy) |
| Gear | Open the Hyusk panel |
| Right click | Dismiss the response card |
| Hover | State, provider/model, running timer, and the latest reply |
| Keybind | `noctalia msg plugin hyusk/indicator:hyusk all start_listening` (or `stop_task`, `dismiss_card`) |

The glyph follows the state: butterfly when idle, ear when waking, microphone
while listening, brain while thinking, tool while working, speaker while
speaking. A card asking for confirmation turns it into a shield.

Beside the glyph the widget spells the state out (`listening`, `thinking`, …),
as the GNOME indicator's panel label does, and adds an hourglass with the
countdown while a timer runs. The resting `idle` state leaves the label off, so
an idle bar stays quiet.

## The panel

The gear opens `hyusk/indicator:settings`, whose tabs hold the GNOME menu's
sections. The API key form stays pinned below the tabs, so a key can be saved
from any tab.

- **Agent** — **Listen**, **Stop**, and **Stop listening** while the microphone
  is open; the latest reply with **More**/**Less**, **Copy**, and **Dismiss**;
  **Approve**, **Allow for this turn**, and **Deny** while a confirmation is
  pending. A running timer shows
  its countdown here as big digits (`25:00`), ticking once a second — the same
  treatment the GNOME card gives it, including a `MM:SS` clock written in the
  reply itself.
- **Models** — a provider picker plus its catalog, with a filter because
  OpenRouter lists hundreds of models. Choosing one stores the selection in
  `~/.config/hyusk/config.json` and starts a fresh conversation on that model.
  **Refresh** (in the header) fetches new catalogs; providers without a key are
  listed as `<provider> · key needed`.
- **Workflows** — every named workflow in `~/.config/hyusk/commands.json`,
  filterable by name or spoken phrase; clicking one runs it. **Open editor**
  launches `hyusk-workflows`. The agent re-reads that file on each request, so
  an editor save is visible here immediately, with no service restart.
- **Device** — phone pairing and the automation browser. A live one-time code
  from `$XDG_RUNTIME_DIR/hyusk-pairing.json` is shown with **Copy code** (paste
  it into the phone's Devices page), **Generate fresh code** refreshes it, and
  **Enable laptop link** runs `setup-mobile-link.sh --enable` when the link is
  off. **Open automation browser** starts Hyusk's dedicated Brave/Chromium/Chrome
  profile for CDP automation, which has its own cookies and sign-ins — see
  [Browser control](browser-control.md).
- **API key** — one field per provider (OpenRouter, OpenAI, Amazon Bedrock).
  The key is handed to the agent, which writes it to the login keyring
  (`secret-tool`, application `hyusk`), registers the provider immediately and
  reloads its catalog — no service restart. The agent overwrites the control
  file as soon as it has the key, so the secret does not linger in the runtime
  directory.

Nova Sonic (AWS profile) and Codex (CLI login) appear as selectable providers
because they authenticate outside the keyring.

## How it works

The widget reads the same runtime files the GNOME indicator uses, so nothing in
the agent is Noctalia-specific:

```text
$XDG_RUNTIME_DIR/hyusk-status.json   state, model, catalogs, latest card, timer
        |                            polled every 1000 ms by the widget, and once
        v                            a second by the open panel
barWidget.render / setTooltip  ·  panel.render

$XDG_RUNTIME_DIR/hyusk-control.json  {revision, action, provider, model, key}
        ^                            written by a click, polled by the agent
        |                            every 250 ms
hyusk-control.json consumer in src/main.rs
```

Actions the widget and panel send: `start_listening`, `stop_task`,
`stop_listening`, `approve`, `deny`, `dismiss_card`, `set_model`,
`set_api_key`, `refresh_models`, `run_workflow`, `pair_phone`. `set_api_key` is
the only one that carries a secret, and the agent rewrites that file to drop it
within one poll interval.

Each action advances `revision` past whatever is on disk, because the agent
ignores any control whose revision is not newer than the last one it processed.

Second boundaries (not every frame) drive the panel: a once-a-second status poll
and the countdown repaint, and only the Agent tab's digits trigger one.

## Limits

- Timers belong to the agent: the panel shows a countdown but cannot cancel one.
  Ask the agent to stop it.
- The Device tab needs scripts the current installer copies. An older install
  says so instead, and re-running `scripts/install-noctalia-plugin.sh` fixes it.
- The widget only exists while it is placed on a bar; an unplaced plugin entry
  cannot be addressed with `noctalia msg plugin`. The panel needs no placement.
- Saving a first key works immediately, but Hyusk must already be running: the
  panel talks to the agent, not to the keyring directly.
- Middle click belongs to Noctalia (it opens the widget's settings), so the
  widget does not use it.
- Approving a pending action only happens through the explicit Approve/Deny
  buttons; a click on the widget itself never approves anything.
