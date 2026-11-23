//! [`SpiceCircuitBackend`] bridging [`crate::Circuit`] into [`dendri_sim_kernel::ElectricalBackend`]
//! with realistic MCU/PLC pin Thevenin/Schmitt boundaries, verified PWM timers (`DEF-001`),
//! exact round-half-even Solver -> ASL ADC conversion (`D6`), and electrical pin fault monitoring
//! (`02-simulation-and-electrical-engine.md` §3.3).

use crate::boundary::{
    BoardElectricalProfile, BoardPinSpec, arduino_uno_r3_electrical, siemens_s7_1200_electrical,
    voltage_to_analog_round_half_even,
};
use crate::devices::{
    CapacitorParams, ComponentSpec, DiodeParams, InductorParams, PotentiometerParams,
    ResistorParams, SwitchParams, VoltageSourceDcParams,
};
use crate::pwm::PwmWaveform;
use crate::solver::{Circuit, CircuitError, ElectricalFault, FaultKind};
use dendri_ast::{
    ASL_ANALOG_FULL_SCALE, AnalogStatus, ChannelRange, HardwareKind, Project, PullMode,
};
use dendri_sim_kernel::ElectricalBackend;
use dendri_vm::{AnalogReading, ElectricalDigitalLevel};
use std::collections::BTreeMap;

/// Board attachment specification for coupling a board's physical pins into a [`Circuit`].
#[derive(Debug, Clone)]
pub struct BoardCircuitSpec {
    pub board_id: String,
    pub profile: BoardElectricalProfile,
    pub pins: Vec<BoardPinSpec>,
}

impl BoardCircuitSpec {
    /// Builds a [`BoardCircuitSpec`] from a validated v2 [`dendri_ast::Board`] and electrical profile.
    #[must_use]
    pub fn from_ast_board(board: &dendri_ast::Board, profile: BoardElectricalProfile) -> Self {
        let mut pins = Vec::with_capacity(board.program.hardware.len());
        for hw in &board.program.hardware {
            let is_output = matches!(
                hw.kind,
                HardwareKind::DigitalOut
                    | HardwareKind::DigitalBidirectional
                    | HardwareKind::AnalogOut
                    | HardwareKind::PwmOut
            );
            let is_analog = matches!(hw.kind, HardwareKind::AnalogIn | HardwareKind::AnalogOut);
            let is_pwm = matches!(hw.kind, HardwareKind::PwmOut);
            pins.push(BoardPinSpec {
                pin: hw.pin.clone(),
                is_output,
                is_analog,
                is_pwm,
                pull: hw.pull,
                analog_range: hw.range.clone(),
            });
        }
        Self {
            board_id: board.id.clone(),
            profile,
            pins,
        }
    }
}

/// Default reference electrical profile resolver for Phase 3 headless scenarios.
#[must_use]
pub fn default_electrical_profile(profile_id: &str) -> BoardElectricalProfile {
    match profile_id {
        "siemens-s7-1200-cpu-1214c" => siemens_s7_1200_electrical(),
        _ => arduino_uno_r3_electrical(),
    }
}

#[derive(Debug, Clone)]
struct AttachedBoard {
    profile: BoardElectricalProfile,
    /// Pin -> synthetic internal voltage source ID for output/PWM/analog-out pins.
    out_sources: BTreeMap<String, String>,
    /// Pin -> configured ADC channel range (falling back to `profile.default_adc_range`).
    analog_ranges: BTreeMap<String, ChannelRange>,
    /// Pin -> active hardware PWM waveform (`DEF-001`) if currently PWM-driven.
    active_pwm: BTreeMap<String, PwmWaveform>,
    /// All monitored physical pins on this board (for overvoltage checks).
    monitored_pins: Vec<String>,
}

/// SPICE-class MNA [`ElectricalBackend`] driving [`dendri_sim_kernel::SimKernel`] (`02` §3.3).
#[derive(Debug, Clone)]
pub struct SpiceCircuitBackend {
    circuit: Circuit,
    boards: BTreeMap<String, AttachedBoard>,
    pin_faults: Vec<ElectricalFault>,
    time_us: u64,
}

impl SpiceCircuitBackend {
    /// Creates a new [`SpiceCircuitBackend`] wrapping an already-constructed [`Circuit`].
    #[must_use]
    pub fn new(circuit: Circuit) -> Self {
        Self {
            circuit,
            boards: BTreeMap::new(),
            pin_faults: Vec::new(),
            time_us: 0,
        }
    }

    /// Borrows the underlying SPICE [`Circuit`] for voltage/current/probe inspection.
    #[must_use]
    pub const fn circuit(&self) -> &Circuit {
        &self.circuit
    }

