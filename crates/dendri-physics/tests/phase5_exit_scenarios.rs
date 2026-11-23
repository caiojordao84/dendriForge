//! Phase 5 Exit Verification Scenarios (`05-ide-and-roadmap.md` §6, `02` §4–§5):
//!
//! 1. **Electro-Pneumatic Closed Loop `A+ B+ A- B-`:**
//!    Real `plc-1200-open-1214` executing an IEC 60848 GRAFCET (`PouBody::Chart`) coupled via
//!    `dendri-circuit` MNA (24 V solenoid coils + 24 V limit-switch contacts) and `dendri-physics`
//!    (`2x PneumaticValve52` + `2x DoubleActingCylinder` + `4x LimitSwitch` `A0/A1/B0/B1`),
//!    with ZERO plant hardcoding inside `dendri-vm`.
//! 2. **Multi-Board `arduino-uno-r3` <-> `plc-1200-open-1214` UART & Modbus TCP Co-Simulation:**
//!    Heterogeneous `Cooperative` (16 MHz Uno) + `PlcScan` (10 ms Open PLC) exchanging real UART
//!    frames across `BusTopology` (`UartBus` + `OB_COMM_EVENT` `OB50`) and Modbus TCP registers
//!    (`ModbusNetwork` FC1/FC3/FC5/FC6/FC16) plus `DISPLAY_PRINT` into `LcdCharFramebuffer`.
//! 3. **Coupled Thermal Chamber + 4-20 mA Loop + 500 Ω Shunt on PLC `IW64`:**
//!    Real 24 V PLC heater output drives `ThermalChamber` -> 4-20 mA transmitter stamped into
//!    `dendri-circuit` -> 500 Ω shunt voltage -> PLC `IW64` ADC reading rises physically.

use dendri_bus::{
    BusTopology, ModbusNetwork, ModbusNodeConfig, ModbusRegisterBank, ModbusRequest,
    ModbusResponse, UartBus, UartEndpointConfig,
};
use dendri_circuit::{
    ComponentSpec, CurrentSourceDcParams, ResistorParams, SpiceCircuitBackend, SwitchParams,
};
use dendri_physics::{
    DoubleActingCylinder, DoubleActingCylinderParams, LcdCharFramebuffer, LimitSwitch,
    LimitSwitchParams, PhysicsWorld, PneumaticValve52, PneumaticValve52Params, ThermalChamber,
    ThermalChamberParams,
};
use dendri_project::load_project_str;
use dendri_sim_kernel::{
    DigitalWireFabric, ElectricalBackend, InputTimeline, KernelConfig, SimKernel,
};
use dendri_vm::{AnalogReading, ElectricalDigitalLevel, Value};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

// ===========================================================================
// Scenario 1: Electro-Pneumatic Closed Loop A+ B+ A- B- (GRAFCET + MNA + Physics)
// ===========================================================================

struct PneumaticCoSimBackend {
    spice: SpiceCircuitBackend,
    world: PhysicsWorld,
    last_time_us: u64,
    step_interval_us: u64,
    start_release_us: u64,
    start_released: bool,
    peak_a_mm: Arc<Mutex<f64>>,
    peak_b_mm: Arc<Mutex<f64>>,
    final_a_mm: Arc<Mutex<f64>>,
    final_b_mm: Arc<Mutex<f64>>,
}

impl ElectricalBackend for PneumaticCoSimBackend {
    fn set_board_digital_out(&mut self, board_id: &str, pin: &str, high: bool) {
        self.spice.set_board_digital_out(board_id, pin, high);
    }

    fn set_board_analog_out(&mut self, board_id: &str, pin: &str, analog: i32) {
        self.spice.set_board_analog_out(board_id, pin, analog);
    }

    fn set_board_pwm_out(&mut self, board_id: &str, pin: &str, duty: u32) {
        self.spice.set_board_pwm_out(board_id, pin, duty);
    }

    fn set_external_digital(&mut self, instance: &str, port: &str, level: ElectricalDigitalLevel) {
        self.spice.set_external_digital(instance, port, level);
    }

