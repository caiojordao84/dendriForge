# Implementation Plan — dendriForge

> Status: **Living document** — updated as phases are completed.
> ASL version tracked: `0.1.0`

---

## Context

The Fase 0 specification is underway. The `docs/` folder already contains:

| File | Status | Description |
|---|---|---|
| `grammar.md` | ✅ Draft complete | TOON EBNF grammar, alias table, parse error catalogue, symbol table |
| `stdlib.md` | ✅ Draft complete | Full stdlib dictionary with IEC 61131-3 names, C++ and MicroPython codegen mappings, trigger rules |
| `README.md` | ✅ Draft complete | Architecture overview, target platforms, key design decisions |

The implementation plan below picks up from this baseline and sequences all remaining work across the platform.

---

## Core Principle

> **ASL is the contract. Every module is a client of ASL — never the reverse.**

The TOON intermediate representation defined in `grammar.md` and `stdlib.md` is the single source of truth for the entire platform. All parsers, editors, simulators, and code generators are decoupled from each other and communicate exclusively through ASL documents. This reduces the integration surface from N×M to N+M and makes every module independently testable.

---

## Architecture Layers

```
┌─────────────────────────────────────────────────────────────┐
│  EDITORS & UI                                               │
│  Ladder · GRAFCET · HMI/SCADA/IoT · Code Editor            │
└────────────────────────┬────────────────────────────────────┘
                         │  produces / consumes ASL
┌────────────────────────▼────────────────────────────────────┐
│  ASL CORE  (dendriForge/asl)                                │
│  grammar.md · stdlib.md · validator · serializer            │
└──────┬──────────────────────────────────────┬───────────────┘
       │ parse                                │ emit
┌──────▼──────────┐                ┌──────────▼───────────────┐
│  FRONTENDS      │                │  BACKENDS                │
│  C++ parser     │                │  C++ codegen             │
│  MicroPython    │                │  MicroPython codegen     │
│  IEC ST parser  │                │  IEC ST codegen          │
│  Ladder parser  │                │  Codesys / TIA Portal    │
│  GRAFCET parser │                └──────────────────────────┘
└─────────────────┘
       │ feeds
┌──────▼──────────────────────────────────────────────────────┐
│  EXECUTION ENGINE                                           │
│  Scan-cycle scheduler · Timer/counter runtime               │
│  Signal bus · Trace/debug                                   │
└─────────────────────────────────────────────────────────────┘
```

---

## Fase 0 — ASL Specification *(in progress)*

**Goal:** Produce a frozen, testable v0.1.0 contract before any editor or simulator is built.

### Completed
- [x] TOON EBNF grammar (`grammar.md`)
- [x] Stdlib dictionary with IEC 61131-3 canonical names (`stdlib.md`)
- [x] Alias normalization table (Pass 0)
- [x] Parse error catalogue (15 error codes)
- [x] Symbol table schema
- [x] Target platform registry (`arduino-avr`, `arduino-esp32`, `micropython-esp32`, `micropython-rp2`, `bare-metal-stm32`, `codesys`, `tia-portal`)

### Remaining — Fase 0 exit criteria
- [ ] **Golden test suite** — minimum 20 reference TOON documents covering: GPIO, PWM, ADC, timers, counters, interrupts, serial, I2C, SPI, RETAIN/PERSISTENT, VENDOR_EXTENSION fallback, nested loops, user functions, and all 15 parse errors
- [ ] **Round-trip proof** — each golden document must survive: parse → serialize → parse → assert identical symbol table
- [ ] **Ladder token spec** — extend `grammar.md` with `ladder_block` EBNF: rung array, CONTACT_NO/NC, COIL, COIL_S/COIL_R, FB_TIMER (TON/TOF/TP), FB_COUNTER (CTU/CTD), FB_COMPARE
- [ ] **GRAFCET token spec** — extend `grammar.md` with `grafcet_block` EBNF: step, transition, action, initial_step, divergence, convergence
- [ ] **HMI binding spec** — define `bindings_block` EBNF: tag_id → ASL variable, update_rate, direction (read/write/bidirectional)
- [ ] **Version policy** — document breaking vs. non-breaking change rules for ASL schema versioning
- [ ] **`ASLversion` bump to `0.1.1`** after Ladder and GRAFCET blocks are merged

