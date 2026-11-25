# dendriForge Shell & Apps — UI/UX Architecture

> **Scope:** the desktop interface of dendriForge (Phase 7 onward): shell, window manager, apps, visual language, keyboard and accessibility, and the planned Files app.
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119. All design decisions (`U1–U11`, `F1–F5`, and Oscilloscope placement) are ratified; only `U6a` (final default window-manager chord table) closes mechanically in Slice 7b via the automated conflict test.
> **Related:** [`docs/architecture/00-overview-and-adrs.md`](../architecture/00-overview-and-adrs.md) (ADR-021, ADR-022), [`docs/architecture/05-ide-and-roadmap.md`](../architecture/05-ide-and-roadmap.md), `crates/dendri-theme`.

---

## 1. Vision and Principles

dendriForge presents itself as a **workstation shell**. Each module is an **app** with its own window: `Simulation`, `CodeStudio`, `Console`, `LadderPLC`, `GRAFCET`, and `Files`. Apps are modules of one process: they share one project model (`dendri-project`), one asset registry, and one simulation worker. The shell only arranges windows.

1. **One truth, many windows.** Every app is a view over the same ASL project. Closing a window never loses project state; switching layout mode never loses app state.
2. **Two layout modes.** *Floating* (overlapping windows with chrome and a taskbar) and *Tiling (dwindle)* (automatic binary-tree layout, keyboard-driven). The mode is chosen per workspace and can be switched at any time.
3. **Polish by construction.** No placeholder icons, no dead dock entries, no stub screens. An app appears in the dock when it exists; planned apps live in the deferral ledger (§7.9).
4. **Accessible by default.** WCAG AAA text contrast from `dendri-theme`; every action reachable from the keyboard; every interactive element has an accessible name (§6).
5. **Quiet interface.** Opaque surfaces, minimal motion, compact density.

---

## 2. Stack and Constraints

- Native `eframe`/`egui` with AccessKit (enabled by default on native platforms), `rfd` for native dialogs, and `egui_plot` for probes (ADR-021). No WebView.
- **`dendri-wm`** (new, pure logic, zero `egui` dependency) owns window geometry for both layout modes (§3.2). **`egui_dock` is used only for tab groups inside an app window** (for example `CodeStudio` tabs). It docks tabs and can detach them into windows, but it does not implement automatic dwindle tiling, so it does not drive the outer shell.
- **Single `usvg` pipeline.** All vector artwork — board/component SVGs and UI icons under `assets/icons/` — MUST be rasterized through the workspace's existing `dendri-assets` (`usvg` / `resvg`) pipeline into cached `egui` textures. `egui_extras`'s `svg` feature MUST remain disabled so the single-`usvg` CI gate (Gate 1) never drifts.
- **No color literals in UI crates.** `dendri-theme` is the single source. One adapter module in `dendri-editor` converts `Color` to `egui::Color32`. CI MUST fail on `Color32::from_rgb` or hex literals outside that adapter.
- **Zero `env!("CARGO_MANIFEST_DIR")` in binaries.** Fonts, icons, and templates ALL resolve through the runtime asset root (`--assets-dir` → `DENDRIFORGE_ASSETS` → `<exe_dir>/assets` → CWD fallback).
- UI code obeys the workspace lints: no `unwrap`, `expect`, indexing, `panic`, or wildcard enum arms.
- The simulation runs on its own thread and publishes `Arc<SimSnapshot>`. The UI repaints only when a snapshot or an input event changes something.
- Neutral naming in the product: the layout modes are called **Floating** and **Tiling (dwindle)**.

---

## 3. The Shell

### 3.1 Regions

- **Top bar:** application menu (`File`, `Edit`, `View`, `Hardware`, `Build`, `Help`); project and board name; **run controls** (`Play`, `Pause`, `Step`, `Reset`); virtual time; layout-mode toggle; workspace selector; RAM/Flash readouts only when the board profile provides them.
- **Dock / taskbar:** one button per existing app plus one per open window, with a state marker. No entries for unimplemented apps.
- **Status bar** (follows the focused app): `Simulation` shows coordinates, zoom, solver state, and virtual time; `CodeStudio` shows cursor, language, and diagnostics count. The focused board's `ExecutionKind` (cooperative or PLC scan) is always shown, never assumed.
- **Workspaces:** numbered; each has its own layout mode and window set.

The main OS window uses native decoration. Only inner windows draw their own chrome.