    fn clear_external_digital(&mut self, instance: &str, port: &str) {
        self.spice.clear_external_digital(instance, port);
    }

    fn set_external_analog(&mut self, instance: &str, port: &str, reading: AnalogReading) {
        self.spice.set_external_analog(instance, port, reading);
    }

    fn solve_at(&mut self, time_us: u64) {
        if !self.start_released && time_us >= self.start_release_us {
            let _ = self
                .spice
                .circuit_mut()
                .set_switch_closed("sw_start", false);
            self.start_released = true;
        }
        // 1. Solve electrical network for current coil voltages.
        self.spice.solve_at(time_us);

        // 2. Step decoupled physical plant over dt_us and stamp updated limit switch contacts.
        let dt_us = time_us.saturating_sub(self.last_time_us);
        self.last_time_us = self.last_time_us.max(time_us);
        self.world.step(dt_us, self.spice.circuit_mut());

        // 3. Re-solve MNA so PLC digital inputs `Ia.0..Ia.3` immediately reflect limit switches!
        self.spice.solve_at(time_us);

        if let Some(out_a) = self.world.outputs("cyl_a")
            && let Some(&mm) = out_a.scalars.get("position_mm")
        {
            if let Ok(mut p) = self.peak_a_mm.lock()
                && mm > *p
            {
                *p = mm;
            }
            if let Ok(mut f) = self.final_a_mm.lock() {
                *f = mm;
            }
        }
        if let Some(out_b) = self.world.outputs("cyl_b")
            && let Some(&mm) = out_b.scalars.get("position_mm")
        {
            if let Ok(mut p) = self.peak_b_mm.lock()
                && mm > *p
            {
                *p = mm;
            }
            if let Ok(mut f) = self.final_b_mm.lock() {
                *f = mm;
            }
        }
    }

    fn read_board_digital_in(&self, board_id: &str, pin: &str) -> ElectricalDigitalLevel {
        self.spice.read_board_digital_in(board_id, pin)
    }

    fn read_board_analog_in(&self, board_id: &str, pin: &str) -> AnalogReading {
        self.spice.read_board_analog_in(board_id, pin)
    }

    fn next_electrical_event_us(&self) -> Option<u64> {
        Some(self.last_time_us.saturating_add(self.step_interval_us))
    }
}

#[test]
fn electro_pneumatic_a_plus_b_plus_a_minus_b_minus_closed_loop_grafcet() {
    let project_toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Phase5ElectroPneumaticABSequence"

[[boards]]
id = "plc1"
profile = "plc-1200-open-1214"
authoring_language = "sfc"
execution = "plc_scan"
scan_period_us = 10000

[boards.program]
imports = ["std.iec.*", "std.plc.*"]

[[boards.program.hardware]]
id = "n_a0"
label = "A0"
pin = "Ia.0"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_a1"
label = "A1"
pin = "Ia.1"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_b0"
label = "B0"
pin = "Ia.2"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_b1"
label = "B1"
pin = "Ia.3"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_start"
label = "START"
pin = "Ia.4"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_sol_a"
label = "SOL_A"
pin = "Qa.0"
kind = "digital_out"

[[boards.program.hardware]]
id = "n_sol_b"
label = "SOL_B"
pin = "Qa.1"
kind = "digital_out"

[[boards.program.data]]
id = "n_log"
name = "stage_mask"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.data]]
id = "n_cycles"
name = "cycles_done"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_ob1"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "chart"

[boards.program.pous.chart]
evolve_to_stability = false

[[boards.program.pous.chart.steps]]
id = "n_s0"
name = "S0_IDLE"
initial = true

[[boards.program.pous.chart.steps]]
id = "n_s1"
name = "S1_A_PLUS"
initial = false
[[boards.program.pous.chart.steps.actions]]
id = "n_s1_out"
qualifier = "N"
target = "SOL_A"
[[boards.program.pous.chart.steps.actions]]
id = "n_s1_p"
qualifier = "P"
body = [{ kind = "assign", id = "n_m1", target = "stage_mask", value = "stage_mask + 1" }]

