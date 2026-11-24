//! Phase 6b Real-Toolchain Differential Gate tests (`cargo test --workspace`).
//!
//! Strictly executes host `g++` (Arduino deterministic shim) AND RuSTy `plc` (LLVM native
//! ST / Ladder / GRAFCET) with ZERO `#[ignore]` and ZERO silent skips.
//!
//! The nightly AVR oracle (`avr-gcc` + `simavr`) and official `arduino-cli` (`arduino:avr:uno`)
//! compile gates are executed via `dendri diff-check --suite all` (and verified explicitly below
//! when `DENDRI_DIFF_ALL=1` is set).

use dendri_assets::AssetCatalog;
use dendri_ast::{Project, ProjectMeta};
use dendri_diff::scenarios::{
    build_cpp_scenarios, build_st_rusty_scenarios, make_plc_board, make_uno_board,
};
use dendri_diff::{DiffError, DiffSuite, Toolchains, run_differential_suite};
use dendri_validate::{ValidateOptions, validate_project};
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn all_wave1_differential_scenarios_validate_against_live_asset_catalog() {
    let root = workspace_root();
    let catalog = AssetCatalog::load_validated(&root, &root.join("assets"))
        .expect("load live certified AssetCatalog");

    for sc in build_cpp_scenarios().expect("build C++ scenarios") {
        let project = Project {
            meta: ProjectMeta {
                schema_version: "2.0".to_owned(),
                asl_version: "0.1.0".to_owned(),
                requires: vec!["asl-core@1".to_owned()],
                name: sc.name.to_owned(),
                author: "DendriForge".to_owned(),
                description: "Phase 6b C++ Differential Scenario".to_owned(),
            },
            libraries: Vec::new(),
            packs: Vec::new(),
            comments: Vec::new(),
            boards: vec![make_uno_board(sc.name, &sc.bundle)],
            components: Vec::new(),
            wires: Vec::new(),
        };
        let bag = validate_project(&project, &catalog, &ValidateOptions::default());
        assert!(
            !bag.has_errors(),
            "C++ scenario `{}` failed dendri-validate: {:#?}",
            sc.name,
            bag.all()
        );
    }

    for sc in build_st_rusty_scenarios().expect("build ST/Ladder/SFC scenarios") {
        let project = Project {
            meta: ProjectMeta {
                schema_version: "2.0".to_owned(),
                asl_version: "0.1.0".to_owned(),
                requires: vec!["asl-core@1".to_owned()],
                name: sc.name.to_owned(),
                author: "DendriForge".to_owned(),
                description: "Phase 6b ST/Ladder/SFC Differential Scenario".to_owned(),
            },
            libraries: Vec::new(),
            packs: Vec::new(),
            comments: Vec::new(),
            boards: vec![make_plc_board(
                sc.name,
                sc.authoring_language,
                sc.scan_period_us,
                &sc.vm_bundle,
            )],
            components: Vec::new(),
            wires: Vec::new(),
        };
        let bag = validate_project(&project, &catalog, &ValidateOptions::default());
        assert!(
            !bag.has_errors(),
            "ST/Ladder/SFC scenario `{}` failed dendri-validate: {:#?}",
            sc.name,
            bag.all()
        );
    }
}

#[test]
fn phase6b_pr_gate_host_gxx_and_rusty_llvm_match_dendri_vm_bit_for_bit() {
    let toolchains = Toolchains::discover();
    let report = run_differential_suite(DiffSuite::PrGate, &toolchains)
        .expect("Phase 6b PR differential gate (g++ host + RuSTy LLVM) MUST pass 100%");
    assert_eq!(
        report.outcomes.len(),
        13,
        "expected 6 C++ host + 7 RuSTy ST/Ladder/SFC differential scenarios"
    );
}

#[test]
fn missing_toolchains_fail_explicitly_without_silent_skips() {
    let empty = Toolchains::default();
    assert!(matches!(
        run_differential_suite(DiffSuite::CppHost, &empty),
        Err(DiffError::ToolchainMissing { tool: "g++", .. })
    ));
    assert!(matches!(
        run_differential_suite(DiffSuite::RustySt, &empty),
        Err(DiffError::ToolchainMissing {
            tool: "plc (RuSTy)",
            ..
        })
    ));
    assert!(matches!(
        run_differential_suite(DiffSuite::ArduinoCli, &empty),
        Err(DiffError::ToolchainMissing {
            tool: "arduino-cli",
            ..
        })
    ));
    assert!(matches!(
        run_differential_suite(DiffSuite::AvrSimavr, &empty),
        Err(DiffError::ToolchainMissing {
            tool: "avr-gcc",
            ..
        })
    ));
}
