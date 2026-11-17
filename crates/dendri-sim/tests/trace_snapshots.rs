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
