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
| `device.hello` | both | Version, device identity, and capability negotiation. |
| `device.ping` | both | Liveness without changing state. |
| `turn.submit` | phone → laptop | Submit text with source and target device. |
| `turn.cancel` | both | Cancel the named turn or remote action. |
| `device.invoke` | laptop → phone | Invoke one structured phone capability. |
| `approval.resolve` | both | Approve or deny the exact pending operation. |
| `event.subscribe` | phone → laptop | Subscribe to state, response, tool, and approval events. |
| `memory.sync` | both | Exchange revisioned encrypted memory records. |
| `workflow.sync` | both | Exchange revisioned workflow records. |

Every state-changing request includes a correlation identifier, source device,
target device, timeout, declared risk, and idempotency key where replay is safe.
Unknown fields are ignored only when protocol compatibility says they are
optional; unknown methods return the standard JSON-RPC method-not-found error.

## Reconnect and replay

The client reconnects with capped exponential backoff. Only idempotent state
sync and explicitly scheduled operations may remain queued. Calls, messages,
clipboard reads, destructive actions, shell commands, and approvals are never
replayed after a disconnect.

Memory and workflow records include UUID, scope, origin device, base laptop
revision, current laptop revision, encrypted payload, and tombstone. A stale
base revision produces a visible conflict; neither side silently overwrites it.
