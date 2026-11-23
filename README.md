# dendriForge v2

**A deterministic, 100% pure-Rust embedded & industrial automation simulation platform unifying MCUs, PLCs, textual languages, visual IEC editors, and SPICE-class electrical co-simulation under ASL (Abstract Syntax Language).**

`dendriForge` is built around **ASL** in a dual role:
1. **Foundation Stone:** Single strictly-typed executable intermediate representation (IR) driving the deterministic multi-board VM, static validator, and single-file `.toml` project format.
2. **Rosetta Stone:** Lossless bidirectional bridge across embedded and industrial paradigms — Wave 1 pairs **C++ (Arduino subset)**, **IEC 61131-3 Structured Text (ST)**, **Ladder Diagram (LD)**, and **GRAFCET / SFC**, followed by **MicroPython** (Wave 2) and **Rust** (Wave 3).

---

## Normative Architecture Specifications (`docs/architecture/`)

All design decisions, formal schemas, contracts, and phased gates live in six authoritative specifications (RFC 2119 baseline, Rev 2):

| Document | Scope |
|---|---|
| [`00-overview-and-adrs.md`](docs/architecture/00-overview-and-adrs.md) | Product vision, Guiding Principles (`P1–P6`), 7 Engineering Invariants, ADR-001..ADR-030 |
| [`01-asl-ir-and-project-schema.md`](docs/architecture/01-asl-ir-and-project-schema.md) | ASL v2 type system, `M = 10⁹` numeric contract, OBs/POUs (`Network`/`Chart`), modules/packs/`Foreign`, verbatim comments, Pratt expressions & 45 frozen diagnostic codes |
| [`02-simulation-and-electrical-engine.md`](docs/architecture/02-simulation-and-electrical-engine.md) | Cycle-based microsecond kernel, SPICE-class event + transient MNA solver, physical-topology bus fabric & pluggable physics |
| [`03-asset-and-profile-contract.md`](docs/architecture/03-asset-and-profile-contract.md) | 48×48 px grid, 8 px semantic `<g id="df-pins">`, original IEC 60617-style dual-view SVGs, provenance, sanitization & `dendri asset-check` |
| [`04-frontends-visual-and-sync.md`](docs/architecture/04-frontends-visual-and-sync.md) | Pure-Rust frontend core (zero C FFI), Language Binding Contract (LBC), structural Ladder/GRAFCET, `dendri-sync` & non-normative Horizon Appendix |
| [`05-ide-and-roadmap.md`](docs/architecture/05-ide-and-roadmap.md) | `egui`/`eframe` desktop IDE, 12-Gate CI suite, DoD templates, Deferral Ledger (`docs/deferrals.toml`) & Phases 0–9 (including Phase 6b) |

---

## Workspace Architecture (Current Phase: Phase 4)

The active milestone is tracked in [`docs/milestone.toml`](docs/milestone.toml) and all deliberate deferrals live in [`docs/deferrals.toml`](docs/deferrals.toml) (`DEF-001` closed in Phase 3; `DEF-002` and `DEF-009` closed in Phase 4). Legacy v1 files are quarantined in `legacy/v1/` strictly as historical datasheet references.

### Layer 0 — Foundations
- `crates/dendri-span` — UTF-8-safe byte spans (`Span`), `SourceMap`, rich `Diagnostic` formatting, and the frozen 45-variant `ErrorCode` enum.
- `crates/dendri-theme` — Pure-Rust **A11Y Slate** palette, 12 semantic `df-pin-*` classes, and automated WCAG 2.1 contrast verification (`AAA >= 7:1` on primary text, syntax tokens, primary buttons, and pin tokens).
- `crates/dendri-stdlib` — Domain-split standard library (`std.iec.*` vs `std.arduino.*` / `std.plc.*`) and live validated `assets/standards.toml` registry.

### Layer 1 — ASL v2 IR, Expressions, Project Schema & Validator
- `crates/dendri-ast` — Canonical typed ASL v2 IR (`PouBody::{Statements, Network, Chart}`, fixed-width types, `M = 10⁹` numeric `Codec` & `ChannelBand`, verbatim comments, assumptions, modules/packs/3-level `Foreign`, multi-board `Project`).
- `crates/dendri-expr` — Official 11-level Pratt expression parser & lossless canonical printer (`proptest`-verified `parse(print(e)) == e`).
- `crates/dendri-project` — Strict single-file v2 `.toml` loader (`#[serde(deny_unknown_fields)]`), canonical serializer, and atomic disk persistence (`write temp -> sync -> rename`).
- `crates/dendri-validate` — Full static typechecker, codec/band/comment/module/capability/OB/ISR/wiring validator with all **45 / 45 frozen `ErrorCode` variants live** and verified by a genuine 45-file negative corpus.

