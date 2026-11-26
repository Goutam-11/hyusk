# Integration API

A bar widget, keybind, or script talks to the running agent through two small
runtime files. Neither is a socket, a private protocol, or a code dependency:
one file reports state (read-only), the other carries commands (write-only).

```text
$XDG_RUNTIME_DIR/hyusk-status.json    agent -> client, poll it
$XDG_RUNTIME_DIR/hyusk-control.json   client -> agent, write one command
```

`XDG_RUNTIME_DIR` is set by the session; when it is unset both files fall back
to `/tmp`. The agent reads the control file every 250 ms. A client should poll
the status file at about one second, not every frame.

## Versioning

Both files carry `"schema": 1`.

- A reader must ignore fields it does not know and keep working when a field is
  absent. A missing `schema` is treated as 1 during the transition, because the
  first indicator builds did not write one.
- A writer must keep `schema: 1` until a new version is documented here. Bump
  the schema only for a breaking change, and never remove `schema` from a file
  you write.
- The GNOME extension and the Noctalia widget are clients of this contract, not
  part of it. Their panel layout and menus can change without changing either
  file.

## `hyusk-status.json` (read-only)

The agent is the only writer. It replaces the file atomically through one writer
task, with mode `0600`, so a client never sees a half-written document. Reading
it is safe at any time; a missing or malformed file means "not ready yet", not
an error to display.

| Field | Type | Meaning |
| --- | --- | --- |
| `schema` | integer | `1`. |
| `revision` | integer | Monotonically increasing write stamp; a larger value means newer state. |
| `state` | string | `Hidden`, `Waking`, `Listening`, `Thinking`, `Working`, or `Speaking`. |
| `active_provider` | string | Provider selection, for example `openrouter`. |
| `active_model` | string | Model id currently selected. |
| `awaiting_reply` | boolean | The agent is waiting for the user's answer. |
| `latest` | object or null | The newest card: `{id, kind, text, persistent}`. `kind` is `approval` while a confirmation is armed; render Approve/Deny for that kind only. |
| `catalogs` | array | Provider catalogs: `{provider, models: [{id, label}], fresh}`. `fresh` is false when the last refresh failed and a cached list is shown. |
| `timer` | object or null | `{label, ends_at_ms}` for a running countdown; `ends_at_ms` is Unix epoch milliseconds. |

Example:

```json
{
  "schema": 1,
  "revision": 1764266400123,
  "state": "Listening",
  "active_provider": "openrouter",
  "active_model": "openai/gpt-4o-mini",
  "awaiting_reply": false,
  "latest": {
    "id": 1764266400122,
    "kind": "approval",
    "text": "Pending confirmation for: shell command=rm -rf build",
    "persistent": true
  },
  "catalogs": [
    {
      "provider": "openrouter",
      "models": [{ "id": "openai/gpt-4o-mini", "label": "GPT-4o mini" }],
      "fresh": true
    }
  ],
  "timer": null
}
```

Card text and `hyusk-status.json` are scrubbed of secret-shaped values and
credential paths before the file is written, so a client can display `text`
without another redaction pass.

## `hyusk-control.json` (one command at a time)

Write **one** JSON object and let the agent consume it. A command is single-use:
the agent remembers the last `revision` it processed and ignores any file whose
revision is not newer. Advance the revision past both the clock and whatever is
already on disk.

| Field | Type | Meaning |
| --- | --- | --- |
| `schema` | integer | `1`. |
| `revision` | integer | Positive wall-clock stamp, strictly newer than the previous command. |
| `action` | string | One of the commands below. |
| `provider` | string | `set_model`, `set_api_key`. |
| `model` | string | `set_model`. |
| `key` | string | `set_api_key`; the agent stores it in the login keyring and rewrites the file without it within one poll. |
| `workflow` | string | `run_workflow`; `text` is accepted for older writers. |
| `text` | string | `set_api_key` fallback, `run_workflow` fallback, and the embedded JSON payload for the two `schedule_*` commands. |

| Action | Fields | Effect |
| --- | --- | --- |
| `start_listening` | none | Wake the agent (same as the wake word). |
| `stop_listening` | none | Stop the current listen. |
| `stop_task` | none | Stop the running turn or task. |
| `approve` | none | Approve the armed confirmation once. |
| `approve_turn` | none | Approve the armed confirmation for the rest of the turn. |
| `deny` | none | Deny the armed confirmation. |
| `dismiss_card` | none | Dismiss the current card. |
| `set_model` | `provider`, `model` | Select a provider/model. |
| `set_api_key` | `provider`, `key` | Store a provider key in the keyring. |
| `refresh_models` | none | Refresh the model catalogs. |
| `run_workflow` | `workflow` or `text` | Run a workflow from `commands.json`. |
| `reload_workflows` | none | Acknowledge a workflow save and show a card. |
| `schedule_workflow` | `text`: `{"workflow":"…","delay_seconds":N}` | Schedule a workflow. |
| `schedule_reminder` | `text`: `{"text":"…","delay_seconds":N}` | Schedule a reminder. |
| `pair_phone` | none | Refresh the pairing code in a `mobile` build. |

