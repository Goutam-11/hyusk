---
name: Hyusk Workflows
description: Native GNOME workflow editing with a voice-first Hyusk status surface.
colors:
  primary-blue-violet: "#82aaff"
  success-green: "#50be82"
  warning-orange: "#ffaa6e"
  destructive-red: "#dc5050"
  response-surface: "#20232a"
  white: "#ffffff"
  state-hidden: "#9aa0a6"
  state-waking: "#82d2ff"
  state-listening: "#78e6aa"
  state-thinking: "#b4a0ff"
  state-working: "#ffaa6e"
  state-speaking: "#ff82c8"
typography:
  body:
    fontFamily: "system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.35
  title:
    fontFamily: "system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 700
    lineHeight: 1.2
  extension-meta:
    fontFamily: "system-ui, sans-serif"
    fontSize: "11px"
    fontWeight: 400
    lineHeight: 1.2
rounded:
  native: "Adwaita default"
  extension-control: "7px"
  response-card: "12px"
spacing:
  extension-control: "6px 10px"
  response-card: "12px"
  editor-form: "18px"
components:
  button-primary:
    backgroundColor: "{colors.success-green}"
    textColor: "{colors.white}"
    rounded: "{rounded.native}"
    padding: "native Adwaita button padding"
  button-run:
    backgroundColor: "{colors.primary-blue-violet}"
    textColor: "{colors.white}"
    rounded: "pill"
  extension-control:
    backgroundColor: "rgba(255,255,255,0.10)"
    textColor: "{colors.white}"
    rounded: "{rounded.extension-control}"
    padding: "{spacing.extension-control}"
---

# Design System: Hyusk Workflows

## Overview

**Creative North Star: “The Native Shortcut Desk”**

Hyusk Workflows is a focused Fedora/GNOME utility for making deterministic desktop routines legible before they run. The primary surface is a GTK4 and libadwaita application: a searchable workflow library sits beside an editor for the selected shortcut, using platform-standard controls and the system theme rather than a separate visual skin. The GNOME Shell extension is the lightweight companion: a small animated butterfly indicator, a status/response card, and a compact workflow launcher.

The visual language is calm, direct, and inspectable. Blue-violet is the recognizable Hyusk action accent; green confirms a save/create action, orange carries warning or active-work meaning, and red is reserved for deletion. The application delegates execution to the Hyusk service, so controls describe and request actions rather than presenting an independent automation runtime.

**Key Characteristics:**

- Native Adwaita surfaces, selection behavior, dialogs, and system typography.
- Searchable library rail paired with a spacious, full-width action editor.
- Explicit status feedback and destructive confirmation.
- Small, state-colored butterfly indicator for at-a-glance runtime status.

## Colors

The GTK application inherits the active Adwaita light/dark palette; the explicit values below are the colors authored by the GNOME Shell companion and its workflow dialog.

### Primary