### 3.2 Window manager (`dendri-wm`)

Pure, deterministic, and headless-testable, in integer logical pixels.

**Model.** `Workspace { mode: Floating | Dwindle, windows, focus }`, `WindowId`, `Rect`. Floating keeps a z-ordered list of rectangles. Dwindle keeps a binary tree of splits whose leaves are windows.

**Dwindle rules.** A new window splits the focused leaf along its longer side; the ratio is adjustable. Splits are not permanent: closing a window collapses its sibling. `gaps_in` and `gaps_out` are configurable.

**Operations, all reachable by keyboard:** open and close; focus next, previous, or by direction; move; resize; toggle a window between tiled and floating; maximize; move to workspace; switch workspace; toggle the workspace mode.

**Invariants, enforced by property tests.**
- **I1:** tiled leaves never overlap and respect the gaps.
- **I2:** tiled leaves cover the workspace rectangle minus the outer gaps (area is conserved).
- **I3:** no window shrinks below its declared minimum; an operation that would do so is refused with a typed error.
- **I4:** toggling Floating ⇄ Dwindle preserves the window set, the focus, and each app's state.
- **I5:** the same operation sequence always yields the same layout.
- **I6:** closing a window never changes unrelated subtrees.

**Persistence.** Layout, keymap, and startup behavior live in user preferences (`workspace.toml` in the platform config directory), never in the project `.toml`. The file is versioned, rejects unknown fields (`#[serde(deny_unknown_fields)]`), and is written atomically.

### 3.3 Startup and identity (`U11`)

- The window title is `dendriForge — <project> — <board>`.
- User preferences (`workspace.toml`) expose a configurable `startup_behavior` option with three modes:
  1. `clean_shell`: opens the workstation shell with all app windows closed.
  2. `blank_project`: opens a blank untitled project ready to edit.
  3. `restore_session`: reopens the last project and window state as the user left it.
- **During Phase 7 development** (prior to full onboarding and `Files` in Phase 8) — and whenever `restore_session` finds no prior saved project on first run — dendriForge loads the hash-locked Uno + button + LED starter template (`assets/templates/uno-button-led.toml`) **in memory (unsaved)** ready to run with one click. Nothing is written to disk automatically until the user explicitly invokes `File → Save` or `Save As`. Once the user selects a `startup_behavior` preference, subsequent launches follow that setting.

---

## 4. Apps

| App | Phase | Content |
|---|---|---|
| **Simulation** | 7 | World canvas for the project, component palette, wiring, live actuators, probes, and dockable Oscilloscope / Logic Analyzer panel |
| **CodeStudio** | 7 | Tabs for **C++** (Arduino Uno), **IEC 61131-3 ST** (`plc-1200-open-1214` and any ST-capable board), and **ASL**, plus derived read-only projections (e.g., Ladder / GRAFCET text projections before Phase 8 visual editors); Board I/O Tag Table; panels for Comments, Assumptions, `Foreign`, and Packs; diagnostics |
| **Console** | 7 | Serial monitor and diagnostics log |
| **LadderPLC** | 8 | Structural Ladder editor over `PouBody::Network` |
| **GRAFCET** | 8 | Structural GRAFCET editor over `PouBody::Chart` |
| **Files** | 8 (`DEF-010`) | In-app project manager (§7) |

The oscilloscope and logic analyzer live as a dock panel inside the `Simulation` app until a dedicated Scope app is justified.

**ASL tab.** The canonical view of the project program: a strict TOML projection with a symbol outline and live diagnostics. Edits re-parse through `dendri-project`. An invalid edit never overwrites valid state; dirty buffers are preserved as in `dendri-sync`.

**Language scope.** Wave 1 languages (`C++`, `IEC 61131-3 ST`, `ASL`, plus derived `Ladder`/`GRAFCET` projections) are available in Phase 7 according to the focused board and POU. MicroPython belongs to Wave 2 (`DEF-004`) and has no tab until then.

**Shell scope.** Top bar, dock/taskbar, window manager, and workspaces. Desktop widgets, an application launcher with search, tray extras, audio or network indicators, and hardware flashing are out of scope; each needs its own ledger entry before it appears.

---

## 5. Visual Language

### 5.1 Tokens and typography (`U10`)

