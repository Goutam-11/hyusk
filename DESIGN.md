---
name: Hyusk Control Deck
description: Native GNOME workflow studio and compact Shell controls for Hyusk.
colors:
  graphite-rail: "#242a30"
  graphite-inset: "#343d46"
  graphite-display: "#20272d"
  graphite-screen: "#151c21"
  text-primary: "#f2f4f5"
  text-secondary: "#bcc7d2"
  transport-blue: "#215bc9"
  transport-blue-hover: "#194aa5"
  module-open: "#dbe6ff"
  module-timer: "#ffe0b9"
  module-media: "#c0ebd7"
  module-system: "#e1d9ed"
  status-hidden: "#9aa0a6"
  status-waking: "#82d2ff"
  status-listening: "#78e6aa"
  status-thinking: "#b4a0ff"
  status-working: "#ffaa6e"
  status-speaking: "#ff82c8"
typography:
  body:
    fontFamily: "system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.35
  title:
    fontFamily: "system-ui, sans-serif"
    fontSize: "26px"
    fontWeight: 800
    lineHeight: 1.2
  module-number:
    fontFamily: "system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 800
    lineHeight: 1.2
  module-kind:
    fontFamily: "system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 700
    lineHeight: 1.2
  extension-meta:
    fontFamily: "system-ui, sans-serif"
    fontSize: "11px"
    fontWeight: 400
    lineHeight: 1.2
rounded:
  module-number: "9px"
  step-row: "12px"
  schedule-panel: "10px"
  shell-menu: "16px"
  shell-key: "8px"
  shell-display: "14px"
  shell-screen: "8px"
spacing:
  library-padding: "22px 16px 16px"
  editor-padding: "24px"
  editor-rhythm: "18px"
  step-row-padding: "12px"
  footer-padding: "12px 22px"
  shell-display-padding: "14px"
components:
  button-run:
    backgroundColor: "{colors.transport-blue}"
    textColor: "#ffffff"
    typography: "700 14px system-ui, sans-serif"
    padding: "8px 18px"
  workflow-module:
    rounded: "{rounded.step-row}"
    padding: "{spacing.step-row-padding}"
  module-number:
    backgroundColor: "{colors.module-open}"
    textColor: "#123575"
    typography: "800 18px system-ui, sans-serif"
    rounded: "{rounded.module-number}"
    size: "30px minimum width, 36px minimum height"
  shell-key:
    backgroundColor: "#3b4752"
    textColor: "#ffffff"
    rounded: "{rounded.shell-key}"
    padding: "8px 12px"
---

# Design System: Hyusk Control Deck

## Overview

**Creative North Star: “The Graphite Control Deck”**

Hyusk's workflow studio is a compact instrument for inspecting and running real desktop routines. It pairs a dark graphite library rail with a native themed GTK editor, where each ordered action becomes a numbered module. The butterfly remains the sole brand mark; native GNOME controls and type keep the experience grounded in the desktop.

The native app's first viewport is organized around the work: a searchable routine list at left, an expansive editor at right, and a fixed feedback footer below. Blue marks transport and selection, while module number keys use category colors to help users scan a sequence without making color the only label. Scheduling stays in an expandable section so the ordered actions remain the focus. The GNOME Shell companion offers quick status and workflow controls, while the Hyusk service remains the execution authority.

**Key Characteristics:**

- Graphite sidebar and native, theme-aware GTK editor surfaces.
- Ordered steps presented as clearly numbered, category-colored modules.
- Fixed status footer and explicit save-before-run behavior for edited drafts.
- Compact GNOME Shell indicator and searchable quick-run menu.

## Colors

The studio combines authored graphite and transport colors with native GTK surfaces; Shell state colors continue to identify the agent's current state.

### Primary

- **Transport Blue**: The main run action and selected library row use a saturated blue for direct interaction.
- **Graphite Rail**: The library rail establishes the dark, instrument-like frame and carries high-contrast labels.

### Secondary

- **Open Module Blue**: Number keys for general/open actions use a pale blue fill and deep blue text.
- **Timer Amber**: Timer modules use a warm amber key.
- **Media Mint**: Media and audio modules use a mint key.
- **System Lilac**: System and memory modules use a soft lilac key.

### Tertiary

- **Success Green**: The GNOME Shell workflow dialog uses green-tinted create/save feedback.
- **Warning Orange**: Attention and the agent's Working state use orange.
- **Speaking Pink**: The Shell indicator uses pink for Speaking state.

### Neutral

- **Graphite Inset**: Search, entries, and Shell control surfaces sit on a lighter graphite inset.
- **Display Charcoal**: The Shell status display separates the card from its darker inner message screen.
- **Primary and Secondary Text**: Near-white carries essential labels; cool gray supports explanatory copy and metadata.
- **Agent State Colors**: Hidden is gray, Waking is sky blue, Listening is green, Thinking is violet, Working is orange, and Speaking is pink.

### Named Rules

**The Labeled Module Rule.** Category color supports scanning, while every step also keeps its number and readable kind label.

**The Transport Rule.** Reserve saturated blue for selection and direct run actions; don't use it as decoration.

## Typography

**Display Font:** System UI supplied by GTK/Adwaita.
**Body Font:** System UI supplied by GTK/Adwaita.
**Label/Mono Font:** No separate mono face is defined.

