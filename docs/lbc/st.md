# Language Binding Contract — IEC 61131-3 Structured Text (`st`)

> **Normative Specification** | Wave 1 (`asl-core@1`) — Crate `dendri-lang-st`  
> **Closes:** `DEF-006` (Comment-style dialect table per IEC ST target)  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`01-asl-ir-and-project-schema.md`](../architecture/01-asl-ir-and-project-schema.md), [`04-frontends-visual-and-sync.md`](../architecture/04-frontends-visual-and-sync.md), [`05-ide-and-roadmap.md`](../architecture/05-ide-and-roadmap.md)

---

## 0. Frontend Identity & Scope

- **Language ID:** `LanguageId::St` (`"st"`)
- **Paradigm:** `Paradigm::Textual`
- **Primary Target Profile:** `plc-1200-open-1214` (`execution = "plc_scan"`, `int_bits = 32`, `std.iec.*` + opt-in `std.plc.*`), while remaining fully usable on MCU targets (e.g., `arduino-uno-r3`) via `BoardContext`.
- **Differential Gate Compiler (`DEF-003`):** **RuSTy** (`plc`, pure-Rust IEC 61131-3 compiler targeting LLVM native code directly without an intermediate C transpilation step).
- **Permanent Ban on Invented Keywords & Magic Comments:** `dendri-lang-st` MUST NOT invent non-IEC keywords such as `SETUP` or `LOOP`, and MUST NOT emit or require machine magic comments (`(* HW: ... *)`, `// ASL_HW: ...`).

---

## 1. Accepted Grammar Subset (EBNF) & Rejected-Construct Diagnostics

### 1.1 Lexical Rules

- **Encoding:** Valid UTF-8 input. Line endings (`\n` or `\r\n`) are detected on parse and stored in `[boards.presentation].eol_style`; internal comment text always stores `\n`.
- **Case Sensitivity:** IEC 61131-3 keywords (`PROGRAM`, `FUNCTION`, `FUNCTION_BLOCK`, `VAR_INPUT`, `IF`, `THEN`, `ELSIF`, `END_IF`, `MOD`, `AND`, `OR`, `XOR`, `NOT`, `TRUE`, `FALSE`, etc.) MUST be accepted case-insensitively on parse and emitted in canonical UPPERCASE. User identifiers preserve their declared spelling while collision checks enforce case-insensitive uniqueness per IEC 61131-3.
- **Literals:**
  - **Boolean:** `TRUE`, `FALSE`
  - **Decimal integer:** `[0-9]([0-9_]*[0-9])?` (arbitrary-precision `i128` during lexing, range-checked against target type)
  - **Based integer:** `2#[01_]+`, `8#[0-7_]+`, `16#[0-9A-Fa-f_]+` (e.g., `2#1010`, `16#FF`, `16#0001`)
  - **Typed literal prefix (optional):** `<ScalarType>#<literal>` (e.g., `INT#-10`, `WORD#16#FF00`, `REAL#3.14`)
  - **Floating-point:** `[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9][0-9_]*)?`
  - **Duration (`TIME`):** `(T|t|TIME|time)#(-)?([0-9]+(d|h|m|s|ms|us))+` normalized to signed microseconds (`i64`, e.g., `T#500us`, `T#200ms`, `T#1s500ms`, `T#2m`)
  - **String:** Single-quoted `'...'` with IEC `$` escapes (`$$`, `$'`, `$L`/`$N` -> `\n`, `$R` -> `\r`, `$T` -> `\t`, `$P` -> `\x0C`, `$XX` hex byte). Double-quoted `WSTRING` literals are not in `asl-core@1` (see §1.3).

### 1.2 EBNF Grammar (`asl-core@1`)

