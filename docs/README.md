# hyusk_agent documentation

These documents describe the current implementation in this directory, not the
separate `hyusk` workspace CLI.

- [Architecture](architecture.md) explains the modules, event flow, model
  protocol, and runtime boundaries.
- [Architecture roadmap](architecture-roadmap.md) records the agreed pruning,
  tool boundary, memory, and preference plan; the closed work record is in
  [`issues/README.md`](../issues/README.md).
- [Tools](tools.md) documents the tool contracts, examples, extension process,
  and security implications.
- [Development](development.md) covers setup, build commands, validation, and
  troubleshooting.
- [The propose-diff loop](self-upgrade.md) states how the agent may change the
  product it runs on: a branch, verified tests, a human review and merge, and a
  supervised restart. It names the safety and update files that are
  human-only.
- [Voice and UI](voice-and-ui.md) describes the wake-word, STT, TTS, and
  overlay pipeline and its remaining limitations.
- [GNOME top-bar indicator](gnome-indicator.md) documents the Shell extension,
  its menus, and the runtime state/status/control files.
- [Noctalia bar widget](noctalia-indicator.md) covers the niri/Hyprland/Sway
  widget and its model and API-key panel.
- [Browser control](browser-control.md) covers the CDP automation profile used
  by the `browser` tool.
- [Android companion](android-agent.md) documents the phone app, and the
  [mobile link protocol](mobile-link-protocol.md) its versioned wire contract.
- [Bedrock Nova Sonic](bedrock-nova-sonic.md) covers the speech-to-speech
  provider, AWS credentials, and live testing.

Research and audit notes are kept for context and may describe work that has
since shipped: [agent-loop research](agent-loop-research.md) and the
[laptop agent audit](laptop-agent-audit.md).

The top-level [`README.md`](../README.md) is the starting point for running the
binary and understanding its current status.
