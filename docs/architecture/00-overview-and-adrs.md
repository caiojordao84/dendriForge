# dendriForge v2 — Architectural Overview & Decision Log

> **Status:** Active Design Phase (Pre-Implementation Specification)  
> **Language:** Technical English (normative specifications)  
> **Scope:** Complete clean-slate v2 architecture for `dendriForge` — a deterministic, pure-Rust embedded & industrial automation simulation platform unifying MCUs, PLCs, visual IEC languages, textual languages, and interactive hardware schematics under **ASL (Abstract Syntax Language)**.

---

## 1. Product Vision

`dendriForge` aims to be a full-grade alternative to interactive hardware simulators (such as Wokwi and Tinkercad) while adding **industrial automation standards compliance** (IEC 61131-3, IEC 60848, IEC 60947, ISO 1219, IEC 60617) built around **ASL** as a dual-purpose core:

1. **Foundation Stone:** ASL is the canonical, strongly-typed intermediate representation (IR) executed by the deterministic simulation engine, validated by the static analyzer, and serialized inside single-file `.toml` projects.
2. **Rosetta Stone:** ASL bridges traditional embedded programming (C++/Arduino, MicroPython, Rust) and industrial automation programming (IEC 61131-3 Structured Text, Ladder Diagram, GRAFCET/SFC). A software developer can transition smoothly into PLC/industrial engineering, and an automation engineer can program an MCU in Ladder or GRAFCET seamlessly within the same environment.

### Core Platform Capabilities (Target Architecture)
- **Multi-board, heterogeneous single-file `.toml` projects:** Multiple MCUs and PLCs co-simulated simultaneously on the same canvas, communicating electrically and over buses/protocols, with each board independently authored in the user's chosen language.
- **Seamless multi-view IDE:** Instant, non-destructive switching across Simulation Canvas, Code, Ladder, GRAFCET/SFC, and future HMI/SCADA/IoT views backed by the same single source of truth.
- **Strict Asset Contract:** Every board, component, and screen has a rigid, machine-validated TOML profile coupled to its SVG geometry, terminal anchors, electrical model, and dynamic visual state bindings.
- **Zero-compromise engineering quality:** Strict TDD, zero panics on untrusted input, zero silent data loss, deterministic execution, and honest acceptance tests enforced in CI.

---

## 2. Non-Negotiable Engineering Invariants (Quality Gates)

To prevent the structural failure modes identified in the v1 audit, every v2 crate must satisfy these hard invariants from commit one:

1. **Strict TDD & Honest Assertions:** Every crate ships with unit + property/negative tests before moving to the next phase. Tautological assertions, loosened thresholds (e.g., `ok >= 2`), and skipped AST fields in round-trip checks are forbidden.
2. **Zero Panics on Untrusted Input:** No `.unwrap()`, `.expect()`, or unchecked slice indexing (`args[i]`) on user-supplied programs, projects, profiles, or SVGs. All parse, validation, routing, and runtime failures return structured `Result` diagnostics with source spans.
3. **Strict Schema Enforcement:** Every TOML schema (ASL IR, project files, board/component/screen profiles, stdlib catalog) uses strongly-typed structs with `#[serde(deny_unknown_fields)]` — zero silent field drops and zero untyped `rest: Map` dumping grounds.
4. **No Magic-Comment Cheating:** Language parsers must parse genuine human-written code without relying on hidden `// ASL_HW:` / `// ASL_DATA:` round-trip comments.
5. **Single Source of Truth:** Never duplicate executable logic across parallel representations (`loop`, `ladder`, `grafcet`, `sources`) that can silently drift or clobber each other.
6. **Zero Compile-Time Machine Paths:** Binaries (`dendri`, `dendri-editor`) must resolve assets relative to runtime paths / CLI flags / environment variables — never hardcoded `env!("CARGO_MANIFEST_DIR")`.
7. **CI Enforcement:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, full `cargo test --workspace`, profile/SVG contract verification, and full golden/error/round-trip matrices must all pass on every PR.

---

## 3. Architecture Decision Records (ADRs)

