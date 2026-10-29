# GNOME top-bar indicator

`gnome-extension/hyusk@hyusk.local` is a small GNOME Shell extension that shows
the Hyusk agent state in the top bar as an animated pearl-white butterfly with a
state-colored aura.

## How it works

The butterfly is drawn directly with Cairo in a `St.DrawingArea`, not an emoji
glyph. That is what allows the color to change per state and the wings to
animate.

```text
HyuskState::publish()
   |
   v
$XDG_RUNTIME_DIR/hyusk-state
   |
   v
GNOME Shell extension polls every 500 ms
   |
   v
top-bar butterfly color/label
```

The Rust side writes the current state name (`Hidden`, `Waking`, `Listening`,
`Thinking`, `Working`, `Speaking`) to `$XDG_RUNTIME_DIR/hyusk-state` and richer
card/model state to `$XDG_RUNTIME_DIR/hyusk-status.json`. The extension reads
those files, smoothly interpolates the color, and animates the wings. Animation
speed changes by state: working flaps fastest, speaking bounces, thinking tilts,
idle stays calm.

The indicator menu is grouped by intent: a state header, **Listen** and **Stop**
transport keys, then Workspace (searchable **Run workflow**, **Workflows**),
Model (model picker, **Refresh models**), Device (**Connect phone**, **Open
automation browser**), and Settings (**API keys**). The quick-run submenu
searches workflow names and voice triggers, while **Workflows** opens the editor
app. The model picker sends a small control JSON file back to the running agent;
it does not store API keys. Keys saved from the API keys submenu go to the login
keyring via `secret-tool`, and the indicator restarts `hyusk.service` so the
service registers the provider with the new key. A compact response card appears below the top bar near the pearl-white
butterfly, with Approve/Deny, More, Copy, and Dismiss actions; it pauses its
auto-dismiss while the pointer or keyboard focus is inside it. The wings stay
recognizable across themes; runtime state is communicated by the colored aura,
the motion, and the state color on the menu header and response card.

**Open automation browser** starts Hyusk's dedicated Brave, Chromium, or Chrome
profile for CDP automation. It is separate from the normal browser profile and
has its own cookies and sign-ins. See [Browser control](browser-control.md) for
setup, profile, and connection details.

## Workflow execution and background work

Run requests from the app, voice, and Shell menu go to the Hyusk service. The
service keeps workflows in a single queue and runs one workflow at a time.
Workflows containing desktop actions wait until the current model or voice
interaction ends; a new foreground voice request cancels an active desktop
workflow so the agent can respond. Background-only workflows can run while a
model turn is active, even if an earlier desktop workflow is waiting. Timers and media controls use their own service paths, so
they can continue without taking over the foreground conversation. The UI
reports queued, running, completed, failed, and cancelled workflow states.

## Updating the extension

GNOME Shell loads extension JavaScript into the running Shell process. Replacing
the installed files does not reload that JavaScript immediately. Disable and
re-enable the extension or log out and back in to load the updated code. On
Wayland, logging out and back in is the reliable Shell reload; the `Alt`+`F2`,
`r` shortcut applies to Xorg sessions. A new panel menu item may therefore not
appear until the next Shell reload or login.

The checked-in control-deck review image
([`workflows-desktop.png`](../.impeccable/review/workflows-desktop.png)) is an
actual GTK render populated with illustrative sample workflows from a temporary
config. It does not show the user's configured routines and is not a live GNOME
Shell render.

For phone pairing, open **Connect phone → Enable laptop link** once. This
creates a local TLS identity, configures the Hyusk user service, and restarts it.
Then choose **Generate fresh code** and **Copy pairing code**. Transfer the code
privately to the phone's Devices page and tap **Pair and connect**. Codes expire
after five minutes and work once; generating another invalidates the previous
code. The secret is kept in an owner-only runtime file, not printed in the
service journal. Both devices must be able to reach the laptop's advertised LAN
address and port 4488; a firewall may need a trusted-network rule.

## Source layout

`extension.js` is only the entry point. The UI lives in small modules:

| Path | Purpose |
| --- | --- |
| `extension.js` | Enables the indicator and exports the D-Bus window service |
| `lib/indicator.js` | Panel button, status polling, state fan-out |
| `lib/drawing.js` | Animated butterfly and state aura |
| `lib/responseCard.js` | Floating reply / approval card |
| `lib/menu/deck.js` | Menu structure and sections |
| `lib/menu/quickRun.js` | Searchable workflow submenu |
| `lib/menu/models.js` | Model picker with provider groups |
| `lib/menu/phone.js` | Phone pairing submenu |
| `lib/menu/credentials.js` | API key submenu |
| `lib/windows.js` | Window list/activate/close over D-Bus |
| `lib/runtime.js` | Runtime and `commands.json` file access |
| `lib/state.js` | State names, labels, and the palette |
| `lib/motion.js` | Reduced-motion policy |
| `lib/launcher.js` | Launching the workflow app and automation browser |

State colors live in `lib/state.js` and are written out per state class in
`stylesheet.css`. St's CSS parser has no custom properties, so the two must be
kept in sync when the palette changes.

## Install

```bash
./scripts/install-gnome-indicator.sh
```

GNOME Shell only scans extensions when the session starts. Log out and back in,
then enable **Hyusk Agent Indicator** in the Extensions app if it is not already
enabled.

## Orb behavior

The agent skips its separate orb window when the extension is enabled in
GNOME Shell. Override with:

| Variable | Behavior |
| --- | --- |
| `HYUSK_ORB=1` | Always show the orb window. |
| `HYUSK_ORB=0` | Never show the orb; indicator only. |
| unset | Show the orb only when the extension is not enabled. |

## State colors

| State | Color |
| --- | --- |
| `Hidden` | grey |
| `Waking` | light blue |
| `Listening` | green |
| `Thinking` | violet |
| `Working` | orange |
| `Speaking` | pink |

## Notes

The extension uses only GNOME Shell APIs and does not require the AppIndicator
extension.

The state file lives in the per-user runtime directory and is recreated on
each state change. To test it manually from any shell (including fish):

```bash
./scripts/set-hyusk-state.sh Working
```

If the butterfly does not appear, verify the extension is enabled:

```bash
gnome-extensions list --enabled | grep hyusk
```