```ebnf
CompilationUnit ::= { Directive | TypeBlock | GlobalVarBlock | PouDecl } ;

Directive       ::= "USING" QualifiedName ";" ;   (* Optional IEC Ed. 3 namespace import *)
QualifiedName   ::= Ident { "." Ident } [ ".*" ] ;

TypeBlock       ::= "TYPE" { TypeDef } "END_TYPE" ;
TypeDef         ::= Ident ":" ( StructDef | EnumDef ) ";" ;
StructDef       ::= "STRUCT" { FieldDecl } "END_STRUCT" ;
FieldDecl       ::= Ident ":" TypeSpec ";" ;
EnumDef         ::= [ TypeSpec ] "(" EnumElem { "," EnumElem } ")" ;
EnumElem        ::= Ident [ ":=" IntLiteral ] ;

GlobalVarBlock  ::= "VAR_GLOBAL" [ VarQual ] { VarDecl } "END_VAR" ;

PouDecl         ::= ProgramDecl | FunctionDecl | FunctionBlockDecl ;

ProgramDecl     ::= "PROGRAM" Ident { VarBlock } StmtList "END_PROGRAM" ;
FunctionDecl    ::= "FUNCTION" Ident ":" TypeSpec { VarBlock } StmtList "END_FUNCTION" ;
FunctionBlockDecl
                ::= "FUNCTION_BLOCK" Ident { VarBlock } StmtList "END_FUNCTION_BLOCK" ;

VarBlock        ::= ( "VAR_INPUT"
                    | "VAR_OUTPUT"
                    | "VAR_IN_OUT"
                    | "VAR" [ VarQual ]
                    | "VAR_TEMP"
                    | "VAR_GLOBAL" [ VarQual ] )
                    { VarDecl }
                    "END_VAR" ;
VarQual         ::= "CONSTANT" | "RETAIN" | "PERSISTENT" ;

VarDecl         ::= Ident [ "AT" DirectAddress ] ":" TypeSpec [ ":=" Expr ] ";" ;
DirectAddress   ::= "%" ( "I" | "Q" | "M" ) ( "X" | "B" | "W" | "D" | "L" )
                    Digits [ "." Digits ] ;

TypeSpec        ::= ScalarType
                  | "STRING" [ "[" IntLiteral "]" ]
                  | "ARRAY" "[" IntLiteral ".." IntLiteral "]" "OF" TypeSpec
                  | StdFbType
                  | Ident ;

StmtList        ::= { Stmt } ;
Stmt            ::= AssignStmt
                  | CallOrFbStmt
                  | IfStmt
                  | CaseStmt
                  | ForStmt
                  | WhileStmt
                  | RepeatStmt
                  | "EXIT" ";"
                  | "CONTINUE" ";"
                  | "RETURN" ";" ;

AssignStmt      ::= LValue ":=" Expr ";" ;
CallOrFbStmt    ::= LValue "(" [ ArgList ] ")" ";" ;
ArgList         ::= Arg { "," Arg } ;
Arg             ::= [ Ident ":=" ] Expr ;

IfStmt          ::= "IF" Expr "THEN" StmtList
                    { "ELSIF" Expr "THEN" StmtList }
                    [ "ELSE" StmtList ]
                    "END_IF" ";" ;

CaseStmt        ::= "CASE" Expr "OF"
                    { CasePattern { "," CasePattern } ":" StmtList }
                    [ "ELSE" StmtList ]
                    "END_CASE" ";" ;
CasePattern     ::= Expr [ ".." Expr ] ;

ForStmt         ::= "FOR" Ident ":=" Expr "TO" Expr [ "BY" Expr ] "DO"
                    StmtList
                    "END_FOR" ";" ;

WhileStmt       ::= "WHILE" Expr "DO" StmtList "END_WHILE" ";" ;
RepeatStmt      ::= "REPEAT" StmtList "UNTIL" Expr "END_REPEAT" ";" ;

LValue          ::= Ident { "[" Expr "]" | "." Ident } ;

(* Expressions follow the canonical precedence hierarchy of 01 §4.2,
   with ST tokens `=` -> `==`, `<>` -> `!=`, `&` -> `AND`, `**` rejected/Foreign. *)
Expr            ::= OrExpr ;
OrExpr          ::= XorExpr { "OR" XorExpr } ;
XorExpr         ::= AndExpr { "XOR" AndExpr } ;
AndExpr         ::= BitExpr { ( "AND" | "&" ) BitExpr } ;
BitExpr         ::= EqExpr { ( "BIT_AND" | "BIT_XOR" | "BIT_OR" ) EqExpr } ;
EqExpr          ::= RelExpr [ ( "=" | "<>" ) RelExpr ] ;       (* Non-chaining *)
RelExpr         ::= ShiftExpr [ ( "<" | "<=" | ">" | ">=" ) ShiftExpr ] ; (* Non-chaining *)
ShiftExpr       ::= AddExpr { ( "SHL" | "SHR" ) AddExpr } ;
AddExpr         ::= MulExpr { ( "+" | "-" ) MulExpr } ;
MulExpr         ::= UnaryExpr { ( "*" | "/" | "MOD" ) UnaryExpr } ;
UnaryExpr       ::= ( "-" | "+" | "NOT" | "BIT_NOT" ) UnaryExpr | PostfixExpr ;
PostfixExpr     ::= Primary { "[" Expr "]" | "." Ident | "(" [ ArgList ] ")" } ;
Primary         ::= Literal | Ident | "(" Expr ")" ;
```

### 1.3 Statement & POU Lowering Rules

- **Organization Blocks (`OB`):** Top-level `PROGRAM` declarations map to `PouKind::Ob`:
  - `PROGRAM OB_STARTUP` (or `OB100`) -> `ObKind::Startup`
  - `PROGRAM OB_MAIN` (or `OB1`, or sole `PROGRAM` when unambiguous) -> `ObKind::Main`
  - `PROGRAM OB_CYCLIC_<N>` / `OB30+` -> `ObKind::CyclicInt { period_us }` (period bound in board metadata)
  - `PROGRAM OB_HW_<N>` / `OB40+` -> `ObKind::HwInt { pin_label, edge, priority }`
  - `PROGRAM OB_COMM_<N>` / `OB50+` -> `ObKind::CommEvent { bus_id }`
