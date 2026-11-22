# 02 — Deterministic Co-Simulation Kernel, SPICE-Class Electrical Solver & Multi-Domain Physics

> **Normative Specification** | Decisions locked in Round 3 (ADRs 009–012)  
> **Depends on:** [`00-overview-and-adrs.md`](./00-overview-and-adrs.md), [`01-asl-ir-and-project-schema.md`](./01-asl-ir-and-project-schema.md)

---

## 1. Architectural Separation of Concerns

In v1, `dendri-sim` mixed CPU instruction execution, hardcoded tank/thermal plants, display text logs, and ad-hoc 1-hop canvas shortcuts. In v2, the simulation engine is strictly decomposed into **four isolated crates** orchestrated by a single deterministic kernel:

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
│ ASL VM fibers + IEC   │ │  transient solver   │ │ UART/I2C/SPI/Modbus   │
│ 61131-3 FBs + ISRs    │ │  (R/C/L/AC/D/Q/M)   │ │ Pneumatics/Thermal/…  │
└───────────────────────┘ └─────────────────────┘ └───────────────────────┘
```

- **`dendri-vm` knows ZERO physics and ZERO board profiles.** It executes typed ASL v2 bytecode/IR against an abstract `BoardIoInterface` (digital pins, analog channels, PWM timers, UART/I2C/SPI/Ethernet mailboxes, NV retain store).
- **`dendri-circuit` knows ZERO ASL.** It solves electrical netlists (nodal voltages and branch currents) with full transient ODE and nonlinear semiconductor models.
- **`dendri-bus` couples directly to physical canvas nets.** UART, I2C, SPI, and Ethernet/Modbus frames only travel between endpoints that are genuinely wired together (or connected via the virtual network switch).
- **`dendri-physics` implements pluggable non-electrical domains** (pneumatic, hydraulic, mechanical, thermal) via a uniform `PhysicalComponent` trait.

---

## 2. Hybrid Discrete-Event + Fixed-Tick Microsecond Kernel (ADR-010)

### 2.1 Master Clock & Determinism
- Global simulation time is a monotonically increasing integer **`time_us: u64`** (1 µs resolution; spans >584,000 years without overflow).
- **100% deterministic replay:** Given `(Project, seed: u64, InputTimeline)`, every pin edge, analog sample, bus frame, and trace record is bit-identical across runs and operating systems.
- **No wall-clock coupling inside the kernel:** Wall-clock pacing (`1×`, `0.5×`, `2×`, single-step, pause) lives strictly in the outer runner/UI driver, never inside the simulation math.

### 2.2 Per-Board Execution Modes
Every `[[boards]]` instance in the project runs its own isolated VM state:
- **Cooperative MCU Fiber (`execution = "cooperative"`):**
  - Executes `init` once at `t = 0`, then loops `scan`.
  - `WAIT_TIME` / `WAIT_TIME_US` suspends **only this board's VM fiber** (`wake_at_us = now_us + duration_us`) by pushing an event onto the kernel's min-heap event queue.
  - While suspended, outputs remain driven onto the circuit, hardware timers (`millis()`/`micros()`/PWM/IEC FBs) continue advancing, and hardware ISRs remain armed.
- **Cyclic PLC Scan (`execution = "plc_scan"`, configurable `scan_period_us`, e.g. `10_000` µs):**
  - At each periodic scan trigger:
    1. **Sample Process Image Inputs (PII)** from the electrical boundary (`I0.0`, `IW64`, etc.).
    2. **Advance IEC 61131-3 timers** (`TON`/`TOF`/`TP`) using exact elapsed `dt_us`.
    3. **Execute `scan` to completion** (with per-scan statement budget reset on EVERY scan!).
    4. **Commit Process Image Outputs (PIQ)** to the electrical boundary (`Q0.0`, `QW64`, etc.).

### 2.3 Hard Safety & Resource Guardrails (Fixing Every v1 Crash/Leak)
- **Per-scan statement budget:** Reset at the start of **every** scan/resume quantum (never cumulative across the lifetime of the session). Empty `For` and `While` loops decrement budget on **every iteration**.
- **Explicit call-stack depth limit (`max_call_depth`, default `64`):** Infinite recursion returns a clean `RuntimeError::StackOverflow` diagnostic — never a Rust thread stack overflow.
- **Bounded ring-buffer trace (`max_trace_events`, configurable; default `100_000` in GUI, streamable in headless CLI):** Zero unbounded memory growth in long-running IDE sessions.
- **Strict `ControlFlow` propagation:** `exec_stmt` returns `Result<ControlFlow, RuntimeError>` where `ControlFlow ∈ { Next, Break, Continue, Return(Option<Value>), YieldWait(u64) }`. `Break` and `Continue` work cleanly inside arbitrarily nested `If`/`While`/`For`/`Switch`.
- **Lexical function frames:** Each function call pushes a dedicated `LocalFrame` stack entry. Lookups resolve `LocalFrame -> Globals`. Writes to parameters/locals NEVER leak into or overwrite shadowed globals or caller locals.
- **Exact IEC 61131-3 FB Compliance:**
  - Separate typed storage per FB instance (eliminating cross-type map collisions and unnamed singleton overwrites).
  - Exact IEC edge-case compliance verified by dedicated unit tests: `TOF` with `PT = 0` drops `Q` immediately; `TP` resets `ET = 0` when `IN = FALSE` after pulse end; `CTD` initializes `CV = 0, Q = TRUE` until `LD = TRUE`; `TON` clears `Q` if `PT` increases above `ET` dynamically; `R_TRIG`/`F_TRIG` expose `.Q` cleanly without double-consuming edges; `SR`/`RS` implemented as first-class FBs; `TIME_DIFF` uses exact 32-bit/64-bit unsigned wrapping subtraction.

### 2.4 Dedicated Background Simulation Thread
- In `dendri-editor`, the simulation kernel executes on a dedicated worker thread, publishing immutable `SimSnapshot` frames over a bounded latest-value channel to the UI thread.
- Heavy SPICE microsecond transients can throttle virtual-time speed gracefully (reporting real-time ratio e.g. `0.85×`) **without ever freezing UI responsiveness or input events**.

---

## 3. SPICE-Class Transient Analog & Mixed-Signal Solver (`dendri-circuit`, ADR-009)

`dendri-circuit` is a pure-Rust **Modified Nodal Analysis (MNA)** transient circuit simulator supporting linear, reactive, AC, and nonlinear semiconductor devices.

### 3.1 Netlist Extraction & Sub-Circuit Partitioning
1. **Union-Find Net Builder:** Connects all wires, breadboard internal strips/rails, closed zero-ohm jumpers/terminal blocks, and ground symbols into canonical electrical **Nets** (`NetId`).
2. **Ground Reference Enforcement:** Each electrically isolated island requires a reference ground node (`0 V`); floating islands are detected cleanly (reporting `Warning::FloatingSubcircuit` or adding a high-impedance `Gmin = 1e-12 S` bleed to prevent singular matrices).
3. **Connected-Component Partitioning:** Electrically independent sub-circuits (e.g. an optocoupler-isolated 24 V PLC loop and a 5 V Arduino circuit) are solved as **separate smaller MNA matrices** — drastically improving performance and numerical conditioning!

### 3.2 Mathematical Core (MNA + Companion Models + Newton-Raphson)
- **Linear System:** At each transient step `dt`, stamp conductance matrix $\mathbf{G}$ and branch equations $\mathbf{B}, \mathbf{C}, \mathbf{D}$ into the MNA system $\mathbf{A}\mathbf{x} = \mathbf{z}$, solving for node voltages $v_k$ and voltage-source/inductor branch currents $i_m$ via LU decomposition with partial pivoting (sparse CSR/LU for larger nets, dense fast-path for small sub-circuits $\le 32$ unknowns).
- **Reactive Companion Models (Transient ODE Integration):**
  - **Capacitor ($i = C \frac{dv}{dt}$):** Trapezoidal / Backward Euler companion conductance $G_{eq}$ in parallel with history current source $I_{eq}$.
  - **Inductor ($v = L \frac{di}{dt}$):** MNA branch current formulation with companion resistance $R_{eq}$ and history voltage source $V_{eq}$ (and mutual inductance $M$ for transformers/current transformers).
  - **Adaptive / Bounded Timestep:** Default macro-step coupled to circuit time constants (e.g., `10 µs`–`100 µs` default, refining down to `1 µs` on fast switching edges / LTE error control, with hard per-tick iteration budget).
- **Nonlinear Semiconductor Devices (Newton-Raphson Iteration):**
  - Linearized tangent companion models stamped per NR iteration with `pnjlim` junction voltage damping, `Gmin` stepping, and source stepping fallback for robust convergence:
    - **Diodes (Standard, LED, Schottky, Zener, TVS):** Shockley exponential model ($I_S, N, R_S$) + reverse breakdown ($V_{BR}$), plus LED optical luminosity proportional to $I_F$ and thermal/overcurrent burn tracking ($I > I_{max}$ or $P > P_{max}$ or $V_R > V_{R,max}$).
    - **BJT (NPN / PNP):** Ebers-Moll transport model ($\beta_F, \beta_R, I_S, V_{AF}$).
    - **MOSFET (N-Ch / P-Ch):** Shichman-Hodges Level-1 (`V_TH`, `K_P`, `R_DS(on)`, body diode).
    - **Power Electronics & Isolation:** IGBT, SCR/Thyristor, TRIAC, Optocoupler, SSR, H-Bridge macromodels built on top of the exact same nonlinear stamping trait.
- **AC & Three-Phase Sources:** Ideal and internal-impedance DC, sinusoidal 1-phase AC ($V_{peak}\sin(2\pi f t + \phi)$), and 3-phase balanced/unbalanced AC ($0^\circ, -120^\circ, +120^\circ$ with L1/L2/L3/N/PE) + PWM and arbitrary signal generators.
- **Electromechanical Switches & Relays:** Modeled as finite low/high conductances ($R_{on} \approx 10\text{ m}\Omega$, $R_{off} \approx 100\text{ M}\Omega$) — naturally handling NO/NC pushbuttons, changeover contacts, series/parallel switch networks, breadboards, and relay coils (inductive coil energizing magnetically coupled NO/NC contacts with flyback voltage spikes when diode is omitted!).

### 3.3 MCU / PLC Pin Boundary Coupling (Mixed-Signal Bridge)
Each physical board pin connects to the MNA matrix via a **realistic pin electrical interface** derived from the board's TOML profile (never hardcoded `"board"` or Uno defaults!):
- **Digital Output:** Thevenin source ($V_{OH}$ / $V_{OL}$ from board's `logic_high_v`, series output impedance $R_{out}$, max sink/source current limit monitoring).
- **Digital Input:** High input impedance ($R_{in} \approx 10\text{ M}\Omega$) in parallel with optional internal pull-up ($R_{pu}$ to $V_{DD}$) or pull-down ($R_{pd}$ to GND), read through Schmitt-trigger thresholds ($V_{IH}, V_{IL}$) to produce clean `HIGH | LOW | INDETERMINATE` states — AND firing hardware ISRs on exact electrical transitions!
- **Analog Input (ADC):** High-impedance sense node reading actual solved net voltage $V_{node}$ and quantizing to the board's real native ADC scale (`0..1023` 10-bit on Uno, `0..4095` 12-bit on ESP32, `0..27648` on S7-1200) **and** normalized ASL `0..65535` transparently, with explicit overvoltage clamp/fault detection when $V_{node} > V_{abs\_max}$ (e.g., wiring 24 V directly to a 3.3 V / 5 V pin raises an explicit overvoltage fault!).
- **PWM Output:** Hardware timer driving true square-wave voltage pulses at configured frequency and duty cycle into the MNA circuit (so an RC low-pass filter, servo, motor, or LED brightness works physically!).
- **Fault Detection:** Automatically detects short circuits ($I > I_{short}$), reverse-biased LED breakdown, resistor/component over-power ($I^2 R > P_{rated}$), and MCU pin overvoltage/overcurrent — surfaced as live diagnostics and visual fault indicators on the canvas.

### 3.4 Phased Solver Verification Tiers (All Under One Unified MNA Architecture)
To guarantee mathematical rock-solidity via TDD (validated against analytical textbook solutions and `ngspice` reference benchmarks):
- **Tier 1 (Foundation):** Netlist builder + MNA Linear DC (`R`, `V`, `I`, potentiometers, switches NO/NC, relays, breadboards, pull-up/down, MCU/PLC pin boundary, short/overvoltage faults) + Newton-Raphson Diode/LED/Zener.
- **Tier 2 (Transient & AC):** Companion-model `C`, `L`, transformers, 1-phase & 3-phase AC sources, PWM waveform driving, virtual oscilloscope/multimeter probe sampling.
- **Tier 3 (Semiconductors & Power):** BJT (NPN/PNP), MOSFET (N/P), Optocoupler, IGBT, SCR, TRIAC, SSR, H-Bridge, AC/DC motor electrical back-EMF models.

---

## 4. Physical-Topology Virtual Bus Fabric (`dendri-bus`, ADR-011)

Communication buses are **strictly coupled to real canvas wiring** — if `TX` isn't wired to `RX` (or `SDA`/`SCL` lack pull-ups or aren't on the same net), communication fails realistically with clear diagnostics:

1. **UART (`UartBus`):** Point-to-point/multi-drop byte streams routed across wired `TX → RX` nets, checking baud rate compatibility, framing, RX/TX FIFO depths, and non-blocking `available()`/`read()`/`write()`/`flush()` (fixing v1's inverted `flush()`).
2. **I2C (`I2cBus`):** Shared `SDA`/`SCL` net verification + pull-up check; devices implement `trait I2cDevice { fn address(&self) -> u8; fn write(&mut self, data: &[u8]) -> Ack; fn read(&mut self, buf: &mut [u8]) -> Ack; }` supporting multi-byte register transactions, bus scans, and multi-device coexistence (e.g., OLED + DHT/BME + RTC on the same wires).
3. **SPI (`SpiBus`):** Shared `MOSI`/`MISO`/`SCK` nets with per-device chip-select (`CS` net voltage low-active check!) so multiple SPI peripherals (SD card + TFT + RFID) coexist honestly on one bus.
4. **Virtual Ethernet / Modbus TCP (`ModbusNetwork`):** Multi-node virtual LAN connecting PLC/MCU Ethernet/Wi-Fi ports by IP & Unit ID; supports FC1–FC6, FC15 (Write Multiple Coils), FC16 (Write Multiple Registers), proper 16-bit addressing, standard Modbus exception codes (`01`–`04`), and optional real OS TCP socket bridge in a later phase for external SCADA testing.

---

## 5. Decoupled Multi-Domain Physical Component Models (`dendri-physics`, ADR-012)

Physical plants are **never** hardcoded inside `dendri-vm`. Instead, every dynamic component implements a decoupled trait:

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

### Concrete Industrial Loop Example (Zero Hardcoding in CPU VM)
- PLC `Qa.0` energizes `24 V` → MNA solves solenoid coil current on `pneu-valve-5-2` → valve spool shifts → pneumatic domain routes `6 bar` air supply to `pneu-cylinder-da` port A → piston position $x(t) \in [0, L]$ integrates forward → at $x = L$, mechanical sensor coupling closes limit switch `A1` → MNA pulls PLC input `Ia.1` to `24 V` → PLC Ladder/GRAFCET detects `A1 = TRUE` and transitions to `B+`!
- Rollout order: **Electrical/Electromechanical first**, followed by pluggable **Pneumatic/Hydraulic**, **Thermal/Tank**, and **Display Framebuffers (LCD char-grid, OLED/TFT pixel buffers)** without ever modifying a single line of `dendri-vm`.
