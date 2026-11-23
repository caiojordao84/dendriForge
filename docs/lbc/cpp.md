# Language Binding Contract — Arduino C++ Subset (`cpp`)

> **Normative Specification** | Wave 1 (`asl-core@1`) — Crate `dendri-lang-cpp`  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`01-asl-ir-and-project-schema.md`](../architecture/01-asl-ir-and-project-schema.md), [`04-frontends-visual-and-sync.md`](../architecture/04-frontends-visual-and-sync.md), [`05-ide-and-roadmap.md`](../architecture/05-ide-and-roadmap.md)

---

## 0. Frontend Identity & Scope

- **Language ID:** `LanguageId::Cpp` (`"cpp"`)
- **Paradigm:** `Paradigm::Textual`
- **Primary Target Profile:** `arduino-uno-r3` (`ATmega328P`, `16 MHz`, `5 V`, `[cpu] int_bits = 16`, `execution = "cooperative"`, `std.arduino.*` + `std.iec.*`), while supporting 32-bit MCU targets (`[cpu] int_bits = 32`) and cross-projection with `plc-1200-open-1214`.
- **Differential Gate Toolchains (`DEF-003`):** Host `g++` against the deterministic Arduino API shim (virtual clock + pin-event log) on every CI run, nightly compile verification against `arduino-cli`, and `avr-gcc` + `simavr` as the independent AVR ground-truth oracle for 16-bit `int`, ADC, and PWM behavior.
- **Permanent Ban on Machine Magic Comments:** All v1 round-trip cheat comments (`// ASL_HW:`, `// ASL_DATA:`, `/* ASL_HW: ... */`) are **permanently banned** (`04` §4.1). Hardware bindings flow strictly through the Board I/O Tag Table (`[[boards.program.hardware]]`) reconciled automatically with idiomatic Arduino pin constants and `pinMode` calls.

---

## 1. Accepted Grammar Subset (EBNF) & Rejected-Construct Diagnostics

### 1.1 Lexical Rules

- **Encoding:** Valid UTF-8 input. Detected EOL style (`\n` vs `\r\n`) is stored in `[boards.presentation].eol_style` and restored on same-language emit; stored comment bodies always normalize to `\n`.
- **Case Sensitivity:** C++ keywords, Arduino APIs, and user identifiers are strictly case-sensitive.
- **Supported Preprocessor Subset:**
  - `#include <Header.h>` and `#include "local.h"` (see §5).
  - Simple object-like numeric/pin/boolean constant macros (`#define LED_PIN 13`, `#define THRESHOLD 512`), lifted deterministically to `CONST` declarations or pin-label bindings.
  - Arbitrary token-pasting (`##`), stringizing (`#`), variadic/function-like macros, and conditional compilation (`#ifdef`/`#if`) preserve verbatim as `Decl::Foreign(F0)` / `Stmt::Foreign(F0)` rather than being silently stripped.
- **Literals:**
  - **Boolean:** `true`, `false`, `HIGH` (`true`), `LOW` (`false`)
  - **Integer:** decimal (`42`), hex (`0xFF`), binary (`0b1010`), octal (`077`) with optional `u`/`U`/`l`/`L`/`ul`/`UL`/`ll`/`LL` suffixes
  - **Floating-point:** `[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?[fF]?`
  - **Character:** single ASCII/escaped char (`'A'`, `'\n'`, `'\0'`)
  - **String:** double-quoted UTF-8 string (`"..."`) with standard C++ escapes (`\"`, `\\`, `\n`, `\r`, `\t`, `\0`, `\xHH`)

### 1.2 EBNF Grammar (`asl-core@1`)

