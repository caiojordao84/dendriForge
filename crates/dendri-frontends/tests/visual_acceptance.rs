//! Acceptance: Ladder / SFC diagram TOML ↔ ASL ↔ sim.

use std::path::PathBuf;

use dendri_ast::SourceLanguage;
use dendri_frontends::{emit_source, parse_source, roundtrip_language};
use dendri_sim::{SimConfig, Simulator};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn ladder_l01_roundtrip_and_sim() {
    let path = repo_root().join("content/fixtures/ladder/L01-blink-actions.ladder.toml");
    let src = std::fs::read_to_string(&path).expect("L01");
    let program = parse_source(SourceLanguage::Ladder, &src).expect("parse ladder");
    assert_eq!(program.ladder.len(), 1);
    roundtrip_language(SourceLanguage::Ladder, &program).expect("ladder RT");

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
    assert!(trace.contains("WAIT ms=100"), "{trace}");
}

#[test]
fn ladder_l02_contact_coil_sim() {
    let path = repo_root().join("content/fixtures/ladder/L02-contact-coil.ladder.toml");
    let src = std::fs::read_to_string(&path).expect("L02");
    let program = parse_source(SourceLanguage::Ladder, &src).expect("parse");
    let mut sim = Simulator::from_program(
        program,
        &SimConfig {
            max_cycles: 1,
            ..SimConfig::default()
        },
    );
    sim.run(1).expect("sim");
    let trace = sim.trace_text();
    assert!(trace.contains("PIN_WRITE pin=13 value=true"), "{trace}");
}

#[test]
fn sfc_g01_roundtrip_and_sim() {
    let path = repo_root().join("content/fixtures/sfc/G01-blink-two-step.sfc.toml");
    let src = std::fs::read_to_string(&path).expect("G01 sfc");
    let program = parse_source(SourceLanguage::Sfc, &src).expect("parse sfc");
    assert_eq!(program.grafcet.len(), 1);
    roundtrip_language(SourceLanguage::Sfc, &program).expect("sfc RT");

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
    assert!(trace.contains("WAIT ms=100"), "{trace}");
}

#[test]
fn emit_ladder_sfc_from_asl_s01() {
    let path = repo_root().join("content/fixtures/sim/S01-blink-set-bool-wait.asl.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let program: dendri_ast::Program = toml::from_str(&text).unwrap();
    for lang in [SourceLanguage::Ladder, SourceLanguage::Sfc] {
        let emitted = emit_source(lang, &program).unwrap();
        assert!(!emitted.trim().is_empty(), "{lang:?}");
        let again = parse_source(lang, &emitted).unwrap();
        // Synthesized views should still drive sim pin writes after lower.
        let mut sim = Simulator::from_program(
            again,
            &SimConfig {
                max_cycles: 2,
                ..SimConfig::default()
            },
        );
        sim.run(2).unwrap();
        assert!(
            sim.trace_text().contains("PIN_WRITE"),
            "{lang:?} {}",
            sim.trace_text()
        );
    }
}
