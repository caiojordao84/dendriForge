//! Acceptance: language → ASL AST → generate, and sim on produced ASL.

use std::collections::HashMap;
use std::path::PathBuf;

use dendri_ast::SourceLanguage;
use dendri_frontends::{emit_source, parse_source, roundtrip_language};
use dendri_parse::parse_program_file;
use dendri_sim::{SimConfig, Simulator};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_asl(rel: &str) -> dendri_ast::Program {
    let path = repo_root().join(rel);
    parse_program_file(&path)
        .unwrap_or_else(|e| panic!("load {rel}: {e}"))
        .program
}

fn s01_program() -> dendri_ast::Program {
    load_asl("content/fixtures/sim/S01-blink-set-bool-wait.asl.toml")
}

fn s02_program() -> dendri_ast::Program {
    load_asl("content/fixtures/sim/S02-ton-on-delay.asl.toml")
}

fn s03_program() -> dendri_ast::Program {
    load_asl("content/fixtures/sim/S03-control-flow-get-bool.asl.toml")
}

fn text_langs() -> [SourceLanguage; 3] {
    [
        SourceLanguage::Cpp,
        SourceLanguage::Micropython,
        SourceLanguage::St,
    ]
}

#[test]
fn cpp_roundtrip_s01() {
    roundtrip_language(SourceLanguage::Cpp, &s01_program()).expect("cpp roundtrip");
}

#[test]
fn micropython_roundtrip_s01() {
    roundtrip_language(SourceLanguage::Micropython, &s01_program()).expect("mpy roundtrip");
}

#[test]
fn st_roundtrip_s01() {
    roundtrip_language(SourceLanguage::St, &s01_program()).expect("st roundtrip");
}

#[test]
fn roundtrip_s02_ton_all_text_langs() {
    let p = s02_program();
    for lang in text_langs() {
        roundtrip_language(lang, &p).unwrap_or_else(|e| panic!("{lang:?} S02: {e}"));
    }
}

#[test]
fn roundtrip_s03_control_flow_all_text_langs() {
    let p = s03_program();
    for lang in text_langs() {
        roundtrip_language(lang, &p).unwrap_or_else(|e| panic!("{lang:?} S03: {e}"));
    }
}

#[test]
fn parse_cpp_fixture_runs_sim() {
    let src = std::fs::read_to_string(
        repo_root().join("content/fixtures/frontends/S01-blink.cpp"),
    )
    .expect("cpp fixture");
    let program = parse_source(SourceLanguage::Cpp, &src).expect("parse cpp");
    let mut sim = Simulator::from_program(
        program,
        &SimConfig {
            max_cycles: 2,
            ..SimConfig::default()
        },
    );
    sim.run(2).expect("sim");
    let trace = sim.trace_text();
    assert!(trace.contains("PIN_WRITE pin=13 value=true"), "{trace}");
    assert!(trace.contains("WAIT ms=100"), "{trace}");
}

#[test]
fn parse_micropython_fixture_runs_sim() {
    let src = std::fs::read_to_string(
        repo_root().join("content/fixtures/frontends/S01-blink.py"),
    )
    .expect("mpy fixture");
    let program = parse_source(SourceLanguage::Micropython, &src).expect("parse mpy");
    let mut sim = Simulator::from_program(
        program,
        &SimConfig {
            max_cycles: 2,
            ..SimConfig::default()
        },
    );
    sim.run(2).expect("sim");
    let trace = sim.trace_text();
    assert!(trace.contains("PIN_WRITE pin=13"), "{trace}");
}

#[test]
fn parse_st_fixture_runs_sim() {
    let src = std::fs::read_to_string(
        repo_root().join("content/fixtures/frontends/S01-blink.st"),
    )
    .expect("st fixture");
    let program = parse_source(SourceLanguage::St, &src).expect("parse st");
    let mut sim = Simulator::from_program(
        program,
        &SimConfig {
            max_cycles: 2,
            ..SimConfig::default()
        },
    );
    sim.run(2).expect("sim");
    let trace = sim.trace_text();
    assert!(trace.contains("PIN_WRITE pin=13"), "{trace}");
}

#[test]
fn parse_s02_ton_fixtures_run_sim() {
    let cases = [
        (SourceLanguage::Cpp, "content/fixtures/frontends/S02-ton.cpp"),
        (
            SourceLanguage::Micropython,
            "content/fixtures/frontends/S02-ton.py",
        ),
        (SourceLanguage::St, "content/fixtures/frontends/S02-ton.st"),
    ];
    for (lang, rel) in cases {
        let src = std::fs::read_to_string(repo_root().join(rel)).expect(rel);
        let program = parse_source(lang, &src).unwrap_or_else(|e| panic!("parse {rel}: {e}"));
        let mut sim = Simulator::from_program(
            program,
            &SimConfig {
                max_cycles: 3,
                ..SimConfig::default()
            },
        );
        sim.run(3).unwrap_or_else(|e| panic!("sim {rel}: {e}"));
        let trace = sim.trace_text();
        assert!(
            trace.contains("TON id=t1"),
            "{lang:?} S02 missing TON trace:\n{trace}"
        );
        assert!(
            trace.contains("WAIT ms=100"),
            "{lang:?} S02 missing WAIT:\n{trace}"
        );
    }
}

#[test]
fn parse_s03_control_flow_fixtures_run_sim() {
    let cases = [
        (
            SourceLanguage::Cpp,
            "content/fixtures/frontends/S03-control-flow.cpp",
        ),
        (
            SourceLanguage::Micropython,
            "content/fixtures/frontends/S03-control-flow.py",
        ),
        (
            SourceLanguage::St,
            "content/fixtures/frontends/S03-control-flow.st",
        ),
    ];
    let mut pins = HashMap::new();
    pins.insert(2, true); // button pressed → then branch
    for (lang, rel) in cases {
        let src = std::fs::read_to_string(repo_root().join(rel)).expect(rel);
        let program = parse_source(lang, &src).unwrap_or_else(|e| panic!("parse {rel}: {e}"));
        let mut sim = Simulator::from_program(
            program,
            &SimConfig {
                max_cycles: 2,
                initial_pins: pins.clone(),
                ..SimConfig::default()
            },
        );
        sim.run(2).unwrap_or_else(|e| panic!("sim {rel}: {e}"));
        let trace = sim.trace_text();
        assert!(
            trace.contains("PIN_READ pin=2 value=true"),
            "{lang:?} S03 missing GET_BOOL/PIN_READ:\n{trace}"
        );
        assert!(
            trace.contains("PIN_WRITE pin=13 value=true"),
            "{lang:?} S03 missing SET_BOOL high:\n{trace}"
        );
        assert!(
            trace.contains("WAIT ms=50"),
            "{lang:?} S03 missing WAIT:\n{trace}"
        );
    }
}

#[test]
fn emit_from_asl_matches_language_parse() {
    for (label, p) in [
        ("S01", s01_program()),
        ("S02", s02_program()),
        ("S03", s03_program()),
    ] {
        for lang in text_langs() {
            let text = emit_source(lang, &p).unwrap();
            let again = parse_source(lang, &text).unwrap();
            assert_eq!(again.setup, p.setup, "{label} {lang:?} setup");
            assert_eq!(again.loop_body, p.loop_body, "{label} {lang:?} loop");
            assert_eq!(again.data, p.data, "{label} {lang:?} data");
            assert_eq!(again.hardware, p.hardware, "{label} {lang:?} hardware");
        }
    }
}