    /// Mutably borrows the underlying SPICE [`Circuit`] (e.g., to attach probes or extra parts).
    pub fn circuit_mut(&mut self) -> &mut Circuit {
        &mut self.circuit
    }

    /// Returns all active electrical faults across both circuit components and board pins.
    #[must_use]
    pub fn all_faults(&self) -> Vec<ElectricalFault> {
        let mut out = self.circuit.faults().to_vec();
        out.extend(self.pin_faults.iter().cloned());
        out
    }

    /// Attaches a board's power supply (`VCC`, `GND`) and physical pins into the MNA netlist.
    ///
    /// - Connects `(board_id, "GND")` to a board-local `Ground` reference (`0 V`).
    /// - Connects `(board_id, "VCC")` / `(board_id, "5V")` / `(board_id, "24V")` to `logic_high_v`.
    /// - Digital/PWM/Analog outputs couple as Thevenin sources (`V_out`, `pin_r_out_ohm`).
    /// - Digital/Analog inputs couple as `pin_r_in_ohm` in parallel with optional `pullup`/`pulldown`.
    pub fn attach_board(&mut self, spec: BoardCircuitSpec) -> Result<(), CircuitError> {
        let bid = spec.board_id.clone();
        let gnd_comp = format!("__board_{bid}_gnd");
        let vcc_comp = format!("__board_{bid}_vcc");

        self.circuit
            .add_component(&gnd_comp, ComponentSpec::Ground)?;
        self.circuit.connect(&gnd_comp, "GND", &bid, "GND");

        self.circuit.add_component(
            &vcc_comp,
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(spec.profile.logic_high_v)),
        )?;
        self.circuit.connect(&vcc_comp, "NEG", &bid, "GND");
        self.circuit.connect(&vcc_comp, "POS", &bid, "VCC");
        if (spec.profile.logic_high_v - 5.0).abs() < 0.2 {
            self.circuit.connect(&vcc_comp, "POS", &bid, "5V");
        } else if (spec.profile.logic_high_v - 24.0).abs() < 0.5 {
            self.circuit.connect(&vcc_comp, "POS", &bid, "24V");
            self.circuit.connect(&vcc_comp, "POS", &bid, "L+");
            self.circuit.connect(&gnd_comp, "GND", &bid, "M");
        }

        let mut out_sources = BTreeMap::new();
        let mut analog_ranges = BTreeMap::new();
        let mut monitored_pins = Vec::new();

        for pin_spec in &spec.pins {
            monitored_pins.push(pin_spec.pin.clone());
            if let Some(range) = &pin_spec.analog_range {
                analog_ranges.insert(pin_spec.pin.clone(), range.clone());
            }

            if pin_spec.is_output {
                let src_id = format!("__pin_{bid}_{}", pin_spec.pin);
                self.circuit.add_component(
                    &src_id,
                    ComponentSpec::VoltageSourceDc(VoltageSourceDcParams {
                        voltage_v: spec.profile.logic_low_v,
                        r_ser_ohm: spec.profile.pin_r_out_ohm.max(1e-3),
                    }),
                )?;
                self.circuit.connect(&src_id, "POS", &bid, &pin_spec.pin);
                self.circuit.connect(&src_id, "NEG", &bid, "GND");
                out_sources.insert(pin_spec.pin.clone(), src_id);
            } else {
                let rin_id = format!("__rin_{bid}_{}", pin_spec.pin);
                self.circuit.add_component(
                    &rin_id,
                    ComponentSpec::Resistor(ResistorParams::with_power(
                        spec.profile.pin_r_in_ohm.max(100.0),
                        10.0,
                    )),
                )?;
                self.circuit.connect(&rin_id, "A", &bid, &pin_spec.pin);
                self.circuit.connect(&rin_id, "B", &bid, "GND");

                match pin_spec.pull {
                    PullMode::PullUp => {
                        let rpu_id = format!("__rpu_{bid}_{}", pin_spec.pin);
                        self.circuit.add_component(
                            &rpu_id,
                            ComponentSpec::Resistor(ResistorParams::with_power(
                                spec.profile.pullup_ohm.max(100.0),
                                1.0,
                            )),
                        )?;
                        self.circuit.connect(&rpu_id, "A", &bid, &pin_spec.pin);
                        self.circuit.connect(&rpu_id, "B", &bid, "VCC");
                    }
                    PullMode::PullDown => {
                        let rpd_id = format!("__rpd_{bid}_{}", pin_spec.pin);
                        self.circuit.add_component(
                            &rpd_id,
                            ComponentSpec::Resistor(ResistorParams::with_power(
                                spec.profile.pulldown_ohm.max(100.0),
                                1.0,
                            )),
                        )?;
                        self.circuit.connect(&rpd_id, "A", &bid, &pin_spec.pin);
                        self.circuit.connect(&rpd_id, "B", &bid, "GND");
                    }
                    PullMode::None => {}
                }
            }
        }

