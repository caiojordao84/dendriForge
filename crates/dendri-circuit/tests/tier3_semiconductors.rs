//! Phase 3c (Tier 3) Headless Analytical Physics Benchmarks (`02` §3.4):
//! Ebers-Moll BJT (NPN/PNP), Shichman-Hodges Level-1 MOSFET (NMOS/PMOS + body diode),
//! galvanically isolated Optocoupler (cross-island 5V <-> 24V), IGBT, SCR latching/commutation,
//! TRIAC bidirectional AC conduction, SSR isolated AC load, H-Bridge 4-quadrant control,
//! and back-EMF DC motor steady-state speed (`error < 0.1%`).

use dendri_circuit::{
    BjtParams, Circuit, CircuitConfig, ComponentSpec, DcMotorParams, HBridgeParams, IgbtParams,
    MosfetParams, OptocouplerParams, ResistorParams, ScrParams, SsrParams, SwitchParams,
    TriacParams, VoltageSourceAcParams, VoltageSourceDcParams,
};

#[test]
fn tier3_npn_bjt_active_gain_and_saturation_switch() {
    // 1. Active region: Vcc = 10 V, Rc = 1 kOhm, Rb = 430 kOhm to 5 V (~10 uA base -> ~1 mA collector!)
    let bjt = BjtParams::npn_2n2222(); // beta_f = 100, V_af = 100 V
    let mut c = Circuit::default();
    c.add_component("gnd", ComponentSpec::Ground).unwrap();
    c.add_component(
        "vcc",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(10.0)),
    )
    .unwrap();
    c.add_component(
        "vbb",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    )
    .unwrap();
    c.add_component("rc", ComponentSpec::Resistor(ResistorParams::new(1_000.0)))
        .unwrap();
    c.add_component(
        "rb",
        ComponentSpec::Resistor(ResistorParams::new(430_000.0)),
    )
    .unwrap();
    c.add_component("q1", ComponentSpec::Bjt(bjt.clone()))
        .unwrap();

    c.connect("vcc", "NEG", "gnd", "GND");
    c.connect("vbb", "NEG", "gnd", "GND");
    c.connect("vcc", "POS", "rc", "A");
    c.connect("rc", "B", "q1", "C");
    c.connect("vbb", "POS", "rb", "A");
    c.connect("rb", "B", "q1", "B");
    c.connect("q1", "E", "gnd", "GND");

    c.solve_dc_op();
    let vb = c.terminal_voltage("q1", "B");
    let vc = c.terminal_voltage("q1", "C");
    let ib = (5.0 - vb) / 430_000.0;
    let ic = (10.0 - vc) / 1_000.0;
    // Exact Ebers-Moll Early-adjusted current gain: beta_eff = beta_f * (1 - V_bc / V_af)
    let expected_ic = ib * bjt.beta_f * (1.0 - (vb - vc) / bjt.v_af);
    assert!(
        (ic - expected_ic).abs() / expected_ic < 0.001,
        "BJT active gain: expected Ic={expected_ic:.6} A, got {ic:.6} A"
    );

    // 2. Hard saturation: reduce Rb to 10 kOhm (~430 uA base -> I_c saturates at ~10 mA, V_CE < 0.2 V!)
    c.set_resistance("rb", 10_000.0).unwrap();
    c.solve_dc_op();
    let v_ce_sat = c.terminal_voltage("q1", "C");
    assert!(
        v_ce_sat < 0.25 && v_ce_sat > 0.0,
        "saturated NPN BJT V_CE must be < 0.25V, got {v_ce_sat} V"
    );
}

