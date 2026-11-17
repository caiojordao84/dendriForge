# Phase 1 — Core + text frontends + Ladder/SFC

**Status:** MVP accepted; ASL language tag **`0.0.1`**

## Crates

| Crate | Role |
|---|---|
| `dendri-parse` | `.asl.toml` + `.dendri.toml` load; `expr_text` subset |
| `dendri-validate` | Semantic checks → `ERR_*` |
| `dendri-emit` | AST → TOML (AST-equality round-trip) |
| `dendri-backend-st` | Structured Text ↔ ASL AST (MVP subset) |
| `dendri-frontend-cpp` | Arduino-style C++ ↔ ASL AST (MVP subset) |
| `dendri-frontend-micropython` | MicroPython ↔ ASL AST (MVP subset) |
| `dendri-frontend-ladder` | Ladder diagram TOML ↔ ASL (DEN-8) |
| `dendri-frontend-sfc` | SFC/GRAFCET diagram TOML ↔ ASL (DEN-8) |
| `dendri-frontends` | Unified P1 language dispatch |
| `dendri-sync` | Bidirectional diagram ↔ TOML ↔ ASL session |
| `dendri-editor` | iced 0.13 Ladder/SFC visual editor MVP |
| `dendri-sim` | Scan-cycle ticks; interactive `step` / pin APIs; opcode traces |
| `dendri-canvas` | Live SVG binding for Uno + LED + button (DEN-13) |
| `dendri-ast` | ASL IR types + legacy ASL TOON→`.asl.toml` migrate (`toon`, DEN-20) |
| `dendri-profiles` | Load board/component/screen TOML profiles + SVG hooks; TOON→TOML migrate |
| `dendri-cli` | `dendri validate\|roundtrip\|sim\|canvas-sim\|profiles\|migrate-profiles\|migrate-asl\|emit-*\|…` |

Language coverage: [`frontends.md`](./frontends.md), [`ladder-sfc.md`](./ladder-sfc.md).

## Single-file project

Extension **`.dendri.toml`**: `[metadata]` + `[board]` + `[[components]]` + `[[wiring]]` + `[[sources]]` + embedded `[asl.*]` program.

`[[sources]].language` for `cpp` / `micropython` / `st` / `ladder` / `sfc` carry real source text. Ladder/SFC texts are diagram TOML sharing the same editor surface as textual languages.

## SVG hooks + live canvas

See `docs/phase-0/svg-readiness.md`. All boards/components/screens live under `content/profiles/` (DEN-14). MVP trio + staged board SVGs under `content/assets/svg/` (preferred) with `core/` fallback. `dendri-canvas` binds scan-cycle pins to SVG classes; iced **Sim SVG** pane + `dendri canvas-sim` for CI.

## Commands

```bash
cargo test --workspace
cargo run -p dendri-cli -- validate content/fixtures/examples/basic-blink-logic.asl.toml
cargo run -p dendri-cli -- roundtrip content/fixtures/golden/G01-gpio-all-modes-probe.asl.toml
cargo run -p dendri-cli -- sim content/fixtures/sim/S01-blink-set-bool-wait.asl.toml --cycles 2
cargo run -p dendri-cli -- canvas-sim content/fixtures/canvas/C01-button-led.dendri.toml --sequence 0,1,0
cargo run -p dendri-cli -- frontend-roundtrip --lang ladder content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- sync-visual content/fixtures/projects/P01-uno-led-blink.dendri.toml
cargo run -p dendri-editor
cargo run -p dendri-cli -- profiles
cargo run -p dendri-cli -- profiles --all
cargo run -p dendri-cli -- migrate-profiles
cargo run -p dendri-cli -- migrate-asl --dry-run
```

