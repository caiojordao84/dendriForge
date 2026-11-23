# 03 — Strict Asset & Profile Contract (TOML ↔ SVG)

> **Normative Specification** | Baseline (Rev 2) — ADRs 013–016, 025, 029, 030  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md), [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md), [`02-simulation-and-electrical-engine.md`](./02-simulation-and-electrical-engine.md)

---

## 1. Zero-Drift Asset Philosophy & Certified Catalog Policy (ADR-016, ADR-030)

In v1, 215 TOML profiles existed while only 11 SVGs and 7 simulated roles existed — producing 563 broken SVG paths, 154 corrupted `[canvas]` tables, 29 corrupted `[limits]` tables, and zero validation.

In v2, **every entry in the live catalog (`assets/`) MUST pass the automated Asset Validator Gate (`dendri asset-check`) in CI with zero warnings**.

### 1.1 Complete Certified Component Unit
A component is considered **done** ONLY when it ships ALL eight verified artifacts/properties:
1. Strict `profile.toml` (`#[serde(deny_unknown_fields)]`, validated schema).
2. Registered electrical/physical simulation model in `dendri-circuit` / `dendri-physics` (or validated composite sub-circuit, §1.3) with analytical unit tests.
3. `physical.svg` conforming to the 48×48 px grid, 8 px `<g id="df-pins">`, and canonical sanitization pipeline (§2, §4).
4. Original IEC 60617-style `schematic.svg` conforming to IEC 61082 conventions, 48×48 px grid, 8 px `<g id="df-pins">`, and sanitization pipeline (§2, §3, §4).
5. Exact **1:1 `df-pins` port parity** across `profile.toml`, `physical.svg`, and `schematic.svg`.
6. Complete `[provenance]` metadata in `profile.toml` and inside each SVG (§4.1).
7. Human review sign-off recorded in `review.toml` with a verified canonical render hash (§4.3).
8. At least one end-to-end simulation scenario exercising the component against an independent oracle.

### 1.2 Phased Catalog Certification Rollout & Legacy Quarantine
- **Legacy Quarantine:** All 215 v1 profiles remain archived in `legacy/v1/` strictly as engineering datasheet references. No v1 asset or fixture graduates into `assets/` without being re-authored to full v2 compliance.
- **Phase 3 (Headless Solver Benchmarks — Zero Asset Dependency):** All analytical solver benchmarks (R, C, L, AC, diode, LED, Zener, BJT, MOSFET, power devices) run **100% headless** inside `dendri-circuit` with zero dependency on Phase 4 SVGs.
- **Phase 4 — Narrowed Certified Core Kit:**
  - **2 Boards:** `arduino-uno-r3` and `plc-1200-open-1214` (original open-brand industrial 24 V DC / IEC 61131-2 Type 1 PLC styled in differentiated **A11Y Slate** colors — zero third-party PLC brand marks; each board ships complete pinout, electrical thresholds, `[cpu]` (`int_bits`, `clock_hz`), `[execution.cost]`, `indeterminate_reads_as`, per-channel codecs + ranges + band tables with `source_ref` (`DEF-002`), PWM transfer functions (`DEF-001`), `iec_address` port mapping, validated SVGs, provenance, `review.toml`, and ≥1 end-to-end scenario).
  - **8 Mirrored Components:** `maker-tactile-4pin`, `cmd-pushbutton-no-start`, `cmd-pushbutton-nc-stop`, `led`, `signal-pilot-lamp`, `potentiometer`, `sensor-pressure-4-20ma`, `resistor` (including 500 Ω shunt configuration).
- **Phase 5 — Subsystem Batch Certification:** Bus devices, display components (LCD/OLED/7-seg with optional `charset` tables, `DEF-007`), and electro-pneumatic components (5/2 solenoid valve, double-acting cylinder, limit switch) are certified as a **batch** under the full §1.1 complete unit without blocking the Phase 4 Core Kit.
- **Wave 2 Gate:** `esp32-devkitc-v4` and `raspberry-pi-pico` are certified at the Wave 2 gate alongside exact per-port MicroPython ADC/PWM scaling verified from target port source code.