#[test]
fn tier3_nmos_shichman_hodges_saturation_exact_within_0_1_percent_and_body_diode() {
    // Saturation test: V_GS = 3.0 V (V_TH = 2.0 V -> V_ov = 1.0 V), V_DS = 5.0 V (> V_ov),
    // K_P = 0.010 A/V^2, lambda = 0.02 / V -> exact I_D = 0.5 * 0.010 * 1^2 * (1 + 0.02 * 5) = 5.5 mA!
    let mut c = Circuit::default();
    c.add_component("gnd", ComponentSpec::Ground).unwrap();
    c.add_component(
        "vgs",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(3.0)),
    )
    .unwrap();
    c.add_component(
        "vds",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    )
    .unwrap();
    c.add_component(
        "m1",
        ComponentSpec::Mosfet(MosfetParams {
            polarity: dendri_circuit::MosfetPolarity::Nmos,
            v_th: 2.0,
            k_p: 0.010,
            lambda: 0.02,
            has_body_diode: true,
        }),
    )
    .unwrap();

    c.connect("vgs", "NEG", "gnd", "GND");
    c.connect("vds", "NEG", "gnd", "GND");
    c.connect("vgs", "POS", "m1", "G");
    c.connect("vds", "POS", "m1", "D");
    c.connect("m1", "S", "gnd", "GND");

    c.solve_dc_op();
    let expected_id = 0.5 * 0.010 * 1.0 * 1.0 * (1.0 + 0.02 * 5.0);
    let got_id = c.component_current("m1");
    assert!(
        (got_id - expected_id).abs() / expected_id < 0.001,
        "MOSFET saturation: expected {expected_id:.6} A, got {got_id:.6} A"
    );

    // Cutoff (V_GS = 0 V, V_DS = +5 V) -> I_D ~ 0 A
    c.set_dc_voltage("vgs", 0.0).unwrap();
    c.solve_dc_op();
    assert!(c.component_current("m1").abs() < 1e-8);

    // Reverse body diode conduction (V_GS = 0 V, V_D = -1.0 V via 100 Ohm -> body diode conducts S -> D!)
    let mut c_bd = Circuit::default();
    c_bd.add_component("gnd", ComponentSpec::Ground).unwrap();
    c_bd.add_component(
        "v_rev",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(-2.0)),
    )
    .unwrap();
    c_bd.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(100.0)))
        .unwrap();
    c_bd.add_component(
        "m1",
        ComponentSpec::Mosfet(MosfetParams::nmos_logic_level()),
    )
    .unwrap();
    c_bd.connect("v_rev", "NEG", "gnd", "GND");
    c_bd.connect("v_rev", "POS", "r1", "A");
    c_bd.connect("r1", "B", "m1", "D");
    c_bd.connect("m1", "G", "gnd", "GND");
    c_bd.connect("m1", "S", "gnd", "GND");
    c_bd.solve_dc_op();
    let v_d = c_bd.terminal_voltage("m1", "D");
    assert!(
        (-1.1..-0.5).contains(&v_d),
        "NMOS body diode must clamp reverse drain to ~ -0.7V..-1.0V, got {v_d} V"
    );
}

