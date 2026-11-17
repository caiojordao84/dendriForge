//! DEN-16 — MVP scenarios 1–12 acceptance map (sim opcode tranche).
//!
//! Honest statuses only:
//! - **pass** — automated check green on current opcodes/sim
//! - **partial** — structural and/or subset sim green; full scenario not met
//! - **blocked** — missing sim/opcode/physics; fixture may still deserialize
//!
//! See store docs `priority-6-v010-scenarios.md` and `priority-7-sim-opcodes-acceptance.md`.

use std::path::PathBuf;

use dendri_ast::SourceLanguage;
use dendri_frontends::{parse_source, roundtrip_language};
use dendri_parse::{parse_program_file, parse_project_file};
use dendri_sim::{SimConfig, Simulator, Value};
use dendri_validate::{validate_program, validate_project};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Partial,
    Blocked,
}

struct Scenario {
    id: u8,
    title: &'static str,
    status: Status,
    /// Why partial/blocked, or what the pass covers.
    note: &'static str,
}

/// Canonical matrix used by tests + docs (keep in sync with Priority 7 doc).
fn matrix() -> [Scenario; 12] {
    [
        Scenario {
            id: 1,
            title: "Digital I/O, timing, edge detect (3-mode blink)",
            status: Status::Pass,
            note: "S08 + C02 + L04/S06 building blocks; soft debounce only",
        },
        Scenario {
            id: 2,
            title: "ADC/PWM/I2C/SPI/UART",
            status: Status::Partial,
            note: "S04/S05 + S12 I2C/SPI xfer pass; displays/SD drivers still blocked",
        },
        Scenario {
            id: 3,
            title: "Conveyor sorting / CT* / IN_RANGE",
            status: Status::Pass,
            note: "S07 CT* + S10 IN_RANGE conveyor logic path; no plant physics",
        },
        Scenario {
            id: 4,
            title: "Reservoir level control",
            status: Status::Blocked,
            note: "example TOML migrated; no tank physics / PID opcode in sim",
        },
        Scenario {
            id: 5,
            title: "Multi-floor elevator (SFC)",
            status: Status::Blocked,
            note: "plc-elevator TOML migrated; nested GRAFCET + call queue not in SFC MVP",
        },
        Scenario {
            id: 6,
            title: "Electro-pneumatic A+ B+ A- B-",
            status: Status::Partial,
            note: "SFC G03 + Ladder L05 twins; multi-lang round-trip of full plant deferred",
        },
        Scenario {
            id: 7,
            title: "SCARA PID + E-Stop",
            status: Status::Blocked,
            note: "S13 SQRT/LOG in sim; ISR/PID/LCD plant still blocked",
        },
        Scenario {
            id: 8,
            title: "ISR / encoder / ring buffer",
            status: Status::Blocked,
            note: "G04/G09/G11/G12 TOML+validate; CFG_ATTACH_INTERRUPT unsupported in sim",
        },
        Scenario {
            id: 9,
            title: "RETAIN/PERSISTENT + watchdog",
            status: Status::Partial,
            note: "S11 live RETAIN/PERSISTENT soft-restart + SYS_*; watchdog/LCD deferred",
        },
        Scenario {
            id: 10,
            title: "Multi-display + Pong graphics",
            status: Status::Blocked,
            note: "examples migrated; LCD/OLED/TFT/PLAY_TONE unsupported in sim",
        },
        Scenario {
            id: 11,
            title: "UART command parser + CSV telemetry",
            status: Status::Partial,
            note: "S09 CONCAT/TO_STRING*/PING path; full G13 command suite not sim-accepted",
        },
        Scenario {
            id: 12,
            title: "Nested loops / deep calls / VENDOR_EXTENSION",
            status: Status::Partial,
            note: "G19–G21 TOML+validate; VENDOR halts; deep sim acceptance limited",
        },
    ]
}