**Fase 0 is done when:** all golden tests pass, round-trips are clean, Ladder and GRAFCET blocks are in `grammar.md`, and the spec has been reviewed against at least one real C++ program and one real Ladder rung.

---

## Fase 1 — ASL Core Library

**Goal:** A standalone, dependency-free library that parses, validates, and serializes TOON documents.

### Entregáveis
- `asl/parser` — tokenizer + recursive descent parser following the EBNF grammar
- `asl/validator` — semantic validation: count checks (PARSE_ERROR_008–012), type checks (PARSE_ERROR_002), TIME_DIFF enforcement (PARSE_ERROR_003), ISR purity check (PARSE_ERROR_015)
- `asl/serializer` — lossless round-trip serialization
- `asl/symbol_table` — build and expose the symbol table defined in `grammar.md`
- `asl/error` — structured error type with code, line, column, message

### CLI tool
```
dendri validate <file.toon>       # parse + validate, print errors
dendri roundtrip <file.toon>      # parse → serialize → parse, assert equal
dendri info <file.toon>           # print symbol table summary
```

### Criteria
- All 20+ golden tests pass
- Parser rejects all 15 error conditions with correct codes
- Round-trip is lossless for all golden documents
- Zero external runtime dependencies

### Stack recommendation
- **Rust** — zero-overhead, no GC, compiles to WASM for browser use, strong type system ideal for AST work
- Expose via `wasm-pack` for the frontend and as a native binary for CLI

---

## Fase 2 — Transpilation Engine (MVP)

**Goal:** Parse real source code into ASL, and emit real target code from ASL.

### Frontends (parse → ASL)

| Frontend | Input | Priority |
|---|---|---|
| `frontend-st` | IEC 61131-3 Structured Text | P0 |
| `frontend-ladder` | Internal Ladder JSON/TOON | P0 |
| `frontend-cpp` | Subset of C++ (Arduino style) | P1 |
| `frontend-upython` | Subset of MicroPython | P1 |
| `frontend-rust` | Subset of Rust (Embassy/HAL) | P2 |

### Backends (ASL → emit)

| Backend | Output | Priority |
|---|---|---|
| `backend-cpp` | Arduino C++ | P0 |
| `backend-st` | IEC 61131-3 ST | P0 |
| `backend-upython` | MicroPython | P1 |
| `backend-codesys` | Codesys ST | P2 |
| `backend-tia` | TIA Portal SCL | P2 |

### Criteria
- `frontend-st` → ASL round-trip correct for: assignment, IF/ELSIF/ELSE, WHILE, FOR, user function call, TON, CTU, SET_BOOL, GET_BOOL, SERIAL_PRINTLN
- `backend-cpp` emits compilable Arduino `.ino` from a golden ASL document
- Error messages include source line and column

---

## Fase 3 — Execution Engine (Runtime)

**Goal:** Execute ASL logic in a deterministic, scannable runtime — no CPU emulation.

### Architecture
```
ASL document
     │
     ▼
┌──────────────────────────────────┐
│  Program Loader                  │  builds runtime state from symbol table
├──────────────────────────────────┤
│  Scan Scheduler                  │  tick-driven, configurable cycle time
├──────────────────────────────────┤
│  Statement Executor              │  walks loop[] and functions[] AST nodes
├──────────────────────────────────┤
│  Signal Bus                      │  holds current value of all data[] vars
├──────────────────────────────────┤
│  Timer/Counter Engine            │  TON, TOF, TP, CTU, CTD — tick-accurate
├──────────────────────────────────┤
│  Trace Buffer                    │  ring buffer of (tick, var, value) events
└──────────────────────────────────┘
```

