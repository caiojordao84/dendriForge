# 02 — Deterministic Co-Simulation Kernel, SPICE-Class Electrical Solver & Multi-Domain Physics

> **Normative Specification** | Baseline (Rev 2) — ADRs 009–012, 025, 029  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md), [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md)

---

## 1. Architectural Separation of Concerns

In v2, the simulation engine is strictly decomposed into **four isolated crates** orchestrated by a single deterministic kernel:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     dendri-sim-kernel (Orchestrator)                    │
│  Master deterministic clock (time_us: u64) · Event queue · Replay seed  │
│  Dedicated background simulation thread → lock-free Snapshot channel    │
└───────────┬───────────────────────┬───────────────────────┬─────────────┘
            │                       │                       │
┌───────────▼───────────┐ ┌─────────▼───────────┐ ┌─────────▼─────────────┐
│   dendri-vm (CPUs)    │ │  dendri-circuit     │ │   dendri-bus &        │
│ Per-board cooperative │ │  SPICE-class MNA    │ │   dendri-physics      │
│ ASL VM fibers + IEC   │ │  event + transient  │ │ UART/I2C/SPI/Modbus   │
│ 61131-3 FBs + OBs/ISR │ │  (R/C/L/AC/D/Q/M)   │ │ Pneumatics/Thermal/…  │
└───────────────────────┘ └─────────────────────┘ └───────────────────────┘
```

- **`dendri-vm` knows ZERO physics and ZERO board profiles.** It executes typed ASL v2 bytecode/IR (lowering structural `Network` and `Chart` POUs at load time — zero synthetic temporaries persisted in ASL) against an abstract `BoardIoInterface`.
- **`dendri-circuit` knows ZERO ASL.** It solves electrical netlists (nodal voltages and branch currents) with event-driven DC operating points plus transient ODE and nonlinear semiconductor models.
- **`dendri-bus` couples directly to physical canvas nets.** UART, I2C, SPI, and Ethernet/Modbus frames only travel between genuinely wired endpoints.
- **`dendri-physics` implements pluggable non-electrical domains** via a uniform `PhysicalComponent` trait.

---

## 2. Hybrid Discrete-Event + Cycle-Accurate Microsecond Kernel (ADR-010, ADR-029)

### 2.1 Master Clock, Cycle-Based Virtual Time & Determinism
- Global simulation time is a monotonically increasing integer **`time_us: u64`** (1 µs resolution; spans >584,000 years without overflow).
- **Cycle-Based Virtual Clock (per Cooperative Fiber):**
  - Each cooperative fiber tracks its **local time in CPU clock cycles (`fiber_cycles: u64`)**.
  - The board profile declares `[cpu] clock_hz` and `[execution.cost]` (cycle costs per statement class and per I/O operation).
  - The kernel derives `time_us = floor(fiber_cycles · 10⁶ / clock_hz)` in **exact 128-bit integer arithmetic** and **NEVER discards the fractional remainder**: sub-microsecond costs (e.g., 62.5 ns/cycle on a 16 MHz AVR) accumulate honestly and advance `time_us` the moment they cross a microsecond boundary.
  - `WAIT_TIME_US(d)` suspends the fiber until `cycles_now + ceil(d · clock_hz / 10⁶)` — it never wakes early.
  - Consequence: polling loops such as `while (!digitalRead(2));` advance virtual time honestly and observe circuit edges/transients instead of freezing the clock.
- **Determinism Rules:**
  - `REAL`/`LREAL` follow IEEE-754 `f32`/`f64` strictly; fused multiply-add (FMA) MUST NOT be introduced implicitly.
  - All transcendental math functions MUST use the pure-Rust **`libm`** crate (never host platform libm).
  - All determinism-critical state MUST use ordered maps (`BTreeMap`/`IndexMap`) — enforced at compile time via **`clippy::iter_over_hash_type`**.
  - **Determinism scope:** Digital and integer simulation traces are **bit-identical across operating systems** given `(Project, seed, InputTimeline)`. Analog waveform goldens compare within a declared numeric tolerance.
- **No wall-clock coupling inside the kernel:** Wall-clock pacing (`1×`, `0.5×`, `2×`, single-step, pause) lives strictly in the outer runner/UI driver.

### 2.2 Per-Board Execution Kinds (`ExecutionKind`)
`ExecutionKind` is a **non-exhaustive enum** (extensible to future task-based `asl-motion` robotics execution, `DEF-005`):
- **`ExecutionKind::Cooperative`** (MCUs):
  - Runs module initializers (topological order), then `OB_STARTUP` at `t = 0`, then loops `OB_MAIN`.
  - Advances `fiber_cycles` by `[execution.cost]`; `WAIT_TIME` / `WAIT_TIME_US` suspends **only this board's fiber** on the kernel event queue while its outputs stay driven, hardware timers (`millis`/`micros`/PWM/IEC FBs) continue advancing, and `OB_HW_INT` / `OB_CYCLIC_INT` / `OB_COMM_EVENT` remain armed.
- **`ExecutionKind::PlcScan`** (PLCs, configurable `scan_period_us`, default `10_000` µs):
  1. Sample Process Image Inputs (PII) from the electrical boundary (`Ia.0`, `IW64`, etc.).
  2. Advance IEC 61131-3 timers (`TON`/`TOF`/`TP`) using exact elapsed `dt_us`.
  3. Execute `OB_MAIN` (and any invoked secondary/tertiary POUs) to completion (with per-scan statement budget reset on EVERY scan).
  4. Commit Process Image Outputs (PIQ) to the electrical boundary (`Qa.0`, `QW64`, etc.).

### 2.3 Hard Safety & Resource Guardrails
- **Per-scan statement budget:** Reset at the start of **every** scan/resume quantum (never cumulative across session lifetime). Empty `For` and `While` loops decrement budget AND accumulate cycle cost on **every iteration**.
- **Explicit call-stack depth limit (`max_call_depth`, default `64`):** Infinite recursion returns `RuntimeError::StackOverflow` — never a Rust thread stack overflow.
- **Bounded ring-buffer trace (`max_trace_events`, default `100_000` in GUI, streamable in CLI):** Zero unbounded memory growth.
- **Strict `ControlFlow` propagation:** `Result<ControlFlow, RuntimeError>` where `ControlFlow ∈ { Next, Break, Continue, Return(Option<Value>), YieldWait(u64) }`.
- **Lexical function frames:** Dedicated `LocalFrame` stack per call; parameters and locals NEVER leak into shadowed globals or caller locals.
- **`INDETERMINATE` Digital Input Policy:** ASL `BOOL` has no third state. Each board profile declares `indeterminate_reads_as` (`previous | low | high`; default `previous`); reading an electrically indeterminate input applies that rule and emits `WARN_INDETERMINATE_LEVEL`.
- **Exact IEC 61131-3 FB Compliance:** Separate typed storage per FB instance; exact IEC edge-case compliance for `TON`, `TOF` (including `PT = 0`), `TP` (`ET` resets to `0` when `IN = FALSE` post-pulse), `CTU`, `CTD` (`CV = 0, Q = TRUE` until `LD`), `CTUD`, `R_TRIG`, `F_TRIG`, `SR`, `RS`, and unsigned wrapping `TIME_DIFF`.
- **Load-Time Structural Lowering:** `dendri-vm` lowers `PouBody::Network` (materialized power rails + `EN`/`ENO`/`Q`) and `PouBody::Chart` (IEC 60848 Rule 5 two-phase + OR priority + `N/S/R/L/D/P` qualifiers + `.X`/`.T`) into executable VM bytecode/CFG on load — zero synthetic temporaries ever leak back into ASL.

### 2.4 Dedicated Background Simulation Thread
In `dendri-editor`, `dendri-sim-kernel` runs on a dedicated worker thread, publishing immutable `Arc<SimSnapshot>` frames over a bounded latest-value channel and waking the UI only when visual state changes. Heavy SPICE transients throttle virtual-time speed gracefully (reporting real-time ratio) without ever freezing UI responsiveness.

---

## 3. SPICE-Class Transient Analog & Mixed-Signal Solver (`dendri-circuit`, ADR-009, ADR-029)

`dendri-circuit` is a pure-Rust **Modified Nodal Analysis (MNA)** mixed-signal circuit simulator supporting linear, reactive, AC, and nonlinear semiconductor devices.

### 3.1 Netlist Extraction, Island Partitioning & Event-Driven Solving
1. **Union-Find Net Builder:** Merges all wires, breadboard internal strips/rails, closed zero-ohm jumpers/terminal blocks, and ground references into canonical electrical **Nets** (`NetId`).
2. **Ground Reference Enforcement:** Each isolated island requires a reference ground (`0 V`); floating islands report `Warning::FloatingSubcircuit` and add a high-impedance `Gmin = 1e-12 S` bleed to guarantee non-singularity.
3. **Connected-Component Partitioning:** Electrically isolated sub-circuits (e.g., an optocoupler-isolated 24 V PLC loop and a 5 V Arduino circuit) solve as independent smaller MNA matrices.
4. **Event-Driven + Time-Varying Transient Stepping:**
   - `dendri-circuit` solves a **DC operating point ONLY when topology or a discrete state changes** (switch click, GPIO edge, relay contact toggle, PWM edge).
   - It runs **transient time steps ONLY while the electrical island contains reactive elements (`C`, `L`) OR continuous time-varying sources** (AC 1φ/3φ sources, signal generators, continuously time-dependent physical couplings — ensuring purely resistive AC circuits step accurately!).
   - Transient step size is bounded by the highest source frequency in the island: `dt ≤ T_min / steps_per_period` (`steps_per_period` default `200`, configurable), refining down to `1 µs` on fast switching edges / LTE error control with a hard per-tick iteration budget.
   - PWM pin edges are discrete events (not continuous sources); an RC low-pass filter on a PWM pin is covered automatically by the reactive-element rule.

### 3.2 Mathematical Core (MNA + Companion Models + Newton-Raphson)
- **Linear System:** Stamps conductance matrix $\mathbf{G}$ and branch equations $\mathbf{B}, \mathbf{C}, \mathbf{D}$ into $\mathbf{A}\mathbf{x} = \mathbf{z}$ and solves via LU decomposition with partial pivoting (dense fast-path for small islands $\le 32$ unknowns, sparse CSR/LU for larger nets; zero unchecked indexing panics).
- **Reactive Companion Models:** Backward Euler / Trapezoidal companion models for capacitors ($i = C \frac{dv}{dt}$), inductors ($v = L \frac{di}{dt}$), and mutually coupled inductors/transformers.
- **Nonlinear Semiconductor Devices (Newton-Raphson):** Linearized tangent companion models with `pnjlim` junction damping, `Gmin` stepping, and source stepping fallback:
  - Diodes / LEDs / Schottky / Zener / TVS: Shockley exponential ($I_S, N, R_S$) + reverse breakdown ($V_{BR}$), optical luminosity proportional to $I_F$, and overcurrent/overpower/reverse-breakdown burn detection.
  - BJT (NPN/PNP): Ebers-Moll transport model ($\beta_F, \beta_R, I_S, V_{AF}$).
  - MOSFET (N/P): Shichman-Hodges Level-1 (`V_TH`, `K_P`, `R_DS(on)`, body diode).
  - Power & Isolation: IGBT, SCR/Thyristor, TRIAC, Optocoupler, SSR, H-Bridge macromodels.
- **AC & Three-Phase Sources:** DC, sinusoidal 1φ AC, 3φ AC ($0^\circ, -120^\circ, +120^\circ$ L1/L2/L3/N/PE), PWM, and signal generators.
- **Electromechanical Switches & Relays:** Finite conductances ($R_{on} \approx 10\text{ m}\Omega$, $R_{off} \approx 100\text{ M}\Omega$) naturally supporting NO/NC pushbuttons, changeover switches, series/parallel networks, breadboards, and inductive relay coils with real flyback spikes when flyback diodes are omitted.

### 3.3 MCU / PLC Pin Boundary Coupling & Solver → ASL Numeric Boundary
Each physical board pin couples to the MNA matrix via realistic parameters from the board's TOML profile (never hardcoded `"board"` or Uno defaults):
- **Digital Output:** Thevenin source ($V_{OH}$/$V_{OL}$ from `logic_high_v`, series $R_{out}$, max sink/source current monitor).
- **Digital Input:** High input impedance ($R_{in} \approx 10\text{ M}\Omega$) in parallel with optional internal pull-up ($R_{pu}$) or pull-down ($R_{pd}$), read via Schmitt thresholds ($V_{IH}, V_{IL}$) producing `HIGH | LOW | INDETERMINATE` (§2.3) and firing hardware ISRs on exact electrical edges.
- **Analog Input (ADC) — Solver → ASL Boundary (ADR-025 / ADR-029):**
  - High-impedance sense node reads the solved node voltage $v$ and converts it to an ASL `ANALOG` fixed-point quantity (`M = 10⁹`) via **round-half-even** (unbiased IEEE-754 banker's rounding):
    $$a = \operatorname{clamp}_{i32}\!\left(\operatorname{round\_half\_even}\!\left(\frac{v}{v_{\text{full\_scale}}} \cdot M\right)\right)$$
    Non-finite $v$ yields status `Fault`.
  - The channel's codec ([`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md#32-normative-formulas-exact-integer-arithmetic)) or band layer (§3.4 of doc 01) converts $a$ to the channel's native `CODE` (and updates `Ok | Underrange | Overrange | Fault` status).
  - Overvoltage beyond the pin's `v_abs_max_v` (e.g., wiring 24 V into a 5 V / 3.3 V MCU pin) raises an explicit electrical fault!