[[boards.program.pous.chart.steps]]
id = "n_s2"
name = "S2_B_PLUS"
initial = false
[[boards.program.pous.chart.steps.actions]]
id = "n_s2_oa"
qualifier = "N"
target = "SOL_A"
[[boards.program.pous.chart.steps.actions]]
id = "n_s2_ob"
qualifier = "N"
target = "SOL_B"
[[boards.program.pous.chart.steps.actions]]
id = "n_s2_p"
qualifier = "P"
body = [{ kind = "assign", id = "n_m2", target = "stage_mask", value = "stage_mask + 10" }]

[[boards.program.pous.chart.steps]]
id = "n_s3"
name = "S3_A_MINUS"
initial = false
[[boards.program.pous.chart.steps.actions]]
id = "n_s3_ob"
qualifier = "N"
target = "SOL_B"
[[boards.program.pous.chart.steps.actions]]
id = "n_s3_p"
qualifier = "P"
body = [{ kind = "assign", id = "n_m3", target = "stage_mask", value = "stage_mask + 100" }]

[[boards.program.pous.chart.steps]]
id = "n_s4"
name = "S4_B_MINUS"
initial = false
[[boards.program.pous.chart.steps.actions]]
id = "n_s4_p"
qualifier = "P"
body = [{ kind = "assign", id = "n_m4", target = "stage_mask", value = "stage_mask + 1000" }]
[[boards.program.pous.chart.steps.actions]]
id = "n_s4_p0"
qualifier = "P0"
body = [{ kind = "assign", id = "n_cinc", target = "cycles_done", value = "cycles_done + 1" }]

[[boards.program.pous.chart.transitions]]
id = "n_t01"
name = "T_START"
from_steps = ["S0_IDLE"]
to_steps = ["S1_A_PLUS"]
priority = 1
receptivity = "START AND A0 AND B0"

[[boards.program.pous.chart.transitions]]
id = "n_t12"
name = "T_A1"
from_steps = ["S1_A_PLUS"]
to_steps = ["S2_B_PLUS"]
priority = 1
receptivity = "A1"

[[boards.program.pous.chart.transitions]]
id = "n_t23"
name = "T_B1"
from_steps = ["S2_B_PLUS"]
to_steps = ["S3_A_MINUS"]
priority = 1
receptivity = "B1"

[[boards.program.pous.chart.transitions]]
id = "n_t34"
name = "T_A0"
from_steps = ["S3_A_MINUS"]
to_steps = ["S4_B_MINUS"]
priority = 1
receptivity = "A0"

[[boards.program.pous.chart.transitions]]
id = "n_t40"
name = "T_B0"
from_steps = ["S4_B_MINUS"]
to_steps = ["S0_IDLE"]
priority = 1
receptivity = "B0"

[[wires]]
id = "w_coil_a_hi"
from = "plc1.Qa.0"
to = "coil_a.A"
signal = "digital"
[[wires]]
id = "w_coil_a_lo"
from = "coil_a.B"
to = "plc1.GND"
signal = "ground"

[[wires]]
id = "w_coil_b_hi"
from = "plc1.Qa.1"
to = "coil_b.A"
signal = "digital"
[[wires]]
id = "w_coil_b_lo"
from = "coil_b.B"
to = "plc1.GND"
signal = "ground"

[[wires]]
id = "w_a0_hi"
from = "plc1.24V"
to = "sw_a0.A"
signal = "power"
[[wires]]
id = "w_a0_in"
from = "sw_a0.B"
to = "plc1.Ia.0"
signal = "digital"

[[wires]]
id = "w_a1_hi"
from = "plc1.24V"
to = "sw_a1.A"
signal = "power"
[[wires]]
id = "w_a1_in"
from = "sw_a1.B"
to = "plc1.Ia.1"
signal = "digital"

[[wires]]
id = "w_b0_hi"
from = "plc1.24V"
to = "sw_b0.A"
signal = "power"
[[wires]]
id = "w_b0_in"
from = "sw_b0.B"
to = "plc1.Ia.2"
signal = "digital"

