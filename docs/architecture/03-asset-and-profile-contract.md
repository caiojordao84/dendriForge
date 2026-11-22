# 03 — Strict Asset & Profile Contract (TOML ↔ SVG)

> **Normative Specification** | Decisions locked in Round 4 (ADRs 013–016)  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md), [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md)

---

## 1. Zero-Drift Asset Philosophy (ADR-016: Certified Catalog Policy)

In v1, 215 TOML profiles existed while only 11 SVGs and 7 simulated roles existed — producing 563 broken SVG paths, 154 corrupted `[canvas]` tables, 29 corrupted `[limits]` tables, and zero validation.

In v2, **every entry in the live catalog (`assets/`) must pass the automated Asset Validator Gate (`dendri asset-check`) in CI**:
1. **No profile without validated SVG(s) AND simulation model:** An asset cannot exist in `assets/` as a stub or broken link.
2. **Legacy quarantine:** All 215 v1 profiles remain archived in `legacy/` solely as engineering datasheets. Assets graduate into v2 only when authored to full v2 contract compliance.
3. **Initial Certified Core Kit (Wave 1–2):**
   - **4 Boards:** `arduino-uno-r3`, `esp32-devkitc-v4`, `raspberry-pi-pico`, `siemens-s7-1200-cpu-1214c` (each with full pinout, real electrical thresholds, ADC scale, PWM/bus capabilities, and validated physical SVG).
   - **~20 Essential Components:** Resistor, Potentiometer, Capacitor, Inductor, Diode, LED (multi-color), Zener, BJT NPN/PNP, MOSFET N, Pushbutton NO, Pushbutton NC, Tactile 4-Pin, Pilot Lamp, Industrial Relay, Breadboard, DC Voltage Source, AC 1φ/3φ Source, 4–20 mA Pressure Transmitter, I2C LCD/OLED, 5/2 Solenoid Valve + Double-Acting Pneumatic Cylinder + Limit Switch.
4. **Strict TOML schemas (`#[serde(deny_unknown_fields)]`):** Zero untyped `rest: Map` dumping grounds; zero silent field drops; zero path-traversal vulnerabilities (profile IDs strictly match `^[a-z0-9]+(-[a-z0-9]+)*$`).

---

## 2. Canonical 48×48 px Grid & 8 px Semantic Pin Contract (ADR-014)

Every board and component asset adheres to a single, mathematically rigid spatial system so wires, pins, rotation (`0/90/180/270`), and mirroring (`mirror_x`/`mirror_y`) **never** drift or collide:

### 2.1 Grid Coordinate System
- **1 Grid Module (`1 u`) = `48 px`** in SVG user coordinates (`viewBox="0 0 W H"`).
- **Sub-grid step:** `0.5 u = 24 px` (or `0.25 u = 12 px` for dense MCU pin headers where required, declared explicitly in the profile).
- **Exact SVG `viewBox`:** Every SVG MUST declare explicit numeric `viewBox="0 0 W H"` where `W` and `H` are exact multiples of the grid pitch (`48 px`, or `24 px` half-module), matching `[geometry].grid_w` and `[geometry].grid_h` in the TOML profile with tolerance `<= 1e-3`.
- **Single Transform Order Everywhere:** Both canvas geometry/hit-testing AND GPU rendering use the exact same transform matrix:
  $$\mathbf{M} = \mathbf{T}(x, y) \cdot \mathbf{T}(c_x, c_y) \cdot \mathbf{R}(\theta) \cdot \mathbf{S}(m_x, m_y) \cdot \mathbf{T}(-c_x, -c_y)$$
  (mirror in local centered space first, then quadrant rotation, then translate to world pose — verified by automated transform-parity tests).

### 2.2 SVG Pin Marker Layer (`<g id="df-pins">`) — 1:1 Source of Truth
Every SVG (both physical AND schematic views) **MUST** contain a dedicated metadata group `<g id="df-pins">` (hidden during base SVG body rendering and instead rendered natively by the canvas engine):
- Every port declared in the TOML profile **MUST** have **exactly one** matching element inside `<g id="df-pins">`:
  ```xml
  <g id="df-pins">
    <circle id="pin:D13" class="df-pin-digital" cx="48" cy="48" r="4" />
    <circle id="pin:GND" class="df-pin-gnd"     cx="96" cy="48" r="4" />
    <circle id="pin:A0"  class="df-pin-analog"  cx="48" cy="240" r="4" />
  </g>
  ```
