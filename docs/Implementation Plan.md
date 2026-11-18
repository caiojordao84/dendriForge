# Implementation Plan — dendriForge

> Status: **Living document** — updated as phases are completed.  
> **ASL:** Abstract Syntax Language, structured in **TOML** (`.toml`).  
> **ASL version:** **`0.0.1`** (`metadata.asl_version`). Do not treat `0.1.0` / `0.1.1` / `v010` as current.  
> **Stack:** pure Rust. **IR:** TOML only (TOON is obsolete / legacy under `core/`).

---

## Context

| Milestone | Status |
|---|---|
| Phase 0 — ASL TOML contract freeze | **Landed** — see [`phase-0/`](./phase-0/) |
| MVP — functional MCU + PLC + peripheral sim | **Accepted** (12/12) |
| ASL version tag `0.0.1` | **Assigned** — `dendri_ast::ASL_VERSION` / `metadata.asl_version` |

| Artifact | Role |
|---|---|
| `docs/phase-0/` | Live contract (IR schema, versioning, errors, layout, SVG) |
| `crates/dendri-ast` | Rust IR types (source of truth) |
| `crates/dendri-stdlib/data/catalog.toml` | IEC stdlib catalog |
| `content/fixtures/` | Golden / error / example `.toml` |
| `docs/grammar.md`, `docs/stdlib.md` | **Superseded** semantic drafts (historical TOON surface) |
| `core/**/*.toon` | Legacy TOON corpus — read-only; see `legacy/toon/README.md` |

---

## Core Principle

> **ASL is the contract. Every module is a client of ASL — never the reverse.**

ASL documents in TOML are the single source of truth for the platform. All parsers, editors, simulators, and code generators communicate exclusively through ASL. This reduces the integration surface from N×M to N+M and makes every module independently testable.

**MVP project shape:** one TOML file per simulation project containing hardware, wiring/connections, original source scripts, and the ASL representation — so a project can be saved, shared, and reopened without external dependency loss.

---

## Architecture Layers

```
┌─────────────────────────────────────────────────────────────┐
│  EDITORS & UI                                               │
│  Ladder · GRAFCET/SFC · HMI/SCADA/IoT · Code Editor        │
│  (Ladder/SFC ↔ same TOML, bidirectional real-time sync)   │
└────────────────────────┬────────────────────────────────────┘
                         │  produces / consumes ASL (.toml)
┌────────────────────────▼────────────────────────────────────┐
│  ASL CORE  (dendri-ast · dendri-parse · dendri-validate …)  │
│  phase-0 IR · catalog.toml · validator · serializer         │
└──────┬──────────────────────────────────────┬───────────────┘
       │ parse                                │ emit
┌──────▼──────────┐                ┌──────────▼───────────────┐
│  FRONTENDS      │                │  BACKENDS                │
│  C++            │                │  C++ codegen             │
│  MicroPython    │                │  MicroPython codegen     │
│  IEC ST         │                │  IEC ST codegen          │
│  Ladder         │                │  (post-MVP: Codesys/TIA) │
│  SFC/GRAFCET    │                └──────────────────────────┘
│  Rust (P2)      │
└─────────────────┘
       │ feeds
┌──────▼──────────────────────────────────────────────────────┐
│  EXECUTION ENGINE                                           │
│  Scan-cycle scheduler · Timer/counter runtime               │
│  Signal bus · Trace/debug                                   │
└─────────────────────────────────────────────────────────────┘
```

---

## Phase 0 — ASL Specification *(done)*

**Goal:** Freeze a testable TOML IR contract before editors or full simulator land.

### Completed
- [x] ASL naming frozen as **Abstract Syntax Language**
- [x] TOML IR schema (`docs/phase-0/ir-schema.md` + `crates/dendri-ast`)
- [x] ASL **`0.0.1`** assigned after MVP (`docs/phase-0/versioning.md`)
- [x] Unified `ERR_*` error catalogue
- [x] Stdlib ported to `catalog.toml`
- [x] Ladder + GRAFCET AST stubs reserved
- [x] MVP fixtures converted (`.toml` goldens + errors)
- [x] Compliance spine `content/standards/bindings.toml`
- [x] Layout policy: new IR in `content/`; legacy TOON in `core/`
- [x] Structural fixture load tests (`cargo test -p dendri-ast --test load_fixtures`)

**Historical drafts** (`grammar.md`, `stdlib.md`) remain for semantic reference only; their TOON surface and any `0.1.0` version labels are not the live authority.

---

## MVP — Functional simulation (current)

**Goal:** Functional simulator of MCU + PLC + peripherals on ASL/TOML. ASL tag **`0.0.1`** assigned after automated acceptance (fixture round-trip + deterministic simulation) — now live.