- **Hyusk Blue-Violet** (#82aaff): New, run, draft-selection, and other direct workflow controls in the Shell surface.

### Secondary

- **Confirmation Green** (#50be82): Create/save confirmation control in the Shell workflow dialog.

### Tertiary

- **Active Orange** (#ffaa6e): Warning feedback and the Working state.
- **State Pink** (#ff82c8): Speaking state only.

### Neutral

- **Response Charcoal** (#20232a): Nearly opaque response-card surface in GNOME Shell.
- **White** (#ffffff): Response-card body and timer text; white is used with alpha for secondary copy and controls.
- **Muted Gray** (#9aa0a6): Hidden/idle butterfly state.
- **Sky Blue** (#82d2ff): Waking butterfly state.
- **Listening Green** (#78e6aa): Listening butterfly state.
- **Thinking Violet** (#b4a0ff): Thinking butterfly state.
- **Destructive Red** (#dc5050): Delete control in the Shell workflow dialog.

### Named Rules

**The Semantic Accent Rule.** Blue-violet signals an available Hyusk action; green confirms persistence; orange explains attention or active work; red means deletion. Do not repurpose these meanings casually.

## Typography

**Display Font:** System UI supplied by GTK/Adwaita.
**Body Font:** System UI supplied by GTK/Adwaita.
**Label/Mono Font:** None defined; use native system labels.

**Character:** The native app is intentionally unbranded in typography: readable GNOME system text, platform hierarchy, and standard label treatments. The Shell response card adds only small explicit sizes where the compact overlay needs them.

### Hierarchy

- **Title** (700, 18px in the Shell workflow dialog): Dialog title and prominent workflow headings.
- **Heading** (native Adwaita heading style): Workflow names, field labels, and section labels in the GTK editor.
- **Body** (native system weight and size): Intro copy, workflow fields, action rows, and status messages.
- **Label** (native system label; 11px metadata in Shell): Secondary state, provider/model metadata, and compact helper text.
- **Timer** (800, 34px): Large minute/second readout in the Shell response card.

## Layout

The native workflow window opens at 1040×700. Its `NavigationSplitView` keeps a library sidebar at 260–360px and gives the editor the remaining width. The sidebar uses 14px horizontal margins, 18px top margin, a search entry, and a vertically scrolling single-selection `ListBox`; rows have 12px horizontal and 10px vertical content insets. The editor is a vertically scrolling form with 32px horizontal margins, 28px vertical margins, and an 18px section rhythm.

The editor order is stable: title and explanatory copy, Name, Voice triggers, Steps, status, and action controls. Steps are full-width horizontal rows with an expanding entry followed by move-up, move-down, and remove icon buttons. A common-action dropdown and “Add custom step” control sit in the Steps header. On smaller available widths, rely on the native split-view collapse and scrolling behavior; do not introduce a second bespoke breakpoint system.

The Shell response card is a 330px-wide overlay with 12px padding and 8px internal spacing. The Shell workflow dialog uses a 760px content width, a 180px scrolling result list, and a compact stacked editor. GNOME panel placement remains the right-side status area.

## Elevation & Depth

The GTK workflow app defines no custom shadows or elevation tokens; depth comes from Adwaita’s native surfaces, list selection, split-view separation, and modal alert dialog. The Shell response card is an opaque overlay (`rgba(32, 35, 42, 0.98)`) and the workflow dialog is modal. Keep resting surfaces flat and let native GNOME treatment communicate hierarchy.

## Shapes

The native application follows Adwaita’s default control radii and silhouettes. Use boxed-list treatment for the workflow library and standard GTK entries, text views, dropdowns, buttons, and alert dialogs. The Shell companion uses 7px rounded controls and a 12px response card; the Run control in the GTK header uses a pill style. Avoid decorative cards, custom corner geometry, or heavy borders that fight the platform surface language.

## Components

### Workflow library and search

- **Shape:** Single-selection Adwaita boxed list in a scrolling sidebar.
- **Content:** Workflow name in heading treatment and a dim summary of step/trigger counts.
- **Search:** Native `SearchEntry` with “Search workflows” placeholder; matching includes names, phrases, and steps.
- **Empty state:** Dim, wrapped copy distinguishes no workflows from no search matches.

### Workflow editor

- **Character:** Direct manipulation of a readable ordered shortcut.
- **Fields:** Name entry, multi-line voice-trigger `TextView`, and editable step entries.
- **Actions:** Native action dropdown offers common Hyusk commands; custom steps remain available.
- **States:** Save and Run communicate through header controls and a status label; invalid name, empty steps, duplicate names, save errors, and successful saves are written as explicit status messages.

### Buttons

- **Primary:** GTK Save uses the native `suggested-action` style; Shell Create uses a translucent green authored background.
- **Run:** GTK Run is a native pill button; Shell Run uses the blue-violet action treatment.
- **Destructive:** GTK Delete uses a trash icon and opens an Adwaita destructive alert dialog; Shell Delete uses red and requires a second “Delete?” activation.
- **Secondary:** New workflow is an icon-only GTK header action with a tooltip; Add custom step is flat; move/remove step controls are icon buttons with tooltips.

### Cards and containers

- **GTK:** Navigation pages, toolbar, form, list, and scrollers are standard libadwaita/GTK containers with no custom shadow layer.
- **Shell response card:** 330px dark card, 12px radius, 12px padding, 8px spacing, wrapped body text, and contextual approval/listening actions.

### Inputs and fields

- **GTK:** Native Entry, TextView, SearchEntry, and DropDown controls. Voice triggers are one per line; steps are one row per action.
- **Shell:** Compact St.Entry fields use clear placeholders for workflow name, comma-separated triggers, and semicolon-separated steps.
- **Feedback:** Validation copy is visible text, not color alone; focus is returned to the invalid name field when appropriate.

### Navigation and indicator

- **GTK navigation:** `NavigationSplitView` with “Workflows” and “Edit workflow” navigation pages.
- **Shell navigation:** The panel menu exposes Model, Workflows (with count), Refresh models, credential entries, and Stop Hyusk.
- **Indicator:** A 24×24 custom butterfly animates subtly and changes through Hidden, Waking, Listening, Thinking, Working, and Speaking colors. Its accessible name reports the current state (for example, “Hyusk: listening”).

## Do's and Don'ts

- **Do** preserve native GTK4/libadwaita controls and Adwaita theme behavior in the workflow application.
- **Do** keep the library searchable and the selected workflow visibly tied to its editor.
- **Do** represent ordered steps as separate, editable rows with explicit reorder and remove controls.
- **Do** make status, validation, approval, cancellation, and destructive confirmation readable as text.
- **Do** use the blue-violet accent for Hyusk actions and reserve green, orange, and red for their established meanings.
- **Do** retain keyboard focusability and meaningful tooltips/accessibility labels for icon-only controls.
- **Don't** add gradients, decorative illustrations, custom shadows, or a web-style card system to the native editor.
- **Don't** make color the only indication of state or success.
- **Don't** let the UI execute actions independently of the Hyusk service.
- **Don't** collapse triggers and ordered actions into an opaque single text blob in the native editor.
