//! Integration tests for `dendri-cli` Wave 1 commands:
//! `packs-check`, `bench-spice`, `translate`, and `roundtrip`.

use dendri_cli::bench_spice::{MAX_ALLOWED_REL_ERROR, run_bench_spice};
use dendri_cli::frontend::{
    CliBoardProfile, CliLanguage, CliStDialect, FrontendCliError, build_board_context,
    run_roundtrip, run_translate,
};
use dendri_cli::packs::{PacksCheckError, run_packs_check};
use dendri_span::ErrorCode;
use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn packs_check_verifies_live_assets_and_rejects_tampering_or_untracked_files() {
    let assets_dir = workspace_root().join("assets");
    let report = run_packs_check(&assets_dir).expect("live assets/packs must pass packs-check");
    assert_eq!(report.packs_verified, 4);
    assert!(report.total_symbols >= 10);
    assert!(report.pack_ids.contains(&"arduino-wire".to_owned()));
    assert!(report.pack_ids.contains(&"arduino-spi".to_owned()));
    assert!(report.pack_ids.contains(&"arduino-servo".to_owned()));
    assert!(report.pack_ids.contains(&"plc-modbus-tcp".to_owned()));

    // Negative 1: tampered SHA-256 checksum MUST fail packs-check
    let tmp = tempfile::tempdir().expect("tempdir");
    let tmp_packs = tmp.path().join("packs");
    fs::create_dir_all(&tmp_packs).expect("mkdir packs");
    for file in [
        "arduino-wire.pack.toml",
        "arduino-spi.pack.toml",
        "arduino-servo.pack.toml",
        "plc-modbus-tcp.pack.toml",
        "checksums.toml",
    ] {
        fs::copy(assets_dir.join("packs").join(file), tmp_packs.join(file)).expect("copy pack");
    }
    let tampered = fs::read_to_string(tmp_packs.join("arduino-wire.pack.toml")).expect("read wire")
        + "\n# tampered byte\n";
    fs::write(tmp_packs.join("arduino-wire.pack.toml"), tampered).expect("write tampered");
    let err = run_packs_check(tmp.path()).expect_err("tampered pack must fail");
    assert!(matches!(err, PacksCheckError::Violations(_)));

    // Negative 2: untracked `.pack.toml` missing from `checksums.toml` MUST fail
    fs::copy(
        assets_dir.join("packs").join("arduino-wire.pack.toml"),
        tmp_packs.join("arduino-wire.pack.toml"),
    )
    .expect("restore wire");
    fs::write(
        tmp_packs.join("rogue-pack.pack.toml"),
        "schema_version = \"2.0\"\n",
    )
    .expect("write rogue");
    let err_untracked = run_packs_check(tmp.path()).expect_err("untracked pack must fail");
    assert!(matches!(err_untracked, PacksCheckError::Violations(_)));
}

#[test]
fn bench_spice_passes_all_analytical_benchmarks_within_0_1_percent() {
    let report = run_bench_spice();
    assert_eq!(report.results.len(), 6);
    assert!(
        report.all_passed,
        "one or more SPICE analytical benchmarks exceeded 0.1%: {:#?}",
        report.results
    );
    assert!(report.max_rel_error < MAX_ALLOWED_REL_ERROR);
}