- **Function Return Assignment:** Inside `FUNCTION MyFunc : T`, assignment `MyFunc := expr;` lowers to `StmtKind::Return { value: Some(expr) }` (or updates the return slot prior to `END_FUNCTION` when followed by trailing statements). Bare `RETURN;` inside `OB_MAIN` ends the current scan cycle (`StmtKind::Return { value: None }`).
- **`FOR` Loops:** `FOR i := a TO b BY s DO` lowers to `StmtKind::For { counter: "i", from: a, to: b, step: s, inclusive: true, body }` (default `step = 1` when `BY` is omitted).
- **`REPEAT ... UNTIL cond END_REPEAT;`:** Lowers deterministically into `StmtKind::While` with an explicit first-iteration guard or structured `If(cond) { Break }` at loop tail, preserving exact post-test semantics.
- **0-Based Arrays:** `ARRAY[0..N-1] OF T` maps 1:1 to ASL `TypeRef::Array { elem: T, len: N }`. Non-zero lower bounds (`ARRAY[1..N]`) are rejected unless explicitly normalized (see §1.4).

### 1.4 Rejected Constructs & Exact Frozen `ErrorCode` Mapping

Per the "Reject, Don't Guess" rule (`04` §2), every unsupported construct MUST either lift into a three-level `Foreign (F0)` node (for vendor/OOP/pointer/external constructs) or produce a spanned diagnostic using the frozen `ErrorCode` enum (`01` §8):

| Construct / Violation | Handling | Frozen `ErrorCode` |
|---|---|---|
| Unterminated string/comment, malformed literal, chained comparison (`a < b < c`), invalid token | Hard parse error | `ERR_TOML_SYNTAX` (lex/syntax diagnostic) |
| Unknown type name or unsupported `WSTRING`/`DATE`/`TOD`/`DT` | Hard error or `Decl::Foreign(F0)` | `ERR_UNKNOWN_TYPE` |
| Mixed-sign or implicit narrowing arithmetic (`INT + UINT`, `DINT := REAL`) | Hard type error | `ERR_TYPE_MISMATCH` |
| Invalid explicit conversion (e.g., `BOOL_TO_REAL`) | Hard type error | `ERR_INVALID_CAST` |
| Constant out-of-bounds array index (`arr[N]`) or non-zero lower bound `ARRAY[1..N]` | Hard error | `ERR_ARRAY_BOUNDS_CONST` |
| Access to nonexistent struct/FB/step field | Hard error | `ERR_UNKNOWN_FIELD_ACCESS` |
| Reference to undeclared variable or tag | Hard error | `ERR_UNDECLARED_SYMBOL` |
| Assignment to `VAR CONSTANT` variable | Hard error | `ERR_CONST_WRITE` |
| Assignment to input hardware tag (`%IX*`, `%IW*`) | Hard error | `ERR_HARDWARE_LABEL_WRITE` |
| Duplicate variable, POU, or FB declaration in same scope | Hard error | `ERR_DUPLICATE_DECL` |
| Collision between hardware tag label and data/POU symbol | Hard error | `ERR_NAME_COLLISION` |
| Call to undeclared function or unimported platform symbol (`std.arduino.*` in pure-PLC ST) | Hard error | `ERR_UNRESOLVED_FUNCTION` / `ERR_UNRESOLVED_IMPORT` |
| Wrong argument count in `FC` or `FB` call | Hard error | `ERR_ARITY_MISMATCH` |
| Return expression type mismatch in `FUNCTION` | Hard error | `ERR_RETURN_TYPE_MISMATCH` |
| `EXIT` or `CONTINUE` outside `FOR`/`WHILE`/`REPEAT` | Hard error | `ERR_BREAK_OUTSIDE_LOOP` / `ERR_CONTINUE_OUTSIDE_LOOP` |
| Blocking `WAIT` inside `OB_HW_INT` / `OB_CYCLIC_INT` | Hard error | `ERR_ISR_FORBIDDEN_OP` |
| Direct `%IX`/`%QX`/`%IW` address not present in board profile | Hard error | `ERR_UNKNOWN_PORT` |
| Direction mismatch (e.g., `AT %QX0.0` bound to `digital_in`-only port) | Hard error | `ERR_PIN_CAPABILITY_MISMATCH` |
| Two `AT` declarations binding the same physical port | Hard error | `ERR_HARDWARE_PIN_CONFLICT` |
| IEC Ed. 3 `METHOD`, `INTERFACE`, `EXTENDS`, `IMPLEMENTS` (deferred to `DEF-004` `asl-oop`) | Preserved verbatim as `Decl::Foreign(F0)` (same-language round-trip lossless) | `ERR_FOREIGN_EXPORT_BLOCKED` on cross-language export without `--allow-foreign` |
| `POINTER TO`, `REF_TO`, `ADR()`, `^` dereference, vendor pragma `{attribute ...}` | Preserved verbatim as `Decl`/`Stmt`/`Expr::Foreign(F0)` | `ERR_FOREIGN_UNTYPED` if `Expr::Foreign` lacks expected type and no `F1` pack applies |
| Cyclic `USING` / module import graph | Hard error | `ERR_IMPORT_CYCLE` |
| Project `requires` lists unknown capability | Hard error | `ERR_UNSUPPORTED_CAPABILITY` |

