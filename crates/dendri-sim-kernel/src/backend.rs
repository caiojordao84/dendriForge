//! Pluggable electrical boundary trait and Phase 2 [`DigitalWireFabric`] implementation (`02` §1).
//!
//! In Phase 2, [`DigitalWireFabric`] connects board and component ports across `[[wires]]`
//! using deterministic Union-Find nets so heterogeneous multi-board simulations communicate
//! honestly over physical wiring. In Phase 3, `dendri-circuit` implements the exact same
//! [`ElectricalBackend`] trait with a full SPICE-class MNA solver.

use crate::KernelError;
use dendri_ast::{AnalogStatus, Endpoint, Project, PullMode};
use dendri_vm::{AnalogReading, ElectricalDigitalLevel};
use std::collections::BTreeMap;

/// Pluggable electrical backend driven by [`crate::SimKernel`].
pub trait ElectricalBackend: Send {
    /// Updates a board digital output pin driver.
    fn set_board_digital_out(&mut self, board_id: &str, pin: &str, high: bool);

    /// Updates a board analog output pin driver (`M = 10^9`).
    fn set_board_analog_out(&mut self, board_id: &str, pin: &str, analog: i32);

    /// Updates a board PWM output pin duty cycle (`0..=M`).
    fn set_board_pwm_out(&mut self, board_id: &str, pin: &str, duty: u32);

    /// Injects an external digital stimulus on `<instance>.<port>`.
    fn set_external_digital(&mut self, instance: &str, port: &str, level: ElectricalDigitalLevel);

    /// Releases a previously injected external digital stimulus on `<instance>.<port>`.
    fn clear_external_digital(&mut self, instance: &str, port: &str);

    /// Injects an external analog stimulus on `<instance>.<port>`.
    fn set_external_analog(&mut self, instance: &str, port: &str, reading: AnalogReading);

    /// Propagates electrical state / solves the circuit at `time_us`.
    fn solve_at(&mut self, time_us: u64);

    /// Reads the solved digital logic level seen by a board input pin.
    fn read_board_digital_in(&self, board_id: &str, pin: &str) -> ElectricalDigitalLevel;

    /// Reads the solved analog voltage/reading seen by a board analog input pin.
    fn read_board_analog_in(&self, board_id: &str, pin: &str) -> AnalogReading;

    /// Returns the next required electrical transient timestamp, if any (used by Phase 3 MNA).
    fn next_electrical_event_us(&self) -> Option<u64> {
        None
    }
}

/// Canonical `(instance_id, port_id)` electrical endpoint key.
type EndpointKey = (String, String);

/// Phase 2 deterministic digital/analog wire net fabric built via Union-Find over `[[wires]]`.
///
/// - Connected pins share a single net (`NetId`).
/// - Agreeing digital drivers resolve cleanly (`High` or `Low`).
/// - Conflicting simultaneous digital drivers (`High` vs `Low`) or an `Indeterminate`
///   driver resolve to [`ElectricalDigitalLevel::Indeterminate`].
/// - Undriven nets honor board pin `PullMode::PullUp` (`High`) / `PullMode::PullDown` (`Low`).
#[derive(Debug, Clone, Default)]
pub struct DigitalWireFabric {
    /// Maps each known endpoint `(instance, port)` to its canonical net index.
    endpoint_to_net: BTreeMap<EndpointKey, usize>,
    /// Per-net pull bias derived from declared board `PullMode`s.
    net_pull: BTreeMap<usize, PullMode>,
    /// Per-endpoint fallback pull bias for unwired board pins.
    endpoint_pull: BTreeMap<EndpointKey, PullMode>,
    /// Active board digital output drivers.
    board_digital_out: BTreeMap<EndpointKey, ElectricalDigitalLevel>,
    /// Active external digital stimulus drivers.
    ext_digital_out: BTreeMap<EndpointKey, ElectricalDigitalLevel>,
    /// Active board analog output drivers.
    board_analog_out: BTreeMap<EndpointKey, AnalogReading>,
    /// Active external analog stimulus drivers.
    ext_analog_out: BTreeMap<EndpointKey, AnalogReading>,
    /// Active board PWM output drivers (stored for inspection/Phase 3 upgrade).
    board_pwm_out: BTreeMap<EndpointKey, u32>,
    /// Solved digital level per canonical net after `solve_at`.
    solved_net_digital: BTreeMap<usize, ElectricalDigitalLevel>,
    /// Solved analog reading per canonical net after `solve_at`.
    solved_net_analog: BTreeMap<usize, AnalogReading>,
}

