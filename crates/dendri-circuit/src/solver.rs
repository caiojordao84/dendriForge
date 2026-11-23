//! SPICE-class Modified Nodal Analysis (MNA) netlist builder, connected-component island solver,
//! Newton-Raphson nonlinear engine, transient ODE integrator, probes, and fault monitor
//! (`02-simulation-and-electrical-engine.md` §3).

use crate::devices::{
    BjtPolarity, ComponentSpec, DEFAULT_GMIN, DiodeParams, MosfetPolarity, VT_300K,
    eval_diode_junction, eval_ebers_moll, eval_shichman_hodges, junction_vcrit, pnjlim,
};
use crate::linalg::MnaMatrix;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Canonical `(instance_id, port_id)` circuit terminal identifier.
pub type TerminalKey = (String, String);

/// Electrical solver configuration (`02` §3.1–§3.3).
#[derive(Debug, Clone, PartialEq)]
pub struct CircuitConfig {
    /// Minimum conductance bleed (`Gmin = 1e-12 S`) preventing singular islands (`02` §3.1).
    pub gmin: f64,
    /// Number of transient steps per period of the highest AC source frequency (`02` §3.1).
    pub steps_per_period: u32,
    /// Maximum transient time step in microseconds when reactive elements are present.
    pub max_transient_dt_us: u64,
    /// Maximum Newton-Raphson iterations per island solve (`02` §3.1 hard budget).
    pub max_nr_iters: usize,
    /// Maximum cross-island coupling iterations (Optocoupler / Relay / SSR).
    pub max_coupling_passes: usize,
    /// Voltage source current threshold (in A) flagging [`FaultKind::ShortCircuit`].
    pub short_circuit_current_a: f64,
}

impl Default for CircuitConfig {
    fn default() -> Self {
        Self {
            gmin: DEFAULT_GMIN,
            steps_per_period: 200,
            max_transient_dt_us: 100,
            max_nr_iters: 60,
            max_coupling_passes: 6,
            short_circuit_current_a: 50.0,
        }
    }
}

/// Hard error returned during circuit construction or topology compilation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CircuitError {
    #[error("duplicate component instance `{0}`")]
    DuplicateComponent(String),
    #[error("unknown component instance `{0}`")]
    UnknownComponent(String),
    #[error("invalid component parameter on `{component}`: {message}")]
    InvalidParameter { component: String, message: String },
}

/// Non-fatal circuit warning (`02` §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarningKind {
    /// Electrical island had no explicit ground (`0 V`) reference; solved with `Gmin` bleed.
    FloatingSubcircuit {
        island_index: usize,
        node_count: usize,
    },
}

/// Electrical fault kind detected by the MNA fault monitor (`02` §3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FaultKind {
    ShortCircuit {
        current_a: f64,
        limit_a: f64,
    },
    PinOvervoltage {
        pin: String,
        voltage_v: f64,
        v_abs_max_v: f64,
        v_abs_min_v: f64,
    },
    PinOvercurrent {
        pin: String,
        current_a: f64,
        limit_a: f64,
    },
    LedBurned {
        forward_current_a: f64,
        i_max_a: f64,
    },
    LedReverseBreakdown {
        reverse_voltage_v: f64,
        v_br_v: f64,
    },
    ResistorOverpower {
        power_w: f64,
        p_rated_w: f64,
    },
    ConvergenceFailure {
        island_index: usize,
    },
}

/// Active electrical fault attached to a component or board instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElectricalFault {
    pub time_us: u64,
    pub instance_id: String,
    pub kind: FaultKind,
}

/// Lightweight dynamic visual state emitted per component for SVG rendering (`02` §5, `03` §5).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ComponentVisualState {
    pub luminosity: f64,
    pub burned: bool,
    pub energized: bool,
    pub current_a: f64,
    pub power_w: f64,
    pub angular_velocity_rad_s: f64,
}

/// Per-island solver telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IslandSolveStats {
    pub island_index: usize,
    pub node_count: usize,
    pub unknown_count: usize,
    pub nr_iterations: usize,
    pub has_ground: bool,
    pub needs_transient: bool,
}

/// Summary returned by a DC or transient solve step.
#[derive(Debug, Clone, PartialEq)]
pub struct SolveReport {
    pub time_us: u64,
    pub islands: Vec<IslandSolveStats>,
    pub warnings: Vec<WarningKind>,
    pub faults: Vec<ElectricalFault>,
}

/// Single oscilloscope waveform sample `(time_us, voltage_v)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProbeSample {
    pub time_us: u64,
    pub voltage_v: f64,
}

/// Aggregated virtual multimeter reading over a captured probe window.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MultimeterReading {
    pub samples: usize,
    pub v_avg: f64,
    pub v_rms: f64,
    pub v_min: f64,
    pub v_max: f64,
}

/// Non-invasive voltage probe measuring `V(pos) - V(neg)` (or `V(pos)` w.r.t. ground if `neg` is `None`).
#[derive(Debug, Clone)]
struct ProbeState {
    pos: TerminalKey,
    neg: Option<TerminalKey>,
    max_samples: usize,
    samples: Vec<ProbeSample>,
}

/// Mutable dynamic state for one component instance across transient steps.
#[derive(Debug, Clone, Default)]
struct DeviceState {
    // Capacitor / Inductor / Transformer history
    v_prev: f64,
    i_prev: f64,
    vl_prev: f64,
    i2_prev: f64,
    vl2_prev: f64,
    // Electromechanical / latching state
    energized: bool,
    latched: bool,
    conducting: bool,
    // Motor angular velocity (rad/s)
    omega_rad_s: f64,
    // Optocoupler transferred LED current (A)
    opto_led_current_a: f64,
    // Solved branch / device telemetry
    last_current_a: f64,
    last_power_w: f64,
    luminosity: f64,
    burned: bool,
}

#[derive(Debug, Clone)]
struct ComponentEntry {
    id: String,
    spec: ComponentSpec,
    state: DeviceState,
}

/// Compiled connected-component island (`02` §3.1).
#[derive(Debug, Clone)]
struct IslandPlan {
    index: usize,
    has_ground: bool,
    has_reactive: bool,
    has_time_varying_source: bool,
    has_nonlinear: bool,
    max_source_freq_hz: f64,
    /// External non-ground net IDs -> local MNA node index `0..ext_node_count`.
    net_to_local: BTreeMap<usize, usize>,
    /// Internal extra nodes allocated for series resistors (`Diode.r_s`, `Capacitor.esr_ohm`, `Opto.led.r_s`).
    /// Keyed by `(comp_idx, slot_tag)`.
    internal_nodes: BTreeMap<(usize, u8), usize>,
    /// Branch-current unknown indices keyed by `(comp_idx, branch_slot)`.
    branch_vars: BTreeMap<(usize, u8), usize>,
    /// Component indices belonging to this island (plus side tag: `0 = whole/primary`, `1 = secondary`).
    members: Vec<(usize, u8)>,
    total_unknowns: usize,
    /// Last converged MNA solution vector `x` (used as NR warm-start).
    last_x: Vec<f64>,
}

/// Integration scheme used during one MNA solve.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SolveMode {
    DcOp,
    Transient { dt_s: f64, trapezoidal: bool },
}

/// Headless pure-Rust SPICE-class MNA circuit simulator (`02` §3).
#[derive(Debug, Clone)]
pub struct Circuit {
    config: CircuitConfig,
    components: Vec<ComponentEntry>,
    comp_index: BTreeMap<String, usize>,
    wires: Vec<(TerminalKey, TerminalKey)>,
    probes: BTreeMap<String, ProbeState>,
    // Compiled netlist & islands (rebuilt lazily when `topology_dirty` is true)
    topology_dirty: bool,
    state_dirty: bool,
    /// True once initial operating point or UIC (`use_initial_conditions`) has been established.
    dc_initialized: bool,
    /// Force Backward Euler on the first transient step after any discrete switch/GPIO change.
    just_switched: bool,
    terminal_to_net: BTreeMap<TerminalKey, usize>,
    ground_nets: BTreeSet<usize>,
    islands: Vec<IslandPlan>,
    /// Solved voltage per canonical net ID.
    net_voltages: BTreeMap<usize, f64>,
    time_us: u64,
    last_warnings: Vec<WarningKind>,
    last_faults: Vec<ElectricalFault>,
}

impl Default for Circuit {
    fn default() -> Self {
        Self::new(CircuitConfig::default())
    }
}

impl Circuit {
    /// Creates an empty circuit with the given [`CircuitConfig`].
    #[must_use]
    pub fn new(config: CircuitConfig) -> Self {
        Self {
            config,
            components: Vec::new(),
            comp_index: BTreeMap::new(),
            wires: Vec::new(),
            probes: BTreeMap::new(),
            topology_dirty: true,
            state_dirty: true,
            dc_initialized: false,
            just_switched: true,
            terminal_to_net: BTreeMap::new(),
            ground_nets: BTreeSet::new(),
            islands: Vec::new(),
            net_voltages: BTreeMap::new(),
            time_us: 0,
            last_warnings: Vec::new(),
            last_faults: Vec::new(),
        }
    }

    /// Current circuit simulation time in microseconds.
    #[must_use]
    pub const fn time_us(&self) -> u64 {
        self.time_us
    }

    /// Number of compiled electrically isolated islands (`02` §3.1).
    #[must_use]
    pub fn island_count(&self) -> usize {
        self.islands.len()
    }

    /// Returns `true` if any electrical island requires transient stepping (reactive elements
    /// OR continuous time-varying sources, `02` §3.1).
    #[must_use]
    pub fn needs_transient(&self) -> bool {
        self.islands
            .iter()
            .any(|isl| isl.has_reactive || isl.has_time_varying_source)
    }

    /// Computes the maximum safe transient step size in microseconds based on the highest AC
    /// frequency (`dt <= T_min / steps_per_period`) and `max_transient_dt_us` (`02` §3.1).
    #[must_use]
    pub fn recommended_transient_dt_us(&self) -> Option<u64> {
        if !self.needs_transient() {
            return None;
        }
        let mut max_freq = 0.0_f64;
        for isl in &self.islands {
            if isl.max_source_freq_hz > max_freq {
                max_freq = isl.max_source_freq_hz;
            }
        }
        let mut dt = self.config.max_transient_dt_us.max(1);
        if max_freq > 0.0 {
            let period_us = 1_000_000.0 / max_freq;
            let spp = f64::from(self.config.steps_per_period.max(1));
            let ac_dt = libm::floor(period_us / spp).max(1.0) as u64;
            dt = dt.min(ac_dt.max(1));
        }
        Some(dt)
    }

    /// Adds a typed component instance to the circuit.
    pub fn add_component(
        &mut self,
        id: impl Into<String>,
        spec: ComponentSpec,
    ) -> Result<(), CircuitError> {
        let id = id.into();
        if self.comp_index.contains_key(&id) {
            return Err(CircuitError::DuplicateComponent(id));
        }
        validate_component_spec(&id, &spec)?;
        let mut state = DeviceState::default();
        if let ComponentSpec::Capacitor(ref c) = spec {
            state.v_prev = c.v_init_v;
        }
        if let ComponentSpec::Inductor(ref l) = spec {
            state.i_prev = l.i_init_a;
        }
        let idx = self.components.len();
        self.comp_index.insert(id.clone(), idx);
        self.components.push(ComponentEntry { id, spec, state });
        self.topology_dirty = true;
        self.state_dirty = true;
        Ok(())
    }