### 1.3 Composite Components & Strict Profile ID Grammar
- **Composite Components:** Components that are electrically compositions of primitive parts (e.g., `signal-pilot-lamp` = LED + series current-limiting resistor) MAY be declared in `profile.toml` as a declarative TOML sub-circuit of registered primitive physics models with their own analytical tests, rather than duplicating low-level physical parameters.
- **Strict ID & Schema Safety:**
  - Every schema struct MUST use `#[serde(deny_unknown_fields)]` — zero untyped `rest: Map` dumping grounds and zero silent field drops.
  - **Profile IDs** MUST match `^[a-z0-9]+(-[a-z0-9]+)*$` (kebab-case; prevents path traversal and matches folder names 1:1). Note that project **instance IDs** match `^[a-z][a-z0-9_]*$` (snake_case, no dots — see [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md#24-identifier-grammar-endpoints--hardware-io-bindings-hardware)).

---

## 2. Canonical 48×48 px Grid & 8 px Semantic Pin Contract (ADR-014)

Every board and component asset adheres to a single, mathematically rigid spatial system so wires, pins, rotation (`0°/90°/180°/270°`), and mirroring (`mirror_x`/`mirror_y`) **never** drift or collide.

### 2.1 Grid Coordinate System
- **1 Grid Module (`1 u`) = `48 px`** in SVG user coordinates (`viewBox="0 0 W H"`).
- **Sub-grid step:** `0.5 u = 24 px` (or `0.25 u = 12 px` for dense MCU pin headers where required, declared explicitly in the profile).
- **Exact SVG `viewBox`:** Every SVG MUST declare an explicit numeric `viewBox="0 0 W H"` where `W` and `H` are exact multiples of the grid pitch (`48 px`, or `24 px` half-module), matching `[geometry].*_grid_w` and `[geometry].*_grid_h` in `profile.toml` within tolerance `<= 1e-3`.
- **Single Transform Order Everywhere:** Both canvas geometry/hit-testing AND GPU rendering MUST use the exact same transform matrix:
  $$\mathbf{M} = \mathbf{T}(x, y) \cdot \mathbf{T}(c_x, c_y) \cdot \mathbf{R}(\theta) \cdot \mathbf{S}(m_x, m_y) \cdot \mathbf{T}(-c_x, -c_y)$$
  (mirror in local centered space first, then quadrant rotation, then translate to world pose — verified by automated transform-parity tests).

### 2.2 SVG Pin Marker Layer (`<g id="df-pins">`) — 1:1 Source of Truth
Every SVG (both physical AND schematic views) **MUST** contain a dedicated metadata group `<g id="df-pins">` (hidden during base SVG body rendering and instead rendered natively by the canvas engine):
- Every port declared in `profile.toml` **MUST** have **exactly one** matching element inside `<g id="df-pins">`:
  ```xml
  <g id="df-pins">
    <circle id="pin:D13" class="df-pin-digital" cx="48" cy="48" r="4" />
    <circle id="pin:GND" class="df-pin-gnd"     cx="96" cy="48" r="4" />
    <circle id="pin:A0"  class="df-pin-analog"  cx="48" cy="240" r="4" />
  </g>
  ```
- **Exact 8 px Terminal Geometry:** Every pin/terminal marker MUST be `8×8 px` (`r="4"` circle or `width="8" height="8"` square terminal block) centered on a valid grid snap point `(cx, cy)`.
- **Zero Hardcoded Pin Colors in SVG:** Pin markers MUST declare **semantic utility classes only** (no inline `fill`/`stroke` hex colors). The renderer paints every 8 px pin/terminal dynamically via `dendri-theme` using the **A11Y Slate palette** (every token verified by automated WCAG contrast tests):

| Semantic SVG Class | TOML `port.role` | Domain | Rendered by `dendri-theme` Token |
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

- **Zero Terminal Collisions:** `dendri asset-check` verifies that no two pins in `<g id="df-pins">` share or overlap coordinates (center-to-center distance `>= 8 px`).

---

## 3. Mandatory Dual-View SVG Policy & Symbology Standard (ADR-015, ADR-030)

### 3.1 Self-Contained Asset Bundles
Every **component** in `assets/components/<id>/` MUST ship both views plus its review sign-off:
```text
assets/components/led/
├── profile.toml          # Strict v2 component contract + [provenance]
├── physical.svg          # Mandatory: realistic bench/panel view (48px grid + df-pins)
├── schematic.svg         # Mandatory: original IEC 60617-style symbol (48px grid + df-pins)
└── review.toml           # Mandatory: human review sign-off + canonical render hash
```
Every **board** in `assets/boards/<id>/`:
```text
assets/boards/plc-1200-open-1214/
├── profile.toml          # Strict v2 board contract + [provenance]
├── physical.svg          # Mandatory: realistic hardware view (48px grid + df-pins)
├── schematic.svg         # Optional: IEC 61082-style schematic pinout block (48px grid + df-pins)
└── review.toml           # Mandatory: human review sign-off + canonical render hash
```
- **1:1 Port Parity Across Views:** `physical.svg` and `schematic.svg` MUST declare the **exact same set of `pin:<port_id>` IDs** as `profile.toml`. Switching the canvas between Physical View and Schematic View preserves every wire seamlessly.

### 3.2 Symbology & Copyright Policy (IEC 60617-Style + IEC 61082)
Schematic symbols are **original drawings in the style of IEC 60617**, presented according to **IEC 61082** (document-preparation conventions for electrotechnical documents) as its complement (and ISO 1219-style conventions for pneumatic/hydraulic components). They are *similar to*, not copies of, copyrighted IEC database graphics:
- **Copyright Protection:** The IEC 60617 database is copyrighted by the IEC. It is used strictly **as a reference for symbol identity** — zero graphic content MAY be copied, traced, or extracted from IEC databases or publications.
- **Identification Metadata:** Every schematic SVG (and its profile `[provenance]`) MUST carry `symbol_ref` identifying the closest IEC 60617 symbol (e.g., `IEC 60617-S00669`) for identification only.
- **Accurate Description:** Documentation and UI MUST describe these assets as **"IEC 60617-style"**. A claim of formal standard conformity MAY ONLY be made after an explicit conformity review recorded in `review.toml`.
- **Reference Designations:** Standard schematic reference-designation schemes (e.g., `-S1`, `-H1`, `-R1`, `-K1` per IEC 81346 / IEC 61082) are finalized during Phase 4 (`DEF-009`).

---

## 4. Provenance, SVG Sanitization Pipeline & Review Sign-Off (ADR-030)

### 4.1 Source-Neutral `[provenance]` Block & License Compliance
Every `profile.toml` and every SVG (`<metadata>`) MUST carry a validated `[provenance]` block. **`source = "original"` is the norm**; third-party-derived artwork is an explicit exception requiring full attribution and a matching entry in the repository root `THIRD_PARTY_NOTICES`:

```toml
[provenance]
source        = "original"                       # "original" | "third-party"
symbol_ref    = "IEC 60617-S00669"               # REQUIRED on schematic SVGs: closest standard symbol
reviewed_by   = "caio"
reviewed_on   = "2026-11-23"
ml_restricted = false                            # true if any license term restricts ML/AI use

# REQUIRED iff source = "third-party" (FORBIDDEN or omitted when source = "original"):
# origin      = "<name and canonical URL/location of the source work>"
# license     = "<valid SPDX identifier>"
# attribution = "<required license/author notice>"
# modified    = true
```

- `dendri asset-check` MUST fail if any required provenance field is missing or if `source = "third-party"` lacks a matching `THIRD_PARTY_NOTICES` entry.
- **ML-Use Restriction Enforcement:** Any third-party license term restricting machine-learning use MUST be recorded (`ml_restricted = true` + license note), and restricted assets MUST be excluded from every training dataset, search index, or embedding corpus (including the Phase 9 AI copilot).

### 4.2 Mandatory SVG Normalization & Sanitization Pipeline
Every SVG in `assets/` — regardless of origin — MUST pass a committed normalization & sanitization tool/rule verified by `dendri asset-check`:
- **Strips editor cruft:** Removes Inkscape, Sodipodi, Adobe Illustrator, and Figma XML namespaces and non-semantic metadata attributes.
- **Rejects forbidden & unsafe constructs:**
  - `<script>`, event-handler attributes (`onload`, `onclick`, `on*`), `<foreignObject>`.
  - External references: `href`/`xlink:href` pointing to external URLs, `<image>` tags, `@import`, `<?xml-stylesheet ...?>`, or inline base64 raster blobs.
  - CSS custom properties (`var(--...)`) and SMIL animations (`<animate>`, `<set>`, `<animateTransform>`).
- **Enforces structural invariants:**
  - Explicit numeric `viewBox="0 0 W H"`.
  - `<g id="df-pins">` present with class-only pin markers (zero hardcoded pin colors).
  - Canonical XML attribute ordering and formatting so the file & render SHA-256 hash is 100% deterministic.
- **Frozen Review Policy:** Any asset import or normalization script MUST be committed to the repository; its output is reviewed once and frozen. Regeneration is always an explicit, human-reviewed change — NEVER an implicit build step.

### 4.3 Human Review Sign-Off (`review.toml`)
Every asset directory MUST include a strict `review.toml` (`#[serde(deny_unknown_fields)]`):
```toml
schema_version          = "2.0"
reviewer                = "caio"
reviewed_on             = "2026-11-23"
physical_render_sha256  = "<hex sha256 of canonical physical.svg + headless render>"
schematic_render_sha256 = "<hex sha256 of canonical schematic.svg + headless render>"
conformity_claimed      = false

[checklist]
grid_and_pins_verified  = true
symbology_style_checked = true
datasheet_params_cited  = true
provenance_verified     = true
```
Any edit to `profile.toml`, `physical.svg`, or `schematic.svg` that changes the canonical render hash automatically invalidates `review.toml` and fails `dendri asset-check` until a human reviews and updates the sign-off.
To streamline visual review workflows, the CLI provides an explicit human-invoked command **`dendri asset-approve <asset-dir> --reviewer <name>`** (never invoked implicitly by builds or CI) that validates the asset, computes its canonical sanitized SVG + headless `usvg`/`resvg` render SHA-256 digests, and atomically writes an updated `review.toml` with `conformity_claimed = false` by default.

---

## 5. Declarative Dynamic SVG Binding Contract (ADR-013)

Runtime string or regex mutation of SVG XML text is **permanently forbidden**.

Every dynamic visual element in an SVG has a stable `id="..."` on a `<g>`, `<path>`, or `<rect>` element parsed **once** at asset load time via the workspace's single `usvg` version into cached GPU textures / path geometry (`PreparedLayer`), driven purely by numeric `ComponentVisualState` values emitted from `dendri-circuit` / `dendri-physics`:

1. **`visibility`:** Show/hide an SVG layer based on boolean or enum state (e.g., `#contact-closed` vs `#contact-open`, `#fault-burn-overlay`).
2. **`paint`:** Override fill/stroke color and opacity from a normalized scalar `0.0..=1.0` or component parameter (e.g., `#led-die` + `#led-glow` driven by LED forward current $I_F$ and `params.color`).
3. **`translate`:** Linear 2D displacement `(dx, dy)` in SVG px proportional to a state scalar `0.0..=1.0` (e.g., `#piston-rod` stroke $x(t)$, `#actuator-cap` press displacement).
4. **`rotate`:** Rotation in degrees around explicit pivot `(cx, cy)` proportional to a state scalar (e.g., `#wiper-knob` `-135°..+135°`, gauge needle).
5. **`overlay_rect`:** Named `<rect id="overlay:screen">` bounds inside `<g id="df-overlays">` where the native canvas draws crisp live vector/bitmap content (16×2 LCD character grid, 128×64 OLED framebuffer, 7-segment display, live pressure numeric readout — eliminating `usvg` `<text>` font dependencies).

---

## 6. Strict TOML Profile Schemas (Normative Outlines)

### 6.1 Component Profile (`profile.toml`)
```toml
schema_version = "2.0"

[meta]
id        = "led"
name      = "Light Emitting Diode (5mm)"
category  = "indicator"
standards = ["IEC 60810", "IEC 60617"]

[provenance]
source        = "original"
symbol_ref    = "IEC 60617-S00669"
reviewed_by   = "caio"
reviewed_on   = "2026-11-23"
ml_restricted = false

[geometry]
grid_px          = 48
pin_px           = 8
physical_grid_w  = 2
physical_grid_h  = 3
schematic_grid_w = 2
schematic_grid_h = 2

[svg]
physical  = "physical.svg"
schematic = "schematic.svg"

[[ports]]
id     = "A"
label  = "Anode (+)"
domain = "electrical"
role   = "passive"          # maps 1:1 to SVG class df-pin-passive

[[ports]]
id     = "K"
label  = "Cathode (-)"
domain = "electrical"
role   = "passive"

[params.color]
type    = "enum"
options = ["red", "green", "blue", "yellow", "white", "amber"]
default = "red"

[params.vf_v]
type    = "real"
unit    = "V"
min     = 1.2
max     = 4.5
default = 2.0

[params.if_max_a]
type    = "real"
unit    = "A"
min     = 0.005
max     = 0.100
default = 0.030

[params.vr_max_v]
type    = "real"
unit    = "V"
default = 5.0

[physics]
model = "spice_led"         # primitive model OR composite subcircuit (§1.3)

# Optional for display components only (Phase 5, DEF-007):
# [display]
# charset = "hd44780_a00"   # enables ERR_STRING_CHARSET validation on strings routed to this display

[[svg.bindings]]
target_id        = "led-body"
kind             = "paint"
source_signal    = "luminosity"     # 0.0..=1.0 from spice_led forward current
color_from_param = "color"

[[svg.bindings]]
target_id     = "led-burned"
kind          = "visibility"
source_signal = "burned"
```

### 6.2 Board Profile (`profile.toml`)
Every board profile uses strict `#[serde(deny_unknown_fields)]` tables covering:
- **`[meta]` & `[provenance]`:** ID, family, standards, provenance (same rules as §4.1).
- **`[cpu]`:**
  - `int_bits: u8` (`16` on AVR / Arduino Uno R3; `32` on 32-bit cores) — governs C++ `int` → ASL type mapping.
  - `clock_hz: u64` (e.g., `16_000_000` on Uno R3) — drives exact 128-bit cycle-based virtual clock (`02` §2.1).
- **`[execution]` & `[execution.cost]`:**
  - `default_kind = "cooperative" | "plc_scan"`, `default_scan_period_us`, `indeterminate_reads_as = "previous" | "low" | "high"` (default `"previous"`).
  - `[execution.cost]`: cycle counts per statement class and per I/O operation (pin read/write, ADC read, arithmetic, branch, call).
- **`[electrical]`:** `logic_high_v`, `v_ih_v`, `v_il_v`, `pin_r_out_ohm`, `pin_i_max_a`, `v_abs_max_v`, `pullup_ohm`, `pulldown_ohm` (each citing datasheet `source_ref`).
- **`[[codecs]]`:** Per-board codec definitions (`id`, `kind = "binned" | "span" | "span_bipolar"`, `d: u64`, `n_max: u64`, `source_ref`), validated for `d <= 1_000_000_000` (`ERR_CODEC_RESOLUTION_EXCEEDS_SCALE`).
- **`[[channels]]`:** Per-channel analog & PWM descriptors:
  - `port_id`, `codec_id`, `range` (signal type `voltage | current`, low/high, `live_zero`, `hardware_normalizes_live_zero`),
  - Optional signed **band table** (`underrange_min`, `overrange_max`, `overflow_code`, `wire_break_code`, bipolar truncation mode, mandatory `source_ref` from official module manual — `DEF-002`; validated against `i32` overflow via `ERR_BAND_LIMIT_OVERFLOW`),
  - For PWM channels: verified **PWM transfer function** per timer mode (`DEF-001`, e.g., AVR Fast PWM `(OCR+1)/256` with `0`/`255` core special cases).
- **`[geometry]` & `[svg]`:** 48×48 px grid & `physical.svg` (+ optional `schematic.svg`).
- **`[[ports]]`:** Canonical `id` (may contain dots, e.g., `D13`, `A0`, `Ia.0`, `Qa.0`, `IW64`), `label`, `domain`, `role`, optional **`iec_address`** (e.g., `Ia.0 ⇄ "%IX0.0"`, `Qa.0 ⇄ "%QX0.0"`, `IW64 ⇄ "%IW64"`), and explicit capabilities (`digital_in`, `digital_out`, `pullup`, `pulldown`, `analog_in`, `analog_out`, `pwm`, `interrupt`, `uart_tx/rx`, `i2c_sda/scl`, `spi_*`, `ethernet`).

---

## 7. Automated Asset Validator Gate (`dendri asset-check`)

Executed in CI on every commit — **zero warnings allowed**:
1. Every directory under `assets/{boards,components,screens}/` deserializes cleanly with `#[serde(deny_unknown_fields)]`.
2. `meta.id` matches `^[a-z0-9]+(-[a-z0-9]+)*$`, matches the directory name exactly, and is globally unique across the catalog.
3. All referenced SVGs exist, pass the canonical sanitization pipeline (§4.2), and parse cleanly via the workspace's single `usvg` version.
4. Zero forbidden SVG constructs (`<script>`, `on*`, `<foreignObject>`, external `href`/`<image>`/`@import`, `<?xml-stylesheet?>`, CSS `var(`, SMIL `<animate`/`<set>`, inline base64 raster, unstripped editor namespaces).
5. `viewBox` width/height match `[geometry]` (`grid_w * 48`, `grid_h * 48`, tolerance `<= 1e-3`).
6. Every view has `<g id="df-pins">` with **exact 1:1 bijection** between `pin:<id>` elements and `[[ports]]` in `profile.toml` (and identical port sets between `physical.svg` and `schematic.svg`).
7. Every pin marker is `8×8 px`, snapped to grid pitch, non-overlapping (`dist >= 8 px`), free of hardcoded colors, and carries the exact `df-pin-*` CSS class matching `port.role`.
8. Every `[[svg.bindings]].target_id` exists in the declared SVG(s) (`<g>`/`<path>`/`<rect>`).
9. `[physics].model` (or composite sub-circuit) resolves to registered, analytically tested implementation(s) in `dendri-circuit` / `dendri-physics`.
10. Every standard string in `meta.standards` resolves against the **loaded & validated** `assets/standards.toml` registry.
11. `[provenance]` is complete in both `profile.toml` and SVG `<metadata>`; schematic SVGs declare `symbol_ref`; `source = "third-party"` entries declare `origin`/`license`/`attribution`/`modified` and exist in `THIRD_PARTY_NOTICES`; `ml_restricted = true` assets are excluded from all corpus manifests.
12. `review.toml` is present, all checklist items are `true`, and `*_render_sha256` matches the live canonical SVG/render hash.
13. Every board codec satisfies `D <= M` (`ERR_CODEC_RESOLUTION_EXCEEDS_SCALE`), every channel band limit fits signed `i32` `ANALOG` (`ERR_BAND_LIMIT_OVERFLOW`), and every `source_ref` on channel/band/electrical tables is non-empty.
