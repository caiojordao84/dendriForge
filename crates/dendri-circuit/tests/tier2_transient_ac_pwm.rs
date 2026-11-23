//! Phase 3b (Tier 2) Headless Analytical Physics Benchmarks (`02` §3.4, `error < 0.1%`):
//! - RC step charging curve (`V_C(t) = V_0 * (1 - exp(-t/RC))`)
//! - RL step current response (`I_L(t) = (V/R) * (1 - exp(-Rt/L))`)
//! - LC undamped resonant tank (`f_0 = 1 / (2*pi*sqrt(LC))`)
//! - Mutually coupled transformer (`V_2 / V_1 = k * sqrt(L_2 / L_1)`)
//! - Purely resistive 1φ AC (ZERO C/L — verifying transient stepping on continuous AC sources!)
//! - 3φ balanced Wye AC (`0°, -120°, +120°`, `V_LL = sqrt(3) * V_LN`, `I_N ≈ 0`)
//! - Arduino Uno R3 PWM (`DEF-001`) Timer 0 Fast PWM & Timer 1 Phase-Correct PWM into RC low-pass filter

use dendri_ast::{Codec, CodecKind, PullMode};
use dendri_circuit::{
    BoardCircuitSpec, BoardPinSpec, CapacitorParams, Circuit, CircuitConfig, ComponentSpec,
    InductorParams, ResistorParams, SpiceCircuitBackend, SwitchParams, TransformerParams,
    VoltageSourceAcParams, VoltageSourceDcParams, arduino_uno_r3_electrical,
};
use dendri_sim_kernel::ElectricalBackend;

#[test]
fn tier2_rc_step_charging_matches_closed_form_within_0_1_percent() {
    // 5.0 V -> switch (starts open at t=0, closes at t=0) -> R = 1 kOhm -> C = 1 uF -> GND
    // Time constant tau = R * C = 1_000 us (1 ms).
    let mut c = Circuit::new(CircuitConfig {
        max_transient_dt_us: 10,
        ..CircuitConfig::default()
    });
    c.add_component("gnd", ComponentSpec::Ground).unwrap();
    c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    )
    .unwrap();
    c.add_component(
        "sw",
        ComponentSpec::Switch(SwitchParams::normally_open(false)),
    )
    .unwrap();
    c.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(1_000.0)))
        .unwrap();
    c.add_component("c1", ComponentSpec::Capacitor(CapacitorParams::ideal(1e-6)))
        .unwrap();

    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "sw", "A");
    c.connect("sw", "B", "r1", "A");
    c.connect("r1", "B", "c1", "A");
    c.connect("c1", "B", "gnd", "GND");

    // Use Initial Conditions (UIC): V_C(0) = 0.0 V, then close switch at t = 0 and check 0.5..3 tau
    c.use_initial_conditions();
    c.set_switch_closed("sw", true).unwrap();
    for &t_us in &[500u64, 1_000, 2_000, 3_000] {
        c.step_to(t_us);
        let t_s = (t_us as f64) * 1e-6;
        let expected = 5.0 * (1.0 - libm::exp(-t_s / 1e-3));
        let got = c.terminal_voltage("c1", "A");
        let rel_err = (got - expected).abs() / expected;
        assert!(
            rel_err < 0.001,
            "RC at t={t_us}us: expected {expected:.6} V, got {got:.6} V (rel err = {:.4}%)",
            rel_err * 100.0
        );
    }
}