- **PWM Output:** Hardware timer drives true square-wave voltage pulses into the MNA circuit using the board profile's verified PWM transfer function (`DEF-001`).
- **Fault Detection:** Short circuits ($I > I_{short}$), reverse LED breakdown, over-power ($I^2 R > P_{rated}$), and pin overvoltage/overcurrent surface as live diagnostics and canvas fault indicators.

### 3.4 Headless Solver Verification Tiers & Tracer Bullet
All Phase 3 analytical benchmarks run **100% headless in `dendri-circuit`** (zero dependency on Phase 4 SVGs/assets), validated against closed-form analytical equations (`error < 0.1%`):
- **Tier 1:** Netlist builder + island partitioning + MNA Linear DC (`R`, `V`, `I`, potentiometers, NO/NC switches, relays, breadboards, pull-ups/downs, MCU/PLC pin boundary, short/overvoltage faults) + Newton-Raphson Diode/LED/Zener.
- **Tier 2:** Transient `C`/`L`/transformers, 1φ & 3φ AC (including purely resistive AC!), PWM driving, virtual oscilloscope/multimeter probes.
- **Tier 3:** BJT, MOSFET, Optocoupler, IGBT, SCR, TRIAC, SSR, H-Bridge, motor back-EMF models.
- **Phase 3 Exit Tracer Bullet:** Immediately upon Phase 3 completion, an end-to-end headless CLI scenario (`Uno + button + LED`) executes across `dendri-vm` + `dendri-circuit` + `dendri-sim-kernel` and is verified against its independent hand-calculated oracle before Phase 4 begins.

