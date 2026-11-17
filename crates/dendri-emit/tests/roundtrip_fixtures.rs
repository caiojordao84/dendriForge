//! Golden + project AST round-trip (parse → emit → parse → equality).

use std::fs;
use std::path::PathBuf;

use dendri_emit::{roundtrip_program, roundtrip_project};
use dendri_parse::{parse_program_file, parse_project_file};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn all_golden_roundtrip() {
    let root = repo().join("content/fixtures/golden");
    let mut files: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    files.sort();
    for path in files {
        let rel = path.file_name().unwrap().to_string_lossy().into_owned();
        let parsed = parse_program_file(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        roundtrip_program(&parsed.program).unwrap_or_else(|e| panic!("{rel}: {e}"));
    }
}

#[test]
fn sim_and_example_roundtrip() {
    let fixtures = repo().join("content/fixtures");
    let names = [
        "examples/basic-blink-logic.asl.toml",
        "examples/dual-button-led-control.asl.toml",
        "sim/S01-blink-set-bool-wait.asl.toml",
        "sim/S02-ton-on-delay.asl.toml",
        "sim/S08-scenario1-three-mode-blink.asl.toml",
    ];
    for rel in names {
        let path = fixtures.join(rel);
        let parsed = parse_program_file(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        roundtrip_program(&parsed.program).unwrap_or_else(|e| panic!("{rel}: {e}"));
    }
}

#[test]
fn project_roundtrip() {
    for rel in [
        "content/fixtures/projects/P01-uno-led-blink.dendri.toml",
        "content/fixtures/canvas/C02-scenario1-modes.dendri.toml",
    ] {
        let path = repo().join(rel);
        let parsed = parse_project_file(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        roundtrip_project(&parsed.project).unwrap_or_else(|e| panic!("{rel}: {e}"));
    }
}