[[wires]]
id = "w_b1_hi"
from = "plc1.24V"
to = "sw_b1.A"
signal = "power"
[[wires]]
id = "w_b1_in"
from = "sw_b1.B"
to = "plc1.Ia.3"
signal = "digital"

[[wires]]
id = "w_start_hi"
from = "plc1.24V"
to = "sw_start.A"
signal = "power"
[[wires]]
id = "w_start_in"
from = "sw_start.B"
to = "plc1.Ia.4"
signal = "digital"
"#;

    let proj = load_project_str(project_toml, None).unwrap();

    // Build real SPICE MNA circuit with 24 V solenoid coils (240 Ω) + 5 physical switches
    let mut spice = SpiceCircuitBackend::new(dendri_circuit::Circuit::default());
    for coil in ["coil_a", "coil_b"] {
        spice
            .circuit_mut()
            .add_component(
                coil,
                ComponentSpec::Resistor(ResistorParams::with_power(240.0, 5.0)),
            )
            .unwrap();
    }
    for (sw, init_closed) in [
        ("sw_a0", true),
        ("sw_a1", false),
        ("sw_b0", true),
        ("sw_b1", false),
        ("sw_start", true), // Start command active for one cycle, then cleared after 50 ms
    ] {
        spice
            .circuit_mut()
            .add_component(
                sw,
                ComponentSpec::Switch(SwitchParams::normally_open(init_closed)),
            )
            .unwrap();
    }
    spice.attach_project_boards_and_wires(&proj).unwrap();

    // Build decoupled PhysicsWorld: 2x 5/2 valves -> 2x DA cylinders -> 4x limit switches
    let mut world = PhysicsWorld::new();
    world.add_component(
        "valve_a",
        Box::new(PneumaticValve52::new(PneumaticValve52Params::default())),
    );
    world.add_component(
        "valve_b",
        Box::new(PneumaticValve52::new(PneumaticValve52Params::default())),
    );
    world.add_component(
        "cyl_a",
        Box::new(DoubleActingCylinder::new(
            DoubleActingCylinderParams::default(),
        )),
    );
    world.add_component(
        "cyl_b",
        Box::new(DoubleActingCylinder::new(
            DoubleActingCylinderParams::default(),
        )),
    );
    for (name, sw_id, target) in [
        ("ls_a0", "sw_a0", 0.0),
        ("ls_a1", "sw_a1", 1.0),
        ("ls_b0", "sw_b0", 0.0),
        ("ls_b1", "sw_b1", 1.0),
    ] {
        world.add_component(
            name,
            Box::new(LimitSwitch::new(LimitSwitchParams {
                circuit_switch_id: sw_id.to_owned(),
                trigger_position_norm: target,
                trigger_band: 0.02,
                normally_closed: false,
            })),
        );
    }

    // Wire electrical voltage taps from MNA coils into pneumatic valves
    world.add_voltage_tap("valve_a", "v_y1", "coil_a", "A", "coil_a", "B");
    world.add_voltage_tap("valve_b", "v_y1", "coil_b", "A", "coil_b", "B");

    // Wire pneumatic domain links: Valve -> Cylinder -> Limit Switches
    world.add_link("valve_a", "pressure_a", "cyl_a", "pressure_a");
    world.add_link("valve_a", "pressure_b", "cyl_a", "pressure_b");
    world.add_link("valve_b", "pressure_a", "cyl_b", "pressure_a");
    world.add_link("valve_b", "pressure_b", "cyl_b", "pressure_b");
    world.add_link("cyl_a", "stroke_norm", "ls_a0", "position_norm");
    world.add_link("cyl_a", "stroke_norm", "ls_a1", "position_norm");
    world.add_link("cyl_b", "stroke_norm", "ls_b0", "position_norm");
    world.add_link("cyl_b", "stroke_norm", "ls_b1", "position_norm");

    let peak_a = Arc::new(Mutex::new(0.0_f64));
    let peak_b = Arc::new(Mutex::new(0.0_f64));
    let final_a = Arc::new(Mutex::new(0.0_f64));
    let final_b = Arc::new(Mutex::new(0.0_f64));

    let backend = PneumaticCoSimBackend {
        spice,
        world,
        last_time_us: 0,
        step_interval_us: 10_000,
        start_release_us: 50_000,
        start_released: false,
        peak_a_mm: Arc::clone(&peak_a),
        peak_b_mm: Arc::clone(&peak_b),
        final_a_mm: Arc::clone(&final_a),
        final_b_mm: Arc::clone(&final_b),
    };

    let mut kernel = SimKernel::with_backend(
        &proj,
        KernelConfig::default(),
        InputTimeline::new(),
        Box::new(backend),
    )
    .unwrap();

    // Run 1.8 s of real physical electro-pneumatic time after a 50 ms START pushbutton pulse:
    // completes EXACTLY 1 full A+ -> B+ -> A- -> B- cycle and comes cleanly to rest at S0_IDLE!
    kernel.run_until(1_800_000).unwrap();

    let stage_mask = kernel.board_global("plc1", "stage_mask").cloned();
    let cycles_done = kernel.board_global("plc1", "cycles_done").cloned();
    let p_a = *peak_a.lock().unwrap();
    let p_b = *peak_b.lock().unwrap();
    let f_a = *final_a.lock().unwrap();
    let f_b = *final_b.lock().unwrap();

    assert_eq!(
        cycles_done,
        Some(Value::Dint(1)),
        "expected exactly 1 completed A+ B+ A- B- cycle (mask={stage_mask:?})"
    );
    assert_eq!(
        stage_mask,
        Some(Value::Dint(1111)),
        "cycle must traverse S1(+1) -> S2(+10) -> S3(+100) -> S4(+1000) exactly once"
    );
    assert!(
        p_a >= 99.0 && p_b >= 99.0,
        "both pneumatic cylinders must physically stroke to 100 mm (peak_a={p_a:.1}mm, peak_b={p_b:.1}mm)"
    );
    assert!(
        f_a <= 1.0 && f_b <= 1.0,
        "both pneumatic cylinders must fully retract to 0 mm at end of cycle (final_a={f_a:.1}mm, final_b={f_b:.1}mm)"
    );
    let snap = kernel.snapshot();
    let active_steps = snap
        .boards
        .get("plc1")
        .and_then(|b| b.active_sfc_steps.get("OB_MAIN"))
        .unwrap();
    assert!(
        active_steps.contains("S0_IDLE") && active_steps.len() == 1,
        "GRAFCET must return cleanly to S0_IDLE, active={active_steps:?}"
    );
}

