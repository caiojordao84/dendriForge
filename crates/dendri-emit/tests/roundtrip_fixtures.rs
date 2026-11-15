//! Golden + project AST round-trip (parse → emit → parse → equality).

use std::path::PathBuf;

use dendri_emit::{roundtrip_program, roundtrip_project};
use dendri_parse::{parse_program_file, parse_project_file};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn golden_and_example_roundtrip() {
    let fixtures = repo().join("content/fixtures");
    let names = [
        "golden/G01-gpio-all-modes-probe.asl.toml",
        "golden/G03-traffic-light-sequencer.asl.toml",
        "golden/G05-pwm-fade-breathing-led.asl.toml",
        "golden/G08-stopwatch-lap-timer.asl.toml",
        "golden/G17-retain-power-cycle-counter.asl.toml",
        "examples/basic-blink-logic.asl.toml",
        "sim/S01-blink-set-bool-wait.asl.toml",
        "sim/S02-ton-on-delay.asl.toml",
    ];
    for rel in names {
        let path = fixtures.join(rel);
        let parsed = parse_program_file(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        roundtrip_program(&parsed.program).unwrap_or_else(|e| panic!("{rel}: {e}"));
    }
}

#[test]
fn project_roundtrip() {
    let path = repo().join("content/fixtures/projects/P01-uno-led-blink.dendri.toml");
    let parsed = parse_project_file(&path).unwrap();
    roundtrip_project(&parsed.project).unwrap();
}
