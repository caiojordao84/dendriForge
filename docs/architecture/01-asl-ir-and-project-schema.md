# 01 — ASL v2 Intermediate Representation & Multi-Board Project Schema

> **Normative Specification** | Decisions locked in Round 2 (ADRs 005–008)  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md)

---

## 1. Design Philosophy: Foundation Stone + Rosetta Stone

ASL v2 (**Abstract Syntax Language**) is the **sole executable intermediate representation** of `dendriForge`. Every board program — whether authored in C++ (Arduino), IEC 61131-3 Structured Text, Ladder Diagram (LD), GRAFCET/SFC, MicroPython, or Rust — compiles to and projects from the exact same strictly-typed ASL v2 AST.

### Core Rules
1. **One AST in memory, zero escape hatches:** There is no `Expr::ExprText`, no duplicate `Expr::Lit`, and no untyped `String` initializers. Every expression, statement, declaration, and literal is statically typed and span-tracked.
2. **Spans everywhere:** Every AST node carries a `Span { start: u32, end: u32 }` (byte offsets into the active source or TOML document) so parser, typechecker, validator, and runtime errors map to exact file/line/column diagnostics.
3. **Stable Node IDs (`NodeId`):** Top-level declarations, statements, Ladder rungs/cells, and GRAFCET steps/transitions carry stable identifiers so non-executable presentation metadata (visual coordinates, wire waypoints, user comments) survives edits without duplicating executable logic.
4. **Strict TOML deserialization:** Every schema struct uses `#[serde(deny_unknown_fields)]`. Unknown keys are hard parse errors, never silently ignored.

---

## 2. ASL v2 Type System (ADR-005)

ASL v2 combines **IEC 61131-3 strong static typing** with **deterministic embedded semantics** (no pointers, no raw memory aliasing, no dynamic heap allocation, no generics in user code).

### 2.1 Primitive Scalar Types (Fixed Width & Exact Semantics)

| ASL Type | Bit Width | Semantics | Overflow Policy |
|---|---:|---|---|
| `BOOL` | 1 bit | `true` / `false` | N/A |
| `SINT` / `INT` / `DINT` / `LINT` | 8 / 16 / 32 / 64 | Two's-complement signed integer | Explicit wrapping (`wrapping_*`) or checked trap per runtime config (never silent saturating mismatch) |
| `USINT` / `UINT` / `UDINT` / `ULINT` | 8 / 16 / 32 / 64 | Unsigned integer | Explicit wrapping (`wrapping_*`) |
| `BYTE` / `WORD` / `DWORD` / `LWORD` | 8 / 16 / 32 / 64 | Bit-string (bitwise/shift/rotate) | Bitwise exact width |
| `REAL` / `LREAL` | 32 / 64 | IEEE-754 `f32` / `f64` | IEEE-754 (explicit `NaN`/`Inf` validation hooks) |
| `TIME` | 64 | Signed/unsigned duration in **microseconds** (`i64` / `u64` internal `time_us`) | Supports `T#500us`, `T#200ms`, `T#1s500ms`, `T#2m` |
| `STRING` | Bounded | UTF-8 fixed-capacity string (`STRING[N]`, default `N = 255`) | Deterministic truncation/error; zero unbounded OOM |

> **No silent cross-sign comparison bug:** Mixed numeric operations require explicit widening or explicit cast (`INT_TO_DINT`, `UINT_TO_REAL`, etc.). No runtime `Int(1) != Uint(1)` corruption can occur because all expressions are statically type-checked before execution.

### 2.2 Composite & User-Defined Types
- **Fixed-size Arrays:** `ARRAY[N] OF T` (0-indexed `0..N`, statically bounded; runtime bounds checks return a clean runtime diagnostic, never a Rust panic). Multidimensional arrays supported as `ARRAY[M] OF ARRAY[N] OF T`.
- **Flat Structs (`STRUCT`):** Named product types with typed fields (`struct.field` read/write). No recursive structs.
- **Enumerations (`ENUM`):** Named strongly-typed symbolic constants backed by an explicit integer width (default `DINT`).
- **First-Class Function Block (FB) Instances:** Standard IEC 61131-3 stateful blocks are real types in the type system:
  - Timers: `TON`, `TOF`, `TP` — fields `.IN`, `.PT`, `.Q`, `.ET`
  - Counters: `CTU`, `CTD`, `CTUD` — fields `.CU`, `.CD`, `.R`, `.LD`, `.PV`, `.Q`/`.QU`/`.QD`, `.CV`
  - Edge detectors: `R_TRIG`, `F_TRIG` — fields `.CLK`, `.Q`
  - Bistables: `SR`, `RS` — fields `.S1`/`.R` or `.S`/`.R1`, `.Q1`
  - User-defined Function Blocks (`FUNCTION_BLOCK`) supported cleanly in later wave using the exact same field-access & stateful-instance mechanism.