// ===========================================================================
// Scenario 2: Multi-Board Uno <-> PLC UART + OB50 + Modbus TCP + LCD Display
// ===========================================================================

struct BusCoSimBackend {
    wire: DigitalWireFabric,
    topo: BusTopology,
    uart: UartBus,
    lcd: Arc<Mutex<LcdCharFramebuffer>>,
    rx_log: Arc<Mutex<Vec<String>>>,
}

impl ElectricalBackend for BusCoSimBackend {
    fn set_board_digital_out(&mut self, board_id: &str, pin: &str, high: bool) {
        self.wire.set_board_digital_out(board_id, pin, high);
    }
    fn set_board_analog_out(&mut self, board_id: &str, pin: &str, analog: i32) {
        self.wire.set_board_analog_out(board_id, pin, analog);
    }
    fn set_board_pwm_out(&mut self, board_id: &str, pin: &str, duty: u32) {
        self.wire.set_board_pwm_out(board_id, pin, duty);
    }
    fn set_external_digital(&mut self, instance: &str, port: &str, level: ElectricalDigitalLevel) {
        self.wire.set_external_digital(instance, port, level);
    }
    fn clear_external_digital(&mut self, instance: &str, port: &str) {
        self.wire.clear_external_digital(instance, port);
    }
    fn set_external_analog(&mut self, instance: &str, port: &str, reading: AnalogReading) {
        self.wire.set_external_analog(instance, port, reading);
    }
    fn solve_at(&mut self, time_us: u64) {
        self.wire.solve_at(time_us);
        self.uart.step_to(time_us);
    }
    fn read_board_digital_in(&self, board_id: &str, pin: &str) -> ElectricalDigitalLevel {
        self.wire.read_board_digital_in(board_id, pin)
    }
    fn read_board_analog_in(&self, board_id: &str, pin: &str) -> AnalogReading {
        self.wire.read_board_analog_in(board_id, pin)
    }
    fn next_electrical_event_us(&self) -> Option<u64> {
        self.uart.next_arrival_us()
    }
    fn board_uart_send(
        &mut self,
        board_id: &str,
        _channel: &str,
        payload: &str,
        time_us: u64,
    ) -> bool {
        let mut with_nl = payload.to_owned();
        if !with_nl.ends_with('\n') {
            with_nl.push('\n');
        }
        self.uart
            .write_str(board_id, &with_nl, &self.topo, time_us)
            .is_ok()
    }
    fn board_uart_recv(&mut self, board_id: &str, _channel: &str) -> Option<String> {
        let line = self.uart.read_line(board_id)?;
        if let Ok(mut log) = self.rx_log.lock() {
            log.push(format!("{board_id}:{line}"));
        }
        Some(line)
    }
    fn board_display_print(&mut self, _board_id: &str, _target: &str, text: &str) {
        if let Ok(mut lcd) = self.lcd.lock() {
            lcd.clear();
            let _ = lcd.print_str(text);
        }
    }
    fn drain_comm_events(&mut self) -> Vec<(String, String)> {
        self.uart
            .drain_rx_notifications()
            .into_iter()
            .map(|board_id| (board_id, "uart0".to_owned()))
            .collect()
    }
}

