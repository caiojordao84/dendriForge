# 00 — dendriForge v2 Architectural Overview & Decision Log

> **Status:** Active Design Phase — Normative Baseline (Rev 2, post-Round 7 review)  
> **Language:** Technical English (normative specification)  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Scope:** Complete clean-slate v2 architecture for `dendriForge` — a deterministic, pure-Rust embedded & industrial automation simulation platform unifying MCUs, PLCs, visual IEC languages, textual languages, and interactive hardware schematics under **ASL (Abstract Syntax Language)**.  
> **Revision Policy:** Documents `00`–`05` form the single normative baseline (no separate override amendment layers) and are revised by explicit ADR updates at the end of each phase (consistent with ADR-002).

---

## 1. Product Vision & Guiding Principles

`dendriForge` aims to be a full-grade alternative to interactive hardware simulators (such as Wokwi and Tinkercad) while adding **industrial automation standards compliance** (IEC 61131-3, IEC 60848, IEC 60947, ISO 1219, IEC 60617-style / IEC 61082) built around **ASL** as a dual-purpose core:

1. **Foundation Stone:** ASL is the canonical, strongly-typed intermediate representation (IR) executed by the deterministic simulation engine, validated by the static analyzer, and serialized inside single-file `.toml` projects.
2. **Rosetta Stone:** ASL bridges traditional embedded programming (C++/Arduino, and later MicroPython and Rust) and industrial automation programming (IEC 61131-3 Structured Text, Ladder Diagram, GRAFCET/SFC). A software developer can transition smoothly into PLC/industrial engineering, and an automation engineer can program an MCU in Ladder or GRAFCET seamlessly within the same environment.

