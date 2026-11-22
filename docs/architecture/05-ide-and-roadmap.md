# 05 — Desktop IDE (`egui`/`eframe`), View Contracts & Phased Execution Roadmap

> **Normative Specification** | Decisions locked in Round 6 (ADRs 021–024)  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md) through [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md)

---

## 1. Desktop IDE Architecture (`dendri-editor`, ADR-021)

`dendri-editor` v2 is a pure-Rust engineering IDE built on **`egui` / `eframe`** (with `wgpu` / `glow` GPU backend), eliminating v1's `iced` state-duplication, overlay event leaks, `static_cache` desyncs, `usvg` dual-version conflict, and missing CAD/oscilloscope primitives.

### Core UI Stack (100% Pure Rust, Zero WebView / JS)
- **Windowing & Rendering:** `eframe` + `egui` (built-in **AccessKit** screen-reader & keyboard navigation support).
- **Docking Workspace:** `egui_dock` — genuine resizable, dockable panels (Palette, Simulation Canvas, Multi-Board Explorer, Code/Ladder/GRAFCET tabs, Live I/O & Tag Watch, Diagnostics Panel, Virtual Oscilloscope / Logic Analyzer) with **zero click/scroll leak through overlays**.
- **Plotting & Probes:** `egui_plot` — real-time multi-channel voltage/current oscilloscope & digital logic analyzer reading directly from `dendri-circuit` / `dendri-sim-kernel` probe buffers.
- **File Dialogs & Atomic I/O:** `rfd` native OS Open / Save As / Export dialogs + atomic write (`write temp -> rename`) so crashes never corrupt a `.toml` project.
- **Asset Resolution:** Runtime asset root resolved via `--assets-dir` → `DENDRIFORGE_ASSETS` env var → `<exe_dir>/assets` → CWD fallback. **Zero `env!("CARGO_MANIFEST_DIR")` in binaries.**
- **Theme (`dendri-theme`):** Single source of truth for the **A11Y Slate** palette, audited with automated contrast unit tests (fixing v1's 2.47:1 hover button and 1.79:1 Ladder inactive rail failures; minimum WCAG AA 4.5:1 text / 3:1 non-text UI, AAA 7:1 on primary text & pin tokens).
- **Single `usvg` Pipeline:** Parse each SVG **once** per loaded asset into cached base GPU textures (`TextureHandle`) + cached dynamic path geometry (`PreparedLayer`) + parsed `<g id="df-pins">` 8 px pin anchors rendered natively via `egui::Painter` colored by `dendri-theme`.
- **Decoupled Simulation Worker Thread:**
  - `dendri-sim-kernel` runs off the UI thread.
  - Publishes lightweight immutable `Arc<SimSnapshot>` frames (node voltages, branch currents, component visual states, active Ladder rails, active GRAFCET steps/timers, probe waveforms, diagnostics) and wakes UI via `ctx.request_repaint()` **only** when state advances.
  - Zero per-pixel disk I/O during drag (all profiles & SVGs pre-loaded in an in-memory `AssetRegistry`); zero deep-cloning of `SimulationProject` at 60 FPS.
  - Editing canvas/wires while paused **stays paused** (fixing v1's forced re-pause bug).
  - Unified `UndoStack` covering Canvas, Wiring, I/O Tag Table, Code, Ladder, and GRAFCET mutations.

---

## 2. IDE Screen & View Contracts (ADR-022)

Views roll out sequentially by foundation — **no fake/hardcoded placeholder diagrams allowed**:

### Wave A — Core Engineering Workspace
1. **Simulation Canvas View (`Physical 48×48 px` + `IEC/ISO Schematic` Toggle):**
   - Full pan/zoom (cursor-anchored + zoom-to-fit), 48×48 px grid & snap, multi-select, copy/cut/paste/duplicate, rotate 90° (`R`), mirror (`X`/`Y`) using the exact unified transform matrix $\mathbf{M}$ (0° drift between hit-testing and visuals).
   - **Batched Multi-Wire Orthogonal Routing:** Passes **all** scene wires simultaneously to the orthogonal channel router so parallel wires separate cleanly onto distinct tracks, with jump arcs/junction dots, click-to-drag waypoints, and `Escape` cancellation.
   - **Live Interactive Actuators:** Click/hold or latch NO/NC pushbuttons, drag potentiometer wipers live, adjust 4–20 mA pressure/temperature/sensor sliders, toggle switches, view real-time LED luminosity/color/burn, electrical fault badges (short-circuit, overvoltage, over-power), and drop **Voltage/Current/Logic Probes** wired into the Oscilloscope dock.
   - **Multi-Board Selector:** Click any board on the canvas (`plc_main`, `mcu_sensor`, ...) to focus its POUs, I/O Tag Table, Code, Ladder, or GRAFCET views seamlessly.
2. **Code Editor + Board I/O Tag Table + Live Diagnostics Dock:**
   - Real syntax highlighting (A11Y Slate tokens), line numbers, inline squiggles & hover messages backed by live `dendri-validate` spans (`ERR_*`).
   - Instant language switcher per POU (`C++` ↔ `IEC ST` ↔ `ASL`, later `MicroPython` / `Rust`) without losing dirty unsaved edits on tab switch.
   - Dedicated **Board I/O Tag Table** panel binding logical tags to physical board ports (`D13`, `A0`, `Ia.0`, `Qa.0`, `IW64`) with real-time electrical values.

### Wave B — Native Visual IEC Editors
3. **Visual Ladder Diagram Editor (IEC 61131-3 LD):**
   - Real grid editor driven directly by the board's Ladder POU AST (primary, secondary, tertiary OBs/POUs).
   - Drag-and-drop / keyboard placement of NO/NC/P/N contacts, Normal/Negated/S/R/P/N coils, parallel OR branches, Timers (`TON`/`TOF`/`TP`), Counters (`CTU`/`CTD`/`CTUD`), Triggers (`R_TRIG`/`F_TRIG`), Bistables (`SR`/`RS`), Compare/Math/Move/Comm boxes.
   - **Live Simulation Overlay:** Energized power rails and active contacts/coils illuminate in real time alongside live `.ET` / `.CV` values!
4. **Visual GRAFCET / SFC Editor (IEC 60848):**
   - Interactive node-graph editor (Draw.io ergonomics) for Steps (initial, normal, macro-steps with drill-down), Transitions (with explicit OR priority badges), AND/OR divergence & convergence bars, and Action blocks with `N/S/R/L/D/P/P0/P1` qualifiers.
   - **Live Simulation Overlay:** Active step tokens glow in real time alongside live `<Step>.T` elapsed timers and transition receptivity states!

### Wave C — Convergence & Extensibility
5. **Unified HMI / SCADA / IoT Builder:** Data-driven dashboard canvas binding validated SVG gauges, numeric displays, trend charts, alarm lists, and operator buttons directly to any board's ASL tags and virtual Modbus/telemetry bus.
6. **AI Engineering Copilot (BYOK):** Provider-agnostic Bring-Your-Own-Key assistant panel (OpenAI / Anthropic / Gemini / local Ollama) constrained by `dendriForge` architecture prompts; **every generated code/diagram edit MUST pass `dendri-validate` before being offered for one-click apply.**

---

## 3. Clean v2 Workspace Crate Architecture (Acyclic DAG)

```
Layer 0 (Foundations — zero internal deps)
├── dendri-span         Byte spans, SourceMap, rich Diagnostics, frozen ErrorCode enum
├── dendri-theme        A11Y Slate palette, semantic pin classes, WCAG contrast tests
└── dendri-stdlib       Typed IEC 61131-3 + embedded stdlib catalog & standards registry

Layer 1 (IR, Expressions & Assets)
├── dendri-ast          ASL v2 typed IR (POUs/OBs, types, statements, expressions, multi-board Project)
├── dendri-expr         Official Pratt expression parser & canonical printer (100% lossless round-trip)
├── dendri-assets       Strict TOML profile loader (deny_unknown_fields), SVG df-pins parser, Asset Validator Gate
├── dendri-validate     Full static typechecker, scope/OB/ISR/wiring validator (ALL ErrorCodes live!)
└── dendri-project      Single-file v2 .toml load/save/round-trip (atomic I/O, serde_spanned)

Layer 2 (Simulation & Multi-Domain Physics)
├── dendri-vm           Per-board cooperative ASL VM fibers, lexical frames, IEC 61131-3 FBs, OBs/ISRs
├── dendri-circuit      SPICE-class MNA transient & nonlinear solver (R/C/L/AC/Diodes/LED/BJT/MOSFET/faults)
├── dendri-bus          Physical-topology UART, I2C, SPI, and Virtual Ethernet / Modbus TCP fabric
├── dendri-physics      Pluggable PhysicalComponent models (Pneumatics/Hydraulics, Thermal, Tank, Displays)
└── dendri-sim-kernel   Hybrid discrete-event + fixed-tick u64 time_us multi-board orchestrator & snapshot channel

Layer 3 (Polyglot Pure-Rust Frontends & Sync)
├── dendri-frontend-core  Shared UTF-8 lexer, Pratt engine, I/O tag resolver, LanguageFrontend trait
├── dendri-lang-st        IEC 61131-3 Structured Text parser & emitter
├── dendri-lang-cpp       C++ (Arduino subset) parser & emitter
├── dendri-lang-ladder    IEC 61131-3 Ladder AST, materialized lowering & structural lifting
├── dendri-lang-sfc       IEC 60848 GRAFCET/SFC AST, Rule 5 two-phase lowering & lifting
├── dendri-sync           Non-destructive per-board/per-POU revisioned sync & unified undo transactions
└── [Wave 2+] dendri-lang-micropython, dendri-lang-rust, dendri-lang-{c,zig,forth,lustre,...}

Layer 4 (Canvas & Applications)
├── dendri-canvas       48×48px Scene graph, unified transform M, batched orthogonal wire routing, SVG layer cache
├── dendri-cli          `dendri` CLI (asset-check, validate, sim, bench-spice, translate, roundtrip)
└── dendri-editor       `egui`/`eframe` desktop IDE (docking, Simulation, Code, Ladder, GRAFCET, HMI, BYOK)
```

---

## 4. Quadruple CI Verification Gate (ADR-023)

Every PR and phase completion MUST pass **all 6 CI jobs with zero warnings**:
1. **Hygiene & Static Analysis:** `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo deny` / duplicate-version check (single `usvg`!).
2. **Unit + Property Tests:** `cargo test --workspace` including `proptest` canonical round-trip invariants (`parse(print(ast)) == ast`) across `dendri-expr`, `dendri-project`, and every `LanguageFrontend`.
3. **Real Negative Error Corpus:** At least **1 genuine `.toml` or source fixture per `ErrorCode` variant** asserting exact error code + accurate `Span` — **ZERO synthetic `[fixture_inject]` harnesses!**
4. **Asset Contract Gate (`dendri asset-check`):** Validates 100% of `assets/{boards,components,screens}/` against the 48×48 px grid, 8 px `df-pins` 1:1 bijection, mandatory dual-view SVGs, dynamic binding IDs, and `standards.toml`.
5. **SPICE Analytical Benchmarks (`dendri-circuit`):** Automated mathematical validation of RC/RL transients, voltage dividers, Shockley diodes/LEDs, 4–20 mA shunts, 1φ/3φ AC RMS, BJT amplifier, and MOSFET switching against closed-form analytical equations (`error < 0.1%`).
6. **Multi-Board & Cross-Language Golden Equivalence Suite:** Same reference industrial/MCU scenarios authored in C++, IEC ST, Ladder, and GRAFCET MUST produce **bit-identical simulation traces**, including real multi-board co-simulation (e.g., Arduino Uno + Siemens S7-1200 communicating electrically and over UART/Modbus).

---

## 5. Multi-Phase Master Implementation Roadmap (ADR-024)

Each phase has a **hard Definition of Done (DoD)** gate. We NEVER advance to Phase $N+1$ until Phase $N$ is 100% green in CI with zero skipped assertions or warnings.

| Phase | Name & Focus | Core Deliverables | Hard DoD Gate |
|---|---|---|---|
| **Phase 0** | **Clean-Slate Bootstrap & Repo Hygiene** | Create branch `v2`; archive v1 (`crates/`, `content/`, `core/`, stale `docs/`) into `legacy/v1/`; delete `.restore-chunks` and `docs/Empty`; clean root `Cargo.toml`, new technical `README.md`, strict CI workflow (`fmt`, `clippy -D warnings`, `test`, `asset-check`), `dendri-span`, `dendri-theme` (WCAG-tested A11Y Slate), `dendri-stdlib` (typed signatures + live `standards.toml`). | Empty workspace builds & passes strict CI; WCAG contrast tests green; zero warnings. |
| **Phase 1** | **ASL v2 IR, Pratt Expressions, Project Schema & Full Validator** | `dendri-ast` (OBs/POUs, full IEC+embedded types, spans, `NodeId`, `[[boards]]` project), `dendri-expr` (Pratt parser + canonical printer), `dendri-project` (atomic TOML I/O), `dendri-validate` (full typechecker, scope/OB/ISR/wiring checks, 100% of `ErrorCode` variants live!). | `proptest` lossless round-trip green; **1 real negative fixture per `ErrorCode`** passing with exact spans; zero dead error codes. |
| **Phase 2** | **Cooperative Multi-Board VM (`dendri-vm`) & Kernel Skeleton** | `dendri-vm` (lexical frames, `ControlFlow`, per-scan budget reset, `max_call_depth`, bounded trace ring, exact IEC 61131-3 FBs `TON/TOF/TP/CTU/CTD/CTUD/R_TRIG/F_TRIG/SR/RS`, OBs `OB100`/`OB1`/`OB30`/`OB40`/`OB50`, cooperative `WAIT_TIME` yield) + `dendri-sim-kernel` (`u64 time_us` hybrid scheduler). | Fuzz tests (infinite recursion, empty `for`, div-by-zero, overflow, `break`/`continue`, param shadowing) 100% safe; all IEC FB edge-case tests green; 2-board cooperative co-sim green. |
| **Phase 3** | **SPICE-Class Electrical Solver (`dendri-circuit`) — Tiers 1–3** | **3a (Linear DC + Diodes/LEDs):** Union-Find netlist, island partitioning, MNA LU solver, `R`/`V`/`I`/pot/switches NO&NC/relays/breadboards, Shockley Diode/LED/Zener NR + `pnjlim`, MCU/PLC pin Thevenin/ADC/overvoltage/short faults.<br>**3b (Transient & AC):** `C`, `L`, transformers, 1φ/3φ AC, PWM driving, oscilloscope probes.<br>**3c (Semiconductors & Power):** BJT, MOSFET, Optocoupler, IGBT, SCR, TRIAC, SSR, H-Bridge. | All analytical physics benchmarks pass within `< 0.1%` error; LED + external resistor works; floating cathode stays OFF; 24V→5V triggers overvoltage fault; NC ≠ NO. |
| **Phase 4** | **Certified Asset Contract, Core Kit & CLI Validator (`dendri-assets`)** | Strict profile schemas (`deny_unknown_fields`), SVG `df-pins` parser (48×48 px grid, 8 px typed pins, zero forbidden CSS/SMIL), dual-view (`physical.svg` + `schematic.svg`) enforcement, `dendri asset-check` CLI gate, and **Certified Core Kit** (4 boards + ~20 components with validated SVGs & models). | `dendri asset-check` passes 100% on Core Kit with zero warnings; every Core Kit component electrically tested end-to-end. |
| **Phase 5** | **Virtual Bus Fabric (`dendri-bus`) & Pluggable Physics (`dendri-physics`)** | Physical-topology UART, I2C, SPI (with CS), and Virtual Ethernet / Modbus TCP (FC1–6, FC15, FC16) + `PhysicalComponent` trait with Pneumatic (5/2 valve + DA cylinder + limit switches), Thermal, Tank, and Display Framebuffers (LCD/OLED/7-seg). | Multi-board Uno ↔ S7-1200 UART & Modbus co-sim green; real electro-pneumatic `A+ B+ A- B-` closed-loop physical stroke + limit switches green! |
| **Phase 6** | **Rosetta Stone Core Frontends & Non-Destructive Sync (Wave 1)** | `dendri-frontend-core`, `dendri-lang-st` (real IEC ST with `AT %IX`/`%QX`), `dendri-lang-cpp` (real Arduino C++ with `#include`/`#define`, zero `ASL_HW` comments), `dendri-lang-ladder` (full IEC LD with materialized power + `EN`/`ENO`/`Q` propagation + lifting), `dendri-lang-sfc` (full IEC 60848 Rule 5 + OR priority + `N/S/R/L/D/P` + `.X`/`.T`), and `dendri-sync`. | **Cross-Language Golden Equivalence Suite:** identical traces across C++, ST, Ladder, and GRAFCET; zero `ASL_HW` cheat comments; editing Ladder/SFC never wipes `OB_HW_INT`/functions. |
| **Phase 7** | **`dendri-editor` v2 (`egui`/`eframe`) — Wave A (Simulation + Code + Oscilloscope)** | `dendri-canvas` (48×48 px scene, unified transform $\mathbf{M}$, batched multi-wire routing, cached `usvg` layers + native 8 px `theme.rs` pins) + `egui`/`eframe` docking shell: Physical & Schematic Simulation Canvas, live actuators/probes/oscilloscope, multi-board selector, Code Editor with highlighting & live `dendri-validate` spans, I/O Tag Table, unified Undo/Redo, native `rfd` Save/Open. | Zero disk I/O during drag; worker thread decoupled from UI; pause preserved across edits; full multi-board interactive demo green; zero clippy warnings. |
| **Phase 8** | **`dendri-editor` v2 — Wave B (Real Visual Ladder & GRAFCET Editors)** | Interactive grid Ladder editor (drag-and-drop, multi-OB/POU sheets, live rail energization & `.ET`/`.CV` overlay) + interactive GRAFCET/SFC graph editor (Draw.io UX, macro-step drill-down, live active tokens & `<Step>.T` timers), backed cleanly by `dendri-sync`. | Editing visually updates Code/ASL live and vice-versa with zero data loss; live energization verified in end-to-end tests. |
| **Phase 9** | **Language Expansion (Waves 2–3), HMI/SCADA, AI Copilot (BYOK) & Target Compilers** | `dendri-lang-micropython`, `dendri-lang-rust`, HMI/SCADA/IoT data-driven builder, AI BYOK copilot gated by `dendri-validate`, real target-compiler CI verification (`arduino-cli`, OpenPLC, `rustc`) via clean runtime shims, plus gradual catalog expansion & long-tail polyglot languages (C, Zig, TinyGo, Forth, Lustre, Blockly, XOD, Node-RED). | Each expansion crate meets the exact same CI gates as Wave 1 before merge. |