### ADR-001: Clean-Slate v2 Rebuild with Strict TDD
- **Status:** Accepted (2026-11-22)
- **Context:** The v1 codebase (~36k LOC across 18 crates) accumulated pervasive structural debt: corrupted TOML profiles (154/155 `[canvas]`, 29/155 `[limits]`), corrupted golden fixtures locked by byte-exact tests, 1,641 LOC of legacy TOON regex migrator inside `dendri-ast`, 83% copy-pasted parsers, non-functioning `break`/`continue`, stack-overflow crashes, unbounded memory/budget leaks in the simulator, hardcoded 1-hop electrical shortcuts, and hardcoded C01 visual diagrams in the editor.
- **Decision:** Archive the existing v1 workspace into `legacy/` (when execution begins) and rebuild `dendriForge` v2 crate-by-crate from scratch under strict TDD. Valid non-code assets (selected base SVGs, reference datasheets/profile facts after automated sanitization, and validated architectural concepts) will be curated and migrated into v2 only after passing v2's strict validators.
- **Consequences:** Higher upfront design discipline, complete elimination of legacy TOON/v010 contamination, and clean crate boundaries from day one.

### ADR-002: Living Technical Specifications in `docs/architecture/`
- **Status:** Accepted (2026-11-22)
- **Context:** v1 suffered severe drift between `README.md`, `docs/`, and actual crate behavior.
- **Decision:** All architectural decisions, formal schemas, contracts, and phased implementation plans are authored in crisp technical English under `docs/architecture/` in the repository before implementation starts, and serve as the normative specification for v2 tests and crates. Obsolete v1 docs will be archived alongside v1 code when v2 execution begins.

### ADR-003: Staged "Rosetta Stone" Language Rollout
- **Status:** Accepted (2026-11-22)
- **Context:** Attempting to ship 6 languages simultaneously in v1 resulted in four half-working copy-pasted text parsers and two fake visual editors.
- **Decision:** Build and harden the language ecosystem in strict quality-gated waves:
  - **Wave 1 (Rosetta Stone Core):** `ASL` + `C++ (Arduino subset)` + `IEC 61131-3 ST` + `Ladder (LD)` + `GRAFCET (SFC)`. This proves both directions of the MCU ↔ Industrial bridge before expanding horizontally.
  - **Wave 2:** `MicroPython` (real lexer/parser, zero string-splitting hacks).
  - **Wave 3:** `Rust (Dendri HAL / Embassy-aligned subset)`.
- **Consequences:** Fewer moving parts during core stabilization; every language wave must pass full parser, validator, simulator, and cross-language translation gates before the next wave opens.

### ADR-004: Internal Deterministic Simulation & Cross-Language Translation First; External Compiler Verification Deferred
- **Status:** Accepted (2026-11-22)
- **Context:** Validating generated code against external toolchains (`arduino-cli`, `avr-gcc`, OpenPLC/CODESYS, `rustc`) adds significant toolchain/shim complexity early in v2.
- **Decision:** Priority 1 is rock-solid deterministic simulation and honest, lossless cross-language translation inside `dendriForge`. Code emitters will still be architected cleanly (valid target grammar, typed signatures, structured runtime boundary) so that real hardware compilation + CI toolchain verification can be added cleanly in a dedicated later phase without redesigning the AST or emitters.

