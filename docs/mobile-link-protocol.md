# Hyusk Mobile Link Protocol v1

Mobile Link is JSON-RPC 2.0 over a certificate-pinned secure WebSocket. The
phone initiates connections to laptop Hyusk over LAN or Tailscale; there is no
cloud relay.

## Building the laptop side

The link is the `mobile` Cargo feature and is off by default (P5), so a plain
agent build has no TLS listener, no pairing file, and no `mobile` tool:

```bash
cargo build --release --features mobile
```

`scripts/setup-mobile-link.sh --enable` writes the `HYUSK_LINK_*` settings and
restarts the service, but the running binary must have been built with the
feature; the panel's pair button reports a build without the link. Link tests
run under the same feature.

## Pairing and authentication

The laptop creates a TLS identity and a five-minute, single-use pairing secret.
The paste payload contains the protocol version, one advertised endpoint, the
TLS certificate fingerprint, and that secret. During pairing, the phone
generates a random per-device secret and protects it with Keystore-backed
encrypted storage. Normal requests prove possession with an HMAC and a strictly
increasing sequence number. Revoked devices and reused or expired challenges
are rejected. The sequence is per device, not per connection: the laptop keeps
the highest value it has accepted and rejects anything at or below it, even on a
new connection (see Reconnect and replay).

Neither side transfers model-provider keys. Frame size, authentication time,
and method execution time are bounded. Text received from either device is
untrusted input, not a system instruction.

## Core methods

| Method | Direction | Purpose |
| --- | --- | --- |
| `device.hello` | both | Version and device identity; the phone sends capability flags, which v1 ignores. |
| `device.ping` | both | Liveness without changing state. |
| `turn.submit` | phone → laptop | Submit text with source and target device. |
| `turn.cancel` | both | Cancel the named turn or remote action. |
| `device.invoke` | laptop → phone | Invoke one structured phone capability. |
| `approval.resolve` | both | Approve or deny the exact pending operation. |
| `event.subscribe` | phone → laptop | Subscribe to state, response, tool, and approval events. |
| `memory.sync` | both | Reserved in v1: memory records are accepted and acknowledged but not merged. |
| `workflow.sync` | both | Reserved in v1: workflow records are accepted and acknowledged but not merged. |

Every state-changing request includes a correlation identifier, source device,
target device, timeout, declared risk, and idempotency key where replay is safe.
Unknown fields are ignored only when protocol compatibility says they are
optional; unknown methods return the standard JSON-RPC method-not-found error.
`turn.submit` currently acts on `text` and echoes `turn_id`; `source_device`,
`target_device`, `timeout_ms`, `idempotency_key`, and `workflow` are accepted
but not yet enforced.

## Sync methods (reserved in v1)

`memory.sync` and `workflow.sync` are accepted for forward compatibility. The
phone advertises both capabilities in `device.hello`, but the laptop ignores the
flags and v1 does not merge data. The laptop validates the payload, logs the
item count, drops it, and answers `{"accepted": true}`. No records are returned,
stored, or applied, so phone and laptop storage remain separate (see
[android-agent.md](android-agent.md)).

The payloads are `memory.sync` `{cursor?, items[]}` with
`{id, content, updated_at?}` items and `workflow.sync` `{revision?, workflows[]}`
with `{id, name, prompt, enabled}` items. These shapes are the v1 wire format,
not a merge contract.

The target semantics, to be enabled only once conflict, deletion, and
encryption are defined: records carry a UUID, scope, origin device, base laptop
revision, current laptop revision, encrypted payload, and tombstone. A stale
base revision produces a visible conflict; neither side silently overwrites it.

## Reconnect and replay

The client reconnects with capped exponential backoff. Once state sync is
implemented, only idempotent sync requests and explicitly scheduled operations
may remain queued; v1 does not replay the reserved sync methods on its own.
Calls, messages, clipboard reads, destructive actions, shell commands, and
approvals are never replayed after a disconnect.

Every authenticated request carries `auth.sequence`, and the laptop keeps the
highest sequence it has accepted for each paired device in the device store
(`link-devices.json`, mode `0600`). A request whose sequence is not strictly
greater than that value is rejected with `invalid or replayed request`, whatever
connection it arrives on and whether or not the laptop restarted in between. The
counter is written with the device map, so it outlives the connection and the
process; a captured request can therefore not be replayed by opening a new
connection.

`device.hello` returns the accepted value as `result.sequence`, and a client
continues from it: a client with no local state (a fresh app process) seeds its
counter from the reply and increments from there. A client must never restart
its counter at zero while the pairing lives. The wire format is unchanged
(`hyusk.link.v1`); this is a rule the laptop enforces and the phone must honour,
and the in-repo Android client seeds its counter from the hello reply.

The MAC is checked before the sequence, so an unauthenticated frame can never
advance the counter. A failure to write the device store is logged and the
in-memory mark still applies for the running process.
