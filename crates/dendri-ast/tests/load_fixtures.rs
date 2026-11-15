//! Phase 0: structural deserialize of converted `.asl.toml` fixtures.
//! Semantic validate / round-trip lands in Phase 1.

use std::fs;
use std::path::PathBuf;

use dendri_ast::Program;

fn content_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/fixtures")
}

fn load_program(path: &std::path::Path) -> Program {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

#[test]
fn golden_mvp_fixtures_deserialize() {
    let root = content_root().join("golden");
    for name in [
        "G01-gpio-all-modes-probe.asl.toml",
        "G03-traffic-light-sequencer.asl.toml",
        "G05-pwm-fade-breathing-led.asl.toml",
        "G08-stopwatch-lap-timer.asl.toml",
        "G17-retain-power-cycle-counter.asl.toml",
    ] {
        let path = root.join(name);
        let prog = load_program(&path);
        assert!(
            prog.metadata.project_name.is_some(),
            "{name} missing project_name"
        );
        assert!(
            !prog.setup.is_empty() || !prog.loop_body.is_empty(),
            "{name} empty body"
        );
    }
}

#[test]
fn error_corpus_fixtures_deserialize() {
    let root = content_root().join("errors");
    // E01 omits version (validate-time error). E02–E07 are structural.
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
        let prog = load_program(&path);
        assert!(
            prog.metadata.expect_error.is_some(),
            "{name} missing expect_error"
        );
        assert!(
            prog.metadata.expect_error.as_ref().unwrap().starts_with("ERR_"),
            "{name} expect_error must be ERR_*"
        );
    }

    let e01 = load_program(&root.join("E01-missing-metadata-field.asl.toml"));
    assert!(e01.metadata.project_name.is_none());
    assert_eq!(
        e01.metadata.expect_error.as_deref(),
        Some("ERR_METADATA_MISSING_FIELD")
    );
}

#[test]
fn example_basic_blink_deserializes() {
    let path = content_root().join("examples/basic-blink-logic.asl.toml");
    let prog = load_program(&path);
    assert_eq!(prog.functions.len(), 1);
    assert_eq!(prog.functions[0].name, "F_TOGGLE_BOOL");
}