#[test]
fn tier3_optocoupler_galvanic_isolation_5v_mcu_to_24v_plc_island() {
    // Island 0 (5 V Arduino side): 5 V -> 330 Ohm -> Opto LED -> GND_5V (~11 mA)
    // Island 1 (24 V PLC side): 24 V -> 4.7 kOhm pull-up -> Opto Collector -> Emitter -> GND_24V
    // CTR = 1.0 -> 11 mA >> 24 V / 4.7 kOhm (5.1 mA) -> Collector pulls hard to V_CE(sat) < 0.3 V!
    let mut c = Circuit::default();
    c.add_component("gnd_5v", ComponentSpec::Ground).unwrap();
    c.add_component(
        "v5",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    )
    .unwrap();
    c.add_component("r_led", ComponentSpec::Resistor(ResistorParams::new(330.0)))
        .unwrap();

    c.add_component("gnd_24v", ComponentSpec::Ground).unwrap();
    c.add_component(
        "v24",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(24.0)),
    )
    .unwrap();
    c.add_component(
        "r_pull",
        ComponentSpec::Resistor(ResistorParams::with_power(4_700.0, 1.0)),
    )
    .unwrap();

    c.add_component(
        "opto",
        ComponentSpec::Optocoupler(OptocouplerParams::default()),
    )
    .unwrap();

    c.connect("v5", "NEG", "gnd_5v", "GND");
    c.connect("v5", "POS", "r_led", "A");
    c.connect("r_led", "B", "opto", "ANODE");
    c.connect("opto", "CATHODE", "gnd_5v", "GND");

    c.connect("v24", "NEG", "gnd_24v", "GND");
    c.connect("v24", "POS", "r_pull", "A");
    c.connect("r_pull", "B", "opto", "COLLECTOR");
    c.connect("opto", "EMITTER", "gnd_24v", "GND");

    c.compile_topology();
    assert_eq!(
        c.island_count(),
        2,
        "Optocoupler MUST keep 5V and 24V domains in 2 galvanically isolated MNA islands!"
    );

    c.solve_dc_op();
    let v_col_on = c.terminal_voltage("opto", "COLLECTOR");
    assert!(
        v_col_on < 0.4,
        "Optocoupler ON must pull 24V collector into saturation (<0.4V), got {v_col_on} V"
    );

    // Turn off 5V LED -> 24V collector rises back to 24.0 V!
    c.set_dc_voltage("v5", 0.0).unwrap();
    c.solve_dc_op();
    let v_col_off = c.terminal_voltage("opto", "COLLECTOR");
    assert!(
        v_col_off > 23.9,
        "Optocoupler OFF must release collector to ~24V, got {v_col_off} V"
    );
}