### Capabilities
- Step, pause, resume, reset
- Inject external inputs (simulate button press, sensor reading)
- Deterministic: same seed + same input sequence = same trace
- Export trace as JSON for debugging and test assertions

### Criteria
- TON timer counts correctly under injected tick stream
- CTU counter increments on rising edge of BOOL input
- Nested IF/WHILE executes correctly
- RETAIN variables survive reset (re-loaded from snapshot)
- Trace buffer exportable and diff-able

---

## Fase 4 — Visual Simulation Canvas

**Goal:** Interactive SVG canvas where components visualize the live runtime state.

### Component model
```
Component (static)   → single SVG file, no animation
Component (dynamic)  → multiple SVG layers, keyed by shared TOON identifier
Wire                 → orthogonal routing, strict 90° angles (A* or Lee's algorithm on sparse grid)
Pin                  → maps to hardware[] entry via label constant
```

### Entregáveis
- Pan/zoom canvas
- Drag-and-drop component placement
- Orthogonal auto-router for wires
- Component library (LED, button, potentiometer, buzzer, display, DHT sensor — minimum set)
- Pin inspector (live value, type, io_mode)
- Real-time state propagation from execution engine to SVG properties

### Criteria
- LED SVG toggles color on SET_BOOL(pin, TRUE)
- Button click injects GET_BOOL=TRUE into the runtime
- Wires re-route on component move without overlap

---

## Fase 5 — Ladder Editor

**Goal:** Visual editor for IEC 61131-3 Ladder logic, natively connected to ASL.

### Data model
```
ladder_document ::= {
  io_table: [ { name, type, address, description } ],   // mandatory before simulate
  rungs:    [ rung ]                                    // array — each rung is independent
}

rung ::= {
  id:      uint,
  comment: string?,
  cells:   cell[][]          // 2D grid [row][col]
}

cell ::= CONTACT_NO | CONTACT_NC | COIL | COIL_S | COIL_R
       | FB_TIMER | FB_COUNTER | FB_COMPARE | WIRE | EMPTY
```

### Scan cycle (virtual)
Each rung is evaluated left-to-right. Power flow state (`TRUE`/`FALSE`) propagates through cells. Result written to output variable on the signal bus. Evaluation is triggered on every runtime tick.

### Criteria
- Add/delete/move rungs
- Drag elements from palette onto cells
- I/O table must be filled before "Simulate" is enabled
- TON block counts correctly in simulation
- Ladder converts to valid ASL (`ladder_block`) without data loss
- Reopen saved project: rung layout and I/O table preserved

---

## Fase 6 — GRAFCET Editor

**Goal:** Visual sequential logic editor conforming to IEC 60848, integrated with ASL.

### Data model
```
grafcet_document ::= {
  io_table:    [ { name, type, address } ],   // mandatory
  steps:       [ { id, label, initial, actions[] } ],
  transitions: [ { id, from_step, to_step, condition } ],
  divergences: [ { type: "AND"|"OR", from, to[] } ]
}
```

### Simulation model
GRAFCET executes as a token-passing finite state machine (Petri net semantics):
- One or more steps are active at any tick
- A transition fires when its condition is TRUE and all predecessor steps are active
- Actions execute while their step is active

### Criteria
- Linear sequence (S0 → S1 → S2 → S0) executes correctly
- AND divergence activates two parallel branches simultaneously
- I/O table validation blocks simulation until all variables are mapped
- Converts to ASL `grafcet_block` without loss
- Active step is visually highlighted in real time

---

## Fase 7 — HMI / SCADA / IoT Editor

**Goal:** Data-driven interface builder, binding SVG elements to live ASL tags.

### Binding model
```
binding ::= {
  element_id:   string,       // SVG element id
  property:     string,       // fill | stroke | text | visibility | transform
  tag:          string,       // ASL data[] variable name
  transform:    expr?,        // optional: scale, threshold, map_range
  direction:    read | write | bidirectional,
  update_ms:    uint          // refresh interval
}
```

