# GNOME top-bar indicator

`gnome-extension/hyusk@hyusk.local` is a small GNOME Shell extension that shows
the Hyusk agent state in the top bar as an animated, color-coded butterfly.

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
`Thinking`, `Working`, `Speaking`) to `$XDG_RUNTIME_DIR/hyusk-state` on every
state change. The extension reads that file, smoothly interpolates the color,
and animates the wings. Animation speed changes by state: working flaps
fastest, speaking bounces, thinking tilts, idle stays calm.

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