    /// Connects two terminals `(inst_a, port_a)` and `(inst_b, port_b)` with an ideal wire.
    pub fn connect(
        &mut self,
        inst_a: impl Into<String>,
        port_a: impl Into<String>,
        inst_b: impl Into<String>,
        port_b: impl Into<String>,
    ) {
        self.wires.push((
            (inst_a.into(), port_a.into()),
            (inst_b.into(), port_b.into()),
        ));
        self.topology_dirty = true;
        self.state_dirty = true;
    }

    /// Attaches a non-invasive virtual oscilloscope/multimeter probe measuring `V(pos) - V(neg)`.
    pub fn add_probe(
        &mut self,
        name: impl Into<String>,
        pos: TerminalKey,
        neg: Option<TerminalKey>,
        max_samples: usize,
    ) {
        self.probes.insert(
            name.into(),
            ProbeState {
                pos,
                neg,
                max_samples: max_samples.max(1),
                samples: Vec::new(),
            },
        );
    }

    /// Updates a switch's closed/open state dynamically at runtime (triggers event-driven solve).
    pub fn set_switch_closed(&mut self, id: &str, closed: bool) -> Result<(), CircuitError> {
        let &idx = self
            .comp_index
            .get(id)
            .ok_or_else(|| CircuitError::UnknownComponent(id.to_owned()))?;
        if let Some(entry) = self.components.get_mut(idx)
            && let ComponentSpec::Switch(ref mut sw) = entry.spec
        {
            if sw.closed != closed {
                sw.closed = closed;
                self.state_dirty = true;
                self.just_switched = true;
            }
            return Ok(());
        }
        Err(CircuitError::InvalidParameter {
            component: id.to_owned(),
            message: "component is not a Switch".to_owned(),
        })
    }

    /// Updates a potentiometer's wiper position (`0.0 ..= 1.0`).
    pub fn set_potentiometer_wiper(&mut self, id: &str, wiper: f64) -> Result<(), CircuitError> {
        let &idx = self
            .comp_index
            .get(id)
            .ok_or_else(|| CircuitError::UnknownComponent(id.to_owned()))?;
        if let Some(entry) = self.components.get_mut(idx)
            && let ComponentSpec::Potentiometer(ref mut pot) = entry.spec
        {
            pot.wiper = wiper.clamp(0.0, 1.0);
            self.state_dirty = true;
            self.just_switched = true;
            return Ok(());
        }
        Err(CircuitError::InvalidParameter {
            component: id.to_owned(),
            message: "component is not a Potentiometer".to_owned(),
        })
    }

    /// Updates a DC voltage source's voltage dynamically (used by MCU/PLC pin boundary & tests).
    pub fn set_dc_voltage(&mut self, id: &str, voltage_v: f64) -> Result<(), CircuitError> {
        let &idx = self
            .comp_index
            .get(id)
            .ok_or_else(|| CircuitError::UnknownComponent(id.to_owned()))?;
        if let Some(entry) = self.components.get_mut(idx)
            && let ComponentSpec::VoltageSourceDc(ref mut vs) = entry.spec
        {
            if (vs.voltage_v - voltage_v).abs() > 1e-15 {
                vs.voltage_v = voltage_v;
                self.state_dirty = true;
                self.just_switched = true;
            }
            return Ok(());
        }
        Err(CircuitError::InvalidParameter {
            component: id.to_owned(),
            message: "component is not a VoltageSourceDc".to_owned(),
        })
    }

    /// Updates a resistor's resistance dynamically (e.g. pull-up/pull-down or variable load).
    pub fn set_resistance(&mut self, id: &str, r_ohm: f64) -> Result<(), CircuitError> {
        let &idx = self
            .comp_index
            .get(id)
            .ok_or_else(|| CircuitError::UnknownComponent(id.to_owned()))?;
        if let Some(entry) = self.components.get_mut(idx)
            && let ComponentSpec::Resistor(ref mut r) = entry.spec
        {
            r.r_ohm = r_ohm.max(1e-6);
            self.state_dirty = true;
            self.just_switched = true;
            return Ok(());
        }
        Err(CircuitError::InvalidParameter {
            component: id.to_owned(),
            message: "component is not a Resistor".to_owned(),
        })
    }

    /// Updates a DC current source's current dynamically (e.g. 4-20 mA sensor transmitter).
    pub fn set_dc_current(&mut self, id: &str, current_a: f64) -> Result<(), CircuitError> {
        let &idx = self
            .comp_index
            .get(id)
            .ok_or_else(|| CircuitError::UnknownComponent(id.to_owned()))?;
        if let Some(entry) = self.components.get_mut(idx)
            && let ComponentSpec::CurrentSourceDc(ref mut cs) = entry.spec
        {
            cs.current_a = current_a;
            self.state_dirty = true;
            self.just_switched = true;
            return Ok(());
        }
        Err(CircuitError::InvalidParameter {
            component: id.to_owned(),
            message: "component is not a CurrentSourceDc".to_owned(),
        })
    }

    /// Reads the solved node voltage at terminal `(inst, port)` (returns `0.0` for ground or unknown).
    #[must_use]
    pub fn terminal_voltage(&self, inst: &str, port: &str) -> f64 {
        let key = (inst.to_owned(), port.to_owned());
        let Some(&net) = self.terminal_to_net.get(&key) else {
            return 0.0;
        };
        if self.ground_nets.contains(&net) {
            return 0.0;
        }
        self.net_voltages.get(&net).copied().unwrap_or(0.0)
    }

    /// Returns the last solved current (in A) for component `id`.
    #[must_use]
    pub fn component_current(&self, id: &str) -> f64 {
        self.comp_index
            .get(id)
            .and_then(|&idx| self.components.get(idx))
            .map_or(0.0, |c| c.state.last_current_a)
    }

    /// Returns the [`ComponentVisualState`] for component `id` (`02` §5, `03` §5).
    #[must_use]
    pub fn visual_state(&self, id: &str) -> ComponentVisualState {
        self.comp_index
            .get(id)
            .and_then(|&idx| self.components.get(idx))
            .map_or_else(ComponentVisualState::default, |c| ComponentVisualState {
                luminosity: c.state.luminosity,
                burned: c.state.burned,
                energized: c.state.energized || c.state.latched || c.state.conducting,
                current_a: c.state.last_current_a,
                power_w: c.state.last_power_w,
                angular_velocity_rad_s: c.state.omega_rad_s,
            })
    }

    /// Returns active warnings from the latest solve.
    #[must_use]
    pub fn warnings(&self) -> &[WarningKind] {
        &self.last_warnings
    }

    /// Returns active faults from the latest solve.
    #[must_use]
    pub fn faults(&self) -> &[ElectricalFault] {
        &self.last_faults
    }

    /// Returns captured oscilloscope waveform samples for `probe_name`.
    #[must_use]
    pub fn probe_samples(&self, probe_name: &str) -> &[ProbeSample] {
        self.probes
            .get(probe_name)
            .map_or(&[], |p| p.samples.as_slice())
    }

    /// Computes virtual multimeter statistics (average, RMS, min, max) over captured samples
    /// (optionally skipping initial samples before `from_time_us` for steady-state RMS!).
    #[must_use]
    pub fn multimeter_reading(&self, probe_name: &str, from_time_us: u64) -> MultimeterReading {
        let Some(probe) = self.probes.get(probe_name) else {
            return MultimeterReading::default();
        };
        let mut count = 0usize;
        let mut sum = 0.0_f64;
        let mut sum_sq = 0.0_f64;
        let mut v_min = f64::INFINITY;
        let mut v_max = f64::NEG_INFINITY;

        for s in &probe.samples {
            if s.time_us < from_time_us {
                continue;
            }
            count += 1;
            sum += s.voltage_v;
            sum_sq += s.voltage_v * s.voltage_v;
            if s.voltage_v < v_min {
                v_min = s.voltage_v;
            }
            if s.voltage_v > v_max {
                v_max = s.voltage_v;
            }
        }
        if count == 0 {
            return MultimeterReading::default();
        }
        let n_f = count as f64;
        MultimeterReading {
            samples: count,
            v_avg: sum / n_f,
            v_rms: libm::sqrt(sum_sq / n_f),
            v_min,
            v_max,
        }
    }