**Character:** Native system type keeps controls familiar and legible. The editor title and step numbers are deliberately bold so the library, sequence, and primary action scan quickly.

### Hierarchy

- **Title** (800, 26px, 1.2): Studio title and selected workflow heading.
- **Body** (400, native 14px, 1.35): Workflow fields, action descriptions, and supporting copy.
- **Module Number** (800, 18px, 1.2): Sequence position inside each colored key.
- **Module Kind** (700, 12px, 1.2): Short action category label beside each number.
- **Shell Metadata** (400, 11px, 1.2): Compact status and secondary labels in the panel companion.

## Layout

The GTK4/libadwaita window opens at 1120×760. An `AdwNavigationSplitView` gives the searchable library a 260–360px sidebar and the remaining width to the editor. The rail contains the butterfly and Hyusk name, New workflow, Search workflows, a scrolling single-selection list, and an agent-state label. The editor keeps its title and controls in the native header bar; its form scrolls independently beneath it.

The editor presents Name and Voice triggers before Steps. Each step is a full-width row with a numbered category key, kind label, editable command, and explicit move-up, move-down, and remove controls. A common-action picker and Add custom step control sit above the sequence. Scheduling is an expander after the steps. A fixed footer keeps validation and runtime feedback visible while the form scrolls. Run stays unavailable until the current editor content matches the saved workflow; save changes before running.

The Shell companion uses a small right-side panel indicator and a compact popup menu. Its workflow quick-run submenu includes search, while the workflow editor dialog uses a 760px content area. These Shell surfaces remain subordinate to the native application.

## Elevation & Depth

The GTK studio uses no authored shadow system. Depth comes from the native theme, boxed workflow rows, inset fields, the split between the rail and editor, and the fixed footer boundary. The Shell status card uses a dark outer display and a still darker inner message surface; keep these planes flat and legible instead of adding decorative shadows.

## Shapes

Use Adwaita's native silhouettes for GTK controls. The authored workflow modules have softly rounded row containers (12px) and distinct number keys (9px); the schedule expander is gently rounded (10px). The Shell companion uses rounded keys (8px), an inset display (14px), and a rounded popup menu (16px). Keep shape subordinate to the native control language.

## Components

### Library rail and search

- **Character:** A dark, dense index that frames routine selection.
- **Layout:** Hyusk butterfly and wordmark, New workflow action, native search entry, scrolling list, and a bottom agent-state label.
- **States:** Selected workflow uses the transport blue row; hover uses a restrained lighter graphite. Search communicates empty and no-match states in text.

### Workflow modules

- **Character:** A sequence of tactile, inspectable command modules.
- **Shape:** Wide theme-aware row with a 12px radius, subtle theme-derived outline, and 12px padding.
- **Content:** Number key, short category label, expandable command entry, and compact native reorder/remove buttons.
- **Color:** General/open keys are pale blue; timer amber; media/audio mint; system/memory lilac. Labels stay visible beside every key.

### Buttons

- **Run:** Saturated blue, bold label, and generous horizontal padding; disabled until the editor matches the saved workflow.
- **Save:** Native GTK suggested-action styling, retaining the active Adwaita theme.
- **Secondary:** New workflow and step editing controls use standard GTK buttons and symbolic icons with tooltips.
- **Shell controls:** Compact dark keys use white text, clear hover/focus states, and a distinct blue primary key; Stop uses a muted red-tinted surface.

### Inputs and fields

- **GTK:** Native `SearchEntry`, `Entry`, `TextView`, and `DropDown` inherit the system theme. The voice trigger editor accepts one phrase per line; each action remains independently editable.
- **Focus:** Keep native keyboard focus behavior visible; authored focus outline uses the Hyusk blue-violet accent.
- **Draft behavior:** Unsaved edits remain drafts and provide visible status. Running requires saving the selected workflow first.

### Footer and status display

- **GTK footer:** Fixed below the scrolling editor, with a theme-derived top divider and wrapped status copy.
- **Shell display:** A compact graphite status card with a darker message inset and state-aware labels. It presents current service status rather than invented progress telemetry.

### Navigation and Shell indicator

- **GTK navigation:** Native split view and single-selection library list; allow the platform's narrow-width collapse and scrolling behavior.
- **Shell navigation:** Panel menu includes workflow count, a searchable Run workflow submenu, the workflow editor, Open automation browser, model controls, phone-link controls, and Stop current task.
- **Indicator:** The pearl-white butterfly and its state-colored aura remain the sole brand mark in the Shell surface.

## Do's and Don'ts

### Do:

- **Do** preserve GTK4/libadwaita controls and their active theme behavior.
- **Do** keep the rail, numbered modules, fixed feedback footer, and expandable scheduling hierarchy.
- **Do** keep step numbers and text labels visible when using category colors.
- **Do** make draft status and the save-before-run requirement explicit.
- **Do** send execution requests to the Hyusk service; keep the editor as the authoring and control surface.
- **Do** treat dedicated browser automation as a separate CDP browser profile, isolated from the user's normal browser profile.

### Don't:

- **Don't** replace native controls with fake hardware switches or decorative console chrome.
- **Don't** let scheduling crowd out the ordered action sequence.
- **Don't** rely on color alone for module kind, agent state, validation, or execution feedback.
- **Don't** imply edits are saved or runnable when the selected workflow still has an unsaved draft.
- **Don't** present an illustrative review capture as the user's actual workflow data or as proof of a live GNOME render.