#[test]
fn tier3_igbt_scr_latch_triac_ac_ssr_hbridge_and_dc_motor_back_emf() {
    // 1. IGBT high-voltage switch (100 V -> 10 Ohm -> IGBT -> GND, V_GE = 15 V)
    let mut c_igbt = Circuit::default();
    c_igbt.add_component("gnd", ComponentSpec::Ground).unwrap();
    c_igbt
        .add_component(
            "vbus",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(100.0)),
        )
        .unwrap();
    c_igbt
        .add_component(
            "vge",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(0.0)),
        )
        .unwrap();
    c_igbt
        .add_component(
            "r_load",
            ComponentSpec::Resistor(ResistorParams::with_power(10.0, 2000.0)),
        )
        .unwrap();
    c_igbt
        .add_component("igbt", ComponentSpec::Igbt(IgbtParams::default()))
        .unwrap();
    c_igbt.connect("vbus", "NEG", "gnd", "GND");
    c_igbt.connect("vge", "NEG", "gnd", "GND");
    c_igbt.connect("vbus", "POS", "r_load", "A");
    c_igbt.connect("r_load", "B", "igbt", "C");
    c_igbt.connect("vge", "POS", "igbt", "G");
    c_igbt.connect("igbt", "E", "gnd", "GND");

    c_igbt.solve_dc_op();
    assert!(c_igbt.terminal_voltage("igbt", "C") > 99.9);
    c_igbt.set_dc_voltage("vge", 15.0).unwrap();
    c_igbt.solve_dc_op();
    assert!(c_igbt.terminal_voltage("igbt", "C") < 2.5);

    // 2. SCR latching & commutation (pulse gate -> stays latched after gate=0V -> turn off when V_bus drops)
    let mut c_scr = Circuit::default();
    c_scr.add_component("gnd", ComponentSpec::Ground).unwrap();
    c_scr
        .add_component(
            "vbus",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(24.0)),
        )
        .unwrap();
    c_scr
        .add_component(
            "vg",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(0.0)),
        )
        .unwrap();
    c_scr
        .add_component(
            "r_load",
            ComponentSpec::Resistor(ResistorParams::with_power(100.0, 20.0)),
        )
        .unwrap();
    c_scr
        .add_component("scr", ComponentSpec::Scr(ScrParams::default()))
        .unwrap();
    c_scr.connect("vbus", "NEG", "gnd", "GND");
    c_scr.connect("vg", "NEG", "gnd", "GND");
    c_scr.connect("vbus", "POS", "r_load", "A");
    c_scr.connect("r_load", "B", "scr", "A");
    c_scr.connect("vg", "POS", "scr", "G");
    c_scr.connect("scr", "K", "gnd", "GND");

    c_scr.solve_dc_op();
    assert!(!c_scr.visual_state("scr").energized); // Initially OFF
    c_scr.set_dc_voltage("vg", 3.3).unwrap();
    c_scr.solve_dc_op();
    assert!(c_scr.visual_state("scr").energized); // Triggered ON
    c_scr.set_dc_voltage("vg", 0.0).unwrap();
    c_scr.solve_dc_op();
    assert!(
        c_scr.visual_state("scr").energized,
        "SCR must stay LATCHED after gate removed!"
    );
    c_scr.set_dc_voltage("vbus", 0.5).unwrap();
    c_scr.solve_dc_op();
    assert!(
        !c_scr.visual_state("scr").energized,
        "SCR must commutate OFF below holding current!"
    );

    // 3. TRIAC bidirectional AC + SSR isolated AC
    let mut c_triac = Circuit::new(CircuitConfig {
        steps_per_period: 100,
        ..CircuitConfig::default()
    });
    c_triac.add_component("gnd", ComponentSpec::Ground).unwrap();
    c_triac
        .add_component(
            "vac",
            ComponentSpec::VoltageSourceAc(VoltageSourceAcParams {
                v_peak: 100.0,
                freq_hz: 50.0,
                phase_rad: 0.0,
                dc_offset: 0.0,
            }),
        )
        .unwrap();
    c_triac
        .add_component(
            "vg",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
        )
        .unwrap();
    c_triac
        .add_component(
            "r_load",
            ComponentSpec::Resistor(ResistorParams::with_power(100.0, 200.0)),
        )
        .unwrap();
    c_triac
        .add_component("triac", ComponentSpec::Triac(TriacParams::default()))
        .unwrap();
    c_triac.connect("vac", "NEG", "gnd", "GND");
    c_triac.connect("vg", "NEG", "gnd", "GND");
    c_triac.connect("vac", "POS", "r_load", "A");
    c_triac.connect("r_load", "B", "triac", "MT2");
    c_triac.connect("triac", "MT1", "gnd", "GND");
    c_triac.connect("vg", "POS", "triac", "G");
    // Step to positive peak (5 ms) and negative peak (15 ms): both half-cycles conduct!
    c_triac.step_to(5_000);
    let v_pos_drop =
        c_triac.terminal_voltage("r_load", "A") - c_triac.terminal_voltage("r_load", "B");
    assert!(
        v_pos_drop > 95.0,
        "TRIAC must conduct positive half-cycle, got {v_pos_drop}"
    );
    c_triac.step_to(15_000);
    let v_neg_drop =
        c_triac.terminal_voltage("r_load", "A") - c_triac.terminal_voltage("r_load", "B");
    assert!(
        v_neg_drop < -95.0,
        "TRIAC must conduct negative half-cycle, got {v_neg_drop}"
    );

    // 4. Isolated SSR switching 24V load across 2 islands
    let mut c_ssr = Circuit::default();
    c_ssr.add_component("gnd1", ComponentSpec::Ground).unwrap();
    c_ssr.add_component("gnd2", ComponentSpec::Ground).unwrap();
    c_ssr
        .add_component(
            "v_ctrl",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
        )
        .unwrap();
    c_ssr
        .add_component(
            "v_load",
            ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(24.0)),
        )
        .unwrap();
    c_ssr
        .add_component(
            "r_load",
            ComponentSpec::Resistor(ResistorParams::with_power(100.0, 20.0)),
        )
        .unwrap();
    c_ssr
        .add_component("ssr", ComponentSpec::Ssr(SsrParams::default()))
        .unwrap();
    c_ssr.connect("v_ctrl", "NEG", "gnd1", "GND");
    c_ssr.connect("v_ctrl", "POS", "ssr", "CTRL_P");
    c_ssr.connect("ssr", "CTRL_N", "gnd1", "GND");
    c_ssr.connect("v_load", "NEG", "gnd2", "GND");
    c_ssr.connect("v_load", "POS", "ssr", "LOAD_A");
    c_ssr.connect("ssr", "LOAD_B", "r_load", "A");
    c_ssr.connect("r_load", "B", "gnd2", "GND");
    c_ssr.compile_topology();
    assert_eq!(c_ssr.island_count(), 2);
    c_ssr.solve_dc_op();
    assert!(c_ssr.terminal_voltage("r_load", "A") > 23.9);

    // 5. H-Bridge driving a DC motor forward (+omega) and reverse (-omega) with exact back-EMF!
    let mot = DcMotorParams::default(); // R=2, L=2mH, Ke=Kt=0.05, J=1e-4, B=1e-3
    let mut c_hb = Circuit::default();
    c_hb.add_component("gnd", ComponentSpec::Ground).unwrap();
    c_hb.add_component(
        "vcc",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(12.0)),
    )
    .unwrap();
    c_hb.add_component(
        "v_en",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    )
    .unwrap();
    c_hb.add_component(
        "v_in1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    )
    .unwrap();
    c_hb.add_component(
        "v_in2",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(0.0)),
    )
    .unwrap();
    c_hb.add_component("hb", ComponentSpec::HBridge(HBridgeParams::default()))
        .unwrap();
    c_hb.add_component("m1", ComponentSpec::DcMotor(mot.clone()))
        .unwrap();
    c_hb.add_component(
        "sw_unused",
        ComponentSpec::Switch(SwitchParams::normally_open(false)),
    )
    .unwrap();

    c_hb.connect("vcc", "NEG", "gnd", "GND");
    c_hb.connect("v_en", "NEG", "gnd", "GND");
    c_hb.connect("v_in1", "NEG", "gnd", "GND");
    c_hb.connect("v_in2", "NEG", "gnd", "GND");
    c_hb.connect("vcc", "POS", "hb", "VCC");
    c_hb.connect("hb", "GND", "gnd", "GND");
    c_hb.connect("v_en", "POS", "hb", "EN");
    c_hb.connect("v_in1", "POS", "hb", "IN1");
    c_hb.connect("v_in2", "POS", "hb", "IN2");
    c_hb.connect("hb", "OUTA", "m1", "A");
    c_hb.connect("hb", "OUTB", "m1", "B");

    // Forward DC steady-state: V_motor = 12 V across R_total = R_arm + 2*R_ds_on = 2.4 Ohm
    // Exact analytical steady-state speed: omega_ss = K_t * V / (R_total * B + K_e * K_t)
    c_hb.solve_dc_op();
    let r_tot = mot.r_arm_ohm + 2.0 * HBridgeParams::default().r_ds_on_ohm;
    let exp_omega = (mot.k_t * 12.0) / (r_tot * mot.friction_b + mot.k_e * mot.k_t);
    let got_omega_fwd = c_hb.visual_state("m1").angular_velocity_rad_s;
    assert!(
        (got_omega_fwd - exp_omega).abs() / exp_omega < 0.001,
        "DC motor forward back-EMF omega: expected {exp_omega:.4} rad/s, got {got_omega_fwd:.4} rad/s"
    );

    // Reverse H-Bridge polarity (IN1=0V, IN2=5V) -> exact -exp_omega!
    c_hb.set_dc_voltage("v_in1", 0.0).unwrap();
    c_hb.set_dc_voltage("v_in2", 5.0).unwrap();
    c_hb.solve_dc_op();
    let got_omega_rev = c_hb.visual_state("m1").angular_velocity_rad_s;
    assert!(
        (got_omega_rev + exp_omega).abs() / exp_omega < 0.001,
        "DC motor reverse back-EMF omega: expected -{exp_omega:.4} rad/s, got {got_omega_rev:.4} rad/s"
    );
}