```ebnf
TranslationUnit ::= { PreprocDirective | TopLevelDecl } ;

PreprocDirective::= "#" "include" ( "<" HeaderPath ">" | "\"" HeaderPath "\"" )
                  | "#" "define" Ident ConstantExpr ;

TopLevelDecl    ::= StructDecl
                  | EnumDecl
                  | GlobalVarDecl
                  | FunctionDecl ;

StructDecl      ::= "struct" Ident "{" { FieldDecl } "}" ";" ;
FieldDecl       ::= TypeSpec Ident [ "[" IntLiteral "]" ] ";" ;

EnumDecl        ::= "enum" [ "class" ] Ident [ ":" IntType ]
                    "{" Enumerator { "," Enumerator } [ "," ] "}" ";" ;
Enumerator      ::= Ident [ "=" IntExpr ] ;

GlobalVarDecl   ::= [ "static" ] [ "const" | "constexpr" ] TypeSpec
                    Ident [ "[" IntLiteral "]" ] [ "=" InitExpr ] ";" ;

FunctionDecl    ::= TypeSpec Ident "(" [ ParamList ] ")" Block ;
ParamList       ::= Param { "," Param } ;
Param           ::= [ "const" ] TypeSpec [ "&" ] Ident ;

Block           ::= "{" { LocalDecl | Stmt } "}" ;
LocalDecl       ::= [ "static" ] [ "const" ] TypeSpec
                    Ident [ "[" IntLiteral "]" ] [ "=" InitExpr ] ";" ;

Stmt            ::= Block
                  | AssignOrUpdateStmt
                  | ExprStmt
                  | IfStmt
                  | SwitchStmt
                  | ForStmt
                  | WhileStmt
                  | DoWhileStmt
                  | "break" ";"
                  | "continue" ";"
                  | "return" [ Expr ] ";" ;

AssignOrUpdateStmt
                ::= LValue ( "=" | "+=" | "-=" | "*=" | "/=" | "%="
                           | "&=" | "|=" | "^=" | "<<=" | ">>=" ) Expr ";"
                  | LValue ( "++" | "--" ) ";"
                  | ( "++" | "--" ) LValue ";" ;

ExprStmt        ::= Expr ";" ;

IfStmt          ::= "if" "(" Expr ")" Block
                    { "else" "if" "(" Expr ")" Block }
                    [ "else" Block ] ;

SwitchStmt      ::= "switch" "(" Expr ")" "{"
                    { "case" ConstantExpr ":" { Stmt } "break" ";" }
                    [ "default" ":" { Stmt } [ "break" ";" ] ]
                    "}" ;

ForStmt         ::= "for" "(" ForInit ";" Expr ";" ForStep ")" Block ;
ForInit         ::= [ TypeSpec ] Ident "=" Expr ;
ForStep         ::= Ident ( "++" | "--" | "+=" Expr | "-=" Expr | "=" Expr ) ;

WhileStmt       ::= "while" "(" Expr ")" Block ;
DoWhileStmt     ::= "do" Block "while" "(" Expr ")" ";" ;

LValue          ::= Ident { "[" Expr "]" | "." Ident } ;

Expr            ::= LogicalOr ;
LogicalOr       ::= LogicalAnd { "||" LogicalAnd } ;
LogicalAnd      ::= BitOr { "&&" BitOr } ;
BitOr           ::= BitXor { "|" BitXor } ;
BitXor          ::= BitAnd { "^" BitAnd } ;
BitAnd          ::= Equality { "&" Equality } ;
Equality        ::= Relational [ ( "==" | "!=" ) Relational ] ;      (* Non-chaining *)
Relational      ::= Shift [ ( "<" | "<=" | ">" | ">=" ) Shift ] ;    (* Non-chaining *)
Shift           ::= Additive { ( "<<" | ">>" ) Additive } ;
Additive        ::= Multiplicative { ( "+" | "-" ) Multiplicative } ;
Multiplicative  ::= Unary { ( "*" | "/" | "%" ) Unary } ;
Unary           ::= ( "-" | "+" | "!" | "~" ) Unary
                  | "(" TypeSpec ")" Unary
                  | "static_cast" "<" TypeSpec ">" "(" Expr ")"
                  | Postfix ;
Postfix         ::= Primary { "[" Expr "]" | "." Ident | "(" [ ArgList ] ")" } ;
Primary         ::= Literal | Ident | "(" Expr ")" ;
```

### 1.3 OB, Statement & Desugaring Rules