- All colors are `dendri-theme` tokens: backgrounds, text, accents, status, syntax, and the 12 `df-pin-*` classes. Faults and interrupts use `DF_STATUS_FAULT` / `DF_PIN_INTERRUPT`. `DF_ACCENT_ROSE` MUST NOT carry text or fault meaning.
- **Fonts (`U10`).** **Noto Sans** (proportional, upright weights for UI chrome) and **Noto Sans Mono** (monospace for code, tags, and numeric readouts), both licensed under **SIL OFL 1.1** (commercial-use free, consistent multi-script family). Fonts resolve at **runtime** from `<assets_dir>/fonts/` so users can customize or substitute font files easily without recompiling; if a font file is absent at runtime, `dendri-editor` falls back cleanly to `egui`'s built-in font and emits a diagnostic notice. Each bundled font is third-party: it ships a `[provenance]` manifest (`source = "third-party"`, `ml_restricted = false`), is registered in `THIRD_PARTY_NOTICES`, and passes `dendri provenance-check` (extended in Slice 7b).
- **Non-Latin comments.** Latin, Cyrillic, and Greek glyph rendering and fallback behavior MUST be verified against the pinned `egui` version in Slice 7b. Complex text shaping (such as RTL Arabic/Hebrew ligatures or Indic scripts) not performed natively by `egui`/`epaint` MUST be recorded as a documented limitation (and tracked in the ledger if targeted later).
- **Icons (`U10`).** An original vector SVG icon set authored under `assets/icons/` (`source = "original"`, sanitized SVG, `review.toml` render-hash, validated by `dendri provenance-check` / `asset-check` via a lightweight UI-icon schema that does not require electrical component `profile.toml`), rasterized once per scale through the single `usvg`/`resvg` pipeline.

### 5.2 Density and grid

- Density is compact, in the style of EDA tools. Interactive targets are at least 24×24 logical pixels (WCAG 2.2 AA); compactness never goes below that.
- The `Simulation` canvas shows **no grid at low zoom**, the **48 px module grid** from a documented zoom threshold, and the **24 px sub-grid only at high zoom**. The thresholds are constants with tests. Snapping follows `03-asset-and-profile-contract.md`: terminals on module intersections, origins on the half-module.

### 5.3 Wires

Wires are always orthogonal. The batched router routes all wires together so parallel runs take distinct tracks, with junction dots and jump arcs. Curved wires and signal-particle animation are never used. A wire's base color comes from the pin class of its net. Logic state is conveyed by line style and by token, never by color alone.

### 5.4 Focus and chrome

The focused window has a solid 2 px `DF_ACCENT_SKY` border (its contrast is verified in `dendri-theme` tests). Inner windows draw a title bar with minimize, maximize, and close. In tiling mode the title bar shrinks to a compact header.

### 5.5 Effects and motion

Surfaces are opaque: no blur, no translucency. Motion is limited to changes that carry information, such as an LED lighting, and a `reduce_motion` preference disables it. The UI repaints only on snapshot or input change.

---

## 6. Keyboard and Accessibility

- **Keymap.** Every action has a configurable chord. Defaults use `Ctrl` (`Cmd` on macOS), combining `Alt` and/or `Shift` (`Ctrl+Alt`, `Ctrl+Shift`, `Alt+Shift`, `Ctrl+Alt+Shift`) where needed to avoid collisions, aligned with popular tiling-WM and IDE conventions per action. The `Super` key is not used by default because the host OS often captures it.
- **Scopes.** Chords are scoped to the global shell, the focused app, or the text editor. A text editor never loses its standard editing chords to a shell chord.
- **Conflict test (`U6a`).** A unit test loads the default keymap and fails on duplicate chords within a scope and on chords in a maintained table of common host-OS shortcuts. The default window-manager chord set is finalized in Slice 7b by running this test (`U6a`).
- **AccessKit.** Every interactive widget has an accessible name and role. The canvas is custom-painted, so the `Simulation` app provides a keyboard-navigable **scene outline** (parts, terminals, wires, selection) as its accessible representation.
- **Focus order** is defined per app and covered by tests. Window focus is reachable from the keyboard in both layout modes. UI tests use an AccessKit-based harness (`egui_kittest`, evaluated in Slice 7b).

---

## 7. Files App (`dendri-files`) — Planning

Users manage projects without leaving dendriForge. The app is planned, not yet implemented; §7.9 is its ledger entry.

### 7.1 Purpose and non-goals

Purpose: browse, create, open, organize, and import/export dendriForge projects. Non-goals: a general-purpose file manager, version control, cloud sync, hardware flashing.

