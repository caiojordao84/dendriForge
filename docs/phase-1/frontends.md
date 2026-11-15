# Phase 1 — Text language frontends (DEN-7)

**Status:** implemented (MVP subset). ASL remains **unversioned** — do **not** assign `0.0.1`.

P1 text languages parse to and generate from the same [`Program`] AST that `dendri-sim` runs.

| Language | Crate | Parse | Generate |
|---|---|---|---|
| C++ (Arduino-style) | `dendri-frontend-cpp` | yes | yes |
| MicroPython | `dendri-frontend-micropython` | yes | yes |
| Structured Text | `dendri-backend-st` | yes | yes |
| Dispatch facade | `dendri-frontends` | yes | yes |

Ladder / SFC = **DEN-8**. Rust-as-user-scripting = **P2**.

## CLI

```bash
cargo run -p dendri-cli -- emit-cpp content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- emit-micropython content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- emit-st content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- parse-source --lang cpp content/fixtures/frontends/S01-blink.cpp
cargo run -p dendri-cli -- frontend-roundtrip --lang st content/fixtures/sim/S01-blink.asl.toml
cargo run -p dendri-cli -- sync-sources content/fixtures/projects/P01-uno-led-blink.dendri.toml
```

`.dendri.toml` `[[sources]]` carries inline `text` for `cpp` / `micropython` / `st`. Empty stubs can be filled from embedded `[asl]` via `sync-sources`.

## Supported MVP subset (honest limits)

Common across languages (blink-class + control flow):

- Hardware digital pins → `CFG_DIGITAL_*` / `SET_BOOL` / `GET_BOOL`
- `TIME` consts (`T#Nms`) and `BOOL` / integer vars
- `setup` + scan `loop` bodies
- Assign, call, `if`/`else`, `while`, simple `for`
- `WAIT_TIME` (and `WAIT_TIME_US` where mapped)

### C++

- Arduino sketch shape: `void setup()` / `void loop()`
- `pinMode` / `digitalWrite` / `digitalRead` / `delay` / `millis`
- `const int ledPin = N` pin labels; `const unsigned long NAME = N UL` → `TIME`

**Out:** classes, templates, pointers, preprocessor semantics, STM32/ESP-IDF HAL beyond Arduino aliases.

### MicroPython

- `from machine import Pin` / `from time import sleep_ms`
- `LED_PIN = N`, `led_pin = Pin(LED_PIN, Pin.OUT)`, `.value(0|1)`, `sleep_ms`
- Entry boilerplate `setup(); while True: loop()` ignored on parse

**Out:** classes, async, ISR decorators, PWM/ADC/I2C/SPI object surface (stdlib mapping exists in docs; not in this frontend yet).

### Structured Text

- `PROGRAM` / `VAR` / `CONSTANT` / `SETUP` / `LOOP` / `FUNCTION`
- IEC operators (`AND`/`OR`/`NOT`, `:=`, `TRUE`/`FALSE`, `T#…`)
- Direct ASL opcode calls (`SET_BOOL`, `WAIT_TIME`, `TON`, …)
- Hardware preserved via `(* HW: … *)` comments for round-trip

**Out:** full IEC 61131-3 (POUs, FBs beyond call form, POINTER, namespaces, vendor dialects). `SETUP`/`LOOP`/`CONSTANT` are Dendri MVP surface keywords for honest setup/loop split.

## Gaps → DEN-8 / full MVP

- Ladder + SFC visual editors and bidirectional diagram ↔ TOML sync
- Broader stdlib opcode coverage in sim + frontends (PWM, ADC, UART, …)
- Project `[[sources]]` live sync while editing (GUI)
- Assign ASL version `0.0.1` only after full MVP acceptance gate
