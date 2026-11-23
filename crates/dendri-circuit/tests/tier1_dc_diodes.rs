//! Phase 3a (Tier 1) Headless Analytical Physics Benchmarks (`02` §3.4):
//! Linear DC (R, V, I, potentiometers, NO/NC switches, breadboards, relays + inductive flyback),
//! Shockley Diodes / LEDs / Zeners, island partitioning, floating warnings, fault detection,
//! and end-to-end T-NUM-5 through the real MNA solver -> round-half-even -> S7 codec (`5529`).

use dendri_ast::{ChannelRange, Codec, CodecKind, PullMode, SignalKind};
use dendri_circuit::{
    BoardCircuitSpec, BoardPinSpec, Circuit, CircuitConfig, ComponentSpec, CurrentSourceDcParams,
    DiodeParams, FaultKind, PotentiometerParams, RelayParams, ResistorParams, SpiceCircuitBackend,
    SwitchParams, VoltageSourceDcParams, WarningKind, arduino_uno_r3_electrical,
    siemens_s7_1200_electrical,
};
use dendri_sim_kernel::ElectricalBackend;
use dendri_vm::ElectricalDigitalLevel;

#[test]
fn tier1_voltage_divider_potentiometer_switches_and_breadboard_exact() {
    let mut c = Circuit::default();
    c.add_component("gnd", ComponentSpec::Ground).unwrap();
    c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(10.0)),
    )
    .unwrap();
    c.add_component(
        "bb_vcc",
        ComponentSpec::BreadboardStrip {
            ports: vec!["p1".to_owned(), "p2".to_owned(), "p3".to_owned()],
        },
    )
    .unwrap();
    c.add_component(
        "sw_no",
        ComponentSpec::Switch(SwitchParams::normally_open(true)),
    )
    .unwrap();
    c.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(2_000.0)))
        .unwrap();
    c.add_component("r2", ComponentSpec::Resistor(ResistorParams::new(3_000.0)))
        .unwrap();
    c.add_component(
        "pot",
        ComponentSpec::Potentiometer(PotentiometerParams {
            r_total_ohm: 10_000.0,
            wiper: 0.25,
        }),
    )
    .unwrap();

    // Wire 10 V -> breadboard rail -> (sw_no -> r1 -> r2 -> gnd) AND (pot A..B -> gnd)
    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "bb_vcc", "p1");
    c.connect("bb_vcc", "p2", "sw_no", "A");
    c.connect("sw_no", "B", "r1", "A");
    c.connect("r1", "B", "r2", "A");
    c.connect("r2", "B", "gnd", "GND");

    c.connect("bb_vcc", "p3", "pot", "A");
    c.connect("pot", "B", "gnd", "GND");

    c.solve_dc_op();
    // Exact divider: 10 * 3000 / (2000 + 3000 + 0.01) = 5.999988 V (< 0.001% error vs 6.0 V)
    let v_div = c.terminal_voltage("r1", "B");
    assert!(
        (v_div - 6.0).abs() / 6.0 < 1e-4,
        "expected ~6.0 V, got {v_div}"
    );

    // Potentiometer wiper at 0.25 from A (10 V) to B (0 V) -> V(W) = 10 * (1 - 0.25) = 7.5 V exact!
    let v_wiper = c.terminal_voltage("pot", "W");
    assert!(
        (v_wiper - 7.5).abs() < 1e-6,
        "expected 7.5 V, got {v_wiper}"
    );

    // Sweep wiper to 0.80 -> V(W) = 2.0 V
    c.set_potentiometer_wiper("pot", 0.80).unwrap();
    c.solve_dc_op();
    assert!((c.terminal_voltage("pot", "W") - 2.0).abs() < 1e-6);

    // Open switch -> divider drops to ~0 V
    c.set_switch_closed("sw_no", false).unwrap();
    c.solve_dc_op();
    assert!(c.terminal_voltage("r1", "B") < 1e-3);
}

#[test]
fn tier1_shockley_diode_exact_iv_curve_within_0_01_percent() {
    // 5.0 V -> 1 kOhm -> 1N4148 -> GND
    let d_params = DiodeParams::silicon_1n4148();
    let mut c = Circuit::default();
    c.add_component("gnd", ComponentSpec::Ground).unwrap();
    c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    )
    .unwrap();
    c.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(1_000.0)))
        .unwrap();
    c.add_component("d1", ComponentSpec::Diode(d_params.clone()))
        .unwrap();

    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "r1", "A");
    c.connect("r1", "B", "d1", "A");
    c.connect("d1", "K", "gnd", "GND");
    c.solve_dc_op();

    let vd = c.terminal_voltage("d1", "A");
    let id = c.component_current("d1");
    // Independent analytical oracle: check KVL & Shockley equation simultaneously:
    //   V_j = V_d - I_d * R_s
    //   I_shockley = I_s * (exp(V_j / (N * V_T)) - 1)
    let vj = vd - id * d_params.r_s;
    let i_oracle = d_params.i_s * (libm::exp(vj / (d_params.n * 0.025_851_99)) - 1.0);
    let i_kvl = (5.0 - vd) / 1_000.0;
    assert!(
        (id - i_oracle).abs() / i_oracle < 1e-4,
        "Shockley mismatch: sim={id}, oracle={i_oracle}"
    );
    assert!(
        (id - i_kvl).abs() / i_kvl < 1e-4,
        "KVL mismatch: sim={id}, kvl={i_kvl}"
    );
    assert!(
        (0.60..0.75).contains(&vd),
        "silicon diode Vf out of range: {vd}"
    );
}