- **Organization Blocks (`04` §3.1):**
  - `void setup()` -> `PouKind::Ob { ob: ObKind::Startup }` (`OB100`).
  - `void loop()` -> `PouKind::Ob { ob: ObKind::Main }` (`OB1`).
  - `attachInterrupt(digitalPinToInterrupt(pin), isr_fn, RISING | FALLING | CHANGE | LOW | HIGH)` inside `setup()` promotes `isr_fn` to `PouKind::Ob { ob: ObKind::HwInt { pin_label, edge, priority } }` (`OB40+`).
  - Helper functions (`int calc(int a)`) -> `PouKind::Fc { return_type }`. Pass-by-reference `T&` parameters map to `ParamDirection::InOut`; value parameters map to `ParamDirection::In`.
  - Stateful IEC FB instances (`TON`, `CTU`, `R_TRIG`, `SR`, `UART_SEND`, etc.) MAY be declared at global scope or inside `static` storage and invoked cleanly.
- **Compound Assignments & `++`/`--`:** `x += e;` desugars to `Assign { target: x, value: x + e }`; `i++;` desugars to `Assign { target: i, value: i + 1 }`.
- **Canonical `for` Loops:** `for (int i = a; i < b; i += s)` lowers to `StmtKind::For { counter: "i", from: a, to: b, step: s, inclusive: false, body }`; `i <= b` sets `inclusive = true`.
- **Cooperative Time Yields:** `delay(ms)` lowers to `StmtKind::Wait` / `WAIT_TIME(ms)`; `delayMicroseconds(us)` lowers to `StmtKind::Wait` / `WAIT_TIME_US(us)`. Calling either inside an `OB_HW_INT` ISR raises `ERR_ISR_FORBIDDEN_OP`.
- **`switch` Fallthrough:** Non-empty `case` arms MUST terminate with `break;` or `return;`; implicit fallthrough between non-empty cases is rejected (or lifted as `Stmt::Foreign(F0)` if untyped) to preserve exact equivalence with ASL `StmtKind::Switch`.

### 1.4 Rejected & `Foreign (F0)` Constructs with Exact `ErrorCode` Mapping

| Construct / Violation | Handling | Frozen `ErrorCode` |
|---|---|---|
| C++ `class` or `struct` containing methods / constructors / inheritance (deferred to `DEF-004` `asl-oop`) | Preserved verbatim as `Decl::Foreign(F0)` (lossless same-language round-trip; `F1` pack supplies method signatures when installed) | `ERR_FOREIGN_EXPORT_BLOCKED` on cross-language export without `--allow-foreign` |
| Direct MCU register manipulation (`PORTB |= (1 << 5);`, `TCCR0A = ...;`, `DDRB`) | Preserved verbatim as `Stmt::Foreign(F0)` in Wave 1 | `ERR_FOREIGN_EXPORT_BLOCKED` on cross-language export without `--allow-foreign` |
| Raw pointers (`*p`, `&x` address-of, `->`), `new`/`delete`, templates, lambdas, exceptions, inline `asm`, `goto` | Preserved as `Decl`/`Stmt`/`Expr::Foreign(F0)` (or rejected if syntactically malformed) | `ERR_FOREIGN_UNTYPED` if `Expr::Foreign` lacks context type and no `F1` pack matches |
| Unterminated string/comment, invalid literal, chained comparison (`a < b < c`) | Hard parse error | `ERR_TOML_SYNTAX` (lex/syntax diagnostic) |
| Unknown type name | Hard error | `ERR_UNKNOWN_TYPE` |
| Mixed-sign comparison or implicit narrowing assignment (`int16_t` + `uint16_t`) | Hard type error | `ERR_TYPE_MISMATCH` |
| Invalid cast (`static_cast` between incompatible domains without codec intrinsic) | Hard type error | `ERR_INVALID_CAST` |
| Constant out-of-bounds array index (`arr[N]`) | Hard error | `ERR_ARRAY_BOUNDS_CONST` |
| Access to undeclared struct/FB field | Hard error | `ERR_UNKNOWN_FIELD_ACCESS` |
| Reference to undeclared variable or pin constant | Hard error | `ERR_UNDECLARED_SYMBOL` |
| Write to `const`/`constexpr` variable or `#define` constant | Hard error | `ERR_CONST_WRITE` |
| Write to input-bound hardware pin label | Hard error | `ERR_HARDWARE_LABEL_WRITE` |
| Duplicate symbol in same lexical scope or hardware↔data collision | Hard error | `ERR_DUPLICATE_DECL` / `ERR_NAME_COLLISION` |
| Unresolved function call or wrong argument count / return type | Hard error | `ERR_UNRESOLVED_FUNCTION` / `ERR_ARITY_MISMATCH` / `ERR_RETURN_TYPE_MISMATCH` |
| `break` / `continue` outside loop (or `break` outside `switch`) | Hard error | `ERR_BREAK_OUTSIDE_LOOP` / `ERR_CONTINUE_OUTSIDE_LOOP` |
| `delay()` / `delayMicroseconds()` inside `attachInterrupt` ISR (`OB_HW_INT`) | Hard error | `ERR_ISR_FORBIDDEN_OP` |
| `attachInterrupt` referencing undefined function or non-interrupt pin | Hard error | `ERR_UNRESOLVED_ISR` / `ERR_PIN_CAPABILITY_MISMATCH` |
| Unknown board port or conflicting pin bindings | Hard error | `ERR_UNKNOWN_PORT` / `ERR_HARDWARE_PIN_CONFLICT` |
| Display sink string literal violating component `[display] charset` (e.g. HD44780 ROM, `DEF-007`) | Hard error (never applied to `Serial`/UART UTF-8 streams) | `ERR_STRING_CHARSET` |

