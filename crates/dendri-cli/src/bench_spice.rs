//! Headless SPICE analytical physics benchmark suite (`dendri bench-spice`, CI Gate 6).
//!
//! Verifies `dendri-circuit` MNA solver accuracy against closed-form analytical equations
//! with a strict `< 0.1%` relative error budget (`02-simulation-and-electrical-engine.md` §3.4).

use dendri_circuit::{
    CapacitorParams, Circuit, CircuitConfig, ComponentSpec, CurrentSourceDcParams, DiodeParams,
    InductorParams, ResistorParams, SwitchParams, VoltageSourceAcParams, VoltageSourceDcParams,
};

/// Thermal voltage $V_T = k_B T / q$ at $T = 300\text{ K}$ (independent analytical constant).
const VT_300K: f64 = 0.025_851_99;

/// Maximum permitted relative error (`0.1%` per CI Gate 6).
pub const MAX_ALLOWED_REL_ERROR: f64 = 0.001;

/// Outcome of a single analytical SPICE benchmark.
#[derive(Debug, Clone, PartialEq)]
pub struct SpiceBenchResult {
    pub name: &'static str,
    pub unit: &'static str,
    pub simulated: f64,
    pub analytical: f64,
    pub rel_error: f64,
    pub passed: bool,
}

/// Summary report returned by [`run_bench_spice`].
#[derive(Debug, Clone, PartialEq)]
pub struct BenchSpiceReport {
    pub results: Vec<SpiceBenchResult>,
    pub max_rel_error: f64,
    pub all_passed: bool,
}

fn make_result(
    name: &'static str,
    unit: &'static str,
    simulated: f64,
    analytical: f64,
) -> SpiceBenchResult {
    let denom = analytical.abs().max(1e-12);
    let rel_error = (simulated - analytical).abs() / denom;
    SpiceBenchResult {
        name,
        unit,
        simulated,
        analytical,
        rel_error,
        passed: rel_error < MAX_ALLOWED_REL_ERROR,
    }
}

/// Runs all headless SPICE analytical physics benchmarks (`< 0.1%` relative error gate).
#[must_use]
pub fn run_bench_spice() -> BenchSpiceReport {
    let results = vec![
        bench_dc_voltage_divider(),
        bench_4_20ma_shunt_plc_t_num_5(),
        bench_shockley_diode_forward_bias(),
        bench_rc_step_charging_1tau(),
        bench_rl_step_current_1tau(),
        bench_ac_purely_resistive_peak(),
    ];

    let mut max_rel_error: f64 = 0.0;
    let mut all_passed = true;
    for r in &results {
        if r.rel_error > max_rel_error {
            max_rel_error = r.rel_error;
        }
        if !r.passed {
            all_passed = false;
        }
    }

    BenchSpiceReport {
        results,
        max_rel_error,
        all_passed,
    }
}

fn bench_dc_voltage_divider() -> SpiceBenchResult {
    // 10.0 V across R1 = 2 kOhm and R2 = 3 kOhm -> V_mid = 10 * 3 / (2 + 3) = 6.0 V
    let mut c = Circuit::default();
    let _ = c.add_component("gnd", ComponentSpec::Ground);
    let _ = c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(10.0)),
    );
    let _ = c.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(2_000.0)));
    let _ = c.add_component("r2", ComponentSpec::Resistor(ResistorParams::new(3_000.0)));
    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "r1", "A");
    c.connect("r1", "B", "r2", "A");
    c.connect("r2", "B", "gnd", "GND");
    let _ = c.solve_dc_op();
    make_result(
        "dc_voltage_divider",
        "V",
        c.terminal_voltage("r2", "A"),
        6.0,
    )
}

fn bench_4_20ma_shunt_plc_t_num_5() -> SpiceBenchResult {
    // 4 mA current source into 500 Ohm precision shunt -> V = 0.004 * 500 = 2.0 V (T-NUM-5)
    let mut c = Circuit::default();
    let _ = c.add_component("gnd", ComponentSpec::Ground);
    let _ = c.add_component(
        "i_tx",
        ComponentSpec::CurrentSourceDc(CurrentSourceDcParams { current_a: 0.004 }),
    );
    let _ = c.add_component(
        "r_shunt",
        ComponentSpec::Resistor(ResistorParams::new(500.0)),
    );
    c.connect("i_tx", "IN", "gnd", "GND");
    c.connect("i_tx", "OUT", "r_shunt", "A");
    c.connect("r_shunt", "B", "gnd", "GND");
    let _ = c.solve_dc_op();
    make_result(
        "shunt_4_20ma_500ohm",
        "V",
        c.terminal_voltage("r_shunt", "A"),
        2.0,
    )
}