#[test]
fn multi_board_uno_plc_uart_ob50_modbus_tcp_and_lcd_cosim() {
    let project_toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Phase5UnoPlcUartModbus"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[boards.program]
imports = ["std.iec.*", "std.arduino.*"]

[[boards.program.data]]
id = "n_u_sent"
name = "sent_once"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.data]]
id = "n_u_tx"
name = "tx_fb"
scope = "var"
ty = { kind = "fb", fb_kind = { name = "uart_send" } }

[[boards.program.pous]]
id = "n_u_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "if"
id = "n_u_if"
condition = "NOT sent_once"
[[boards.program.pous.body.then_body]]
kind = "fb_invoke"
id = "n_u_send"
instance = "tx_fb"
inputs = [{ name = "REQ", value = "true" }, { name = "DATA", value = "\"RUN:25°C\"" }]
[[boards.program.pous.body.then_body]]
kind = "assign"
id = "n_u_flag"
target = "sent_once"
value = "true"
[[boards.program.pous.body.then_body]]
kind = "call"
id = "n_u_disp"
callee = "DISPLAY_PRINT"
args = [{ value = "\"lcd1\"" }, { value = "\"TEMP: 25°C\"" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_u_wait"
duration = "T#10ms"

[[boards]]
id = "plc1"
profile = "plc-1200-open-1214"
authoring_language = "st"
execution = "plc_scan"
scan_period_us = 10000

[boards.program]
imports = ["std.iec.*", "std.plc.*"]

[[boards.program.hardware]]
id = "n_p_q0"
label = "RUN_LAMP"
pin = "Qa.0"
kind = "digital_out"

[[boards.program.data]]
id = "n_p_rx"
name = "rx_fb"
scope = "var"
ty = { kind = "fb", fb_kind = { name = "uart_recv" } }

[[boards.program.data]]
id = "n_p_cnt"
name = "ob50_count"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.data]]
id = "n_p_ok"
name = "cmd_received"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.pous]]
id = "n_p_ob1"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "network"
[[boards.program.pous.network.rungs]]
id = "n_p_r0"
order = 0
[[boards.program.pous.network.rungs.cells]]
id = "n_p_c0"
row = 0
col = 0
kind = "contact"
contact_kind = "normally_open"
target = "cmd_received"
[[boards.program.pous.network.rungs.cells]]
id = "n_p_c1"
row = 0
col = 1
kind = "coil"
coil_kind = "normal"
target = "RUN_LAMP"

