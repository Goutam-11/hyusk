# Noctalia bar widget (niri, Hyprland, Sway)

`noctalia-plugin/` is the Noctalia v5 counterpart of the
[GNOME top-bar indicator](gnome-indicator.md). GNOME Shell extensions cannot
load under niri, so this is a bar widget plugin instead: a Tabler glyph that
follows the agent's state, with the latest card in its tooltip, plus a settings
panel for switching models and saving API keys.

## Install

```bash
./scripts/install-noctalia-plugin.sh
```

Then add the widget to a bar once, in **Settings → Bar → Bar Widgets → Hyusk**
(or `noctalia msg settings-open bar`). Noctalia stores the placement in
`$XDG_STATE_HOME/noctalia/settings.toml`, which is why the script leaves that
one step to the GUI.

## Usage

| Gesture | Result |
| --- | --- |
| Left click | Start listening, exactly as after the wake word (stop while busy) |
| Gear | Open the model and API-key panel |
| Right click | Dismiss the response card |
| Hover | State, provider/model, and the latest reply |
| Scroll / keybind | `noctalia msg plugin hyusk/indicator:hyusk all start_listening` (or `stop_task`) |

The glyph follows the state: butterfly when idle, ear when waking, microphone
while listening, brain while thinking, tool while working, speaker while
speaking. A card asking for confirmation turns it into a shield.

## Model and API key panel

The gear opens `hyusk/indicator:settings`:

- **Model** — a provider picker plus its catalog, with a filter because
  OpenRouter lists hundreds of models. Choosing one stores the selection in
  `~/.config/hyusk/config.json` and starts a fresh conversation on that model.
- **API key** — one field per provider (OpenRouter, OpenAI, Amazon Bedrock).
  The key is handed to the agent, which writes it to the login keyring
  (`secret-tool`, application `hyusk`), registers the provider immediately and
  reloads its catalog — no service restart. The agent overwrites the control
  file as soon as it has the key, so the secret does not linger in the runtime
  directory.
- **Refresh** fetches new catalogs; providers without a key are listed as
  `<provider> · key needed`.

Nova Sonic (AWS profile) and Codex (CLI login) appear as selectable providers
because they authenticate outside the keyring.

## How it works

The widget reads the same runtime files the GNOME indicator uses, so nothing in
the agent is Noctalia-specific:

```text
$XDG_RUNTIME_DIR/hyusk-status.json   state, model, catalogs, latest card
        |                            polled every 1000 ms (900 ms while the
        v                            panel is open)
barWidget.render / setTooltip  ·  panel.render

$XDG_RUNTIME_DIR/hyusk-control.json  {revision, action, provider, model, key}
        ^                            written by a click, polled by the agent
        |                            every 250 ms
hyusk-control.json consumer in src/main.rs
```

Actions the panel sends: `set_model`, `set_api_key`, and `refresh_models`.
`set_api_key` is the only one that carries a secret, and the agent rewrites that
file to drop it within one poll interval.

Each action advances `revision` past whatever is on disk, because the agent
ignores any control whose revision is not newer than the last one it processed.

## Limits

- The widget only exists while it is placed on a bar; an unplaced plugin entry
  cannot be addressed with `noctalia msg plugin`. The panel needs no placement.
- Saving a first key works immediately, but Hyusk must already be running: the
  panel talks to the agent, not to the keyring directly.
- Middle click belongs to Noctalia (it opens the widget's settings), so the
  widget does not use it.
- Approving a pending action still means saying yes or no: the widget shows that
  a confirmation is waiting but deliberately does not approve on a click.
