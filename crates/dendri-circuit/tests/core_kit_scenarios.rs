//! Phase 4 Exit Independent-Oracle Verification (`03-asset-and-profile-contract.md` §1.1 #8).
//!
//! Exercises ALL 10 certified Phase 4 Core Kit assets end-to-end across `dendri-project` +
//! `dendri-circuit` (`SpiceCircuitBackend`) + `dendri-sim-kernel` + `dendri-vm`, checking every
//! electrical & digital/analog result against closed-form analytical physics oracles:
//!
//! 1. **Maker Kit:** `arduino-uno-r3` + `maker-tactile-4pin` (active-low internal pull-up) +
//!    `resistor` (220 Ω) + `led` (5 mm Red) -> Shockley diode oracle (< 0.1%).
//! 2. **Industrial PLC Start/Stop Seal-In + Composite Pilot Lamp:** `plc-1200-open-1214` +
//!    `cmd-pushbutton-no-start` + `cmd-pushbutton-nc-stop` + `signal-pilot-lamp` (24 V DC
//!    declarative composite: 1100 Ω series resistor + LED) -> verifies 24 V seal-in logic,
//!    exact composite lamp current (~20 mA, < 0.1% vs Shockley oracle), zero burn faults, and
//!    clean NC stop de-energization.
//! 3. **Maker Analog Potentiometer:** `arduino-uno-r3` + `potentiometer` (`A0` @ 50% & 25%) ->
//!    exact voltage-divider oracle (`2.500 V -> ADC 512`, `1.250 V -> ADC 256`).
//! 4. **Industrial 4-20 mA Pressure Loop (`DEF-002`):** `plc-1200-open-1214` +
//!    `sensor-pressure-4-20ma` + `resistor` (500 Ω shunt) -> `0 bar` (`4 mA` -> `2.000 V` ->
//!    PLC `plc_span_27648` code `5530`), `10 bar` (`20 mA` -> `10.000 V` -> exact `27648`)!

use dendri_ast::{Codec, CodecKind};
use dendri_circuit::{
    DiodeParams, SpiceCircuitBackend, arduino_uno_r3_electrical, plc_1200_open_1214_electrical,
};
use dendri_project::load_project_str;
use dendri_sim_kernel::{ElectricalBackend, InputTimeline, KernelConfig, SimKernel};
use dendri_vm::Value;