#[test]
fn tier2_rl_step_current_matches_closed_form_within_0_1_percent() {
    // 10.0 V -> switch (open at t=0, closes at t=0) -> R = 100 Ohm -> L = 10 mH -> GND
    // Time constant tau = L / R = 100 us; I_inf = 0.100 A.
    let mut c = Circuit::new(CircuitConfig {
        max_transient_dt_us: 1,
        ..CircuitConfig::default()
    });
    c.add_component("gnd", ComponentSpec::Ground).unwrap();
    c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(10.0)),
    )
    .unwrap();
    c.add_component(
        "sw",
        ComponentSpec::Switch(SwitchParams::normally_open(false)),
    )
    .unwrap();
    c.add_component(
        "r1",
        ComponentSpec::Resistor(ResistorParams::with_power(100.0, 5.0)),
    )
    .unwrap();
    c.add_component(
        "l1",
        ComponentSpec::Inductor(InductorParams::new(0.010, 0.0)),
    )
    .unwrap();

    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "sw", "A");
    c.connect("sw", "B", "r1", "A");
    c.connect("r1", "B", "l1", "A");
    c.connect("l1", "B", "gnd", "GND");

    c.solve_dc_op();
    assert!(c.component_current("l1").abs() < 1e-6);

    c.set_switch_closed("sw", true).unwrap();
    for &t_us in &[50u64, 100, 200, 300] {
        c.step_to(t_us);
        let t_s = (t_us as f64) * 1e-6;
        let expected = 0.100 * (1.0 - libm::exp(-t_s / 1e-4));
        let got = c.component_current("l1");
        let rel_err = (got - expected).abs() / expected;
        assert!(
            rel_err < 0.001,
            "RL at t={t_us}us: expected {expected:.6} A, got {got:.6} A (rel err = {:.4}%)",
            rel_err * 100.0
        );
    }
}

#[test]
fn tier2_lc_resonant_tank_matches_natural_frequency_within_0_1_percent() {
    // L = 1 mH, C = 1 uF -> omega_0 = 1 / sqrt(1e-9) = 31622.7766 rad/s, T = 198.6918 us.
    // Charge C to 10 V via switch, open switch at t = 0, observe cosine oscillation!
    let mut c = Circuit::new(CircuitConfig {
        max_transient_dt_us: 1,
        ..CircuitConfig::default()
    });
    c.add_component("gnd", ComponentSpec::Ground).unwrap();
    c.add_component(
        "v_init",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(10.0)),
    )
    .unwrap();
    c.add_component(
        "sw_chg",
        ComponentSpec::Switch(SwitchParams::normally_open(true)),
    )
    .unwrap();
    c.add_component(
        "sw_tank",
        ComponentSpec::Switch(SwitchParams::normally_open(false)),
    )
    .unwrap();
    c.add_component("c1", ComponentSpec::Capacitor(CapacitorParams::ideal(1e-6)))
        .unwrap();
    c.add_component(
        "l1",
        ComponentSpec::Inductor(InductorParams::new(1e-3, 0.0)),
    )
    .unwrap();

    c.connect("v_init", "NEG", "gnd", "GND");
    c.connect("v_init", "POS", "sw_chg", "A");
    c.connect("sw_chg", "B", "c1", "A");
    c.connect("c1", "B", "gnd", "GND");
    c.connect("c1", "A", "sw_tank", "A");
    c.connect("sw_tank", "B", "l1", "A");
    c.connect("l1", "B", "gnd", "GND");

    c.solve_dc_op();
    assert!((c.terminal_voltage("c1", "A") - 10.0).abs() < 1e-4);

    // Disconnect charger, connect L across C -> free undamped LC oscillation!
    c.set_switch_closed("sw_chg", false).unwrap();
    c.set_switch_closed("sw_tank", true).unwrap();

    // After 1/2 period (~99 us), V_C should swing to ~ -10 V; after 1 full period (~199 us), back to ~ +10 V!
    c.step_to(99);
    let v_half = c.terminal_voltage("c1", "A");
    let omega = 1.0 / libm::sqrt(1e-9);
    let exp_half = 10.0 * libm::cos(omega * 99e-6);
    assert!(
        (v_half - exp_half).abs() / 10.0 < 0.005,
        "LC half-cycle: expected {exp_half:.4} V, got {v_half:.4} V"
    );
    c.step_to(199);
    let v_full = c.terminal_voltage("c1", "A");
    let exp_full = 10.0 * libm::cos(omega * 199e-6);
    assert!(
        (v_full - exp_full).abs() / 10.0 < 0.005,
        "LC full-cycle: expected {exp_full:.4} V, got {v_full:.4} V"
    );
}

