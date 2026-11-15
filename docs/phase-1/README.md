# Phase 1 — Core + text frontends

**Status:** in progress toward MVP (ASL still **unversioned**; do **not** assign `0.0.1` yet)

## Crates

| Crate | Role |
|---|---|
| `dendri-parse` | `.asl.toml` + `.dendri.toml` load; `expr_text` subset |
| `dendri-validate` | Semantic checks → `ERR_*` |
| `dendri-emit` | AST → TOML (AST-equality round-trip) |
| `dendri-backend-st` | Structured Text ↔ ASL AST (MVP subset) |
| `dendri-frontend-cpp` | Arduino-style C++ ↔ ASL AST (MVP subset) |
| `dendri-frontend-micropython` | MicroPython ↔ ASL AST (MVP subset) |
| `dendri-frontends` | Unified P1 text dispatch |
| `dendri-sim` | Scan-cycle ticks; `SET_BOOL` / `GET_BOOL` / `WAIT_TIME` / `TON` traces |
| `dendri-profiles` | Load board/component profiles + SVG hooks |
| `dendri-cli` | `dendri validate\|roundtrip\|sim\|emit-*\|parse-source\|frontend-roundtrip\|…` |

Language coverage limits: [`frontends.md`](./frontends.md).

## Single-file project

Extension **`.dendri.toml`**: `[metadata]` + `[board]` + `[[components]]` + `[[wiring]]` + `[[sources]]` + embedded `[asl.*]` program.

`[[sources]].language` for `cpp` / `micropython` / `st` carry real source text (DEN-7). `ladder` / `sfc` remain DEN-8 stubs.

## SVG hooks (logical sim only here)

See `docs/phase-0/svg-readiness.md`. Profiles load for `arduino-uno-r3`, `led`, `cmd-pushbutton-no-start`. Button SVG still missing.

## Commands

```bash
cargo test --workspace
cargo run -p dendri-cli -- validate content/fixtures/examples/basic-blink-logic.asl.toml
cargo run -p dendri-cli -- roundtrip content/fixtures/golden/G01-gpio-all-modes-probe.asl.toml
cargo run -p dendri-cli -- sim content/fixtures/sim/S01-blink-set-bool-wait.asl.toml --cycles 2
cargo run -p dendri-cli -- emit-st content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- emit-cpp content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- frontend-roundtrip --lang micropython content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- profiles
```
