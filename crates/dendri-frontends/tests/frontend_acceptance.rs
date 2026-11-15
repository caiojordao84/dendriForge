//! Acceptance: language → ASL AST → generate, and sim on produced ASL.

use std::path::PathBuf;

use dendri_ast::SourceLanguage;
use dendri_frontends::{emit_source, parse_source, roundtrip_language};
use dendri_parse::parse_program_file;
use dendri_sim::{SimConfig, Simulator};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn s01_program() -> dendri_ast::Program {
    let path = repo_root().join("content/fixtures/sim/S01-blink-set-bool-wait.asl.toml");
    parse_program_file(&path).expect("load S01").program
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
fn emit_from_asl_matches_language_parse() {
    let p = s01_program();
    for lang in [
        SourceLanguage::Cpp,
        SourceLanguage::Micropython,
        SourceLanguage::St,
    ] {
        let text = emit_source(lang, &p).unwrap();
        let again = parse_source(lang, &text).unwrap();
        assert_eq!(again.setup, p.setup, "{lang:?}");
        assert_eq!(again.loop_body, p.loop_body, "{lang:?}");
    }
}