---

## 2. Per-Board Type Mapping

IEC 61131-3 scalar widths are fixed by the standard and map 1:1 to ASL v2 types independent of `[cpu] int_bits`. `[cpu] int_bits` (`16` on `arduino-uno-r3`, `32` on `plc-1200-open-1214`) affects only cross-language projection to C++ `int` and default integer literal inference when context is unconstrained (defaulting to `DINT` in ST per IEC 61131-3).

| IEC 61131-3 ST Type | ASL v2 Type | Width | Semantics & Constraints |
|---|---|---:|---|
| `BOOL` | `BOOL` | 1 bit | `TRUE` / `FALSE` |
| `SINT` / `INT` / `DINT` / `LINT` | `SINT` / `INT` / `DINT` / `LINT` | 8 / 16 / 32 / 64 | Signed two's-complement; wrapping or checked trap per VM config |
| `USINT` / `UINT` / `UDINT` / `ULINT` | `USINT` / `UINT` / `UDINT` / `ULINT` | 8 / 16 / 32 / 64 | Unsigned integer; explicit wrapping |
| `BYTE` / `WORD` / `DWORD` / `LWORD` | `BYTE` / `WORD` / `DWORD` / `LWORD` | 8 / 16 / 32 / 64 | Bit-string (exact width for `BIT_AND`, `BIT_OR`, `BIT_XOR`, `BIT_NOT`, `SHL`, `SHR`) |
| `REAL` / `LREAL` | `REAL` / `LREAL` | 32 / 64 | IEEE-754 `f32` / `f64`, no implicit FMA, `libm` transcendentals |
| `TIME` | `TIME` | 64 | Signed microseconds (`i64`) |
| `STRING` / `STRING[N]` | `STRING[255]` / `STRING[N]` | Bounded | UTF-8 fixed-capacity string (default `N = 255`); display sinks check `ERR_STRING_CHARSET` |
| `ANALOG` (ASL domain type) | `ANALOG` (`DINT` representation when targeting external ST compilers) | 32 | Fixed-point `[-2.147M, +2.147M]` at `M = 10⁹` (never emitted as raw `M` in native code projections; see §3) |
| `DUTY` (ASL domain type) | `DUTY` (`UDINT` representation) | 32 | Unsigned `[0, M]`; values `> M` clamp with `WARN_DUTY_CLAMPED` |
| `CODE<codec_id>` | `CODE<codec_id>` | Codec-bound | Native converter integer code bound to declared channel codec (`INT` for 16-bit signed PLC `IW64`/`IW66`) |
| `ARRAY[0..N-1] OF T` | `ARRAY[N] OF T` | Static | 0-indexed fixed-size array; runtime bounds check never panics |
| `STRUCT ... END_STRUCT` | `TypeDecl::Struct` | Flat | Non-recursive flat product type |
| `ENUM` (`TYPE E : (...)`) | `TypeDecl::Enum` | Explicit (default `DINT`) | Strongly typed named integer constants |
| `TON`, `TOF`, `TP`, `CTU`, `CTD`, `CTUD`, `R_TRIG`, `F_TRIG`, `SR`, `RS`, `UART_SEND`, `UART_RECV`, `I2C_XFER`, `SPI_XFER`, `MB_CLIENT`, `MB_SERVER`, user `FUNCTION_BLOCK` | `TypeRef::Fb` | Stateful | First-class stateful FB instances with typed `.IN`/`.PT`/`.Q`/`.ET`/`.CV`/`.QU`/`.QD`/`.Q1`/`.DONE`/`.NDR`/`.ERR` fields |

### Storage Scope Mapping

