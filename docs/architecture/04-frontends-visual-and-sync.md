# 04 — Polyglot Pure-Rust Frontends, LBC, Structural Ladder/GRAFCET & Bidirectional Sync

> **Normative Specification** | Baseline (Rev 2) — ADRs 003, 017–020, 025–028  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md), [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md), [`03-asset-and-profile-contract.md`](./03-asset-and-profile-contract.md)

---

## 1. Pure-Rust Polyglot Frontend Architecture & Staged Scope (ADR-003, ADR-017)

### 1.1 Normative Language Scope
- **Wave 1 (Rosetta Stone Core):** `C++ (Arduino subset)`, `IEC 61131-3 Structured Text (ST)`, `Ladder Diagram (LD)`, and `GRAFCET / SFC`. Both directions of the MCU ↔ industrial PLC bridge are proven in Wave 1; Wave 1 exits ONLY when **Phase 6b** (real-toolchain differential gate) is 100% green.
- **Wave 2:** `MicroPython` — opens ONLY after Wave 1 exit and completion of `asl-oop` (`DEF-004`).
- **Wave 3:** `Rust` (Dendri HAL subset) — opens ONLY after Wave 2 exit.
- **Horizon (Non-Normative):** Robotic languages (`asl-motion`, `DEF-005`) and long-tail languages are **outside normative scope** (see [Appendix A](#appendix-a--non-normative-horizon-notes)). ASL MUST NOT foreclose them: versioned `requires` capability modules and the non-exhaustive `ExecutionKind` enum preserve forward extensibility.

### 1.2 Pure-Rust Frontend Infrastructure (`dendri-frontend-core`, Zero C `tree-sitter` FFI)
1. **100% Pure Rust Toolchain & Portability:** Zero C compiler / FFI build glue across Linux, Windows, macOS, or future WASM targets; complete control over UTF-8 safety, byte spans, and syntax error recovery.
2. **Zero Copy-Paste Duplication:** All language crates build on one shared infrastructure crate **`dendri-frontend-core`**:
   - Zero-copy UTF-8-safe `Cursor` / `Lexer` primitives with exact byte `Span { start, end }` tracking.
   - Reusable, table-driven **Pratt Expression Parser** engine parameterized by operator precedence, associativity, and literal syntax.
   - Shared **Symbol, Module & I/O Tag Resolver**, **Diagnostic Builder**, **Canonical Formatter**, and **Comment Renderer/Matcher** utilities.
   - Unified plugin trait:
     ```rust
     pub trait LanguageFrontend: Send + Sync {
         fn id(&self) -> LanguageId;
         fn paradigm(&self) -> Paradigm; // Textual | Ladder | Grafcet
         fn parse(&self, src: &str, ctx: &BoardContext) -> FrontendResult<AslModule>;
         fn emit(&self, module: &AslModule, ctx: &BoardContext) -> FrontendResult<String>;
         fn render_comment(&self, text: &str, placement: CommentPlacement) -> RenderedComment;
         fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool;
         fn capabilities(&self) -> LanguageCapabilities;
     }
     ```
3. **Strict Crate Isolation:** Each language lives in its own crate (`dendri-lang-cpp`, `dendri-lang-st`, `dendri-lang-ladder`, `dendri-lang-sfc`; later `dendri-lang-micropython`, `dendri-lang-rust`) implementing `LanguageFrontend` against `dendri-frontend-core` + `dendri-ast`.

---

## 2. Language Binding Contract (LBC)

ASL is the single executable truth (Principle P2). Every frontend crate **MUST** ship an LBC specification document and pass the shared frontend conformance suite before merging. An LBC MUST specify:

1. **Accepted Grammar Subset (EBNF)** and the exact frozen `ErrorCode` for every rejected construct.
2. **Per-Board Type Mapping:** Explicit mapping from language types to ASL scalar/composite types, honoring `[cpu] int_bits` (`16` on AVR, `32` on 32-bit cores), IEEE-754 `f32`/`f64` widths, and string/array limits.
3. **Numeric Codecs & Intrinsic Mapping:** Exact mapping of analog reads/writes and PWM calls to `ANALOG` / `DUTY` / `CODE` and per-channel codecs ([`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md#3-asl-numeric-contract-adc-420-ma-pwm-adr-025)):
   - Wave 1 normative idioms: `analogRead(A0)` ⇄ `ADC_CODE(A0)` (board default codec); ST `IW64` ⇄ `ADC_CODE(IW64)`.
   - Runtime-changeable resolution (`analogReadResolution`) is a statement updating the channel codec; fixed-codec emitters MUST either prove resolution static or emit explicit clamped conversion.
   - **Emitters NEVER emit `M` ($10^9$):** cross-platform native translations emit the clamped reduced rational `min(n_max2, floor(n * D2 / D1))` with integer width selected by range analysis (32-bit when `n_max * D2 < 2^32`, else 64-bit).
4. **Comment Mapping & `comment_styles`:** Supported line/block comment syntax per target dialect (`DEF-006`), `render_comment` / `matches_comment` semantics, hazard handling, bidi escaping, and `comment_encoding` behavior (§4.3).
5. **Module & Import Mapping:** Mapping between source files/headers/libraries and ASL modules, `exports`, `requires`, and domain-split stdlib (`std.iec.*` vs platform modules).
6. **Hardware Address & Tag Mapping:** Deterministic bidirectional mapping between logical I/O tags, board ports, and `iec_address` (e.g., `Ia.0 ⇄ %IX0.0`, `Qa.0 ⇄ %QX0.0`, `IW64 ⇄ %IW64`).
7. **Mandatory Round-Trip Class — `LOSSLESS_ASL`:** `parse ∘ emit` MUST be the identity on ASL (`AslModule`). Text-level identity is promised strictly for verbatim comment text (plus optional `blank_lines_before: 0..=2`); emitted code formatting is canonical per language.
8. **`LanguageCapabilities`:** Declared supported comment styles, encodings (`utf8_passthrough`, `ascii_fold`, `warn_only`), and supported capability modules (`asl-core@1`, etc.).

### Rejection, Feature Matrix & Shared Conformance Suite
- **Reject, Don't Guess:** An unrecognized construct MUST become a `Foreign` (`F0`) node or a spanned diagnostic — NEVER silently dropped or guessed.
- **Generated Feature Matrix:** The language feature matrix published in documentation MUST be auto-generated from the conformance test suite, never written or maintained by hand.
- **Shared Conformance Suite (mandatory for every frontend crate):**
  1. Lift/lower `proptest` verifying `LOSSLESS_ASL` (`parse(emit(m)) == m`).
  2. Comment conservation suite (`C1–C5`, including hostile corpus).
  3. Numeric codec, clamped composition, and intrinsic mapping tests.
  4. Three-level `Foreign` preservation tests (`Decl::Foreign`, `Stmt::Foreign`, `Expr::Foreign`).
  5. At least one genuine negative fixture per `ErrorCode` produced by the frontend.

---

## 3. Organization Blocks (OBs), Multi-POU Structure, Modules & `Foreign` (ADR-018, ADR-028)

### 3.1 Organization Block & POU Semantic Bridging
To guarantee that NO construct is silently lost when moving between MCU and industrial PLC paradigms, ASL v2 uses an IEC 61131-3 / Industrial PLC OB & POU hierarchy:

| Embedded / MCU Concept | ASL v2 Canonical POU / OB | Industrial PLC / Ladder / GRAFCET View |
|---|---|---|
| `void setup()` | `OB_STARTUP` (`OB100`) | Startup Ladder `Network` / Init ST POU |
| `void loop()` | `OB_MAIN` (`OB1`) | Main Ladder `Network` (calling secondary/tertiary POUs) or top-level GRAFCET `Chart` |
| Hardware Timer Interrupt | `OB_CYCLIC_INT` (`OB30+`) | Periodic Cyclic Interrupt OB (e.g., 1 ms PID network) |
| `attachInterrupt(pin, fn, RISING)` | `OB_HW_INT` (`OB40+` Event OB) | Event OB triggered by hardware edge — viewable & editable as Ladder/ST |
| `Serial` / UART / TTL RX & TX | `UART_SEND` / `UART_RECV` FBs + `OB_COMM_EVENT` (`OB50+`) | Standard IEC `SEND_PTP` / `RCV_PTP` Ladder FB boxes + Comm Event OB |
| Helper function `int calc(int a)` | Typed `FC` (`Function`) | Callable `FC` box inside any Ladder rung, GRAFCET action, or ST/C++ call |
| Reusable stateful controller | Typed `FB` (`Function Block`) | Instantiated FB block with `.IN`/`.OUT`/`.Q` pins across all views |
| Secondary/Tertiary Ladder subroutines | Named `FC`/`FB` POUs invoked by `OB_MAIN` | Multi-sheet Ladder hierarchy (`Main` → `Conveyor_Ctrl` → `Safety_Interlock`) |

- **Per-POU `preferred_view`:** Every POU selects its own view (`cpp | st | ladder | sfc`) while sharing the exact same Board I/O Tag Table and Global Data Table.
- **Non-Liftable Structured POUs:** A POU containing constructs outside pure Ladder contacts/coils (e.g., a string tokenizer `while` loop) remains intact as a typed **Structured POU** callable as a box from Ladder/GRAFCET — never flattened into an opaque dump and never erasing functions or ISRs.

### 3.2 Wave 1 Module & Domain-Split Stdlib Mapping
| Language | Module & Import Mapping |
|---|---|
| C++ (Arduino subset) | Project headers / translation units ⇄ module `exports` / implementation; multi-tab Arduino sketches merge per documented Arduino rules; `#include <Arduino.h>` implies `std.arduino.*`; other standard headers resolve to `std.*` |
| IEC 61131-3 ST | Multiple POUs and user libraries ⇄ modules; `FB`/`FC` types map 1:1; only `std.iec.*` is in scope unless a platform module is explicitly imported (`ERR_UNRESOLVED_IMPORT` otherwise) |
| Ladder / GRAFCET | `FB`/`FC` boxes reference library blocks; macro-steps reference sub-charts |

- **C++ Classes & Methods:** Flat `STRUCT` is supported in `asl-core@1`. C++ `class`/`struct` **with methods** lifts to `Foreign (F0)` unless declared by a pack, until `asl-oop` (IEC 61131-3 3rd-edition FB methods & interfaces, `DEF-004`) opens at the Wave 2 gate.

### 3.3 Three-Level `Foreign` Nodes, Packs & Cross-Language Export
`VendorExtension` is permanently removed. Unsupported or vendor-specific constructs preserve cleanly at all three AST levels (`Decl::Foreign`, `Stmt::Foreign`, `Expr::Foreign` — [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md#62-packs--three-level-foreign-nodes-replacing-vendorextension)):
- **Fidelity Tiers:** `F0 Preserved` (verbatim same-language round-trip) → `F1 Declared` (signatures/types from a hash-locked `pack.toml`, `DEF-008`) → `F2 Modeled` (full simulation via ASL or native pack model).
- **Direct Register Access** (e.g., AVR `PORTB |= (1<<5)`) starts at `F0` in Wave 1 and becomes modelable later through board profile register maps.
- **Four-Part Diagnostic Contract:** Every `Foreign`/unmodeled diagnostic MUST state: **(1) what** (construct), **(2) where** (span), **(3) why** (missing fidelity level), and **(4) what now** (record an explicit assumption in `[[boards.assumptions]]`, install a pack, or replace with an equivalent).
- **Cross-Language Views & Export:**
  - In other views, a `Foreign` node renders as a visibly labeled **locked box** (showing source language, fidelity badge, and text).
  - Exporting a file containing `Foreign` to a *different* language is refused for that file only (`ERR_FOREIGN_EXPORT_BLOCKED`); passing `--allow-foreign` emits a clearly demarcated `"NOT TRANSLATED"` comment block instead. Same-language round-trips are NEVER blocked.

---

## 4. Zero Machine Magic Comments, Board I/O Tag Table & Verbatim Comments (ADR-020, ADR-027)

### 4.1 Permanent Ban on Machine Magic Comments
All v1 round-trip cheat comments (`// ASL_HW:`, `// ASL_DATA:`, `(* HW: ... *)`) are **permanently banned**. Hardware binding flows strictly through the board's canonical **I/O Tag Table** (`[[boards.program.hardware]]`) paired with the board profile's port & `iec_address` table.

### 4.2 Idiomatic Wave 1 Textual Frontends
- **C++ (Arduino Subset — `dendri-lang-cpp`):**
  - Supports `#include`, `#define LED_PIN 13`, `const uint8_t ledPin = 13;`, `pinMode`, `digitalRead`/`digitalWrite`, `analogRead` (⇄ `ADC_CODE`), `analogWrite` (⇄ `DUTY`), `analogReadResolution`, `delay`/`delayMicroseconds`, `millis`/`micros`, `attachInterrupt`, `Serial.*`/`Wire.*`/`SPI.*`, fixed-width integers + `[cpu] int_bits`-mapped `int`, `float`/`double`/`bool`, `for`/`while`/`do-while`/`switch-case`/`break`/`continue`, `i++`/`+=`, arrays, flat structs, enums, and typed functions.
  - Emits idiomatic Arduino C++ bound to the I/O Tag Table with zero hidden machine comments; parsing handwritten sketches automatically reconciles the I/O Tag Table.
- **IEC 61131-3 Structured Text (`dendri-lang-st`):**
  - Real IEC 61131-3 grammar: `PROGRAM` / `FUNCTION` / `FUNCTION_BLOCK`, separate `VAR_INPUT`, `VAR_OUTPUT`, `VAR_IN_OUT`, `VAR`, `VAR CONSTANT`, `VAR RETAIN`, `VAR_TEMP`, `VAR_GLOBAL`, direct hardware addressing (`StartBtn AT %IX0.0 : BOOL;`, `MotorOut AT %QX0.0 : BOOL;`, `PressRaw AT %IW64 : INT;` mapped via board profile `iec_address`), `16#FF` / `2#1010` / `T#1s500ms` literals, infix `MOD`, `CASE ... OF`, `FOR i := 1 TO 10 BY 2 DO`, `REPEAT ... UNTIL`, `EXIT`, `CONTINUE`, `.Q`/`.ET`/`.CV` FB member access, and function return assignment (`MyFunc := expr;`).
  - Zero invented `SETUP`/`LOOP` keywords: OBs emit as proper IEC `PROGRAM` / `ORGANIZATION_BLOCK` POUs.

### 4.3 First-Class Verbatim Human Comments (`ADR-027`)
Human comments live in `[[boards.comments]]` and top-level `[[comments]]` (NEVER inside optional `[boards.presentation]`):
- **Verbatim storage & closed `origin.style`:** Exact text between delimiters, zero trimming, zero NFC/NFD normalization, LF internal line endings (original EOL restored on same-language emit). `origin.style` uses the closed enum `line | block | paren_block | doc_line | doc_block | visual` ([`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md#7-comments--first-class-verbatim-data-adr-027)).
- **Grouping & nesting rules:** Contiguous `leading` line comments of matching style with no blank line merge into one record joined by `\n` (blank lines split records; `trailing` comments are always single-line). The IEC ST lexer tracks nesting depth for `(* ... *)` (`paren_block`) and stores nested bodies verbatim.
- **Hazard avoidance (projection-only):**
  - C++ line comment ending in `\` → emitted as a block comment.
  - Block comment containing `*/` (C++), `*)` (ST), or nested `(* ... *)` projected to C++ → emitted as line comments where the target dialect allows (`DEF-006`); otherwise a documented non-destructive degradation applies.
  - Bidirectional control characters (Trojan-Source class) → `WARN_COMMENT_BIDI` and rendered escaped in emitted code.
- **Re-Import Idempotence:** On re-import, stored `text` is replaced ONLY if `matches_comment(parsed, stored_text)` returns `false` — guaranteeing a degraded projection never overwrites the verbatim original.
- **`inner` Relocation & Orphan Preservation:** `inner` expression comments (with required `token_ordinal`) relocate deterministically to the nearest enclosing statement boundary on cross-language emission (`INFO_COMMENT_RELOCATED`). Deleting an anchored node turns its comment into `dangling` on the parent scope (`INFO_COMMENT_ORPHANED`) — never deleting user commentary.
- **Encoding & Display Charsets:** Per-target `comment_encoding` (`utf8_passthrough` default | `ascii_fold` | `warn_only`) degrades only the emitted projection (`WARN_COMMENT_ENCODING`, never blocking). `ERR_STRING_CHARSET` applies strictly to string literals routed to display sinks declaring a component `[display] charset` (`DEF-007`) — NEVER to UTF-8 `Serial`/UART byte streams.

---

## 5. Structural Industrial IEC 61131-3 Ladder & IEC 60848 GRAFCET (ADR-019, ADR-026)

`PouBody::Network` (Ladder) and `PouBody::Chart` (GRAFCET/SFC) are **first-class structural POU body kinds in ASL** (ADR-026). **NO synthetic `__rail_N` or `__sfc_*` temporaries are EVER stored in ASL.** Both are lowered strictly in memory by `dendri-vm` at load time.

### 5.1 Ladder Diagram (IEC 61131-3 LD — `dendri-lang-ladder`)
1. **Structural IR & Load-Time Materialized Power-Flow Lowering:**
   - Stored in ASL as `PouBody::Network(LadderNetwork)`: ordered rungs (keyed by explicit `order` and `(row, col)` grid coordinates — never trusting raw array ordering) containing typed Contact, Coil, Wire, Empty, Branch, and Box cells, with network title, body, and element comments anchored by `NodeId`.
   - At load time inside `dendri-vm` (in memory only), each rung lowers left-to-right into materialized power-rail registers, eliminating mid-rung mutation hazards (`[a] --(R a)-- (b)` reads the pre-reset rail), parallel branch interference, side-effect duplication, and $O(2^k)$ AST blowup.
2. **Complete IEC 61131-3 LD Element Set:**
   - **Contacts:** Normally Open `--| |--`, Normally Closed `--|/|--`, Positive Transition `--|P|--`, Negative Transition `--|N|--`.
   - **Coils:** Normal `--( )--`, Negated `--(/)--`, Set/Latch `--(S)--`, Reset/Unlatch `--(R)--`, Positive Transition `--(P)--`, Negative Transition `--(N)--`.
   - **Wiring:** Horizontal wire (conducts), **Empty cell (OPEN CIRCUIT — `false`, NEVER conducts)**, arbitrary nested parallel OR branches.
   - **Function Block Boxes with Real `EN`/`ENO` and `Q` Propagation:**
     - `TON`, `TOF`, `TP`: `IN` driven by incoming power rail; **outgoing power rail = `fb.Q`**.
     - `CTU`, `CTD`, `CTUD`: `CU`/`CD` driven by power rail(s); outgoing rail = `fb.Q` / `fb.QU` / `fb.QD`.
     - `R_TRIG`, `F_TRIG`, `SR`, `RS`: outgoing rail = `fb.Q` / `fb.Q1`.
     - **Comparison Boxes (`EQ`, `NE`, `LT`, `LE`, `GT`, `GE`):** Strictly-typed enum (never string-matched fallback), gated by `EN`, outputting `EN AND cmp_result`.
     - **Math / Move / Convert / Comm / User `FC`/`FB` Boxes (`ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `MOVE`, `MAP_RANGE`, `CLAMP`, `UART_SEND`, `UART_RECV`, etc.):** Standard IEC `EN`/`ENO` semantics — execute only when `EN == true`, pass `ENO` downstream.
3. **Mandatory I/O Tag Binding:** Contacts and coils bind directly to declared I/O tags or typed `BOOL` variables; normal coils unconditionally write `tag := rail` on both `true` AND `false`.
4. **Derived Projection & Conservative Structural Lifting:**
   - Emitting C++ or ST from a `Network` is a deterministic derived projection (preserving rung & element comments).
   - Lifting C++/ST text into a `Network` recognizes **only declared canonical patterns** (boolean networks, latches, FB invocations, comparisons, `EN`-gated math/move); non-matching POUs remain safely intact as callable `Structured POU` boxes and are NEVER flattened.

### 5.2 GRAFCET / SFC (IEC 60848 & IEC 61131-3 SFC — `dendri-lang-sfc`)
1. **Structural IR & Load-Time IEC 60848 Rule 5 Evolution:**
   - Stored in ASL as `PouBody::Chart(SfcChart)`: typed Steps (initial, normal, macro-steps), Transitions (with explicit `priority: u32`, default left-to-right), AND/OR divergence & convergence links, and Actions with typed qualifiers — plus step, transition, and action comments anchored by `NodeId`.
   - Lowered in memory at load time by `dendri-vm` enforcing exact **IEC 60848** evolution semantics:
     - **Deterministic Selective (OR) Divergence Priority:** When competing OR transitions exiting a shared step are simultaneously true, **strictly the highest-priority transition fires**.
     - **Exact Rule 5 Two-Phase Evolution:**
       1. Phase 1: Evaluate all transition receptivities against the **frozen pre-evolution step state**.
       2. Phase 2a: Deactivate all `from` steps of fired transitions.
       3. Phase 2b: Activate all `to` steps of fired transitions (**after** all clears, so a step simultaneously deactivated by `T_b` and activated by `T_a` remains **ACTIVE**).
     - **Simultaneous (AND) Divergence & Convergence:** Full synchronization barrier verifying all upstream steps are active before firing convergence.
     - **Transient / Instability Guard:** Configurable single-step-per-scan vs. stability evolution with cycle detection preventing infinite zero-time loops.
2. **First-Class Action Qualifiers (IEC 61131-3 Table 47):**
   - `N` (Non-stored — **automatically resets boolean coil when step deactivates**), `S` (Set/Stored), `R` (Overriding Reset), `L` (Time Limited for duration `T`), `D` (Time Delayed by `T`), `P` (1-scan Pulse on activation), `P1` (Rising edge), `P0` (Falling edge on deactivation).
3. **Native Step Flags, Timers & Macro-Steps (Zero Mangled `__sfc_*` Names):**
   - `<StepName>.X` (`BOOL`), `<StepName>.T` (`TIME`), and `<MacroStep>.DONE` (`BOOL`) are **first-class typed expressions** across ASL, ST, C++, Ladder, and GRAFCET.
   - Macro-steps (`enclosed` sub-charts) have explicit Entry Step and Exit Step semantics, initializing cleanly on Scan 1 even if initial and resetting properly on same-scan re-entry with zero duplicate setup statements across round-trips.

---

## 6. Non-Destructive Bidirectional Sync Engine (`dendri-sync`)

1. **Single Authoritative Document per Board (`BoardDoc`):**
   - `dendri-sync` manages a versioned (`revision: u64`) per-board `AslModule`, `Comments` (`[[boards.comments]]`), `Assumptions` (`[[boards.assumptions]]`), and `PresentationStore` (keyed by `NodeId`).
   - Editing in **any** view (C++, ST, Ladder, GRAFCET, I/O Tag Table) executes: `Parse -> Validate -> Diff/Merge into BoardDoc (preserving untouched OBs/POUs, comments, assumptions, Foreign nodes & presentation hints) -> Increment revision`.
2. **Zero Silent Clobbering or Comment Loss:**
   - Editing a Ladder POU updates **only** that POU — `OB_STARTUP`, `OB_HW_INT` (ISRs), other POUs, `Foreign` nodes, `[[boards.assumptions]]`, and board metadata stay 100% intact.
   - Deleting an anchored node transitions its comments to `dangling` (`INFO_COMMENT_ORPHANED`) — never deleting them.
   - Invalid edits in a text buffer are **never** silently overwritten or discarded on tab switch; the editor retains the dirty buffer with inline red diagnostics until fixed or explicitly reverted.
   - Every mutation records a unified **Command/Undo Transaction** spanning Canvas, I/O Tag Table, Code, Ladder, GRAFCET, Comments, and Assumptions.

---

## Appendix A — Non-Normative Horizon Notes

Items in this appendix are explicitly **outside normative scope** for Waves 1–3 and serve solely as architectural guardrails so current design choices do not foreclose future expansion:

- **MicroPython (Wave 2) Prerequisites:** Requires `asl-oop` (`DEF-004`), multi-file module semantics exercised end-to-end, exact `read_u16`/`duty_u16` scaling taken directly from each target port's source code (never from documentation summaries), and certified `esp32-devkitc-v4` / `raspberry-pi-pico` board profiles with per-port ADC resolution.
- **Rust (Wave 3):** Opens after Wave 2 exit.
- **Robotic Languages:** Vendor robot programming languages (collectively **robotic languages**) require a future `asl-motion` capability (`DEF-005`: blocking motion instructions, task-based `ExecutionKind`, structured pose/tool data, and program-local comment syntaxes). The syntax-independent comment model of `01` §7 and versioned `requires` capability negotiation ensure compatibility without complicating `asl-core@1`.
- **Long-Tail Languages:** Languages formerly noted in early brainstorming (C, JavaScript, Java, Forth/Mecrisp-Stellaris, Zig, TinyGo, Ada/SPARK, uLisp, Toit, C#, Assembly, Lustre, FBD, XOD, Blockly, Node-RED) are non-goals until after Wave 3 exit.