impl DigitalWireFabric {
    /// Builds deterministic electrical nets from all project `[[wires]]` and board pin pull modes.
    pub fn from_project(project: &Project) -> Result<Self, KernelError> {
        // Collect all endpoints in deterministic order (boards hardware pins + wire endpoints).
        let mut endpoint_ids: BTreeMap<EndpointKey, usize> = BTreeMap::new();
        let mut endpoint_pull: BTreeMap<EndpointKey, PullMode> = BTreeMap::new();

        for board in &project.boards {
            for hw in &board.program.hardware {
                let key = (board.id.clone(), hw.pin.clone());
                let next_id = endpoint_ids.len();
                endpoint_ids.entry(key.clone()).or_insert(next_id);
                if hw.pull != PullMode::None {
                    endpoint_pull.insert(key, hw.pull);
                }
            }
        }

        let mut parsed_edges: Vec<(usize, usize)> = Vec::with_capacity(project.wires.len());
        for wire in &project.wires {
            let from_ep =
                Endpoint::parse(&wire.from).map_err(|e| KernelError::MalformedWireEndpoint {
                    wire_id: wire.id.clone(),
                    endpoint: wire.from.clone(),
                    reason: e.to_string(),
                })?;
            let to_ep =
                Endpoint::parse(&wire.to).map_err(|e| KernelError::MalformedWireEndpoint {
                    wire_id: wire.id.clone(),
                    endpoint: wire.to.clone(),
                    reason: e.to_string(),
                })?;

            let from_key = (from_ep.instance_id, from_ep.port_id);
            let to_key = (to_ep.instance_id, to_ep.port_id);
            let next_a = endpoint_ids.len();
            let a = *endpoint_ids.entry(from_key).or_insert(next_a);
            let next_b = endpoint_ids.len();
            let b = *endpoint_ids.entry(to_key).or_insert(next_b);
            parsed_edges.push((a, b));
        }

        // Union-Find over endpoint indices.
        let mut uf = UnionFind::new(endpoint_ids.len());
        for (a, b) in parsed_edges {
            uf.union(a, b);
        }

        let mut endpoint_to_net = BTreeMap::new();
        for (key, idx) in endpoint_ids {
            endpoint_to_net.insert(key, uf.find(idx));
        }

        let mut net_pull = BTreeMap::new();
        for (key, &pull) in &endpoint_pull {
            if let Some(&net) = endpoint_to_net.get(key) {
                net_pull.entry(net).or_insert(pull);
            }
        }

        let mut fabric = Self {
            endpoint_to_net,
            net_pull,
            endpoint_pull,
            board_digital_out: BTreeMap::new(),
            ext_digital_out: BTreeMap::new(),
            board_analog_out: BTreeMap::new(),
            ext_analog_out: BTreeMap::new(),
            board_pwm_out: BTreeMap::new(),
            solved_net_digital: BTreeMap::new(),
            solved_net_analog: BTreeMap::new(),
        };
        fabric.solve_at(0);
        Ok(fabric)
    }

    fn resolve_net(&self, instance: &str, port: &str) -> Option<usize> {
        self.endpoint_to_net
            .get(&(instance.to_owned(), port.to_owned()))
            .copied()
    }
}

impl ElectricalBackend for DigitalWireFabric {
    fn set_board_digital_out(&mut self, board_id: &str, pin: &str, high: bool) {
        let level = if high {
            ElectricalDigitalLevel::High
        } else {
            ElectricalDigitalLevel::Low
        };
        self.board_digital_out
            .insert((board_id.to_owned(), pin.to_owned()), level);
    }

    fn set_board_analog_out(&mut self, board_id: &str, pin: &str, analog: i32) {
        self.board_analog_out.insert(
            (board_id.to_owned(), pin.to_owned()),
            AnalogReading {
                analog,
                status: AnalogStatus::Ok,
            },
        );
    }