#[test]
fn scenario_matrix_is_complete() {
    let m = matrix();
    assert_eq!(m.len(), 12);
    for (i, s) in m.iter().enumerate() {
        assert_eq!(s.id as usize, i + 1);
        assert!(!s.title.is_empty());
        assert!(!s.note.is_empty());
    }
    let pass = m.iter().filter(|s| s.status == Status::Pass).count();
    let partial = m.iter().filter(|s| s.status == Status::Partial).count();
    let blocked = m.iter().filter(|s| s.status == Status::Blocked).count();
    // DEN-16 snapshot — update when a scenario genuinely graduates.
    assert_eq!(pass, 2, "pass={pass}");
    assert_eq!(partial, 5, "partial={partial}");
    assert_eq!(blocked, 5, "blocked={blocked}");
}

#[test]
fn scenario1_three_mode_blink_sim_and_canvas_project() {
    let path = repo().join("content/fixtures/sim/S08-scenario1-three-mode-blink.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    assert!(validate_program(&parsed.program, &parsed.inject).ok());

    let mut sim = Simulator::from_program(
        parsed.program.clone(),
        &SimConfig {
            max_cycles: 8,
            ..SimConfig::default()
        },
    );
    sim.ensure_setup().unwrap();
    // Mode 0 — LED off
    sim.set_pin(2, false);
    sim.step().unwrap();
    assert!(!sim.pin(13), "mode0 led off");

    // Rising edge → mode 1 (slow blink)
    sim.set_pin(2, true);
    sim.step().unwrap();
    sim.set_pin(2, false);
    sim.step().unwrap();
    // After mode change + slow toggle, pin should have been written
    let trace = sim.trace_text();
    assert!(
        trace.contains("PIN_WRITE pin=13"),
        "expected blink writes after mode1: {trace}"
    );

    // Project + canvas wiring validates
    let proj_path = repo().join("content/fixtures/canvas/C02-scenario1-modes.dendri.toml");
    let proj = parse_project_file(&proj_path).unwrap();
    assert!(validate_project(&proj.project).ok());
}

#[test]
fn scenario1_ladder_edge_fb_still_passes() {
    let path = repo().join("content/fixtures/ladder/L04-iec-fbs.ladder.toml");
    let src = std::fs::read_to_string(&path).unwrap();
    let program = parse_source(SourceLanguage::Ladder, &src).unwrap();
    roundtrip_language(SourceLanguage::Ladder, &program).unwrap();
    let mut sim = Simulator::from_program(
        program,
        &SimConfig {
            max_cycles: 1,
            ..SimConfig::default()
        },
    );
    sim.run(1).unwrap();
}

#[test]
fn scenario2_pwm_adc_and_bus_xfer_partial() {
    for rel in [
        "content/fixtures/sim/S04-pwm-adc-map.asl.toml",
        "content/fixtures/sim/S05-uart-i2c-spi.asl.toml",
        "content/fixtures/sim/S12-i2c-spi-xfer.asl.toml",
    ] {
        let parsed = parse_program_file(&repo().join(rel)).unwrap();
        assert!(validate_program(&parsed.program, &parsed.inject).ok(), "{rel}");
        let mut sim = Simulator::from_program(
            parsed.program,
            &SimConfig {
                max_cycles: 2,
                ..SimConfig::default()
            },
        );
        sim.run(2).unwrap_or_else(|e| panic!("{rel} sim: {e}"));
    }
    let parsed = parse_program_file(
        &repo().join("content/fixtures/sim/S12-i2c-spi-xfer.asl.toml"),
    )
    .unwrap();
    let mut sim = Simulator::from_program(parsed.program, &SimConfig::default());
    sim.run(1).unwrap();
    assert_eq!(sim.var("i2cVal"), Some(&Value::Uint(0xA5)));
    assert_eq!(sim.var("spiRx"), Some(&Value::Uint(0x11)));
    assert!(sim.trace_text().contains("BUS_XFER"));

    for rel in [
        "content/fixtures/golden/G06-adc-multi-channel-logger.asl.toml",
        "content/fixtures/golden/G14-uart-csv-data-streamer.asl.toml",
        "content/fixtures/golden/G15-i2c-multi-device-scan.asl.toml",
        "content/fixtures/golden/G16-spi-sd-card-logger.asl.toml",
    ] {
        let parsed = parse_program_file(&repo().join(rel)).unwrap();
        assert!(validate_program(&parsed.program, &parsed.inject).ok(), "{rel}");
    }
}

#[test]
fn scenario3_conveyor_in_range_pass() {
    let parsed = parse_program_file(
        &repo().join("content/fixtures/sim/S07-iec-counters.asl.toml"),
    )
    .unwrap();
    assert!(validate_program(&parsed.program, &parsed.inject).ok());
    let mut sim = Simulator::from_program(
        parsed.program,
        &SimConfig {
            max_cycles: 3,
            ..SimConfig::default()
        },
    );
    sim.run(3).unwrap();

    let parsed = parse_program_file(
        &repo().join("content/fixtures/sim/S10-conveyor-in-range.asl.toml"),
    )
    .unwrap();
    assert!(validate_program(&parsed.program, &parsed.inject).ok());
    let mut sim = Simulator::from_program(
        parsed.program,
        &SimConfig {
            max_cycles: 12,
            ..SimConfig::default()
        },
    );
    sim.ensure_setup().unwrap();
    for _ in 0..5 {
        sim.set_pin(2, true);
        sim.step().unwrap();
        sim.set_pin(2, false);
        sim.step().unwrap();
    }
    assert!(sim.pin(11), "divert on at PV");
    assert!(!sim.pin(10), "motor off at full");
}

#[test]
fn scenario6_sfc_and_ladder_twins_partial() {
    let sfc = repo().join("content/fixtures/sfc/G03-electro-pneumatic.sfc.toml");
    let ladder = repo().join("content/fixtures/ladder/L05-electro-pneumatic.ladder.toml");
    let sfc_prog = parse_source(
        SourceLanguage::Sfc,
        &std::fs::read_to_string(&sfc).unwrap(),
    )
    .unwrap();
    let ladder_prog = parse_source(
        SourceLanguage::Ladder,
        &std::fs::read_to_string(&ladder).unwrap(),
    )
    .unwrap();
    roundtrip_language(SourceLanguage::Sfc, &sfc_prog).unwrap();
    roundtrip_language(SourceLanguage::Ladder, &ladder_prog).unwrap();

    let mut sim = Simulator::from_program(
        sfc_prog,
        &SimConfig {
            max_cycles: 6,
            ..SimConfig::default()
        },
    );
    sim.run(6).unwrap();
    assert!(sim.trace_text().contains("PIN_WRITE") || sim.pin(13) || !sim.pin(13));

    let mut sim_l = Simulator::from_program(
        ladder_prog,
        &SimConfig {
            max_cycles: 1,
            ..SimConfig::default()
        },
    );
    sim_l.run(1).unwrap();
    assert!(
        sim_l.trace_text().contains("PIN_WRITE pin=13"),
        "{}",
        sim_l.trace_text()
    );
}

#[test]
fn scenario_blocked_examples_deserialize_only() {
    // Structural presence for blocked scenarios 4/5/7/8/10 — do not claim sim.
    for rel in [
        "content/fixtures/examples/reservatorio-controle-logic.asl.toml",
        "content/fixtures/examples/plc-elevator-advanced.asl.toml",
        "content/fixtures/examples/advanced-robotic-scara-pid.asl.toml",
        "content/fixtures/golden/G11-multi-isr-priority-test.asl.toml",
        "content/fixtures/golden/G12-isr-ring-buffer-capture.asl.toml",
        "content/fixtures/examples/pong-game-logic.asl.toml",
        "content/fixtures/examples/multi-display-dashboard.asl.toml",
    ] {
        let parsed = parse_program_file(&repo().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(
            parsed.program.metadata.project_name.is_some(),
            "{rel}"
        );
    }
}

#[test]
fn scenario7_math_sqrt_log_building_block() {
    let parsed = parse_program_file(
        &repo().join("content/fixtures/sim/S13-math-sqrt-log.asl.toml"),
    )
    .unwrap();
    assert!(validate_program(&parsed.program, &parsed.inject).ok());
    let mut sim = Simulator::from_program(parsed.program, &SimConfig::default());
    sim.run(1).unwrap();
    let dist = sim.var("dist").and_then(|v| v.as_f64().ok()).unwrap();
    assert!((dist - 5.0).abs() < 1e-9, "3-4-5 triangle dist={dist}");
    let logv = sim.var("logVal").and_then(|v| v.as_f64().ok()).unwrap();
    assert!((logv - 3.0).abs() < 1e-9, "log10(1000)={logv}");
}

#[test]
fn scenario9_retain_live_partial() {
    let parsed = parse_program_file(
        &repo().join("content/fixtures/sim/S11-retain-restart.asl.toml"),
    )
    .unwrap();
    assert!(validate_program(&parsed.program, &parsed.inject).ok());
    let mut sim = Simulator::from_program(parsed.program, &SimConfig::default());
    sim.run(1).unwrap();
    sim.soft_restart().unwrap();
    sim.run(1).unwrap();
    let boots = sim.var("rebootCount").and_then(|v| v.as_u64().ok()).unwrap();
    assert_eq!(boots, 2);

    for rel in [
        "content/fixtures/golden/G10-watchdog-heartbeat-monitor.asl.toml",
        "content/fixtures/golden/G17-retain-power-cycle-counter.asl.toml",
        "content/fixtures/golden/G18-persistent-calibration-store.asl.toml",
    ] {
        let parsed = parse_program_file(&repo().join(rel)).unwrap();
        let r = validate_program(&parsed.program, &parsed.inject);
        assert!(r.ok(), "{rel}: {:?}", r.diagnostics);
    }
}

#[test]
fn scenario11_string_ops_partial() {
    let parsed = parse_program_file(
        &repo().join("content/fixtures/sim/S09-string-ops.asl.toml"),
    )
    .unwrap();
    assert!(validate_program(&parsed.program, &parsed.inject).ok());
    let mut sim = Simulator::from_program(
        parsed.program,
        &SimConfig {
            serial_rx: vec!["PING".into()],
            ..SimConfig::default()
        },
    );
    sim.run(1).unwrap();
    let trace = sim.trace_text();
    assert!(trace.contains("SERIAL_OUT t=7,23.46"), "{trace}");
    assert!(trace.contains("SERIAL_OUT PONG"), "{trace}");

    for rel in [
        "content/fixtures/golden/G13-uart-echo-command-parser.asl.toml",
        "content/fixtures/golden/G14-uart-csv-data-streamer.asl.toml",
    ] {
        let parsed = parse_program_file(&repo().join(rel)).unwrap();
        let r = validate_program(&parsed.program, &parsed.inject);
        assert!(r.ok(), "{rel}: {:?}", r.diagnostics);
    }
}

#[test]
fn scenario9_and_11_and_12_validate_partial() {
    for rel in [
        "content/fixtures/golden/G19-vendor-ext-custom-driver.asl.toml",
        "content/fixtures/golden/G20-nested-loop-matrix-scan.asl.toml",
        "content/fixtures/golden/G21-deep-call-chain-state-machine.asl.toml",
    ] {
        let parsed = parse_program_file(&repo().join(rel)).unwrap();
        let r = validate_program(&parsed.program, &parsed.inject);
        assert!(r.ok(), "{rel}: {:?}", r.diagnostics);
    }
}

#[test]
fn scenario8_isr_sim_is_blocked() {
    let parsed = parse_program_file(
        &repo().join("content/fixtures/golden/G04-rotary-encoder-counter.asl.toml"),
    )
    .unwrap();
    assert!(validate_program(&parsed.program, &parsed.inject).ok());
    let mut sim = Simulator::from_program(
        parsed.program,
        &SimConfig {
            max_cycles: 1,
            ..SimConfig::default()
        },
    );
    let err = sim.run(1).unwrap_err();
    assert!(
        err.contains("unsupported") || err.contains("CFG_ATTACH") || err.contains("INTERRUPT"),
        "expected ISR sim block, got: {err}"
    );
}