### Guiding Engineering Principles (P1–P6)
- **P1 — Polish by construction:** No stubs, no workarounds, no forgotten placeholders. Enforced mechanically via the CI Deferral Ledger (`docs/deferrals.toml`, [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md#5-definition-of-done-and-the-deferral-ledger)): a deferral is a dated, owned ledger entry or it MUST NOT exist.
- **P2 — ASL is the bedrock:** ASL is the single executable truth and the Rosetta Stone. Every frontend is subordinate to ASL and bound by a formal Language Binding Contract (LBC, [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md#2-language-binding-contract-lbc)). A frontend MUST NOT define semantics absent from ASL and MUST NOT silently drop or guess constructs.
- **P3 — No shortcuts in assets:** Every certified component ships a strict TOML profile, a physical SVG, and an original IEC 60617-style schematic SVG, validated mechanically by `dendri asset-check` and reviewed individually with a signed `review.toml`.
- **P4 — Independent oracles:** v1 fixtures MUST NOT be reused; only their behavioral scenario descriptions carry over. Expected values MUST be derived independently from datasheets and hand/analytical calculations — never from simulator output.
- **P5 — Early real-toolchain verification:** Emitted code is compiled and executed against real external toolchains in a dedicated differential gate (**Phase 6b**, immediately after Wave 1 frontends and **before** the desktop IDE), not deferred to the end of the project.
- **P6 — Comments are first-class data:** Human comments are mandatory, verbatim data anchored in their own validated ASL tables — never stripped decoration.

### Core Platform Capabilities (Target Architecture)
- **Multi-board, heterogeneous single-file `.toml` projects:** Multiple MCUs and PLCs co-simulated simultaneously on the same canvas, communicating electrically and over wired buses/protocols, with each board independently authored in the user's chosen language.
- **Seamless multi-view IDE (`egui`/`eframe`):** Instant, non-destructive switching across Simulation Canvas (Physical 48×48 px + Schematic), Code, Ladder, GRAFCET/SFC, Oscilloscope, and future HMI/SCADA/IoT + AI BYOK views backed by one authoritative per-board AST.
- **Strict Asset Contract:** Rigid TOML schemas paired 1:1 with SVG `<g id="df-pins">` 8 px semantic pin markers, provenance metadata, and verified SPICE/multi-domain physics models.

---

## 2. Non-Negotiable Engineering Invariants (Quality Gates)

To prevent every structural failure mode identified in the v1 audit, every v2 crate MUST satisfy these hard invariants from commit one:

1. **Strict TDD, Mutation & Fuzz Verification:** Every crate ships with unit + property (`proptest`) + negative tests before advancing phases. Weekly `cargo-mutants` runs catch tautological tests; `cargo-fuzz` targets guard every parser.
2. **Zero Panics & Exhaustive Domain Matching in Production Code:** Workspace Clippy lints set `unwrap_used`, `expect_used`, `indexing_slicing`, `panic`, `todo`, `unimplemented`, `wildcard_enum_match_arm` (domain crates), and `iter_over_hash_type` (determinism-critical crates) to `deny`, with test-only carve-outs in `clippy.toml` (`allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-indexing-slicing-in-tests`, `allow-panic-in-tests`). Every failure returns a structured `Result` diagnostic with byte spans.
3. **Strict Schema Enforcement:** Every TOML schema (ASL IR, projects, profiles, stdlib, packs, deferrals, milestones) uses strongly-typed structs with `#[serde(deny_unknown_fields)]` — zero silent field drops and zero untyped `rest: Map` dumping grounds.
4. **No Machine Magic-Comment Cheating (Human Comments First-Class):** Machine-emitted round-trip cheat comments (`// ASL_HW:`, `// ASL_DATA:`, `(* HW: ... *)`) are permanently banned. Conversely, human comments are mandatory, verbatim first-class data stored in dedicated ASL comment tables ([`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md#7-comments--first-class-verbatim-data-adr-027)).
5. **Single Source of Truth:** Never duplicate executable logic across parallel representations (`loop`, `ladder`, `grafcet`, `sources`) that can silently drift or clobber each other. Ladder (`Network`) and GRAFCET (`Chart`) are native structural POU body kinds in ASL.
6. **Zero Compile-Time Machine Paths:** Binaries (`dendri`, `dendri-editor`) MUST resolve assets relative to runtime flags / env vars / executable path — never `env!("CARGO_MANIFEST_DIR")`.
7. **Full 12-Gate CI Enforcement:** Formatting, Clippy lints, unit+proptest, real negative error corpus (1 fixture per `ErrorCode`), `dendri asset-check` + provenance, SPICE `< 0.1%` analytical benchmarks, numeric `T-NUM-1..5`, comment conservation `C1–C5`, multi-board cross-language golden equivalence, Phase 6b real-toolchain differential gate, fuzz/mutation jobs, and the Deferral Ledger gate MUST all pass with zero warnings ([`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md#4-twelve-gate-ci-verification-suite)).

---

## 3. Architecture Decision Records (ADRs)

### Core Foundation ADRs

#### ADR-001: Clean-Slate v2 Rebuild with Strict TDD
- **Status:** Accepted
- **Context:** The v1 codebase (~36k LOC across 18 crates) accumulated pervasive structural debt: corrupted TOML profiles (154/155 `[canvas]`, 29/155 `[limits]`), corrupted golden fixtures locked by byte-exact tests, 1,641 LOC of legacy TOON regex migrator inside `dendri-ast`, 83% copy-pasted parsers, non-functioning `break`/`continue`, stack-overflow crashes, unbounded memory/budget leaks in the simulator, hardcoded 1-hop electrical shortcuts, and hardcoded C01 visual diagrams in the editor.
- **Decision:** Archive the existing v1 workspace into `legacy/v1/` on branch `v2` and rebuild `dendriForge` v2 crate-by-crate from scratch under strict TDD. v1 fixtures are never reused (only scenario descriptions carry over, per P4); legacy 215 profiles stay in `legacy/v1/` strictly as engineering datasheet references.

#### ADR-002: Living Technical Specifications in `docs/architecture/`
- **Status:** Accepted
- **Context:** v1 suffered severe drift between `README.md`, `docs/`, and actual crate behavior.
- **Decision:** All architectural decisions, formal schemas, contracts, and phased plans are maintained in technical English under `docs/architecture/00..05` as the single authoritative baseline, revised via explicit ADR updates at the end of each phase.

#### ADR-003: Staged "Rosetta Stone" Language Rollout & Robotics Horizon
- **Status:** Accepted (Amended in Round 7)
- **Decision:**
  - **Wave 1 (Rosetta Stone Core):** `ASL` + `C++ (Arduino subset)` + `IEC 61131-3 ST` + `Ladder (LD)` + `GRAFCET (SFC)`. Exits only when **Phase 6b** (real-toolchain differential gate) is 100% green.
  - **Wave 2:** `MicroPython` (gated on `asl-oop` design, `DEF-004`, and Wave 1 exit).
  - **Wave 3:** `Rust` (Dendri HAL subset, gated on Wave 2 exit).
  - **Robotics Horizon:** Vendor robot programming languages (collectively **robotic languages**) are NOT simulated in Waves 1–3, but ASL MUST NOT foreclose them: capabilities negotiate via versioned `requires = ["asl-core@1", ...]` (`ERR_UNSUPPORTED_CAPABILITY` on unknown capability), and `ExecutionKind` is a non-exhaustive enum ready for a future `asl-motion` task-based capability (`DEF-005`).

#### ADR-004: Early Real-Toolchain Differential Verification (Phase 6b)
- **Status:** Accepted (Amended in Round 7 — promoted from Phase 9 to Phase 6b)
- **Context:** Waiting until Phase 9 to verify generated code against real compilers risks letting emitters drift into uncompilable output before the IDE is even built.
- **Decision:** Introduce a dedicated **Phase 6b** immediately after Wave 1 frontends (and before IDE Phases 7–8) that compiles and executes emitted C++ (host deterministic Arduino shim + nightly `arduino-cli`) and emitted ST (open IEC 61131-3 compiler + I/O shim; candidates `matiec` / RuSTy evaluated in `DEF-003`), comparing traces directly against the ASL VM and exercising the hostile comment corpus (`C3`). MicroPython and Rust differential gates open with their respective waves.

### Complete ADR Index (ADR-005 through ADR-030)

| ADR | Decision Summary | Normative Spec |
|---|---|---|
| **ADR-005** | Full IEC 61131-3 + Embedded Core static type system (fixed-width scalars, `ANALOG`/`DUTY`/`CODE`, fixed arrays, flat structs, enums, first-class FBs, typed functions; zero pointers/heap) | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-006** | Single-file multi-board `.toml` project (`[[boards]]`) with per-board `authoring_language`, ASL as sole executable truth, validated ID grammar (`instance_id` no dots; split endpoints on first dot), and NodeId-anchored presentation metadata | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-007** | Single canonical typed AST in memory (zero `ExprText` / duplicate `Expr::Lit`) with formal Pratt expression grammar + canonical printer | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-008** | Unified cooperative execution model (`WAIT_TIME`/`WAIT_TIME_US` yield only that board's fiber while PLCs, ISRs, timers, and LEDs continue live) | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-009** | Full SPICE-class MNA transient & nonlinear analog solver (`dendri-circuit`) from day one (R/C/L/AC 1φ&3φ/Diodes/LED/BJT/MOSFET/Power) with headless Phase 3 analytical benchmarks | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-010** | Hybrid discrete-event + fixed-tick deterministic microsecond kernel (`u64 time_us`) running on a dedicated worker thread | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-011** | Physical-topology virtual bus fabric (UART, I2C, SPI with CS, Virtual Ethernet / Modbus TCP) wired strictly to canvas nets | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-012** | Decoupled multi-domain `PhysicalComponent` trait outside CPU VM (Electrical first, then Pneumatic/Hydraulic/Thermal/Displays) | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-013** | Declarative TOML `[svg.bindings]` ↔ SVG IDs (visibility/paint/translate/rotate/overlay rects); zero CSS `var()`, SMIL, `<script>`, or XML string slicing | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |
| **ADR-014** | Canonical 48×48 px grid module + `<g id="df-pins">` 8 px semantic pin markers validated 1:1 against TOML and painted by `theme.rs` (A11Y Slate) | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |
| **ADR-015** | Mandatory dual-view SVGs (`physical.svg` + original IEC 60617-style `schematic.svg` with IEC 61082 conventions) with 1:1 port parity for every component | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |
| **ADR-016** | Certified Catalog Policy: zero profiles without validated SVGs + physics model + provenance + `review.toml` sign-off; Phase 4 narrowed Core Kit (Uno R3 + S7-1200 + 8 components; ESP32/Pico in Wave 2; Phase 5 batch-certifies bus/display/pneumatic parts) | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |
| **ADR-017** | 100% Pure-Rust Polyglot Frontend Framework (`dendri-frontend-core`, zero C `tree-sitter` FFI) + mandatory Language Binding Contract (LBC). Long-tail languages removed from normative scope to non-normative Horizon Appendix | [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) |
| **ADR-018** | IEC 61131-3 / TIA Portal Organization Blocks (`OB_STARTUP`, `OB_MAIN`, `OB_CYCLIC_INT`, `OB_HW_INT`, `OB_COMM_EVENT`) & Multi-POU (`FC`/`FB`) hierarchy for lossless MCU↔PLC bridging | [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) |
| **ADR-019** | Full industrial IEC 61131-3 Ladder (`EN`/`ENO`/`Q`, P/N edges, Compare/Math/Move) & IEC 60848 GRAFCET (OR priority, Rule 5 two-phase, `N/S/R/L/D/P`, `.X`/`.T`, macro-steps) | [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) |
| **ADR-020** | Project-level Board I/O Tag Table with idiomatic language bindings (`const`/`#define`/`pinMode`, `AT %IX/%QX` via `iec_address`, visual tag picker) — zero machine `ASL_HW` comments | [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) |
| **ADR-021** | Pure-Rust `egui`/`eframe` desktop IDE (`egui_dock`, `egui_plot`, `rfd`, AccessKit, single `usvg`, worker sim thread) | [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) |
| **ADR-022** | Sequential foundation-first IDE view delivery (Wave A: Simulation+Code+Oscilloscope → Wave B: Visual Ladder+GRAFCET → Wave C: HMI/SCADA + AI BYOK) | [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) |
| **ADR-023** | Twelve-gate CI verification suite (hygiene, panic/wildcard/determinism lints, unit+proptest, real negative corpus, asset+provenance, SPICE `< 0.1%`, numeric `T-NUM-*`, comments `C1–C5`, golden equivalence, Phase 6b differential, fuzz/mutants, deferral ledger) + independent oracles | [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) |
| **ADR-024** | Clean `v2` Git branch archiving v1 into `legacy/v1/`, merging into `main` only once foundation phases pass 100% CI | [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) |
| **ADR-025** | ASL Fixed-Point Numeric Contract (`M = 10⁹`, `ANALOG` signed `i32`, `DUTY` unsigned `u32 [0, M]`, `CODE`, `binned`/`span`/`span_bipolar`, Lemmas 1–2 with mandatory `span → binned` clamp & `lcm(D1,D2) ≤ M`, signed band layer & status channel) | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-026** | Ladder (`Network`) & GRAFCET (`Chart`) as first-class structural POU body kinds in ASL (lowered by VM at load time; zero stored synthetic `__rail_N` / `__sfc_*` temporaries) | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-027** | First-class verbatim human comments (`[[boards.comments]]` + top-level `[[comments]]`, tagged anchors, `inner` relocation, orphan preservation, re-import idempotence, bidi defense, display-only `ERR_STRING_CHARSET`) | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-028** | Modules, domain-split stdlib (`std.iec.*` vs platform modules), versioned capability negotiation (`requires`), hash-locked Packs & 3-level `Foreign` (`Decl`/`Stmt`/`Expr`, `F0`/`F1`/`F2` + explicit `[[boards.assumptions]]`, replacing `VendorExtension`) | [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) |
| **ADR-029** | Cycle-based virtual clock (`fiber_cycles` + `clock_hz` in 128-bit arithmetic), event-driven + time-varying source transient stepping (`steps_per_period = 200`), round-half-even solver→ASL boundary, `libm` & `clippy::iter_over_hash_type` determinism | [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) |
| **ADR-030** | Original IEC 60617-style symbology with IEC 61082 conventions, source-neutral `[provenance]` (ML-restriction enforcement), canonical SVG sanitization, composite component sub-circuits & Deferral Ledger (`docs/deferrals.toml` + `docs/milestone.toml`) | [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) |

---

## 4. Specification Index

| Doc | Topic | Status |
|---|---|---|
| [`00-overview-and-adrs.md`](./00-overview-and-adrs.md) | Vision, Principles (P1–P6), Invariants, Core ADRs (ADR-001..ADR-030) | Baseline (Rev 2) |
| [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md) | ASL v2 Type System, Numeric Contract (`M = 10⁹`), OBs/POUs (`Network`/`Chart`), Modules/Packs/`Foreign`, Comments, Pratt Expressions & Error Codes | Baseline (Rev 2) |
| [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md) | Cycle-Based Microsecond Kernel, SPICE-Class Event/Transient MNA Solver, Bus Fabric & Multi-Domain Physics | Baseline (Rev 2) |
| [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md) | 48×48 px Grid, 8 px Semantic `df-pins`, Original IEC 60617-Style Dual-View SVGs, Provenance, Sanitization & Asset Gate | Baseline (Rev 2) |
| [`04-frontends-visual-and-sync.md`](./04-frontends-visual-and-sync.md) | Pure-Rust Polyglot Frontends, LBC, OBs/POUs, Structural Ladder/GRAFCET, Comments/Foreign Sync & Horizon Appendix | Baseline (Rev 2) |
| [`05-ide-and-roadmap.md`](./05-ide-and-roadmap.md) | `egui`/`eframe` IDE Architecture, 12-Gate CI Suite, DoD Templates, Deferral Ledger & Phases 0–9 (incl. Phase 6b) Roadmap | Baseline (Rev 2) |