---

## 4. Physical-Topology Virtual Bus Fabric (`dendri-bus`, ADR-011)

Communication buses are **strictly coupled to real canvas wiring**:
1. **UART (`UartBus`):** Point-to-point/multi-drop byte streams across wired `TX → RX` nets checking baud rate, framing, FIFO depths, and non-blocking `available`/`read`/`write`/`flush`.
2. **I2C (`I2cBus`):** Shared `SDA`/`SCL` net + pull-up check; devices implement `trait I2cDevice` supporting multi-byte register transactions, bus scans, and multi-device coexistence.
3. **SPI (`SpiBus`):** Shared `MOSI`/`MISO`/`SCK` nets with per-device active-low `CS` net voltage verification so SD/TFT/RFID coexist honestly on one bus.
4. **Virtual Ethernet / Modbus TCP (`ModbusNetwork`):** Multi-node virtual LAN connecting PLC/MCU Ethernet/Wi-Fi ports by IP & Unit ID; supports FC1–FC6, FC15, FC16, 16-bit addressing, standard Modbus exception codes (`01`–`04`), and optional real OS TCP socket bridge later.

---

## 5. Decoupled Multi-Domain Physical Component Models (`dendri-physics`, ADR-012)

Physical plants are **never** hardcoded inside `dendri-vm`. Dynamic external components implement:

```rust
pub trait PhysicalComponent: Send {
    /// Stamp electrical ports into the circuit MNA matrix (coils, sensors, contacts).
    fn stamp_electrical(&self, ctx: &mut CircuitStampCtx);
    /// Advance non-electrical domain state over dt_us (pneumatics, motion, thermal, fluid).
    fn step_physics(&mut self, dt_us: u64, inputs: &DomainInputs) -> DomainOutputs;
    /// Produce lightweight visual state struct for SVG layer rendering.
    fn visual_state(&self) -> ComponentVisualState;
}
```

- Example closed industrial loop (zero CPU VM hardcoding): PLC `Qa.0` drives `24 V` → MNA solves solenoid coil current on `pneu-valve-5-2` → pneumatic domain routes `6 bar` air to `pneu-cylinder-da` → piston stroke $x(t)$ integrates forward → at $x = L$, mechanical coupling closes limit switch `A1` → MNA pulls PLC `Ia.1` to `24 V` → PLC Ladder/GRAFCET transitions to `B+`.
- Rollout order: Electrical/Electromechanical (Phase 3–4), followed by pluggable Pneumatic/Hydraulic, Thermal/Tank, and Display Framebuffers (LCD char-grid with ROM `charset` `DEF-007`, OLED/TFT pixel buffers) in Phase 5.
