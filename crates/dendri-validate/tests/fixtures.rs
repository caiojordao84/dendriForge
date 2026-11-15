//! Phase 1: validate Phase 0 goldens + error corpus.

use std::fs;
use std::path::PathBuf;

use dendri_parse::{parse_program_file, parse_project_file};
use dendri_validate::{validate_error_fixture, validate_program, validate_project};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn golden_programs_validate() {
    let root = repo().join("content/fixtures/golden");
    for name in [
        "G01-gpio-all-modes-probe.asl.toml",
        "G03-traffic-light-sequencer.asl.toml",
        "G05-pwm-fade-breathing-led.asl.toml",
        "G08-stopwatch-lap-timer.asl.toml",
        "G17-retain-power-cycle-counter.asl.toml",
    ] {
        let path = root.join(name);
        let parsed = parse_program_file(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let result = validate_program(&parsed.program, &parsed.inject);
        assert!(
            result.ok(),
            "{name} diagnostics: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn example_blink_validates() {
    let path = repo().join("content/fixtures/examples/basic-blink-logic.asl.toml");
    let parsed = parse_program_file(&path).unwrap();
    let result = validate_program(&parsed.program, &parsed.inject);
    assert!(result.ok(), "{:?}", result.diagnostics);
}

#[test]
fn project_p01_validates() {
    let path = repo().join("content/fixtures/projects/P01-uno-led-blink.dendri.toml");
    let parsed = parse_project_file(&path).unwrap();
    let result = validate_project(&parsed.project);
    assert!(result.ok(), "{:?}", result.diagnostics);
}

#[test]
fn error_corpus_matches_expect_error() {
    let root = repo().join("content/fixtures/errors");
    for name in [
        "E01-missing-metadata-field.asl.toml",
        "E02-unknown-data-type.asl.toml",
        "E03-undeclared-variable.asl.toml",
        "E04-hardware-pin-conflict.asl.toml",
        "E05-setup-count-mismatch.asl.toml",
        "E06-invalid-scope-write-const.asl.toml",
        "E07-function-arity-mismatch.asl.toml",
    ] {
        let path = root.join(name);
        let text = fs::read_to_string(&path).unwrap();
        let expect = dendri_parse::parse_program_str(&text)
            .unwrap()
            .program
            .metadata
            .expect_error
            .clone()
            .expect("expect_error");
        let result = validate_error_fixture(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let codes: Vec<_> = result.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert!(
            codes.iter().any(|c| *c == expect),
            "{name}: expected {expect}, got {codes:?}"
        );
    }
}
