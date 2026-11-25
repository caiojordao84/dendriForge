# 05 — Desktop IDE (`egui`/`eframe`), 12-Gate CI Suite, Deferral Ledger & Phased Roadmap

> **Normative Specification** | Baseline (Rev 2) — ADRs 004, 021–024, 030  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md) through [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md)

---

## 1. Desktop IDE Architecture (`dendri-wm` + `dendri-editor`, ADR-021 — Amended)

`dendri-editor` v2 is a 100% pure-Rust engineering workstation shell built on **`egui` / `eframe`** (`wgpu` / `glow` GPU backend) and **`dendri-wm`**, eliminating v1's `iced` state-duplication, overlay event leaks, `static_cache` desyncs, dual `usvg` dependency conflicts, and missing CAD/oscilloscope primitives. Full UI/UX, window-manager, accessibility, and app contracts live in [`docs/ui/DESIGN.md`](../ui/DESIGN.md).

### Core UI Stack (Zero WebView / JS)
- **Windowing & Rendering:** `eframe` + `egui` with built-in **AccessKit** screen-reader and keyboard navigation support (plus a keyboard-navigable **scene outline** for the custom-painted `Simulation` canvas).
- **Workstation Shell & Window Manager (`dendri-wm`):** Pure-Rust, zero-`egui` crate operating in integer logical pixels and owning **Floating** (default, z-ordered overlapping windows) and **Tiling (dwindle)** (binary split tree) per workspace, enforced by property tests (`I1–I6`) and fuzzing. **`egui_dock` is restricted strictly to tab groups inside an app window** (e.g., `CodeStudio` tabs) and does not drive the outer shell.
- **Plotting & Probes:** `egui_plot` rendering real-time multi-channel voltage/current waveforms and digital logic traces directly from `dendri-circuit` / `dendri-sim-kernel` probe buffers inside the `Simulation` app's Oscilloscope / Logic Analyzer dock panel.
- **File Dialogs & Atomic I/O:** `rfd` native OS dialogs + atomic persistence (`write temp -> sync -> rename`) preventing partial/corrupted `.toml` saves on crash. User preferences (layout, keymap, `startup_behavior`) live in versioned, strict `workspace.toml` under the platform config directory — never inside project `.toml` files.
- **Runtime Asset Resolution:** Resolves fonts (`assets/fonts/` — Noto Sans & Noto Sans Mono, SIL OFL 1.1), original UI icons (`assets/icons/`), starter templates (`assets/templates/`), and certified hardware assets via `--assets-dir` → `DENDRIFORGE_ASSETS` env var → `<exe_dir>/assets` → CWD fallback. **Zero `env!("CARGO_MANIFEST_DIR")` in binaries.**
- **Theme (`dendri-theme`):** Single source of truth for the **A11Y Slate** palette, verified by automated WCAG contrast unit tests (minimum WCAG AA 4.5:1 text / 3:1 non-text UI, AAA 7:1 on primary text & pin tokens). One adapter module in `dendri-editor` converts `Color` → `egui::Color32`; CI fails on color literals outside that adapter.
- **Single `usvg` Pipeline:** Parses every hardware SVG and UI icon SVG **once** via `dendri-assets` (`usvg` / `resvg`) into cached base GPU textures (`TextureHandle`), cached dynamic path geometry (`PreparedLayer`), and parsed `<g id="df-pins">` 8 px pin anchors painted natively via `egui::Painter` colored by `dendri-theme`. `egui_extras`'s `svg` feature MUST stay disabled.
- **Decoupled Simulation Worker Thread:**
  - `dendri-sim-kernel` runs off the UI thread, publishing immutable `Arc<SimSnapshot>` frames and waking UI via `ctx.request_repaint()` **only** when visual state or user input advances.
  - Zero per-pixel disk I/O during drag (pre-loaded in-memory `AssetRegistry`); zero deep-cloning of `SimulationProject` at 60 FPS.
  - Editing canvas/wires while paused **stays paused**.
  - Unified `UndoStack` covering Canvas, Wiring, I/O Tag Table, Code, Ladder, GRAFCET, Comments, and Assumptions.

---

## 2. IDE Screen & App Contracts (ADR-022 — Amended)