| ST Declaration Block | ASL `StorageScope` / `ParamDirection` | Semantics |
|---|---|---|
| `VAR_INPUT` | `ParamDirection::In` | Caller-supplied read-only input parameter |
| `VAR_OUTPUT` | `ParamDirection::Out` | Callee-written output parameter |
| `VAR_IN_OUT` | `ParamDirection::InOut` | Reference-passed read/write parameter |
| `VAR` (in `PROGRAM` / `FB` / `VAR_GLOBAL`) | `StorageScope::Var` | Persistent across scans in volatile RAM; re-initialized on reset |
| `VAR CONSTANT` / `VAR_GLOBAL CONSTANT` | `StorageScope::Const` | Compile-time constant; writes raise `ERR_CONST_WRITE` |
| `VAR RETAIN` / `VAR_GLOBAL RETAIN` | `StorageScope::Retain` | Retained across warm restart; cleared on cold restart |
| `VAR PERSISTENT` / `VAR_GLOBAL PERSISTENT` | `StorageScope::Persistent` | Retained across warm restarts and power cycles |
| `VAR_TEMP` (and locals inside `FUNCTION`) | `StorageScope::Temp` | Re-initialized to default value on every scan / function invocation |

---

## 3. Numeric Codecs & Intrinsic Mapping

All analog channels conform strictly to `01-asl-ir-and-project-schema.md` §3 (`M = 1_000_000_000`, 128-bit intermediate arithmetic).

### 3.1 Normative ST Idioms

- **Analog Input Read:** Reading a direct-addressed analog input tag declared with `PressRaw AT %IW64 : INT;` lifts to `ADC_CODE(PressRaw)` bound to the channel's declared codec in the board profile:
  - `plc-1200-open-1214` port `IW64` (`%IW64`, `0–10 V`): codec `plc_span_27648` (`kind = "span"`, `D = 27648`, `n_max = 27648`).
  - `plc-1200-open-1214` port `IW66` (`%IW66`, `±10 V`): codec `plc_bipolar_27648` (`kind = "span_bipolar"`, `D = 27648`, `n_max = 27648`, `bipolar_truncation = "toward_zero"`).
- **Explicit Scaling Conversions:**
  - `CODE_TO_ANALOG(raw, 'plc_span_27648')` converts native code `n` -> `ANALOG` via `ceil(n * M / D)` (or `sign(n) * ceil(|n| * M / D)` for `span_bipolar`).
  - `ANALOG_TO_CODE(val, 'plc_span_27648')` converts `ANALOG` -> native code via `min(n_max, floor(a * D / M))` (truncation toward zero for bipolar).
  - `ANALOG_TO_DUTY(val)` converts `[0, M]` `ANALOG` to unsigned `DUTY`, clamping `> M` with `WARN_DUTY_CLAMPED`.
  - Optional `std.plc.scale` (`NORM_X`, `SCALE_X`) operates in `REAL` when explicitly imported.
- **Worked Reference (`T-NUM-5`):** A 4–20 mA sensor across a 500 Ω shunt produces `2.0 V` at 4 mA on a `0–10 V` channel (`IW64`), yielding exact normalized fraction `20% = 200_000_000` ppb and PLC integer code `floor(200_000_000 * 27648 / 1_000_000_000) = 5529`.

### 3.2 Band Layer & Separate Status Channel (`DEF-002`)

Nominal codes (`0..27648` unipolar; `-27648..+27648` bipolar) map via the nominal codec. Extended industrial band codes on `plc-1200-open-1214`:
- `IW64`: `underrange_min = -4864`, `overrange_max = 32511`, `overflow_code = 32767`, `wire_break_code = -32768`.
- `IW66`: `underrange_min = -32512`, `overrange_max = 32511`, `overflow_code = 32767`, `wire_break_code = -32768`.

Every channel codec MUST satisfy `D <= M` (`ERR_CODEC_RESOLUTION_EXCEEDS_SCALE`) and every band limit MUST fit inside signed `i32` `ANALOG` (`ERR_BAND_LIMIT_OVERFLOW`). Channel fault/range health is exposed via the separate `AnalogStatus` channel (`Ok | Underrange | Overrange | Fault`) and is never conflated with ordinary numeric values.

### 3.3 Cross-Platform Reduced-Rational Emission Rule (NEVER Emit `M`)

When emitting ST for an ASL program whose source channel uses codec $(D_1, n_{\max,1})$ and destination target uses codec $(D_2, n_{\max,2})$:
1. The ST emitter **MUST NEVER** emit the internal constant `1_000_000_000` (`M`).
2. Compute $g = \gcd(D_1, D_2)$, $D_{1,\text{red}} = D_1 / g$, $D_{2,\text{red}} = D_2 / g$.
3. Select the intermediate integer width by range analysis:
   - If $n_{\max,1} \cdot D_{2,\text{red}} < 2^{32}$, use 32-bit `UDINT` (or `DINT` for bipolar).
   - Otherwise, use 64-bit `ULINT` (or `LINT` for bipolar).
