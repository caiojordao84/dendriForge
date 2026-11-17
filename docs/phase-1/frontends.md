# Phase 1 — Text language frontends (DEN-7 / DEN-10 / DEN-11)

**Status:** implemented (MVP subset through DEN-11 peripherals + IEC FBs). ASL language tag **`0.0.1`**.

P1 text languages parse to and generate from the same [`Program`] AST that `dendri-sim` runs.

| Language | Crate | Parse | Generate |
|---|---|---|---|
| C++ (Arduino-style) | `dendri-frontend-cpp` | yes | yes |
| MicroPython | `dendri-frontend-micropython` | yes | yes |
| Structured Text | `dendri-backend-st` | yes | yes |
| Dispatch facade | `dendri-frontends` | yes | yes |

Ladder / SFC = **DEN-8** (see [`ladder-sfc.md`](./ladder-sfc.md)). Rust-as-user-scripting = **P2**.

## CLI

```bash
cargo run -p dendri-cli -- emit-cpp content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- emit-micropython content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- emit-st content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- parse-source --lang cpp content/fixtures/frontends/S01-blink.cpp
cargo run -p dendri-cli -- frontend-roundtrip --lang st content/fixtures/sim/S04-pwm-adc-map.asl.toml
cargo run -p dendri-cli -- sync-sources content/fixtures/projects/P01-uno-led-blink.dendri.toml
```

`.dendri.toml` `[[sources]]` carries inline `text` for `cpp` / `micropython` / `st`. Empty stubs can be filled from embedded `[asl]` via `sync-sources`.

## Supported MVP subset (honest limits)

Common across languages:

- Hardware digital / analog / bus pins → `CFG_*` / `SET_BOOL` / `GET_BOOL` / PWM / ADC / UART / I2C / SPI cfg
- `TIME` consts (`T#Nms`) and `BOOL` / integer / `REAL` / `STRING` vars
- `setup` + scan `loop` bodies
- Assign, call, `if`/`else`, `while`, simple `for`
- `WAIT_TIME` (and `WAIT_TIME_US` where mapped)
- IEC FBs: `TON` / `TOF` / `TP` / `CTU` / `CTD` / `CTUD` / `R_TRIG` / `F_TRIG`

Fixtures: `S01`–`S07` under `content/fixtures/frontends/` + `content/fixtures/sim/`.

### C++

- Arduino sketch shape: `void setup()` / `void loop()`
- `pinMode` / `digitalWrite` / `digitalRead` / `delay` / `millis`
- `analogWrite` / `analogRead` / `Serial.begin|print|println`
- `CFG_PWM_OUTPUT` / `CFG_ADC_INPUT` / `CFG_I2C` / `CFG_SPI` (ASL call form; `Wire.begin` / `SPI.begin` accepted on parse)
- IEC FBs as calls (same arity as sim)

**Out:** classes, templates, pointers, preprocessor semantics, STM32/ESP-IDF HAL beyond Arduino aliases; ledc channel allocation; I2C/SPI *transfer* APIs.

### MicroPython

- `from machine import Pin[, PWM, ADC, UART, I2C, SPI]` / `from time import sleep_ms`
- Pin / PWM / ADC objects; `uart_0.write`; `CFG_I2C` / `CFG_SPI` ASL call form (ctors accepted on parse)
- Entry boilerplate `setup(); while True: loop()` ignored on parse
- `ASL_HW` / `ASL_DATA` comments for label + type round-trip

**Out:** classes, async, ISR decorators, full I2C/SPI transfer APIs.

### Structured Text

- `PROGRAM` / `VAR` / `CONSTANT` / `SETUP` / `LOOP` / `FUNCTION`
- IEC operators (`AND`/`OR`/`NOT`, `:=`, `TRUE`/`FALSE`, `T#…`)
- Direct ASL opcode calls (GPIO, PWM/ADC/UART/I2C/SPI cfg, IEC FBs, `MAP_RANGE`, …)
- Hardware preserved via `(* HW: … *)` comments for round-trip

**Out:** full IEC 61131-3 (POUs, FBs beyond call form, POINTER, namespaces, vendor dialects). `SETUP`/`LOOP`/`CONSTANT` are Dendri MVP surface keywords for honest setup/loop split.

## Supported peripherals / IEC FBs (DEN-11)

| Area | Ops (sim + frontends) |
|---|---|
| PWM | `CFG_PWM_OUTPUT`, `SET_PWM_FREQ`, `SET_PWM_DUTY`, `GET_PWM_DUTY` |
| ADC | `CFG_ADC_INPUT`, `READ_ADC` (+ `MAP_RANGE`, casts) |
| UART | `CFG_SERIAL`, `SERIAL_PRINT(LN)`, `SERIAL_READ(LN)`, `SERIAL_AVAILABLE`, `SERIAL_FLUSH` |
| I2C / SPI | `CFG_I2C`, `CFG_SENSOR_I2C`, `CFG_SPI` (config only — no transfer yet) |
| IEC FBs | `TON` (ET clamp), `TOF`, `TP`, `CTU`, `CTD`, `CTUD`, `R_TRIG`, `F_TRIG` |

## Gaps → DEN-12+ / later priorities

- I2C/SPI **transfer** + display/sensor driver FBs (scenario 2 remainder)
- Full Ladder/SFC canvas polish beyond linear (priority 3)
- Broader math/string ops in sim
- Retain/persistent EEPROM story in frontends
- ASL `0.0.1` is assigned (see `docs/phase-0/versioning.md`)
