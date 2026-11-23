# 01 — ASL v2 Intermediate Representation, Numeric Contract, Modules, Comments & Project Schema

> **Normative Specification** | Baseline (Rev 2) — ADRs 005–008, 018, 025–028  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md)

---

## 1. Design Philosophy: Foundation Stone + Rosetta Stone

ASL v2 (**Abstract Syntax Language**) is the **sole executable intermediate representation** of `dendriForge`. Every board program — whether authored in C++ (Arduino), IEC 61131-3 Structured Text, Ladder Diagram (LD), GRAFCET/SFC, or later MicroPython and Rust — compiles to and projects from the exact same strictly-typed ASL v2 AST.

### Core Rules
1. **One AST in memory, zero escape hatches:** There is no `Expr::ExprText`, no duplicate `Expr::Lit`, no `VendorExtension`, and no untyped `String` initializers. Every expression, statement, declaration, and literal is statically typed and span-tracked.
2. **Spans everywhere:** Every AST node carries a `Span { start: u32, end: u32 }` (byte offsets into the active source or TOML document) so parser, typechecker, validator, and runtime errors map to exact file/line/column diagnostics.
3. **Stable Node IDs (`NodeId`):** Top-level declarations, POUs, statements, Ladder rungs/cells, GRAFCET steps/transitions, and `Foreign` nodes carry stable identifiers (`n_0001`, …) that anchor **first-class comments**, **simulation assumptions**, and **optional presentation metadata** (visual coordinates, `blank_lines_before`, `origin_file`) without ever duplicating executable logic.
4. **Strict TOML deserialization:** Every schema struct MUST use `#[serde(deny_unknown_fields)]`. Unknown keys are hard parse errors, never silently ignored.

---

## 2. ASL v2 Type System (ADR-005)

ASL v2 combines **IEC 61131-3 strong static typing** with **deterministic embedded semantics** (no pointers, no raw memory aliasing, no dynamic heap allocation, no generics in user code).

### 2.1 Primitive Scalar Types (Fixed Width & Exact Semantics)

| ASL Type | Bit Width | Semantics | Overflow / Range Policy |
|---|---:|---|---|
| `BOOL` | 1 bit | `true` / `false` | N/A |
| `SINT` / `INT` / `DINT` / `LINT` | 8 / 16 / 32 / 64 | Two's-complement signed integer | Explicit wrapping (`wrapping_*`) or checked trap per runtime config (never silent saturating mismatch) |
| `USINT` / `UINT` / `UDINT` / `ULINT` | 8 / 16 / 32 / 64 | Unsigned integer | Explicit wrapping (`wrapping_*`) |
| `BYTE` / `WORD` / `DWORD` / `LWORD` | 8 / 16 / 32 / 64 | Bit-string (bitwise/shift/rotate) | Bitwise exact width |
| `REAL` / `LREAL` | 32 / 64 | IEEE-754 `f32` / `f64` (no implicit FMA; `libm` transcendentals) | IEEE-754 (explicit `NaN`/`Inf` validation hooks) |
| `TIME` | 64 | Duration in **microseconds** (`i64` / `u64`) | Supports `T#500us`, `T#200ms`, `T#1s500ms`, `T#2m` |
| `STRING` | Bounded | UTF-8 fixed-capacity string (`STRING[N]`, default `N = 255`) | Deterministic truncation/error; zero unbounded OOM |
| `ANALOG` | 32 | Signed fixed-point analog quantity at scale `M = 10⁹` (§3) | Domain `[-2.147M, +2.147M]` (supports bipolar & S7 over/underrange) |
| `DUTY` | 32 | **Unsigned** fixed-point PWM duty cycle in `[0, M]` (§3.5) | Values `> M` clamp to `M` with `WARN_DUTY_CLAMPED` |
| `CODE` | Codec-bound | Signed or unsigned native converter code bound to a declared codec id (§3.5) | Forbidden to mix with plain integers without explicit conversion |

> **No silent cross-sign comparison bug:** Mixed numeric operations require explicit widening or explicit cast (`INT_TO_DINT`, `UINT_TO_REAL`, `ANALOG_TO_CODE`, `CODE_TO_ANALOG`). Because all expressions are statically type-checked before execution, `Int(1) != Uint(1)` runtime corruption cannot occur.

### 2.2 Composite & User-Defined Types
- **Fixed-size Arrays:** `ARRAY[N] OF T` (0-indexed `0..N`, statically bounded; runtime bounds checks return a clean runtime diagnostic, never a Rust panic). Multidimensional arrays supported as `ARRAY[M] OF ARRAY[N] OF T`.
- **Flat Structs (`STRUCT`):** Named product types with typed fields (`struct.field` read/write). No recursive structs; C++ `class`/`struct` with methods is outside `asl-core@1` (deferred to `asl-oop`, `DEF-004`, and preserved as `Foreign (F0)` meanwhile).
- **Enumerations (`ENUM`):** Named strongly-typed symbolic constants backed by an explicit integer width (default `DINT`).
- **First-Class Function Block (FB) Instances:** Standard IEC 61131-3 stateful blocks are real types in the type system:
  - Timers: `TON`, `TOF`, `TP` — fields `.IN`, `.PT`, `.Q`, `.ET`
  - Counters: `CTU`, `CTD`, `CTUD` — fields `.CU`, `.CD`, `.R`, `.LD`, `.PV`, `.Q`/`.QU`/`.QD`, `.CV`
  - Edge detectors: `R_TRIG`, `F_TRIG` — fields `.CLK`, `.Q`
  - Bistables: `SR`, `RS` — fields `.S1`/`.R` or `.S`/`.R1`, `.Q1`
  - Communication FBs: `UART_SEND`, `UART_RECV`, `I2C_XFER`, `SPI_XFER`, `MB_CLIENT`, `MB_SERVER`
  - User-defined stateful Function Blocks (`FB`) using the exact same field-access & instance mechanism.

