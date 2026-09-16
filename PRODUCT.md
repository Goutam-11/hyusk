# Product

<!-- impeccable:product-schema 1 -->

## Platform

adaptive

## Users

Hyusk is primarily a personal computer assistant for its owner on Fedora Linux with GNOME, with Hyprland also present. The main job is to control the computer quickly by voice and to automate repeated multi-step routines without requiring an LLM for deterministic actions.

## Product Purpose

Hyusk listens for a personalized wake phrase, understands spoken commands, performs native system actions, and exposes visible status when audio is unavailable. Success means common commands and reusable workflows are fast, inspectable, dependable, and controllable without repeatedly opening a terminal or composing prompts.

## Positioning

Hyusk combines a local, voice-first Linux desktop agent with deterministic user-authored workflows. The same workflow can be invoked by voice, from a native application, or from the desktop indicator while the long-running service owns execution and safety.

## Operating Context

Hyusk runs as a user service in a Fedora graphical session. A GNOME Shell extension provides quick status and controls. Workflows currently live in `~/.config/hyusk/commands.json` and use the built-in deterministic command vocabulary. The native workflow application is the primary place to discover, create, edit, test, search, and run workflows.

## Capabilities and Constraints

- Existing version-one workflow files and voice triggers must remain compatible.
- The Hyusk service remains the execution authority; UI clients must not independently execute privileged actions.
- Safe operations should run without confirmation. Dangerous operations retain explicit approval.
- The application should use GTK4 and libadwaita and follow native GNOME behavior.
- The GNOME extension remains lightweight and opens the workflow application for editing.
- Workflows must function without an LLM; LLM assistance may be optional rather than required.
- The first release supports dependable linear workflows before adding branching, loops, and variables.

## Brand Commitments

The product name is Hyusk. The butterfly mark and the existing blue-violet accent are recognizable assets. The interface should feel like a native Fedora/GNOME utility while matching the clarity and direct manipulation users expect from Apple Shortcuts.

## Evidence on Hand

- The Rust workflow parser, validator, and deterministic command router are implemented in `src/agent/commands.rs`.
- The GNOME extension already reads and writes workflows and can request workflow execution.
- Existing configuration and examples are documented in `README.md`.
- No customer claims, public benchmarks, or third-party endorsements are available and none should be fabricated.

## Product Principles

- Native actions first; use an LLM only where reasoning is actually needed.
- Make every automation understandable before it runs.
- Keep voice, app, and extension controls consistent.
- Preserve user control with clear progress, cancellation, and focused safety prompts.
- Prefer reliable, fast primitives over clever but fragile automation.

## Accessibility & Inclusion

Core workflow creation and execution must be keyboard accessible, expose meaningful assistive-technology labels, avoid color-only state communication, and remain usable when the system is muted.