    /// Compiles netlist & connected-component islands (idempotent if topology unchanged).
    pub fn compile_topology(&mut self) {
        if !self.topology_dirty {
            return;
        }
        self.topology_dirty = false;

        // 1. Collect every terminal across components & wires in deterministic order.
        let mut term_to_idx: BTreeMap<TerminalKey, usize> = BTreeMap::new();
        for comp in &self.components {
            for port in component_ports(&comp.spec) {
                let key = (comp.id.clone(), port);
                let next = term_to_idx.len();
                term_to_idx.entry(key).or_insert(next);
            }
        }
        for (a, b) in &self.wires {
            let na = term_to_idx.len();
            term_to_idx.entry(a.clone()).or_insert(na);
            let nb = term_to_idx.len();
            term_to_idx.entry(b.clone()).or_insert(nb);
        }

        // 2. Union-Find to merge wires & breadboard strips into canonical Nets.
        let mut uf = UnionFind::new(term_to_idx.len());
        for (a, b) in &self.wires {
            if let (Some(&ia), Some(&ib)) = (term_to_idx.get(a), term_to_idx.get(b)) {
                uf.union(ia, ib);
            }
        }
        for comp in &self.components {
            if let ComponentSpec::BreadboardStrip { ref ports } = comp.spec
                && let Some(first_port) = ports.first()
                && let Some(&first_idx) = term_to_idx.get(&(comp.id.clone(), first_port.clone()))
            {
                for p in ports.iter().skip(1) {
                    if let Some(&pi) = term_to_idx.get(&(comp.id.clone(), p.clone())) {
                        uf.union(first_idx, pi);
                    }
                }
            }
        }

        // Remap UF roots to dense NetIds 0..num_nets
        let mut root_to_net: BTreeMap<usize, usize> = BTreeMap::new();
        self.terminal_to_net.clear();
        for (term, idx) in &term_to_idx {
            let root = uf.find(*idx);
            let next_net = root_to_net.len();
            let net = *root_to_net.entry(root).or_insert(next_net);
            self.terminal_to_net.insert(term.clone(), net);
        }
        let num_nets = root_to_net.len();

        // Mark explicit Ground nets (each ground island keeps its own 0 V reference!)
        self.ground_nets.clear();
        for comp in &self.components {
            if matches!(comp.spec, ComponentSpec::Ground)
                && let Some(&net) = self
                    .terminal_to_net
                    .get(&(comp.id.clone(), "GND".to_owned()))
            {
                self.ground_nets.insert(net);
            }
        }

        // 3. Connected-component island partitioning over Nets using ONLY galvanic sub-groups.
        //    Galvanically isolated devices (Optocoupler, Relay, SSR) split into primary (side 0)
        //    and secondary (side 1) so isolated domains solve as separate smaller MNA islands!
        let mut island_uf = UnionFind::new(num_nets);
        let mut comp_groups: Vec<(usize, u8, Vec<usize>)> = Vec::new();

        for (c_idx, comp) in self.components.iter().enumerate() {
            if matches!(
                comp.spec,
                ComponentSpec::Ground | ComponentSpec::BreadboardStrip { .. }
            ) {
                continue;
            }
            for (side, ports) in galvanic_port_groups(&comp.spec) {
                let mut nets: Vec<usize> = Vec::new();
                for p in ports {
                    if let Some(&net) = self.terminal_to_net.get(&(comp.id.clone(), p.to_owned())) {
                        nets.push(net);
                    }
                }
                if let Some(&first_net) = nets.first() {
                    for &n in nets.iter().skip(1) {
                        island_uf.union(first_net, n);
                    }
                    comp_groups.push((c_idx, side, nets));
                }
            }
        }

        // Group nets & component sides by island root
        let mut island_nets: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        let mut island_members: BTreeMap<usize, Vec<(usize, u8)>> = BTreeMap::new();
        for (c_idx, side, nets) in comp_groups {
            if let Some(&first_net) = nets.first() {
                let root = island_uf.find(first_net);
                let net_set = island_nets.entry(root).or_default();
                for n in nets {
                    net_set.insert(n);
                }
                island_members.entry(root).or_default().push((c_idx, side));
            }
        }

        // Build each IslandPlan deterministically
        self.islands.clear();
        for (island_idx, (root, nets)) in island_nets.into_iter().enumerate() {
            let members = island_members.remove(&root).unwrap_or_default();
            let has_ground = nets.iter().any(|n| self.ground_nets.contains(n));

            let mut net_to_local = BTreeMap::new();
            for &n in &nets {
                if !self.ground_nets.contains(&n) {
                    let local = net_to_local.len();
                    net_to_local.insert(n, local);
                }
            }

            let mut next_unknown = net_to_local.len();
            let mut internal_nodes = BTreeMap::new();
            let mut branch_vars = BTreeMap::new();
            let mut has_reactive = false;
            let mut has_time_varying_source = false;
            let mut has_nonlinear = false;
            let mut max_source_freq_hz = 0.0_f64;

            for &(c_idx, side) in &members {
                let Some(comp) = self.components.get(c_idx) else {
                    continue;
                };
                match &comp.spec {
                    ComponentSpec::Resistor(_)
                    | ComponentSpec::CurrentSourceDc(_)
                    | ComponentSpec::Potentiometer(_)
                    | ComponentSpec::Switch(_)
                    | ComponentSpec::Ssr(_)
                    | ComponentSpec::BreadboardStrip { .. }
                    | ComponentSpec::Ground => {}
                    ComponentSpec::HBridge(_) => {
                        has_nonlinear = true;
                    }
                    ComponentSpec::VoltageSourceDc(vs) => {
                        if vs.r_ser_ohm <= 0.0 {
                            branch_vars.insert((c_idx, 0), next_unknown);
                            next_unknown += 1;
                        }
                    }
                    ComponentSpec::VoltageSourceAc(ac) => {
                        branch_vars.insert((c_idx, 0), next_unknown);
                        next_unknown += 1;
                        if ac.freq_hz > 0.0 {
                            has_time_varying_source = true;
                            if ac.freq_hz > max_source_freq_hz {
                                max_source_freq_hz = ac.freq_hz;
                            }
                        }
                    }
                    ComponentSpec::Diode(d) => {
                        has_nonlinear = true;
                        if d.r_s > 0.0 {
                            internal_nodes.insert((c_idx, 0), next_unknown);
                            next_unknown += 1;
                        }
                    }
                    ComponentSpec::Capacitor(c) => {
                        has_reactive = true;
                        if c.esr_ohm > 0.0 {
                            internal_nodes.insert((c_idx, 0), next_unknown);
                            next_unknown += 1;
                        }
                    }
                    ComponentSpec::Inductor(_) => {
                        has_reactive = true;
                        branch_vars.insert((c_idx, 0), next_unknown);
                        next_unknown += 1;
                    }
                    ComponentSpec::Transformer(_) => {
                        has_reactive = true;
                        branch_vars.insert((c_idx, 0), next_unknown);
                        next_unknown += 1;
                        branch_vars.insert((c_idx, 1), next_unknown);
                        next_unknown += 1;
                    }
                    ComponentSpec::Relay(r) => {
                        if side == 0 && r.l_coil_h > 0.0 {
                            has_reactive = true;
                            branch_vars.insert((c_idx, 0), next_unknown);
                            next_unknown += 1;
                        }
                    }
                    ComponentSpec::Bjt(_) | ComponentSpec::Mosfet(_) => {
                        has_nonlinear = true;
                    }
                    ComponentSpec::Optocoupler(opt) => {
                        has_nonlinear = true;
                        if side == 0 && opt.led.r_s > 0.0 {
                            internal_nodes.insert((c_idx, 0), next_unknown);
                            next_unknown += 1;
                        }
                    }
                    ComponentSpec::Igbt(_) | ComponentSpec::Scr(_) | ComponentSpec::Triac(_) => {
                        has_nonlinear = true;
                    }
                    ComponentSpec::DcMotor(m) => {
                        if m.l_arm_h > 0.0 || m.inertia_j > 0.0 {
                            has_reactive = true;
                        }
                        branch_vars.insert((c_idx, 0), next_unknown);
                        next_unknown += 1;
                    }
                }
            }

            self.islands.push(IslandPlan {
                index: island_idx,
                has_ground,
                has_reactive,
                has_time_varying_source,
                has_nonlinear,
                max_source_freq_hz,
                net_to_local,
                internal_nodes,
                branch_vars,
                members,
                total_unknowns: next_unknown,
                last_x: vec![0.0; next_unknown],
            });
        }
    }

    /// Solves the DC operating point across all islands at current `time_us` (`02` §3.1).
    pub fn solve_dc_op(&mut self) -> SolveReport {
        self.compile_topology();
        let report = self.solve_all_islands(SolveMode::DcOp);
        self.state_dirty = false;
        self.dc_initialized = true;
        self.just_switched = true;
        self.record_probes();
        report
    }

    /// Initializes reactive device states directly from declared `v_init_v` / `i_init_a`
    /// without running a $t = \infty$ DC steady-state solve (SPICE `.TRAN ... UIC`).
    pub fn use_initial_conditions(&mut self) {
        self.compile_topology();
        for comp in &mut self.components {
            if let ComponentSpec::Capacitor(ref c) = comp.spec {
                comp.state.v_prev = c.v_init_v;
                comp.state.i_prev = 0.0;
            }
            if let ComponentSpec::Inductor(ref l) = comp.spec {
                comp.state.i_prev = l.i_init_a;
                comp.state.vl_prev = 0.0;
            }
        }
        self.dc_initialized = true;
        self.just_switched = true;
    }

    /// Advances the circuit to `target_us` using event-driven DC + transient ODE stepping (`02` §3.1):
    /// - Islands with neither reactive elements nor continuous time-varying sources solve DC only
    ///   when discrete state/topology changed!
    /// - Islands with reactive elements (`C`/`L`/`Transformer`/`Relay`/`DcMotor`) OR continuous AC
    ///   sources step with `dt <= min(T_min / steps_per_period, max_transient_dt_us)`.
    pub fn step_to(&mut self, target_us: u64) -> SolveReport {
        self.compile_topology();

        if target_us <= self.time_us {
            if self.state_dirty || !self.dc_initialized {
                return self.solve_dc_op();
            }
            return self.current_report();
        }

        let Some(max_dt_us) = self.recommended_transient_dt_us() else {
            // Purely non-reactive, non-AC circuit: event-driven DC solve only when dirty!
            self.time_us = target_us;
            if self.state_dirty || !self.dc_initialized {
                return self.solve_dc_op();
            }
            self.record_probes();
            return self.current_report();
        };

        // Only auto-solve DC operating point if NEVER initialized yet (preserves switch-flip
        // initial conditions established by an explicit `solve_dc_op()` or `use_initial_conditions()`!).
        if !self.dc_initialized {
            let _ = self.solve_dc_op();
        }

        let mut last_report = self.current_report();
        while self.time_us < target_us {
            let remaining = target_us - self.time_us;
            let step_us = remaining.min(max_dt_us).max(1);
            self.time_us = self.time_us.saturating_add(step_us);
            let dt_s = (step_us as f64) * 1e-6;
            // Use Backward Euler right after a switching/discrete discontinuity to prevent
            // Trapezoidal ringing, then switch to 2nd-order Trapezoidal for smooth ODE accuracy!
            let trapezoidal = !self.just_switched;
            last_report = self.solve_all_islands(SolveMode::Transient { dt_s, trapezoidal });
            self.just_switched = false;
            self.state_dirty = false;
            self.record_probes();
        }

        last_report
    }

    fn current_report(&self) -> SolveReport {
        SolveReport {
            time_us: self.time_us,
            islands: self
                .islands
                .iter()
                .map(|i| IslandSolveStats {
                    island_index: i.index,
                    node_count: i.net_to_local.len(),
                    unknown_count: i.total_unknowns,
                    nr_iterations: 0,
                    has_ground: i.has_ground,
                    needs_transient: i.has_reactive || i.has_time_varying_source,
                })
                .collect(),
            warnings: self.last_warnings.clone(),
            faults: self.last_faults.clone(),
        }
    }

    fn solve_all_islands(&mut self, mode: SolveMode) -> SolveReport {
        let mut warnings = Vec::new();
        let mut faults = Vec::new();
        let mut stats = Vec::with_capacity(self.islands.len());

        for isl in &self.islands {
            if !isl.has_ground {
                warnings.push(WarningKind::FloatingSubcircuit {
                    island_index: isl.index,
                    node_count: isl.net_to_local.len(),
                });
            }
        }

        // Cross-island fixed-point loop (handles Optocoupler / Relay / SSR / SCR / TRIAC updates).
        let max_passes = self.config.max_coupling_passes.max(1);
        for pass in 0..max_passes {
            stats.clear();
            let mut coupling_changed = false;

            let island_count = self.islands.len();
            for i_idx in 0..island_count {
                let (x_sol, nr_iters, converged) = self.solve_single_island(i_idx, mode);
                if !converged {
                    faults.push(ElectricalFault {
                        time_us: self.time_us,
                        instance_id: format!("island_{i_idx}"),
                        kind: FaultKind::ConvergenceFailure {
                            island_index: i_idx,
                        },
                    });
                }
                if let Some(isl) = self.islands.get_mut(i_idx) {
                    for (&net_id, &local_idx) in &isl.net_to_local {
                        let v = x_sol.get(local_idx).copied().unwrap_or(0.0);
                        self.net_voltages.insert(net_id, v);
                    }
                    isl.last_x = x_sol;
                    stats.push(IslandSolveStats {
                        island_index: isl.index,
                        node_count: isl.net_to_local.len(),
                        unknown_count: isl.total_unknowns,
                        nr_iterations: nr_iters,
                        has_ground: isl.has_ground,
                        needs_transient: isl.has_reactive || isl.has_time_varying_source,
                    });
                }
            }

            // Update cross-island & latching states (Optocoupler LED current, Relay coil, SSR, SCR, TRIAC).
            if self.update_switching_and_coupling_states() {
                coupling_changed = true;
            }
            if !coupling_changed || pass + 1 == max_passes {
                break;
            }
        }

        // Commit reactive companion histories (C, L, Transformer, Relay coil, DcMotor) & check faults.
        self.commit_post_solve_states(mode, &mut faults);

        self.last_warnings = warnings.clone();
        self.last_faults = faults.clone();
        SolveReport {
            time_us: self.time_us,
            islands: stats,
            warnings,
            faults,
        }
    }