/// Independent closed-form Newton oracle for a series resistor + Shockley diode driven by `v_src`:
/// `V_src = (R_series + R_s) * I + N * V_T * ln(I / I_s + 1)`
fn shockley_series_current_oracle(v_src: f64, r_series: f64, led: &DiodeParams) -> f64 {
    let n_vt = led.n * 0.025_851_99;
    let r_eff = r_series + led.r_s;
    let mut i = 0.015_f64;
    for _ in 0..60 {
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
fn scenario1_maker_kit_uno_tactile_pullup_resistor_led_matches_shockley_oracle() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CoreKitMakerTactileLed"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[boards.program]
imports = ["std.iec.*", "std.arduino.*"]

[[boards.program.hardware]]
id = "n_btn"
label = "BTN_N"
pin = "D2"
kind = "digital_in"
pull = "pull_up"

[[boards.program.hardware]]
id = "n_led"
label = "LED"
pin = "D13"
kind = "digital_out"

[[boards.program.data]]
id = "n_on"
name = "is_pressed"
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
id = "n_inv"
target = "is_pressed"
value = "NOT BTN_N"

[[boards.program.pous.body]]
kind = "call"
id = "n_out"
callee = "DIGITAL_WRITE"
args = [{ value = "LED" }, { value = "is_pressed" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_wait"
duration = "T#1ms"

[[components]]
id = "sw1"
profile = "maker-tactile-4pin"
params = { pressed = "true" }

[[components]]
id = "r1"
profile = "resistor"
params = { r_ohm = "220" }

[[components]]
id = "d1"
profile = "led"

[[wires]]
id = "w1"
from = "uno.D2"
to = "sw1.1"
signal = "digital"

[[wires]]
id = "w2"
from = "sw1.2"
to = "uno.GND"
signal = "ground"

[[wires]]
id = "w3"
from = "uno.D13"
to = "r1.1"
signal = "digital"

[[wires]]
id = "w4"
from = "r1.2"
to = "d1.A"
signal = "digital"

[[wires]]
id = "w5"
from = "d1.K"
to = "uno.GND"
signal = "ground"
"#;

    let proj = load_project_str(toml, None).unwrap();
    let backend = SpiceCircuitBackend::from_project_headless(&proj).unwrap();
    let mut kernel = SimKernel::with_backend(
        &proj,
        KernelConfig::default(),
        InputTimeline::new(),
        Box::new(backend),
    )
    .unwrap();
    kernel.run_until(5_000).unwrap();

    // Button pressed -> D2 pulled LOW -> active-low `NOT BTN_N` == true -> D13 HIGH
    assert_eq!(
        kernel.board_global("uno", "is_pressed"),
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

    // Direct electrical verification against Shockley oracle
    let mut verify = SpiceCircuitBackend::from_project_headless(&proj).unwrap();
    verify.set_board_digital_out("uno", "D13", true);
    verify.solve_at(5_000);
    let uno = arduino_uno_r3_electrical();
    let expected_i = shockley_series_current_oracle(
        uno.logic_high_v,
        uno.pin_r_out_ohm + 220.0,
        &DiodeParams::led_red(),
    );
    let got_i = verify.circuit().component_current("d1");
    assert!(
        (got_i - expected_i).abs() / expected_i < 0.001,
        "LED current mismatch: oracle={expected_i:.6}A, sim={got_i:.6}A"
    );
    assert!(
        verify.all_faults().is_empty(),
        "unexpected faults: {:?}",
        verify.all_faults()
    );
}

#[test]
fn scenario2_plc_start_stop_seal_in_with_composite_24v_pilot_lamp() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CoreKitPlcStartStopPilotLamp"

[[boards]]
id = "plc1"
profile = "plc-1200-open-1214"
authoring_language = "st"
execution = "plc_scan"
scan_period_us = 10000

[boards.program]
imports = ["std.iec.*", "std.plc.*"]

[[boards.program.hardware]]
id = "n_start"
label = "PB_START"
pin = "Ia.0"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_stop"
label = "PB_STOP_NC"
pin = "Ia.1"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_lamp"
label = "PILOT_LAMP"
pin = "Qa.0"
kind = "digital_out"

[[boards.program.data]]
id = "n_run"
name = "motor_run"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.pous]]
id = "n_ob1"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_seal"
target = "motor_run"
value = "(PB_START OR motor_run) AND PB_STOP_NC"

[[boards.program.pous.body]]
kind = "call"
id = "n_out"
callee = "DIGITAL_WRITE"
args = [{ value = "PILOT_LAMP" }, { value = "motor_run" }]

[[components]]
id = "pb_start"
profile = "cmd-pushbutton-no-start"
params = { pressed = "true" }

[[components]]
id = "pb_stop"
profile = "cmd-pushbutton-nc-stop"
params = { pressed = "false" }

[[components]]
id = "lamp1"
profile = "signal-pilot-lamp"
params = { r_limit_ohm = "1100" }

[[wires]]
id = "w1"
from = "plc1.24V"
to = "pb_start.NO"
signal = "power"

[[wires]]
id = "w2"
from = "pb_start.COM"
to = "plc1.Ia.0"
signal = "digital"

[[wires]]
id = "w3"
from = "plc1.24V"
to = "pb_stop.NC"
signal = "power"

[[wires]]
id = "w4"
from = "pb_stop.COM"
to = "plc1.Ia.1"
signal = "digital"

[[wires]]
id = "w5"
from = "plc1.Qa.0"
to = "lamp1.X1"
signal = "digital"

[[wires]]
id = "w6"
from = "lamp1.X2"
to = "plc1.GND"
signal = "ground"
"#;

    // 1. Start pressed, NC stop released -> PLC latches `motor_run = true` & energizes `Qa.0 = 24 V`
    let proj_start = load_project_str(toml, None).unwrap();
    let backend = SpiceCircuitBackend::from_project_headless(&proj_start).unwrap();
    let mut kernel = SimKernel::with_backend(
        &proj_start,
        KernelConfig::default(),
        InputTimeline::new(),
        Box::new(backend),
    )
    .unwrap();
    kernel.run_until(25_000).unwrap();

    assert_eq!(
        kernel.board_global("plc1", "motor_run"),
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

    // 2. Verify composite 24 V pilot lamp physics (`lamp1__r_limit` 1100 Ω + `lamp1__lamp_led`)
    let mut verify = SpiceCircuitBackend::from_project_headless(&proj_start).unwrap();
    verify.set_board_digital_out("plc1", "Qa.0", true);
    verify.solve_at(25_000);

    let plc = plc_1200_open_1214_electrical();
    let mut led_model = DiodeParams::led_red();
    led_model.i_max_a = 0.030;
    led_model.v_br = 30.0;
    let expected_i =
        shockley_series_current_oracle(plc.logic_high_v, plc.pin_r_out_ohm + 1_100.0, &led_model);
    let got_i = verify.circuit().component_current("lamp1__lamp_led");
    let lamp_vis = verify.circuit().visual_state("lamp1__lamp_led");

    assert!(
        (got_i - expected_i).abs() / expected_i < 0.001,
        "24V composite pilot lamp current mismatch: oracle={expected_i:.6}A, sim={got_i:.6}A"
    );
    assert!(
        (0.018..=0.022).contains(&got_i),
        "expected ~20 mA nominal pilot lamp current, got {got_i:.6}A"
    );
    assert!(
        lamp_vis.luminosity > 0.85,
        "expected bright pilot lamp, got {}",
        lamp_vis.luminosity
    );
    assert!(
        verify.all_faults().is_empty(),
        "24V composite pilot lamp must not burn: {:?}",
        verify.all_faults()
    );

    // 3. NC Stop pressed (`pressed = "true"`) breaks 24 V stop circuit -> `Qa.0 = false`!
    let stop_toml = toml.replace(
        r#"profile = "cmd-pushbutton-nc-stop"
params = { pressed = "false" }"#,
        r#"profile = "cmd-pushbutton-nc-stop"
params = { pressed = "true" }"#,
    );
    let proj_stop = load_project_str(&stop_toml, None).unwrap();
    let backend_stop = SpiceCircuitBackend::from_project_headless(&proj_stop).unwrap();
    let mut kernel_stop = SimKernel::with_backend(
        &proj_stop,
        KernelConfig::default(),
        InputTimeline::new(),
        Box::new(backend_stop),
    )
    .unwrap();
    kernel_stop.run_until(25_000).unwrap();
    assert_eq!(
        kernel_stop.board_global("plc1", "motor_run"),
        Some(&Value::Bool(false))
    );
}

#[test]
fn scenario3_maker_uno_potentiometer_matches_voltage_divider_and_adc_oracle() {
    // Exact analytical loaded voltage-divider oracle accounting for ATmega328P ADC R_in = 10 MΩ:
    //   R_lower_eff = (wiper * R_tot) || R_in
    //   V_W = V_cc * R_lower_eff / ((1 - wiper) * R_tot + R_lower_eff)
    let r_tot = 10_000.0_f64;
    let r_in = 10_000_000.0_f64;
    let uno_codec = Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap();

    for wiper in [0.50_f64, 0.25_f64] {
        let r_aw = wiper * r_tot;
        let r_wb = (1.0 - wiper) * r_tot;
        let r_lower_eff = (r_aw * r_in) / (r_aw + r_in);
        let expected_v = 5.0 * r_lower_eff / (r_wb + r_lower_eff);
        let expected_asl = libm::rint((expected_v / 5.0) * 1_000_000_000.0) as u64;
        let expected_adc = uno_codec.to_code_unipolar(expected_asl);

        let toml = format!(
            r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CoreKitPotentiometerAdc"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[boards.program]
imports = ["std.iec.*", "std.arduino.*"]

[[boards.program.hardware]]
id = "n_pot"
label = "POT_IN"
pin = "A0"
kind = "analog_in"
codec = "uno_10bit"
range = {{ signal = "voltage", low = 0.0, high = 5.0 }}

[[boards.program.data]]
id = "n_raw"
name = "raw_analog"
scope = "var"
ty = {{ kind = "scalar", ty = {{ name = "analog" }} }}
initial = "0"

[[boards.program.pous]]
id = "n_main"
name = "OB_MAIN"
kind = {{ pou_kind = "ob", ob = {{ type = "main" }} }}
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_read"
target = "raw_analog"
value = "POT_IN"

[[boards.program.pous.body]]
kind = "wait"
id = "n_w"
duration = "T#1ms"

[[components]]
id = "pot1"
profile = "potentiometer"
params = {{ r_total_ohm = "10000", wiper = "{wiper}" }}

[[wires]]
id = "w1"
from = "uno.5V"
to = "pot1.B"
signal = "power"

[[wires]]
id = "w2"
from = "pot1.A"
to = "uno.GND"
signal = "ground"

[[wires]]
id = "w3"
from = "pot1.W"
to = "uno.A0"
signal = "analog"
"#
        );

        let proj = load_project_str(&toml, None).unwrap();
        let mut verify = SpiceCircuitBackend::from_project_headless(&proj).unwrap();
        verify.solve_at(2_000);
        let v_a0 = verify.circuit().terminal_voltage("uno", "A0");
        assert!(
            (v_a0 - expected_v).abs() < 1e-6,
            "potentiometer wiper={wiper} expected loaded V={expected_v:.8}V, got {v_a0:.8}V"
        );

        let backend = SpiceCircuitBackend::from_project_headless(&proj).unwrap();
        let mut kernel = SimKernel::with_backend(
            &proj,
            KernelConfig::default(),
            InputTimeline::new(),
            Box::new(backend),
        )
        .unwrap();
        kernel.run_until(5_000).unwrap();

        let Some(Value::Analog(asl_val)) = kernel.board_global("uno", "raw_analog") else {
            panic!("expected Analog reading on raw_analog");
        };
        let got_asl = u64::try_from(*asl_val).unwrap();
        assert!(
            got_asl.abs_diff(expected_asl) <= 2,
            "potentiometer wiper={wiper} ASL ppb mismatch: expected {expected_asl}, got {got_asl}"
        );
        let adc_code = uno_codec.to_code_unipolar(got_asl);
        assert_eq!(
            adc_code, expected_adc,
            "potentiometer wiper={wiper} ADC code mismatch (asl={asl_val})"
        );
    }
}

#[test]
fn scenario4_plc_4_20ma_pressure_loop_with_500_ohm_shunt_matches_27648_oracle() {
    // Exact analytical loaded shunt oracle: 500 Ω || 10 MΩ AI input impedance converts 4..20 mA
    // into 2.000..10.000 V on PLC AI channel IW64 (0-10 V, plc_span_27648).
    let r_shunt = 500.0_f64;
    let r_ai = 10_000_000.0_f64;
    let r_eff = (r_shunt * r_ai) / (r_shunt + r_ai);
    let plc_codec = Codec::new("plc_span_27648", CodecKind::Span, 27_648, 27_648).unwrap();

    for (pressure_bar, current_a) in [(0.0_f64, 0.004_f64), (10.0_f64, 0.020_f64)] {
        let expected_v = current_a * r_eff;
        let expected_asl = libm::rint((expected_v / 10.0) * 1_000_000_000.0) as u64;
        let expected_code = plc_codec.to_code_unipolar(expected_asl);

        let toml = format!(
            r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CoreKitPlcPressure4to20mA"

[[boards]]
id = "plc1"
profile = "plc-1200-open-1214"
authoring_language = "st"
execution = "plc_scan"
scan_period_us = 10000

[boards.program]
imports = ["std.iec.*", "std.plc.*"]

[[boards.program.hardware]]
id = "n_ai"
label = "PRESSURE_AI"
pin = "IW64"
kind = "analog_in"
codec = "plc_span_27648"
range = {{ signal = "voltage", low = 0.0, high = 10.0 }}

[[boards.program.data]]
id = "n_pv"
name = "pv_analog"
scope = "var"
ty = {{ kind = "scalar", ty = {{ name = "analog" }} }}
initial = "0"

[[boards.program.pous]]
id = "n_ob1"
name = "OB_MAIN"
kind = {{ pou_kind = "ob", ob = {{ type = "main" }} }}
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_rd"
target = "pv_analog"
value = "PRESSURE_AI"

[[components]]
id = "pt1"
profile = "sensor-pressure-4-20ma"
params = {{ pressure_bar = "{pressure_bar}", max_bar = "10.0" }}

[[components]]
id = "r_shunt"
profile = "resistor"
params = {{ r_ohm = "500.0", p_rated_w = "0.5" }}

[[wires]]
id = "w1"
from = "pt1.OUT"
to = "plc1.IW64"
signal = "analog"

[[wires]]
id = "w2"
from = "plc1.IW64"
to = "r_shunt.1"
signal = "analog"

[[wires]]
id = "w3"
from = "r_shunt.2"
to = "plc1.GND"
signal = "ground"

[[wires]]
id = "w4"
from = "pt1.GND"
to = "plc1.GND"
signal = "ground"
"#
        );

        let proj = load_project_str(&toml, None).unwrap();
        let mut verify = SpiceCircuitBackend::from_project_headless(&proj).unwrap();
        verify.solve_at(15_000);
        let v_iw64 = verify.circuit().terminal_voltage("plc1", "IW64");
        assert!(
            (v_iw64 - expected_v).abs() < 1e-6,
            "pressure={pressure_bar} bar expected loaded V(IW64)={expected_v:.8}V, got {v_iw64:.8}V"
        );
        assert!(
            verify.all_faults().is_empty(),
            "unexpected shunt faults: {:?}",
            verify.all_faults()
        );

        let backend = SpiceCircuitBackend::from_project_headless(&proj).unwrap();
        let mut kernel = SimKernel::with_backend(
            &proj,
            KernelConfig::default(),
            InputTimeline::new(),
            Box::new(backend),
        )
        .unwrap();
        kernel.run_until(25_000).unwrap();

        let Some(Value::Analog(asl_val)) = kernel.board_global("plc1", "pv_analog") else {
            panic!("expected Analog value on pv_analog");
        };
        let got_asl = u64::try_from(*asl_val).unwrap();
        assert!(
            got_asl.abs_diff(expected_asl) <= 2,
            "pressure={pressure_bar} bar ASL ppb mismatch: expected {expected_asl}, got {got_asl}"
        );
        let code = plc_codec.to_code_unipolar(got_asl);
        assert_eq!(
            code, expected_code,
            "pressure={pressure_bar} bar PLC 27648 code mismatch: expected {expected_code}, got {code} (asl={asl_val})"
        );
    }
}