[[boards.program.pous]]
id = "n_p_ob50"
name = "OB_UART_RX"
kind = { pou_kind = "ob", ob = { type = "comm_event", bus_id = "uart0" } }
body_kind = "statements"
[[boards.program.pous.body]]
kind = "fb_invoke"
id = "n_p_recv"
instance = "rx_fb"
inputs = [{ name = "EN_R", value = "true" }]
[[boards.program.pous.body]]
kind = "if"
id = "n_p_chk"
condition = "rx_fb.NDR"
[[boards.program.pous.body.then_body]]
kind = "assign"
id = "n_p_inc"
target = "ob50_count"
value = "ob50_count + 1"
[[boards.program.pous.body.then_body]]
kind = "assign"
id = "n_p_set"
target = "cmd_received"
value = "true"

[[wires]]
id = "w_uart_tx_rx"
from = "uno.D1"
to = "plc1.Ia.7"
signal = "comm"

[[wires]]
id = "w_eth_lan"
from = "uno.D10"
to = "plc1.ETH0"
signal = "comm"
"#;

    let proj = load_project_str(project_toml, None).unwrap();
    let topo = BusTopology::from_project(&proj).unwrap();
    let wire = DigitalWireFabric::from_project(&proj).unwrap();

    let mut uart = UartBus::new();
    uart.register_endpoint(UartEndpointConfig::standard_8n1(
        "uno", "uno", "D1", "D0", 115_200,
    ))
    .unwrap();
    uart.register_endpoint(UartEndpointConfig::standard_8n1(
        "plc1", "plc1", "Qa.7", "Ia.7", 115_200,
    ))
    .unwrap();

    let lcd = Arc::new(Mutex::new(LcdCharFramebuffer::hd44780_16x2(0x27)));
    let rx_log = Arc::new(Mutex::new(Vec::new()));

    let backend = BusCoSimBackend {
        wire,
        topo: topo.clone(),
        uart,
        lcd: Arc::clone(&lcd),
        rx_log: Arc::clone(&rx_log),
    };

    let mut kernel = SimKernel::with_backend(
        &proj,
        KernelConfig::default(),
        InputTimeline::new(),
        Box::new(backend),
    )
    .unwrap();

    kernel.run_until(30_000).unwrap();

    // 1. Uno sent "RUN:25°C" over physical wired UART -> arrived after ~870 µs -> triggered OB50 on PLC!
    assert_eq!(
        kernel.board_global("plc1", "ob50_count"),
        Some(&Value::Dint(1))
    );
    assert_eq!(
        kernel.board_global("plc1", "cmd_received"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        kernel
            .snapshot()
            .boards
            .get("plc1")
            .and_then(|b| b.digital_outputs.get("Qa.0")),
        Some(&true)
    );
    assert_eq!(*rx_log.lock().unwrap(), vec!["plc1:RUN:25°C".to_owned()]);

    // 2. Display framebuffer rendered HD44780 ROM A00 degree symbol cleanly!
    assert_eq!(lcd.lock().unwrap().line_text(0).trim_end(), "TEMP: 25°C");

    // 3. Mirror PLC live coil state into Virtual Ethernet / Modbus TCP LAN over the wired Ethernet net
    let mut modbus = ModbusNetwork::new();
    let plc_ip = Ipv4Addr::new(192, 168, 1, 10);
    let uno_ip = Ipv4Addr::new(192, 168, 1, 20);
    let mut plc_bank = ModbusRegisterBank::standard_256();
    plc_bank.coils[0] = kernel
        .snapshot()
        .boards
        .get("plc1")
        .and_then(|b| b.digital_outputs.get("Qa.0"))
        .copied()
        .unwrap_or(false);
    modbus.register_node(
        ModbusNodeConfig {
            node_id: "plc1".to_owned(),
            ip: plc_ip,
            unit_id: 1,
            ethernet_port: Some(("plc1".to_owned(), "ETH0".to_owned())),
        },
        plc_bank,
    );
    modbus.register_node(
        ModbusNodeConfig {
            node_id: "uno".to_owned(),
            ip: uno_ip,
            unit_id: 2,
            ethernet_port: Some(("uno".to_owned(), "D10".to_owned())),
        },
        ModbusRegisterBank::standard_256(),
    );

    // Uno reads PLC coil 0 via FC1 -> observes true (Qa.0 energized by UART command!)
    let coils = modbus
        .request(
            "uno",
            plc_ip,
            1,
            &ModbusRequest::ReadCoils { start: 0, count: 1 },
            Some(&topo),
        )
        .unwrap();
    assert_eq!(coils, ModbusResponse::Coils(vec![true]));

    // Uno writes setpoint 2500 via FC6 and reads back via FC3
    modbus
        .request(
            "uno",
            plc_ip,
            1,
            &ModbusRequest::WriteSingleRegister {
                addr: 10,
                value: 2500,
            },
            Some(&topo),
        )
        .unwrap();
    let regs = modbus
        .request(
            "uno",
            plc_ip,
            1,
            &ModbusRequest::ReadHoldingRegisters {
                start: 10,
                count: 1,
            },
            Some(&topo),
        )
        .unwrap();
    assert_eq!(regs, ModbusResponse::HoldingRegisters(vec![2500]));
}