    fn solve_single_island(&self, island_idx: usize, mode: SolveMode) -> (Vec<f64>, usize, bool) {
        let Some(isl) = self.islands.get(island_idx) else {
            return (Vec::new(), 0, true);
        };
        if isl.total_unknowns == 0 {
            return (Vec::new(), 0, true);
        }

        // Fast path: purely linear island -> 1 single LU solve!
        if !isl.has_nonlinear {
            let mat = self.build_island_matrix(isl, &isl.last_x, mode, self.config.gmin, 1.0);
            return match mat.solve() {
                Ok(x) => (x, 1, true),
                Err(_) => {
                    // Try slightly stronger Gmin bleed on near-singular floating island
                    let mat2 = self.build_island_matrix(isl, &isl.last_x, mode, 1e-9, 1.0);
                    match mat2.solve() {
                        Ok(x) => (x, 1, true),
                        Err(_) => (vec![0.0; isl.total_unknowns], 1, false),
                    }
                }
            };
        }

        // Strategy 1: Direct Newton-Raphson with pnjlim damping
        if let Ok((x, iters)) = self.run_nr_loop(
            isl,
            &isl.last_x,
            mode,
            self.config.gmin,
            1.0,
            self.config.max_nr_iters,
        ) {
            return (x, iters, true);
        }

        // Strategy 2: Gmin stepping fallback (`02` §3.2)
        let mut x_warm = vec![0.0; isl.total_unknowns];
        let mut total_iters = 0usize;
        let gmin_ladder = [1e-3, 1e-5, 1e-7, 1e-9, 1e-11, self.config.gmin];
        let mut gmin_ok = true;
        for &g in &gmin_ladder {
            match self.run_nr_loop(isl, &x_warm, mode, g, 1.0, self.config.max_nr_iters) {
                Ok((x, it)) => {
                    x_warm = x;
                    total_iters = total_iters.saturating_add(it);
                }
                Err(_) => {
                    gmin_ok = false;
                    break;
                }
            }
        }
        if gmin_ok {
            return (x_warm, total_iters, true);
        }

        // Strategy 3: Source stepping fallback (`02` §3.2)
        x_warm = vec![0.0; isl.total_unknowns];
        let source_ladder = [0.1, 0.25, 0.5, 0.75, 1.0];
        for &scale in &source_ladder {
            match self.run_nr_loop(isl, &x_warm, mode, 1e-10, scale, self.config.max_nr_iters) {
                Ok((x, it)) => {
                    x_warm = x;
                    total_iters = total_iters.saturating_add(it);
                }
                Err(_) => return (x_warm, total_iters, false),
            }
        }
        match self.run_nr_loop(
            isl,
            &x_warm,
            mode,
            self.config.gmin,
            1.0,
            self.config.max_nr_iters,
        ) {
            Ok((x, it)) => (x, total_iters.saturating_add(it), true),
            Err(_) => (x_warm, total_iters, false),
        }
    }

    fn run_nr_loop(
        &self,
        isl: &IslandPlan,
        x_start: &[f64],
        mode: SolveMode,
        gmin: f64,
        source_scale: f64,
        max_iters: usize,
    ) -> Result<(Vec<f64>, usize), ()> {
        let mut x_cur = x_start.to_vec();
        if x_cur.len() != isl.total_unknowns {
            x_cur.resize(isl.total_unknowns, 0.0);
        }

        for iter in 1..=max_iters.max(1) {
            let mat = self.build_island_matrix(isl, &x_cur, mode, gmin, source_scale);
            let mut x_next = mat.solve().map_err(|_| ())?;

            // Apply SPICE3 `pnjlim` junction damping on diode/LED/Zener/BJT updates
            let damped = self.apply_pnjlim(isl, &x_cur, &mut x_next);

            let mut max_err = 0.0_f64;
            for idx in 0..isl.total_unknowns {
                let old_v = x_cur.get(idx).copied().unwrap_or(0.0);
                let new_v = x_next.get(idx).copied().unwrap_or(0.0);
                let diff = libm::fabs(new_v - old_v);
                let tol = 1e-7 * (1.0 + libm::fabs(new_v)) + 1e-9;
                let rel = diff / tol;
                if rel > max_err {
                    max_err = rel;
                }
            }

            x_cur = x_next;
            if !damped && max_err <= 1.0 {
                return Ok((x_cur, iter));
            }
        }
        Err(())
    }

    fn apply_pnjlim(&self, isl: &IslandPlan, x_old: &[f64], x_new: &mut [f64]) -> bool {
        let mut any_clamped = false;
        let read_v = |x: &[f64], node: Option<usize>| -> f64 {
            node.and_then(|i| x.get(i).copied()).unwrap_or(0.0)
        };

        for &(c_idx, side) in &isl.members {
            let Some(comp) = self.components.get(c_idx) else {
                continue;
            };
            match &comp.spec {
                ComponentSpec::Diode(d) => {
                    let na = if d.r_s > 0.0 {
                        isl.internal_nodes.get(&(c_idx, 0)).copied()
                    } else {
                        self.local_node(isl, &comp.id, "A")
                    };
                    let nk = self.local_node(isl, &comp.id, "K");
                    if limit_two_node_junction(d, na, nk, x_old, x_new) {
                        any_clamped = true;
                    }
                }
                ComponentSpec::Optocoupler(opt) if side == 0 => {
                    let na = if opt.led.r_s > 0.0 {
                        isl.internal_nodes.get(&(c_idx, 0)).copied()
                    } else {
                        self.local_node(isl, &comp.id, "ANODE")
                    };
                    let nk = self.local_node(isl, &comp.id, "CATHODE");
                    if limit_two_node_junction(&opt.led, na, nk, x_old, x_new) {
                        any_clamped = true;
                    }
                }
                ComponentSpec::Bjt(b) => {
                    let nb = self.local_node(isl, &comp.id, "B");
                    let nc = self.local_node(isl, &comp.id, "C");
                    let ne = self.local_node(isl, &comp.id, "E");
                    let sign = match b.polarity {
                        BjtPolarity::Npn => 1.0,
                        BjtPolarity::Pnp => -1.0,
                    };
                    let vcrit = junction_vcrit(VT_300K, b.i_s);
                    let v_be_old = sign * (read_v(x_old, nb) - read_v(x_old, ne));
                    let v_be_new = sign * (read_v(x_new, nb) - read_v(x_new, ne));
                    let v_be_lim = pnjlim(v_be_new, v_be_old, VT_300K, vcrit);
                    if libm::fabs(v_be_lim - v_be_new) > 1e-12 {
                        let delta = sign * (v_be_lim - v_be_new);
                        if let Some(b_idx) = nb
                            && let Some(slot) = x_new.get_mut(b_idx)
                        {
                            *slot += delta;
                            any_clamped = true;
                        }
                    }
                    let v_bc_old = sign * (read_v(x_old, nb) - read_v(x_old, nc));
                    let v_bc_new = sign * (read_v(x_new, nb) - read_v(x_new, nc));
                    let v_bc_lim = pnjlim(v_bc_new, v_bc_old, VT_300K, vcrit);
                    if libm::fabs(v_bc_lim - v_bc_new) > 1e-12 {
                        let delta = sign * (v_bc_lim - v_bc_new);
                        if let Some(c_idx_n) = nc
                            && let Some(slot) = x_new.get_mut(c_idx_n)
                        {
                            *slot -= delta;
                            any_clamped = true;
                        }
                    }
                }
                ComponentSpec::Resistor(_)
                | ComponentSpec::VoltageSourceDc(_)
                | ComponentSpec::CurrentSourceDc(_)
                | ComponentSpec::Potentiometer(_)
                | ComponentSpec::Switch(_)
                | ComponentSpec::Relay(_)
                | ComponentSpec::Capacitor(_)
                | ComponentSpec::Inductor(_)
                | ComponentSpec::Transformer(_)
                | ComponentSpec::VoltageSourceAc(_)
                | ComponentSpec::Mosfet(_)
                | ComponentSpec::Optocoupler(_)
                | ComponentSpec::Igbt(_)
                | ComponentSpec::Scr(_)
                | ComponentSpec::Triac(_)
                | ComponentSpec::Ssr(_)
                | ComponentSpec::HBridge(_)
                | ComponentSpec::DcMotor(_)
                | ComponentSpec::BreadboardStrip { .. }
                | ComponentSpec::Ground => {}
            }
        }

        any_clamped
    }

    fn local_node(&self, isl: &IslandPlan, comp_id: &str, port: &str) -> Option<usize> {
        let &net = self
            .terminal_to_net
            .get(&(comp_id.to_owned(), port.to_owned()))?;
        if self.ground_nets.contains(&net) {
            return None;
        }
        isl.net_to_local.get(&net).copied()
    }