- **Exact 8 px terminal geometry:** Every pin/terminal marker MUST be `8×8 px` (`r="4"` circle or `width="8" height="8"` square terminal block) centered on a valid grid snap point `(cx, cy)`.
- **Zero hardcoded pin hex colors in SVG:** Pins declare **semantic utility classes** only. The renderer paints every 8 px pin/terminal dynamically via `theme.rs` using the project's **A11Y Slate palette** (every token verified for WCAG contrast compliance!):

| Semantic SVG Class | TOML `port.role` | Domain | Rendered by `theme.rs` Token |
|---|---|---|---|
| `df-pin-power` | `power` | Electrical (VCC / 5V / 3V3 / 24V / L+) | `DF_PIN_POWER` |
| `df-pin-gnd` | `gnd` | Electrical (GND / 0V / M / PE) | `DF_PIN_GND` |
| `df-pin-ac-phase` | `ac_phase` | Electrical (L1 / L2 / L3 / N) | `DF_PIN_AC_PHASE` |
| `df-pin-digital` | `digital` | Electrical GPIO / DI / DO | `DF_PIN_DIGITAL` |
| `df-pin-analog` | `analog` | Electrical AI / AO / ADC | `DF_PIN_ANALOG` |
| `df-pin-pwm` | `pwm` | Electrical PWM / Timer Out | `DF_PIN_PWM` |
| `df-pin-comm` | `comm` | UART / I2C / SPI / CAN / Ethernet | `DF_PIN_COMM` |
| `df-pin-interrupt` | `interrupt` | External Interrupt / E-Stop | `DF_PIN_INTERRUPT` |
| `df-pin-reset` | `reset` | Reset / Enable / Control | `DF_PIN_RESET` |
| `df-pin-passive` | `passive` | Unpolarized Passive Terminal (R/C/L) | `DF_PIN_PASSIVE` |
| `df-pin-pneumatic` | `pneumatic` | ISO 1219 Air Port (1/2/3/4/5) | `DF_PIN_PNEUMATIC` |
| `df-pin-hydraulic` | `hydraulic` | ISO 1219 Oil Port (P/T/A/B) | `DF_PIN_HYDRAULIC` |

- **Zero Terminal Collisions:** The Asset Validator checks that no two pins in `<g id="df-pins">` share the same `(cx, cy)` coordinate (minimum separation `>= 8 px`).

---

## 3. Mandatory Dual-View SVG Policy (ADR-015)

Every **component** profile in `assets/components/<id>/` MUST ship **both** views as a self-contained bundle:
```
assets/components/led/
├── profile.toml          # Strict v2 component contract
├── physical.svg          # Mandatory: realistic bancada/panel view (48px grid + df-pins)
└── schematic.svg         # Mandatory: IEC 60617 / ISO 1219 symbol (48px grid + df-pins)
```
Every **board** profile in `assets/boards/<id>/`:
```
assets/boards/siemens-s7-1200-cpu-1214c/
├── profile.toml          # Strict v2 board contract (MCU/PLC specs, pins, ADC/PWM/bus caps)
├── physical.svg          # Mandatory: realistic hardware view (48px grid + df-pins)
└── schematic.svg         # Optional: IEC schematic pinout block (48px grid + df-pins)
```
- **1:1 Port Parity Across Views:** `physical.svg` and `schematic.svg` MUST declare the **exact same set of `pin:<port_id>` IDs** as `profile.toml`. Switching the canvas between Physical View and Schematic View preserves every wire seamlessly!

---

## 4. Declarative Dynamic SVG Binding Contract (ADR-013)

### Forbidden SVG Features (Enforced by Asset Validator)
To guarantee 100% deterministic, zero-corruption rendering via a single workspace `usvg` version:
- **FORBIDDEN:** `<?xml-stylesheet ...?>` external CSS files.
- **FORBIDDEN:** CSS custom properties (`var(--...)`).
- **FORBIDDEN:** SMIL animations (`<animate>`, `<set>`, `<animateTransform>`).
- **FORBIDDEN:** `<script>`, `<foreignObject>`, external `<image href="http...">`, or inline base64 raster blobs.
- **FORBIDDEN:** Runtime string/regex mutation of SVG XML text!

### Supported Declarative Bindings (`[svg.bindings]` in `profile.toml`)
Every dynamic visual element in the SVG has a stable `id="..."` on a `<g>` or `<path>` element (parsed **once** into cached GPU path geometry at asset load time) and is driven purely by numeric `ComponentVisualState` values emitted from `dendri-circuit` / `dendri-physics`:

1. **`visibility`:** Show/hide an SVG layer based on boolean or enum state (e.g., `#contact-closed` vs `#contact-open`, `#fault-burn-overlay`).
2. **`paint`:** Override fill/stroke color and opacity from a normalized scalar `0.0..=1.0` or component parameter (e.g., `#led-die` + `#led-glow` driven by LED forward current $I_F$ and `params.color` — fixing v1's red-only hardcoded glow!).
3. **`translate`:** Linear 2D displacement `(dx, dy)` in SVG px proportional to a state scalar `0.0..=1.0` (e.g., `#piston-rod` translating along cylinder stroke $x(t)$, `#actuator-cap` depressing `+4 px` when pressed).
4. **`rotate`:** Rotation `deg` around explicit pivot `(cx, cy)` proportional to a state scalar (e.g., `#wiper-knob` rotating `-135°..+135°` for potentiometer wiper position, gauge needle).
5. **`overlay_rect`:** Named `<rect id="overlay:screen">` bounds in `<g id="df-overlays">` where the native canvas draws crisp live vector/bitmap content (e.g., 16×2 LCD character grid, 128×64 OLED pixel buffer, 7-segment display, live pressure numeric readout — eliminating `usvg` `<text>` font dependencies!).

---

## 5. Strict TOML Profile Schemas (Normative Outline)

### 5.1 Component Profile (`profile.toml`)
```toml
schema_version = "2.0"

[meta]
id = "led"
name = "Light Emitting Diode (5mm)"
category = "indicator"
standards = ["IEC 60810", "IEC 60617"]

[geometry]
grid_px = 48
pin_px = 8
physical_grid_w = 2
physical_grid_h = 3
schematic_grid_w = 2
schematic_grid_h = 2

[svg]
physical = "physical.svg"
schematic = "schematic.svg"

[[ports]]
id = "A"
label = "Anode (+)"
domain = "electrical"
role = "passive"          # maps to SVG class df-pin-passive

[[ports]]
id = "K"
label = "Cathode (-)"
domain = "electrical"
role = "passive"

[params.color]
type = "enum"
options = ["red", "green", "blue", "yellow", "white", "amber"]
default = "red"

[params.vf_v]
type = "real"
unit = "V"
min = 1.2
max = 4.5
default = 2.0

[params.if_max_a]
type = "real"
unit = "A"
min = 0.005
max = 0.100
default = 0.030

[params.vr_max_v]
type = "real"
unit = "V"
default = 5.0

[physics]
model = "spice_led"       # Strongly-typed enum dispatching into dendri-circuit / dendri-physics

[[svg.bindings]]
target_id = "led-body"
kind = "paint"
source_signal = "luminosity"     # 0.0..=1.0 from spice_led forward current
color_from_param = "color"

[[svg.bindings]]
target_id = "led-burned"
kind = "visibility"
source_signal = "burned"
```

### 5.2 Board Profile (`profile.toml`)
Declares strict `[electrical]` (`logic_high_v`, `v_ih_v`, `v_il_v`, `pin_r_out_ohm`, `pin_i_max_a`, `v_abs_max_v`, `adc_bits`, `adc_v_ref_v`, `adc_native_max`, `pullup_ohm`, `pulldown_ohm`), `[execution]` (`default_mode = "cooperative" | "plc_scan"`, `default_scan_period_us`, `clock_hz`), `[svg]`, and `[[ports]]` (with explicit capabilities: `digital_in`, `digital_out`, `pullup`, `pulldown`, `analog_in`, `pwm`, `interrupt`, `uart_tx/rx`, `i2c_sda/scl`, `spi_*`, `ethernet`).

---

## 6. Automated Asset Validator Gate (`dendri asset-check`)

Executed in CI on every commit — **zero warnings allowed**:
1. Every directory under `assets/{boards,components,screens}/` deserializes cleanly with `#[serde(deny_unknown_fields)]`.
2. `meta.id` matches folder name exactly; globally unique across catalog.
3. All referenced SVGs exist and parse cleanly via the workspace's single `usvg` version.
4. Zero forbidden SVG constructs (`var(`, `xml-stylesheet`, `<animate`, `<script`, `<foreignObject>`).
5. `viewBox` width/height match `[geometry]` (`grid_w * 48`, `grid_h * 48`).
6. Every view has `<g id="df-pins">` with **exact 1:1 bijection** between `pin:<id>` elements and `[[ports]]` in `profile.toml`.
7. Every pin marker is `8×8 px`, snapped to grid pitch, non-overlapping (`dist >= 8 px`), and carries the exact `df-pin-*` CSS class matching `port.role`.
8. Every `[[svg.bindings]].target_id` exists in the declared SVG(s) (`<g>`/`<path>`/`<rect>`).
9. `[physics].model` resolves to a registered implementation in `dendri-circuit` / `dendri-physics` with unit-tested electrical/physical invariants.
10. Every standard string in `meta.standards` resolves against the **actually loaded & validated** `assets/standards.toml` registry!