### 2.3 Storage Scopes
- `CONST` — Immutable compile-time constant (writes forbidden by validator & runtime).
- `VAR` — Standard persistent-across-scans volatile RAM variable (re-initialized on reset).
- `RETAIN` — Preserved across warm/soft restarts; reset on cold restart.
- `PERSISTENT` — Preserved across warm restarts and power cycles (non-volatile EEPROM/NV image).
- `TEMP` — Re-initialized to default on every scan/function entry (strict lexical scope).

### 2.4 Identifier Grammar, Endpoints & Hardware I/O Bindings (`[[hardware]]`)

#### Identifier & Endpoint Grammar
- **Profile IDs** (boards, components, screens): `^[a-z0-9]+(-[a-z0-9]+)*$` (kebab-case, e.g., `arduino-uno-r3`, `cmd-pushbutton-no-start`).
- **Instance IDs** (boards and components in a project): `^[a-z][a-z0-9_]*$` — **no dots, no hyphens** (globally unique across `[[boards]]` and `[[components]]`).
- **Port IDs:** MAY contain dots (e.g., `D13`, `A0`, `Ia.0`, `Qa.0`, `IW64`).
- **Wire Endpoints:** `<instance_id>.<port_id>`, **strictly split at the first dot**. Any malformed endpoint raises `ERR_BAD_ENDPOINT`.
- **IEC Direct Address Mapping:** Board profiles declare optional `iec_address` per port (e.g., `Ia.0 ⇄ %IX0.0`, `Qa.0 ⇄ %QX0.0`, `IW64 ⇄ %IW64`) so IEC ST round-trips recover tag bindings deterministically.