4. Emit the clamped reduced rational `MIN(n_max2, (n * D2_red) / D1_red)` (omitting `MIN` ONLY when static range analysis proves $\lfloor n_{\max,1} \cdot D_{2,\text{red}} / D_{1,\text{red}} \rfloor \le n_{\max,2}$; the clamp is **MANDATORY** whenever source is `span` and destination is `binned`).
   - Example (`arduino-uno-r3` 10-bit `D1=1024` -> `plc-1200-open-1214` `D2=27648`): `g = 1024`, `D1_red = 1`, `D2_red = 27`, `1023 * 27 = 27621 <= 27648` -> emits `DINT_TO_INT(INT_TO_DINT(raw) * 27)` (32-bit, no clamp needed).
   - Example (`plc-1200-open-1214` `D1=27648` -> `arduino-uno-r3` 10-bit `D2=1024`, `n_max2=1023`): `g = 1024`, `D1_red = 27`, `D2_red = 1` -> emits `MIN(1023, raw / 27)` (mandatory clamp for `n = 27648 -> 1024 -> 1023`).
5. Dynamic resolution changes (`ANALOG_READ_RESOLUTION` lifted from MCU C++) MUST either be proven static per channel prior to ST emission or lower to an explicit runtime-selected reduced-rational conversion branch.

---

## 4. Comment Mapping & Authoritative `DEF-006` ST Comment Dialect Table

Comments are first-class verbatim data (`[[boards.comments]]`, ADR-027). `dendri-lang-st` MUST preserve verbatim text between comment delimiters with zero trimming, zero Unicode normalization, and LF internal line endings (restoring `[boards.presentation].eol_style` on same-language emit).

### 4.1 Normative ST Comment Dialect Table (`DEF-006`)

`BoardContext.st_dialect` selects one of four closed `StCommentDialect` variants, driving `LanguageCapabilities`:

| `StCommentDialect` Variant | Target Standard / Toolchain | `// ...` (`Line`) | `(* ... *)` (`ParenBlock`) | Nested `(* (* *) *)` | `/* ... */` (`Block`) | `supported_comment_styles` |
|---|---|---|---|---|---|---|
| `Iec61131Ed3` *(default)* | IEC 61131-3:2013 (3rd ed.) | Supported | Supported | **Yes** | Supported | `[Line, ParenBlock, Block]` |
| `RuSTy` | RuSTy `plc` compiler (Phase 6b gate, `DEF-003`) | Supported | Supported | **Yes** | Supported | `[Line, ParenBlock, Block]` |
| `StrictEd2` | IEC 61131-3:2003 (2nd ed. strict) | **No** | Supported | **No** | **No** | `[ParenBlock]` |
| `ClassicPlc` | Legacy industrial PLC ST compilers | Supported | Supported | **No** | **No** | `[Line, ParenBlock]` |

### 4.2 Lexing, Grouping & Nesting Rules

- **Nested `(* ... *)` Lexer:** Under `Iec61131Ed3` and `RuSTy`, the lexer MUST track nesting depth for `(* ... *)`. Only the outermost `(*` and `*)` delimiters are stripped; any inner `(* ... *)` pairs are stored verbatim in `Comment.text` with `origin.style = "paren_block"`. Under `StrictEd2` and `ClassicPlc`, the first `*)` terminates the comment.
- **Distinct `block` vs `paren_block`:** `/* ... */` stores `origin.style = "block"`; `(* ... *)` stores `origin.style = "paren_block"`, guaranteeing faithful same-language round-trips of mixed-style ST files.
- **Consecutive Line-Comment Grouping:** Contiguous `leading` `//` comments with no intervening blank line merge into a single `Comment` record with lines joined by `\n`. Any blank line splits adjacent records (spacing recorded in `blank_lines_before: 0..=2`). `trailing` comments are always single-line records.

### 4.3 `render_comment`, Projection-Only Hazard Avoidance & Re-Import Idempotence

All degradations apply **strictly to the emitted projection** and MUST NEVER mutate stored `Comment.text`:

1. **Trojan-Source Bidi Controls (`WARN_COMMENT_BIDI`):** Any Unicode bidirectional control character (`U+061C`, `U+200E`, `U+200F`, `U+202A..=U+202E`, `U+2066..=U+2069`) emits `WARN_COMMENT_BIDI` and renders escaped as `\u{XXXX}` in emitted ST.
2. **Target `comment_encoding` (`WARN_COMMENT_ENCODING`):**
   - `Utf8Passthrough` *(default)*: emits UTF-8 verbatim.
   - `AsciiFold`: replaces non-ASCII characters in the emitted projection with deterministic ASCII transliteration or `\u{XXXX}` escapes and emits non-blocking `WARN_COMMENT_ENCODING`.
   - `WarnOnly`: emits UTF-8 unchanged and raises non-blocking `WARN_COMMENT_ENCODING` when non-ASCII characters are present.
