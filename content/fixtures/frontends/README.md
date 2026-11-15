# Frontend fixtures (DEN-7)

Hand-checkable P1 language sources that parse into the shared ASL AST and run on `dendri-sim`.

| File | Language | Paired ASL |
|---|---|---|
| `S01-blink.cpp` | Arduino-style C++ | `../sim/S01-blink-set-bool-wait.asl.toml` |
| `S01-blink.py` | MicroPython | same |
| `S01-blink.st` | Structured Text | same |

Regenerate from ASL:

```bash
cargo run -q -p dendri-cli -- emit-cpp content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -q -p dendri-cli -- emit-micropython content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -q -p dendri-cli -- emit-st content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
```

Acceptance tests live in `crates/dendri-frontends/tests/frontend_acceptance.rs`.