### ADR-005 through ADR-024 Index
| ADR | Decision | Specification |
|---|---|---|
| **ADR-005** | Full IEC 61131-3 + Embedded Core static type system (fixed-width scalars, fixed arrays, flat structs, enums, first-class FBs, typed functions; zero pointers/heap) | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-006** | Single-file multi-board `.toml` project (`[[boards]]`) with per-board `authoring_language`, ASL as sole executable truth, and NodeId-anchored presentation metadata | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-007** | Single canonical typed AST in memory (zero `ExprText` / duplicate `Expr::Lit`) with formal Pratt expression grammar + canonical printer | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-008** | Unified cooperative execution model (`WAIT_TIME` yields only that board's fiber while PLCs, ISRs, timers, and LEDs continue live) | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-009** | Full SPICE-class MNA transient & nonlinear analog solver (`dendri-circuit`) from day one (R/C/L/AC 1φ&3φ/Diodes/LED/BJT/MOSFET/Power) | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-010** | Hybrid discrete-event + fixed-tick deterministic microsecond kernel (`u64 time_us`) running on a dedicated worker thread | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-011** | Physical-topology virtual bus fabric (UART, I2C, SPI with CS, Virtual Ethernet / Modbus TCP) wired strictly to canvas nets | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-012** | Decoupled multi-domain `PhysicalComponent` trait outside CPU VM (Electrical first, then Pneumatic/Hydraulic/Thermal/Displays) | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-013** | Declarative TOML `[svg.bindings]` ↔ SVG IDs (visibility/paint/translate/rotate/overlay rects); zero CSS `var()`, SMIL, or XML string slicing | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |
| **ADR-014** | Canonical 48×48 px grid module + `<g id="df-pins">` 8 px semantic pin markers validated 1:1 against TOML and painted by `theme.rs` (A11Y Slate) | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |
| **ADR-015** | Mandatory dual-view SVGs (`physical.svg` + IEC 60617 / ISO 1219 `schematic.svg`) with 1:1 port parity for every component | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |
| **ADR-016** | Certified Catalog Policy: zero profiles without validated SVGs + physics model; start with 100%-green Core Kit; legacy 215 profiles stay in `legacy/` | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |
| **ADR-017** | 100% Pure-Rust Polyglot Frontend Framework (`dendri-frontend-core`, zero C `tree-sitter` FFI) enabling long-tail languages (C, Zig, Forth, Lustre, Blockly, XOD, Node-RED, etc.) | [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) |
| **ADR-018** | IEC 61131-3 / TIA Portal Organization Blocks (`OB_STARTUP`, `OB_MAIN`, `OB_CYCLIC_INT`, `OB_HW_INT`, `OB_COMM_EVENT`) & Multi-POU (`FC`/`FB`) hierarchy for lossless MCU↔PLC bridging | [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) |
| **ADR-019** | Full industrial IEC 61131-3 Ladder (materialized rail, `EN`/`ENO`/`Q`, P/N edges, Compare/Math/Move) & IEC 60848 GRAFCET (OR priority, Rule 5 two-phase, `N/S/R/L/D/P`, `.X`/`.T`, macro-steps) | [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) |
| **ADR-020** | Project-level Board I/O Tag Table with idiomatic language bindings (`const`/`#define`/`pinMode`, `AT %IX/%QX`, visual tag picker) — zero `ASL_HW` magic comments | [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) |
| **ADR-021** | Pure-Rust `egui`/`eframe` desktop IDE (`egui_dock`, `egui_plot`, `rfd`, AccessKit, single `usvg`, worker sim thread) | [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) |
| **ADR-022** | Sequential foundation-first IDE view delivery (Wave A: Simulation+Code+Oscilloscope → Wave B: Visual Ladder+GRAFCET → Wave C: HMI/SCADA + AI BYOK) | [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) |
| **ADR-023** | Quadruple CI Gate (Unit+Proptest, 100% Real Negative Error Corpus, Multi-Board Cross-Language Golden Equivalence, SPICE `< 0.1%` Analytical Benchmarks + Asset Gate) | [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) |
| **ADR-024** | Clean `v2` Git branch archiving v1 into `legacy/v1/`, merging into `main` only once foundation phases pass 100% CI | [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) |

---

## 4. Specification Roadmap (Interview Rounds)

| Doc | Topic | Status |
|---|---|---|
| [`00-overview-and-adrs.md`](./00-overview-and-adrs.md) | Vision, Invariants, Core ADRs (ADR-001..ADR-024) | **Locked** |
| [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) | ASL v2 Type System, POUs/OBs, Pratt Expressions, Multi-Board Project Schema | **Locked** |
| [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) | Hybrid Microsecond Kernel, SPICE-Class MNA Solver, Bus Fabric & Multi-Domain Physics | **Locked** |
| [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) | 48×48 px Grid, 8 px Semantic `df-pins`, Mandatory Dual-View SVGs & Asset Validator Gate | **Locked** |
| [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) | Pure-Rust Polyglot Frontends, OBs/POUs, Full IEC Ladder/GRAFCET & Non-Destructive Sync | **Locked** |
| [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) | `egui`/`eframe` IDE Architecture, Screen Contracts, Crate DAG & Phases 0–9 Roadmap | **Locked** |