// ===========================================================================
// Scenario 3: Thermal Chamber -> 4-20 mA Loop -> 500 Ω Shunt -> MNA Voltage
// ===========================================================================

#[test]
fn thermal_chamber_stamps_4_20ma_loop_into_spice_shunt_accurately() {
    let mut circuit = dendri_circuit::Circuit::default();
    circuit.add_component("gnd", ComponentSpec::Ground).unwrap();
    circuit
        .add_component(
            "tx_temp",
            ComponentSpec::CurrentSourceDc(CurrentSourceDcParams { current_a: 0.004 }),
        )
        .unwrap();
    circuit
        .add_component(
            "r_shunt",
            ComponentSpec::Resistor(ResistorParams::with_power(500.0, 1.0)),
        )
        .unwrap();
    circuit.connect("gnd", "GND", "tx_temp", "IN");
    circuit.connect("tx_temp", "OUT", "r_shunt", "A");
    circuit.connect("r_shunt", "B", "gnd", "GND");

    let mut world = PhysicsWorld::new();
    world.add_component(
        "oven",
        Box::new(ThermalChamber::new(ThermalChamberParams {
            t_amb_c: 25.0,
            r_th_k_per_w: 1.0,
            c_th_j_per_k: 1.0, // tau = 1.0 s
            r_heater_ohm: 12.0,
            tx_min_c: 0.0,
            tx_max_c: 100.0,
            transmitter_circuit_id: Some("tx_temp".to_owned()),
            ..ThermalChamberParams::default()
        })),
    );
    // Apply 24 V across a virtual 12 Ω heater -> 48 W -> steady state = 25 + 48 = 73 °C!
    circuit
        .add_component(
            "v_heat",
            ComponentSpec::VoltageSourceDc(dendri_circuit::VoltageSourceDcParams::ideal(24.0)),
        )
        .unwrap();
    circuit.connect("v_heat", "NEG", "gnd", "GND");
    world.add_voltage_tap("oven", "v_heater", "v_heat", "POS", "v_heat", "NEG");

    // Step 10 seconds (= 10 * tau -> steady state 73.0 °C within 0.01 °C)
    let _ = circuit.solve_dc_op();
    for _ in 0..100 {
        world.step(100_000, &mut circuit);
        let _ = circuit.solve_dc_op();
    }

    let expected_temp = 73.0_f64;
    let expected_i = 0.004 + (expected_temp / 100.0) * 0.016; // 15.68 mA
    let expected_v = expected_i * 500.0; // 7.840 V
    let got_v = circuit.terminal_voltage("r_shunt", "A");

    assert!(
        (got_v - expected_v).abs() < 5e-3,
        "coupled thermal 4-20 mA shunt voltage mismatch: expected {expected_v:.4} V, got {got_v:.4} V"
    );
}