fn bench_shockley_diode_forward_bias() -> SpiceBenchResult {
    // 5.0 V -> 1 kOhm -> silicon diode (1N4148) -> GND
    // Exact analytical check: I_d = (5.0 - V_d) / 1000 must satisfy Shockley I_s*(exp(V_j/(n*Vt))-1).
    let diode = DiodeParams::silicon_1n4148();
    let r_ohm = 1_000.0;
    let v_src = 5.0;
    let mut c = Circuit::default();
    let _ = c.add_component("gnd", ComponentSpec::Ground);
    let _ = c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(v_src)),
    );
    let _ = c.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(r_ohm)));
    let _ = c.add_component("d1", ComponentSpec::Diode(diode.clone()));
    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "r1", "A");
    c.connect("r1", "B", "d1", "A");
    c.connect("d1", "K", "gnd", "GND");
    let _ = c.solve_dc_op();

    let v_d = c.terminal_voltage("d1", "A");
    let i_sim = (v_src - v_d) / r_ohm;
    // Closed-form Shockley current at the solved diode voltage (plus series resistance check)
    let v_j = v_d - i_sim * diode.r_s;
    let i_shockley = diode.i_s * (libm::exp(v_j / (diode.n * VT_300K)) - 1.0);
    make_result("shockley_diode_1n4148", "A", i_sim, i_shockley)
}

fn bench_rc_step_charging_1tau() -> SpiceBenchResult {
    // 5.0 V -> R = 1 kOhm -> C = 1 uF -> GND; tau = 1 ms; V_C(1 ms) = 5 * (1 - e^-1)
    let mut c = Circuit::new(CircuitConfig {
        max_transient_dt_us: 10,
        ..CircuitConfig::default()
    });
    let _ = c.add_component("gnd", ComponentSpec::Ground);
    let _ = c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(5.0)),
    );
    let _ = c.add_component(
        "sw",
        ComponentSpec::Switch(SwitchParams::normally_open(false)),
    );
    let _ = c.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(1_000.0)));
    let _ = c.add_component("c1", ComponentSpec::Capacitor(CapacitorParams::ideal(1e-6)));
    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "sw", "A");
    c.connect("sw", "B", "r1", "A");
    c.connect("r1", "B", "c1", "A");
    c.connect("c1", "B", "gnd", "GND");
    c.use_initial_conditions();
    let _ = c.set_switch_closed("sw", true);
    let _ = c.step_to(1_000);
    let expected = 5.0 * (1.0 - libm::exp(-1.0));
    make_result(
        "rc_step_charging_1tau",
        "V",
        c.terminal_voltage("c1", "A"),
        expected,
    )
}

fn bench_rl_step_current_1tau() -> SpiceBenchResult {
    // 10.0 V -> R = 100 Ohm -> L = 10 mH -> GND; tau = 100 us; I_L(100 us) = 0.1 * (1 - e^-1)
    let mut c = Circuit::new(CircuitConfig {
        max_transient_dt_us: 1,
        ..CircuitConfig::default()
    });
    let _ = c.add_component("gnd", ComponentSpec::Ground);
    let _ = c.add_component(
        "v1",
        ComponentSpec::VoltageSourceDc(VoltageSourceDcParams::ideal(10.0)),
    );
    let _ = c.add_component(
        "sw",
        ComponentSpec::Switch(SwitchParams::normally_open(false)),
    );
    let _ = c.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(100.0)));
    let _ = c.add_component(
        "l1",
        ComponentSpec::Inductor(InductorParams::new(10e-3, 0.0)),
    );
    c.connect("v1", "NEG", "gnd", "GND");
    c.connect("v1", "POS", "sw", "A");
    c.connect("sw", "B", "r1", "A");
    c.connect("r1", "B", "l1", "A");
    c.connect("l1", "B", "gnd", "GND");
    c.use_initial_conditions();
    let _ = c.set_switch_closed("sw", true);
    let _ = c.step_to(100);
    let expected = 0.1 * (1.0 - libm::exp(-1.0));
    make_result(
        "rl_step_current_1tau",
        "A",
        c.component_current("l1"),
        expected,
    )
}

fn bench_ac_purely_resistive_peak() -> SpiceBenchResult {
    // Purely resistive 1-phase AC (zero C/L): 10 Vpk @ 50 Hz -> R1 = 1 kOhm -> R2 = 1 kOhm -> GND
    // At quarter period t = 5_000 us (5 ms = T/4), sin(2*pi*50*0.005) = 1.0 -> V_mid = 5.0 V.
    let mut c = Circuit::default();
    let _ = c.add_component("gnd", ComponentSpec::Ground);
    let _ = c.add_component(
        "vac",
        ComponentSpec::VoltageSourceAc(VoltageSourceAcParams {
            v_peak: 10.0,
            freq_hz: 50.0,
            phase_rad: 0.0,
            dc_offset: 0.0,
        }),
    );
    let _ = c.add_component("r1", ComponentSpec::Resistor(ResistorParams::new(1_000.0)));
    let _ = c.add_component("r2", ComponentSpec::Resistor(ResistorParams::new(1_000.0)));
    c.connect("vac", "NEG", "gnd", "GND");
    c.connect("vac", "POS", "r1", "A");
    c.connect("r1", "B", "r2", "A");
    c.connect("r2", "B", "gnd", "GND");
    c.use_initial_conditions();
    let _ = c.step_to(5_000);
    make_result(
        "ac_purely_resistive_quarter_period",
        "V",
        c.terminal_voltage("r2", "A"),
        5.0,
    )
}
