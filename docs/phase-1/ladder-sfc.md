# Phase 1 — Ladder & SFC/GRAFCET (DEN-8)

**Status:** MVP subset landed. ASL still **unversioned** (do not assign `0.0.1`).

## Editor choice

| Choice | Detail |
|---|---|
| GUI | **iced 0.13** (`dendri-editor`) — pure Rust, Elm-style |
| Toolchain | **rustc 1.86.0** (MSRV bump from 1.85.1 required by Wayland/`calloop` stack used by iced) |
| Sync core | **`dendri-sync`** — headless, CI-tested; GUI is a thin shell |

Full canvas polish (drag handles, IEC glyphs, SVG board overlay) is incremental. Sync + IR are the solid path.

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

## Crates

| Crate | Role |
|---|---|
| `dendri-frontend-ladder` | Ladder diagram TOML ↔ ASL; series power-flow lower |
| `dendri-frontend-sfc` | GRAFCET/SFC TOML ↔ ASL; step-token state machine lower |
| `dendri-sync` | Bidirectional real-time `SyncSession` |
| `dendri-editor` | iced GUI (palette + TOML pane) |

## Honest MVP subset

**In**

- Ladder: NO/NC contacts, coils (normal/set/reset), Action cells (ASL stmts), Wire/Empty, layout `col`/`row`, optional TON lower
- SFC: steps with ASL actions, transitions with receptivity expr, layout `x`/`y`, linear charts
- Round-trip fixtures `L01`/`L02` / `G01`; P01 `[[sources]]` ladder/sfc filled; `[[asl.ladder]]` / `[[asl.grafcet]]` populated
- Button SVG authored (`cmd-pushbutton-no-start.svg`) + staged under `content/assets/svg/`

**Out / deferred**

- Parallel ladder branches, nested GRAFCET, full FB palette UX
- Drag-canvas polish, live SVG sim binding classes
- Claiming ASL `0.0.1`
- Rust user-language frontend (P2)

## Commands

```bash
cargo test -p dendri-frontends --test visual_acceptance
cargo run -p dendri-cli -- parse-source --lang ladder content/fixtures/ladder/L01-blink-actions.ladder.toml
cargo run -p dendri-cli -- parse-source --lang sfc content/fixtures/sfc/G01-blink-two-step.sfc.toml
cargo run -p dendri-cli -- frontend-roundtrip --lang ladder content/fixtures/sim/S01-blink-set-bool-wait.asl.toml
cargo run -p dendri-cli -- sync-visual content/fixtures/projects/P01-uno-led-blink.dendri.toml
cargo run -p dendri-editor
```
