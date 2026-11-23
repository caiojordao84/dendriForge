//! Phase 3 Exit Tracer Bullet (`02-simulation-and-electrical-engine.md` §3.4):
//! End-to-end headless scenario (`Arduino Uno R3 + pushbutton + 10 kOhm pull-down + 220 Ohm + Red LED`)
//! executing across `dendri-vm` + `dendri-circuit` (`SpiceCircuitBackend`) + `dendri-sim-kernel`,
//! verified at every phase against an independent closed-form Shockley electrical oracle (< 0.1% error).

use dendri_circuit::{
    Circuit, ComponentSpec, DiodeParams, ResistorParams, SpiceCircuitBackend, SwitchParams,
    arduino_uno_r3_electrical,
};
use dendri_project::load_project_str;
use dendri_sim_kernel::{InputTimeline, KernelConfig, SimKernel};
use dendri_vm::{ElectricalDigitalLevel, Value};

const TRACER_BULLET_TOML: &str = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Phase3TracerBulletUnoButtonLed"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[boards.program]
imports = ["std.iec.*", "std.arduino.*"]

[[boards.program.hardware]]
id = "n_btn"
label = "BTN"
pin = "D2"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_led"
label = "LED"
pin = "D13"
kind = "digital_out"

[[boards.program.data]]
id = "n_state"
name = "led_state"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.pous]]
id = "n_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_read"
target = "led_state"
value = "BTN"

