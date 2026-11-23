//! Phase 1 / DEN-15: validate goldens + error corpus + scenario fixtures.

use std::fs;
use std::path::PathBuf;

use dendri_parse::{parse_program_file, parse_project_file};
use dendri_validate::{validate_error_fixture, validate_program, validate_project};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn all_golden_programs_validate() {
    let root = repo().join("content/fixtures/golden");
    let mut files: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    files.sort();
    assert!(files.len() >= 21, "expected G01–G21, got {}", files.len());
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
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
fn migrated_examples_validate_structurally() {
    // Examples may reference opcodes that are catalogued but not all sim-runnable.
    // Validate still requires resolved catalog names + declared vars.
    let root = repo().join("content/fixtures/examples");
    let mut files: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .collect();
    files.sort();
    let mut ok = 0usize;
    let mut soft_fail = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let parsed = parse_program_file(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let result = validate_program(&parsed.program, &parsed.inject);
        if result.ok() {
            ok += 1;
        } else {
            soft_fail.push((name, result.diagnostics));
        }
    }
    // basic-blink + dual-button at minimum must validate; others documented in Priority 6.
    assert!(
        ok >= 2,
        "expected ≥2 examples to validate, ok={ok} soft_fail={soft_fail:?}"
    );
    let blink = repo().join("content/fixtures/examples/basic-blink-logic.toml");
    let parsed = parse_program_file(&blink).unwrap();
    assert!(validate_program(&parsed.program, &parsed.inject).ok());
}

#[test]
fn scenario1_s08_validates() {
    let path = repo().join("content/fixtures/sim/S08-scenario1-three-mode-blink.toml");
    let parsed = parse_program_file(&path).unwrap();
    let result = validate_program(&parsed.program, &parsed.inject);
    assert!(result.ok(), "{:?}", result.diagnostics);
}

#[test]
fn project_p01_and_c02_validate() {
    for rel in [
        "content/fixtures/projects/P01-uno-led-blink.toml",
        "content/fixtures/canvas/C02-scenario1-modes.toml",
    ] {
        let path = repo().join(rel);
        let parsed = parse_project_file(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let result = validate_project(&parsed.project);
        assert!(result.ok(), "{rel}: {:?}", result.diagnostics);
    }
}

#[test]
fn error_corpus_matches_expect_error() {
    let root = repo().join("content/fixtures/errors");
    for name in [
        "E01-missing-metadata-field.toml",
        "E02-unknown-data-type.toml",
        "E03-undeclared-variable.toml",
        "E04-hardware-pin-conflict.toml",
        "E05-setup-count-mismatch.toml",
        "E06-invalid-scope-write-const.toml",
        "E07-function-arity-mismatch.toml",
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