#### Hardware Binding Fields (`[[hardware]]`)
- `label: String` — Immutable symbolic identifier (cannot be assigned as a variable; validated against `data` and POUs for collisions).
- `pin: String` — Canonical board port ID from the board profile.
- `kind`: `DigitalIn | DigitalOut | DigitalBidirectional | AnalogIn | AnalogOut | PwmOut | BusPin`
- `pull`: `None | PullUp | PullDown` (validated against the board profile's pin capabilities).
- `active_low: bool` (optional electrical polarity hint, cleanly separated from `pull`).
- `codec: Option<String>` — **Required** for `AnalogIn`, `AnalogOut`, `PwmOut`; references a codec ID declared in the board profile (§3).
- `range: Option<ChannelRange>` — **Required** for `AnalogIn`, `AnalogOut`; declares signal type, low/high, live-zero flag, and whether the module itself normalizes live zero (§3.4).

### 2.5 Program Organization Units (POUs), Organization Blocks (OBs) & Structural IR (ADR-018, ADR-026)

Every board's ASL `Program` is structured into typed **Organization Blocks (OBs)** and reusable **POUs (`FC` / `FB`)** (`[[boards.program.pous]]`):
- **Organization Blocks (`kind = "ob"`):**
  - `OB_STARTUP` (`OB100`): Runs once at cold/warm boot (after module top-level initialization; maps to Arduino `setup()`).
  - `OB_MAIN` (`OB1`): Main cyclic scan / `loop()`, which MAY invoke secondary/tertiary Ladder networks, GRAFCET charts, or functions.
  - `OB_CYCLIC_INT` (`OB30+`): Periodic timed interrupt OBs with exact microsecond period (e.g., deterministic 1 ms PID loop).
  - `OB_HW_INT` (`OB40+`, Event OBs): Hardware pin interrupts / ISRs (`pin_label`, `edge: Rising | Falling | Change | Low | High`, `priority: u8`). First-class in ASL so ISRs appear cleanly as Event OBs across C++, ST, Ladder, and GRAFCET.
  - `OB_COMM_EVENT` (`OB50+`): Asynchronous bus/protocol event OBs (e.g., UART/TTL RX frame ready, I2C slave request, Modbus register write event).
- **Functions (`FC`) & Function Blocks (`FB`):**
  - `FC`: Stateless typed functions with explicit parameter directions (`IN | IN_OUT | OUT`) and explicit return type (`VOID` or any ASL type). Lexical scoping strictly enforced: parameters and locals **never** mutate shadowed globals or caller locals.
  - `FB`: Stateful Function Blocks usable uniformly across C++, ST, Ladder boxes, and GRAFCET actions.
- **First-Class Structural POU Bodies (ADR-026 / D1):**
  Every POU body is one of three canonical structural variants:
  ```rust
  pub enum PouBody {
      Statements(Vec<Stmt>),  // C++ / ST / MicroPython / Rust / Structured POU
      Network(LadderNetwork), // IEC 61131-3 LD structural IR (ordered rungs + typed cells)
      Chart(SfcChart),        // IEC 60848 SFC/GRAFCET structural IR (steps, transitions, actions, macro-steps)
  }
  ```
  - `dendri-vm` lowers `Network` and `Chart` **at load time**; **no synthetic temporaries (`__rail_N`, `__sfc_*`) are EVER persisted in ASL**.
  - C++/ST emission from a `Network`/`Chart` is a deterministic derived projection. Lifting from text recognizes only declared canonical patterns; non-liftable POUs stay intact as callable `Structured POU` boxes and are never flattened.
- Validator enforces safety rules per OB class (e.g., no blocking `WAIT_TIME` inside `OB_HW_INT` / `OB_CYCLIC_INT` -> `ERR_ISR_FORBIDDEN_OP`).

---

## 3. ASL Numeric Contract (ADC, 4–20 mA, PWM) (ADR-025)

ASL defines a single frozen fixed-point scale for all analog quantities; every language, board, and converter maps to and from it via exact integer arithmetic.

### 3.1 Constants and Codec Kinds
- **`ASL_ANALOG_FULL_SCALE`**, written **`M`**, is **`1_000_000_000`** ($10^9$). It is frozen (changing it requires a major schema version bump + migration).
- An **analog quantity** expresses the fraction of a channel's declared range in parts per billion: nominal domain `[0, M]` (unipolar) or `[-M, +M]` (bipolar).
- A **codec** relates native integer codes to that fraction via parameters `D` (divisions), `n_max` (largest native magnitude), and `kind`:

| Kind | Meaning | `D` | `n_max` | Native codes | Example |
|---|---|---|---|---|---|
| `binned` | Real ADC: `code = floor(fraction · 2^b)`; full scale is never reached exactly | $2^b$ | $D - 1$ | `0 .. n_max` | Uno 10-bit: `D = 1024, n_max = 1023` |
| `span` | Endpoint-inclusive scale: 0 and 100% are both codes | `n_max` | `n_max` | `0 .. n_max` | S7 nominal 0–10 V / 4–20 mA: `D = n_max = 27648` |
| `span_bipolar` | Symmetric endpoint-inclusive scale | `n_max` | `n_max` | `-n_max .. +n_max` | S7 ±10 V: `D = n_max = 27648` |

### 3.2 Normative Formulas (Exact Integer Arithmetic)
```text
Unipolar (binned, span):
  native → ASL :  a = ceil( n · M / D )                      (lower edge of the code's interval)
  ASL → native :  n = min( n_max , floor( a · D / M ) )      (truncation, as in a real ADC)

Bipolar (span_bipolar), magnitude-symmetric:
  native → ASL :  a = sign(n) · ceil( |n| · M / D )
  ASL → native :  n = sign(a) · min( n_max , floor( |a| · D / M ) )  (truncation toward zero)

Validity condition (hard error ERR_CODEC_RESOLUTION_EXCEEDS_SCALE):  D <= M
```
- The bipolar truncation rule is the default; each real module's behavior MUST be verified against its official manual and recorded in the board profile (`DEF-002`).
- Intermediate products MUST use at least 128-bit signed arithmetic (`i128`/`u128`); the validator MUST reject any codec where `n_max · M` could overflow the chosen width.

### 3.3 Mathematical Lemmas & Emitter Consequence
- **Lemma 1 (Exact Round Trip):** If $D \le M$, then for every integer $n \in [0, n_{\max}]$:
  $$\left\lfloor \frac{\lceil nM / D \rceil \cdot D}{M} \right\rfloor = n$$
  *Proof:* Let $a = \lceil nM / D \rceil$. Then $nM/D \le a < nM/D + 1$, so $n \le aD/M < n + D/M \le n + 1$, hence $\lfloor aD/M \rfloor = n$. By magnitude symmetry the same holds for `span_bipolar`. Necessity follows by pigeonhole ($D > M \implies$ collision). ∎
- **Lemma 2 (Composition Equals Platform Ratio):** Let $g = \gcd(D_1, D_2)$ and $\operatorname{lcm}(D_1, D_2) = D_1 D_2 / g$. If $\operatorname{lcm}(D_1, D_2) \le M$, then for every $n \in [0, n_{\max,1}]$:
  $$\left\lfloor \frac{\lceil nM / D_1 \rceil \cdot D_2}{M} \right\rfloor = \left\lfloor \frac{n D_2}{D_1} \right\rfloor$$
  *Proof:* Write $a = nM/D_1 + \delta$ with $0 \le \delta < 1$, so $a D_2 / M = n D_2 / D_1 + \delta D_2 / M$. Let $q = \lfloor n D_2 / D_1 \rfloor$ and $r = n D_2 / D_1 - q$. Since $n D_2 / D_1 = n(D_2/g)/(D_1/g)$, $r$ is a multiple of $g/D_1$ and $r \le 1 - g/D_1$. Since $\delta D_2 / M < D_2 / M \le g/D_1 \iff \operatorname{lcm}(D_1, D_2) \le M$, we have $q \le a D_2 / M < q + 1$. ∎
  (Sufficient, not necessary; for any pair with $\operatorname{lcm} > M$, the exhaustive pair test `T-NUM-2` is the proof obligation.)
- **Mandatory Clamp Rule:** The destination codec bounds the result:
  $$n_2 = \min\left(n_{\max,2},\; \left\lfloor \frac{n \cdot D_2}{D_1} \right\rfloor\right)$$
  The `min` clamp is **MANDATORY whenever source is `span` and destination is `binned`** (otherwise the top source code $n = D_1$ yields $D_2 = n_{\max,2} + 1$, verified across all `S7 27648 → binned` pairs). It MAY be omitted only when range analysis proves $\lfloor n D_2 / D_1 \rfloor \le n_{\max,2}$ for all $n \le n_{\max,1}$.
- **Emitter Consequence:** Emitters NEVER emit `M`. When translating a native read across platforms, they emit the clamped reduced rational (e.g., 12-bit → 10-bit is `n >> 2`), selecting 32-bit vs 64-bit intermediate width via range analysis.

### 3.4 Channel Ranges, Band Layer & Status
- Codecs and ranges are declared **per channel**, not globally per board.
- Current-input modules normalizing 4–20 mA map 4 mA → code `0` and 20 mA → nominal max (S7: `27648`). A 0–10 V input measuring a 4–20 mA loop across a 500 Ω shunt sees 4 mA as 2 V = 20% = `a = 200_000_000` → S7 code `5529` (`floor(5529.6)`): live zero is then program logic.
- **Nominal vs. Band Layer:** The nominal codec (§3.2) covers only the nominal interval. **Underrange (`< 0` / below nominal), overrange (`> M`, e.g., S7 codes above `27648` up to `+17.6%`), and bipolar inputs** are handled by the band layer using the same formulas on signed `i64` values with 128-bit intermediates restricted to channel-declared band intervals. Band limits and special diagnostic codes (overflow, wire-break) MUST come from the official module manual and be cited via `source_ref` (`DEF-002`).
- `ANALOG` (`i32`) holds `±2.147 · M`; the validator MUST reject any channel band limit that would overflow `i32` (`ERR_BAND_LIMIT_OVERFLOW`).
- Analog reads carry a separate **status channel** (`Ok | Underrange | Overrange | Fault`); fault states are never encoded as ordinary values.

### 3.5 Types, Intrinsics & Two-Layer Separation
- `DUTY` is **unsigned `u32` in `[0, M]`** (0–100% PWM duty). Bidirectional actuation (H-bridge, servo) uses `DUTY` + a direction pin, or a bipolar `ANALOG` command at the component level. Values `> M` clamp with `WARN_DUTY_CLAMPED`.
- Idiom mappings (Wave 1): `analogRead(A0)` ⇄ `ADC_CODE(A0)` (board default codec); ST `IW64` ⇄ `ADC_CODE(IW64)`. Runtime-changeable resolution (`analogReadResolution`) is a statement updating the channel codec; fixed-codec emitters MUST prove resolution static or emit explicit conversion.
- **Two Layers:** (1) **Codec** (§3.2): pure integer mapping between language-visible codes and ASL values, exhaustively testable; (2) **Transfer Function** (board profile): physical behavior of a code (e.g., AVR Fast PWM `(OCR+1)/256` with 0/255 special cases — tracked in `DEF-001`, verified against datasheets and core source).

### 3.6 Reference Codec (Illustrative; §3.2 Formulas Are Normative)
```rust
pub const ASL_ANALOG_FULL_SCALE: u64 = 1_000_000_000;

pub struct Codec { d: u64, n_max: u64 }

#[derive(Debug, PartialEq, Eq)]
pub enum CodecError { ResolutionExceedsScale, InvalidRange }

impl Codec {
    pub fn new(d: u64, n_max: u64) -> Result<Self, CodecError> {
        if d == 0 || n_max == 0 || n_max > d { return Err(CodecError::InvalidRange); }
        if d > ASL_ANALOG_FULL_SCALE { return Err(CodecError::ResolutionExceedsScale); }
        Ok(Self { d, n_max })
    }

    /// Unipolar native -> ASL: ceil(n * M / D). None outside nominal range.
    pub fn to_analog(&self, n: u64) -> Option<u64> {
        if n > self.n_max { return None; }
        let num = u128::from(n) * u128::from(ASL_ANALOG_FULL_SCALE);
        let d = u128::from(self.d);
        Some(((num + d - 1) / d) as u64)
    }

    /// Unipolar ASL -> native: min(n_max, floor(a * D / M)); inputs above M clamp to M.
    pub fn to_code(&self, a: u64) -> u64 {
        let a = a.min(ASL_ANALOG_FULL_SCALE);
        let n = (u128::from(a) * u128::from(self.d) / u128::from(ASL_ANALOG_FULL_SCALE)) as u64;
        n.min(self.n_max)
    }

    /// Bipolar (`span_bipolar`) native -> ASL, magnitude-symmetric.
    pub fn to_analog_signed(&self, n: i64) -> Option<i64> {
        let a = self.to_analog(n.unsigned_abs())? as i64;
        Some(if n < 0 { -a } else { a })
    }

    /// Bipolar ASL -> native, truncation toward zero.
    pub fn to_code_signed(&self, a: i64) -> i64 {
        let n = self.to_code(a.unsigned_abs()) as i64;
        if a < 0 { -n } else { n }
    }
}
```
*(Numeric audit evidence: exhaustive runs at `M = 10⁹` produced 0 round-trip errors for D ∈ {256, 1024, 4096, 27648 unipolar & bipolar, 65536, 2¹⁹, 2²⁰, 2²⁴} up to 29-bit lossless width, and 0 clamped-composition errors across all catalog pairs up to 16-bit sources and 380 ordered pairs with `lcm ≤ M`; `M = 10⁶` failed from 20 bits; `20%` is exact `200_000_000` and `50%` is exact `500_000_000`.)*

---

## 4. Statements & Canonical Expression Grammar (ADR-007)

### 4.1 Statement AST (`Stmt`)
Every statement carries a stable `NodeId` and `Span`, serialized as strictly-typed TOML tables inside `[[boards.program.pous]]`:
- `Assign { id, target: LValue, value: Expr }` — `LValue ∈ { Var(name), Index(base, idx), Field(base, field) }`.
- `Call { id, callee: String, args: Vec<CallArg> }` — positional and named (`IN := ...`) arguments.
- `FbInvoke { id, instance: LValue, inputs: Vec<(String, Expr)> }` — first-class stateful FB invocation (`ton1(IN := start, PT := T#500ms)`).
- `If { id, condition: Expr, then_body, elif_arms, else_body }`
- `Switch { id, scrutinee: Expr, cases: Vec<(Vec<CasePattern>, Vec<Stmt>)>, default: Option<Vec<Stmt>> }` — unifies C++ `switch`, IEC ST `CASE ... OF`, and Rust `match`.
- `While { id, condition: Expr, body: Vec<Stmt> }`
- `For { id, counter: String, from: Expr, to: Expr, step: Expr, inclusive: bool, body: Vec<Stmt> }` — losslessly represents IEC `FOR i := 1 TO 10 BY 2` (`inclusive = true`) and C++ `for (int i = 0; i < 10; i++)` (`inclusive = false`).
- `Break { id }` / `Continue { id }` — structured loop control flow (validated to appear only inside loops).
- `Return { id, value: Option<Expr> }` — validated against function return type; early `Return` inside `OB_MAIN` cleanly ends the current scan cycle.
- `Wait { id, duration: Expr }` — cooperative suspend point (§5).
- `Foreign { id, language, fidelity, text, pack_hint }` — explicit foreign construct (§6.2; replaces v1's removed `VendorExtension`).

### 4.2 Formal Canonical Expression Grammar (Pratt Parser + Canonical Printer)
Expressions serialize in `.toml` as a canonical string parsed by the official, 100% deterministic **Pratt expression parser** (`dendri-expr`) directly into the typed `Expr` AST at load time (with full byte spans):

| Precedence (High → Low) | Operators | Associativity |
|---:|---|---|
| 1 (Highest) | Primary: literals (`true`, `false`, `42`, `-5`, `16#FF`, `3.14`, `T#200ms`, `"hi"`), identifiers, `a[i]`, `s.field`, `fn(args...)`, `FOREIGN("lang", "text")`, `(expr)` | Left |
| 2 | Unary: `-x`, `+x`, `NOT x`, `BIT_NOT x` | Right |
| 3 | Multiplicative: `*`, `/`, `MOD` | Left |
| 4 | Additive: `+`, `-` | Left |
| 5 | Bitwise shift: `SHL`, `SHR` | Left |
| 6 | Relational: `<`, `<=`, `>`, `>=` (non-chaining!) | Non-assoc |
| 7 | Equality: `==`, `!=` (strictly lower than relational!) | Non-assoc |
| 8 | Bitwise: `BIT_AND`, `BIT_XOR`, `BIT_OR` | Left |
| 9 | Logical: `AND` (short-circuit) | Left |
| 10 | Logical: `XOR` | Left |
| 11 | Logical: `OR` (short-circuit) | Left |

- **String escaping:** Full standard escapes (`\"`, `\\`, `\n`, `\r`, `\t`, `\0`, `\u{...}`); unterminated strings are hard lex errors.
- **No chained comparisons:** `a < b < c` is a syntax error.
- **Canonical round-trip property:** `parse_expr(&print_expr(&e)) == Ok(e)` enforced by `proptest`.

---

## 5. Unified Cooperative Execution Semantics (ADR-008, ADR-018)

Every board's ASL `Program` organizes execution into **Organization Blocks (OBs)** and callable **POUs (`FC` / `FB`)**:
1. **Module Initialization:** Runs once in topological import order before `OB_STARTUP` (§6.1).
2. **`OB_STARTUP` (`OB100`):** Runs once at cold/warm boot.
3. **`OB_MAIN` (`OB1`):** Repeated main execution body (Arduino `loop()`, PLC cyclic `OB1`, primary Ladder `Network`, or top-level GRAFCET `Chart`), calling secondary/tertiary POUs.
4. **`OB_CYCLIC_INT` / `OB_HW_INT` / `OB_COMM_EVENT`:** Prioritized timed, hardware-edge (ISR), and communication/TTL-serial Event OBs.

`ExecutionKind` is a **non-exhaustive enum**: `Cooperative` (Arduino/MCU fiber with cycle-based virtual clock & cooperative `WAIT_TIME`/`WAIT_TIME_US` yields) and `PlcScan` (periodic scan with Process Image PII/PIQ) today, cleanly extensible to a future task-based kind when `asl-motion` arrives (`DEF-005`).

---

## 6. Modules, Capabilities, Packs, `Foreign` & Multi-Board Project Schema (ADR-006, ADR-028)

### 6.1 Module Model, Domain-Split Stdlib & Versioned Capabilities
- A **module** is a named unit of declarations (types, constants, `FC`, `FB`, data) with explicit `exports` and `requires`. A **library** is a set of modules with a manifest (name, version, license, content hash).
- Three origins: **project modules**, the **standard library**, and **packs** (§6.2).
- **Domain-Split Standard Library:**
  - `std.iec.*` (IEC 61131-3 timers, counters, edges, bistables, math, bitwise, conversions, communication FBs) is platform-neutral.
  - Platform modules (`std.arduino.*`, later `std.siemens.*`, `std.micropython.*`, …) are visible **only** when imported or implied by a frontend (e.g., `#include <Arduino.h>` implies `std.arduino.*`). A pure-PLC ST/Ladder project cannot accidentally resolve Arduino APIs (`ERR_UNRESOLVED_IMPORT`).
- Imports MUST be explicit and acyclic (`ERR_IMPORT_CYCLE`, `ERR_UNRESOLVED_IMPORT`). Nothing leaks into an implicit global namespace.
- **Versioned Capabilities:** Projects declare `requires = ["asl-core@1", ...]`. Future capabilities (`asl-oop` `DEF-004`, `asl-motion` `DEF-005`) are negotiated explicitly; any unknown capability MUST be rejected with `ERR_UNSUPPORTED_CAPABILITY` (never partially loaded).
- **Initialization Order:** Module top-level initializers execute once in topological dependency order prior to `OB_STARTUP`.
- **Storage:** Modules embed inside the single project `.toml` by default, or reference external `.dendrilib` files via `(name, version, sha256)` lock entries. Source file boundaries live in presentation metadata (`origin_file`) to reproduce user file layouts.

### 6.2 Packs & Three-Level `Foreign` Nodes (Replacing `VendorExtension`)
Unsupported or vendor-specific code MUST NEVER trigger a dead-end refusal. `Foreign` exists at **all three syntactic AST levels**:
1. **`Decl::Foreign`** — declarations / unsupported type definitions (e.g., `Servo s;`, C++ `class` with methods until `asl-oop`).
2. **`Stmt::Foreign`** — statements (with `NodeId`, e.g., `servo.write(angle);`).
3. **`Expr::Foreign`** — expressions (`FOREIGN("cpp", "servo.read()")` in canonical grammar). Its type comes from context (assignment/declaration/parameter) or an `F1` pack; an untyped `Expr::Foreign` outside expression-statement position raises `ERR_FOREIGN_UNTYPED`.

| Fidelity | Meaning | Capabilities |
|---|---|---|
| **F0 Preserved** | Unrecognized construct stored verbatim (`language`, `text`, `span`, attached comments) | Open, edit, save, re-emit in the **same** language with zero loss |
| **F1 Declared** | A hash-locked **pack** (`pack.toml`) provides signatures, types, FBs, header mappings | Type checking, I/O tag table, data-flow inspection |
| **F2 Modeled** | Pack supplies an ASL reference implementation or native physics/bus model | Full normal simulation |

- **Simulation Policy & Explicit Assumptions:** Default simulation policy on an unmodeled `Foreign` node is `halt` (stopping the board with a 4-part diagnostic: **what**, **where**, **why**, **what now**). Alternatively, the user may record an explicit assumption in `[[boards.assumptions]]` (`node`, optional `expr_ordinal`, `policy = "no_effect" | "return_value" | "user_model"`, `value`, `note`). Active assumptions are visibly badged in the UI, listed by `dendri validate --report` (`WARN_ASSUMPTION_ACTIVE`), and block release mode.
- **Cross-Language Export:** Other views display `Foreign` as a locked box. Exporting a file containing `Foreign` to a *different* language fails for that file (`ERR_FOREIGN_EXPORT_BLOCKED`); `--allow-foreign` emits a marked `"NOT TRANSLATED"` comment block. Same-language round-trips are never blocked.
- **Direct register access** (e.g., AVR `PORTB |= (1<<5)`) starts at `F0` in Wave 1 and becomes modelable later via board register maps.

### 6.3 Multi-Board Single-File `.toml` Project Layout
```toml
[project]
schema_version = "2.0"
asl_version    = "0.1.0"
requires       = ["asl-core@1"]
name           = "Uno-S7-Conveyor-Cell"
author         = "Engineering Team"
description    = "Arduino Uno sensor node communicating with Siemens S7-1200 PLC"

# Optional external reusable library lock entries (.dendrilib)
[[libraries]]
name    = "conveyor_motion_utils"
version = "1.0.0"
path    = "libs/conveyor_motion_utils.dendrilib"
sha256  = "<64-hex-sha256>"

# Optional hash-locked Packs supplying F1 declarations or F2 models (pack.toml)
[[packs]]
id      = "arduino-servo"
version = "1.0.0"
path    = "packs/arduino-servo/pack.toml"
sha256  = "<64-hex-sha256>"

[[comments]]
id        = "c_p001"
anchor    = { kind = "component", id = "btn_start" }   # project | board | component | wire
placement = "body"
text      = " S1 — start button"
origin    = { language = "asl", style = "visual" }     # line | block | paren_block | doc_line | doc_block | visual

[[boards]]
id                 = "plc_main"
profile            = "siemens-s7-1200-cpu-1214c"
authoring_language = "ladder"          # cpp | st | ladder | sfc | micropython | rust
execution          = "plc_scan"
scan_period_us     = 10_000
[boards.layout]
x = 2.0
y = 2.0
rotation = 0
mirror_x = false
mirror_y = false

[boards.program]
imports = ["std.iec.timers", "project.motor_ctrl", "conveyor_motion_utils.ramps"]
# Sole executable truth for plc_main: imports, [[hardware]], [[types]], [[data]],
# [[boards.program.modules]] (embedded multi-file project modules), [[boards.program.pous]].
# Statement-level Foreign inside a POU body:
#   { kind = "foreign", id = "n_0042", language = "cpp", fidelity = "F0",
#     text = "servo.write(angle);", pack_hint = "arduino-servo" }

# Embedded project modules (multi-file headers/TUs, multi-tab Arduino sketches, multi-POU ST)
[[boards.program.modules]]
name    = "motor_ctrl"
imports = ["std.iec.timers"]
exports = ["StartMotor", "MotorState"]
# Contains module-scoped types, data, and pous; initialized once in topological order before OB_STARTUP.

[[boards.comments]]
id        = "c_0007"
anchor    = { kind = "node", id = "n_0042" }
placement = "leading"                  # leading | trailing | dangling | header | inner | title | body
text      = " Drive actuator on start"
origin    = { language = "cpp", style = "line" }

[[boards.assumptions]]
node         = "n_0042"
expr_ordinal = 1                       # optional: targets an Expr::Foreign inside node
policy       = "no_effect"             # no_effect | return_value | user_model
note         = "Servo not modeled yet"

[boards.presentation]
# Optional NON-EXECUTABLE visual/layout metadata only (NEVER comments!):
eol_style = "lf"                       # "lf" | "crlf" (restored on same-language emit)
# rung grid coords, GRAFCET step x/y, blank_lines_before (0..=2 per NodeId)
[boards.presentation.origin_files]
# Maps module / POU NodeIds back to multi-file user layout on emit/export:
# mod_motor_ctrl = "motor_ctrl.h"
# pou_start_motor = "motor_ctrl.cpp"

[[components]]
id      = "btn_start"
profile = "cmd-pushbutton-no-start"
label   = "S1 Start"
[components.layout]
x = 34.0
y = 18.0
rotation = 0
[components.params]

[[wires]]
id        = "w1"
from      = "plc_main.Qa.0"
to        = "mcu_sensor.D2"
signal    = "digital"
waypoints = []
```

---

## 7. Comments — First-Class Verbatim Data (ADR-027)

Comments are **mandatory validated data**, stored outside `[boards.presentation]` in `[[boards.comments]]` (per-board program nodes) and top-level `[[comments]]` (project, board, component, wire anchors):
- **Verbatim storage:** `text` stores the exact characters between comment delimiters (stripped of `//`, `/* */`, `(* *)`, `#`) — zero trimming, zero Unicode NFC/NFD normalization, zero escaping. Internal line endings are LF (`\n`); original EOL style lives in `[boards.presentation].eol_style` and is restored on same-language emit.
- **Closed `origin.style` Enum (Strict Schema):**

| `origin.style` | Source Syntax & Meaning |
|---|---|
| `line` | Single-line prefix comments: `// ...` (C++, IEC ST 3rd ed., Rust) or `# ...` (MicroPython, ASL/TOML) |
| `block` | C-style block comments: `/* ... */` (C++, IEC ST 3rd ed., Rust) |
| `paren_block` | Classic IEC 61131-3 ST parenthesis block comments: `(* ... *)` — kept distinct from `block` so mixed-style ST files round-trip faithfully |
| `doc_line` | Documentation line comments: `/// ...` (`placement = "leading"`) or `//! ...` (`placement = "header"`) |
| `doc_block` | Documentation block comments: `/** ... */` (`placement = "leading"`) or `/*! ... */` (`placement = "header"`) |
| `visual` | Comments authored in visual UI surfaces (Ladder rung title/body, GRAFCET step/transition/action note, Tag Table note, Canvas annotation) |

- **Consecutive Line-Comment Grouping Rule:**
  - Contiguous `leading` line comments with matching `origin.style` (`line` or `doc_line`) and **no blank line between them** merge into a **single** `[[boards.comments]]` record whose `text` joins each line's body with `\n`. On emission, each `\n`-separated line receives the target line-comment prefix (or emits as one cohesive block comment when targeting a block-only dialect).
  - Any blank line between comment runs splits them into distinct comment records (preserving paragraph spacing via `blank_lines_before`).
  - `trailing` end-of-line comments are ALWAYS stored as individual single-line records anchored to their owning node.
- **Nested IEC 61131-3 Block Comments (`(* outer (* inner *) outer *)`):**
  - The IEC ST lexer MUST track nesting depth for `(* ... *)`. The outer delimiters are stripped and the nested text is stored verbatim in `text` with `style = "paren_block"`.
  - When projecting to a target whose block comments do NOT nest (e.g., C++ `/* ... */`), the emitter projects via line comments (`// ...`) or documented non-destructive fallback without mutating stored `text`.
- **Anchor & Placement Validation:**
  - `anchor` is a tagged enum: `{ kind = "node", id }` (resolves in board's `NodeId` space), `{ kind = "project" }`, or `{ kind = "board" | "component" | "wire", id }` (resolves in global unique instance IDs). Unresolved anchor -> `ERR_COMMENT_ANCHOR`.
  - `placement`: `leading | trailing | dangling | header | inner | title | body`. `token_ordinal: Option<u32>` is **REQUIRED iff `placement == "inner"`** and **FORBIDDEN otherwise** (`ERR_COMMENT_FIELD`). Non-node anchors allow only `header | body` (`ERR_COMMENT_FIELD`).
- **Relocation & Orphan Preservation:**
  - `inner` comments inside expressions relocate deterministically to the nearest enclosing statement boundary on cross-language emission (`INFO_COMMENT_RELOCATED`).
  - Deleting an anchored node MUST NEVER delete its comment: it becomes `dangling` on the parent scope (`INFO_COMMENT_ORPHANED`) and remains visible in the IDE comments panel.
- **Formatting & Blank Lines:** Code formatting is canonical per language; only comment text and optional `blank_lines_before: u8` (`0..=2`, presentation hint) are preserved.
- **Hazard-Safe Emission & Re-Import Idempotence:**
  - Each frontend implements `render_comment(text, placement) -> Rendered` and `matches_comment(rendered, text) -> bool` (target dialect comment syntax tracked in `DEF-006`).
  - Target hazards (trailing `\` in C++ line comments, `*/` or `*)` inside block comments, nested `(* ... *)` projected to C++) are resolved at emission time without mutating stored `text`.
  - Bidirectional control characters (Trojan-Source class) emit `WARN_COMMENT_BIDI` and render escaped in emitted code.
  - **Re-import idempotence:** On re-import, stored `text` is replaced ONLY if the parsed comment fails `matches_comment(rendered, stored_text)` — preventing degraded projections from overwriting the verbatim original.
- **Encoding & Display Charsets:**
  - ASL stores UTF-8. Per-target `comment_encoding` (`utf8_passthrough` default, `ascii_fold`, or `warn_only`) applies strictly to emitted projections with non-blocking `WARN_COMMENT_ENCODING`.
  - **Display string charsets:** `ERR_STRING_CHARSET` applies **strictly** to string literals routed to a display sink whose component profile declares a `charset` (e.g., HD44780 LCD ROM, `DEF-007`) — NEVER to UTF-8 `Serial`/UART byte streams.

---

## 8. Frozen Diagnostic & Error Code Contract

Every `Diagnostic` carries `{ code: ErrorCode, severity: Error | Warning | Info, board_id: Option<String>, span: Option<Span>, message: String, hint: Option<String> }`. **Every single `ErrorCode` variant MUST have at least one live producer and genuine negative fixture (zero dead codes, zero synthetic `fixture_inject`).**

| Code | Severity | Meaning |
|---|---|---|
| `ERR_TOML_SYNTAX` / `ERR_UNKNOWN_FIELD` / `ERR_UNSUPPORTED_VERSION` | Error | Structural / schema / version error |
| `ERR_DUPLICATE_ID` / `ERR_DUPLICATE_DECL` / `ERR_HARDWARE_PIN_CONFLICT` / `ERR_NAME_COLLISION` | Error | Duplicate instance/decl/pin or hardware↔data collision |
| `ERR_UNKNOWN_TYPE` / `ERR_TYPE_MISMATCH` / `ERR_INVALID_CAST` / `ERR_ARRAY_BOUNDS_CONST` / `ERR_UNKNOWN_FIELD_ACCESS` / `ERR_UNDECLARED_SYMBOL` | Error | Static type checking & name resolution |
| `ERR_CODEC_RESOLUTION_EXCEEDS_SCALE` | Error | Channel codec declares `D > M` ($10^9$) |
| `ERR_BAND_LIMIT_OVERFLOW` | Error | Channel underrange/overrange band overflows signed `i32` `ANALOG` |
| `ERR_CONST_WRITE` / `ERR_HARDWARE_LABEL_WRITE` | Error | Write to `CONST` or immutable hardware label |
| `ERR_UNRESOLVED_FUNCTION` / `ERR_ARITY_MISMATCH` / `ERR_RETURN_TYPE_MISMATCH` | Error | Call / function signature mismatch |
| `ERR_BREAK_OUTSIDE_LOOP` / `ERR_CONTINUE_OUTSIDE_LOOP` | Error | Loop control flow outside loop |
| `ERR_ISR_FORBIDDEN_OP` / `ERR_UNRESOLVED_ISR` / `ERR_RECURSION_DEPTH` | Error | Interrupt safety / call-depth violation |
| `ERR_IMPORT_CYCLE` / `ERR_UNRESOLVED_IMPORT` / `ERR_UNSUPPORTED_CAPABILITY` | Error | Module graph or unknown capability in `requires` |
| `ERR_FOREIGN_UNTYPED` / `ERR_FOREIGN_EXPORT_BLOCKED` | Error | Untyped `Expr::Foreign` or cross-language export of `Foreign` |
| `ERR_STRING_CHARSET` | Error | String literal sent to display sink violates component `charset` |
| `ERR_COMMENT_FIELD` / `ERR_COMMENT_ANCHOR` | Error | Invalid `token_ordinal`/placement or unresolved comment anchor |
| `ERR_UNKNOWN_PROFILE` / `ERR_UNKNOWN_PORT` / `ERR_PIN_CAPABILITY_MISMATCH` / `ERR_BAD_ENDPOINT` / `ERR_UNSAFE_VOLTAGE_DOMAIN` | Error / Warning | Project hardware/wiring integrity & voltage-domain mismatch |
| `WARN_INDETERMINATE_LEVEL` | Warning | Digital read of electrically indeterminate input level |
| `WARN_ASSUMPTION_ACTIVE` | Warning | Active simulation assumption on a `Foreign` node |
| `WARN_COMMENT_ENCODING` / `WARN_COMMENT_BIDI` | Warning | Comment encoding degraded or bidi controls escaped |
| `WARN_DUTY_CLAMPED` | Warning | `DUTY` value above `M` clamped to `M` |
| `INFO_COMMENT_RELOCATED` / `INFO_COMMENT_ORPHANED` | Info | `inner` comment relocated or deleted-anchor comment orphaned |