    fn build_island_matrix(
        &self,
        isl: &IslandPlan,
        x_cur: &[f64],
        mode: SolveMode,
        gmin: f64,
        source_scale: f64,
    ) -> MnaMatrix {
        let mut m = MnaMatrix::new(isl.total_unknowns);
        let read_v = |node: Option<usize>| -> f64 {
            node.and_then(|i| x_cur.get(i).copied()).unwrap_or(0.0)
        };

        // Diagonal Gmin bleed to reference ground on every node prevents singular floating nodes
        // (and ensures floating subcircuits solve non-singularly per `02` §3.1).
        let node_count = isl.net_to_local.len() + isl.internal_nodes.len();
        for node_idx in 0..node_count {
            m.add(node_idx, node_idx, gmin);
        }

        for &(c_idx, side) in &isl.members {
            let Some(comp) = self.components.get(c_idx) else {
                continue;
            };
            match &comp.spec {
                ComponentSpec::Resistor(r) => {
                    let na = self.local_node(isl, &comp.id, "A");
                    let nb = self.local_node(isl, &comp.id, "B");
                    m.stamp_conductance(na, nb, 1.0 / r.r_ohm.max(1e-9));
                }
                ComponentSpec::VoltageSourceDc(vs) => {
                    let np = self.local_node(isl, &comp.id, "POS");
                    let nn = self.local_node(isl, &comp.id, "NEG");
                    let v = vs.voltage_v * source_scale;
                    if vs.r_ser_ohm > 0.0 {
                        // Norton equivalent: G = 1/R_ser, I = V/R_ser from NEG -> POS
                        let g = 1.0 / vs.r_ser_ohm;
                        m.stamp_conductance(np, nn, g);
                        m.stamp_current_source(nn, np, v * g);
                    } else if let Some(&br) = isl.branch_vars.get(&(c_idx, 0)) {
                        m.stamp_voltage_source(np, nn, br, v);
                    }
                }
                ComponentSpec::VoltageSourceAc(ac) => {
                    let np = self.local_node(isl, &comp.id, "POS");
                    let nn = self.local_node(isl, &comp.id, "NEG");
                    let v = ac.voltage_at_us(self.time_us) * source_scale;
                    if let Some(&br) = isl.branch_vars.get(&(c_idx, 0)) {
                        m.stamp_voltage_source(np, nn, br, v);
                    }
                }
                ComponentSpec::CurrentSourceDc(cs) => {
                    let ni = self.local_node(isl, &comp.id, "IN");
                    let no = self.local_node(isl, &comp.id, "OUT");
                    m.stamp_current_source(ni, no, cs.current_a * source_scale);
                }
                ComponentSpec::Potentiometer(pot) => {
                    let na = self.local_node(isl, &comp.id, "A");
                    let nw = self.local_node(isl, &comp.id, "W");
                    let nb = self.local_node(isl, &comp.id, "B");
                    let w = pot.wiper.clamp(0.0, 1.0);
                    let r_tot = pot.r_total_ohm.max(1e-6);
                    let r_aw = (w * r_tot).max(1e-6);
                    let r_wb = ((1.0 - w) * r_tot).max(1e-6);
                    m.stamp_conductance(na, nw, 1.0 / r_aw);
                    m.stamp_conductance(nw, nb, 1.0 / r_wb);
                }
                ComponentSpec::Switch(sw) => {
                    let na = self.local_node(isl, &comp.id, "A");
                    let nb = self.local_node(isl, &comp.id, "B");
                    let r = if sw.closed { sw.r_on_ohm } else { sw.r_off_ohm };
                    m.stamp_conductance(na, nb, 1.0 / r.max(1e-9));
                }
                ComponentSpec::Relay(rel) => {
                    if side == 0 {
                        // Inductive relay coil (`COIL_A` -> `COIL_B`)
                        let na = self.local_node(isl, &comp.id, "COIL_A");
                        let nb = self.local_node(isl, &comp.id, "COIL_B");
                        if rel.l_coil_h > 0.0
                            && let Some(&br) = isl.branch_vars.get(&(c_idx, 0))
                        {
                            stamp_inductor_branch(
                                &mut m,
                                na,
                                nb,
                                br,
                                rel.l_coil_h,
                                rel.r_coil_ohm,
                                comp.state.i_prev,
                                comp.state.vl_prev,
                                mode,
                            );
                        } else {
                            m.stamp_conductance(na, nb, 1.0 / rel.r_coil_ohm.max(1e-6));
                        }
                    } else {
                        // Galvanically isolated changeover contacts (`COM`, `NO`, `NC`)
                        let ncom = self.local_node(isl, &comp.id, "COM");
                        let nno = self.local_node(isl, &comp.id, "NO");
                        let nnc = self.local_node(isl, &comp.id, "NC");
                        let r_no = if comp.state.energized {
                            rel.r_on_ohm
                        } else {
                            rel.r_off_ohm
                        };
                        let r_nc = if comp.state.energized {
                            rel.r_off_ohm
                        } else {
                            rel.r_on_ohm
                        };
                        m.stamp_conductance(ncom, nno, 1.0 / r_no.max(1e-9));
                        m.stamp_conductance(ncom, nnc, 1.0 / r_nc.max(1e-9));
                    }
                }
                ComponentSpec::Diode(d) => {
                    let na = self.local_node(isl, &comp.id, "A");
                    let nk = self.local_node(isl, &comp.id, "K");
                    let nj = if d.r_s > 0.0 {
                        let nint = isl.internal_nodes.get(&(c_idx, 0)).copied();
                        m.stamp_conductance(na, nint, 1.0 / d.r_s.max(1e-9));
                        nint
                    } else {
                        na
                    };
                    let vj = read_v(nj) - read_v(nk);
                    let st = eval_diode_junction(d, vj, gmin);
                    m.stamp_conductance(nj, nk, st.conductance_s);
                    m.stamp_current_source(nj, nk, st.i_eq_a);
                }
                ComponentSpec::Capacitor(c) => {
                    let na = self.local_node(isl, &comp.id, "A");
                    let nb = self.local_node(isl, &comp.id, "B");
                    let nc = if c.esr_ohm > 0.0 {
                        let nint = isl.internal_nodes.get(&(c_idx, 0)).copied();
                        m.stamp_conductance(na, nint, 1.0 / c.esr_ohm.max(1e-9));
                        nint
                    } else {
                        na
                    };
                    match mode {
                        SolveMode::DcOp => {
                            // Open circuit at DC (tiny Gmin keeps isolated cap node well-conditioned)
                            m.stamp_conductance(nc, nb, gmin);
                        }
                        SolveMode::Transient { dt_s, trapezoidal } => {
                            if trapezoidal {
                                let g_eq = (2.0 * c.c_f) / dt_s;
                                let i_eq = g_eq * comp.state.v_prev + comp.state.i_prev;
                                m.stamp_conductance(nc, nb, g_eq);
                                // Companion current source from B -> A so I_A->B = g_eq * v - i_eq
                                m.stamp_current_source(nb, nc, i_eq);
                            } else {
                                let g_eq = c.c_f / dt_s;
                                let i_eq = g_eq * comp.state.v_prev;
                                m.stamp_conductance(nc, nb, g_eq);
                                m.stamp_current_source(nb, nc, i_eq);
                            }
                        }
                    }
                }
                ComponentSpec::Inductor(l) => {
                    let na = self.local_node(isl, &comp.id, "A");
                    let nb = self.local_node(isl, &comp.id, "B");
                    if let Some(&br) = isl.branch_vars.get(&(c_idx, 0)) {
                        stamp_inductor_branch(
                            &mut m,
                            na,
                            nb,
                            br,
                            l.l_h,
                            l.r_ser_ohm,
                            comp.state.i_prev,
                            comp.state.vl_prev,
                            mode,
                        );
                    }
                }
                ComponentSpec::Transformer(tr) => {
                    let np1 = self.local_node(isl, &comp.id, "P1");
                    let np2 = self.local_node(isl, &comp.id, "P2");
                    let ns1 = self.local_node(isl, &comp.id, "S1");
                    let ns2 = self.local_node(isl, &comp.id, "S2");
                    if let (Some(&b1), Some(&b2)) = (
                        isl.branch_vars.get(&(c_idx, 0)),
                        isl.branch_vars.get(&(c_idx, 1)),
                    ) {
                        let m_ind =
                            tr.k_coupling.clamp(0.0, 0.9999) * libm::sqrt(tr.l1_h * tr.l2_h);
                        // Branch incidence KCL & KVL
                        stamp_branch_incidence(&mut m, np1, np2, b1);
                        stamp_branch_incidence(&mut m, ns1, ns2, b2);
                        match mode {
                            SolveMode::DcOp => {
                                m.add(b1, b1, -tr.r1_ohm);
                                m.add(b2, b2, -tr.r2_ohm);
                            }
                            SolveMode::Transient { dt_s, trapezoidal } => {
                                let factor = if trapezoidal { 2.0 / dt_s } else { 1.0 / dt_s };
                                let z11 = factor * tr.l1_h;
                                let z22 = factor * tr.l2_h;
                                let z12 = factor * m_ind;
                                m.add(b1, b1, -(z11 + tr.r1_ohm));
                                m.add(b1, b2, -z12);
                                m.add(b2, b1, -z12);
                                m.add(b2, b2, -(z22 + tr.r2_ohm));
                                let rhs1 = -(z11 * comp.state.i_prev + z12 * comp.state.i2_prev)
                                    - if trapezoidal { comp.state.vl_prev } else { 0.0 };
                                let rhs2 = -(z12 * comp.state.i_prev + z22 * comp.state.i2_prev)
                                    - if trapezoidal {
                                        comp.state.vl2_prev
                                    } else {
                                        0.0
                                    };
                                m.add_rhs(b1, rhs1);
                                m.add_rhs(b2, rhs2);
                            }
                        }
                    }
                }
                ComponentSpec::Bjt(b) => {
                    let nb = self.local_node(isl, &comp.id, "B");
                    let nc = self.local_node(isl, &comp.id, "C");
                    let ne = self.local_node(isl, &comp.id, "E");
                    let sign = match b.polarity {
                        BjtPolarity::Npn => 1.0,
                        BjtPolarity::Pnp => -1.0,
                    };
                    let v_be = sign * (read_v(nb) - read_v(ne));
                    let v_bc = sign * (read_v(nb) - read_v(nc));
                    let st = eval_ebers_moll(b, v_be, v_bc, gmin);

                    // B-E diode branch (direction flips cleanly for PNP)
                    m.stamp_conductance(nb, ne, st.g_be);
                    m.stamp_current_source(nb, ne, sign * st.i_be_eq);
                    // B-C diode branch
                    m.stamp_conductance(nb, nc, st.g_bc);
                    m.stamp_current_source(nb, nc, sign * st.i_bc_eq);
                    // Collector-emitter transport current I_ct from C -> E (scaled by sign;
                    // since v_be = sign*(Vb-Ve), sign * gm * (sign*(Vb-Ve)) = gm * (Vb-Ve)!)
                    m.stamp_vccs(nc, ne, nb, ne, st.gm_f);
                    m.stamp_vccs(nc, ne, nb, nc, st.gm_r);
                    m.stamp_current_source(nc, ne, sign * st.i_ct_eq);
                }
                ComponentSpec::Mosfet(mos) => {
                    let ng = self.local_node(isl, &comp.id, "G");
                    let nd = self.local_node(isl, &comp.id, "D");
                    let ns = self.local_node(isl, &comp.id, "S");
                    let sign = match mos.polarity {
                        MosfetPolarity::Nmos => 1.0,
                        MosfetPolarity::Pmos => -1.0,
                    };
                    let raw_vds = sign * (read_v(nd) - read_v(ns));
                    let (eff_d, eff_s, v_ds) = if raw_vds >= 0.0 {
                        (nd, ns, raw_vds)
                    } else {
                        (ns, nd, -raw_vds)
                    };
                    let v_gs = sign * (read_v(ng) - read_v(eff_s));
                    let st = eval_shichman_hodges(mos, v_gs, v_ds, gmin);

                    // Channel conductance & transconductance from eff_d -> eff_s
                    m.stamp_conductance(eff_d, eff_s, st.g_ds);
                    m.stamp_vccs(eff_d, eff_s, ng, eff_s, st.g_m);
                    m.stamp_current_source(eff_d, eff_s, sign * st.i_eq);

                    // Optional integral body diode (S -> D for NMOS, D -> S for PMOS)
                    if mos.has_body_diode {
                        let (ba, bk) = match mos.polarity {
                            MosfetPolarity::Nmos => (ns, nd),
                            MosfetPolarity::Pmos => (nd, ns),
                        };
                        let bd = DiodeParams {
                            i_s: 1e-13,
                            n: 1.5,
                            r_s: 0.0,
                            v_br: 0.0,
                            i_bv: 1e-4,
                            n_br: 1.0,
                            i_max_a: 100.0,
                            is_led: false,
                            i_nom_a: 1.0,
                            color: None,
                        };
                        let v_bd = read_v(ba) - read_v(bk);
                        let bd_st = eval_diode_junction(&bd, v_bd, gmin);
                        m.stamp_conductance(ba, bk, bd_st.conductance_s);
                        m.stamp_current_source(ba, bk, bd_st.i_eq_a);
                    }
                }
                ComponentSpec::Optocoupler(opt) => {
                    if side == 0 {
                        // Input LED side (Island A)
                        let na = self.local_node(isl, &comp.id, "ANODE");
                        let nk = self.local_node(isl, &comp.id, "CATHODE");
                        let nj = if opt.led.r_s > 0.0 {
                            let nint = isl.internal_nodes.get(&(c_idx, 0)).copied();
                            m.stamp_conductance(na, nint, 1.0 / opt.led.r_s.max(1e-9));
                            nint
                        } else {
                            na
                        };
                        let vj = read_v(nj) - read_v(nk);
                        let st = eval_diode_junction(&opt.led, vj, gmin);
                        m.stamp_conductance(nj, nk, st.conductance_s);
                        m.stamp_current_source(nj, nk, st.i_eq_a);
                    } else {
                        // Galvanically isolated output phototransistor side (Island B)
                        let nc = self.local_node(isl, &comp.id, "COLLECTOR");
                        let ne = self.local_node(isl, &comp.id, "EMITTER");
                        let g_dark = 1.0 / opt.r_off_ohm.max(1e3);
                        m.stamp_conductance(nc, ne, g_dark);

                        let i_target = (opt.ctr * comp.state.opto_led_current_a).max(0.0);
                        if i_target > 1e-12 {
                            let v_ce = read_v(nc) - read_v(ne);
                            let v_sat = opt.v_ce_sat.max(0.05);
                            if v_ce >= 0.0 {
                                let u = (v_ce / v_sat).min(20.0);
                                let th = libm::tanh(u);
                                let sech2 = 1.0 - th * th;
                                let i_c = i_target * th;
                                let g_c = (i_target / v_sat) * sech2;
                                let i_eq = i_c - g_c * v_ce;
                                m.stamp_conductance(nc, ne, g_c);
                                m.stamp_current_source(nc, ne, i_eq);
                            }
                        }
                    }
                }
                ComponentSpec::Igbt(igbt) => {
                    let ng = self.local_node(isl, &comp.id, "G");
                    let nc = self.local_node(isl, &comp.id, "C");
                    let ne = self.local_node(isl, &comp.id, "E");
                    let v_ge = read_v(ng) - read_v(ne);
                    let v_ce = read_v(nc) - read_v(ne);
                    if v_ge >= igbt.v_ge_th && v_ce > igbt.v_ce_sat {
                        // Piecewise-linear bipolar saturation drop V_ce_sat + R_on
                        let g = 1.0 / igbt.r_on_ohm.max(1e-6);
                        m.stamp_conductance(nc, ne, g);
                        // I_ce = g * (v_ce - v_ce_sat) -> offset current source from E -> C = g * v_ce_sat
                        m.stamp_current_source(ne, nc, g * igbt.v_ce_sat);
                    } else {
                        m.stamp_conductance(nc, ne, 1.0 / igbt.r_off_ohm.max(1e3));
                    }
                }
                ComponentSpec::Scr(scr) => {
                    let na = self.local_node(isl, &comp.id, "A");
                    let nk = self.local_node(isl, &comp.id, "K");
                    let ng = self.local_node(isl, &comp.id, "G");
                    m.stamp_conductance(ng, nk, 1.0 / scr.r_gate_ohm.max(1.0));
                    let v_ak = read_v(na) - read_v(nk);
                    if comp.state.latched && v_ak > scr.v_on {
                        let g = 1.0 / scr.r_on_ohm.max(1e-6);
                        m.stamp_conductance(na, nk, g);
                        m.stamp_current_source(nk, na, g * scr.v_on);
                    } else {
                        m.stamp_conductance(na, nk, 1.0 / scr.r_off_ohm.max(1e3));
                    }
                }
                ComponentSpec::Triac(tr) => {
                    let n1 = self.local_node(isl, &comp.id, "MT1");
                    let n2 = self.local_node(isl, &comp.id, "MT2");
                    let ng = self.local_node(isl, &comp.id, "G");
                    m.stamp_conductance(ng, n1, 1.0 / tr.r_gate_ohm.max(1.0));
                    let v21 = read_v(n2) - read_v(n1);
                    if comp.state.latched && libm::fabs(v21) > tr.v_on {
                        let g = 1.0 / tr.r_on_ohm.max(1e-6);
                        let sign = if v21 >= 0.0 { 1.0 } else { -1.0 };
                        m.stamp_conductance(n2, n1, g);
                        m.stamp_current_source(n1, n2, sign * g * tr.v_on);
                    } else {
                        m.stamp_conductance(n2, n1, 1.0 / tr.r_off_ohm.max(1e3));
                    }
                }
                ComponentSpec::Ssr(ssr) => {
                    if side == 0 {
                        let np = self.local_node(isl, &comp.id, "CTRL_P");
                        let nn = self.local_node(isl, &comp.id, "CTRL_N");
                        m.stamp_conductance(np, nn, 1.0 / ssr.r_ctrl_ohm.max(1.0));
                    } else {
                        let na = self.local_node(isl, &comp.id, "LOAD_A");
                        let nb = self.local_node(isl, &comp.id, "LOAD_B");
                        let r = if comp.state.conducting {
                            ssr.r_on_ohm
                        } else {
                            ssr.r_off_ohm
                        };
                        m.stamp_conductance(na, nb, 1.0 / r.max(1e-6));
                    }
                }
                ComponentSpec::HBridge(hb) => {
                    let nvcc = self.local_node(isl, &comp.id, "VCC");
                    let ngnd = self.local_node(isl, &comp.id, "GND");
                    let nin1 = self.local_node(isl, &comp.id, "IN1");
                    let nin2 = self.local_node(isl, &comp.id, "IN2");
                    let nen = self.local_node(isl, &comp.id, "EN");
                    let noa = self.local_node(isl, &comp.id, "OUTA");
                    let nob = self.local_node(isl, &comp.id, "OUTB");

                    let v_gnd = read_v(ngnd);
                    let en = (read_v(nen) - v_gnd) >= hb.v_logic_th;
                    let in1 = (read_v(nin1) - v_gnd) >= hb.v_logic_th;
                    let in2 = (read_v(nin2) - v_gnd) >= hb.v_logic_th;
                    let g_on = 1.0 / hb.r_ds_on_ohm.max(1e-6);
                    let g_off = 1.0 / hb.r_off_ohm.max(1e3);

                    // High-side A, Low-side A, High-side B, Low-side B
                    let (ga_hi, ga_lo, gb_hi, gb_lo) = if !en {
                        (g_off, g_off, g_off, g_off)
                    } else {
                        match (in1, in2) {
                            (true, false) => (g_on, g_off, g_off, g_on),  // Forward
                            (false, true) => (g_off, g_on, g_on, g_off),  // Reverse
                            (true, true) => (g_on, g_off, g_on, g_off),   // High-side brake
                            (false, false) => (g_off, g_on, g_off, g_on), // Low-side brake
                        }
                    };
                    m.stamp_conductance(nvcc, noa, ga_hi);
                    m.stamp_conductance(noa, ngnd, ga_lo);
                    m.stamp_conductance(nvcc, nob, gb_hi);
                    m.stamp_conductance(nob, ngnd, gb_lo);
                }
                ComponentSpec::DcMotor(mot) => {
                    let na = self.local_node(isl, &comp.id, "A");
                    let nb = self.local_node(isl, &comp.id, "B");
                    if let Some(&br) = isl.branch_vars.get(&(c_idx, 0)) {
                        stamp_branch_incidence(&mut m, na, nb, br);
                        match mode {
                            SolveMode::DcOp => {
                                // Exact DC steady-state back-EMF resistance: R_a + K_e * K_t / B
                                let r_emf = if mot.friction_b > 0.0 {
                                    (mot.k_e * mot.k_t) / mot.friction_b
                                } else {
                                    0.0
                                };
                                m.add(br, br, -(mot.r_arm_ohm + r_emf));
                            }
                            SolveMode::Transient { dt_s, .. } => {
                                // Coupled implicit Backward Euler electromechanical step:
                                // omega_n = ( (J/dt)*omega_prev + K_t*i_n ) / (J/dt + B)
                                let j_dt = mot.inertia_j / dt_s;
                                let denom = (j_dt + mot.friction_b).max(1e-12);
                                let l_dt = mot.l_arm_h / dt_s;
                                let r_eff = mot.r_arm_ohm + l_dt + (mot.k_e * mot.k_t) / denom;
                                let rhs = -l_dt * comp.state.i_prev
                                    + (mot.k_e * j_dt / denom) * comp.state.omega_rad_s;
                                m.add(br, br, -r_eff);
                                m.add_rhs(br, rhs);
                            }
                        }
                    }
                }
                ComponentSpec::BreadboardStrip { .. } | ComponentSpec::Ground => {}
            }
        }

        m
    }