### 2.3 Storage Scopes
- `CONST` — Immutable compile-time constant (writes forbidden by validator & runtime).
- `VAR` — Standard persistent-across-scans volatile RAM variable (zeroed/re-initialized on reset).
- `RETAIN` — Preserved across warm/soft restarts; reset on cold restart.
- `PERSISTENT` — Preserved across warm restarts and power cycles (non-volatile EEPROM/NV image).
- `TEMP` — Re-initialized to default on every scan/function entry (strict lexical scope).

### 2.4 Hardware I/O Bindings (`[[hardware]]`)
Hardware bindings declare the **typed logical I/O interface** between a board's ASL program and physical pins on its board profile:
- `label: String` — Immutable symbolic identifier (cannot be assigned as a variable; validated against `data` and `functions` for collisions).
- `pin: String` — Canonical board port ID from the board profile (e.g., `"D13"`, `"A0"`, `"Ia.0"`, `"Qa.0"`, `"IW64"`), replacing fragile raw integer pin numbers.
- `kind`: `DigitalIn | DigitalOut | DigitalBidirectional | AnalogIn | AnalogOut | PwmOut | BusPin`
- `pull`: `None | PullUp | PullDown` (validated against the board profile's pin capabilities!).
- `active_low: bool` (optional electrical polarity hint, cleanly separated from `pull`).

### 2.5 Program Organization Units (POUs) & Organization Blocks (OBs) (ADR-018)
To bridge embedded C++/MCU patterns and industrial IEC 61131-3 / TIA Portal architectures cleanly without losing constructs across views, every board's ASL `Program` is structured into typed **Organization Blocks (OBs)** and reusable **POUs (`FC` / `FB`)**:
- **Organization Blocks (`[[pous]]` with `kind = "ob"`):**
  - `OB_STARTUP` (`OB100`): Runs once at cold/warm boot (maps to Arduino `setup()`, MicroPython init).
  - `OB_MAIN` (`OB1`): Main cyclic scan / `loop()`, which may invoke secondary/tertiary Ladder networks, GRAFCET charts, or functions.
  - `OB_CYCLIC_INT` (`OB30+`): Periodic timed interrupt OBs with exact microsecond period (e.g., deterministic 1 ms PID loop).
  - `OB_HW_INT` (`OB40+`, Event OBs): Hardware pin interrupts / ISRs (`pin_label`, `edge: Rising | Falling | Change | Low | High`, `priority: u8`). First-class in ASL so ISRs appear cleanly as Event OBs across C++, ST, Ladder, and GRAFCET!
  - `OB_COMM_EVENT` (`OB50+`): Asynchronous bus/protocol event OBs (e.g., UART/TTL RX frame ready, I2C slave request, Modbus register write event).
- **Functions (`FC`) & Function Blocks (`FB`):**
  - `FC`: Stateless typed functions with explicit parameter directions (`IN | IN_OUT | OUT`) and explicit return type (`VOID` or any ASL type). Lexical scoping strictly enforced: parameters and locals **never** mutate shadowed globals or caller locals.
  - `FB`: Stateful user-defined Function Blocks (plus built-in IEC FBs and communication FBs such as `UART_SEND`/`UART_RECV`, `I2C_XFER`, `SPI_XFER`, `MB_CLIENT`/`MB_SERVER`) usable uniformly from C++, ST, Ladder boxes, and GRAFCET actions.
- Validator enforces safety rules per OB class (e.g., no blocking `WAIT_TIME` inside `OB_HW_INT` / `OB_CYCLIC_INT` -> `ERR_ISR_FORBIDDEN_OP`).

---

## 3. Statements & Canonical Expression Grammar (ADR-007)

### 3.1 Statement AST (`Stmt`)
All statements are serialized as strictly typed TOML tables:
- `Assign { target: LValue, value: Expr }` — where `LValue` supports `Var(name)`, `Index(base, idx)`, and `Field(base, field)`.
- `Call { callee: String, args: Vec<CallArg> }` — supports positional and named (`IN := ...`) arguments.
- `FbInvoke { instance: LValue, inputs: Vec<(String, Expr)> }` — first-class stateful FB invocation (`ton1(IN := start, PT := T#500ms)`).
- `If { condition: Expr, then_body, elif_arms, else_body }`
- `Switch { scrutinee: Expr, cases: Vec<(Vec<CasePattern>, Vec<Stmt>)>, default: Option<Vec<Stmt>> }` — unifies C++ `switch`, IEC ST `CASE ... OF`, and Rust `match`.
- `While { condition: Expr, body: Vec<Stmt> }`
- `For { counter: String, from: Expr, to: Expr, step: Expr, inclusive: bool, body: Vec<Stmt> }` — losslessly represents both IEC `FOR i := 1 TO 10 BY 2` (`inclusive = true`) and C++ `for (int i = 0; i < 10; i++)` (`inclusive = false`).
- `Break` / `Continue` — real structured loop control flow (validated to appear only inside loops).
- `Return { value: Option<Expr> }` — validated against function return type; early `Return` inside `scan` cleanly ends the current scan cycle.
- `Wait { duration: Expr }` — cooperative suspend point (see §4).
- `VendorExtension { tag: String, payload: String }` — explicit, validated escape hatch for unsupported hardware drivers (never silently parsed with magic FAT strings).

### 3.2 Formal Canonical Expression Grammar (Pratt Parser + Canonical Printer)
To avoid both unreadable 40-line nested TOML tables and v1's broken `ExprText` hack, expressions serialize in `.toml` as a single canonical string parsed by an official, 100% deterministic **Pratt expression parser** (`dendri-expr`) directly into the typed `Expr` AST at load time (with full byte spans):

| Precedence (High → Low) | Operators | Associativity |
|---:|---|---|
| 1 (Highest) | Primary: literals (`true`, `false`, `42`, `-5`, `16#FF`, `3.14`, `T#200ms`, `"hi"`), identifiers, `a[i]`, `s.field`, `fn(args...)`, `(expr)` | Left |
| 2 | Unary: `-x`, `+x`, `NOT x`, `BIT_NOT x` | Right |
| 3 | Power / Multiplicative: `*`, `/`, `MOD` | Left |
| 4 | Additive: `+`, `-` | Left |
| 5 | Bitwise shift: `SHL`, `SHR` | Left |
| 6 | Relational: `<`, `<=`, `>`, `>=` (non-chaining!) | Non-assoc |
| 7 | Equality: `==`, `!=` (strictly lower than relational!) | Non-assoc |
| 8 | Bitwise: `BIT_AND`, `BIT_XOR`, `BIT_OR` | Left |
| 9 | Logical: `AND` (short-circuit) | Left |
| 10 | Logical: `XOR` | Left |
| 11 (Lowest) | Logical: `OR` (short-circuit) | Left |

- **String escaping:** Full standard escape handling (`\"`, `\\`, `\n`, `\r`, `\t`, `\0`, `\u{...}`); unterminated strings are hard lex errors.
- **No chained comparisons:** `a < b < c` is a syntax error (must write `(a < b) AND (b < c)`).
- **Canonical round-trip property:** For every valid AST `e`, `parse_expr(&print_expr(&e)) == Ok(e)` is enforced by property tests!

---

## 4. Unified Cooperative Execution Semantics (ADR-008 & ADR-018)

Every board's ASL `Program` organizes execution into **Organization Blocks (OBs)** and callable **POUs (`FC` / `FB`)**:

1. **`OB_STARTUP` (`OB100`):** Runs once at cold/warm boot (Arduino `setup()`, MicroPython init, PLC startup).
2. **`OB_MAIN` (`OB1`):** Repeated main execution body (Arduino `loop()`, PLC cyclic `OB1`, primary Ladder network, or top-level GRAFCET chart) which can call secondary/tertiary Ladder POUs, `FC`s, and `FB`s.
3. **`OB_CYCLIC_INT` / `OB_HW_INT` / `OB_COMM_EVENT`:** Prioritized timed, hardware-edge (ISR), and communication/TTL-serial Event OBs.

### Cooperative Wait (`WAIT_TIME` / `delay()`) vs. Cyclic Scan
- Each board executes on its own lightweight **cooperative VM fiber (continuation/program-counter stack)** managed by the master simulation scheduler (`time_us: u64`).
- When an Arduino/MCU board executes `WAIT_TIME(T#500ms)`, **only that board's VM yields** with `wake_at_us = now_us + 500_000`.
  - Its GPIO output pins **immediately reflect their current state** on the canvas and electrical solver (so `HIGH; WAIT 500ms; LOW; WAIT 500ms` visibly blinks!).
  - Its hardware ISRs remain armed and can fire during the wait.
  - Other boards in the same project (e.g., a Siemens S7-1200 PLC scanning every 10 ms) continue running uninterrupted in lockstep with the master clock.
- PLC boards execute their `scan` section atomically at their configured scan period (`scan_period_us`, default `10_000` µs = 10 ms) with an explicit **Process Image** (`Read Inputs -> Execute Scan -> Write Outputs`) unless configured for immediate I/O.

---

## 5. Multi-Board Single-File `.toml` Project Schema (ADR-006)

A `dendriForge` v2 project is a single self-contained `.toml` file (`schema_version = "2.0"`):

```toml
[project]
schema_version = "2.0"
asl_version = "0.1.0"
name = "Uno-S7-Conveyor-Cell"
author = "Engineering Team"
description = "Arduino Uno sensor node communicating with Siemens S7-1200 PLC"

# Multiple programmable boards co-simulated in one project
[[boards]]
id = "plc_main"
profile = "siemens-s7-1200-cpu-1214c"
authoring_language = "ladder"        # cpp | st | ladder | sfc | micropython | rust
scan_period_us = 10_000              # 10 ms PLC scan cycle
[boards.layout]
x = 2.0
y = 2.0
rotation = 0
mirror_x = false
mirror_y = false

[boards.program]
# Sole executable truth for plc_main (ASL v2 IR)
# ... hardware, types, data, init, scan, functions, isr ...

[boards.presentation]
# Optional non-executable visual metadata keyed by stable AST NodeId
# (Ladder rung grid coords, GRAFCET step x/y, comments)

[[boards]]
id = "mcu_sensor"
profile = "arduino-uno-r3"
authoring_language = "cpp"
[boards.layout]
x = 34.0
y = 2.0
rotation = 0
mirror_x = false
mirror_y = false

[boards.program]
# Sole executable truth for mcu_sensor (ASL v2 IR)

# Shared external physical components on the same canvas
[[components]]
id = "btn_start"
profile = "cmd-pushbutton-no-start"
label = "S1 Start"
[components.layout]
x = 34.0
y = 18.0
rotation = 0
[components.params]
# Strictly typed against component profile schema

# Physical wiring across ANY boards and components
[[wires]]
id = "w1"
from = "plc_main.Qa.0"
to = "mcu_sensor.D2"
signal = "digital"
waypoints = []
```

### Why This Eliminates v1's Sync Bugs
- **No `"board"` singleton:** Every endpoint is `<instance_id>.<port_id>` where `instance_id` resolves against `boards` OR `components` (enforced globally unique by the validator).
- **No triple-duplicated logic:** `boards[*].program` is the **only** executable IR stored on disk. `boards[*].presentation` holds only layout/annotation metadata keyed by AST `NodeId`, never competing executable statements.
- **Per-board language freedom:** `plc_main` can be edited in Ladder or GRAFCET while `mcu_sensor` is edited in C++, all simulated together deterministically in one project file.

---

## 6. Frozen Diagnostic & Error Code Contract

Every `Diagnostic` carries:
- `code: ErrorCode` (every single variant in the enum MUST have at least one producer + negative test — zero dead error codes!)
- `severity: Error | Warning | Info`
- `board_id: Option<String>`
- `span: Option<Span>`
- `message: String`
- `hint: Option<String>`

Mandatory static validation checks (executed by CLI, CI, **and live inside `dendri-editor`**):
1. Schema & structure (`ERR_TOML_SYNTAX`, `ERR_UNKNOWN_FIELD`, `ERR_UNSUPPORTED_VERSION`)
2. Unique identifiers (`ERR_DUPLICATE_ID`, `ERR_DUPLICATE_DECL`, `ERR_HARDWARE_PIN_CONFLICT`, `ERR_NAME_COLLISION`)
3. Type system (`ERR_UNKNOWN_TYPE`, `ERR_TYPE_MISMATCH`, `ERR_INVALID_CAST`, `ERR_ARRAY_BOUNDS_CONST`, `ERR_UNKNOWN_FIELD_ACCESS`, `ERR_UNDECLARED_SYMBOL`)
4. Mutability & scopes (`ERR_CONST_WRITE`, `ERR_HARDWARE_LABEL_WRITE`, `ERR_UNRESOLVED_FUNCTION`, `ERR_ARITY_MISMATCH`, `ERR_RETURN_TYPE_MISMATCH`)
5. Control flow & safety (`ERR_BREAK_OUTSIDE_LOOP`, `ERR_CONTINUE_OUTSIDE_LOOP`, `ERR_ISR_FORBIDDEN_OP`, `ERR_UNRESOLVED_ISR`, `ERR_RECURSION_DISALLOWED` or bounded call depth)
6. Project & hardware integrity (`ERR_UNKNOWN_PROFILE`, `ERR_UNKNOWN_PORT`, `ERR_PIN_CAPABILITY_MISMATCH`, `ERR_INVALID_WIRE_ENDPOINT`, `ERR_UNSAFE_VOLTAGE_DOMAIN` warning/error)