---

## 2. Per-Board Type Mapping (`[cpu] int_bits` Honored Strictly)

`dendri-lang-cpp` MUST read `BoardContext.int_bits` from the active board profile's `[cpu] int_bits` field. On `arduino-uno-r3` (AVR `ATmega328P`), `int_bits = 16`; on 32-bit targets (e.g., `plc-1200-open-1214` or future 32-bit MCUs), `int_bits = 32`.

| Arduino C++ Type | ASL v2 Type (`int_bits = 16`, AVR) | ASL v2 Type (`int_bits = 32`, 32-bit) | Notes |
|---|---|---|---|
| `bool` | `BOOL` | `BOOL` | 1-bit boolean |
| `int8_t` / `signed char` | `SINT` (8-bit) | `SINT` (8-bit) | Exact signed 8-bit |
| `uint8_t` / `byte` / `unsigned char` | `USINT` (8-bit) | `USINT` (8-bit) | Exact unsigned 8-bit |
| `int16_t` / `short` | `INT` (16-bit) | `INT` (16-bit) | Exact signed 16-bit |
| `uint16_t` / `unsigned short` / `word` | `UINT` (16-bit) | `UINT` (16-bit) | Exact unsigned 16-bit |
| **`int`** | **`INT` (16-bit)** | **`DINT` (32-bit)** | **Honors `[cpu] int_bits`** |
| **`unsigned int`** | **`UINT` (16-bit)** | **`UDINT` (32-bit)** | **Honors `[cpu] int_bits`** |
| `int32_t` / `long` | `DINT` (32-bit) | `DINT` (32-bit) | Exact signed 32-bit |
| `uint32_t` / `unsigned long` | `UDINT` (32-bit) | `UDINT` (32-bit) | Exact unsigned 32-bit |
| `int64_t` / `long long` | `LINT` (64-bit) | `LINT` (64-bit) | Exact signed 64-bit |
| `uint64_t` / `unsigned long long` | `ULINT` (64-bit) | `ULINT` (64-bit) | Exact unsigned 64-bit |
| `float` | `REAL` (IEEE-754 `f32`) | `REAL` (IEEE-754 `f32`) | No implicit FMA; `libm` transcendentals |
| `double` | `REAL` (`f32` on AVR-GCC) | `LREAL` (`f64`) | Matches AVR-GCC 32-bit `double` vs 64-bit IEEE-754 |
| `const char*` / `char[N]` | `STRING[N]` (default `255`) | `STRING[N]` (default `255`) | Fixed-capacity UTF-8; Arduino heap `String` class -> `Foreign(F0)` |
| `T arr[N]` | `ARRAY[N] OF T` | `ARRAY[N] OF T` | 0-indexed fixed-size array |
| Flat `struct S { ... };` | `TypeDecl::Struct` | `TypeDecl::Struct` | Data fields only; structs/classes with methods -> `Decl::Foreign(F0)` (`DEF-004`) |
| `enum` / `enum class` | `TypeDecl::Enum` | `TypeDecl::Enum` | Explicit integer backing type (default `INT` on 16-bit / `DINT` on 32-bit) |
| `void` (function return) | `TypeRef::Void` | `TypeRef::Void` | Stateless `FC` with no return value |