Views roll out as workstation-shell **apps** sequentially by foundation — **no fake/hardcoded placeholder diagrams, stub screens, or dead dock icons allowed** (see [`docs/ui/DESIGN.md`](../ui/DESIGN.md#4-apps)). All IDE UI surfaces (including Comments, Assumptions, `Foreign`, and Packs) are implemented ONLY in Phases 7–8 **after Phase 6b (Wave 1 real-toolchain differential gate) is 100% green**.

### Wave A — Core Engineering Shell & Apps (Phase 7, Slices 7a–7e)
1. **Workstation Shell (`dendri-wm` + Top Bar, Dock/Taskbar, Status Bar, Keymap & Preferences):**
   - Floating (default) and Tiling (dwindle) per numbered workspace; native outer OS window decoration with custom inner-window chrome; solid 2 px `DF_ACCENT_SKY` focused border; compact EDA density (targets ≥ 24×24 logical px); opaque surfaces and `reduce_motion`.
   - Configurable keyboard chords (`Ctrl`/`Cmd` + `Alt`/`Shift` combos, no `Super`), scoped to shell / focused app / text editor and verified by automated conflict tests (`U6a`).
   - Working `File → Open / Save / Save As` via `rfd` and atomic persistence (no placeholder `Files` icon until Phase 8 / `DEF-010`).
2. **`Simulation` App (`Physical 48×48 px` + `IEC 60617-Style / IEC 61082 Schematic` Toggle + Oscilloscope Dock):**
   - Cursor-anchored pan/zoom, zoom-to-fit, zoom-thresholded grid (none at low zoom, 48 px module grid at medium zoom, 24 px sub-grid at high zoom only), multi-select, copy/cut/paste/duplicate, rotate 90° (`R`), mirror (`X`/`Y`) using unified transform matrix $\mathbf{M}$ (zero drift between hit-testing and visuals).
   - **Batched Multi-Wire Orthogonal Routing:** Routes all scene wires simultaneously onto clean non-overlapping channels with jump arcs/junction dots, draggable waypoints, and `Escape` cancellation (never curved wires or signal-particle animations).
   - **Live Interactive Actuators, Probes & Oscilloscope Dock:** Momentary/latched NO/NC pushbuttons, live potentiometer wipers, 4–20 mA sensor sliders, LED luminosity/color/burn, electrical fault badges, Voltage/Current/Logic Probes feeding the dockable Oscilloscope / Logic Analyzer panel, and an AccessKit keyboard-navigable **scene outline**.
   - **Multi-Board Selector:** Clicking any board on the canvas (`plc_main`, `mcu_sensor`, …) focuses its POUs, I/O Tag Table, Code, Ladder, or GRAFCET views seamlessly.
3. **`CodeStudio` App (Code Editor, ASL Tab, Board I/O Tag Table & Governance Panels):**
   - A11Y Slate syntax highlighting, line numbers, inline squiggles and hover tooltips backed by live `dendri-validate` spans (`ERR_*`).
   - Instant per-POU view/language switcher (**`C++`** ↔ **`IEC 61131-3 ST`** ↔ **`ASL`**, plus read-only derived projections, later `MicroPython` / `Rust`) retaining unsaved dirty buffers with inline errors on tab switch.
   - Dedicated **Board I/O Tag Table** binding logical tags to physical ports (`D13`, `A0`, `Ia.0 ⇄ %IX0.0`, `Qa.0 ⇄ %QX0.0`, `IW64`) with live electrical readouts.
   - **Comments Panel:** Inspect, edit, filter, and re-attach `[[boards.comments]]` and top-level `[[comments]]`, including orphaned `dangling` comments (`INFO_COMMENT_ORPHANED`).
   - **Assumptions Panel & Badges:** Visibly badges every active `[[boards.assumptions]]` entry (`WARN_ASSUMPTION_ACTIVE`), displays rationale/policy, and blocks "release" export mode while any assumption is active.
   - **`Foreign` Locked Boxes:** Displays `Decl`/`Stmt`/`Expr::Foreign` nodes clearly in foreign views with source language, fidelity badge (`F0`/`F1`/`F2`), and 4-part actionable guidance (what / where / why / what now).
   - **Packs & Capabilities Inspector:** Displays version/hash-locked `pack.toml` dependencies and project `requires` capabilities.
4. **`Console` App:**
   - Live Serial monitor and structured diagnostics log matching the headless runner.

### Wave B — Native Visual IEC Editors & In-App Project Manager (Phase 8)
5. **`LadderPLC` App (Visual Ladder Diagram Editor — IEC 61131-3 LD):**
   - Real grid editor backed directly by `PouBody::Network` + `dendri-sync` (multi-OB/POU sheets; zero stored `__rail_N` temporaries).
   - Drag-and-drop & keyboard editing of NO/NC/P/N contacts, Normal/Negated/S/R/P/N coils, parallel OR branches, Timers/Counters/Triggers/Bistables (`EN`/`ENO`/`Q`), Compare/Math/Move/Comm boxes, callable `Structured POU` boxes, `Foreign` locked boxes, and network title/body/element comments.
   - **Live Simulation Overlay:** Energized power rails and active contacts/coils illuminate in real time alongside live `.ET` / `.CV` values.
6. **`GRAFCET` App (Visual GRAFCET / SFC Editor — IEC 60848):**
   - Interactive node-graph editor (Draw.io UX) backed directly by `PouBody::Chart` + `dendri-sync` for initial/normal/macro-steps (with drill-down), transitions (explicit OR priority badges), AND/OR divergence & convergence bars, `N/S/R/L/D/P/P1/P0` action blocks, and step/transition/action comments.
   - **Live Simulation Overlay:** Active step tokens glow in real time alongside live `<Step>.T` elapsed timers and transition receptivity states.
7. **`Files` App (`dendri-files`, `DEF-010`):**
   - Headless workspace index, bundle-aware move/rename/duplicate (`.toml` + `.dendrilib` locks), hash-locked templates (`assets/templates/`), last-5 rolling backups in platform user-data dir, portable `.dendri-trash/` with restore, off-thread validation/assumption/`Foreign` badges, safe external-change detection, and fuzzed canonical path confinement ([`docs/ui/DESIGN.md`](../ui/DESIGN.md#7-files-app-dendri-files--planning)).

### Wave C — Convergence & Extensibility (Phase 9)
8. **Unified HMI / SCADA / IoT Builder:** Data-driven dashboard canvas binding validated SVG gauges, numeric displays, trend charts, alarm lists, and operator buttons directly to any board's ASL tags and virtual Modbus/telemetry bus.
9. **AI Engineering Copilot (BYOK):** Provider-agnostic Bring-Your-Own-Key assistant (OpenAI / Anthropic / Gemini / local Ollama); **every generated edit MUST pass `dendri-validate` before one-click apply**, and all assets flagged `ml_restricted = true` MUST be excluded from every index, prompt context, and embedding corpus.

---

## 3. Clean v2 Workspace Crate Architecture (Acyclic DAG)

```text
Layer 0 (Foundations — zero internal deps)
├── dendri-span           Byte spans, SourceMap, rich Diagnostics, frozen ErrorCode enum
├── dendri-theme          A11Y Slate palette, semantic pin tokens, WCAG contrast tests
└── dendri-stdlib         Domain-split stdlib (std.iec.* vs std.arduino.*/std.plc.*) & standards.toml

Layer 1 (IR, Expressions, Assets & Validation)
├── dendri-ast            ASL v2 IR (OBs/POUs, Network/Chart, ANALOG/DUTY/CODE, modules/packs/Foreign, comments)
├── dendri-expr           Official Pratt expression parser & canonical printer (incl. FOREIGN(...))
├── dendri-assets         Strict TOML profiles, [provenance], SVG sanitization, df-pins, review.toml, asset-check
├── dendri-validate       Full static typechecker, codec/band/comment/module/capability/OB/ISR/wiring validator
└── dendri-project        Single-file v2 .toml load/save/round-trip (atomic I/O, serde_spanned)

Layer 2 (Simulation & Multi-Domain Physics)
├── dendri-vm             Cycle-based ASL VM fibers, lexical frames, libm, IEC FBs, load-time Network/Chart lowering
├── dendri-circuit        SPICE-class event-driven + time-varying transient MNA solver, round-half-even ASL boundary
├── dendri-bus            Physical-topology UART, I2C, SPI, and Virtual Ethernet / Modbus TCP fabric
├── dendri-physics        Pluggable PhysicalComponent models (Pneumatics, Thermal, Tank, Display charsets)
└── dendri-sim-kernel     Hybrid discrete-event + cycle-accurate u64 time_us orchestrator & snapshot channel

Layer 3 (Polyglot Pure-Rust Frontends & Sync)
├── dendri-frontend-core  Shared UTF-8 lexer, Pratt engine, I/O tag resolver, comment renderer, LanguageFrontend trait
├── dendri-lang-st        IEC 61131-3 Structured Text parser & emitter + LBC
├── dendri-lang-cpp       C++ (Arduino subset) parser & emitter + LBC
├── dendri-lang-ladder    IEC 61131-3 Ladder structural IR, derived projection & conservative lifting + LBC
├── dendri-lang-sfc       IEC 60848 GRAFCET/SFC structural IR, derived projection & lifting + LBC
├── dendri-sync           Non-destructive per-board/per-POU revisioned BoardDoc & unified undo transactions
└── [Wave 2 / Wave 3]     dendri-lang-micropython (post Wave 1 + DEF-004), dendri-lang-rust (post Wave 2)

Layer 4 (Shell, Canvas & Applications)
├── dendri-wm             Pure-Rust floating z-order, dwindle split tree & workspaces (zero egui; Slice 7a)
├── dendri-canvas         48×48px Scene graph, unified transform M, batched wire routing, cached usvg layers (Slice 7c)
├── dendri-diff           Real-toolchain differential verification harness (g++, RuSTy, arduino-cli, simavr)
├── dendri-cli            `dendri` CLI (asset-check, validate, ledger-check, sim, bench-spice, translate, roundtrip, diff-check)
├── dendri-editor         `egui`/`eframe` workstation shell & apps (Simulation, CodeStudio, Console, LadderPLC, GRAFCET, Files, HMI, BYOK)
└── [Phase 8 / DEF-010]   dendri-files (headless workspace index, bundles, templates, rolling backups, portable trash, path safety)
```

---

## 4. Twelve-Gate CI Verification Suite

Enforces **ADR-004** and **ADR-023**. Every PR and phase completion MUST pass all applicable CI gates with zero warnings and zero skipped assertions:

1. **Hygiene & Formatting:** `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo deny` / single `usvg` dependency check.
2. **Panic, Wildcard & Determinism Lints:**
   - Workspace `[workspace.lints.clippy]` sets `unwrap_used`, `expect_used`, `indexing_slicing`, `panic`, `todo`, and `unimplemented` to **`deny`**.
   - Workspace `clippy.toml` sets `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`, `allow-indexing-slicing-in-tests = true`, and `allow-panic-in-tests = true` so only test code may panic. Any justified non-test exception MUST use `#[expect(..., reason = "...")]`.
   - ASL domain crates (`ast`, `vm`, `circuit`, `validate`, frontends) set `clippy::wildcard_enum_match_arm = "deny"` (enum-only; `char` lexers unaffected).
   - Determinism-critical crates (`vm`, `circuit`, `bus`, `physics`, `sim-kernel`, `validate`, `sync`, emitters) set `clippy::iter_over_hash_type = "deny"` (enforcing `BTreeMap` / `IndexMap`).
3. **Unit & Property Tests (`proptest`):** `cargo test --workspace` including canonical round-trip properties (`parse(print(ast)) == ast` and `LOSSLESS_ASL`) across `dendri-expr`, `dendri-project`, and every frontend.
4. **Real Negative Error Corpus:** At least **1 genuine `.toml` or source fixture per `ErrorCode` variant** asserting exact error code and accurate byte `Span` — **zero synthetic `fixture_inject` harnesses**.
5. **Asset Contract, Provenance & License Gate (`dendri asset-check`):** Validates 100% of `assets/` against the 48×48 px grid, 8 px `df-pins` 1:1 bijection, mandatory dual-view SVGs, canonical sanitization pipeline, `[provenance]` + `THIRD_PARTY_NOTICES` + `ml_restricted` exclusion, `review.toml` canonical render SHA-256 freshness, `standards.toml`, codec `D <= M`, and signed band `i32` bounds ([`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md#7-automated-asset-validator-gate-dendri-asset-check)).
6. **SPICE Analytical Benchmarks (`dendri-circuit`, headless in Phase 3):** Validates RC/RL transients, voltage dividers, Shockley diodes/LEDs/Zener, 4–20 mA shunts, 1φ/3φ AC (including purely resistive AC), BJT, and MOSFET against closed-form analytical equations (`error < 0.1%`).
7. **Numeric Contract Gate (`T-NUM-1..5`):**
   - `T-NUM-1`: exhaustive `n → a → n` round trip for every catalog codec with `D ≤ 2^24` (release-mode job; signed codes for `span_bipolar`; proof + boundaries + `≥ 10^7` random samples for larger `D`).
   - `T-NUM-2`: composition for every ordered pair of catalog codecs against the clamped rational `min(n_max2, floor(n * D2 / D1))`; zero deviations required whenever `lcm(D1, D2) ≤ M`; exhaustive enumeration when `D1 ≤ 2^16` for `lcm > M` recorded in the catalog.
   - `T-NUM-3`: monotonicity in both directions, endpoints (`0 → 0`; `a = M → n_max` for `span`; `n_max → n_max`), idempotence, and mandatory clamp check for every `span → binned` pair.
   - `T-NUM-4`: every profile codec satisfies `D ≤ M` and every band limit fits signed `i32`, with negative fixtures (`ERR_CODEC_RESOLUTION_EXCEEDS_SCALE`, `ERR_BAND_LIMIT_OVERFLOW`).
   - `T-NUM-5`: worked examples including `2 V` on a `0–10 V` channel → `a = 200_000_000` → PLC 27648-scale code `5529` (truncated from `5529.6`).
8. **Comment Conservation Gate (`C1–C5`):**
   - `C1`: multiset of comment texts invariant across any Wave 1 translation chain (modulo declared `inner` relocation).
   - `C2`: zero comments lost when nodes are edited or deleted in another view (`dangling` + `INFO_COMMENT_ORPHANED`).
   - `C3`: hostile corpus (`*/`, `*)`, trailing `\`, CRLF, combining characters, emoji, RTL/bidi control characters, very long lines) survives every Wave 1 emitter and compiles cleanly in Gate 10.
   - `C4`: re-import idempotence (`emit → parse` leaves stored verbatim text unchanged).
   - `C5`: negative fixtures for `ERR_COMMENT_FIELD`, `ERR_COMMENT_ANCHOR`, and `deny_unknown_fields`.
9. **Multi-Board & Cross-Language Golden Equivalence Suite + Independent Oracles (Principle P4):**
   - **Zero v1 fixture reuse:** Only v1's *behavioral scenario descriptions* carry over (button → LED on Uno; NO button → pilot lamp on `plc-1200-open-1214`; tactile pull-up → LED; potentiometer on A0 → ADC; 4–20 mA transmitter + 500 Ω shunt on PLC AI; multi-board Uno ↔ `plc-1200-open-1214` electrical + UART/Modbus). All v2 scenarios are authored cleanly in the v2 schema.
   - **Independent Oracles:** Expected outputs MUST derive from datasheets and hand/analytical calculation — **never from simulator output**. Snapshots MAY be committed only after human approval with the approver recorded in the scenario file.
   - **Equivalence Definition:** (i) equivalent sources lift to structurally equal ASL; (ii) identical ASL yields bit-identical digital/integer traces across OSes (analog waveforms compared within declared tolerance); (iii) cross-idiom comparisons (e.g., `delay()` vs `TON`) use an explicit per-scenario tolerance declared in the scenario.
   - **Post-Phase 3 Tracer Bullet:** Immediately after Phase 3, a headless CLI scenario (Uno + button + LED) runs end-to-end across `dendri-vm` and `dendri-circuit` against its independent oracle before any later phase builds on the VM↔circuit seam.
10. **Real-Toolchain Differential Gate (Phase 6b — Wave 1 Exit, ADR-004):**
    - **C++:** Emitted C++ is compiled and **executed** on the host against a deterministic Arduino API shim (virtual clock, pin-event log); pin/serial traces are compared against `dendri-vm`. Nightly CI runs compile-only checks against the real Arduino Uno core (`arduino-cli`).
    - **AVR Oracle (`DEF-003` — ratified):** `avr-gcc` + `simavr` as independent ground truth for AVR-specific behavior (16-bit `int`, PWM, ADC).
    - **IEC ST (`DEF-003` — ratified):** Emitted ST is compiled directly to optimized native code via LLVM using **RuSTy** (`plc`, written in Rust — zero C intermediate step, native multi-architecture support) linked against a deterministic Rust I/O shim. Ladder and GRAFCET are verified through their emitted ST.
    - Hostile comment corpus (`C3`) compiles and executes through this gate.
    - MicroPython and Rust differential gates open with Waves 2 and 3.
11. **Fuzzing & Weekly Mutation Testing:**
    - `cargo-fuzz` target for every parser (`dendri-expr`, `dendri-project`, `dendri-assets` profiles, and each language frontend).
    - Weekly `cargo-mutants` job on core crates; **surviving mutants in tested code fail the weekly job** (catching tautological tests).
12. **Stub & Deferral Ledger Gate (`dendri ledger-check`):**
    - Validates `docs/deferrals.toml` against `docs/milestone.toml` (§5.2) and reports open deferrals, `Foreign` nodes, and active assumptions in the CI summary.

---

## 5. Definition of Done and the Deferral Ledger

### 5.1 Definition of Done Templates

| Unit | Done Means |
|---|---|
| **Feature / Crate** | Unit + property + negative tests; every new `ErrorCode` has a genuine producer fixture; zero production panics / wildcard / hash-iter violations (Gate 2); docs updated; deferral ledger clean |
| **Frontend** | LBC published (`04` §2); generated feature matrix updated; shared conformance suite green; comment conservation (`C1–C5`) green; numeric mapping green; differential gate green (from Phase 6b) |
| **Component** | Complete 8-part unit of [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md#11-complete-certified-component-unit) |
| **Board** | Pinout, electrical thresholds, `[cpu]` (`int_bits`, `clock_hz`), `[execution.cost]`, `indeterminate_reads_as`, ADC/PWM codecs, band tables & transfer functions with `source_ref`, `iec_address` map, validated SVGs + provenance + `review.toml`, and ≥1 end-to-end scenario |
| **Phase** | All applicable CI gates green with zero warnings or skipped assertions; all deferral ledger entries due in the phase closed |

### 5.2 The Deferral Ledger (`docs/deferrals.toml` & `docs/milestone.toml`)
Under Principle P1, a deliberate deferral exists **only** as a typed entry in `docs/deferrals.toml` (deserialized with `#[serde(deny_unknown_fields)]`). Milestones belong to a **closed, totally ordered enum** (zero mixed integers/strings):

```text
phase-0 < phase-1 < phase-2 < phase-3 < phase-4 < phase-5 < phase-6 < phase-6b
        < wave-1-exit < phase-7 < phase-8 < wave-2-gate < phase-9 < wave-3-exit < post-wave-3
```
*(Ordering note: `wave-2-gate` strictly precedes `phase-9` so Wave 2 prerequisites such as `DEF-004` (`asl-oop`) MUST close before entering Phase 9's Wave 2 implementation.)*

```toml
[[deferral]]
id                 = "DEF-001"
title              = "PWM transfer functions per timer mode"
owner_milestone    = "phase-3"
deadline_milestone = "phase-3"
reason             = "Requires datasheet + Arduino core source verification"
refs               = ["docs/architecture/01-asl-ir-and-project-schema.md#35-types-intrinsics--two-layer-separation"]
```

The active milestone is recorded in `docs/milestone.toml`. `dendri ledger-check` MUST fail CI on:
- any `todo!`, `unimplemented!`, `#[ignore]`, or `TODO` / `FIXME` / `XXX` comment without a valid `DEF-nnn` reference;
- any open ledger entry whose `deadline_milestone < current_milestone`;
- any ledger entry with an empty `refs` list or referencing a nonexistent path;
- any unknown milestone string or unknown TOML key.

#### Initial Deferral Ledger Entries

| ID | Item | Owner | Deadline | Status Note |
|---|---|---|---|---|
| `DEF-001` | PWM transfer functions per timer mode (Arduino Uno R3 first) | `phase-3` | `phase-3` | Verify against datasheet + core source |
| `DEF-002` | Channel band tables and bipolar truncation behavior (industrial 27,648-count PLC AI & SM modules) | `phase-4` | `phase-4` | Mandatory `source_ref` per channel |
| `DEF-003` | Toolchain selection for Phase 6b differential gate (AVR oracle `simavr`; `matiec` vs `RuSTy`; `arduino-cli`) | `phase-5` | `phase-5` | **Sole open evaluation decision (D7)** |
| `DEF-004` | `asl-oop` capability design (IEC 61131-3 3rd-ed. FB methods & interfaces) | `wave-2-gate` | `wave-2-gate` | Prerequisite for Wave 2 MicroPython |
| `DEF-005` | `asl-motion` capability and task-based `ExecutionKind` (robotic languages horizon) | `post-wave-3` | `post-wave-3` | Horizon guardrail |
| `DEF-006` | Comment-style dialect table per IEC ST target | `phase-6` | `phase-6` | Feeds `LanguageCapabilities.comment_styles` |
| `DEF-007` | Display charset tables (HD44780 LCD ROM variants) | `phase-5` | `phase-5` | Feeds display-sink `ERR_STRING_CHARSET` |
| `DEF-008` | Initial `pack.toml` set derived from Phase 6 scenario list | `phase-6` | `phase-6` | `F1`/`F2` packs for Wave 1 scenarios |
| `DEF-009` | Reference-designation scheme for schematic labels (IEC 81346 / IEC 61082) | `phase-4` | `phase-4` | Applied across schematic SVGs (Closed) |
| `DEF-010` | `Files` app (`dendri-files`): in-app project manager | `phase-7` | `phase-8` | Design ratified in `docs/ui/DESIGN.md` §7; Phase 7 ships `File → Open/Save/Save As` via `rfd` |

---

## 6. Multi-Phase Master Implementation Roadmap (ADR-024)

We NEVER advance to Phase $N+1$ until Phase $N$ meets its full DoD (§5.1) with all applicable CI gates 100% green and zero skipped assertions or warnings.

| Phase | Name & Focus | Core Deliverables | Hard DoD Gate |
|---|---|---|---|
| **Phase 0** | **Clean-Slate Bootstrap, Lint & Ledger Scaffolding** | Create branch `v2`; archive v1 (`crates/`, `content/`, `core/`, stale `docs/`) into `legacy/v1/`; delete `.restore-chunks` and `docs/Empty`; clean root `Cargo.toml` with workspace panic/wildcard/determinism lints + `clippy.toml`; technical `README.md`; `docs/deferrals.toml` + `docs/milestone.toml` + `dendri ledger-check` (with unit tests); fuzz (`cargo-fuzz`) and mutation (`cargo-mutants`) CI scaffolding; provenance/license gate scaffolding; `dendri-span`, `dendri-theme` (WCAG-tested A11Y Slate), `dendri-stdlib` (domain-split `std.iec.*` + `standards.toml`). | Empty workspace builds & passes strict CI (Gates 1, 2, 11, 12); WCAG contrast tests green; ledger & provenance checkers self-tested; zero warnings. |
| **Phase 1** | **ASL v2 IR, Numeric Contract, Modules, Comments, Expressions & Validator** | `dendri-ast` (OBs/POUs with `PouBody::{Statements, Network, Chart}`, full scalar+composite types, `ANALOG`/`DUTY`/`CODE` + unipolar/bipolar `Codec` at `M = 10⁹`, identifier & endpoint grammar, modules/`requires`/packs/3-level `Foreign`, `[[boards.comments]]` + `[[comments]]`, `[[boards.assumptions]]`), `dendri-expr` (Pratt parser + canonical printer incl. `FOREIGN`), `dendri-project` (atomic TOML I/O), `dendri-validate` (100% of `ErrorCode` variants in `01` §8 live!). | `proptest` lossless round-trip green; **1 real negative fixture per `ErrorCode`** passing with exact spans; `T-NUM-1..4` and model-level `C1–C2, C5` green; zero dead error codes. |
| **Phase 2** | **Cycle-Accurate Multi-Board VM (`dendri-vm`) & Kernel Skeleton** | `dendri-vm` (cycle-based virtual clock `fiber_cycles`/`clock_hz` in 128-bit arithmetic + `[execution.cost]`, `libm` transcendentals, ordered maps, `INDETERMINATE` policy, lexical frames, `ControlFlow`, per-scan budget reset, `max_call_depth`, bounded trace ring, exact IEC 61131-3 FBs, OBs `OB100`/`OB1`/`OB30`/`OB40`/`OB50`, load-time lowering of `Network` & `Chart`, cooperative `WAIT_TIME`/`WAIT_TIME_US` yield) + `dendri-sim-kernel` (`u64 time_us`, non-exhaustive `ExecutionKind`). | Fuzz tests (infinite recursion, empty `for`, polling loops, div-by-zero, overflow, `break`/`continue`, param shadowing) 100% safe; all IEC FB & structural lowering tests green; 2-board cooperative co-sim green. |
| **Phase 3** | **Headless SPICE-Class Solver (`dendri-circuit`) & Tracer Bullet** | **Headless (zero SVG dependency):** **3a (Linear DC + Diodes/LEDs):** Union-Find netlist, island partitioning, MNA LU solver, `R`/`V`/`I`/pot/NO&NC switches/relays/breadboards, Shockley Diode/LED/Zener NR + `pnjlim`, MCU/PLC pin Thevenin + round-half-even Solver→ASL boundary (`M = 10⁹`) + faults.<br>**3b (Transient & AC):** Event-driven DC + transient stepping for reactive elements OR time-varying sources (`steps_per_period = 200`), `C`/`L`/transformers, 1φ/3φ AC (incl. purely resistive AC), verified PWM transfer functions (`DEF-001` closed), probes.<br>**3c (Semiconductors & Power):** BJT, MOSFET, Optocoupler, IGBT, SCR, TRIAC, SSR, H-Bridge.<br>**Tracer Bullet:** Headless CLI Uno + button + LED end-to-end scenario through `dendri-vm` + `dendri-circuit`. | All headless analytical physics benchmarks pass `< 0.1%` error; `DEF-001` closed; `T-NUM-5` verified through solver; Tracer Bullet passes against independent oracle. |
| **Phase 4** | **Certified Asset Contract, Narrowed Core Kit & `dendri asset-check`** | `dendri-assets` (strict profile schemas, SVG normalization & sanitization pipeline, `<g id="df-pins">` 48×48 px / 8 px parser, dual-view `physical.svg` + original IEC 60617-style / IEC 61082 `schematic.svg`, source-neutral `[provenance]` + `THIRD_PARTY_NOTICES` + `ml_restricted`, `review.toml` render-hash gate + `dendri asset-approve`, composite sub-circuits) + **Narrowed Core Kit:** 2 boards (`arduino-uno-r3`, `plc-1200-open-1214` with verified industrial 27,648-count band tables closing `DEF-002` and reference designations closing `DEF-009`) + 8 mirrored components (`maker-tactile-4pin`, `cmd-pushbutton-no-start`, `cmd-pushbutton-nc-stop`, `led`, `signal-pilot-lamp`, `potentiometer`, `sensor-pressure-4-20ma`, `resistor`/shunt). | `dendri asset-check` passes 100% with zero warnings; `DEF-002` & `DEF-009` closed; every Core Kit component & board passes complete `03` §1.1 unit + independent-oracle scenario. |
| **Phase 5** | **Virtual Bus Fabric, Pluggable Physics & Subsystem Batch Certification** | `dendri-bus` (UART, I2C, SPI w/ CS, Virtual Ethernet / Modbus TCP FC1–6/15/16) + `dendri-physics` (Pneumatics 5/2 valve + DA cylinder + limit switches, Thermal, Tank, Display framebuffers w/ charset tables closing `DEF-007`) + **Batch Certification** of bus, display, and pneumatic components under the full `03` §1.1 unit + finalize differential gate toolchains (closing `DEF-003` / D7). | Multi-board Uno ↔ `plc-1200-open-1214` UART & Modbus co-sim green; electro-pneumatic `A+ B+ A- B-` closed-loop physical stroke + limit switches green; `DEF-003` & `DEF-007` closed; batch assets pass `dendri asset-check`. |
| **Phase 6** | **Rosetta Stone Wave 1 Frontends, LBCs & Non-Destructive Sync** | `dendri-frontend-core`, `dendri-lang-st` (real IEC ST, `iec_address`, comment dialects closing `DEF-006`), `dendri-lang-cpp` (Arduino C++, `[cpu] int_bits`, multi-tab, zero `ASL_HW`), `dendri-lang-ladder` (`PouBody::Network` + conservative lifting), `dendri-lang-sfc` (`PouBody::Chart`), `dendri-sync` (revisioned `BoardDoc` preserving comments/assumptions/`Foreign`/POUs), LBC docs + auto-generated feature matrix, initial packs (closing `DEF-008`). | Shared conformance suite green for all 4 Wave 1 frontends; `C1–C5` & `T-NUM-*` green; Golden Equivalence Suite green across C++, ST, Ladder, GRAFCET; `DEF-006` & `DEF-008` closed. |
| **Phase 6b** | **Real-Toolchain Differential Gate (Wave 1 Exit — ADR-004)** | Automated compile-and-execute differential harness running emitted Wave 1 C++ (host deterministic Arduino shim + nightly `arduino-cli` + AVR oracle per `DEF-003`) and emitted IEC ST (selected open ST compiler + I/O shim per `DEF-003`, also verifying Ladder & GRAFCET via emitted ST) against `dendri-vm` traces, including full hostile comment corpus (`C3`). | **Wave 1 Exit Gate:** 100% differential trace match on all Wave 1 golden scenarios + hostile comment corpus compiles & runs cleanly across real toolchains. |
| **Phase 7** | **`dendri-wm`, `dendri-canvas` & `dendri-editor` v2 (`egui`/`eframe`) — Wave A Shell & Apps (Slices 7a–7e)** | See [`docs/ui/DESIGN.md`](../ui/DESIGN.md#82-slices): **7a** `dendri-wm` (Floating + Dwindle, `I1–I6` proptests + fuzz); **7b** single theme adapter, shell (top bar, dock, status bar), runtime Noto fonts + original SVG icons + `provenance-check` extension, `assets/templates/uno-button-led.toml`, keymap w/ conflict test (`U6a`), `workspace.toml` prefs (`startup_behavior`), `File → Open/Save/Save As`; **7c** `Simulation` app (`dendri-canvas`, zoom-thresholded grid, batched orthogonal wires, actuators/probes, Oscilloscope dock panel, AccessKit scene outline); **7d** `CodeStudio` (`C++`, `IEC ST`, `ASL` tabs + derived projections, Board I/O Tag Table, Comments/Assumptions/`Foreign`/Packs, diagnostics); **7e** `Console` (Serial + diagnostics). | All 7a–7e slice gates green: zero color literals outside adapter; `I1–I6` proptests + fuzz green; `U6a` keymap conflict test green; zero disk I/O during drag; worker thread decoupled; pause preserved; headless tracer-bullet oracle + roundtrip/comment suites green; zero warnings. |
| **Phase 8** | **`dendri-editor` v2 — Wave B (`LadderPLC`, `GRAFCET` & `Files` [`DEF-010`])** | Interactive grid `LadderPLC` app (`PouBody::Network`, drag-and-drop, multi-OB/POU sheets, live rail energization & `.ET`/`.CV` overlay, comments & `Foreign` boxes) + interactive `GRAFCET` app (`PouBody::Chart`, Draw.io UX, macro-step drill-down, live active tokens & `<Step>.T`) backed cleanly by `dendri-sync` + `Files` app (`dendri-files`, closing `DEF-010`: bundle-aware ops, `assets/templates/`, rolling backups, `.dendri-trash/`, off-thread badges, external-change detection, path-safety fuzz target). | Visual edits update Code/ASL live and vice-versa with zero comment/assumption/POU loss; live energization verified in end-to-end tests; `Files` DoD green and `DEF-010` closed. |
| **Phase 9** | **Waves 2–3 Languages, ESP32/Pico, HMI/SCADA & AI Copilot (BYOK)** | **Pre-entry (`wave-2-gate`):** Close `DEF-004` (`asl-oop`) + certify `esp32-devkitc-v4` & `raspberry-pi-pico` (exact port `read_u16`/`duty_u16` from source).<br>**Phase 9:** `dendri-lang-micropython` (Wave 2 + differential gate), `dendri-lang-rust` (Wave 3 + differential gate), HMI/SCADA/IoT data-driven builder, AI BYOK copilot gated by `dendri-validate` (excluding `ml_restricted` assets). Long-tail & robotic languages remain in Non-Normative Horizon Appendix (`04` Appendix A, `DEF-005`). | `DEF-004` closed before Wave 2 opens; every expansion crate & board passes the exact same 12-Gate CI suite as Wave 1 before merge. |