        self.boards.insert(
            bid,
            AttachedBoard {
                profile: spec.profile,
                out_sources,
                analog_ranges,
                active_pwm: BTreeMap::new(),
                monitored_pins,
            },
        );
        Ok(())
    }

    /// Builds a complete headless [`SpiceCircuitBackend`] directly from a [`Project`]
    /// (`[[components]]`, `[[boards]]`, and `[[wires]]`), mapping Core Kit component port aliases
    /// (`resistor`: `1`/`2`, `cmd-pushbutton-no-start`: `NO`/`COM`, `cmd-pushbutton-nc-stop`: `NC`/`COM`,
    /// `maker-tactile-4pin`: `1`/`2`, `led`: `A`/`K`, `potentiometer`: `A`/`W`/`B`).
    pub fn from_project_headless(project: &Project) -> Result<Self, CircuitError> {
        let mut backend = Self::new(Circuit::default());
        for comp in &project.components {
            let parse_f64 = |key: &str, default: f64| -> f64 {
                comp.params
                    .get(key)
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(default)
            };
            let parse_bool = |key: &str, default: bool| -> bool {
                comp.params
                    .get(key)
                    .and_then(|s| s.parse::<bool>().ok())
                    .unwrap_or(default)
            };

            match comp.profile.as_str() {
                "resistor" => {
                    let r = parse_f64("r_ohm", parse_f64("resistance_ohm", 220.0));
                    let p = parse_f64("p_rated_w", 0.25);
                    backend.circuit.add_component(
                        &comp.id,
                        ComponentSpec::Resistor(ResistorParams::with_power(r, p)),
                    )?;
                    // Alias Core Kit ports `1`/`2` -> primitive `A`/`B`
                    backend.circuit.connect(&comp.id, "1", &comp.id, "A");
                    backend.circuit.connect(&comp.id, "2", &comp.id, "B");
                }
                "led" => {
                    let mut led = DiodeParams::led_red();
                    led.i_max_a = parse_f64("if_max_a", led.i_max_a);
                    led.v_br = parse_f64("vr_max_v", led.v_br);
                    backend
                        .circuit
                        .add_component(&comp.id, ComponentSpec::Diode(led))?;
                }
                "diode" => {
                    backend.circuit.add_component(
                        &comp.id,
                        ComponentSpec::Diode(DiodeParams::silicon_1n4148()),
                    )?;
                }
                "maker-tactile-4pin" => {
                    let closed = parse_bool("closed", parse_bool("pressed", false));
                    backend.circuit.add_component(
                        &comp.id,
                        ComponentSpec::Switch(SwitchParams::normally_open(closed)),
                    )?;
                    backend.circuit.connect(&comp.id, "1", &comp.id, "A");
                    backend.circuit.connect(&comp.id, "2", &comp.id, "B");
                }
                "cmd-pushbutton-no-start" => {
                    let closed = parse_bool("closed", parse_bool("pressed", false));
                    backend.circuit.add_component(
                        &comp.id,
                        ComponentSpec::Switch(SwitchParams::normally_open(closed)),
                    )?;
                    backend.circuit.connect(&comp.id, "NO", &comp.id, "A");
                    backend.circuit.connect(&comp.id, "COM", &comp.id, "B");
                }
                "cmd-pushbutton-nc-stop" => {
                    let pressed = parse_bool("pressed", false);
                    backend.circuit.add_component(
                        &comp.id,
                        ComponentSpec::Switch(SwitchParams::normally_closed(pressed)),
                    )?;
                    backend.circuit.connect(&comp.id, "NC", &comp.id, "A");
                    backend.circuit.connect(&comp.id, "COM", &comp.id, "B");
                }
                "potentiometer" => {
                    let r = parse_f64("r_total_ohm", 10_000.0);
                    let w = parse_f64("wiper", 0.5);
                    backend.circuit.add_component(
                        &comp.id,
                        ComponentSpec::Potentiometer(PotentiometerParams {
                            r_total_ohm: r,
                            wiper: w,
                        }),
                    )?;
                }
                "capacitor" => {
                    let c = parse_f64("c_f", 1e-6);
                    backend.circuit.add_component(
                        &comp.id,
                        ComponentSpec::Capacitor(CapacitorParams::ideal(c)),
                    )?;
                }
                "inductor" => {
                    let l = parse_f64("l_h", 1e-3);
                    let r = parse_f64("r_ser_ohm", 0.0);
                    backend.circuit.add_component(
                        &comp.id,
                        ComponentSpec::Inductor(InductorParams::new(l, r)),
                    )?;
                }
                "ground" => {
                    backend
                        .circuit
                        .add_component(&comp.id, ComponentSpec::Ground)?;
                }
                _ => {
                    // Unmodeled/auxiliary component left as passive wire junction in headless mode
                }
            }
        }
        backend.attach_project_boards_and_wires(project)?;
        Ok(backend)
    }

    /// Attaches all boards and wires from a [`Project`] using reference electrical profiles.
    pub fn attach_project_boards_and_wires(
        &mut self,
        project: &Project,
    ) -> Result<(), CircuitError> {
        for board in &project.boards {
            let profile = default_electrical_profile(&board.profile);
            self.attach_board(BoardCircuitSpec::from_ast_board(board, profile))?;
        }
        for wire in &project.wires {
            if let (Ok(from_ep), Ok(to_ep)) = (
                dendri_ast::Endpoint::parse(&wire.from),
                dendri_ast::Endpoint::parse(&wire.to),
            ) {
                self.circuit.connect(
                    from_ep.instance_id,
                    from_ep.port_id,
                    to_ep.instance_id,
                    to_ep.port_id,
                );
            }
        }
        Ok(())
    }

    fn next_pwm_edge_after(&self, now_us: u64) -> Option<u64> {
        let mut earliest: Option<u64> = None;
        for b in self.boards.values() {
            for wf in b.active_pwm.values() {
                if let Some(edge) = wf.next_edge_after(now_us) {
                    earliest = Some(match earliest {
                        Some(cur) => cur.min(edge),
                        None => edge,
                    });
                }
            }
        }
        earliest
    }

    fn apply_pwm_levels_at(&mut self, time_us: u64) {
        let mut updates: Vec<(String, f64)> = Vec::new();
        for b in self.boards.values() {
            for (pin, wf) in &b.active_pwm {
                if let Some(src_id) = b.out_sources.get(pin) {
                    let v = if wf.is_high_at(time_us) {
                        b.profile.logic_high_v
                    } else {
                        b.profile.logic_low_v
                    };
                    updates.push((src_id.clone(), v));
                }
            }
        }
        for (src_id, v) in updates {
            let _ = self.circuit.set_dc_voltage(&src_id, v);
        }
    }

    fn monitor_pin_faults(&mut self, time_us: u64) {
        self.pin_faults.clear();
        for (bid, b) in &self.boards {
            for pin in &b.monitored_pins {
                let v = self.circuit.terminal_voltage(bid, pin);
                if v > b.profile.v_abs_max_v || v < b.profile.v_abs_min_v {
                    self.pin_faults.push(ElectricalFault {
                        time_us,
                        instance_id: bid.clone(),
                        kind: FaultKind::PinOvervoltage {
                            pin: pin.clone(),
                            voltage_v: v,
                            v_abs_max_v: b.profile.v_abs_max_v,
                            v_abs_min_v: b.profile.v_abs_min_v,
                        },
                    });
                }
            }
            for (pin, src_id) in &b.out_sources {
                let i = libm::fabs(self.circuit.component_current(src_id));
                if i > b.profile.pin_i_max_a {
                    self.pin_faults.push(ElectricalFault {
                        time_us,
                        instance_id: bid.clone(),
                        kind: FaultKind::PinOvercurrent {
                            pin: pin.clone(),
                            current_a: i,
                            limit_a: b.profile.pin_i_max_a,
                        },
                    });
                }
            }
        }
    }
}

