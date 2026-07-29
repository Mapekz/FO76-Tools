---
name: FO76 ESM Viewer
description: A dark, monospace-first workbench for taking Fallout 76 game records apart, byte by byte.
colors:
  trace-blue: "#7ec8e3"
  signature-blue: "#82aaff"
  complete-green: "#c3e88d"
  gap-amber: "#e8a838"
  fault-red: "#e88"
  workbench-black: "#1a1a2e"
  panel-steel: "#16213e"
  row-slate: "#1e1e2e"
  hover-graphite: "#2a2a3a"
  focus-indigo: "#33395a"
  bench-light: "#e0e0e0"
  dim-readout: "#aaa"
  faint-readout: "#888"
  ghost-text: "#666"
  seam: "#444"
  rule: "#333"
  hairline: "#222"
typography:
  data:
    fontFamily: "monospace"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: "normal"
    letterSpacing: "normal"
  label:
    fontFamily: "monospace"
    fontSize: "11px"
    fontWeight: 400
    lineHeight: "normal"
    letterSpacing: "normal"
  chrome:
    fontFamily: "sans-serif"
    fontWeight: 400
rounded:
  sm: "3px"
spacing:
  xs: "2px"
  sm: "4px"
  md: "6px"
  lg: "8px"
components:
  button-primary:
    backgroundColor: "{colors.panel-steel}"
    textColor: "{colors.bench-light}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "3px 8px"
  button-primary-active:
    backgroundColor: "{colors.focus-indigo}"
  input-text:
    backgroundColor: "{colors.panel-steel}"
    textColor: "{colors.bench-light}"
    typography: "{typography.data}"
    rounded: "{rounded.sm}"
    padding: "4px 6px"
  input-text-disabled:
    backgroundColor: "{colors.hairline}"
---

# Design System: FO76 ESM Viewer

## Overview

**Creative North Star: "The Hex Workbench"**

FO76 ESM Viewer looks like a technician's bench for taking game data apart byte by byte, not a consumer app for browsing it. Every surface is dark, every value is set in monospace, and depth exists only as a tone-scale, never as a shadow — the same visual grammar as a debugger or hex editor, applied to Fallout 76 record data. Nothing here is decorative: color exists to carry meaning (a reference to follow, a value that's missing, a record that isn't fully decoded), and everything else recedes into a narrow band of near-black blues and grays so that meaning stands out immediately in a dense, data-heavy view.

This is a strictly utilitarian aesthetic and stays that way: it must never soften into a rounded, playful, or colorful consumer-app look, no matter what feature gets added. The tool is read constantly, for long stretches, by someone cross-referencing hundreds of records — legibility and density win over warmth every time.

**Key Characteristics:**
- Near-black navy canvas with a five-step tonal scale doing all the work shadows would normally do
- Monospace is the actual body font of the app; sans-serif appears exactly once, on the outer shell
- A three-color semaphore (green / amber / red) is the only place expressive color appears
- Zero border-radius above 3px, zero box-shadows, zero icon library — plain Unicode glyphs stand in for icons

## Colors

A near-monochrome navy scale carries the interface; a small, disciplined set of accent colors carries meaning.