3. **Delimiter Hazard Matrix by Dialect:**
   - **Dialect supports `//` (`Iec61131Ed3`, `RuSTy`, `ClassicPlc`):**
     - `Line` / `DocLine` renders with `//` per `\n`-delimited line (or `(* ... *)` if `placement == Inner`).
     - `ParenBlock` renders as `(* ... *)` directly when safe. If `text` contains unbalanced `*)`, or contains nested `(*` / `*)` under non-nesting `ClassicPlc`, `render_comment` degrades the projection to `//` line comments per line.
     - `Block` / `DocBlock` renders as `/* ... */` under `Iec61131Ed3`/`RuSTy` when `text` does not contain `*/`; if `text` contains `*/`, or under `ClassicPlc` (which lacks `/* */`), `render_comment` degrades to `//` line comments.
   - **Block-only dialect (`StrictEd2`, `(* ... *)` non-nested ONLY):**
     - All styles (`Line`, `DocLine`, `Block`, `DocBlock`, `ParenBlock`, `Visual`) render inside `(* ... *)`.
     - If `text` contains `*)` (or `(*` that would confuse strict scanners), `render_comment` applies the documented non-destructive delimiter escape `*)` -> `* )` (or `*\u{29}`) solely in the emitted projection so the output compiles cleanly without truncating the comment.
4. **Re-Import Idempotence (`matches_comment` — Gate `C4`):**
   - `matches_comment(rendered_parsed_text, stored_text)` returns `true` iff `rendered_parsed_text == stored_text` OR `rendered_parsed_text` equals the exact deterministic output of `render_comment(stored_text, ...)` stripped of outer delimiters.
   - On re-import, stored `Comment.text` is overwritten ONLY when `matches_comment` returns `false`, ensuring degraded projections (`StrictEd2` escaping, `AsciiFold`, bidi escaping, line-comment fallback) NEVER corrupt the verbatim original.
5. **Relocation & Orphan Preservation (`C2`):**
   - `inner` comments (requiring `token_ordinal`) emitted into a position where ST cannot place an inline comment relocate deterministically to the nearest enclosing statement boundary with `INFO_COMMENT_RELOCATED`.
   - Deleting an anchored AST node transitions its comments to `placement = "dangling"` on the parent POU/module scope with `INFO_COMMENT_ORPHANED`.

---

## 5. Module, Import & Pack Mapping

- **POU & Module Organization:** Multiple POUs (`PROGRAM`, `FUNCTION`, `FUNCTION_BLOCK`) and `TYPE` blocks in a single ST source unit map to `[boards.program]` and embedded `[[boards.program.modules]]`. Multi-file ST layouts round-trip via `[boards.presentation.origin_files]`.
- **Topological Initialization Order:** Embedded module `VAR` / `VAR_GLOBAL` initializers execute once in topological import order prior to `OB_STARTUP` (`01` §6.1). Cyclic imports fail validation with `ERR_IMPORT_CYCLE`.
- **Domain-Split Standard Library Isolation:**
  - `std.iec.*` (`std.iec.timers`, `std.iec.counters`, `std.iec.edges`, `std.iec.bistables`, `std.iec.math`, `std.iec.convert`, `std.iec.comm`) is available by default in IEC ST.
  - Platform modules (`std.plc.*`, e.g., `std.plc.scale`) are visible ONLY when explicitly imported via `USING std.plc.scale;` or `[boards.program].imports`.
  - MCU platform modules (`std.arduino.*`) are NEVER implicitly in scope; referencing `DIGITAL_READ` or `MILLIS` in a pure-PLC ST project raises `ERR_UNRESOLVED_IMPORT` / `ERR_UNRESOLVED_FUNCTION`.
- **Hash-Locked Packs (`DEF-008`) & Three-Level `Foreign`:**
  - `assets/packs/plc-modbus-tcp.pack.toml` supplies `F1 Declared` signatures for `MB_CLIENT` and `MB_SERVER` (covering Modbus TCP FC1–FC6, FC15, FC16 on `plc-1200-open-1214` `ETH0`).
  - Unrecognized vendor FBs or Ed. 3 OOP constructs preserve as `Decl`/`Stmt`/`Expr::Foreign(F0)`.
  - Cross-language export of any file containing `Foreign` nodes is blocked with `ERR_FOREIGN_EXPORT_BLOCKED` unless `--allow-foreign` (`BoardContext.allow_foreign_export = true`) is set, in which case the emitter outputs a clearly demarcated `(* NOT TRANSLATED [<lang>/<fidelity>]: <text> *)` block. Same-language ST round-trips are never blocked.

---

## 6. Hardware Address & Tag Mapping

Hardware bindings flow strictly through `[[boards.program.hardware]]` paired with the active board profile's `[[ports]]` table (`iec_address`). Machine magic comments are forbidden (`04` §4.1).

### 6.1 Bidirectional `iec_address` Mapping (`plc-1200-open-1214`)