### Hard requirements
1. **Full TOML transition** — no TOON as supported IR
2. **Single-file simulation project** — hardware, wiring, original scripts, ASL
3. **P1 languages** (parse + generate): C++, MicroPython, ST, Ladder, SFC/GRAFCET
4. **Ladder & GRAFCET** — visual editors; same TOML as textual languages; bidirectional real-time sync
5. **Rust scripting** — **P2 after MVP** (not required for `0.0.1`)
6. **Pure Rust** app stack

### Explicit non-goals for MVP / `0.0.1`
- Rust-as-scripting-language frontends
- Full industrial protocol stacks beyond what sim needs
- Parallel version schemes beyond `metadata.asl_version`

### Phase 1 core library (under MVP)

**Goal:** Parse, validate, serialize, and simulate ASL TOML.

#### Entregáveis
- `dendri-parse` — load `.toml` into `dendri-ast`
- `dendri-validate` — semantic validation (`ERR_*`)
- `dendri-emit` — lossless round-trip / target emit
- `dendri-sim` — deterministic scan-cycle execution
- CLI: `dendri validate|roundtrip|info|sim <file.toml>`

#### Criteria
- Golden fixtures load and round-trip
- Negative fixtures emit expected `ERR_*`
- Deterministic sim traces for MVP scenarios
- Zero TOON input path in Phase 1

---

## Post-MVP phases (outline)

Priorities below assume MVP acceptance and the first ASL tag **`0.0.1`**.

### Transpilation depth
| Frontend | Priority |
|---|---|
| C++, MicroPython, ST, Ladder, SFC/GRAFCET | P1 (MVP) |
| Rust (Dendri HAL subset → ASL; Embassy depth later) | P2 (DEN-24 first slice) |

| Backend | Priority |
|---|---|
| C++, MicroPython, ST | P1 (MVP) |
| Codesys / TIA Portal | P2+ |

### Execution engine
Scan scheduler, statement executor, signal bus, timer/counter engine, trace buffer — step/pause/resume/reset; deterministic seed replay.

### Visual simulation canvas
SVG canvas keyed by ASL identifiers in the TOML project; orthogonal wire routing; pin inspector; real-time state from `dendri-sim`.

### Ladder / GRAFCET editors
Mandatory I/O tables; bidirectional diagram ↔ TOML sync; reopen preserves layout and bindings.

### Industrial protocols (P2)
| Protocol | Status |
|---|---|
| Modbus TCP | **First slice (DEN-25):** virtual register map in `dendri-sim` (`CFG_MODBUS_TCP` + FC1–6 singles); profile `comm-modbus-tcp`; fixture S29. Not a wire MBAP/TCP stack. |
| Modbus RTU / MQTT / OPC UA / PROFINET | Deferred |

### HMI / SCADA / IoT (later)
Data-driven SVG bindings to ASL tags; internal sim bus first; deeper protocol stacks after Modbus TCP map.

### AI assistant (BYOK, later)
User-supplied keys / local CLI; always validate AI output through `dendri-validate`.

---

## Cross-Cutting Concerns

### Testing strategy

| Layer | Test type | Tool |
|---|---|---|
| ASL IR / fixtures | Structural + golden | Rust `cargo test` |
| Transpiler | Semantic equivalence | Compare AST / symbol tables |
| Runtime | Deterministic trace | Seed-based replay |
| Ladder / GRAFCET | Sync + FSM traces | Round-trip TOML + state sequences |
| End-to-end | Full pipeline | Project open → simulate → export |

### Definition of Done (per phase)
1. Listed criteria pass in CI
2. At least one real-world scenario works end-to-end
3. Errors are human-readable with location info
4. Module integrates with ASL TOML without adapters
5. Documentation in `docs/` stays aligned (no TOON / no stale `0.1.x` as current)

### Key risks

| Risk | Impact | Mitigation |
|---|---|---|
| Ladder/GRAFCET tokens underspecified in IR | High | Extend `dendri-ast` stubs before freezing MVP acceptance |
| Simulation diverges from real hardware | High | Conformance suite against physical boards |
| Canvas performance with many SVGs | Medium | Virtualization, incremental diff, SVG caching |
| Docs reintroduce TOON or `0.1.x` as current | Medium | Phase 0 versioning + this plan as banner authority |

---

## Immediate Next Steps

1. Implement `dendri-parse` / `dendri-validate` / `dendri-emit` / `dendri-sim` against `dendri-ast`
2. Expand golden round-trips and deterministic sim acceptance tests
3. Wire P1 frontends: C++, MicroPython, ST
4. Build Ladder and SFC/GRAFCET editors with bidirectional TOML sync
5. Keep docs TOML-only; ASL `0.0.1` is the live language tag after MVP acceptance

---

*Updated: 2026-11-15 | dendriForge platform planning*
