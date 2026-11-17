# Frontend fixtures (DEN-7 / DEN-10 / DEN-11 / DEN-24)

Hand-checkable language sources that parse into the shared ASL AST and run on `dendri-sim`.

| File | Language | Paired ASL |
|---|---|---|
| `S01-blink.{cpp,py,st,rs}` | C++ / MicroPython / ST / Rust | `../sim/S01-blink-set-bool-wait.asl.toml` |
| `S02-ton.{cpp,py,st,rs}` | C++ / MicroPython / ST / Rust | `../sim/S02-ton-on-delay.asl.toml` |
| `S03-control-flow.{cpp,py,st,rs}` | C++ / MicroPython / ST / Rust | `../sim/S03-control-flow-get-bool.asl.toml` |
| `S04-pwm-adc.{cpp,py,st,rs}` | C++ / MicroPython / ST / Rust | `../sim/S04-pwm-adc-map.asl.toml` |
| `S05-uart-buses.{cpp,py,st,rs}` | C++ / MicroPython / ST / Rust | `../sim/S05-uart-i2c-spi.asl.toml` |
| `S06-tof-trig.{cpp,py,st,rs}` | C++ / MicroPython / ST / Rust | `../sim/S06-iec-tof-trig.asl.toml` |
| `S07-counters.{cpp,py,st,rs}` | C++ / MicroPython / ST / Rust | `../sim/S07-iec-counters.asl.toml` |

Coverage intent:

- **S01** — blink: `SET_BOOL` + `WAIT_TIME` + TIME consts
- **S02** — `TON` + `CFG_DIGITAL_INPUT` + DI/DO + BOOL vars
- **S03** — `GET_BOOL` + if/elif/else + `for` + BOOL/UINT/INT/TIME
- **S04** — `READ_ADC` + `MAP_RANGE` + `SET_PWM_DUTY` (scenario 2 core)
- **S05** — `CFG_SERIAL` / `SERIAL_PRINTLN` + `CFG_I2C` / `CFG_SPI`
- **S06** — `TOF` + `R_TRIG` / `F_TRIG` (scenario 1 edges)
- **S07** — `CTU` / `CTD` / `CTUD`

Regenerate from ASL:

```bash
cargo run -q -p dendri-cli -- emit-cpp content/fixtures/sim/S04-pwm-adc-map.asl.toml
cargo run -q -p dendri-cli -- emit-micropython content/fixtures/sim/S06-iec-tof-trig.asl.toml
cargo run -q -p dendri-cli -- emit-st content/fixtures/sim/S07-iec-counters.asl.toml
cargo run -q -p dendri-cli -- emit-rust content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
```

Acceptance tests live in `crates/dendri-frontends/tests/frontend_acceptance.rs`.

Rust (P2 / DEN-24) is a Dendri HAL subset that lowers to ASL — it does not replace P1 languages or introduce a second sim engine.
