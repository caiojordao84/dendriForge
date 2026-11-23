//! Decoupled multi-domain physical component models and display framebuffers
//! (`02-simulation-and-electrical-engine.md` §5, ADR-012, closes `DEF-007`).
//!
//! Physical plants are NEVER hardcoded inside `dendri-vm`. External domain components implement
//! [`PhysicalComponent`] and couple strictly to `dendri-circuit::Circuit` through electrical
//! terminals (`CircuitStampCtx`) and to each other through explicit [`DomainLink`]s in
//! [`PhysicsWorld`].

pub mod display;
pub mod pneumatics;
pub mod tank;
pub mod thermal;

pub use display::{
    CharsetViolation, DisplayCharsetSpec, LcdCharFramebuffer, OledPixelFramebuffer,
    encode_hd44780_i2c_byte,
};
pub use pneumatics::{
    DoubleActingCylinder, DoubleActingCylinderParams, LimitSwitch, LimitSwitchParams,
    PneumaticValve52, PneumaticValve52Params,
};
pub use tank::{FluidTank, FluidTankParams};
pub use thermal::{ThermalChamber, ThermalChamberParams, pt100_resistance_ohm};

use dendri_circuit::{Circuit, CircuitError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Borrowed electrical circuit context passed into [`PhysicalComponent::stamp_electrical`].
pub struct CircuitStampCtx<'a> {
    circuit: &'a mut Circuit,
}

impl<'a> CircuitStampCtx<'a> {
    /// Creates a new stamping context wrapping `circuit`.
    pub fn new(circuit: &'a mut Circuit) -> Self {
        Self { circuit }
    }

    /// Reads the solved node voltage at `(inst, port)`.
    #[must_use]
    pub fn terminal_voltage(&self, inst: &str, port: &str) -> f64 {
        self.circuit.terminal_voltage(inst, port)
    }

    /// Updates a SPICE electromechanical switch closure state inside `circuit`.
    pub fn set_switch(&mut self, component_id: &str, closed: bool) -> Result<(), CircuitError> {
        self.circuit.set_switch_closed(component_id, closed)
    }

    /// Updates a SPICE DC current source (e.g., 4–20 mA sensor transmitter) inside `circuit`.
    pub fn set_current_source(
        &mut self,
        component_id: &str,
        current_a: f64,
    ) -> Result<(), CircuitError> {
        self.circuit.set_dc_current(component_id, current_a)
    }

    /// Updates a SPICE variable resistance (e.g., PT100 RTD) inside `circuit`.
    pub fn set_resistance(&mut self, component_id: &str, r_ohm: f64) -> Result<(), CircuitError> {
        self.circuit.set_resistance(component_id, r_ohm)
    }

    /// Borrows the underlying [`Circuit`] immutably.
    #[must_use]
    pub const fn circuit(&self) -> &Circuit {
        self.circuit
    }
}

/// Non-electrical physical inputs delivered to [`PhysicalComponent::step_physics`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DomainInputs {
    pub scalars: BTreeMap<String, f64>,
    pub flags: BTreeMap<String, bool>,
}

impl DomainInputs {
    /// Reads a named scalar input signal or returns `default`.
    #[must_use]
    pub fn scalar_or(&self, key: &str, default: f64) -> f64 {
        self.scalars.get(key).copied().unwrap_or(default)
    }

    /// Reads a named boolean flag input or returns `default`.
    #[must_use]
    pub fn flag_or(&self, key: &str, default: bool) -> bool {
        self.flags.get(key).copied().unwrap_or(default)
    }
}

/// Non-electrical physical outputs produced by [`PhysicalComponent::step_physics`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DomainOutputs {
    pub scalars: BTreeMap<String, f64>,
    pub flags: BTreeMap<String, bool>,
}

/// Lightweight deterministic visual state emitted for SVG layer bindings (`03` §5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ComponentVisualState {
    pub scalars: BTreeMap<String, f64>,
    pub flags: BTreeMap<String, bool>,
}

/// Normative pluggable physical component trait (`02-simulation-and-electrical-engine.md` §5).
pub trait PhysicalComponent: Send {
    /// Stamp or synchronize electrical ports into the circuit MNA matrix (coils, sensors, contacts).
    fn stamp_electrical(&self, ctx: &mut CircuitStampCtx<'_>);

    /// Advance non-electrical domain state over `dt_us` (pneumatics, motion, thermal, fluid).
    fn step_physics(&mut self, dt_us: u64, inputs: &DomainInputs) -> DomainOutputs;

    /// Produce lightweight visual state struct for SVG layer rendering.
    fn visual_state(&self) -> ComponentVisualState;
}

/// Directed coupling of one physical component's scalar output into another component's input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainLink {
    pub from_component: String,
    pub from_signal: String,
    pub to_component: String,
    pub to_signal: String,
}

/// Electrical differential voltage tap feeding `V(inst_a.port_a) - V(inst_b.port_b)` into a
/// [`PhysicalComponent`]'s [`DomainInputs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoltageTap {
    pub component_id: String,
    pub input_key: String,
    pub inst_pos: String,
    pub port_pos: String,
    pub inst_neg: String,
    pub port_neg: String,
}

