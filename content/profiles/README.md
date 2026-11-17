# Profile library (TOML)

Board, component, and screen profiles live here as pure TOML (DEN-14).

```
content/profiles/
  boards/*.toml
  components/*.toml
  screens/*.toml
```

## Schema (common)

| Table | Role |
|---|---|
| `[metadata]` | `project_name`, `editor`, `author` (ASL version is on program IR, not profiles) |
| `[identification]` | `id`, `name`, category/family, `image` / `symbol` |
| `[specs]` / `[dims]` | Boards — flash/SRAM/clock and physical size |
| `[params.*]` | Components/screens — editable parameters |
| `[[power_pins]]` / `[[gpio]]` / `[[peripherals]]` | Boards — electrical map |
| `[[ports]]` (and pneumatic/hydraulic/protocol variants) | Components |
| `[simulation]` | Model + discrete states |
| `[canvas]` / `[canvas.port_*]` | Visual layout hooks |
| `[dendri]` | Packer ids, `svg_path`, `svg_status`, MVP overlays |

## SVG resolution

`[dendri].svg_path` points at `content/assets/svg/{boards,components,screens}/…`.
`dendri-profiles` / `dendri-canvas` prefer that tree and fall back to legacy `core/`
paths when needed (DEN-13). Missing SVGs warn (`svg_status = "missing"`); they do
not hard-fail validation.

## Migration

Legacy sources: `core/{boards,components,screens}/*.toon` (read-only).

```bash
cargo run -p dendri-cli -- migrate-profiles
cargo run -p dendri-cli -- profiles --all
```

ASL language tag **`0.0.1`** lives on program IR (`metadata.asl_version`), not on profiles.