    /// Evaluates cross-island coupling & latching states (`Optocoupler`, `Relay`, `Ssr`, `Scr`, `Triac`).
    /// Returns `true` if any discrete/coupled state changed significantly requiring another pass.
    fn update_switching_and_coupling_states(&mut self) -> bool {
        let mut changed = false;
        let v_of = |net_voltages: &BTreeMap<usize, f64>,
                    ground_nets: &BTreeSet<usize>,
                    t2n: &BTreeMap<TerminalKey, usize>,
                    id: &str,
                    port: &str|
         -> f64 {
            let Some(&net) = t2n.get(&(id.to_owned(), port.to_owned())) else {
                return 0.0;
            };
            if ground_nets.contains(&net) {
                0.0
            } else {
                net_voltages.get(&net).copied().unwrap_or(0.0)
            }
        };

        for (c_idx, comp) in self.components.iter_mut().enumerate() {
            match &comp.spec {
                ComponentSpec::Optocoupler(opt) => {
                    let va = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "ANODE",
                    );
                    let vk = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "CATHODE",
                    );
                    let i_led = if opt.led.r_s > 0.0 {
                        // Read internal node voltage from the primary island
                        let v_int = find_internal_voltage(&self.islands, c_idx, 0).unwrap_or(va);
                        (va - v_int) / opt.led.r_s
                    } else {
                        eval_diode_junction(&opt.led, va - vk, self.config.gmin).current_a
                    }
                    .max(0.0);
                    if libm::fabs(i_led - comp.state.opto_led_current_a) > 1e-7 {
                        comp.state.opto_led_current_a = i_led;
                        changed = true;
                    }
                }
                ComponentSpec::Relay(rel) => {
                    let va = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "COIL_A",
                    );
                    let vb = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "COIL_B",
                    );
                    let v_abs = libm::fabs(va - vb);
                    let next = if comp.state.energized {
                        v_abs > rel.v_drop_out_v
                    } else {
                        v_abs >= rel.v_pull_in_v
                    };
                    if next != comp.state.energized {
                        comp.state.energized = next;
                        changed = true;
                    }
                }
                ComponentSpec::Ssr(ssr) => {
                    let vp = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "CTRL_P",
                    );
                    let vn = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "CTRL_N",
                    );
                    let v_ctrl = vp - vn;
                    let next = if comp.state.conducting {
                        v_ctrl > ssr.v_ctrl_off
                    } else {
                        v_ctrl >= ssr.v_ctrl_on
                    };
                    if next != comp.state.conducting {
                        comp.state.conducting = next;
                        changed = true;
                    }
                }
                ComponentSpec::Scr(scr) => {
                    let va = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "A",
                    );
                    let vk = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "K",
                    );
                    let vg = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "G",
                    );
                    let v_ak = va - vk;
                    let v_gk = vg - vk;
                    let next = if !comp.state.latched {
                        v_gk >= scr.v_gt && v_ak > scr.v_on
                    } else {
                        let i_ak = if v_ak > scr.v_on {
                            (v_ak - scr.v_on) / scr.r_on_ohm.max(1e-6)
                        } else {
                            0.0
                        };
                        i_ak >= scr.i_hold
                    };
                    if next != comp.state.latched {
                        comp.state.latched = next;
                        changed = true;
                    }
                }
                ComponentSpec::Triac(tr) => {
                    let v1 = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "MT1",
                    );
                    let v2 = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "MT2",
                    );
                    let vg = v_of(
                        &self.net_voltages,
                        &self.ground_nets,
                        &self.terminal_to_net,
                        &comp.id,
                        "G",
                    );
                    let v21 = v2 - v1;
                    let vg1 = vg - v1;
                    let next = if !comp.state.latched {
                        libm::fabs(vg1) >= tr.v_gt && libm::fabs(v21) > tr.v_on
                    } else {
                        let excess = (libm::fabs(v21) - tr.v_on).max(0.0);
                        let i_mt = excess / tr.r_on_ohm.max(1e-6);
                        i_mt >= tr.i_hold
                    };
                    if next != comp.state.latched {
                        comp.state.latched = next;
                        changed = true;
                    }
                }
                ComponentSpec::Resistor(_)
                | ComponentSpec::VoltageSourceDc(_)
                | ComponentSpec::CurrentSourceDc(_)
                | ComponentSpec::Potentiometer(_)
                | ComponentSpec::Switch(_)
                | ComponentSpec::Diode(_)
                | ComponentSpec::Capacitor(_)
                | ComponentSpec::Inductor(_)
                | ComponentSpec::Transformer(_)
                | ComponentSpec::VoltageSourceAc(_)
                | ComponentSpec::Bjt(_)
                | ComponentSpec::Mosfet(_)
                | ComponentSpec::Igbt(_)
                | ComponentSpec::HBridge(_)
                | ComponentSpec::DcMotor(_)
                | ComponentSpec::BreadboardStrip { .. }
                | ComponentSpec::Ground => {}
            }
        }
        changed
    }

    /// Commits reactive companion histories, computes component currents/luminosities, and emits faults.
    fn commit_post_solve_states(&mut self, mode: SolveMode, faults: &mut Vec<ElectricalFault>) {
        let now_us = self.time_us;
        let gmin = self.config.gmin;
        let i_short = self.config.short_circuit_current_a;

        let read_term = |t2n: &BTreeMap<TerminalKey, usize>,
                         gnd: &BTreeSet<usize>,
                         nv: &BTreeMap<usize, f64>,
                         id: &str,
                         port: &str|
         -> f64 {
            let Some(&net) = t2n.get(&(id.to_owned(), port.to_owned())) else {
                return 0.0;
            };
            if gnd.contains(&net) {
                0.0
            } else {
                nv.get(&net).copied().unwrap_or(0.0)
            }
        };

        for (c_idx, comp) in self.components.iter_mut().enumerate() {
            match &comp.spec {
                ComponentSpec::Resistor(r) => {
                    let va = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "A",
                    );
                    let vb = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "B",
                    );
                    let dv = va - vb;
                    let i = dv / r.r_ohm.max(1e-9);
                    let p = dv * i;
                    comp.state.last_current_a = i;
                    comp.state.last_power_w = p;
                    if r.p_rated_w > 0.0 && p > r.p_rated_w {
                        comp.state.burned = true;
                        faults.push(ElectricalFault {
                            time_us: now_us,
                            instance_id: comp.id.clone(),
                            kind: FaultKind::ResistorOverpower {
                                power_w: p,
                                p_rated_w: r.p_rated_w,
                            },
                        });
                    }
                }
                ComponentSpec::VoltageSourceDc(vs) => {
                    let i = if vs.r_ser_ohm > 0.0 {
                        let vp = read_term(
                            &self.terminal_to_net,
                            &self.ground_nets,
                            &self.net_voltages,
                            &comp.id,
                            "POS",
                        );
                        let vn = read_term(
                            &self.terminal_to_net,
                            &self.ground_nets,
                            &self.net_voltages,
                            &comp.id,
                            "NEG",
                        );
                        (vs.voltage_v - (vp - vn)) / vs.r_ser_ohm
                    } else {
                        -find_branch_current(&self.islands, c_idx, 0).unwrap_or(0.0)
                    };
                    comp.state.last_current_a = i;
                    if libm::fabs(i) > i_short {
                        faults.push(ElectricalFault {
                            time_us: now_us,
                            instance_id: comp.id.clone(),
                            kind: FaultKind::ShortCircuit {
                                current_a: libm::fabs(i),
                                limit_a: i_short,
                            },
                        });
                    }
                }
                ComponentSpec::VoltageSourceAc(_) => {
                    let i = -find_branch_current(&self.islands, c_idx, 0).unwrap_or(0.0);
                    comp.state.last_current_a = i;
                    if libm::fabs(i) > i_short {
                        faults.push(ElectricalFault {
                            time_us: now_us,
                            instance_id: comp.id.clone(),
                            kind: FaultKind::ShortCircuit {
                                current_a: libm::fabs(i),
                                limit_a: i_short,
                            },
                        });
                    }
                }
                ComponentSpec::Diode(d) => {
                    let va = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "A",
                    );
                    let vk = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "K",
                    );
                    let vd = va - vk;
                    let i_d = if d.r_s > 0.0 {
                        let v_int = find_internal_voltage(&self.islands, c_idx, 0).unwrap_or(va);
                        (va - v_int) / d.r_s
                    } else {
                        eval_diode_junction(d, vd, gmin).current_a
                    };
                    comp.state.last_current_a = i_d;
                    comp.state.last_power_w = vd * i_d;

                    if d.is_led {
                        if i_d > d.i_max_a {
                            comp.state.burned = true;
                            faults.push(ElectricalFault {
                                time_us: now_us,
                                instance_id: comp.id.clone(),
                                kind: FaultKind::LedBurned {
                                    forward_current_a: i_d,
                                    i_max_a: d.i_max_a,
                                },
                            });
                        } else if d.v_br > 0.0 && -vd >= d.v_br && -i_d >= 1e-4 {
                            comp.state.burned = true;
                            faults.push(ElectricalFault {
                                time_us: now_us,
                                instance_id: comp.id.clone(),
                                kind: FaultKind::LedReverseBreakdown {
                                    reverse_voltage_v: -vd,
                                    v_br_v: d.v_br,
                                },
                            });
                        }
                        comp.state.luminosity = if comp.state.burned {
                            0.0
                        } else {
                            (i_d / d.i_nom_a.max(1e-6)).clamp(0.0, 1.0)
                        };
                    }
                }
                ComponentSpec::Capacitor(c) => {
                    let va = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "A",
                    );
                    let vb = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "B",
                    );
                    let vc_a = if c.esr_ohm > 0.0 {
                        find_internal_voltage(&self.islands, c_idx, 0).unwrap_or(va)
                    } else {
                        va
                    };
                    let v_new = vc_a - vb;
                    let i_new = match mode {
                        SolveMode::DcOp => 0.0,
                        SolveMode::Transient { dt_s, trapezoidal } => {
                            if trapezoidal {
                                (2.0 * c.c_f / dt_s) * (v_new - comp.state.v_prev)
                                    - comp.state.i_prev
                            } else {
                                (c.c_f / dt_s) * (v_new - comp.state.v_prev)
                            }
                        }
                    };
                    comp.state.v_prev = v_new;
                    comp.state.i_prev = i_new;
                    comp.state.last_current_a = i_new;
                }
                ComponentSpec::Inductor(l) => {
                    let va = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "A",
                    );
                    let vb = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "B",
                    );
                    let i_new = find_branch_current(&self.islands, c_idx, 0).unwrap_or(0.0);
                    let vl_new = (va - vb) - l.r_ser_ohm * i_new;
                    comp.state.i_prev = i_new;
                    comp.state.vl_prev = vl_new;
                    comp.state.last_current_a = i_new;
                }
                ComponentSpec::Relay(rel) => {
                    let va = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "COIL_A",
                    );
                    let vb = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "COIL_B",
                    );
                    let i_new = if rel.l_coil_h > 0.0 {
                        find_branch_current(&self.islands, c_idx, 0).unwrap_or(0.0)
                    } else {
                        (va - vb) / rel.r_coil_ohm.max(1e-6)
                    };
                    comp.state.i_prev = i_new;
                    comp.state.vl_prev = (va - vb) - rel.r_coil_ohm * i_new;
                    comp.state.last_current_a = i_new;
                }
                ComponentSpec::Transformer(tr) => {
                    let vp = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "P1",
                    ) - read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "P2",
                    );
                    let vs = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "S1",
                    ) - read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "S2",
                    );
                    let i1 = find_branch_current(&self.islands, c_idx, 0).unwrap_or(0.0);
                    let i2 = find_branch_current(&self.islands, c_idx, 1).unwrap_or(0.0);
                    comp.state.i_prev = i1;
                    comp.state.i2_prev = i2;
                    comp.state.vl_prev = vp - tr.r1_ohm * i1;
                    comp.state.vl2_prev = vs - tr.r2_ohm * i2;
                    comp.state.last_current_a = i1;
                }
                ComponentSpec::DcMotor(mot) => {
                    let i_new = find_branch_current(&self.islands, c_idx, 0).unwrap_or(0.0);
                    let omega_new = match mode {
                        SolveMode::DcOp => {
                            if mot.friction_b > 0.0 {
                                (mot.k_t / mot.friction_b) * i_new
                            } else {
                                0.0
                            }
                        }
                        SolveMode::Transient { dt_s, .. } => {
                            let j_dt = mot.inertia_j / dt_s;
                            let denom = (j_dt + mot.friction_b).max(1e-12);
                            (j_dt * comp.state.omega_rad_s + mot.k_t * i_new) / denom
                        }
                    };
                    comp.state.i_prev = i_new;
                    comp.state.omega_rad_s = omega_new;
                    comp.state.last_current_a = i_new;
                }
                ComponentSpec::Bjt(b) => {
                    let vb = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "B",
                    );
                    let vc = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "C",
                    );
                    let ve = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "E",
                    );
                    let sign = match b.polarity {
                        BjtPolarity::Npn => 1.0,
                        BjtPolarity::Pnp => -1.0,
                    };
                    let st = eval_ebers_moll(b, sign * (vb - ve), sign * (vb - vc), gmin);
                    comp.state.last_current_a = st.i_c;
                }
                ComponentSpec::Mosfet(mos) => {
                    let vg = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "G",
                    );
                    let vd = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "D",
                    );
                    let vs = read_term(
                        &self.terminal_to_net,
                        &self.ground_nets,
                        &self.net_voltages,
                        &comp.id,
                        "S",
                    );
                    let sign = match mos.polarity {
                        MosfetPolarity::Nmos => 1.0,
                        MosfetPolarity::Pmos => -1.0,
                    };
                    let raw_vds = sign * (vd - vs);
                    let (v_gs, v_ds, dir) = if raw_vds >= 0.0 {
                        (sign * (vg - vs), raw_vds, 1.0)
                    } else {
                        (sign * (vg - vd), -raw_vds, -1.0)
                    };
                    let st = eval_shichman_hodges(mos, v_gs, v_ds, gmin);
                    comp.state.last_current_a = sign * dir * st.i_d;
                }
                ComponentSpec::CurrentSourceDc(cs) => {
                    comp.state.last_current_a = cs.current_a;
                }
                ComponentSpec::Potentiometer(_)
                | ComponentSpec::Switch(_)
                | ComponentSpec::Optocoupler(_)
                | ComponentSpec::Igbt(_)
                | ComponentSpec::Scr(_)
                | ComponentSpec::Triac(_)
                | ComponentSpec::Ssr(_)
                | ComponentSpec::HBridge(_)
                | ComponentSpec::BreadboardStrip { .. }
                | ComponentSpec::Ground => {}
            }
        }
    }

    fn record_probes(&mut self) {
        let now = self.time_us;
        let probe_names: Vec<String> = self.probes.keys().cloned().collect();
        for name in probe_names {
            let Some(p) = self.probes.get(&name) else {
                continue;
            };
            let vp = self.terminal_voltage(&p.pos.0, &p.pos.1);
            let vn = match &p.neg {
                Some((i, pt)) => self.terminal_voltage(i, pt),
                None => 0.0,
            };
            let v = vp - vn;
            if let Some(probe) = self.probes.get_mut(&name) {
                if probe.samples.len() >= probe.max_samples {
                    probe.samples.remove(0);
                }
                probe.samples.push(ProbeSample {
                    time_us: now,
                    voltage_v: v,
                });
            }
        }
    }
}