### Storage Scope Mapping

- File-scope `const` / `constexpr` / simple `#define` -> `StorageScope::Const`
- File-scope mutable variable or function-local `static` variable -> `StorageScope::Var`
- Non-`static` local variable inside `setup()`/`loop()`/functions -> `StorageScope::Temp`
- `RETAIN` / `PERSISTENT` ASL variables project to file-scope variables annotated via board persistence metadata (never via magic comments).

---

## 3. Numeric Codecs & Intrinsic Mapping

### 3.1 Normative Wave 1 Arduino Intrinsics (`01` §3.5, `04` §2 #3)

| Arduino C++ Call | ASL v2 Representation | Board Profile Binding (`arduino-uno-r3`) |
|---|---|---|
| `analogRead(A0)` | `ADC_CODE(A0)` -> `CODE<uno_10bit>` | `uno_10bit`: `kind = "binned"`, `D = 1024`, `n_max = 1023`, `0.0..5.0 V` (`A0..A5`) |
| `analogWrite(D5, val)` | `PWM_WRITE(D5, ANALOG_TO_DUTY(CODE_TO_ANALOG(val, "pwm_8bit")))` | `pwm_8bit`: `kind = "binned"`, `D = 256`, `n_max = 255` (`D3, D5, D6, D9, D10, D11`) |
| `analogReadResolution(bits)` | `ANALOG_READ_RESOLUTION(bits)` | Statement updating active channel ADC codec (`D = 2^bits`, `n_max = 2^bits - 1`) |
| `digitalRead(pin)` | `DIGITAL_READ(pin)` -> `BOOL` | Reads pin electrical level per `[electrical]` `v_ih_v`/`v_il_v` & `indeterminate_reads_as` |
| `digitalWrite(pin, val)` | `DIGITAL_WRITE(pin, val)` | Drives digital output pin (`HIGH`/`true`, `LOW`/`false`) |
| `millis()` / `micros()` | `MILLIS()` / `MICROS()` -> `UDINT` | Cycle-based deterministic virtual clock |
| `delay(ms)` / `delayMicroseconds(us)` | `WAIT_TIME(ms)` / `WAIT_TIME_US(us)` | Cooperative fiber yield (`StmtKind::Wait`) |

- **Two-Layer Separation (`01` §3.5, `DEF-001`):** `analogWrite(pin, code)` maps through the pure integer `pwm_8bit` codec (`D = 256, n_max = 255`) at the ASL boundary. The physical timer transfer function (Timer 0 Fast PWM `(OCR+1)/256` with `0`/`255` Arduino core bypass on `D5`/`D6` at `976.56 Hz`; Timer 1/2 Phase-Correct PWM `OCR/255` on `D3`/`D9`/`D10`/`D11` at `490.20 Hz`) lives strictly in `assets/boards/arduino-uno-r3/profile.toml` and `dendri-circuit`.
- **`DUTY` Clamping:** Values exceeding `M = 10⁹` clamp to `M` (`255` in `pwm_8bit`) with `WARN_DUTY_CLAMPED`.

### 3.2 `analogReadResolution` & Fixed-Codec Emission

- `analogReadResolution(bits)` is represented as a first-class statement (`ANALOG_READ_RESOLUTION(bits)`), validating `2^bits <= M` (`bits <= 29`; higher widths fail with `ERR_CODEC_RESOLUTION_EXCEEDS_SCALE`).
- When emitting C++ for a fixed-resolution board such as `arduino-uno-r3` (hardware 10-bit ADC only), the emitter MUST either prove via static analysis that the active resolution equals the board's native width or emit an explicit clamped reduced-rational shift/scale expression.

