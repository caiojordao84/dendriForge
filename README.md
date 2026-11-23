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

## Workspace Architecture (Current Phase: Phase 2)

The active milestone is tracked in [`docs/milestone.toml`](docs/milestone.toml) and all deliberate deferrals live in [`docs/deferrals.toml`](docs/deferrals.toml). Legacy v1 files are quarantined in `legacy/v1/` strictly as historical datasheet references.

### Layer 0 — Foundations
- `crates/dendri-span` — UTF-8-safe byte spans (`Span`), `SourceMap`, rich `Diagnostic` formatting, and the frozen 45-variant `ErrorCode` enum.
- `crates/dendri-theme` — Pure-Rust **A11Y Slate** palette, 12 semantic `df-pin-*` classes, and automated WCAG 2.1 contrast verification (`AAA >= 7:1` on primary text, syntax tokens, primary buttons, and pin tokens).
- `crates/dendri-stdlib` — Domain-split standard library (`std.iec.*` vs `std.arduino.*` / `std.siemens.*`) and live validated `assets/standards.toml` registry.

### Layer 1 — ASL v2 IR, Expressions, Project Schema & Validator
- `crates/dendri-ast` — Canonical typed ASL v2 IR (`PouBody::{Statements, Network, Chart}`, fixed-width types, `M = 10⁹` numeric `Codec` & `ChannelBand`, verbatim comments, assumptions, modules/packs/3-level `Foreign`, multi-board `Project`).
- `crates/dendri-expr` — Official 11-level Pratt expression parser & lossless canonical printer (`proptest`-verified `parse(print(e)) == e`).
- `crates/dendri-project` — Strict single-file v2 `.toml` loader (`#[serde(deny_unknown_fields)]`), canonical serializer, and atomic disk persistence (`write temp -> sync -> rename`).
- `crates/dendri-validate` — Full static typechecker, codec/band/comment/module/capability/OB/ISR/wiring validator with all **45 / 45 frozen `ErrorCode` variants live** and verified by a genuine 45-file negative corpus.

### Layer 2 — Deterministic Multi-Board VM & Co-Simulation Kernel
- `crates/dendri-vm` — Cycle-accurate ASL v2 virtual machine (`fiber_cycles`/`clock_hz` in exact 128-bit arithmetic, `[execution.cost]`, pure-Rust `libm` transcendentals, ordered `BTreeMap` state, `INDETERMINATE` policy + `WARN_INDETERMINATE_LEVEL`, lexical frames, per-scan budget reset, `max_call_depth`, bounded `TraceRing`, exact IEC 61131-3 FBs `TON`/`TOF`/`TP`/`CTU`/`CTD`/`CTUD`/`R_TRIG`/`F_TRIG`/`SR`/`RS`, OBs `OB100`/`OB1`/`OB30`/`OB40`/`OB50`, load-time lowering of `PouBody::Network` & `PouBody::Chart`, cooperative `WAIT_TIME`/`WAIT_TIME_US` yield/resume).
- `crates/dendri-sim-kernel` — Deterministic microsecond (`u64 time_us`) discrete-event orchestrator supporting heterogeneous multi-board co-simulation (`ExecutionKind::Cooperative` + `ExecutionKind::PlcScan`), pluggable `ElectricalBackend` (`DigitalWireFabric` wire-net Union-Find), bit-identical `InputTimeline` replay, and dedicated background `SimWorker` thread publishing immutable `Arc<SimSnapshot>` frames.

### Layer 4 — CLI Governance, Verification & Headless Runner
- `crates/dendri-cli` (`dendri`) — Automated verification & simulation commands:
  - `dendri validate <file.toml> [--report] [--allow-foreign]` — Parse & statically validate a v2 project with rich caret snippets.
  - `dendri sim <file.toml> [--duration-ms <N>] [--seed <S>] [--allow-foreign]` — Validate and run a deterministic headless multi-board co-simulation.
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

# Governance & policy gates
cargo run -q -p dendri-cli -- ledger-check
cargo run -q -p dendri-cli -- provenance-check
cargo run -q -p dendri-cli -- standards-check
```

### Fuzzing & Mutation Testing Scaffolding
- Fuzzing targets live in [`fuzz/`](fuzz/README.md) (built with `cargo fuzz` on nightly).
- Weekly mutation testing (`cargo mutants`) and fuzz smoke checks are configured in [`.github/workflows/weekly.yml`](.github/workflows/weekly.yml).