// ===========================================================================
// Helper Functions
// ===========================================================================

fn stamp_branch_incidence(m: &mut MnaMatrix, pos: Option<usize>, neg: Option<usize>, br: usize) {
    if let Some(p) = pos {
        m.add(p, br, 1.0);
        m.add(br, p, 1.0);
    }
    if let Some(n) = neg {
        m.add(n, br, -1.0);
        m.add(br, n, -1.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn stamp_inductor_branch(
    m: &mut MnaMatrix,
    na: Option<usize>,
    nb: Option<usize>,
    br: usize,
    l_h: f64,
    r_ser: f64,
    i_prev: f64,
    vl_prev: f64,
    mode: SolveMode,
) {
    stamp_branch_incidence(m, na, nb, br);
    match mode {
        SolveMode::DcOp => {
            m.add(br, br, -r_ser);
        }
        SolveMode::Transient { dt_s, trapezoidal } => {
            if trapezoidal {
                let r_eq = (2.0 * l_h) / dt_s;
                m.add(br, br, -(r_eq + r_ser));
                m.add_rhs(br, -r_eq * i_prev - vl_prev);
            } else {
                let r_eq = l_h / dt_s;
                m.add(br, br, -(r_eq + r_ser));
                m.add_rhs(br, -r_eq * i_prev);
            }
        }
    }
}

fn limit_two_node_junction(
    d: &DiodeParams,
    na: Option<usize>,
    nk: Option<usize>,
    x_old: &[f64],
    x_new: &mut [f64],
) -> bool {
    let read_v = |x: &[f64], node: Option<usize>| -> f64 {
        node.and_then(|i| x.get(i).copied()).unwrap_or(0.0)
    };
    let n_vt = (d.n * VT_300K).max(1e-4);
    let vcrit = junction_vcrit(n_vt, d.i_s);
    let v_old = read_v(x_old, na) - read_v(x_old, nk);
    let v_new = read_v(x_new, na) - read_v(x_new, nk);

    // Forward pnjlim
    let mut v_lim = pnjlim(v_new, v_old, n_vt, vcrit);

    // Reverse Zener breakdown limiting around -v_br
    if d.v_br > 0.0 && d.v_br.is_finite() {
        let vt_br = (d.n_br * VT_300K).max(1e-4);
        let rev_old = -v_old - d.v_br;
        let rev_new = -v_new - d.v_br;
        if rev_new > 2.0 * vt_br && libm::fabs(rev_new - rev_old) > 2.0 * vt_br {
            let rev_lim = if rev_old > 0.0 {
                let arg = 1.0 + (rev_new - rev_old) / vt_br;
                if arg > 0.0 {
                    rev_old + vt_br * libm::log(arg)
                } else {
                    2.0 * vt_br
                }
            } else {
                vt_br * libm::log((rev_new / vt_br).max(1e-12))
            };
            v_lim = -(d.v_br + rev_lim);
        }
    }

    if libm::fabs(v_lim - v_new) > 1e-12 {
        let delta = v_lim - v_new;
        if let Some(a_idx) = na {
            if let Some(slot) = x_new.get_mut(a_idx) {
                *slot += delta;
                return true;
            }
        } else if let Some(k_idx) = nk
            && let Some(slot) = x_new.get_mut(k_idx)
        {
            *slot -= delta;
            return true;
        }
    }
    false
}

fn find_internal_voltage(islands: &[IslandPlan], c_idx: usize, slot: u8) -> Option<f64> {
    for isl in islands {
        if let Some(&u_idx) = isl.internal_nodes.get(&(c_idx, slot)) {
            return isl.last_x.get(u_idx).copied();
        }
    }
    None
}

fn find_branch_current(islands: &[IslandPlan], c_idx: usize, slot: u8) -> Option<f64> {
    for isl in islands {
        if let Some(&u_idx) = isl.branch_vars.get(&(c_idx, slot)) {
            return isl.last_x.get(u_idx).copied();
        }
    }
    None
}

fn validate_component_spec(id: &str, spec: &ComponentSpec) -> Result<(), CircuitError> {
    let err = |msg: &str| CircuitError::InvalidParameter {
        component: id.to_owned(),
        message: msg.to_owned(),
    };
    match spec {
        ComponentSpec::Resistor(r) if !r.r_ohm.is_finite() || r.r_ohm <= 0.0 => {
            Err(err("resistance must be finite and > 0"))
        }
        ComponentSpec::Capacitor(c) if !c.c_f.is_finite() || c.c_f <= 0.0 => {
            Err(err("capacitance must be finite and > 0"))
        }
        ComponentSpec::Inductor(l) if !l.l_h.is_finite() || l.l_h <= 0.0 => {
            Err(err("inductance must be finite and > 0"))
        }
        ComponentSpec::Transformer(t)
            if t.l1_h <= 0.0 || t.l2_h <= 0.0 || !(0.0..1.0).contains(&t.k_coupling) =>
        {
            Err(err("transformer L1/L2 must be > 0 and 0 <= k < 1"))
        }
        ComponentSpec::Potentiometer(p) if !p.r_total_ohm.is_finite() || p.r_total_ohm <= 0.0 => {
            Err(err("potentiometer resistance must be finite and > 0"))
        }
        ComponentSpec::Resistor(_)
        | ComponentSpec::VoltageSourceDc(_)
        | ComponentSpec::CurrentSourceDc(_)
        | ComponentSpec::Potentiometer(_)
        | ComponentSpec::Switch(_)
        | ComponentSpec::Relay(_)
        | ComponentSpec::Diode(_)
        | ComponentSpec::Capacitor(_)
        | ComponentSpec::Inductor(_)
        | ComponentSpec::Transformer(_)
        | ComponentSpec::VoltageSourceAc(_)
        | ComponentSpec::Bjt(_)
        | ComponentSpec::Mosfet(_)
        | ComponentSpec::Optocoupler(_)
        | ComponentSpec::Igbt(_)
        | ComponentSpec::Scr(_)
        | ComponentSpec::Triac(_)
        | ComponentSpec::Ssr(_)
        | ComponentSpec::HBridge(_)
        | ComponentSpec::DcMotor(_)
        | ComponentSpec::BreadboardStrip { .. }
        | ComponentSpec::Ground => Ok(()),
    }
}

fn component_ports(spec: &ComponentSpec) -> Vec<String> {
    match spec {
        ComponentSpec::Resistor(_)
        | ComponentSpec::Switch(_)
        | ComponentSpec::Capacitor(_)
        | ComponentSpec::Inductor(_)
        | ComponentSpec::DcMotor(_) => vec!["A".to_owned(), "B".to_owned()],
        ComponentSpec::VoltageSourceDc(_) | ComponentSpec::VoltageSourceAc(_) => {
            vec!["POS".to_owned(), "NEG".to_owned()]
        }
        ComponentSpec::CurrentSourceDc(_) => vec!["IN".to_owned(), "OUT".to_owned()],
        ComponentSpec::Potentiometer(_) => vec!["A".to_owned(), "W".to_owned(), "B".to_owned()],
        ComponentSpec::Relay(_) => vec![
            "COIL_A".to_owned(),
            "COIL_B".to_owned(),
            "COM".to_owned(),
            "NO".to_owned(),
            "NC".to_owned(),
        ],
        ComponentSpec::Diode(_) => vec!["A".to_owned(), "K".to_owned()],
        ComponentSpec::Transformer(_) => vec![
            "P1".to_owned(),
            "P2".to_owned(),
            "S1".to_owned(),
            "S2".to_owned(),
        ],
        ComponentSpec::Bjt(_) => vec!["B".to_owned(), "C".to_owned(), "E".to_owned()],
        ComponentSpec::Mosfet(_) => vec!["G".to_owned(), "D".to_owned(), "S".to_owned()],
        ComponentSpec::Optocoupler(_) => vec![
            "ANODE".to_owned(),
            "CATHODE".to_owned(),
            "COLLECTOR".to_owned(),
            "EMITTER".to_owned(),
        ],
        ComponentSpec::Igbt(_) => vec!["G".to_owned(), "C".to_owned(), "E".to_owned()],
        ComponentSpec::Scr(_) => vec!["A".to_owned(), "K".to_owned(), "G".to_owned()],
        ComponentSpec::Triac(_) => vec!["MT1".to_owned(), "MT2".to_owned(), "G".to_owned()],
        ComponentSpec::Ssr(_) => vec![
            "CTRL_P".to_owned(),
            "CTRL_N".to_owned(),
            "LOAD_A".to_owned(),
            "LOAD_B".to_owned(),
        ],
        ComponentSpec::HBridge(_) => vec![
            "VCC".to_owned(),
            "GND".to_owned(),
            "IN1".to_owned(),
            "IN2".to_owned(),
            "EN".to_owned(),
            "OUTA".to_owned(),
            "OUTB".to_owned(),
        ],
        ComponentSpec::BreadboardStrip { ports } => ports.clone(),
        ComponentSpec::Ground => vec!["GND".to_owned()],
    }
}

/// Splits galvanically isolated components into `(side_idx, port_names)` groups for island partitioning.
fn galvanic_port_groups(spec: &ComponentSpec) -> Vec<(u8, Vec<&'static str>)> {
    match spec {
        ComponentSpec::Optocoupler(_) => vec![
            (0, vec!["ANODE", "CATHODE"]),
            (1, vec!["COLLECTOR", "EMITTER"]),
        ],
        ComponentSpec::Relay(_) => {
            vec![(0, vec!["COIL_A", "COIL_B"]), (1, vec!["COM", "NO", "NC"])]
        }
        ComponentSpec::Ssr(_) => vec![(0, vec!["CTRL_P", "CTRL_N"]), (1, vec!["LOAD_A", "LOAD_B"])],
        ComponentSpec::Resistor(_)
        | ComponentSpec::Switch(_)
        | ComponentSpec::Capacitor(_)
        | ComponentSpec::Inductor(_)
        | ComponentSpec::DcMotor(_) => vec![(0, vec!["A", "B"])],
        ComponentSpec::VoltageSourceDc(_) | ComponentSpec::VoltageSourceAc(_) => {
            vec![(0, vec!["POS", "NEG"])]
        }
        ComponentSpec::CurrentSourceDc(_) => vec![(0, vec!["IN", "OUT"])],
        ComponentSpec::Potentiometer(_) => vec![(0, vec!["A", "W", "B"])],
        ComponentSpec::Diode(_) => vec![(0, vec!["A", "K"])],
        ComponentSpec::Transformer(_) => vec![(0, vec!["P1", "P2", "S1", "S2"])],
        ComponentSpec::Bjt(_) => vec![(0, vec!["B", "C", "E"])],
        ComponentSpec::Mosfet(_) => vec![(0, vec!["G", "D", "S"])],
        ComponentSpec::Igbt(_) => vec![(0, vec!["G", "C", "E"])],
        ComponentSpec::Scr(_) => vec![(0, vec!["A", "K", "G"])],
        ComponentSpec::Triac(_) => vec![(0, vec!["MT1", "MT2", "G"])],
        ComponentSpec::HBridge(_) => {
            vec![(0, vec!["VCC", "GND", "IN1", "IN2", "EN", "OUTA", "OUTB"])]
        }
        ComponentSpec::BreadboardStrip { .. } | ComponentSpec::Ground => Vec::new(),
    }
}

/// Deterministic Union-Find with path compression and zero unchecked indexing.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        let mut root = x;
        while let Some(&p) = self.parent.get(root) {
            if p == root {
                break;
            }
            root = p;
        }
        while let Some(&p) = self.parent.get(x) {
            if p == root {
                break;
            }
            if let Some(slot) = self.parent.get_mut(x) {
                *slot = root;
            }
            x = p;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            if let Some(slot) = self.parent.get_mut(hi) {
                *slot = lo;
            }
        }
    }
}
