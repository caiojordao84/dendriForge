# Frontend fixtures (DEN-7 / DEN-10)

Hand-checkable P1 language sources that parse into the shared ASL AST and run on `dendri-sim`.

| File | Language | Paired ASL |
|---|---|---|
| `S01-blink.{cpp,py,st}` | C++ / MicroPython / ST | `../sim/S01-blink-set-bool-wait.asl.toml` |
| `S02-ton.{cpp,py,st}` | C++ / MicroPython / ST | `../sim/S02-ton-on-delay.asl.toml` |
| `S03-control-flow.{cpp,py,st}` | C++ / MicroPython / ST | `../sim/S03-control-flow-get-bool.asl.toml` |

Coverage intent:

- **S01** — blink: `SET_BOOL` + `WAIT_TIME` + TIME consts
- **S02** — `TON` + `CFG_DIGITAL_INPUT` + DI/DO + BOOL vars
- **S03** — `GET_BOOL` + if/elif/else + `for` + BOOL/UINT/INT/TIME

Regenerate from ASL:

```bash
cargo run -q -p dendri-cli -- emit-cpp content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -q -p dendri-cli -- emit-micropython content/fixtures/sim/S02-ton-on-delay.asl.toml
cargo run -q -p dendri-cli -- emit-st content/fixtures/sim/S03-control-flow-get-bool.asl.toml
```

Acceptance tests live in `crates/dendri-frontends/tests/frontend_acceptance.rs`.
