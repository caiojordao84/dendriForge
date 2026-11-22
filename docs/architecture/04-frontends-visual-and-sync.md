# 04 — Polyglot Pure-Rust Frontends, IEC Ladder/GRAFCET & Bidirectional Sync

> **Normative Specification** | Decisions locked in Round 5 (ADRs 017–020)  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md), [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md)

---

## 1. Pure-Rust Polyglot Frontend Architecture (ADR-017)

`dendriForge` is designed to grow over time far beyond its Wave 1 Rosetta Stone core (C++, IEC ST, Ladder, GRAFCET) and Waves 2–3 (MicroPython, Rust) to eventually welcome a broad polyglot ecosystem: **C, JavaScript, Java, Forth/Mecrisp-Stellaris, Zig, TinyGo, Ada/SPARK, uLisp, Toit, C#, Assembly, Lustre (synchronous dataflow), FBD, XOD, Blockly, and Node-RED**.

### Why Pure Rust (Zero C `tree-sitter` FFI Dependency)
1. **100% Pure Rust toolchain & portability:** No C compiler / FFI glue needed across Linux, Windows, macOS, or future WASM builds; full control over spans, UTF-8 safety, and error recovery.
2. **Long-tail & niche languages:** Many target languages (IEC 61131-3 ST, Forth/Mecrisp-Stellaris, uLisp, Toit, Lustre, XOD, Blockly, Node-RED) lack maintained C `tree-sitter` grammars anyway.
3. **Zero copy-paste duplication (eliminating v1's 83% copy-pasted parsers):** All frontends build on ONE shared pure-Rust infrastructure crate **`dendri-frontend-core`**:
   - Zero-copy UTF-8-safe `Cursor` / `Lexer` primitives with exact `Span { start, end }` tracking (zero char-index vs byte-index panics!).
   - Reusable, table-driven **Pratt Expression Parser** engine (parameterized by language operator precedence, associativity, and literal syntax — fixing v1's universal `==` vs `<` precedence bug).
   - Shared **Symbol & I/O Tag Resolver**, **Diagnostic Builder** (rich error recovery, hints, line/col), and **Canonical Formatter** utilities.
   - Unified plugin trait:
     ```rust
     pub trait LanguageFrontend: Send + Sync {
         fn id(&self) -> LanguageId;
         fn paradigm(&self) -> Paradigm; // Textual | Ladder | Grafcet | Dataflow
         fn parse(&self, src: &str, ctx: &BoardContext) -> FrontendResult<AslModule>;
         fn emit(&self, module: &AslModule, ctx: &BoardContext) -> FrontendResult<String>;
         fn capabilities(&self) -> LanguageCapabilities;
     }
     ```
- **Strict crate isolation:** Each language is its own independent crate (`dendri-lang-cpp`, `dendri-lang-st`, `dendri-lang-ladder`, `dendri-lang-sfc`, later `dendri-lang-micropython`, `dendri-lang-rust`, etc.) implementing `LanguageFrontend` against `dendri-frontend-core` + `dendri-ast`.

---

## 2. Organization Blocks (OBs), Multi-POU Structure & Semantic Bridging (ADR-018)

To guarantee that **NO construct is ever silently lost** when translating between embedded MCU paradigms (C++/MicroPython/Rust) and industrial PLC paradigms (ST/Ladder/GRAFCET), ASL v2 adopts an **IEC 61131-3 / TIA Portal Organization Block (OB) & POU hierarchy**:

| Embedded / MCU Concept | ASL v2 Canonical POU / OB | Industrial PLC / Ladder / GRAFCET View |
|---|---|---|
| `void setup()` | `OB_STARTUP` (`OB100`) | Startup Ladder network / Init ST POU |
| `void loop()` | `OB_MAIN` (`OB1`) | Main Ladder network (calling secondary/tertiary Ladder POUs) or top-level GRAFCET chart |
| Hardware Timer Interrupt | `OB_CYCLIC_INT` (`OB30+`) | Periodic Cyclic Interrupt OB (e.g., 1 ms PID network) |
| `attachInterrupt(pin, fn, RISING)` | `OB_HW_INT` (`OB40+` Event OB) | **Event OB** triggered by hardware edge — viewable & editable as its own Ladder/ST POU! |
| `Serial` / UART / TTL RX & TX | `UART_SEND` / `UART_RECV` Communication FBs + `OB_COMM_EVENT` (`OB50+`) | Standard IEC `SEND_PTP` / `RCV_PTP` Ladder FB boxes + Comm Event OB! |
| Helper function `int calc(int a)` | Typed `FC` (`Function`) | Callable `FC` box inside any Ladder rung, GRAFCET action, or ST/C++ call |
| Reusable stateful controller | Typed `FB` (`Function Block`) | Instantiated FB block with `.IN`/`.OUT`/`.Q` pins in Ladder, ST, C++, or GRAFCET |
| Secondary/Tertiary Ladder subroutines | Named `FC`/`FB` POUs invoked by `OB_MAIN` | Multi-sheet Ladder hierarchy (`Main` → `Conveyor_Ctrl` → `Safety_Interlock`) |

### Zero-Loss Translation Rule
- **Every POU has its own `preferred_view` (`cpp | st | ladder | sfc | ...`)** while sharing the exact same board I/O Tag Table and Global Data Table!
- Standard control logic (boolean networks, latches, timers, counters, edges, comparisons, math, moves, communication FBs, state machines) lifts & lowers **1:1** between C++, IEC ST, Ladder, and GRAFCET.
- When a specific POU contains constructs not expressible in pure Ladder contacts/coils (e.g., a complex string tokenizer `while` loop), that POU remains safely intact as a typed **Structured POU** callable as a box from Ladder/GRAFCET — **never flattened into a destructive opaque dump, and never erasing `functions` or `ISRs`!**

---

## 3. Zero Magic Comments — Project I/O Tag Table Binding (ADR-020)

All `// ASL_HW:` / `// ASL_DATA:` / `(* HW: ... *)` round-trip cheat comments from v1 are **permanently banned**.

Instead, hardware binding flows through the board's **canonical I/O Tag Table** (`[[boards.program.hardware]]`):
- **Single Source of Hardware Truth:** Every physical pin used by a board is registered in its I/O Tag Table with `label`, `pin` (e.g., `"D13"`, `"A0"`, `"Ia.0"`, `"Qa.0"`, `"IW64"`), `kind`, `pull`, and `active_low`.
- **Idiomatic C++ (Arduino) Projection & Parsing:**
  - Supports `#include` (recognized standard headers), `#define LED_PIN 13`, `const uint8_t ledPin = 13;`, `pinMode(ledPin, INPUT_PULLUP);`, `digitalRead`/`digitalWrite`/`analogRead`/`analogWrite`/`delay`/`delayMicroseconds`/`millis`/`micros`/`attachInterrupt`/`Serial.*`/`Wire.*`/`SPI.*`, `int`/`uint8_t`/`uint16_t`/`uint32_t`/`int32_t`/`float`/`double`/`bool`, `for`/`while`/`do-while`/`switch-case`/`break`/`continue`, `i++`/`+=`, arrays, structs, enums, and real typed functions.
  - Emitted C++ uses idiomatic declarations bound to the I/O Tag Table without any hidden comments; handwritten Arduino sketches automatically populate/reconcile the I/O Tag Table!
- **Idiomatic IEC 61131-3 ST Projection & Parsing:**
  - Real IEC 61131-3 grammar: `PROGRAM` / `FUNCTION` / `FUNCTION_BLOCK`, separate `VAR_INPUT`, `VAR_OUTPUT`, `VAR_IN_OUT`, `VAR`, `VAR CONSTANT`, `VAR RETAIN`, `VAR_TEMP`, `VAR_GLOBAL`, standard direct hardware addressing (`StartBtn AT %IX0.0 : BOOL;`, `MotorOut AT %QX0.0 : BOOL;`, `PressRaw AT %IW64 : INT;`), `16#FF` / `2#1010` / `T#1s500ms` literals, infix `MOD`, `CASE ... OF`, `FOR i := 1 TO 10 BY 2 DO`, `REPEAT ... UNTIL`, `EXIT`, `CONTINUE`, `.Q`/`.ET`/`.CV` FB member access, and function return assignment (`MyFunc := expr;`).
  - Zero invented `SETUP`/`LOOP` keywords! (`OB_STARTUP` / `OB_MAIN` / `OB_HW_INT` emit as proper IEC `PROGRAM` / `ORGANIZATION_BLOCK` POUs.)

---

## 4. Full Industrial IEC 61131-3 Ladder & IEC 60848 GRAFCET Semantics (ADR-019)

### 4.1 Ladder Diagram (IEC 61131-3 LD — `dendri-lang-ladder`)
1. **Materialized Power-Flow Lowering (Fixing all v1 Ladder bugs):**
   - Every rung lowers left-to-right (strictly sorted by `(rung.order, col, row)` — never trusting raw array order!) by materializing power-rail segments into scoped `TEMP BOOL` wires (`__rail_0`, `__rail_1`, ...).
   - Eliminates v1's mid-rung mutation hazard (`[a] --(R a)-- (b)` reads the pre-reset rail!), parallel branch interference, side-effect duplication, and $O(2^k)$ AST blowup.
2. **Complete IEC 61131-3 LD Element Set:**
   - **Contacts:** Normally Open `--| |--`, Normally Closed `--|/|--`, Positive Transition `--|P|--`, Negative Transition `--|N|--`.
   - **Coils:** Normal `--( )--`, Negated `--(/)--`, Set/Latch `--(S)--`, Reset/Unlatch `--(R)--`, Positive Transition `--(P)--`, Negative Transition `--(N)--`.
   - **Wiring:** Horizontal wire (conducts), **Empty cell (OPEN CIRCUIT — `false`, never conducts!)**, arbitrary nested parallel OR branches.
   - **Function Block Boxes with Real `EN`/`ENO` and `Q` Propagation:**
     - `TON`, `TOF`, `TP`: `IN` driven by incoming power rail; **outgoing power rail = `fb.Q`!** (Fixes v1's completely broken timer/counter propagation.)
     - `CTU`, `CTD`, `CTUD`: `CU`/`CD` driven by power rail(s); outgoing rail = `fb.Q` / `fb.QU` / `fb.QD`.
     - `R_TRIG`, `F_TRIG`, `SR`, `RS`: outgoing rail = `fb.Q` / `fb.Q1`.
     - **Comparison Boxes (`EQ`, `NE`, `LT`, `LE`, `GT`, `GE`):** Strictly typed enum (never string-matched fallback to `==`!), gated by incoming `EN`, outputting `EN AND cmp_result`.
     - **Math / Move / Convert / Comm / User `FC`/`FB` Boxes (`ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `MOVE`, `MAP_RANGE`, `CLAMP`, `UART_SEND`, `UART_RECV`, etc.):** Standard IEC `EN`/`ENO` semantics — execute only when `EN == true`, pass `ENO` to downstream rail.
3. **Mandatory I/O Tag Binding:** Contacts and coils bind directly to declared I/O tags or typed `BOOL` variables — never dropping pin writes on `FALSE` (normal coils unconditionally write `tag := rail` on both `true` AND `false`!).
4. **Real Structural Lifting (ASL → Ladder):** Standard boolean expressions, `if` latches, FB invocations, comparisons, and `EN`-gated math/move statements lift deterministically into native contacts, coils, parallel branches, and FB boxes!

### 4.2 GRAFCET / SFC (IEC 60848 & IEC 61131-3 SFC — `dendri-lang-sfc`)
1. **Exact IEC 60848 Evolution Rules (Fixing all v1 SFC bugs):**
   - **Deterministic Selective (OR) Divergence Priority:** Transitions exiting a shared step carry explicit priority order (`priority: u32`, default left-to-right); when multiple competing OR transitions are simultaneously true, **strictly the highest-priority transition fires** (never activating both branches simultaneously!).
   - **Exact Rule 5 Two-Phase Evolution:**
     - Phase 1: Evaluate all transition receptivities against the **frozen pre-evolution step state**.
     - Phase 2a: Deactivate all `from` steps of fired transitions.
     - Phase 2b: Activate all `to` steps of fired transitions (**after** all clears, so a step simultaneously deactivated by `T_b` and activated by `T_a` remains **ACTIVE** per IEC 60848 Rule 5!).
   - **Simultaneous (AND) Divergence & Convergence:** Full synchronization barrier verifying all upstream steps are active before firing convergence transition.
   - **Transient / Instability Guard:** Configurable single-step-per-scan vs. stability evolution with cycle detection (preventing infinite zero-time loops).
2. **First-Class Action Qualifiers (IEC 61131-3 Table 47):**
   - `N` (Non-stored — **automatically resets boolean coil when step deactivates!**)
   - `S` (Set / Stored)
   - `R` (Reset / Overriding Reset)
   - `L` (Time Limited — active for duration `T` while step is active)
   - `D` (Time Delayed — activates after duration `T` while step remains active)
   - `P` (Pulse — 1 scan on step activation), `P1` (Rising edge), `P0` (Falling edge on deactivation)
3. **Native Step Flags & Timers (Zero Mangled `__sfc_*` Names!):**
   - Step activity `<StepName>.X` (`BOOL`) and step active elapsed time `<StepName>.T` (`TIME`) are **first-class typed expressions** in ASL/ST/C++/Ladder/GRAFCET!
   - Macro-steps (`enclosed` sub-charts) have explicit **Entry Step** and **Exit Step** semantics (`<MacroStep>.DONE`), initializing cleanly on Scan 1 even if initial, and resetting properly on same-scan re-entry — **with zero duplicate `setup` statements across round-trips!**

---

## 5. Non-Destructive Bidirectional Sync Engine (`dendri-sync`)

1. **Single Authoritative AST per Board (`BoardDoc`):**
   - `dendri-sync` manages a versioned (`revision: u64`) per-board `AslModule` + `PresentationStore` (keyed by `NodeId`).
   - Editing in **any** view (C++, ST, Ladder, GRAFCET, I/O Tag Table) runs: `Parse -> Validate -> Diff/Merge into AslModule (preserving untouched OBs/POUs & PresentationStore) -> Increment revision`.
2. **Zero Silent Clobbering:**
   - Editing a Ladder POU updates **only** that POU — `OB_STARTUP`, `OB_HW_INT` (ISRs), other POUs, and board metadata remain 100% intact.
   - Invalid edits in a text buffer are **never** silently overwritten or discarded on tab switch; the editor retains the dirty buffer with inline red diagnostics until fixed or explicitly reverted by the user.
   - Every mutation records a unified **Command/Undo Transaction** spanning Canvas, I/O Table, Code, Ladder, and GRAFCET!