| ST `AT` Declaration | `iec_address` | Board Port ID | ASL `HardwareKind` | Channel Codec |
|---|---|---|---|---|
| `StartBtn AT %IX0.0 : BOOL;` | `%IX0.0` .. `%IX0.7` | `Ia.0` .. `Ia.7` | `DigitalIn` | N/A |
| `MotorOut AT %QX0.0 : BOOL;` | `%QX0.0` .. `%QX0.1` | `Qa.0` .. `Qa.1` | `DigitalOut` / `PwmOut` | `plc_span_27648` (PWM) |
| `LampOut AT %QX0.2 : BOOL;` | `%QX0.2` .. `%QX0.7` | `Qa.2` .. `Qa.7` | `DigitalOut` | N/A |
| `PressRaw AT %IW64 : INT;` | `%IW64` | `IW64` | `AnalogIn` | `plc_span_27648` (`0..10 V`) |
| `BipolarRaw AT %IW66 : INT;` | `%IW66` | `IW66` | `AnalogIn` | `plc_bipolar_27648` (`±10 V`) |

- **Parse Rule:** Every `VarDecl` with an `AT %...` clause looks up `BoardContext.iec_to_port`. Missing entries raise `ERR_UNKNOWN_PORT`; incompatible pin capabilities raise `ERR_PIN_CAPABILITY_MISMATCH`; duplicate bindings on the same port raise `ERR_HARDWARE_PIN_CONFLICT`.
- **Emit Rule:** Every `HardwareBinding` whose port has a mapping in `BoardContext.port_to_iec` emits as `<label> AT <iec_address> : <ST_TYPE>;` inside `VAR_GLOBAL`. On MCU boards without `iec_address` entries (e.g., `arduino-uno-r3`), tags emit as `VAR_GLOBAL` symbols reconciled deterministically against the Board I/O Tag Table.

---

## 7. Mandatory Round-Trip Class — `LOSSLESS_ASL` & Conformance Suite

`dendri-lang-st` MUST satisfy **`LOSSLESS_ASL`**:

1. **AST Identity:** For every valid `AslModule` / `BoardProgram` within the `asl-core@1` ST subset:
   $$\text{parse}(\text{emit}(M, \text{ctx}), \text{ctx}) \equiv M$$
   modulo freshly computed byte `Span` offsets.
2. **Comment Conservation (`C1–C5`):** Verbatim comment text multisets are strictly invariant across ST ↔ ASL ↔ C++/Ladder/GRAFCET chains (modulo documented `inner` relocation). Code formatting is canonical (4-space indentation, uppercase IEC keywords, explicit `;`); only verbatim comment text, `eol_style`, and `blank_lines_before: 0..=2` are preserved.
3. **Mandatory Conformance Gates (`04` §2, `05` §4):**
   - `proptest` verifying `LOSSLESS_ASL` across generated POUs, declarations, statements, expressions, and FB invocations.
   - Full `C1–C5` comment conservation suite including the hostile corpus (`*/`, `*)`, nested `(* (* *) *)`, trailing `\`, CRLF, combining marks, emoji, bidi controls, very long lines) across all 4 `StCommentDialect` targets.
   - Numeric codec, `span -> binned` clamp, and reduced-rational emitter tests verifying `M` never appears in emitted ST.
   - Three-level `Foreign` preservation (`Decl`, `Stmt`, `Expr`) and `--allow-foreign` export tests.
   - At least one genuine negative source fixture per produced `ErrorCode` asserting exact code and byte `Span`.
   - **Phase 6b Differential Gate:** Emitted ST (including hostile comments `C3`) compiles natively via RuSTy (`plc`, dialect `StCommentDialect::RuSTy`) and matches `dendri-vm` execution traces bit-for-bit.

---

## 8. `LanguageCapabilities` & Feature Matrix Contract

```rust
// Normative capability profile for `dendri-lang-st` (default dialect: Iec61131Ed3)
LanguageCapabilities {
    supported_comment_styles: match ctx.st_dialect {
        StCommentDialect::Iec61131Ed3 | StCommentDialect::RuSTy => vec![
            CommentStyle::Line,
            CommentStyle::ParenBlock,
            CommentStyle::Block,
        ],
        StCommentDialect::StrictEd2 => vec![CommentStyle::ParenBlock],
        StCommentDialect::ClassicPlc => vec![CommentStyle::Line, CommentStyle::ParenBlock],
    },
    supports_nested_paren_comments: matches!(
        ctx.st_dialect,
        StCommentDialect::Iec61131Ed3 | StCommentDialect::RuSTy
    ),
    supports_line_comments: !matches!(ctx.st_dialect, StCommentDialect::StrictEd2),
    comment_encoding: ctx.comment_encoding, // Utf8Passthrough | AsciiFold | WarnOnly
    capabilities: vec!["asl-core@1".to_owned()],
    st_dialect: Some(ctx.st_dialect),
}
```

- **Auto-Generated Feature Matrix:** The published ST feature-support matrix MUST be generated directly from the `dendri-lang-st` conformance test suite via `dendri-frontend-core::conformance` and MUST NOT be maintained by hand (`04` §2).