### Primary
- **Trace Blue** (#7ec8e3): the color of a FormID cross-reference link — underlined, monospace, clickable — in every panel that shows a record. This is the app's signature interaction (jump to what this references), so it's the one color guaranteed to appear on nearly every screen.

### Secondary
- **Signature Blue** (#82aaff): record-type signatures and other bold structural labels (e.g. a subrecord's 4-letter type code). Distinguishes "this labels the record's structure" from "this is a value" or "this is a reference."

### Tertiary — the coverage/status semaphore
- **Complete Green** (#c3e88d): a record type has full schema coverage (no undecoded gaps); also marks plain scalar values in the record table.
- **Gap Amber** (#e8a838): a record type has undecoded/unmapped fields — the coverage warning color, used identically in the Coverage panel and inline next to any record that has gaps.
- **Fault Red** (#e88): error text everywhere in the app. At 10% opacity (`rgba(238,136,136,0.10)`) it also washes an entire table row to mean "this value is absent in this file" during a cross-file diff — deliberately a translucent tint, not a solid fill, so "missing" reads differently from "present but different" at a glance.

### Neutral
- **Workbench Black** (#1a1a2e): the outermost app shell background — the darkest surface in the app.
- **Panel Steel** (#16213e): the default surface for panels, sticky table headers, and every text input/select.
- **Row Slate** (#1e1e2e): the resting background of tree and list rows.
- **Hover Graphite** (#2a2a3a): a row's background when it's hovered or is the active open file.
- **Focus Indigo** (#33395a): a row's background when it's keyboard-focused/selected, and a view tab's background when it's the active tab.
- **Bench Light** (#e0e0e0): primary text — record values, labels, body copy.
- **Dim Readout** (#aaa): secondary/metadata text — type tags, editor IDs, row summaries.
- **Faint Readout** (#888): the rare fine-print tier (e.g. a depth caption).
- **Ghost Text** (#666): empty-state and disabled-value text ("Select a record to view details," a null value's em dash).
- **Seam** (#444): the major divider — between the sidebar and detail pane, between a panel and the content below it.
- **Rule** (#333): the divider between rows in a list or tree.
- **Hairline** (#222): the finest divider (table cell borders, nested-row dividers) and, doing double duty, the background of a disabled input.

### Named Rules
**The Missing-Tint Rule.** A value absent from one file in a comparison gets a 10%-opacity Fault Red wash over its row, never a solid color swap. Solid color means "different"; translucent red means "not here at all."

**The One Accent Rule.** Trace Blue marks references, Signature Blue marks structure, and the green/amber/red trio marks coverage/error state. No other color is introduced for emphasis — if something new needs to stand out, it borrows from this set rather than adding a color.

## Typography

**Data Font:** monospace (browser default monospace stack)
**Chrome Font:** sans-serif (browser default sans-serif stack)

**Character:** Monospace is the real body font of this app — FormIDs, record signatures, hex dumps, and nearly every field value render in it, because alignment and byte-legibility matter more than warmth. Sans-serif appears exactly once, on the outermost app shell `<div>`, and nowhere else.

### Hierarchy
- **Data** (400, 12px, normal line-height): the primary reading size — tree rows, list items, table cell values. This is what gets read the most.
- **Label** (400, 11px, normal line-height): controls, tab labels, inline badges (coverage counts, diff annotations) — one step down from Data.
- **Micro** (400, 10px): the rare caption tier — a depth annotation, a tiny close (✕) button. Use sparingly; it's the floor of the scale.
- **Chrome** (400, browser default ~16px, sans-serif): reserved for the app shell only. Do not extend it to any panel content.

### Named Rules
**The Monospace-For-Data Rule.** Anything that is a value, an ID, a signature, or raw bytes renders in monospace. Sans-serif is chrome, not content.

## Layout

A fixed two-pane desktop shell, not a responsive page: a 320px-wide left sidebar (Open Files, then a tab strip switching between Tree / Search / Filter / Coverage / Diff) and a flexible right pane (Nav History bar, then Record Detail, then Referenced-By) filling the remaining width at `height: 100vh`. There is no breakpoint or mobile behavior — this is an Electron desktop window, and the layout assumes a resizable but always-desktop-sized viewport.

Spacing is dense throughout: rows and controls pad `2–4px` vertically and `6–8px` horizontally (see `spacing.xs`–`spacing.lg`); nothing in the app uses generous whitespace. The density is deliberate — this is a tool for scanning many small facts per screen, not a leisurely read.

## Elevation & Depth

Flat. There is not a single `box-shadow` anywhere in the codebase. Depth and state (default vs. hover vs. focused/selected) are conveyed entirely by moving one step along the five-stop background scale (Row Slate → Hover Graphite → Focus Indigo) plus a 2px solid Trace Blue left-border on the focused/selected row. Nothing lifts, glows, or casts a shadow.

### Named Rules
**The No-Shadow Rule.** Depth is a background-tone step, never a shadow. If a new state needs to read as "elevated" or "active," move it one step up the existing five-stop scale before reaching for anything else.

## Shapes

Nearly square. The one radius value in the entire app is **3px** (`rounded.sm`), applied to the few things that round at all: styled buttons and text inputs. Separation between regions is done with 1px solid borders in a three-step scale (Seam #444 for major dividers, Rule #333 for row dividers, Hairline #222 for cell-level dividers) rather than spacing or shadow alone. There is no icon library: expand/collapse uses plain ▶/▼ glyphs, close uses ✕, and back/forward use ← →. This is a first-pass choice noted in the code itself, not a permanent design decision — see Do's and Don'ts.

## Components

### Buttons
- **Shape:** 3px radius (`rounded.sm`), 1px solid Seam (#444) border.
- **Standard (`button-primary`):** Panel Steel (#16213e) background, Bench Light (#e0e0e0) text, Label typography (11px monospace), `3px 8px` padding. This is the canonical button — e.g. the Tree/Search/Filter/Coverage/Diff view-tab strip.
- **Active (`button-primary-active`):** background steps up to Focus Indigo (#33395a); everything else unchanged.
- **Known gap:** "Open ESM…" / "Open Folder…" (Open Files panel) and Back/Forward (Nav History) currently render as unstyled native `<button>` elements. This is not a second intentional variant — every button should converge on `button-primary` above.

### Inputs / Fields
- **Style (`input-text`):** Panel Steel (#16213e) background, Bench Light (#e0e0e0) text, Data typography (monospace), 1px solid Seam border, 3px radius, `4px 6px` padding. Used for every search/filter/diff text field.
- **Disabled:** background drops to Hairline (#222), signaling "not editable right now" (e.g. a filter's value field when the operator is `exists`).
- **Known gap:** native `<select>` elements (record-type pickers, operator dropdowns) are currently unstyled and should converge on the same treatment as `input-text` rather than staying a separate native look.

### Rows (Tree / List)
- **Default:** Row Slate (#1e1e2e) background, 1px solid Rule (#333) bottom divider.
- **Hover / active file:** Hover Graphite (#2a2a3a).
- **Focused / selected:** Focus Indigo (#33395a) background plus a 2px solid Trace Blue (#7ec8e3) left border. This left-border-on-focus is the app's one consistent "you are here" signal across Tree, Search, Filter, and Diff results.

### Table (Record Table)
- **Header:** sticky, Panel Steel (#16213e) background, sits above scrolling content.
- **Cell dividers:** 1px solid Hairline (#222).
- **Values:** Complete Green (#c3e88d) for plain scalars, Dim Readout (#aaa) for structured/nested values, Signature Blue (#82aaff, bold) for the record's own type label.
- **Missing-in-file wash:** see the Missing-Tint Rule in Colors.

### Links (FormID cross-references)
- Trace Blue (#7ec8e3), underlined, pointer cursor, monospace. Appears anywhere a record links to another record — this is the primary way a user moves through the app, so its treatment is never varied.

## Do's and Don'ts

### Do:
- **Do** set any record value, ID, signature, or raw byte content in monospace (`typography.data` or `.label`); reserve sans-serif for app chrome only.
- **Do** build any new status indicator from the existing three-color semaphore (Complete Green / Gap Amber / Fault Red) instead of introducing a fourth meaning-color.
- **Do** express any new hover/active/focus state as a step along the existing five-stop background scale (Row Slate → Hover Graphite → Focus Indigo), plus the 2px Trace Blue left-border for focus/selection specifically.
- **Do** cap border-radius at 3px (`rounded.sm`) on any new rounded control — nothing in this app rounds more.
- **Do** converge new buttons, selects, and text inputs on the styled treatment above (`button-primary` / `input-text`), not the native browser default.

### Don't:
- **Don't** add a `box-shadow` anywhere; this system conveys depth with background tone only (the No-Shadow Rule).
- **Don't** pull in an icon font or SVG icon library for a one-off glyph; the app currently uses plain Unicode characters (▶ ▼ ✕ ← →) everywhere a symbol is needed.
- **Don't** treat the unstyled native `<button>`/`<select>` instances (Open Files panel, Nav History, all `<select>` dropdowns) as a second intentional style — they're a known first-pass gap, not a pattern to copy into new UI.
- **Don't** soften this into a rounded, playful, or brightly colorful consumer-app look, even for a feature aimed at a broader audience — the utilitarian workbench identity is a permanent constraint, not a placeholder.