#[test]
fn tier2_purely_resistive_1ph_ac_and_3ph_wye_within_0_1_percent() {
    // 1. PURELY RESISTIVE 1-phase AC (ZERO C/L — explicitly mandated by `02` §3.1!)
    //    230 Vrms (325.269119 Vpk) @ 50 Hz -> 1 kOhm -> GND
    let v_rms_expected = 230.0_f64;
    let v_peak = v_rms_expected * core::f64::consts::SQRT_2;

    let mut c1 = Circuit::new(CircuitConfig {
        steps_per_period: 200,
        max_transient_dt_us: 1_000,
        ..CircuitConfig::default()
    });
    c1.add_component("gnd", ComponentSpec::Ground).unwrap();
    c1.add_component(
        "vac",
        ComponentSpec::VoltageSourceAc(VoltageSourceAcParams {
            v_peak,
            freq_hz: 50.0,
            phase_rad: 0.0,
            dc_offset: 0.0,
        }),
    )
    .unwrap();
    c1.add_component(
        "r_load",
        ComponentSpec::Resistor(ResistorParams::with_power(1_000.0, 100.0)),
    )
    .unwrap();
    c1.connect("vac", "NEG", "gnd", "GND");
    c1.connect("vac", "POS", "r_load", "A");
    c1.connect("r_load", "B", "gnd", "GND");
    c1.add_probe("v_ac", ("r_load".to_owned(), "A".to_owned()), None, 10_000);

    c1.compile_topology();
    assert!(
        c1.needs_transient(),
        "purely resistive AC island MUST set needs_transient() == true (`02` §3.1)!"
    );
    // Run exactly 2 full 50 Hz periods (40 ms = 40_000 us)
    c1.step_to(40_000);
    // Measure RMS over the second exact cycle (20_100 ..= 40_000 us = 200 exact samples!)
    let meter = c1.multimeter_reading("v_ac", 20_100);
    assert_eq!(meter.samples, 200);
    let rel_err = (meter.v_rms - v_rms_expected).abs() / v_rms_expected;
    assert!(
        rel_err < 0.001,
        "1-phase resistive AC RMS: expected {v_rms_expected:.4} V, got {:.4} V (err {:.4}%)",
        meter.v_rms,
        rel_err * 100.0
    );

    // 2. 3-phase balanced Wye (L1=0°, L2=-120°, L3=+120°, 230 Vrms L-N -> 398.3717 Vrms L-L)
    let mut c3 = Circuit::new(CircuitConfig {
        steps_per_period: 200,
        max_transient_dt_us: 1_000,
        ..CircuitConfig::default()
    });
    c3.add_component("n_gnd", ComponentSpec::Ground).unwrap();
    let phases = [
        ("l1", 0.0_f64),
        ("l2", -2.0 * core::f64::consts::PI / 3.0),
        ("l3", 2.0 * core::f64::consts::PI / 3.0),
    ];
    for &(name, phase_rad) in &phases {
        c3.add_component(
            name,
            ComponentSpec::VoltageSourceAc(VoltageSourceAcParams {
                v_peak,
                freq_hz: 50.0,
                phase_rad,
                dc_offset: 0.0,
            }),
        )
        .unwrap();
        let r_name = format!("r_{name}");
        c3.add_component(
            &r_name,
            ComponentSpec::Resistor(ResistorParams::with_power(100.0, 1_000.0)),
        )
        .unwrap();
        c3.connect(name, "NEG", "n_gnd", "GND");
        c3.connect(name, "POS", &r_name, "A");
        c3.connect(&r_name, "B", "r_neutral", "A");
    }
    // Neutral return shunt resistor (1 Ohm) to measure neutral current I_N!
    c3.add_component(
        "r_neutral",
        ComponentSpec::Resistor(ResistorParams::with_power(1.0, 100.0)),
    )
    .unwrap();
    c3.connect("r_neutral", "B", "n_gnd", "GND");

    c3.add_probe(
        "v_l1_l2",
        ("l1".to_owned(), "POS".to_owned()),
        Some(("l2".to_owned(), "POS".to_owned())),
        10_000,
    );
    c3.add_probe(
        "v_neutral",
        ("r_neutral".to_owned(), "A".to_owned()),
        None,
        10_000,
    );

    c3.step_to(40_000);
    let m_ll = c3.multimeter_reading("v_l1_l2", 20_100);
    let expected_ll = v_rms_expected * libm::sqrt(3.0);
    let err_ll = (m_ll.v_rms - expected_ll).abs() / expected_ll;
    assert!(
        err_ll < 0.001,
        "3-phase V_LL RMS: expected {expected_ll:.4} V, got {:.4} V",
        m_ll.v_rms
    );
    let m_n = c3.multimeter_reading("v_neutral", 20_100);
    assert!(
        m_n.v_rms < 1e-6,
        "balanced 3-phase neutral current must be ~0 A, got {:.9} A",
        m_n.v_rms
    );
}

