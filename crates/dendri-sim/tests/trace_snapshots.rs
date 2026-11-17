//! Deterministic sim trace snapshots.

use std::fs;
use std::path::PathBuf;

use dendri_parse::{parse_program_file, parse_project_file};
use dendri_sim::{SimConfig, Simulator};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn assert_trace_with(name: &str, cycles: u64, cfg: SimConfig) {
    let path = repo().join("content/fixtures/sim").join(name);
    let parsed = parse_program_file(&path).unwrap();
    let mut sim = Simulator::from_program(parsed.program, &cfg);
    sim.run(cycles).unwrap();
    let got = sim.trace_text();
    let snap = repo()
        .join("crates/dendri-sim/tests/snapshots")
        .join(name.replace(".asl.toml", ".trace.txt"));
    if std::env::var("UPDATE_SIM_SNAPSHOTS").as_deref() == Ok("1") {
        fs::create_dir_all(snap.parent().unwrap()).unwrap();
        fs::write(&snap, &got).unwrap();
    }
    let expected = fs::read_to_string(&snap)
        .unwrap_or_else(|e| panic!("missing snapshot {}: {e}", snap.display()));
    assert_eq!(got, expected, "trace mismatch for {name}");
}

fn assert_trace(name: &str, cycles: u64) {
    assert_trace_with(
        name,
        cycles,
        SimConfig {
            max_cycles: cycles,
            ..SimConfig::default()
        },
    );
}

#[test]
fn s01_blink_trace() {
    assert_trace("S01-blink-set-bool-wait.asl.toml", 2);
}

#[test]
fn s02_ton_trace() {
    assert_trace("S02-ton-on-delay.asl.toml", 3);
}

#[test]
fn s04_pwm_adc_map_trace() {
    let mut adc = std::collections::HashMap::new();
    adc.insert(34, 32768); // mid-scale → duty ≈ 127
    assert_trace_with(
        "S04-pwm-adc-map.asl.toml",
        1,
        SimConfig {
            max_cycles: 1,
            initial_adc: adc,
            ..SimConfig::default()
        },
    );
}

#[test]
fn s05_uart_i2c_spi_trace() {
    assert_trace("S05-uart-i2c-spi.asl.toml", 1);
}

#[test]
fn s06_iec_tof_trig_trace() {
    assert_trace("S06-iec-tof-trig.asl.toml", 3);
}

#[test]
fn s07_iec_counters_trace() {
    assert_trace("S07-iec-counters.asl.toml", 3);
}

#[test]
fn s09_string_ops_trace() {
    assert_trace_with(
        "S09-string-ops.asl.toml",
        1,
        SimConfig {
            max_cycles: 1,
            serial_rx: vec!["PING".into()],
            ..SimConfig::default()
        },
    );
}

#[test]
fn s12_i2c_spi_xfer_trace() {
    assert_trace("S12-i2c-spi-xfer.asl.toml", 1);
}

#[test]
fn s13_math_sqrt_log_trace() {
    assert_trace("S13-math-sqrt-log.asl.toml", 1);
}

#[test]
fn s10_conveyor_in_range_runs() {
    let path = repo().join("content/fixtures/sim/S10-conveyor-in-range.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let mut sim = Simulator::from_program(
        parsed.program,
        &SimConfig {
            max_cycles: 6,
            ..SimConfig::default()
        },
    );
    sim.ensure_setup().unwrap();
    // Five optic rising edges → CTU reaches PV=5 → divert on.
    for _ in 0..5 {
        sim.set_pin(2, true).unwrap();
        sim.step().unwrap();
        sim.set_pin(2, false).unwrap();
        sim.step().unwrap();
    }
    assert!(sim.pin(11), "divert should be on at full count");
    assert!(!sim.pin(10), "motor should stop when QU");
    let text = sim.trace_text();
    assert!(text.contains("CTU"), "{text}");
}

#[test]
fn s11_retain_survives_soft_restart() {
    let path = repo().join("content/fixtures/sim/S11-retain-restart.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let mut sim = Simulator::from_program(
        parsed.program.clone(),
        &SimConfig {
            max_cycles: 1,
            ..SimConfig::default()
        },
    );
    sim.run(1).unwrap();
    assert_eq!(
        sim.var("rebootCount").and_then(|v| v.as_u64().ok()),
        Some(1)
    );
    let gain = sim.var("calGain").and_then(|v| v.as_f64().ok()).unwrap();
    assert!((gain - 1.5).abs() < 1e-9);
    let image = sim.retain_image();
    assert!(image.contains_key("rebootCount"));

    sim.soft_restart().unwrap();
    sim.run(1).unwrap();
    assert_eq!(
        sim.var("rebootCount").and_then(|v| v.as_u64().ok()),
        Some(2),
        "rebootCount must accumulate across soft restart"
    );
    // calGain was written to NV as 1.5; setup does not reset PERSISTENT.
    let gain2 = sim.var("calGain").and_then(|v| v.as_f64().ok()).unwrap();
    assert!((gain2 - 1.5).abs() < 1e-9);

    // Fresh instance seeded from exported image behaves the same.
    let mut sim2 = Simulator::from_program(
        parsed.program,
        &SimConfig {
            retain_seed: image,
            max_cycles: 1,
            ..SimConfig::default()
        },
    );
    sim2.ensure_setup().unwrap();
    assert_eq!(
        sim2.var("rebootCount").and_then(|v| v.as_u64().ok()),
        Some(2)
    );
}