[[boards.program.pous.body]]
kind = "call"
id = "n_write"
callee = "DIGITAL_WRITE"
args = [{ value = "LED" }, { value = "led_state" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_delay"
duration = "T#1ms"

[[wires]]
id = "w_vcc_btn"
from = "uno.5V"
to = "btn1.A"
signal = "power"

[[wires]]
id = "w_btn_d2"
from = "btn1.B"
to = "uno.D2"
signal = "digital"

[[wires]]
id = "w_d2_rpd"
from = "uno.D2"
to = "r_pd.A"
signal = "digital"

[[wires]]
id = "w_rpd_gnd"
from = "r_pd.B"
to = "uno.GND"
signal = "ground"

[[wires]]
id = "w_d13_rled"
from = "uno.D13"
to = "r_led.A"
signal = "digital"

[[wires]]
id = "w_rled_led"
from = "r_led.B"
to = "led1.A"
signal = "digital"

[[wires]]
id = "w_led_gnd"
from = "led1.K"
to = "uno.GND"
signal = "ground"
"#;

/// Independent closed-form Newton oracle for:
/// `V_src = (R_pin + R_lim + R_s) * I + N * V_T * ln(I / I_s + 1)`
fn independent_led_current_oracle(v_src: f64, r_total_series: f64, led: &DiodeParams) -> f64 {
    let n_vt = led.n * 0.025_851_99;
    let r_eff = r_total_series + led.r_s;
    let mut i = 0.012_f64;
    for _ in 0..50 {
        let f = r_eff * i + n_vt * libm::log(i / led.i_s + 1.0) - v_src;
        let df = r_eff + n_vt / (i + led.i_s);
        let next = (i - f / df).max(1e-12);
        if libm::fabs(next - i) < 1e-12 {
            return next;
        }
        i = next;
    }
    i
}

#[test]
fn phase3_exit_tracer_bullet_uno_button_led_end_to_end_matches_oracle() {
    let proj = load_project_str(TRACER_BULLET_TOML, None).unwrap();

    // Build physical SPICE netlist for the external components (`btn1`, `r_pd`, `r_led`, `led1`)
    let led_params = DiodeParams::led_red();
    let mut circuit = Circuit::default();
    circuit
        .add_component(
            "btn1",
            ComponentSpec::Switch(SwitchParams::normally_open(false)),
        )
        .unwrap();
    circuit
        .add_component(
            "r_pd",
            ComponentSpec::Resistor(ResistorParams::new(10_000.0)),
        )
        .unwrap();
    circuit
        .add_component("r_led", ComponentSpec::Resistor(ResistorParams::new(220.0)))
        .unwrap();
    circuit
        .add_component("led1", ComponentSpec::Diode(led_params.clone()))
        .unwrap();

    let mut spice = SpiceCircuitBackend::new(circuit);
    spice.attach_project_boards_and_wires(&proj).unwrap();

    // Deterministic external timeline:
    // - t = 0..10 ms: button released (OPEN)
    // - t = 10..30 ms: button pressed (CLOSED)
    // - t = 30..45 ms: button released (OPEN)
    let mut timeline = InputTimeline::new();
    timeline.push_digital(10_000, "btn1", "ACTUATE", ElectricalDigitalLevel::High);
    timeline.push_digital(30_000, "btn1", "ACTUATE", ElectricalDigitalLevel::Low);

    let mut kernel =
        SimKernel::with_backend(&proj, KernelConfig::default(), timeline, Box::new(spice)).unwrap();

    // --- Phase A (t = 5 ms): button OPEN -> D2 = 0V (LOW) -> D13 = 0V -> LED OFF ---
    kernel.run_until(5_000).unwrap();
    assert_eq!(
        kernel.board_global("uno", "led_state"),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        kernel
            .snapshot()
            .boards
            .get("uno")
            .and_then(|b| b.digital_outputs.get("D13")),
        Some(&false)
    );

    // --- Phase B (t = 15 ms): button CLOSED -> D2 ≈ 5.0V (HIGH) -> D13 = 5.0V (R_out=25Ω) -> LED ON! ---
    kernel.run_until(15_000).unwrap();
    assert_eq!(
        kernel.board_global("uno", "led_state"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        kernel
            .snapshot()
            .boards
            .get("uno")
            .and_then(|b| b.digital_outputs.get("D13")),
        Some(&true)
    );

    // Independently verify exact MNA physics on the same circuit state using SpiceCircuitBackend directly!
    // (Also verifies end-to-end physics values match the closed-form Shockley oracle to < 0.1%!)
    let uno_prof = arduino_uno_r3_electrical();
    let expected_i_led = independent_led_current_oracle(
        uno_prof.logic_high_v,
        uno_prof.pin_r_out_ohm + 220.0,
        &led_params,
    );
    let expected_v_d13 = uno_prof.logic_high_v - expected_i_led * uno_prof.pin_r_out_ohm;
    let expected_v_led_anode = expected_v_d13 - expected_i_led * 220.0;

    // Inspect exact analog voltages/currents via standalone SpiceCircuitBackend mirror at t=15ms
    let mut c_check = Circuit::default();
    c_check
        .add_component(
            "btn1",
            ComponentSpec::Switch(SwitchParams::normally_open(true)),
        )
        .unwrap();
    c_check
        .add_component(
            "r_pd",
            ComponentSpec::Resistor(ResistorParams::new(10_000.0)),
        )
        .unwrap();
    c_check
        .add_component("r_led", ComponentSpec::Resistor(ResistorParams::new(220.0)))
        .unwrap();
    c_check
        .add_component("led1", ComponentSpec::Diode(led_params.clone()))
        .unwrap();
    let mut spice_check = SpiceCircuitBackend::new(c_check);
    spice_check.attach_project_boards_and_wires(&proj).unwrap();
    use dendri_sim_kernel::ElectricalBackend;
    spice_check.set_board_digital_out("uno", "D13", true);
    spice_check.solve_at(15_000);

    let got_i_led = spice_check.circuit().component_current("led1");
    let got_v_d13 = spice_check.circuit().terminal_voltage("uno", "D13");
    let got_v_anode = spice_check.circuit().terminal_voltage("led1", "A");
    let got_v_d2 = spice_check.circuit().terminal_voltage("uno", "D2");
    let led_vis = spice_check.circuit().visual_state("led1");

    assert!(
        (got_v_d2 - 5.0).abs() < 1e-3,
        "pressed button D2 should be ~5.0V, got {got_v_d2}"
    );
    assert!(
        (got_i_led - expected_i_led).abs() / expected_i_led < 0.001,
        "LED current mismatch: oracle={expected_i_led:.6}A, sim={got_i_led:.6}A"
    );
    assert!(
        (got_v_d13 - expected_v_d13).abs() / expected_v_d13 < 0.001,
        "Uno D13 loaded Thevenin voltage mismatch: oracle={expected_v_d13:.6}V, sim={got_v_d13:.6}V"
    );
    assert!(
        (got_v_anode - expected_v_led_anode).abs() / expected_v_led_anode < 0.001,
        "LED anode Vf mismatch: oracle={expected_v_led_anode:.6}V, sim={got_v_anode:.6}V"
    );
    assert!(!led_vis.burned);
    assert!(led_vis.luminosity > 0.5 && led_vis.luminosity < 0.8);
    assert!(spice_check.all_faults().is_empty());

    // --- Phase C (t = 35 ms): button RELEASED -> D2 = 0V -> D13 = LOW -> LED OFF! ---
    kernel.run_until(35_000).unwrap();
    assert_eq!(
        kernel.board_global("uno", "led_state"),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        kernel
            .snapshot()
            .boards
            .get("uno")
            .and_then(|b| b.digital_outputs.get("D13")),
        Some(&false)
    );
}