#[test]
fn tier2_coupled_transformer_2_to_1_step_down_within_0_1_percent() {
    // Primary L1 = 400 mH, Secondary L2 = 100 mH (turns ratio 2:1 -> sqrt(L2/L1) = 0.5), k = 0.999
    // Driven by 100 Vpk @ 50 Hz -> secondary open-circuit peak = 49.95 V (RMS = 35.31998 V)
    let mut c = Circuit::new(CircuitConfig {
        steps_per_period: 200,
        max_transient_dt_us: 100,
        ..CircuitConfig::default()
    });
    c.add_component("gnd1", ComponentSpec::Ground).unwrap();
    c.add_component("gnd2", ComponentSpec::Ground).unwrap();
    c.add_component(
        "vac",
        ComponentSpec::VoltageSourceAc(VoltageSourceAcParams {
            v_peak: 100.0,
            freq_hz: 50.0,
            phase_rad: 0.0,
            dc_offset: 0.0,
        }),
    )
    .unwrap();
    c.add_component(
        "t1",
        ComponentSpec::Transformer(TransformerParams {
            l1_h: 0.400,
            l2_h: 0.100,
            k_coupling: 0.999,
            r1_ohm: 0.01,
            r2_ohm: 0.01,
        }),
    )
    .unwrap();
    c.add_component(
        "r_sec",
        ComponentSpec::Resistor(ResistorParams::with_power(100_000.0, 10.0)),
    )
    .unwrap();

    c.connect("vac", "NEG", "gnd1", "GND");
    c.connect("vac", "POS", "t1", "P1");
    c.connect("t1", "P2", "gnd1", "GND");
    c.connect("t1", "S1", "r_sec", "A");
    c.connect("r_sec", "B", "gnd2", "GND");
    c.connect("t1", "S2", "gnd2", "GND");

    c.add_probe("v_sec", ("t1".to_owned(), "S1".to_owned()), None, 10_000);
    c.step_to(40_000);
    let m = c.multimeter_reading("v_sec", 20_100);
    let expected_rms = (100.0 / core::f64::consts::SQRT_2) * 0.5 * 0.999;
    let err = (m.v_rms - expected_rms).abs() / expected_rms;
    assert!(
        err < 0.001,
        "2:1 transformer secondary RMS: expected {expected_rms:.4} V, got {:.4} V (err {:.4}%)",
        m.v_rms,
        err * 100.0
    );
}

