# SVG asset staging

Canonical visual assets for profiles and `dendri-canvas` (DEN-13 / DEN-14):

```
content/assets/svg/
  boards/
  components/
  screens/
```

## Present (authored or staged from `core/`)

| Path | Source |
|---|---|
| `boards/arduino-uno-r3.svg` | MVP board (also `core/boards/`) |
| `boards/esp32-devkitc-v4.svg` | staged from `core/boards/` |
| `boards/raspberry-pi-pico.svg` | staged from `core/boards/` |
| `boards/siemens-s7-1200-cpu-1214c.svg` | staged from `core/boards/siemens-s7-1200-cpu-1214c-corrigido.svg` (filename fixed) |
| `components/led.svg` | MVP LED (`#led-body`) |
| `components/cmd-pushbutton-no-start.svg` | MVP button (`#actuator`, `#contact`) |

## Resolution

1. Prefer `content/assets/svg/{family}/{image}`
2. Else profile `[dendri].svg_path` if present on disk
3. Else legacy `core/{family}/{image}`

Missing assets stay `svg_status = "missing"` — warn, do not hard-fail.

See `GAPS.md` for the full missing-image inventory. No mass SVG stubs were invented;
author assets as needed for future canvas scenarios.