#[test]
fn tier1_led_luminosity_burn_reverse_breakdown_and_resistor_overpower() {
    // 1. Healthy LED: 5 V -> 220 Ohm -> Red LED -> GND (~14 mA, luminosity ~0.7, zero faults)
    let mut c_ok = Circuit::default();
    c_ok.add_component("gnd", ComponentSpec::Ground).unwrap();
    c_ok.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    )
    .unwrap();
    c_ok.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(220.0)))
        .unwrap();
    c_ok.add_component("led1", ComponentSpec::Diode(DiodeParams::led_red()))
        .unwrap();
    c_ok.connect("v1", "NEG", "gnd", "GND");
    c_ok.connect("v1", "POS", "r1", "A");
    c_ok.connect("r1", "B", "led1", "A");
    c_ok.connect("led1", "K", "gnd", "GND");
    let rep_ok = c_ok.solve_dc_op();
    assert!(rep_ok.faults.is_empty());
    let vs_ok = c_ok.visual_state("led1");
    assert!(!vs_ok.burned);
    assert!(
        (0.55..0.85).contains(&vs_ok.luminosity),
        "got lum={}",
        vs_ok.luminosity
    );
    assert!(
        (0.012..0.016).contains(&vs_ok.current_a),
        "got i={}",
        vs_ok.current_a
    );

    // 2. Overcurrent burn + resistor overpower: 5 V -> 10 Ohm (0.25 W) -> Red LED (> 100 mA > 30 mA!)
    let mut c_burn = Circuit::default();
    c_burn.add_component("gnd", ComponentSpec::Ground).unwrap();
    c_burn
        .add_component(
            "v1",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
        )
        .unwrap();
    c_burn
        .add_component(
            "r1",
            ComponentSpec::Resistor(ResistorParams::with_power(10.0, 0.05)),
        )
        .unwrap();
    c_burn
        .add_component("led1", ComponentSpec::Diode(DiodeParams::led_red()))
        .unwrap();
    c_burn.connect("v1", "NEG", "gnd", "GND");
    c_burn.connect("v1", "POS", "r1", "A");
    c_burn.connect("r1", "B", "led1", "A");
    c_burn.connect("led1", "K", "gnd", "GND");
    let rep_burn = c_burn.solve_dc_op();
    assert!(c_burn.visual_state("led1").burned);
    assert_eq!(c_burn.visual_state("led1").luminosity, 0.0);
    assert!(
        rep_burn
            .faults
            .iter()
            .any(|f| matches!(f.kind, FaultKind::LedBurned { .. }))
    );
    assert!(
        rep_burn
            .faults
            .iter()
            .any(|f| matches!(f.kind, FaultKind::ResistorOverpower { .. }))
    );

    // 3. Reverse LED breakdown: 12 V reversed across Red LED (V_br = 5 V)
    let mut c_rev = Circuit::default();
    c_rev.add_component("gnd", ComponentSpec::Ground).unwrap();
    c_rev
        .add_component(
            "v1",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(12.0)),
        )
        .unwrap();
    c_rev
        .add_component("r1", ComponentSpec::Resistor(ResistorParams::new(1_000.0)))
        .unwrap();
    c_rev
        .add_component("led1", ComponentSpec::Diode(DiodeParams::led_red()))
        .unwrap();
    c_rev.connect("v1", "NEG", "gnd", "GND");
    c_rev.connect("v1", "POS", "r1", "A");
    c_rev.connect("r1", "B", "led1", "K"); // Reversed!
    c_rev.connect("led1", "A", "gnd", "GND");
    let rep_rev = c_rev.solve_dc_op();
    assert!(c_rev.visual_state("led1").burned);
    assert!(
        rep_rev
            .faults
            .iter()
            .any(|f| matches!(f.kind, FaultKind::LedReverseBreakdown { .. }))
    );
}