#[test]
fn tier2_uno_def001_pwm_timer0_fast_and_timer1_phase_correct_into_rc_filter() {
    // Verify BOTH verified Arduino Uno R3 PWM transfer functions (`DEF-001`) into RC low-pass filters:
    //   R = 10 kOhm, C = 4.7 uF (tau = 47 ms >> T_pwm = 1.024 ms / 2.040 ms)
    //   - D6 (Timer 0 Fast PWM): analogWrite(6, 127) -> EXACT (127+1)/256 = 0.5000 -> V_avg = 2.500 V!
    //   - D9 (Timer 1 Phase-Correct PWM): analogWrite(9, 127) -> EXACT 127/255 = 0.498039 -> V_avg = 2.4902 V!
    let mut circuit = Circuit::new(CircuitConfig {
        max_transient_dt_us: 64,
        ..CircuitConfig::default()
    });
    circuit
        .add_component(
            "r_d6",
            ComponentSpec::Resistor(ResistorParams::new(10_000.0)),
        )
        .unwrap();
    circuit
        .add_component(
            "c_d6",
            ComponentSpec::Capacitor(CapacitorParams::ideal(4.7e-6)),
        )
        .unwrap();
    circuit
        .add_component(
            "r_d9",
            ComponentSpec::Resistor(ResistorParams::new(10_000.0)),
        )
        .unwrap();
    circuit
        .add_component(
            "c_d9",
            ComponentSpec::Capacitor(CapacitorParams::ideal(4.7e-6)),
        )
        .unwrap();

    let mut backend = SpiceCircuitBackend::new(circuit);
    backend
        .attach_board(BoardCircuitSpec {
            board_id: "uno".to_owned(),
            profile: arduino_uno_r3_electrical(),
            pins: vec![
                BoardPinSpec {
                    pin: "D6".to_owned(),
                    is_output: true,
                    is_analog: false,
                    is_pwm: true,
                    pull: PullMode::None,
                    analog_range: None,
                },
                BoardPinSpec {
                    pin: "D9".to_owned(),
                    is_output: true,
                    is_analog: false,
                    is_pwm: true,
                    pull: PullMode::None,
                    analog_range: None,
                },
            ],
        })
        .unwrap();

    backend.circuit_mut().connect("uno", "D6", "r_d6", "A");
    backend.circuit_mut().connect("r_d6", "B", "c_d6", "A");
    backend.circuit_mut().connect("c_d6", "B", "uno", "GND");

    backend.circuit_mut().connect("uno", "D9", "r_d9", "A");
    backend.circuit_mut().connect("r_d9", "B", "c_d9", "A");
    backend.circuit_mut().connect("c_d9", "B", "uno", "GND");

    backend
        .circuit_mut()
        .add_probe("v_d6", ("c_d6".to_owned(), "A".to_owned()), None, 20_000);
    backend
        .circuit_mut()
        .add_probe("v_d9", ("c_d9".to_owned(), "A".to_owned()), None, 20_000);

    // Encode native OCR = 127 through the Uno 8-bit span codec -> ASL DUTY
    let pwm_codec = Codec::new("uno_pwm8", CodecKind::Span, 255, 255).unwrap();
    let asl_127 = u32::try_from(pwm_codec.to_analog_unipolar(127).unwrap()).unwrap();
    backend.set_board_pwm_out("uno", "D6", asl_127);
    backend.set_board_pwm_out("uno", "D9", asl_127);

    // Settle for 8 * tau (~380 ms -> 99.97% settled)
    backend.solve_at(380_000);

    let m_d6 = backend.circuit().multimeter_reading("v_d6", 350_000);
    let m_d9 = backend.circuit().multimeter_reading("v_d9", 350_000);

    let exp_d6 = 5.0 * (128.0 / 256.0); // 2.50000 V exact (Fast PWM (OCR+1)/256!)
    let exp_d9 = 5.0 * (127.0 / 255.0); // 2.49020 V exact (Phase-Correct OCR/255!)

    assert!(
        (m_d6.v_avg - exp_d6).abs() < 0.015,
        "Timer 0 Fast PWM D6 avg: expected {exp_d6:.4} V, got {:.4} V",
        m_d6.v_avg
    );
    assert!(
        (m_d9.v_avg - exp_d9).abs() < 0.015,
        "Timer 1 Phase-Correct PWM D9 avg: expected {exp_d9:.4} V, got {:.4} V",
        m_d9.v_avg
    );
    // Explicitly confirm Timer 0 Fast PWM (128/256) sits measurably above Timer 1 Phase-Correct (127/255)!
    assert!(
        m_d6.v_avg > m_d9.v_avg,
        "Fast PWM (128/256 = 0.5000) MUST exceed Phase-Correct PWM (127/255 = 0.4980) for OCR=127!"
    );
}
