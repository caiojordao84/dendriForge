# Phase 1 — Core parse / validate / emit / sim

**Status:** in progress toward MVP (ASL still **unversioned**; do **not** assign `0.0.1` yet)

## Crates

| Crate | Role |
|---|---|
| `dendri-parse` | `.asl.toml` + `.dendri.toml` load; `expr_text` subset |
| `dendri-validate` | Semantic checks → `ERR_*` |
| `dendri-emit` | AST → TOML (AST-equality round-trip) |
| `dendri-backend-st` | AST → ST emit **stub** (same AST as sim) |
| `dendri-sim` | Scan-cycle ticks; `SET_BOOL` / `GET_BOOL` / `WAIT_TIME` / `TON` traces |
| `dendri-profiles` | Load board/component profiles + SVG hooks |
| `dendri-cli` | `dendri validate\|roundtrip\|sim\|emit-st\|info\|profiles` |

## Single-file project

Extension **`.dendri.toml`**: `[metadata]` + `[board]` + `[[components]]` + `[[wiring]]` + `[[sources]]` + embedded `[asl.*]` program.

`[[sources]].language` hooks for P1 frontends (`cpp`, `micropython`, `st`, `ladder`, `sfc`) — **not implemented** in this milestone.

## SVG hooks (logical sim only here)

See `docs/phase-0/svg-readiness.md`. Profiles load for `arduino-uno-r3`, `led`, `cmd-pushbutton-no-start`. Button SVG still missing.

## Commands

```bash
cargo test --workspace
cargo run -p dendri-cli -- validate content/fixtures/examples/basic-blink-logic.asl.toml
cargo run -p dendri-cli -- roundtrip content/fixtures/golden/G01-gpio-all-modes-probe.asl.toml
cargo run -p dendri-cli -- sim content/fixtures/sim/S01-blink-set-bool-wait.asl.toml --cycles 2
cargo run -p dendri-cli -- emit-st content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- profiles
```