### 7.2 Concepts

- **Workspace root:** a directory configured in user preferences (default `dendriForge/` under the user's home), scanned lazily.
- **Project:** a single `.toml` file (`01-asl-ir-and-project-schema.md`). It may reference `.dendrilib` libraries (name, version, SHA-256 lock). The app treats a project and its referenced libraries as one **bundle** for move, rename, and duplicate.
- **Template:** a versioned, hash-locked starter project stored in the single canonical directory `assets/templates/` under the runtime asset root (zero duplicated template directories), validated by CI and paired with an independent oracle scenario. `assets/templates/uno-button-led.toml` is introduced in Slice 7b/7c (used by first-run startup in Phase 7) and directly reused by `dendri-files` in Phase 8 alongside the multi-board Uno + open PLC example template.

### 7.3 Requirements

- **FR-1 Open:** validates with the strict loader and shows diagnostics without crashing.
- **FR-2 New from template, duplicate, rename, move:** bundle-aware; references stay valid or the operation is refused with an explicit message.
- **FR-3 Delete:** moves to a recoverable trash with restore (`F5`).
- **FR-4 Recents and pinned:** stored in user preferences.
- **FR-5 Badges:** per project (validation state, active assumptions, `Foreign` nodes), computed off the UI thread.
- **FR-6 External-change detection:** if a file changes on disk while open, a dirty buffer is never overwritten; the user chooses reload, keep mine, or compare.
- **FR-7 Export and import:** export emitted source (C++, ST) through the existing translation path; import a single source file into a new project (Phase 8 or later, same ledger entry).
- **FR-8 Keyboard-complete** tree with accessible names.

### 7.4 Safety

- All paths are canonicalized. Operations are confined to the configured roots unless the user picks a location in the native dialog.
- Symlinks are not followed outside a root. Windows reserved names and case-insensitive collisions are handled.
- Writes reuse the atomic `write temp → sync → rename` path of `dendri-project`, so a crash never leaves a half-written project.
- No `unwrap` or indexing; a `cargo-fuzz` target covers path handling.

### 7.5 Crate split

`dendri-files` (new, headless): workspace index, templates, bundle model, file operations, trash, recents, and path safety. The UI lives in `dendri-editor` (`apps::files`). Dependencies: `dendri-project`, `dendri-validate`.

### 7.6 Until the app exists

Phase 7 ships working `File → Open`, `Save`, and `Save As` through `rfd` and the atomic writer. There is no `Files` icon, crate skeleton, or placeholder window.

### 7.7 Decisions

| ID | Decision | Ratified Choice |
|---|---|---|
| F1 | Browsable area | Configured workspace roots; anything else through the native dialog (`rfd`) |
| F2 | Project layout | Single `.toml` (`01-asl-ir-and-project-schema.md`) with referenced `.dendrilib` shown as a bundle |
| F3 | Template location | Runtime asset root (`assets/templates/`), hash-locked like packs; never compiled-in paths |
| F4 | Backups | Keep the last 5 saves per project in the platform user-data directory, not beside the project |
| F5 | Trash | Portable `.dendri-trash/` inside the workspace root, with restore (testable on every OS) |

### 7.8 Definition of Done

All FR items implemented; property tests for path safety and bundle moves; fuzz target; external-change scenarios; accessible names; no open ledger entry for the app.

### 7.9 Ledger entry (`docs/deferrals.toml`)

```toml
[[deferral]]
id                 = "DEF-010"
title              = "Files app (dendri-files): in-app project manager"
owner_milestone    = "phase-7"
deadline_milestone = "phase-8"
reason             = "Not in the original roadmap; design closes in Phase 7 (this section), implementation in Phase 8. Phase 7 ships working File > Open/Save/Save As via rfd instead of a placeholder."
refs               = ["docs/ui/DESIGN.md#7-files-app-dendri-files--planning"]
```

---

## 8. Deliverables and Phase Slices

### 8.1 Repository additions

```text
crates/
  dendri-wm/       # NEW (7a): floating z-order, dwindle tree, workspaces (zero egui)
  dendri-canvas/   # NEW (7c): scene, camera, transform M, orthogonal router, hit-testing
  dendri-editor/   # NEW (7b): eframe app: shell, apps, keymap, single theme adapter
  dendri-files/    # PLANNED (Phase 8 / DEF-010): created only when its slice starts
  dendri-theme/    # existing: tokens; the egui adapter lives in dendri-editor
assets/
  fonts/           # NEW (7b): runtime Noto Sans & Noto Sans Mono + [provenance] + THIRD_PARTY_NOTICES
  icons/           # NEW (7b): original vector SVG icon set (sanitized, provenance, review.toml)
  templates/       # NEW (7b/7c): single canonical hash-locked starter projects (reused in Phase 8)
docs/ui/
  DESIGN.md        # this document
```

### 8.2 Slices

| Slice | Content | Gate |
|---|---|---|
| **7a** | `dendri-wm` complete with invariants `I1–I6` | Property tests (`I1–I6`); fuzz target for operation sequences |
| **7b** | Theme adapter; shell (top bar, dock, status bar); runtime Noto fonts & original SVG icons + `provenance-check` extension; `assets/templates/uno-button-led.toml`; keymap with conflict test (`U6a`); `workspace.toml` prefs (incl. `startup_behavior`); `File → Open/Save/Save As` | Zero color literals outside adapter; accessible names; keymap conflict test (`U6a`); Latin/Cyrillic/Greek check; provenance & single-`usvg` gates green |
| **7c** | `Simulation` app: camera, grid thresholds, parts from asset registry, batched orthogonal wires, live snapshot, Oscilloscope/Logic Analyzer dock panel, scene outline | Tracer-bullet scenario with independent oracle, rendered headless |
| **7d** | `CodeStudio`: **C++**, **IEC 61131-3 ST**, and **ASL** tabs (plus read-only derived projections), Board I/O Tag Table, Comments/Assumptions/`Foreign`/Packs panels, diagnostics | Round-trip and comment-conservation suites reused through the UI |
| **7e** | `Console` | Serial and diagnostic traces match the headless runner |
| **8** | `LadderPLC`, `GRAFCET`, `Files` (`DEF-010`) | Per-app Definition of Done; `DEF-010` closed |

---

## 9. Amendments and Decisions

- **ADR-021 (stack):** Add `dendri-wm` as owner of shell window geometry; restrict `egui_dock` to tab groups inside apps; keep single-`usvg` rasterization for icons (`egui_extras/svg` disabled); load runtime Noto Sans / Noto Sans Mono from `<assets_dir>/fonts/`.
- **ADR-022 (screen contracts):** Replace the fixed docking workspace with the workstation shell/app model of §3–§4; add the `ASL` tab alongside `C++` and `IEC ST` in `CodeStudio`, keep Oscilloscope as a `Simulation` dock panel, and schedule `Files` (`DEF-010`) in Phase 8.
- **Roadmap (`docs/architecture/05-ide-and-roadmap.md`):** Split Phase 7 into Slices 7a–7e (§8.2); place `Files` alongside `LadderPLC` and `GRAFCET` in Phase 8; add `DEF-010` to `docs/deferrals.toml`.

| ID | Decision | Status |
|---|---|---|
| U1 | Floating by default; Tiling (dwindle) per workspace | Decided |
| U2 | Native decoration for the main window; own chrome for inner windows | Decided |
| U3 | Compact EDA density, targets ≥ 24×24 px | Decided |
| U4 | 48 px grid, 24 px sub-grid at high zoom only, none at low zoom | Decided |
| U5 | Opaque surfaces, minimal motion | Decided |
| U6 | Configurable chords, `Ctrl`/`Cmd` default (+ `Alt`/`Shift` combos; no `Super`) | Decided |
| U6a | Default window-manager chord set (researched popular conventions) | Closes in Slice 7b by conflict test |
| U7 | ASL as a tab of `CodeStudio` alongside C++ and IEC ST | Decided |
| U8 | Layout, keymap, and startup behavior stored in user preferences (`workspace.toml`) | Decided |
| U9 | Minimal shell; no widgets, launcher search, tray extras, or flashing | Decided |
| U10 | Runtime Noto Sans + Noto Sans Mono (SIL OFL 1.1) in `assets/fonts/`; original SVG icons in `assets/icons/` | Decided |
| U11 | Configurable `startup_behavior` (`clean_shell` / `blank_project` / `restore_session`); in-memory Uno + button + LED starter during Phase 7 dev / first run | Decided |
| F1–F5 | `Files` app decisions (§7.7) | Decided |
| Scope | Oscilloscope / Logic Analyzer as a dock panel inside `Simulation` | Decided |