    fn set_board_pwm_out(&mut self, board_id: &str, pin: &str, duty: u32) {
        self.board_pwm_out
            .insert((board_id.to_owned(), pin.to_owned()), duty);
    }

    fn set_external_digital(&mut self, instance: &str, port: &str, level: ElectricalDigitalLevel) {
        self.ext_digital_out
            .insert((instance.to_owned(), port.to_owned()), level);
    }

    fn clear_external_digital(&mut self, instance: &str, port: &str) {
        self.ext_digital_out
            .remove(&(instance.to_owned(), port.to_owned()));
    }

    fn set_external_analog(&mut self, instance: &str, port: &str, reading: AnalogReading) {
        self.ext_analog_out
            .insert((instance.to_owned(), port.to_owned()), reading);
    }

    fn solve_at(&mut self, _time_us: u64) {
        self.solved_net_digital.clear();
        self.solved_net_analog.clear();

        // Combine all digital drivers on each net deterministically.
        for (key, &level) in self
            .board_digital_out
            .iter()
            .chain(self.ext_digital_out.iter())
        {
            if let Some(&net) = self.endpoint_to_net.get(key) {
                self.solved_net_digital
                    .entry(net)
                    .and_modify(|existing| *existing = merge_digital(*existing, level))
                    .or_insert(level);
            }
        }

        // Propagate analog drivers on each net deterministically (external stimulus overrides board out if both present).
        for (key, &reading) in self
            .board_analog_out
            .iter()
            .chain(self.ext_analog_out.iter())
        {
            if let Some(&net) = self.endpoint_to_net.get(key) {
                self.solved_net_analog.insert(net, reading);
            }
        }
    }

    fn read_board_digital_in(&self, board_id: &str, pin: &str) -> ElectricalDigitalLevel {
        let key = (board_id.to_owned(), pin.to_owned());
        if let Some(net) = self.resolve_net(board_id, pin) {
            if let Some(&level) = self.solved_net_digital.get(&net) {
                return level;
            }
            if let Some(&pull) = self.net_pull.get(&net) {
                return match pull {
                    PullMode::PullUp => ElectricalDigitalLevel::High,
                    PullMode::None | PullMode::PullDown => ElectricalDigitalLevel::Low,
                };
            }
        }
        // Fallback for unwired pins receiving a direct stimulus
        if let Some(&level) = self.ext_digital_out.get(&key) {
            return level;
        }
        if let Some(&level) = self.board_digital_out.get(&key) {
            return level;
        }
        match self.endpoint_pull.get(&key).copied().unwrap_or_default() {
            PullMode::PullUp => ElectricalDigitalLevel::High,
            PullMode::None | PullMode::PullDown => ElectricalDigitalLevel::Low,
        }
    }

    fn read_board_analog_in(&self, board_id: &str, pin: &str) -> AnalogReading {
        let key = (board_id.to_owned(), pin.to_owned());
        if let Some(net) = self.resolve_net(board_id, pin)
            && let Some(&reading) = self.solved_net_analog.get(&net)
        {
            return reading;
        }
        if let Some(&reading) = self.ext_analog_out.get(&key) {
            return reading;
        }
        if let Some(&reading) = self.board_analog_out.get(&key) {
            return reading;
        }
        AnalogReading::default()
    }
}

const fn merge_digital(
    a: ElectricalDigitalLevel,
    b: ElectricalDigitalLevel,
) -> ElectricalDigitalLevel {
    match (a, b) {
        (ElectricalDigitalLevel::High, ElectricalDigitalLevel::High) => {
            ElectricalDigitalLevel::High
        }
        (ElectricalDigitalLevel::Low, ElectricalDigitalLevel::Low) => ElectricalDigitalLevel::Low,
        (ElectricalDigitalLevel::High, ElectricalDigitalLevel::Low)
        | (ElectricalDigitalLevel::Low, ElectricalDigitalLevel::High)
        | (ElectricalDigitalLevel::Indeterminate, _)
        | (_, ElectricalDigitalLevel::Indeterminate) => ElectricalDigitalLevel::Indeterminate,
    }
}

/// Deterministic Union-Find with path compression using zero unchecked indexing.
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
            // Deterministic canonical root: always point larger index to smaller index.
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            if let Some(slot) = self.parent.get_mut(hi) {
                *slot = lo;
            }
        }
    }
}