### 3.3 Cross-Platform Reduced-Rational Emission Rule (NEVER Emit `M`)

1. The C++ emitter **MUST NEVER** emit `1000000000` (`M`).
2. Given source codec $(D_1, n_{\max,1})$ and destination codec $(D_2, n_{\max,2})$, compute $g = \gcd(D_1, D_2)$, $D_{1,\text{red}} = D_1 / g$, $D_{2,\text{red}} = D_2 / g$.
3. Select the intermediate unsigned/signed width strictly by range analysis:
   - If $n_{\max,1} \cdot D_{2,\text{red}} < 2^{32}$, emit `uint32_t` (or `int32_t` for signed bipolar).
   - Otherwise, emit `uint64_t` (or `int64_t` for signed bipolar).
4. Emit clamped `min(n_max2, (n * D2_red) / D1_red)` (mandatory `min` clamp whenever source is `span` and destination is `binned`):
   - **12-bit (`D1 = 4096`) -> Uno 10-bit (`D2 = 1024`):** `g = 1024`, `D1_red = 4`, `D2_red = 1` -> emits `(n >> 2)` (no clamp needed since `4095 >> 2 == 1023`).
   - **PLC 27648 (`span`, `D1 = 27648`) -> Uno 10-bit (`binned`, `D2 = 1024, n_max2 = 1023`):** `g = 1024`, `D1_red = 27`, `D2_red = 1` -> emits `((uint32_t)(n) / 27U > 1023U ? 1023U : (uint32_t)(n) / 27U)` (mandatory clamp prevents `27648 / 27 == 1024` overflow!).
   - **Uno 10-bit (`D1 = 1024`) -> PLC 27648 (`D2 = 27648`):** `g = 1024`, `D1_red = 1`, `D2_red = 27` -> emits `((uint32_t)(n) * 27U)`.

---

## 4. Comment Mapping & `comment_styles`

Comments are stored verbatim in `[[boards.comments]]` (ADR-027) with zero trimming, zero Unicode normalization, and LF internal line endings.

### 4.1 Supported C++ Comment Styles

| `CommentStyle` | C++ Syntax | Placement & Grouping |
|---|---|---|
| `Line` | `// ...` | Contiguous `leading` lines with no blank line merge with `\n`; blank line splits records. `trailing` comments are always single-line. |
| `Block` | `/* ... */` | Leading, trailing, header, or `inner` (`token_ordinal` required). C++ block comments do **not** nest. |
| `DocLine` | `/// ...` | Leading documentation line comments (contiguous lines merged with `\n`). |
| `DocBlock` | `/** ... */` | Leading or header documentation block comments. |
| `ParenBlock` *(from ST)* | Projected to `/* ... */` or `// ...` | Non-destructive projection; original `origin.style = "paren_block"` retained in ASL. |
| `Visual` *(from LD/SFC/UI)* | Projected to `// ...` | Non-destructive projection; original `origin.style = "visual"` retained in ASL. |

### 4.2 Projection-Only Hazard Avoidance & Re-Import Idempotence

Every mitigation applies strictly to the emitted C++ text without mutating stored `Comment.text`:

1. **Trailing Backslash Line-Splice Hazard (`\`):** In C++, a `//` comment whose line ends with `\` splices the next source line into the comment. Whenever any line of a `Line` / `DocLine` comment ends with `\`, `render_comment` MUST project the comment as a `/* ... */` block comment (or insert a safe non-splicing delimiter escape if `text` also contains `*/`).
2. **Embedded `*/` & Nested ST `(* ... *)` Hazard:**
   - A `Block` / `DocBlock` comment containing `*/` MUST degrade to `//` line comments (unless a line also ends with `\`, in which case `/* ... */` with documented non-destructive `* /` escape is emitted).
   - A nested IEC ST `ParenBlock` comment (`(* outer (* inner *) outer *)`) projected to C++ MUST emit as `//` line comments (never invalid nested `/* ... */`).
3. **Trojan-Source Bidi Controls (`WARN_COMMENT_BIDI`):** Bidi control code points (`U+061C`, `U+200E`, `U+200F`, `U+202A..=U+202E`, `U+2066..=U+2069`) raise `WARN_COMMENT_BIDI` and render escaped as `\u{XXXX}`.
4. **Target `comment_encoding` (`WARN_COMMENT_ENCODING`):** Supports `Utf8Passthrough` (default), `AsciiFold` (projection-only ASCII folding/escaping + warning), and `WarnOnly` (UTF-8 emitted + warning).
5. **Re-Import Idempotence (`matches_comment` — Gate `C4`):** Stored `Comment.text` is overwritten on re-parse ONLY if `matches_comment(parsed_text, stored_text)` returns `false`, guaranteeing degraded projections never clobber the verbatim original.
6. **Relocation & Orphans (`C2`):** `inner` comments unable to remain inline on cross-language emission relocate to the nearest enclosing statement boundary (`INFO_COMMENT_RELOCATED`). Deleting an anchored node transitions its comments to `dangling` on the parent scope (`INFO_COMMENT_ORPHANED`).

---

## 5. Module, Multi-Tab Sketch, Import & Pack Mapping

- **Multi-Tab Arduino Sketch Rules (`04` §3.2):**
  - A sketch directory containing a primary `<sketch>.ino` plus auxiliary `.ino` tabs merges them in canonical Arduino order: primary `<sketch>.ino` first, followed by remaining `.ino` tabs in lexicographic ASCII order. Automatic function forward-declaration hoisting is performed across merged `.ino` tabs so call order never causes spurious `ERR_UNDECLARED_SYMBOL`.
  - Accompanying `.h` / `.cpp` tab pairs map 1:1 to embedded `[[boards.program.modules]]` (`exports` derived from header declarations; implementation POUs from the `.cpp` unit). Source boundaries are recorded in `[boards.presentation.origin_files]` for lossless multi-file re-emission.
- **Domain-Split Stdlib Mapping:**
  - `#include <Arduino.h>` (explicit or implicit in `.ino` sketches) brings `std.arduino.*` (`std.arduino.gpio`, `std.arduino.time`) and `std.iec.*` math/conversion helpers into scope.
  - Pure-PLC modules (`std.plc.*`) are NOT in scope unless explicitly imported.
- **Hash-Locked Wave 1 Packs (`DEF-008`):**
  - `#include <Wire.h>` resolves `F1 Declared` signatures from `assets/packs/arduino-wire.pack.toml` (`Wire.begin`, `Wire.beginTransmission`, `Wire.write`, `Wire.endTransmission`, `Wire.requestFrom`, `Wire.read`).
  - `#include <SPI.h>` resolves `F1 Declared` signatures from `assets/packs/arduino-spi.pack.toml` (`SPI.begin`, `SPI.beginTransaction`, `SPI.transfer`, `SPI.endTransaction`).
  - `#include <Servo.h>` resolves `F1 Declared` signatures from `assets/packs/arduino-servo.pack.toml` (`Servo.attach`, `Servo.write`, `Servo.writeMicroseconds`, `Servo.detach`).
  - Unrecognized third-party headers and their calls preserve cleanly as `Decl`/`Stmt`/`Expr::Foreign(F0)` with four-part diagnostics (what / where / why / what now) on simulation halt unless governed by `[[boards.assumptions]]`.
- **Cross-Language Export Guard:** Exporting a file containing `Foreign` nodes to ST/Ladder/GRAFCET fails with `ERR_FOREIGN_EXPORT_BLOCKED`; `--allow-foreign` emits a demarcated `/* NOT TRANSLATED [<lang>/<fidelity>]: <text> */` (or target-native) comment block. Same-language C++ round-trips are never blocked.

---

## 6. Hardware Address & Tag Mapping (Zero Magic Comments)

Hardware bindings live exclusively in `[[boards.program.hardware]]` (`04` §4.1):

- **Pin Identifier Reconciliation (`arduino-uno-r3`):**
  - Integer literals `0..13` reconcile to canonical port IDs `D0..D13`; symbolic identifiers `A0..A5` reconcile to `A0..A5`; `LED_BUILTIN` reconciles to `D13`.
  - `#define LED_PIN 13` or `const uint8_t ledPin = 13;` paired with `pinMode(ledPin, OUTPUT)` / `pinMode(btnPin, INPUT_PULLUP)` reconciles cleanly to a `HardwareBinding` entry (`label`, `pin`, `kind`, `pull`, `codec`, `range`) validated against `assets/boards/arduino-uno-r3/profile.toml`.
  - When projecting a `plc-1200-open-1214` board to C++, ports bind by their logical tag labels from `[[boards.program.hardware]]` (preserving `Ia.0 ⇄ %IX0.0`, `Qa.0 ⇄ %QX0.0`, `IW64 ⇄ %IW64` in the Board I/O Tag Table without ever emitting `// ASL_HW:` comments).
- **Capability Validation:** Binding `analogWrite` to a non-PWM pin (e.g., `D4` on `arduino-uno-r3`), `analogRead` to a digital-only pin, or `attachInterrupt` to a non-interrupt pin (anything other than `D2`/`D3` on Uno) raises `ERR_PIN_CAPABILITY_MISMATCH`. Duplicate pin bindings raise `ERR_HARDWARE_PIN_CONFLICT`.

---

## 7. Mandatory Round-Trip Class — `LOSSLESS_ASL` & Conformance Suite

`dendri-lang-cpp` MUST satisfy **`LOSSLESS_ASL`**:

1. **AST Identity:** For every valid `AslModule` / `BoardProgram` in the `asl-core@1` C++ subset:
   $$\text{parse}(\text{emit}(M, \text{ctx}), \text{ctx}) \equiv M$$
   modulo freshly computed byte `Span` offsets.
2. **Verbatim Comment Invariance:** Comment text multisets are preserved across all C++ ↔ ASL ↔ ST/Ladder/GRAFCET round-trips (`C1–C5`). Code formatting is canonical (2-space or 4-space consistent K&R braces); only verbatim comment text, `eol_style`, `origin_files`, and `blank_lines_before: 0..=2` are preserved.
3. **Mandatory Conformance Suite (`04` §2, `05` §4):**
   - `proptest` verifying `LOSSLESS_ASL` for both `int_bits = 16` (AVR) and `int_bits = 32` contexts.
   - Comment conservation suite (`C1–C5`) including hostile trailing `\`, `*/`, nested ST `(* (* *) *)`, CRLF, bidi controls, and emoji.
   - Numeric codec & reduced-rational emitter tests verifying `M = 10⁹` never appears in emitted C++ and mandatory `span -> binned` clamping holds.
   - Three-level `Foreign` preservation tests (`class` with methods -> `Decl::Foreign(F0)`, `PORTB |= (1<<5);` -> `Stmt::Foreign(F0)`, `servo.read()` -> `Expr::Foreign(F0)`), plus `F1` pack resolution for `Wire`, `SPI`, and `Servo`.
   - At least one genuine negative source fixture per produced `ErrorCode`.
   - **Phase 6b Differential Gate:** Emitted C++ (including hostile comments `C3`) compiles and executes under host `g++` + deterministic Arduino shim, compiles under nightly `arduino-cli`, and matches `avr-gcc` + `simavr` on AVR-specific 16-bit/ADC/PWM scenarios.

---

## 8. `LanguageCapabilities` & Feature Matrix Contract

```rust
// Normative capability profile for `dendri-lang-cpp`
LanguageCapabilities {
    supported_comment_styles: vec![
        CommentStyle::Line,
        CommentStyle::Block,
        CommentStyle::DocLine,
        CommentStyle::DocBlock,
    ],
    supports_nested_paren_comments: false,
    supports_line_comments: true,
    comment_encoding: ctx.comment_encoding, // Utf8Passthrough | AsciiFold | WarnOnly
    capabilities: vec!["asl-core@1".to_owned()],
    st_dialect: None,
}
```

- **Auto-Generated Feature Matrix:** The published C++ feature-support matrix MUST be generated directly from the `dendri-lang-cpp` conformance test suite via `dendri-frontend-core::conformance` and MUST NOT be maintained by hand (`04` §2).
