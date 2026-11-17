# SVG readiness (Phase 0 — no visual library build)

SVGs are **critical at simulation time**. Phase 0 does **not** ship a visual
library; it freezes conventions, inventories the gap, and picks the MVP set so
Phase 1 / `dendri-sim` do not invent asset paths later.

## Asset conventions

| Rule | Detail |
|---|---|
| Path field | Profiles use `image` (board photo/render) and optional `symbol` (schematic glyph) |
| Relative root | Paths are relative to the profile’s family directory (`boards/`, `components/`, `screens/`) today under `core/`; Phase 1 packer should resolve via `content/assets/svg/{boards,components,screens}/` once assets are relocated |
| Naming | `{profile-id}.svg` for board/component body; `{profile-id}-symbol.svg` for IEC/ISO schematic symbol |
| Coordinate space | Component canvas uses profile `canvas.width` / `canvas.height` with port anchors in the same space |
| Optional until renderer | Missing SVGs must **warn**, not hard-fail validate (Phase 0 policy) |
| Standards | Prefer `symbol_standard` (`IEC`, `ISO 1219-1`, …) when a symbol SVG exists |
| Simulation binding | Sim state drives CSS/SMIL classes on known ids: e.g. LED `#led-body` on/off/fault; button `#actuator` pressed |

### Required ids for MVP interactive parts

| Asset | Required element ids / classes |
|---|---|
| Board (`arduino-uno-r3.svg`) | Pin pads groupable by digital pin label; status LED near D13 optional |
| LED (`led.svg`) | `#led-body` (fill/opacity + `on`/`off` class), `#anode`, `#cathode` optional |
| Pushbutton (`cmd-pushbutton-no-start.svg`) | `#actuator` (press transform / `pressed` class), `#contact` (NO `open`/`closed`) |
| Board status | `#led-pin13` `data-state` + class follows digital pin 13 |

## Gap inventory (re-verified from state map / checkout)

| Class | Refs in profiles | Matching files on disk | Gap |
|---|---|---|---|
| Board `image:` | ≈59 | **3** matching (+ Siemens filename mismatch `*-corrigido.svg`) | ~56 missing |
| Component `image` / `symbol` | ≈300 | **1** (`led.svg`) | ~299 missing |
| Screen image/symbol | 4 | **0** | 4 missing |

### SVGs present today

```
core/boards/arduino-uno-r3.svg
core/boards/esp32-devkitc-v4.svg
core/boards/raspberry-pi-pico.svg
core/boards/siemens-s7-1200-cpu-1214c-corrigido.svg   # profile id expects siemens-s7-1200-cpu-1214c.svg
core/components/led.svg
```

## Visual MVP set (locked for Phase 1 sim)

| Role | Profile id | Required SVG path (do not invent later) |
|---|---|---|
| Board | `arduino-uno-r3` | `core/boards/arduino-uno-r3.svg` → later `content/assets/svg/boards/arduino-uno-r3.svg` |
| Indicator | `led` | `core/components/led.svg` → later `content/assets/svg/components/led.svg` |
| Input | `cmd-pushbutton-no-start` | `core/components/cmd-pushbutton-no-start.svg` (**authored DEN-8**; also staged under `content/assets/svg/components/`) |
| Symbol (LED) | `led` | `led-symbol.svg` (MISSING; optional for MVP canvas) |
| Symbol (button) | `cmd-pushbutton-no-start` | `cmd-pushbutton-no-start-symbol.svg` (MISSING; optional) |

Sample TOML profiles for this trio live under `content/profiles/`.

## Phase 1 note

Sim may run headless without SVGs. Visual sim attaches the MVP trio only;
all other `image`/`symbol` refs stay optional warnings.