### Layer 2 — Deterministic Multi-Board VM, Co-Simulation Kernel & SPICE-Class MNA Solver
- `crates/dendri-vm` — Cycle-accurate ASL v2 virtual machine (`fiber_cycles`/`clock_hz` in exact 128-bit arithmetic, `[execution.cost]`, pure-Rust `libm` transcendentals, ordered `BTreeMap` state, `INDETERMINATE` policy + `WARN_INDETERMINATE_LEVEL`, lexical frames, per-scan budget reset, `max_call_depth`, bounded `TraceRing`, exact IEC 61131-3 FBs `TON`/`TOF`/`TP`/`CTU`/`CTD`/`CTUD`/`R_TRIG`/`F_TRIG`/`SR`/`RS`, OBs `OB100`/`OB1`/`OB30`/`OB40`/`OB50`, load-time lowering of `PouBody::Network` & `PouBody::Chart`, cooperative `WAIT_TIME`/`WAIT_TIME_US` yield/resume).
- `crates/dendri-sim-kernel` — Deterministic microsecond (`u64 time_us`) discrete-event orchestrator supporting heterogeneous multi-board co-simulation (`ExecutionKind::Cooperative` + `ExecutionKind::PlcScan`), pluggable `ElectricalBackend`, bit-identical `InputTimeline` replay, and dedicated background `SimWorker` thread publishing immutable `Arc<SimSnapshot>` frames.
- `crates/dendri-circuit` — Headless pure-Rust SPICE-class Modified Nodal Analysis (MNA) mixed-signal electrical solver: Union-Find netlist & connected-component island partitioning, LU solver with partial pivoting, Newton-Raphson with `pnjlim` + `Gmin`/source stepping, Tier 1 (linear DC, potentiometers, NO/NC switches, inductive flyback relays, Shockley diodes/LEDs/Zeners, MCU/PLC Thevenin & Schmitt boundaries, round-half-even Solver→ASL boundary `T-NUM-5`, electrical faults), Tier 2 (event-driven DC + Backward Euler / Trapezoidal transient `C`/`L`/coupled transformers, purely resistive 1φ & 3φ AC, verified AVR PWM transfer functions closing `DEF-001`, virtual probes), Tier 3 (Ebers-Moll BJT, Shichman-Hodges MOSFET + body diode, galvanically isolated 2-island Optocoupler, IGBT, SCR, TRIAC, SSR, H-Bridge, back-EMF DC motor), and the end-to-end Phase 3 Tracer Bullet (`Uno + button + LED`).

### Layer 3 — Certified Asset & Profile Contract + Core Kit (`assets/`)
- `crates/dendri-assets` — Strict `#[serde(deny_unknown_fields)]` schemas for component/board `profile.toml`, declarative composite sub-circuits, `review.toml` sign-offs, IEC 81346-2 reference designations (`DEF-009`), unified 2D transform matrix $\mathbf{M}$, canonical SVG sanitizer & `48×48 px` / `8 px` `<g id="df-pins">` parser, deterministic headless `usvg`/`resvg`/`tiny-skia` render SHA-256 hashing (single workspace `usvg` version), 13-rule validator (`validate_assets_dir`), explicit human sign-off helper (`approve_asset_dir`), and live `AssetCatalog` implementing `dendri-validate::ProfileCatalog`.
- `assets/boards/` & `assets/components/` — Certified 10-asset Phase 4 Core Kit (100% original dual-view `physical.svg` + IEC 60617-style / IEC 61082 `schematic.svg` + verified `profile.toml` + canonical `review.toml`):
  - **Boards (2):** `arduino-uno-r3`, `plc-1200-open-1214` (vendor-neutral 24 V DC open PLC in A11Y Slate with verified `DEF-002` 27,648-count analog band tables).
  - **Mirrored Components (8):** `maker-tactile-4pin`, `cmd-pushbutton-no-start`, `cmd-pushbutton-nc-stop`, `led`, `signal-pilot-lamp` (24 V DC declarative composite sub-circuit), `potentiometer`, `sensor-pressure-4-20ma`, `resistor`.

### Layer 4 — CLI Governance, Verification & Headless Runner
- `crates/dendri-cli` (`dendri`) — Automated verification & simulation commands:
  - `dendri asset-check` — Enforces all 13 normative rules across `assets/{boards,components}` (TOML schema, `df-pins` bijection & grid snap, zero collisions, SVG sanitization, composite sub-circuits, `standards.toml`, provenance, and live `usvg`/`resvg` `review.toml` render hashes).
  - `dendri asset-approve <dir> --reviewer <name>` — Explicitly signs off an inspected asset directory, writing canonical render SHA-256 hashes to `review.toml`.
  - `dendri validate <file.toml> [--report] [--allow-foreign]` — Parse & statically validate a v2 project against the live certified `AssetCatalog`.
  - `dendri sim <file.toml> [--spice] [--duration-ms <N>] [--seed <S>] [--allow-foreign]` — Validate and run a deterministic headless multi-board co-simulation (digital wire fabric or full SPICE MNA).
  - `dendri ledger-check` — Enforces `docs/deferrals.toml` against `docs/milestone.toml` and forbids unreferenced stubs/markers in workspace crates.
  - `dendri provenance-check` — Enforces strict `[provenance]` metadata, `THIRD_PARTY_NOTICES` parity, and `ml_restricted` exclusion rules.
  - `dendri standards-check` — Loads and validates `assets/standards.toml`.

---

## Development & Verification Commands

Toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml) (`rustc 1.100.0` with `rustfmt` and `clippy`).

```bash
# Formatting & strict workspace lints (zero warnings, zero production panics/unwraps)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Unit, property tests & 45-code real negative corpus
cargo test --workspace

# Exhaustive release-mode numeric contract gate (T-NUM-1..5 incl. 2^24 codes)
cargo test --release -p dendri-ast numeric::tests

# Governance, asset contract & policy gates
cargo run -q -p dendri-cli -- asset-check
cargo run -q -p dendri-cli -- ledger-check
cargo run -q -p dendri-cli -- provenance-check
cargo run -q -p dendri-cli -- standards-check
```

### Fuzzing & Mutation Testing Scaffolding
- Fuzzing targets live in [`fuzz/`](fuzz/README.md) (built with `cargo fuzz` on nightly).
- Weekly mutation testing (`cargo mutants`) and fuzz smoke checks are configured in [`.github/workflows/weekly.yml`](.github/workflows/weekly.yml).
