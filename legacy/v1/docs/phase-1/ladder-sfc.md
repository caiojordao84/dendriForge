# Phase 1 — Ladder & SFC/GRAFCET (DEN-8 → DEN-12)

**Status:** Beyond-linear subset landed (parallel OR, AND fork/join, IEC FB lower). ASL language tag **`0.0.1`**.

## Editor choice

| Choice | Detail |
|---|---|
| GUI | **iced 0.13** (`dendri-editor`) — pure Rust, Elm-style |
| Toolchain | **rustc 1.86.0** (MSRV bump from 1.85.1 required by Wayland/`calloop` stack used by iced) |
| Sync core | **`dendri-sync`** — headless, CI-tested; GUI is a thin shell |

Full canvas polish (drag handles, IEC glyphs, SVG board overlay) is incremental — see DEN-13. Sync + IR are the solid path.

## Sync design

```
DiagramModel  ←→  [[sources]].text (diagram TOML)  ←→  [asl] Program
                      │ ladder / sfc                     │ [[ladder]] / [[grafcet]]
                      └──────── SyncSession ─────────────┘
```

- **Diagram edit** → serialize `LadderDocument` / `GrafcetDocument` → update matching `[[sources]]` + ASL blocks; optionally **lower** into `setup`/`loop` for sim.
- **TOML edit** → parse source text → replace diagram + ASL blocks (+ lower).
- **ASL edit** → emit ladder/sfc documents into `[[sources]]` (synthesize Action/step views when blocks empty).
- Determinism: AST equality after round-trips (not byte-identical TOML).
- Edit sequences (add / remove / reorder) covered by `dendri-sync` unit tests.

## Crates

| Crate | Role |
|---|---|
| `dendri-frontend-ladder` | Ladder diagram TOML ↔ ASL; series + parallel OR + IEC FB lower |
| `dendri-frontend-sfc` | GRAFCET/SFC TOML ↔ ASL; two-phase step-token machine (AND fork/join) |
| `dendri-sync` | Bidirectional real-time `SyncSession` |
| `dendri-editor` | iced GUI (palette + TOML pane) |

## Honest subset (DEN-12 → DEN-19)

**In**

- Ladder: NO/NC contacts, coils (normal/set/reset), Action cells, Wire/Empty, layout `col`/`row`
- Ladder: **one-level** `Parallel` OR rails (`LadderBranch`)
- Ladder: IEC FBs lowered to sim opcodes — `TON` / `TOF` / `TP`, `CTU` / `CTD` / `CTUD`, `R_TRIG` / `F_TRIG` (`FbEdge`)
- SFC: steps with ASL actions, transitions with receptivity expr, layout `x`/`y`
- SFC: **AND divergence / convergence** via multi `from` / `to`; two-phase fire then apply
- SFC: **nested / macro-steps** via `GrafcetStep.enclosed` (namespaced `__sfc_<parent>__<child>` tokens; rising-edge init / clear on exit) — DEN-19
- Fixtures `L01`–`L05` / `G01`–`G04`; P01 visual sources; visual acceptance + sync edit-sequence tests

**Out / deferred**

- Deep OR networks / vertical wire matrix / multi-level parallel nesting
- Drag-canvas polish for nested charts, live SVG sim binding classes
- Rust user-language frontend (P2)

## Commands

```bash
cargo test -p dendri-frontends --test visual_acceptance
cargo test -p dendri-sync --lib
cargo run -p dendri-cli -- parse-source --lang ladder content/fixtures/ladder/L03-parallel-or.ladder.toml
cargo run -p dendri-cli -- parse-source --lang sfc content/fixtures/sfc/G02-and-divergence.sfc.toml
cargo run -p dendri-cli -- frontend-roundtrip --lang ladder content/fixtures/sim/S01-blink-set-bool-wait.toml
cargo run -p dendri-cli -- sync-visual content/fixtures/projects/P01-uno-led-blink.toml
cargo run -p dendri-editor
```
