# Hyusk Mobile Link Protocol v1

Mobile Link is JSON-RPC 2.0 over a certificate-pinned secure WebSocket. The
phone initiates connections to laptop Hyusk over LAN or Tailscale; there is no
cloud relay.

## Pairing and authentication

The laptop creates a TLS identity and a five-minute, single-use pairing secret.
The paste payload contains the protocol version, one advertised endpoint, the
TLS certificate fingerprint, and that secret. During pairing, the phone
generates a random per-device secret and protects it with Keystore-backed
encrypted storage. Normal requests prove possession with an HMAC and a strictly
increasing sequence number. Revoked devices and reused or expired challenges
are rejected.

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
