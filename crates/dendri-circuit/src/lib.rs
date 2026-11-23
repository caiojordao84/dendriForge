//! Pure-Rust SPICE-class Modified Nodal Analysis (MNA) mixed-signal electrical solver
//! (`02-simulation-and-electrical-engine.md` §3).
//!
//! - Knows **ZERO ASL** and has **ZERO dependency** on Phase 4 SVGs/assets.
//! - Deterministic across platforms (`BTreeMap` ordered nets/islands, pure-Rust `libm`, zero FMA).
//! - Supports:
//!   - **Tier 1:** Union-Find netlist extraction, connected-component island partitioning,
//!     LU MNA Linear DC (`R`, `V`, `I`, potentiometers, NO/NC switches, inductive relays,
//!     breadboard strips, MCU/PLC Thevenin & Schmitt pin boundaries, Solver→ASL round-half-even
//!     at `M = 10⁹`, electrical faults) + Newton-Raphson Diode / LED / Zener with `pnjlim`,
//!     `Gmin` stepping, and source stepping fallback.
//!   - **Tier 2:** Event-driven DC + transient ODE stepping for reactive islands OR continuous
//!     time-varying sources (`steps_per_period = 200` default), `C`, `L`, mutually coupled
//!     transformers, 1φ & 3φ AC (including purely resistive AC), verified PWM transfer functions
//!     (closing `DEF-001`), and oscilloscope/multimeter probes.
//!   - **Tier 3:** Ebers-Moll BJT (NPN/PNP), Shichman-Hodges MOSFET (NMOS/PMOS + body diode),
//!     galvanically isolated Optocoupler (cross-island CTR), IGBT, SCR/Thyristor, TRIAC, SSR,
//!     H-Bridge, and back-EMF DC motor.

pub mod backend;
pub mod boundary;
pub mod devices;
pub mod linalg;
pub mod pwm;
pub mod solver;

pub use backend::{BoardCircuitSpec, SpiceCircuitBackend};
pub use boundary::{
    BoardElectricalProfile, BoardPinSpec, arduino_uno_r3_electrical, round_half_even_f64,
    siemens_s7_1200_electrical, voltage_to_analog_round_half_even,
};
pub use devices::{
    BjtParams, BjtPolarity, CapacitorParams, ComponentSpec, CurrentSourceDcParams, DcMotorParams,
    DiodeParams, HBridgeParams, IgbtParams, InductorParams, MosfetParams, MosfetPolarity,
    OptocouplerParams, PotentiometerParams, RelayParams, ResistorParams, ScrParams, SsrParams,
    SwitchParams, TransformerParams, TriacParams, VoltageSourceAcParams, VoltageSourceDcParams,
};
pub use pwm::{
    PwmTransferFunction, PwmTransferMode, PwmWaveform, linear_pwm, uno_timer0_fast_pwm,
    uno_timer1_2_phase_correct_pwm,
};
pub use solver::{
    Circuit, CircuitConfig, CircuitError, ComponentVisualState, ElectricalFault, FaultKind,
    IslandSolveStats, MultimeterReading, ProbeSample, SolveReport, WarningKind,
};