/// Multi-domain physics orchestrator coupling [`PhysicalComponent`] instances to [`Circuit`].
#[derive(Default)]
pub struct PhysicsWorld {
    components: BTreeMap<String, Box<dyn PhysicalComponent>>,
    links: Vec<DomainLink>,
    voltage_taps: Vec<VoltageTap>,
    last_outputs: BTreeMap<String, DomainOutputs>,
}

impl PhysicsWorld {
    /// Creates an empty [`PhysicsWorld`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a named physical component instance.
    pub fn add_component(&mut self, id: impl Into<String>, component: Box<dyn PhysicalComponent>) {
        self.components.insert(id.into(), component);
    }

    /// Connects scalar output `(from_component, from_signal)` -> input `(to_component, to_signal)`.
    pub fn add_link(
        &mut self,
        from_component: impl Into<String>,
        from_signal: impl Into<String>,
        to_component: impl Into<String>,
        to_signal: impl Into<String>,
    ) {
        self.links.push(DomainLink {
            from_component: from_component.into(),
            from_signal: from_signal.into(),
            to_component: to_component.into(),
            to_signal: to_signal.into(),
        });
    }

    /// Taps differential voltage `V(inst_pos.port_pos) - V(inst_neg.port_neg)` from `Circuit`
    /// into `component_id`'s scalar input `input_key`.
    pub fn add_voltage_tap(
        &mut self,
        component_id: impl Into<String>,
        input_key: impl Into<String>,
        inst_pos: impl Into<String>,
        port_pos: impl Into<String>,
        inst_neg: impl Into<String>,
        port_neg: impl Into<String>,
    ) {
        self.voltage_taps.push(VoltageTap {
            component_id: component_id.into(),
            input_key: input_key.into(),
            inst_pos: inst_pos.into(),
            port_pos: port_pos.into(),
            inst_neg: inst_neg.into(),
            port_neg: port_neg.into(),
        });
    }

    /// Advances all physical components by `dt_us`, propagates domain links, and stamps updated
    /// electrical boundary states (e.g., limit switches, 4–20 mA transmitters) back into `circuit`.
    pub fn step(&mut self, dt_us: u64, circuit: &mut Circuit) {
        // 1. Gather electrical voltage taps & previous-step domain links per component.
        let mut inputs_by_comp: BTreeMap<String, DomainInputs> = BTreeMap::new();
        for tap in &self.voltage_taps {
            let v_pos = circuit.terminal_voltage(&tap.inst_pos, &tap.port_pos);
            let v_neg = circuit.terminal_voltage(&tap.inst_neg, &tap.port_neg);
            inputs_by_comp
                .entry(tap.component_id.clone())
                .or_default()
                .scalars
                .insert(tap.input_key.clone(), v_pos - v_neg);
        }
        for link in &self.links {
            if let Some(prev_out) = self.last_outputs.get(&link.from_component) {
                if let Some(&val) = prev_out.scalars.get(&link.from_signal) {
                    inputs_by_comp
                        .entry(link.to_component.clone())
                        .or_default()
                        .scalars
                        .insert(link.to_signal.clone(), val);
                }
                if let Some(&flag) = prev_out.flags.get(&link.from_signal) {
                    inputs_by_comp
                        .entry(link.to_component.clone())
                        .or_default()
                        .flags
                        .insert(link.to_signal.clone(), flag);
                }
            }
        }

        // 2. Advance every component deterministically in BTreeMap ID order.
        let mut next_outputs = BTreeMap::new();
        for (id, comp) in &mut self.components {
            let inp = inputs_by_comp.get(id).cloned().unwrap_or_default();
            let out = comp.step_physics(dt_us, &inp);
            next_outputs.insert(id.clone(), out);
        }
        self.last_outputs = next_outputs;

        // 3. Second zero-dt propagation pass for instant kinematic/switch couplings (e.g., limit switches)
        //    so a piston crossing `x = L` closes its limit switch on the exact same step!
        let mut switch_inputs: BTreeMap<String, DomainInputs> = BTreeMap::new();
        for link in &self.links {
            if let Some(out) = self.last_outputs.get(&link.from_component)
                && let Some(&val) = out.scalars.get(&link.from_signal)
            {
                switch_inputs
                    .entry(link.to_component.clone())
                    .or_default()
                    .scalars
                    .insert(link.to_signal.clone(), val);
            }
        }
        for (id, inp) in &switch_inputs {
            if let Some(comp) = self.components.get_mut(id) {
                let out = comp.step_physics(0, inp);
                self.last_outputs.insert(id.clone(), out);
            }
        }

        // 4. Stamp electrical contacts / sensors back into the MNA circuit.
        let mut ctx = CircuitStampCtx::new(circuit);
        for comp in self.components.values() {
            comp.stamp_electrical(&mut ctx);
        }
    }

    /// Reads the latest visual state of a registered physical component.
    #[must_use]
    pub fn visual_state(&self, id: &str) -> Option<ComponentVisualState> {
        self.components.get(id).map(|c| c.visual_state())
    }

    /// Reads the latest [`DomainOutputs`] of a registered physical component.
    #[must_use]
    pub fn outputs(&self, id: &str) -> Option<&DomainOutputs> {
        self.last_outputs.get(id)
    }
}
