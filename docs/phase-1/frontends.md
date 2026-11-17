# Phase 1 — Text language frontends (DEN-7 / DEN-10)

**Status:** deepened on the **current** sim opcode set (priority 1). ASL remains **unversioned** — do **not** assign `0.0.1`.

P1 text languages parse to and generate from the same [`Program`] AST that `dendri-sim` runs.

| Language | Crate | Parse | Generate |
|---|---|---|---|
| C++ (Arduino-style) | `dendri-frontend-cpp` | yes | yes |
| MicroPython | `dendri-frontend-micropython` | yes | yes |
| Structured Text | `dendri-backend-st` | yes | yes |
| Dispatch facade | `dendri-frontends` | yes | yes |

Ladder / SFC = visual track (DEN-8 landed linear subset). Rust-as-user-scripting = **P2**.

## CLI

```bash
cargo run -p dendri-cli -- emit-cpp content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- emit-micropython content/fixtures/sim/S02-ton-on-delay.asl.toml
cargo run -p dendri-cli -- emit-st content/fixtures/sim/S03-control-flow-get-bool.asl.toml
cargo run -p dendri-cli -- parse-source --lang cpp content/fixtures/frontends/S03-control-flow.cpp
cargo run -p dendri-cli -- frontend-roundtrip --lang st content/fixtures/sim/S02-ton-on-delay.asl.toml
cargo run -p dendri-cli -- sync-sources content/fixtures/projects/P01-uno-led-blink.dendri.toml
```

`.dendri.toml` `[[sources]]` carries inline `text` for `cpp` / `micropython` / `st`. Empty stubs can be filled from embedded `[asl]` via `sync-sources`.

## Supported subset (current sim lock)

Common across languages — locked to opcodes `dendri-sim` already executes:

- GPIO: `CFG_DIGITAL_OUTPUT` / `CFG_DIGITAL_INPUT` / `CFG_DIGITAL_INPUT_PULLUP` / `CFG_DIGITAL_INPUT_PULLDOWN`, `SET_BOOL`, `GET_BOOL`
- Time: `WAIT_TIME` (+ `WAIT_TIME_US` mapped where applicable), `GET_SYS_TIME` aliases (`millis` / `ticks_ms`)
- FB: `TON(instance, IN, PT)` as expression / assign RHS (sim-backed)
- Data: `TIME`, `BOOL`, `UINT`, `INT`, `REAL`, `STRING` (via `ASL_DATA` annotations on C++/MicroPython emit)
- Stmts: assign, call, `if` / `elif`/`else`, `while`, simple `for`, `return` / `break`/`EXIT`, user `FUNCTION`/`def`
- Hardware labels preserved via `ASL_HW` (C++/MPY) or `(* HW: … *)` (ST)

Fixtures: `S01` blink, `S02` TON, `S03` control-flow + GET_BOOL — each with `.cpp` / `.py` / `.st` under `content/fixtures/frontends/`.

### C++

- Arduino sketch shape: `void setup()` / `void loop()`
- `pinMode` / `digitalWrite` / `digitalRead` / `delay` / `millis`
- `const int label = N` pin labels (+ `// ASL_HW:` for non-`*Pin` names and DI/DO honesty)
- `const unsigned long` → `TIME`; `const unsigned int` / `int` / `bool` / `float` with `// ASL_DATA:`

**Out:** classes, templates, pointers, preprocessor semantics, STM32/ESP-IDF HAL beyond Arduino aliases.

### MicroPython

- `from machine import Pin` / `from time import sleep_ms`
- Pin consts + `*_pin = None` / `Pin(..., Pin.IN|OUT|PULL_UP|PULL_DOWN)` / `.value(0|1)` / `sleep_ms`
- Entry boilerplate `setup(); while True: loop()` ignored on parse
- `ASL_HW` / `ASL_DATA` comments for label + type round-trip (ALL_CAPS ints without annotation still default to `TIME`)

**Out:** classes, async, ISR decorators, PWM/ADC/I2C/SPI object surface.

### Structured Text

- `PROGRAM` / `VAR` / `CONSTANT` / `SETUP` / `LOOP` / `FUNCTION`
- IEC operators (`AND`/`OR`/`NOT`, `:=`, `TRUE`/`FALSE`, `T#…`)
- Direct ASL opcode calls (`SET_BOOL`, `GET_BOOL`, `WAIT_TIME`, `TON`, …)
- Hardware preserved via `(* HW: … *)` comments for round-trip
- `FOR … TO n - 1` emit form unwraps on parse to ASL exclusive `to`

**Out:** full IEC 61131-3 (POUs, FBs beyond call form, POINTER, namespaces, vendor dialects). `SETUP`/`LOOP`/`CONSTANT` are Dendri MVP surface keywords for honest setup/loop split.

## Gaps → priority 2 / DEN-11 (sim + opcodes first)

Parsers/generators stay locked to the AST `dendri-sim` runs. **Do not** pretend these work in frontends until sim executes them:

| Area | Catalog ops (examples) | Frontend status |
|---|---|---|
| PWM | `CFG_PWM_OUTPUT`, `SET_PWM_DUTY`, `SET_PWM_FREQ` | emit as raw call only; **no** Arduino/`machine.PWM` surface |
| ADC | `CFG_ADC_INPUT`, `READ_ADC` | not mapped |
| UART / I2C / SPI | bus CFG + read/write family | not mapped |
| Extra IEC FBs | `TOF`, `TP`, `CTU`/`CTD`/`CTUD`, `R_TRIG`/`F_TRIG` | call form may parse as opaque `Call`; **sim rejects** |
| Math / string | `SQRT`, `CONCAT`, … | not in sim MVP |
| Retain / persistent | scopes exist in AST | no frontend EEPROM/flash story yet |

**DEN-11** owns expanding sim + opcodes; frontends follow the same AST in lockstep.

## Version gate

Assign ASL version `0.0.1` only after full MVP acceptance (priority 7 / DEN-9) — not after this priority-1 deepen.