### Built-in widgets (MVP)
- Indicator lamp (BOOL → color)
- Numeric display (INT/REAL → text)
- Push button (click → BOOL write)
- Gauge (REAL → SVG arc rotation)
- Trend chart (time series, last N ticks)
- Text label (STRING tag → text content)

### Protocol connectors (MVP)
- Internal simulation bus (zero-config)
- MQTT (broker URL, topic map)
- Modbus TCP (IP, unit ID, register map)

### Criteria
- Lamp widget changes color when linked BOOL tag changes
- Button widget injects write event into runtime
- Same panel works in simulation mode and live MQTT mode
- Upload custom SVG and bind properties via UI

---

## Fase 8 — AI Assistant (BYOK + Local CLI)

**Goal:** On-demand engineering copilot, controllable by the user, operating within ASL rules.

### Modes
| Mode | Description |
|---|---|
| BYOK | User provides API key (OpenAI, Anthropic, Gemini, etc.) |
| Local CLI | Aggregator already installed on system (Ollama, LM Studio, etc.) |

### Ruleset (markdown training)
The user provides a `.md` file that defines:
- Vocabulary: ASL token names, stdlib functions, platform targets
- Generation rules: what the AI is allowed to produce (ASL snippets, refactored logic)
- Prohibited patterns: raw pin integers, missing hardware[] declarations, TIME_DIFF violations
- Style: IEC canonical names, 2-space indent, explicit types

### Workflow
1. User selects context (open ASL document + relevant block)
2. User types natural-language request
3. AI receives: ruleset + context + request
4. AI responds with: explanation + proposed ASL diff or code
5. User reviews and applies (never auto-applied)
6. All interactions logged to `ai_audit.log` in project folder

### Criteria
- AI respects PARSE_ERROR_003 rule (never suggests raw time subtraction)
- AI uses IEC canonical names, not aliases
- User can swap provider without changing workflow
- No API key is ever logged or transmitted outside the configured provider

---

## Cross-Cutting Concerns

### Testing strategy

| Layer | Test type | Tool |
|---|---|---|
| ASL parser | Unit + golden files | Rust `cargo test` |
| Transpiler | Semantic equivalence | Compare symbol tables |
| Runtime | Deterministic trace | Seed-based replay |
| Ladder editor | Visual regression | Snapshot tests |
| GRAFCET | FSM trace | State sequence assertions |
| HMI binding | Integration | Simulated tag stream |
| End-to-end | Full pipeline | Project open → simulate → export |

### Definition of Done (per phase)
A phase is closed when:
1. All listed criteria pass in CI
2. At least one real-world usage scenario works end-to-end
3. Errors are human-readable with location info
4. The module integrates with ASL without adapters or workarounds
5. Documentation is updated in `docs/`

### Key risks

| Risk | Impact | Mitigation |
|---|---|---|
| ASL grammar too narrow for Ladder/GRAFCET tokens | High | Prototype Ladder and GRAFCET blocks before freezing v0.1.1 |
| Simulation diverges from real hardware behavior | High | Define conformance test suite against physical boards |
| Canvas performance with many SVG elements | Medium | Virtualization, incremental diff, SVG caching |
| Ladder editor complexity underestimated | Medium | Ship minimal rung subset first; add parallel branches in a later iteration |
| AI generates non-conformant ASL | Medium | Always validate AI output through `asl/validator` before applying |

---

## Immediate Next Steps

1. Write the 20 golden TOON test documents (one per stdlib category)
2. Add `ladder_block` and `grafcet_block` to `grammar.md`
3. Define `bindings_block` EBNF for HMI phase
4. Choose and lock the Rust + WASM toolchain for `asl/`
5. Build `dendri validate` CLI as the first executable artifact
6. Run round-trip proof on all golden documents
7. Bump `ASLversion` to `0.1.1` after grammar extensions are merged

---

*Generated: 2026-06-17 | dendriForge platform planning*