#[test]
fn roundtrip_and_translate_cover_all_four_wave1_languages_and_foreign_gate() {
    let assets_dir = workspace_root().join("assets");
    let plc_ctx = build_board_context(
        CliBoardProfile::Plc1200Open1214,
        CliStDialect::IecEd3,
        false,
        Some(&assets_dir),
    );
    let uno_ctx = build_board_context(
        CliBoardProfile::ArduinoUnoR3,
        CliStDialect::IecEd3,
        false,
        Some(&assets_dir),
    );

    // 1. Roundtrip C++
    let cpp_src = r#"// Uno counter sketch
#include <stdint.h>

int32_t acc = 0;

void loop() {
  acc = acc + 1;
}
"#;
    let cpp_rt = run_roundtrip(cpp_src, CliLanguage::Cpp, &uno_ctx).expect("C++ roundtrip");
    assert_eq!(cpp_rt.pou_count, 1);
    assert_eq!(cpp_rt.comment_count, 1);

    // 2. Roundtrip ST
    let st_src = r#"(* PLC latch *)
VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
END_VAR

PROGRAM OB_MAIN
    MotorOut := StartBtn OR MotorOut;
END_PROGRAM
"#;
    let st_rt = run_roundtrip(st_src, CliLanguage::St, &plc_ctx).expect("ST roundtrip");
    assert_eq!(st_rt.pou_count, 1);
    assert_eq!(st_rt.comment_count, 1);

    // 3. Roundtrip Ladder
    let lad_src = r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
END_VAR

NETWORK OB_MAIN
    RUNG 0 (n_r0): | | StartBtn @(0, 0, n_c0) -- ( ) MotorOut @(0, 1, n_coil);
END_NETWORK
"#;
    let lad_rt = run_roundtrip(lad_src, CliLanguage::Ladder, &plc_ctx).expect("Ladder roundtrip");
    assert_eq!(lad_rt.pou_count, 1);

    // 4. Roundtrip SFC
    let sfc_src = r#"VAR_GLOBAL
    Go AT %IX0.0 : BOOL;
    Lamp AT %QX0.0 : BOOL;
END_VAR

SFC_CHART OB_MAIN
    INITIAL_STEP S0:
    END_STEP
    STEP S1:
        N: Lamp;
    END_STEP
    TRANSITION T0 PRIORITY 0 FROM S0 TO S1 := Go;
    END_TRANSITION
END_SFC_CHART
"#;
    let sfc_rt = run_roundtrip(sfc_src, CliLanguage::Sfc, &plc_ctx).expect("SFC roundtrip");
    assert_eq!(sfc_rt.pou_count, 1);

    // 5. Cross-language translate: C++ -> ST -> C++ and Ladder -> ST
    let cpp_to_st = run_translate(cpp_src, CliLanguage::Cpp, CliLanguage::St, &plc_ctx)
        .expect("translate C++ -> ST");
    assert!(cpp_to_st.emitted.contains("PROGRAM OB_MAIN"));
    let st_to_cpp = run_translate(
        &cpp_to_st.emitted,
        CliLanguage::St,
        CliLanguage::Cpp,
        &uno_ctx,
    )
    .expect("translate ST -> C++");
    assert!(st_to_cpp.emitted.contains("void loop()"));

    let lad_to_st = run_translate(lad_src, CliLanguage::Ladder, CliLanguage::St, &plc_ctx)
        .expect("translate Ladder -> ST");
    assert!(lad_to_st.emitted.contains("MotorOut"));

    // 6. Foreign cross-language export gate: blocked by default, allowed with --allow-foreign
    let cpp_foreign = r#"#include <Arduino.h>
void loop() {
  PORTB |= (1 << 5);
}
"#;
    let blocked = run_translate(cpp_foreign, CliLanguage::Cpp, CliLanguage::St, &uno_ctx)
        .expect_err("Foreign cross-language export MUST be blocked without --allow-foreign");
    match blocked {
        FrontendCliError::Diagnostics(diags) => {
            assert!(
                diags
                    .iter()
                    .any(|d| d.code == ErrorCode::ErrForeignExportBlocked),
                "expected ErrForeignExportBlocked, got {diags:#?}"
            );
        }
        FrontendCliError::Conformance(msg) => panic!("unexpected conformance error: {msg}"),
    }

    let uno_allow_ctx = build_board_context(
        CliBoardProfile::ArduinoUnoR3,
        CliStDialect::IecEd3,
        true,
        Some(&assets_dir),
    );
    let allowed = run_translate(
        cpp_foreign,
        CliLanguage::Cpp,
        CliLanguage::St,
        &uno_allow_ctx,
    )
    .expect("Foreign cross-language export MUST succeed with --allow-foreign");
    assert!(allowed.emitted.contains("NOT TRANSLATED"));
}