impl ElectricalBackend for SpiceCircuitBackend {
    fn set_board_digital_out(&mut self, board_id: &str, pin: &str, high: bool) {
        let Some(b) = self.boards.get_mut(board_id) else {
            return;
        };
        b.active_pwm.remove(pin);
        let v = if high {
            b.profile.logic_high_v
        } else {
            b.profile.logic_low_v
        };
        if let Some(src_id) = b.out_sources.get(pin).cloned() {
            let _ = self.circuit.set_dc_voltage(&src_id, v);
        }
    }

    fn set_board_analog_out(&mut self, board_id: &str, pin: &str, analog: i32) {
        let Some(b) = self.boards.get_mut(board_id) else {
            return;
        };
        b.active_pwm.remove(pin);
        let range = b
            .analog_ranges
            .get(pin)
            .unwrap_or(&b.profile.default_adc_range);
        let frac = (f64::from(analog)) / (ASL_ANALOG_FULL_SCALE as f64);
        let v = range.low + frac * (range.high - range.low);
        if let Some(src_id) = b.out_sources.get(pin).cloned() {
            let _ = self.circuit.set_dc_voltage(&src_id, v);
        }
    }

    fn set_board_pwm_out(&mut self, board_id: &str, pin: &str, duty: u32) {
        let Some(b) = self.boards.get_mut(board_id) else {
            return;
        };
        let wf = if let Some(tf) = b.profile.pwm_channels.get(pin) {
            tf.evaluate_asl_duty(duty)
        } else {
            crate::pwm::linear_pwm(1_000, "fallback linear PWM").evaluate_asl_duty(duty)
        };
        let v = if wf.is_high_at(self.time_us) {
            b.profile.logic_high_v
        } else {
            b.profile.logic_low_v
        };
        b.active_pwm.insert(pin.to_owned(), wf);
        if let Some(src_id) = b.out_sources.get(pin).cloned() {
            let _ = self.circuit.set_dc_voltage(&src_id, v);
        }
    }