#[test]
fn s17_reservoir_plant_fills() {
    let path = repo().join("content/fixtures/sim/S17-reservoir-plant.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let mut sim = Simulator::from_program(
        parsed.program,
        &SimConfig {
            max_cycles: 8,
            ..SimConfig::default()
        },
    );
    sim.run(8).unwrap();
    let level = sim.plant_level().unwrap();
    assert!(level > 20.0, "expected fill after 8s, level={level}");
    let text = sim.trace_text();
    assert!(text.contains("PLANT TANK"), "{text}");
    assert!(text.contains("DISP LCD"), "{text}");
}

#[test]
fn s18_pid_estop_heats_then_halts() {
    let path = repo().join("content/fixtures/sim/S18-pid-estop.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let mut sim = Simulator::from_program(
        parsed.program,
        &SimConfig {
            max_cycles: 20,
            ..SimConfig::default()
        },
    );
    sim.run(15).unwrap();
    assert!(sim.plant_temp().unwrap() > 28.0);
    sim.set_pin(2, true).unwrap();
    sim.set_pin(2, false).unwrap();
    assert_eq!(
        sim.var("sysSafetyHalt").and_then(|v| v.as_bool().ok()),
        Some(true)
    );
    let text = sim.trace_text();
    assert!(text.contains("E-STOP"), "{text}");
}

#[test]
fn s19_display_stubs_emit_disp_and_tone() {
    assert_trace("S19-display-stubs.asl.toml", 1);
}

#[test]
fn s14_isr_edges_fire_on_set_pin() {
    let path = repo().join("content/fixtures/sim/S14-isr-edge-modes.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let mut sim = Simulator::from_program(parsed.program, &SimConfig::default());
    sim.ensure_setup().unwrap();
    sim.set_pin(10, true).unwrap();
    assert!(sim.trace_text().contains("ISR_FIRE pin=10 mode=RISING"));
    sim.set_pin(11, true).unwrap();
    sim.set_pin(11, false).unwrap();
    assert!(sim.trace_text().contains("ISR_FIRE pin=11 mode=FALLING"));
    sim.set_pin(12, true).unwrap();
    assert!(sim.trace_text().contains("ISR_FIRE pin=12 mode=CHANGE"));
}

#[test]
fn s15_encoder_counts_on_change() {
    let path = repo().join("content/fixtures/sim/S15-encoder-quadrature.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let mut sim = Simulator::from_program(parsed.program, &SimConfig::default());
    sim.ensure_setup().unwrap();
    sim.set_pin(21, false).unwrap();
    sim.set_pin(20, true).unwrap();
    assert_eq!(sim.var("encoderCount").and_then(|v| v.as_i64().ok()), Some(1));
    sim.step().unwrap();
    assert!(sim.trace_text().contains("count=1"));
}

#[test]
fn s16_ring_buffer_overflow_path() {
    let path = repo().join("content/fixtures/sim/S16-isr-ring-buffer.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let mut sim = Simulator::from_program(parsed.program, &SimConfig::default());
    sim.ensure_setup().unwrap();
    for _ in 0..4 {
        sim.set_pin(14, false).unwrap();
        sim.set_pin(14, true).unwrap();
    }
    assert_eq!(
        sim.var("bufOverflow").and_then(|v| v.as_bool().ok()),
        Some(true)
    );
    sim.step().unwrap();
    assert!(sim.trace_text().contains("OVERFLOW"));
}

#[test]
fn project_sim_runs() {
    let path = repo().join("content/fixtures/projects/P01-uno-led-blink.dendri.toml");
    let parsed = parse_project_file(&path).unwrap();
    let cfg = SimConfig {
        max_cycles: 2,
        ..SimConfig::default()
    };
    let mut sim = Simulator::from_project(&parsed.project, &cfg);
    sim.run(2).unwrap();
    let text = sim.trace_text();
    assert!(text.contains("PIN_WRITE"));
    assert!(text.contains("WAIT"));
}

#[test]
fn s29_modbus_tcp_trace() {
    let mut mb_input_seed = std::collections::HashMap::new();
    mb_input_seed.insert((1, 10), 0x00AB);
    let mut mb_discrete_seed = std::collections::HashMap::new();
    mb_discrete_seed.insert((1, 2), true);
    assert_trace_with(
        "S29-modbus-tcp.asl.toml",
        1,
        SimConfig {
            max_cycles: 1,
            mb_input_seed,
            mb_discrete_seed,
            ..SimConfig::default()
        },
    );
}

#[test]
fn s29_modbus_tcp_map_survives_soft_restart() {
    let path = repo().join("content/fixtures/sim/S29-modbus-tcp.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let mut mb_input_seed = std::collections::HashMap::new();
    mb_input_seed.insert((1, 10), 0x00AB);
    let mut sim = Simulator::from_program(
        parsed.program,
        &SimConfig {
            max_cycles: 1,
            mb_input_seed,
            ..SimConfig::default()
        },
    );
    sim.run(1).unwrap();
    assert_eq!(sim.mb_holding(1, 100), 0x1234);
    assert!(sim.mb_coil(1, 0));
    sim.soft_restart().unwrap();
    assert_eq!(sim.mb_holding(1, 100), 0x1234);
    assert!(sim.mb_coil(1, 0));
    assert_eq!(sim.modbus().read_input(1, 10).0, 0x00AB);
}