#[test]
fn tier1_zener_5v1_regulator_holds_output_voltage() {
    // 12 V -> 100 Ohm -> 5.1 V Zener (cathode at output, anode at GND) || 1 kOhm load
    let mut c = Circuit::default();
    c.add_component("gnd", ComponentSpec::Ground).unwrap();
    c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(12.0)),
    )
    .unwrap();
    c.add_component(
        "r_ser",
        ComponentSpec::Resistor(ResistorParams::with_power(100.0, 2.0)),
    )
    .unwrap();
    c.add_component("z1", ComponentSpec::Diode(DiodeParams::zener(5.1)))
        .unwrap();
    c.add_component(
        "r_load",
        ComponentSpec::Resistor(ResistorParams::new(1_000.0)),
    )
    .unwrap();

    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "r_ser", "A");
    c.connect("r_ser", "B", "z1", "K");
    c.connect("r_ser", "B", "r_load", "A");
    c.connect("z1", "A", "gnd", "GND");
    c.connect("r_load", "B", "gnd", "GND");

    c.solve_dc_op();
    let v_out = c.terminal_voltage("r_load", "A");
    assert!(
        (5.10..5.35).contains(&v_out),
        "5.1V Zener regulator should clamp near ~5.2V under 60mA, got {v_out} V"
    );
}

#[test]
fn tier1_relay_inductive_flyback_spike_without_vs_with_flyback_diode() {
    // Helper builds: 12 V -> relay coil (100 Ohm, 50 mH) -> switch -> GND,
    // with optional 1N4148 flyback diode across coil (A at COIL_B, K at COIL_A = 12 V).
    let run_relay_turn_off = |with_flyback: bool| -> (f64, bool) {
        let mut c = Circuit::new(CircuitConfig {
            max_transient_dt_us: 10,
            ..CircuitConfig::default()
        });
        c.add_component("gnd", ComponentSpec::Ground).unwrap();
        c.add_component(
            "v1",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(12.0)),
        )
        .unwrap();
        c.add_component("k1", ComponentSpec::Relay(RelayParams::default()))
            .unwrap();
        c.add_component(
            "sw",
            ComponentSpec::Switch(SwitchParams::normally_open(true)),
        )
        .unwrap();
        // Isolated load island on relay NO contact
        c.add_component("gnd2", ComponentSpec::Ground).unwrap();
        c.add_component(
            "v2",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(24.0)),
        )
        .unwrap();
        c.add_component(
            "r_load",
            ComponentSpec::Resistor(ResistorParams::with_power(1_000.0, 2.0)),
        )
        .unwrap();

        c.connect("v1", "NEG", "gnd", "GND");
        c.connect("v1", "POS", "k1", "COIL_A");
        c.connect("k1", "COIL_B", "sw", "A");
        c.connect("sw", "B", "gnd", "GND");

        c.connect("v2", "NEG", "gnd2", "GND");
        c.connect("v2", "POS", "k1", "COM");
        c.connect("k1", "NO", "r_load", "A");
        c.connect("r_load", "B", "gnd2", "GND");

        if with_flyback {
            c.add_component("d_fly", ComponentSpec::Diode(DiodeParams::silicon_1n4148()))
                .unwrap();
            c.connect("d_fly", "A", "k1", "COIL_B");
            c.connect("d_fly", "K", "k1", "COIL_A");
        }

        c.add_probe(
            "v_coil_b",
            ("k1".to_owned(), "COIL_B".to_owned()),
            None,
            100,
        );

        // Steady state DC: coil carries 120 mA, relay energized -> NO closed -> V(r_load.A) == 24 V!
        c.solve_dc_op();
        let load_on = c.terminal_voltage("r_load", "A") > 23.5;

        // Open switch at t = 0 -> step 30 us and read peak flyback voltage on COIL_B via probe!
        c.set_switch_closed("sw", false).unwrap();
        c.step_to(30);
        let v_spike = c.multimeter_reading("v_coil_b", 1).v_max;
        (v_spike, load_on)
    };

    let (v_spike_bare, load_on_bare) = run_relay_turn_off(false);
    let (v_spike_clamped, load_on_clamped) = run_relay_turn_off(true);

    assert!(
        load_on_bare && load_on_clamped,
        "energized relay must close NO contact"
    );
    assert!(
        v_spike_bare > 100.0,
        "unclamped inductive relay turn-off must generate a real flyback spike (>100V), got {v_spike_bare} V"
    );
    assert!(
        v_spike_clamped < 13.5,
        "flyback diode must clamp inductive spike near Vcc + Vf (~12.8V), got {v_spike_clamped} V"
    );
}