`approve`, `approve_turn`, and `deny` are decisions, not user messages: a stale
card click can never start a turn or answer as the user.

`key_received` is reserved for the agent's own scrub marker; clients should not
send it. An unknown action, an unsupported schema, revision zero, or a file that
is not JSON is ignored silently, which is what makes a half-written or foreign
file harmless. A recognised command with an empty required value is reported to
the user as a card (for example an empty key or workflow name) rather than
retried.

Example commands:

```json
{ "schema": 1, "revision": 1764266400123, "action": "stop_listening" }
{ "schema": 1, "revision": 1764266400124, "action": "approve" }
{ "schema": 1, "revision": 1764266400125, "action": "set_model", "provider": "openrouter", "model": "openai/gpt-4o-mini" }
```

### Writing from a keybind or script

`hyusk_agent ctl` writes one valid command with the next revision, atomically
and with mode `0600`. It needs nothing but the installed agent binary:

```bash
hyusk_agent ctl pause
hyusk_agent ctl resume
hyusk_agent ctl stop
hyusk_agent ctl approve
hyusk_agent ctl approve-turn
hyusk_agent ctl deny
hyusk_agent ctl dismiss
```

Exit code 0 means the command was written (the agent will pick it up within
250 ms); an unknown command prints the usage and exits 2. The current revision
is read from the file, so using `ctl` twice in the same millisecond still
produces two distinct commands.

A niri keybind:

```kdl
binds {
    Mod+Shift+P { spawn "hyusk_agent" "ctl" "pause"; }
    Mod+Shift+R { spawn "hyusk_agent" "ctl" "resume"; }
    Mod+Shift+A { spawn "hyusk_agent" "ctl" "approve"; }
}
```

### Writing from a bar widget

A widget reads the status file and writes a control file. The write is a
temporary file in the same directory followed by a rename, so the agent never
sees a partial command:

```js
const fs = require("node:fs");
const path = require("node:path");

const runtime = process.env.XDG_RUNTIME_DIR || "/tmp";
const statusPath = path.join(runtime, "hyusk-status.json");
const controlPath = path.join(runtime, "hyusk-control.json");

function readStatus() {
  try {
    return JSON.parse(fs.readFileSync(statusPath, "utf8"));
  } catch {
    return null;
  }
}

function send(action) {
  let previous = {};
  try {
    previous = JSON.parse(fs.readFileSync(controlPath, "utf8"));
  } catch {
    // No command yet, or a foreign file: start from the clock.
  }
  const revision = Math.max(Date.now(), Number(previous.revision || 0) + 1);
  const temporary = `${controlPath}.${process.pid}.tmp`;
  fs.writeFileSync(
    temporary,
    JSON.stringify({ schema: 1, revision, action }),
    { mode: 0o600 }
  );
  fs.renameSync(temporary, controlPath);
}

// A click on the card:
send("approve");
```

Reading is equally small:

```js
const status = readStatus();
const label = status?.latest?.kind === "approval" ? "Approve" : status?.state;
```

### Writing from a shell script

Prefer `hyusk_agent ctl`; it handles the revision. If JSON must be written by
hand, advance the revision and replace the file atomically:

```bash
runtime="${XDG_RUNTIME_DIR:-/tmp}"
revision="$(date +%s%3N)"
tmp="$runtime/.hyusk-control.$$"
printf '{"schema":1,"revision":%s,"action":"start_listening"}\n' "$revision" > "$tmp"
chmod 600 "$tmp"
mv "$tmp" "$runtime/hyusk-control.json"
```

## What this API is not

- It is not the compositor window service. `org.hyusk.Shell` is the GNOME
  window backend used by the `window` tool inside `src/desktop/`; a third-party
  bar should not implement or depend on it. Window control is a desktop-adapter
  concern, not an indicator feature.
- It is not a remote control channel. Both files live in the per-session runtime
  directory and are owner-only; there is no network listener, and the control
  file is validated by the same parser for a plugin, `hyusk_agent ctl`, and a
  hand-written command.
- It is not a place for secrets beyond `set_api_key`. The agent moves a key into
  the login keyring and rewrites the file without it; never write a key into the
  status file or into a command another process can read.