    fn set_external_digital(&mut self, instance: &str, _port: &str, level: ElectricalDigitalLevel) {
        let closed = matches!(level, ElectricalDigitalLevel::High);
        // If `instance` is a physical Switch/Pushbutton component, actuate it directly!
        let _ = self.circuit.set_switch_closed(instance, closed);
    }

    fn clear_external_digital(&mut self, instance: &str, _port: &str) {
        let _ = self.circuit.set_switch_closed(instance, false);
    }

    fn set_external_analog(&mut self, instance: &str, _port: &str, reading: AnalogReading) {
        // Allows InputTimeline to sweep a Potentiometer wiper (0..=M) or DC current source!
        let frac = (f64::from(reading.analog) / (ASL_ANALOG_FULL_SCALE as f64)).clamp(0.0, 1.0);
        if self.circuit.set_potentiometer_wiper(instance, frac).is_ok() {
            return;
        }
        // Or drive a 4-20 mA DC current source directly in mA-scaled fraction (0..M -> 4..20 mA)
        let current_a = 0.004 + frac * 0.016;
        let _ = self.circuit.set_dc_current(instance, current_a);
    }

    fn solve_at(&mut self, target_us: u64) {
        self.circuit.compile_topology();

        // Step cleanly across any intermediate discrete PWM edges between `self.time_us` and `target_us`
        // so RC filters / motors see exact hardware square-wave edges even while CPUs sleep!
        while self.time_us < target_us {
            let next_edge = self.next_pwm_edge_after(self.time_us);
            if let Some(edge_t) = next_edge
                && edge_t <= target_us
            {
                let _ = self.circuit.step_to(edge_t);
                self.time_us = edge_t;
                self.apply_pwm_levels_at(edge_t);
                // Re-evaluate DC/switching boundary cleanly right after edge toggle
                if edge_t == target_us {
                    let _ = self.circuit.step_to(target_us);
                }
            } else {
                let _ = self.circuit.step_to(target_us);
                self.time_us = target_us;
            }
        }

        // Handle same-timestamp dirty solve (e.g., GPIO write or switch actuation at current time_us)
        self.apply_pwm_levels_at(target_us);
        let _ = self.circuit.step_to(target_us);
        self.time_us = target_us;
        self.monitor_pin_faults(target_us);
    }

    fn read_board_digital_in(&self, board_id: &str, pin: &str) -> ElectricalDigitalLevel {
        let Some(b) = self.boards.get(board_id) else {
            return ElectricalDigitalLevel::Low;
        };
        let v = self.circuit.terminal_voltage(board_id, pin);
        b.profile.classify_digital_voltage(v)
    }

    fn read_board_analog_in(&self, board_id: &str, pin: &str) -> AnalogReading {
        let Some(b) = self.boards.get(board_id) else {
            return AnalogReading {
                analog: 0,
                status: AnalogStatus::Fault,
            };
        };
        let v = self.circuit.terminal_voltage(board_id, pin);
        let range = b
            .analog_ranges
            .get(pin)
            .unwrap_or(&b.profile.default_adc_range);
        voltage_to_analog_round_half_even(v, range)
    }

    fn next_electrical_event_us(&self) -> Option<u64> {
        let pwm_next = self.next_pwm_edge_after(self.time_us);
        let trans_next = self
            .circuit
            .recommended_transient_dt_us()
            .map(|dt| self.time_us.saturating_add(dt));
        match (pwm_next, trans_next) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}