#[test]
fn tier1_islands_floating_warning_pin_overvoltage_and_t_num_5_end_to_end() {
    // 1. Floating subcircuit emits WarningKind::FloatingSubcircuit and solves non-singularly
    let mut c_float = Circuit::default();
    c_float
        .add_component(
            "v1",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
        )
        .unwrap();
    c_float
        .add_component("r1", ComponentSpec::Resistor(ResistorParams::new(1_000.0)))
        .unwrap();
    c_float.connect("v1", "POS", "r1", "A");
    c_float.connect("v1", "NEG", "r1", "B");
    let rep = c_float.solve_dc_op();
    assert!(
        rep.warnings
            .iter()
            .any(|w| matches!(w, WarningKind::FloatingSubcircuit { .. }))
    );
    let dv = c_float.terminal_voltage("r1", "A") - c_float.terminal_voltage("r1", "B");
    assert!((dv - 5.0).abs() < 1e-6);

    // 2. Pin overvoltage fault: wiring 24 V directly to a 5 V Arduino Uno pin
    let mut backend = SpiceCircuitBackend::new(Circuit::default());
    backend
        .attach_board(BoardCircuitSpec {
            board_id: "uno".to_owned(),
            profile: arduino_uno_r3_electrical(),
            pins: vec![BoardPinSpec {
                pin: "D2".to_owned(),
                is_output: false,
                is_analog: false,
                is_pwm: false,
                pull: PullMode::None,
                analog_range: None,
            }],
        })
        .unwrap();
    backend
        .circuit_mut()
        .add_component(
            "v24",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(24.0)),
        )
        .unwrap();
    backend.circuit_mut().connect("v24", "NEG", "uno", "GND");
    backend.circuit_mut().connect("v24", "POS", "uno", "D2");
    backend.solve_at(0);
    assert!(
        backend
            .all_faults()
            .iter()
            .any(|f| matches!(f.kind, FaultKind::PinOvervoltage { ref pin, .. } if pin == "D2")),
        "wiring 24V to a 5V Uno pin MUST trigger PinOvervoltage!"
    );
    assert_eq!(
        backend.read_board_digital_in("uno", "D2"),
        ElectricalDigitalLevel::High
    );

    // 3. T-NUM-5 through REAL SPICE MNA solver:
    //    4 mA current source -> 500 Ohm shunt resistor -> S7-1200 IW64 (0-10V AI, R_in = 100kOhm)
    //    Note: With R_in = 100 kOhm in parallel with 500 Ohm, use exact 500 Ohm parallel-compensated
    //    or high-impedance 10 MOhm AI input: verify exact 2.000 V -> a = 200_000_000 -> S7 code 5529!
    let mut s7_prof = siemens_s7_1200_electrical();
    s7_prof.pin_r_in_ohm = 10_000_000.0; // High-Z voltage input (error < 0.005%)
    let mut s7_backend = SpiceCircuitBackend::new(Circuit::default());
    s7_backend
        .attach_board(BoardCircuitSpec {
            board_id: "plc".to_owned(),
            profile: s7_prof,
            pins: vec![BoardPinSpec {
                pin: "IW64".to_owned(),
                is_output: false,
                is_analog: true,
                is_pwm: false,
                pull: PullMode::None,
                analog_range: Some(ChannelRange {
                    signal: SignalKind::Voltage,
                    low: 0.0,
                    high: 10.0,
                    live_zero: false,
                    hardware_normalizes_live_zero: false,
                }),
            }],
        })
        .unwrap();
    s7_backend
        .circuit_mut()
        .add_component(
            "i_4ma",
            ComponentSpec::CurrentSourceDc(CurrentSourceDcParams { current_a: 0.004 }),
        )
        .unwrap();
    s7_backend
        .circuit_mut()
        .add_component(
            "r_shunt",
            ComponentSpec::Resistor(ResistorParams::new(500.0)),
        )
        .unwrap();
    // 4 mA flows OUT of i_4ma.OUT into top of 500 Ohm shunt -> returns via GND to i_4ma.IN
    s7_backend
        .circuit_mut()
        .connect("i_4ma", "IN", "plc", "GND");
    s7_backend
        .circuit_mut()
        .connect("i_4ma", "OUT", "r_shunt", "A");
    s7_backend
        .circuit_mut()
        .connect("r_shunt", "A", "plc", "IW64");
    s7_backend
        .circuit_mut()
        .connect("r_shunt", "B", "plc", "GND");
    s7_backend.solve_at(0);

    let v_shunt = s7_backend.circuit().terminal_voltage("plc", "IW64");
    assert!(
        (v_shunt - 2.0).abs() < 2e-4,
        "expected ~2.0 V, got {v_shunt}"
    );
    let reading = s7_backend.read_board_analog_in("plc", "IW64");
    let s7_codec = Codec::new("s7_ai", CodecKind::Span, 27_648, 27_648).unwrap();
    let code = s7_codec.to_code_unipolar(u64::try_from(reading.analog).unwrap());
    assert_eq!(
        code, 5_529,
        "T-NUM-5 through MNA solver MUST produce S7 code 5529!"
    );
}
