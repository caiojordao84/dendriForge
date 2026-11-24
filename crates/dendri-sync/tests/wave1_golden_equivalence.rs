//! Stage C — Phase 6 Exit: Wave 1 Golden Equivalence, Live Sync, Packs & Feature Matrix Suite.
//!
//! Exercises all four Wave 1 pure-Rust frontends (`dendri-lang-cpp`, `dendri-lang-st`,
//! `dendri-lang-ladder`, `dendri-lang-sfc`), `dendri-sync`, `dendri-validate` against the live
//! certified `AssetCatalog`, `dendri-vm`, `dendri-sim-kernel`, and hash-locked `assets/packs/`.

use dendri_assets::AssetCatalog;
use dendri_ast::{
    ASL_ANALOG_FULL_SCALE, Assumption, AssumptionPolicy, AuthoringLanguage, Board, BoardLayout,
    BoardPresentation, BoardProgram, Codec, CodecKind, Comment, CommentAnchor, CommentOrigin,
    CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, ExecutionKind, Expr, ExprKind,
    FbKind, ForeignFidelity, HardwareBinding, HardwareKind, InterruptEdge, LValue, LadderCellKind,
    Literal, NodeId, ObKind, ParamDecl, ParamDirection, Pou, PouBody, PouKind, Project,
    ProjectMeta, PullMode, ScalarType, Stmt, StmtKind, StorageScope, TypeDecl, TypeRef, Wire,
    WireSignal, composed_clamped_rational,
};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, FeatureMatrixCollector, LanguageFrontend, LanguageId,
    PackChecksums, PackError, PackManifest, ReducedRationalPlan, StCommentDialect,
    assert_c1_multiset_invariant, assert_c5_comment_fields, assert_lossless_asl,
    collect_foreign_sites, contains_analog_full_scale_literal, contains_banned_magic_comment,
    hostile_comment_corpus, is_four_part_diagnostic, verify_frontend_roundtrip,
    verify_hostile_corpus_on_frontend,
};
use dendri_lang_cpp::{CppFrontend, emit_codec_conversion_cpp};
use dendri_lang_ladder::{LadderFrontend, lift_statements_to_network};
use dendri_lang_sfc::{SfcFrontend, project_chart_state_decls, project_chart_to_statements};
use dendri_lang_st::{StFrontend, emit_scaled_code_expr};
use dendri_sim_kernel::{InputTimeline, KernelConfig, SimKernel};
use dendri_span::{ErrorCode, Severity, Span};
use dendri_sync::{BoardDoc, ExportScope, SyncEditError, SyncEngine, ViewProjection};
use dendri_validate::{ValidateOptions, validate_project};
use dendri_vm::{BoardVm, ElectricalDigitalLevel, FbState, MemoryBoardIo, Value, VmConfig};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Shared Helpers (Workspace Root, Live AssetCatalog, Validation, VM Config)
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_live_catalog() -> AssetCatalog {
    let root = workspace_root();
    let assets_dir = root.join("assets");
    let loaded = AssetCatalog::load_validated(&root, &assets_dir);
    assert!(
        loaded.is_ok(),
        "live certified AssetCatalog must pass all 13 asset-check rules: {:?}",
        loaded.as_ref().err()
    );
    match loaded {
        Ok(cat) => cat,
        Err(_) => unreachable!(),
    }
}

fn build_single_board_project(board: Board) -> Project {
    Project {
        meta: ProjectMeta {
            schema_version: "2.0".to_owned(),
            asl_version: "0.1.0".to_owned(),
            requires: vec!["asl-core@1".to_owned()],
            name: format!("wave1_golden_{}", board.id),
            author: "DendriForge".to_owned(),
            description: "Wave 1 Golden Equivalence Verification".to_owned(),
        },
        libraries: Vec::new(),
        packs: Vec::new(),
        comments: Vec::new(),
        boards: vec![board],
        components: Vec::new(),
        wires: Vec::new(),
    }
}

fn make_uno_board(id: &str, bundle: &BoardProgramBundle) -> Board {
    Board {
        id: id.to_owned(),
        profile: "arduino-uno-r3".to_owned(),
        authoring_language: AuthoringLanguage::Cpp,
        execution: ExecutionKind::Cooperative,
        scan_period_us: None,
        layout: BoardLayout::default(),
        program: bundle.program.clone(),
        comments: bundle.comments.clone(),
        assumptions: bundle.assumptions.clone(),
        presentation: bundle.presentation.clone(),
        span: Span::default(),
    }
}

fn make_plc_board(id: &str, lang: AuthoringLanguage, bundle: &BoardProgramBundle) -> Board {
    Board {
        id: id.to_owned(),
        profile: "plc-1200-open-1214".to_owned(),
        authoring_language: lang,
        execution: ExecutionKind::PlcScan,
        scan_period_us: Some(10_000),
        layout: BoardLayout::default(),
        program: bundle.program.clone(),
        comments: bundle.comments.clone(),
        assumptions: bundle.assumptions.clone(),
        presentation: bundle.presentation.clone(),
        span: Span::default(),
    }
}

fn assert_valid_against_live_catalog(board: &Board, catalog: &AssetCatalog) {
    let project = build_single_board_project(board.clone());
    let bag = validate_project(&project, catalog, &ValidateOptions::default());
    assert!(
        !bag.has_errors(),
        "board `{}` failed dendri-validate against live AssetCatalog: {:#?}",
        board.id,
        bag.all()
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect::<Vec<_>>()
    );
}

fn vm_config_with_codecs(ctx: &BoardContext) -> VmConfig {
    let mut cfg = VmConfig::default();
    if ctx.profile_id == "plc-1200-open-1214" {
        cfg.clock_hz = 100_000_000;
    } else {
        cfg.clock_hz = 16_000_000;
    }
    cfg.codecs = ctx.codecs.clone();
    cfg
}

fn read_digital_out(io: &MemoryBoardIo, pin: &str) -> bool {
    io.digital_outputs.get(pin).copied().unwrap_or(false)
}

fn run_latch_timeline(
    board: &Board,
    ctx: &BoardContext,
    start_pin: &str,
    stop_pin: &str,
    motor_pin: &str,
    timeline: &[(bool, bool)],
) -> Vec<bool> {
    let compiled = BoardVm::new(board, vm_config_with_codecs(ctx));
    assert!(
        compiled.is_ok(),
        "compile BoardVm failed: {:?}",
        compiled.as_ref().err()
    );
    let Ok(mut vm) = compiled else {
        unreachable!();
    };
    let mut io = MemoryBoardIo::default();
    let mut trace = Vec::with_capacity(timeline.len());
    for &(start, stop) in timeline {
        io.set_digital_in(start_pin, start);
        io.set_digital_in(stop_pin, stop);
        match board.execution {
            ExecutionKind::Cooperative => {
                let res = vm.step_cooperative(10_000, &mut io);
                assert!(res.is_ok(), "cooperative scan failed: {:?}", res.err());
            }
            ExecutionKind::PlcScan => {
                let res = vm.run_plc_scan(10_000, &mut io);
                assert!(res.is_ok(), "plc scan failed: {:?}", res.err());
            }
            other => {
                assert!(
                    matches!(other, ExecutionKind::Cooperative | ExecutionKind::PlcScan),
                    "unsupported execution kind for Wave 1 test: {other:?}"
                );
            }
        }
        trace.push(read_digital_out(&io, motor_pin));
    }
    trace
}

// ---------------------------------------------------------------------------
// Scenario 1: Cross-Paradigm Motor Start/Stop Seal-In Latch (C++ <-> ST <-> Ladder)
// ---------------------------------------------------------------------------

#[test]
fn scenario_1_cross_paradigm_motor_start_stop_seal_in_latch() {
    let catalog = load_live_catalog();
    let cpp_fe = CppFrontend::new();
    let st_fe = StFrontend::new();
    let lad_fe = LadderFrontend::new();

    let uno_ctx = BoardContext::arduino_uno_r3("uno_latch");
    let plc_ctx = BoardContext::plc_1200_open_1214("plc_latch");

    // 1a. Idiomatic Arduino C++ motor start/stop seal-in latch
    let cpp_src = r#"#include <Arduino.h>

#define StartBtn 2
#define StopBtn 3
#define MotorOut 13

bool motorLatched = false;

void setup() {
  pinMode(StartBtn, INPUT);
  pinMode(StopBtn, INPUT);
  pinMode(MotorOut, OUTPUT);
}

void loop() {
  bool startPressed = digitalRead(StartBtn);
  bool stopPressed = digitalRead(StopBtn);
  motorLatched = (startPressed || motorLatched) && !stopPressed;
  digitalWrite(MotorOut, motorLatched);
}
"#;
    let cpp_parsed = cpp_fe.parse(cpp_src, &uno_ctx).expect("parse C++ latch");
    verify_frontend_roundtrip(&cpp_fe, &cpp_parsed.value, &uno_ctx)
        .expect("C++ LOSSLESS_ASL roundtrip");
    let cpp_board = make_uno_board("uno_latch", &cpp_parsed.value);
    assert_valid_against_live_catalog(&cpp_board, &catalog);

    // 1b. Idiomatic IEC 61131-3 ST motor start/stop seal-in latch
    let st_src = r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    StopBtn AT %IX0.1 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
END_VAR

PROGRAM OB_MAIN
    MotorOut := (StartBtn OR MotorOut) AND NOT StopBtn;
END_PROGRAM
"#;
    let st_parsed = st_fe.parse(st_src, &plc_ctx).expect("parse ST latch");
    verify_frontend_roundtrip(&st_fe, &st_parsed.value, &plc_ctx)
        .expect("ST LOSSLESS_ASL roundtrip");
    let st_board = make_plc_board("plc_st_latch", AuthoringLanguage::St, &st_parsed.value);
    assert_valid_against_live_catalog(&st_board, &catalog);

    // 1c. First-class structural Ladder Diagram (`PouBody::Network`, zero `__rail_` in ASL)
    let ladder_src = r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    StopBtn AT %IX0.1 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
END_VAR

NETWORK OB_MAIN
    RUNG 0 (n_r0): BRANCH @(0, 0, n_br) { | | StartBtn @(0, 0, n_c_start) || | | MotorOut @(1, 0, n_c_seal) } -- |/| StopBtn @(0, 1, n_c_stop) -- ( ) MotorOut @(0, 2, n_coil_motor);
END_NETWORK
"#;
    let lad_parsed = lad_fe
        .parse_and_validate(ladder_src, &plc_ctx)
        .expect("parse & validate Ladder latch");
    assert!(
        !format!("{:#?}", lad_parsed.value.program).contains("__rail_"),
        "ASL PouBody::Network MUST NOT persist synthetic __rail_ variables"
    );
    verify_frontend_roundtrip(&lad_fe, &lad_parsed.value, &plc_ctx)
        .expect("Ladder LOSSLESS_ASL roundtrip");
    let lad_board = make_plc_board(
        "plc_lad_latch",
        AuthoringLanguage::Ladder,
        &lad_parsed.value,
    );
    assert_valid_against_live_catalog(&lad_board, &catalog);

    // Identical stimulus timeline:
    // idle x2 -> start pulse -> release start (latched x3) -> stop pulse -> release x2 ->
    // simultaneous start+stop (stop-dominant) -> release
    let timeline: Vec<(bool, bool)> = vec![
        (false, false),
        (false, false),
        (true, false),
        (false, false),
        (false, false),
        (false, false),
        (false, true),
        (false, false),
        (false, false),
        (true, true),
        (false, false),
    ];
    let expected = vec![
        false, false, true, true, true, true, false, false, false, false, false,
    ];

    let cpp_trace = run_latch_timeline(&cpp_board, &uno_ctx, "D2", "D3", "D13", &timeline);
    let st_trace = run_latch_timeline(&st_board, &plc_ctx, "Ia.0", "Ia.1", "Qa.0", &timeline);
    let lad_trace = run_latch_timeline(&lad_board, &plc_ctx, "Ia.0", "Ia.1", "Qa.0", &timeline);

    assert_eq!(cpp_trace, expected, "C++ latch trace mismatch");
    assert_eq!(st_trace, expected, "ST latch trace mismatch");
    assert_eq!(lad_trace, expected, "Ladder latch trace mismatch");
    assert_eq!(cpp_trace, st_trace);
    assert_eq!(st_trace, lad_trace);

    // 1d. Verify deterministic SimKernel replay on the PLC Ladder board with InputTimeline
    let mut kernel_timeline = InputTimeline::new();
    kernel_timeline.push_digital(
        15_000,
        "plc_lad_latch",
        "Ia.0",
        ElectricalDigitalLevel::High,
    );
    kernel_timeline.push_digital(25_000, "plc_lad_latch", "Ia.0", ElectricalDigitalLevel::Low);
    kernel_timeline.push_digital(
        55_000,
        "plc_lad_latch",
        "Ia.1",
        ElectricalDigitalLevel::High,
    );
    kernel_timeline.push_digital(65_000, "plc_lad_latch", "Ia.1", ElectricalDigitalLevel::Low);
    let lad_project = build_single_board_project(lad_board.clone());
    let mut kernel = SimKernel::new(&lad_project, KernelConfig::default(), kernel_timeline)
        .expect("construct SimKernel");
    kernel.run_until(40_000).expect("run to latched phase");
    assert_eq!(
        kernel
            .snapshot()
            .boards
            .get("plc_lad_latch")
            .and_then(|b| b.digital_outputs.get("Qa.0")),
        Some(&true),
        "SimKernel Ladder board must remain latched after StartBtn release"
    );
    kernel.run_until(80_000).expect("run through StopBtn pulse");
    assert_eq!(
        kernel
            .snapshot()
            .boards
            .get("plc_lad_latch")
            .and_then(|b| b.digital_outputs.get("Qa.0")),
        Some(&false),
        "SimKernel Ladder board must unlatch after StopBtn pulse"
    );

    // 1e. Conservative Ladder lifting (`lift_statements_to_network`) <-> derived ST & C++ round-trip
    let st_main_stmts = match &st_parsed.value.program.pous[0].body {
        PouBody::Statements(stmts) => stmts,
        PouBody::Network(_) | PouBody::Chart(_) => panic!("expected ST statements"),
    };
    let lifted_net =
        lift_statements_to_network(st_main_stmts).expect("seal-in latch MUST lift to Ladder");
    assert_eq!(lifted_net.rungs.len(), 1);
    assert!(
        lifted_net.rungs[0]
            .cells
            .iter()
            .any(|c| matches!(c.kind, LadderCellKind::ParallelOr { .. })),
        "lifted seal-in rung must contain ParallelOr branch"
    );

    let mut lifted_bundle = st_parsed.value.clone();
    lifted_bundle.program.pous[0].body = PouBody::Network(lifted_net);
    lifted_bundle.program.pous[0].preferred_view = Some(AuthoringLanguage::Ladder);

    // Round-trip lifted network through LadderFrontend
    let lifted_lad_text = lad_fe
        .emit(&lifted_bundle, &plc_ctx)
        .expect("emit lifted Ladder")
        .value;
    assert!(!contains_banned_magic_comment(&lifted_lad_text));
    let reparsed_lifted = lad_fe
        .parse_and_validate(&lifted_lad_text, &plc_ctx)
        .expect("reparse lifted Ladder");
    assert_lossless_asl(&lifted_bundle.program, &reparsed_lifted.value.program)
        .expect("lifted Ladder LOSSLESS_ASL");

    // Derived ST emission from PouBody::Network -> ST parse -> validate -> VM bit-identical trace
    let derived_st_text = st_fe
        .emit(&lifted_bundle, &plc_ctx)
        .expect("emit derived ST from Ladder")
        .value;
    assert!(!contains_banned_magic_comment(&derived_st_text));
    let derived_st_parsed = st_fe
        .parse(&derived_st_text, &plc_ctx)
        .expect("reparse derived ST");
    let derived_st_board = make_plc_board(
        "plc_derived_st",
        AuthoringLanguage::St,
        &derived_st_parsed.value,
    );
    assert_valid_against_live_catalog(&derived_st_board, &catalog);
    let derived_st_trace = run_latch_timeline(
        &derived_st_board,
        &plc_ctx,
        "Ia.0",
        "Ia.1",
        "Qa.0",
        &timeline,
    );
    assert_eq!(
        derived_st_trace, expected,
        "derived ST from Ladder must match bit-for-bit"
    );

    // Derived C++ emission from PouBody::Network (mapped to Uno pins) -> C++ parse -> VM bit-identical trace
    let mut uno_lifted = lifted_bundle.clone();
    uno_lifted.program.hardware[0].pin = "D2".to_owned();
    uno_lifted.program.hardware[1].pin = "D3".to_owned();
    uno_lifted.program.hardware[2].pin = "D13".to_owned();
    let derived_cpp_text = cpp_fe
        .emit(&uno_lifted, &uno_ctx)
        .expect("emit derived C++ from Ladder")
        .value;
    assert!(!contains_banned_magic_comment(&derived_cpp_text));
    let derived_cpp_parsed = cpp_fe
        .parse(&derived_cpp_text, &uno_ctx)
        .expect("reparse derived C++ from Ladder");
    let derived_cpp_board = make_uno_board("uno_derived_cpp", &derived_cpp_parsed.value);
    assert_valid_against_live_catalog(&derived_cpp_board, &catalog);
    let derived_cpp_trace =
        run_latch_timeline(&derived_cpp_board, &uno_ctx, "D2", "D3", "D13", &timeline);
    assert_eq!(
        derived_cpp_trace, expected,
        "derived C++ from Ladder must match bit-for-bit"
    );

    // Conservative guard: non-liftable WHILE loop MUST return None
    let non_liftable = vec![Stmt::new(
        NodeId::new("n_while"),
        StmtKind::While {
            condition: Expr::new(ExprKind::Literal(Literal::Bool(true)), Span::default()),
            body: vec![Stmt::new(
                NodeId::new("n_brk"),
                StmtKind::Break,
                Span::default(),
            )],
        },
        Span::default(),
    )];
    assert_eq!(lift_statements_to_network(&non_liftable), None);
}

// ---------------------------------------------------------------------------
// Scenario 2: Timer (TON) + Counter (CTU) + Comparison Equivalence (C++ <-> ST <-> Ladder)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimerCounterSample {
    timer_done_out: bool,
    batch_ready_out: bool,
    ton_q: bool,
    ton_et_us: i64,
    ctu_q: bool,
    ctu_cv: i32,
}

fn run_timer_counter_timeline(
    board: &Board,
    ctx: &BoardContext,
    enable_pin: &str,
    reset_pin: &str,
    timer_out_pin: &str,
    batch_out_pin: &str,
    timeline: &[(bool, bool)],
) -> Vec<TimerCounterSample> {
    let compiled = BoardVm::new(board, vm_config_with_codecs(ctx));
    assert!(
        compiled.is_ok(),
        "compile BoardVm failed: {:?}",
        compiled.as_ref().err()
    );
    let Ok(mut vm) = compiled else {
        unreachable!();
    };
    let mut io = MemoryBoardIo::default();
    let mut trace = Vec::with_capacity(timeline.len());

    for &(enable, reset) in timeline {
        io.set_digital_in(enable_pin, enable);
        io.set_digital_in(reset_pin, reset);
        match board.execution {
            ExecutionKind::Cooperative => {
                let res = vm.step_cooperative(10_000, &mut io);
                assert!(res.is_ok(), "cooperative scan failed: {:?}", res.err());
            }
            ExecutionKind::PlcScan => {
                let res = vm.run_plc_scan(10_000, &mut io);
                assert!(res.is_ok(), "plc scan failed: {:?}", res.err());
            }
            other => {
                assert!(
                    matches!(other, ExecutionKind::Cooperative | ExecutionKind::PlcScan),
                    "unsupported execution kind: {other:?}"
                );
            }
        }

        let ton_state = vm.get_global("tonDelay");
        assert!(
            matches!(ton_state, Some(Value::Fb(FbState::Ton { .. }))),
            "expected TON FB state for `tonDelay`, got {ton_state:?}"
        );
        let (ton_q, ton_et_us) = match ton_state {
            Some(Value::Fb(FbState::Ton { q, et_us, .. })) => (*q, *et_us),
            _ => unreachable!(),
        };
        let ctu_state = vm.get_global("batchCnt");
        assert!(
            matches!(ctu_state, Some(Value::Fb(FbState::Ctu { .. }))),
            "expected CTU FB state for `batchCnt`, got {ctu_state:?}"
        );
        let (ctu_q, ctu_cv) = match ctu_state {
            Some(Value::Fb(FbState::Ctu { q, cv, .. })) => (*q, *cv),
            _ => unreachable!(),
        };

        trace.push(TimerCounterSample {
            timer_done_out: read_digital_out(&io, timer_out_pin),
            batch_ready_out: read_digital_out(&io, batch_out_pin),
            ton_q,
            ton_et_us,
            ctu_q,
            ctu_cv,
        });
    }
    trace
}

#[test]
fn scenario_2_timer_ton_counter_ctu_and_comparison_equivalence() {
    let catalog = load_live_catalog();
    let cpp_fe = CppFrontend::new();
    let st_fe = StFrontend::new();
    let lad_fe = LadderFrontend::new();

    let uno_ctx = BoardContext::arduino_uno_r3("uno_ton_ctu");
    let plc_ctx = BoardContext::plc_1200_open_1214("plc_ton_ctu");

    // 2a. Arduino C++: TON (30ms = 30000us) -> CTU (PV=2) -> Comparison (CV >= 2)
    let cpp_src = r#"#include <Arduino.h>

#define EnableIn 2
#define ResetIn 3
#define TimerDoneOut 12
#define BatchReadyOut 13

TON tonDelay;
CTU batchCnt;

void setup() {
  pinMode(EnableIn, INPUT);
  pinMode(ResetIn, INPUT);
  pinMode(TimerDoneOut, OUTPUT);
  pinMode(BatchReadyOut, OUTPUT);
}

void loop() {
  tonDelay({.IN = digitalRead(EnableIn), .PT = 30000});
  digitalWrite(TimerDoneOut, tonDelay.Q);
  batchCnt({.CU = tonDelay.Q, .R = digitalRead(ResetIn), .PV = 2});
  bool reached = batchCnt.Q && (batchCnt.CV >= 2);
  digitalWrite(BatchReadyOut, reached);
}
"#;
    let cpp_parsed = cpp_fe.parse(cpp_src, &uno_ctx).expect("parse C++ TON+CTU");
    verify_frontend_roundtrip(&cpp_fe, &cpp_parsed.value, &uno_ctx)
        .expect("C++ TON+CTU LOSSLESS_ASL");
    let cpp_board = make_uno_board("uno_ton_ctu", &cpp_parsed.value);
    assert_valid_against_live_catalog(&cpp_board, &catalog);

    // 2b. IEC 61131-3 ST: TON (T#30ms) -> CTU (PV := 2) -> Comparison (CV >= 2)
    let st_src = r#"VAR_GLOBAL
    EnableIn AT %IX0.0 : BOOL;
    ResetIn AT %IX0.1 : BOOL;
    TimerDoneOut AT %QX0.0 : BOOL;
    BatchReadyOut AT %QX0.1 : BOOL;
    tonDelay : TON;
    batchCnt : CTU;
END_VAR

PROGRAM OB_MAIN
    tonDelay(IN := EnableIn, PT := T#30ms);
    TimerDoneOut := tonDelay.Q;
    batchCnt(CU := tonDelay.Q, R := ResetIn, PV := 2);
    BatchReadyOut := batchCnt.Q AND (batchCnt.CV >= 2);
END_PROGRAM
"#;
    let st_parsed = st_fe.parse(st_src, &plc_ctx).expect("parse ST TON+CTU");
    verify_frontend_roundtrip(&st_fe, &st_parsed.value, &plc_ctx).expect("ST TON+CTU LOSSLESS_ASL");
    let st_board = make_plc_board("plc_st_ton_ctu", AuthoringLanguage::St, &st_parsed.value);
    assert_valid_against_live_catalog(&st_board, &catalog);

    // 2c. IEC 61131-3 Ladder (`PouBody::Network`): TON box -> CTU box -> CMP GE box -> Coils
    let ladder_src = r#"VAR_GLOBAL
    EnableIn AT %IX0.0 : BOOL;
    ResetIn AT %IX0.1 : BOOL;
    TimerDoneOut AT %QX0.0 : BOOL;
    BatchReadyOut AT %QX0.1 : BOOL;
    tonDelay : TON;
    batchCnt : CTU;
END_VAR

NETWORK OB_MAIN
    RUNG 0 (n_r0): | | EnableIn @(0, 0, n_en) -- FB TON tonDelay(PT := T#30ms) @(0, 1, n_ton) -- ( ) TimerDoneOut @(0, 2, n_tdone);
    RUNG 1 (n_r1): | | tonDelay.Q @(0, 0, n_tq) -- FB CTU batchCnt(R := ResetIn, PV := 2) @(0, 1, n_ctu) -- CMP GE(batchCnt.CV, 2) @(0, 2, n_cmp) -- ( ) BatchReadyOut @(0, 3, n_batch);
END_NETWORK
"#;
    let lad_parsed = lad_fe
        .parse_and_validate(ladder_src, &plc_ctx)
        .expect("parse & validate Ladder TON+CTU");
    verify_frontend_roundtrip(&lad_fe, &lad_parsed.value, &plc_ctx)
        .expect("Ladder TON+CTU LOSSLESS_ASL");
    let lad_board = make_plc_board(
        "plc_lad_ton_ctu",
        AuthoringLanguage::Ladder,
        &lad_parsed.value,
    );
    assert_valid_against_live_catalog(&lad_board, &catalog);

    // 15 stepped scans @ 10 ms each:
    // - Scans 0..=4: Enable=true -> ET ramps 10ms, 20ms, 30ms (Q=true, CV=1), holds 30ms
    // - Scans 5..=6: Enable=false -> ET resets to 0, Q=false, CV remains 1
    // - Scans 7..=11: Enable=true -> second TON fires at scan 9 -> CV=2, ctu.Q=true, BatchReadyOut=true
    // - Scan 12: Reset=true, Enable=false -> CV=0, ctu.Q=false, BatchReadyOut=false
    // - Scans 13..=14: Idle
    let timeline = vec![
        (true, false),
        (true, false),
        (true, false),
        (true, false),
        (true, false),
        (false, false),
        (false, false),
        (true, false),
        (true, false),
        (true, false),
        (true, false),
        (true, false),
        (false, true),
        (false, false),
        (false, false),
    ];

    let cpp_trace =
        run_timer_counter_timeline(&cpp_board, &uno_ctx, "D2", "D3", "D12", "D13", &timeline);
    let st_trace = run_timer_counter_timeline(
        &st_board, &plc_ctx, "Ia.0", "Ia.1", "Qa.0", "Qa.1", &timeline,
    );
    let lad_trace = run_timer_counter_timeline(
        &lad_board, &plc_ctx, "Ia.0", "Ia.1", "Qa.0", "Qa.1", &timeline,
    );

    // Verify exact milestone values before asserting 3-way trace equality
    assert_eq!(st_trace[0].ton_et_us, 10_000);
    assert!(!st_trace[0].ton_q);
    assert_eq!(st_trace[1].ton_et_us, 20_000);
    assert!(!st_trace[1].ton_q);
    assert_eq!(st_trace[2].ton_et_us, 30_000);
    assert!(st_trace[2].ton_q);
    assert_eq!(st_trace[2].ctu_cv, 1);
    assert!(!st_trace[2].batch_ready_out);
    assert_eq!(st_trace[5].ton_et_us, 0);
    assert_eq!(st_trace[5].ctu_cv, 1);
    assert_eq!(st_trace[9].ton_et_us, 30_000);
    assert!(st_trace[9].ton_q);
    assert_eq!(st_trace[9].ctu_cv, 2);
    assert!(st_trace[9].ctu_q);
    assert!(st_trace[9].batch_ready_out);
    assert_eq!(st_trace[12].ctu_cv, 0);
    assert!(!st_trace[12].ctu_q);
    assert!(!st_trace[12].batch_ready_out);

    assert_eq!(
        cpp_trace, st_trace,
        "C++ and ST TON+CTU+Compare traces MUST be bit-identical"
    );
    assert_eq!(
        st_trace, lad_trace,
        "ST and Ladder TON+CTU+Compare traces MUST be bit-identical"
    );
}

// ---------------------------------------------------------------------------
// Scenario 3: Structural GRAFCET/SFC (PouBody::Chart) <-> Derived ST Equivalence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct SfcScanSample {
    active_steps: BTreeSet<String>,
    coil_n: bool,
    coil_latch: bool,
    coil_limited: bool,
    coil_delayed: bool,
    pulse_hits: i32,
    out_qa0: bool,
    out_qa1: bool,
}

#[test]
fn scenario_3_structural_sfc_chart_and_derived_st_equivalence() {
    let catalog = load_live_catalog();
    let sfc_fe = SfcFrontend::new();
    let st_fe = StFrontend::new();
    let plc_ctx = BoardContext::plc_1200_open_1214("plc_sfc");

    // Multi-step GRAFCET/SFC chart exercising:
    // - Deterministic OR priority divergence (Idle -> T_prio1 PRIORITY 1 vs T_prio2 PRIORITY 2)
    // - Action qualifiers N (auto-reset), L (T#20ms limited), D (T#20ms delayed) on SyncStart
    // - Simultaneous AND divergence (SyncStart -> (BranchA, BranchB)) & asymmetric AND convergence
    // - Action qualifiers S (latch on BranchA), P (1-scan pulse counter on BranchB)
    // - IEC 60848 Rule 5 two-phase simultaneous deactivate+activate (R5_Left->R5_Mid & R5_Mid->R5_Right)
    // - Action qualifier R (overriding reset of S-latched coil on R5_Right)
    let sfc_src = r#"VAR_GLOBAL
    go_fast AT %IX0.0 : BOOL;
    go_slow AT %IX0.1 : BOOL;
    fork_now AT %IX0.2 : BOOL;
    done_a AT %IX0.3 : BOOL;
    done_b AT %IX0.4 : BOOL;
    shift_r5 AT %IX0.5 : BOOL;
    reset_all AT %IX0.6 : BOOL;
    ValveOut AT %QX0.0 : BOOL;
    AlarmOut AT %QX0.1 : BOOL;
    coil_n : BOOL := FALSE;
    coil_latch : BOOL := FALSE;
    coil_limited : BOOL := FALSE;
    coil_delayed : BOOL := FALSE;
    pulse_flag : BOOL := FALSE;
    pulse_hits : DINT := 0;
END_VAR

SFC_CHART OB_MAIN
    INITIAL_STEP Idle:
    END_STEP

    STEP SyncStart:
        N: coil_n;
        N: ValveOut;
        L(T#20ms): coil_limited;
        D(T#20ms): coil_delayed;
    END_STEP

    STEP SlowAlt:
        N: AlarmOut;
    END_STEP

    STEP BranchA:
        S: coil_latch;
        S: AlarmOut;
    END_STEP

    STEP BranchB:
        P: pulse_flag { pulse_hits := pulse_hits + 1; }
    END_STEP

    STEP WaitA:
    END_STEP

    STEP WaitB:
    END_STEP

    STEP R5_Left:
    END_STEP

    STEP R5_Mid:
        N: ValveOut;
    END_STEP

    STEP R5_Right:
        R: coil_latch;
        R: AlarmOut;
    END_STEP

    TRANSITION T_prio2 PRIORITY 2 FROM Idle TO SlowAlt := go_slow;
    END_TRANSITION

    TRANSITION T_prio1 PRIORITY 1 FROM Idle TO SyncStart := go_fast;
    END_TRANSITION

    TRANSITION T_fork FROM SyncStart TO (BranchA, BranchB) := fork_now;
    END_TRANSITION

    TRANSITION T_a_done FROM BranchA TO WaitA := done_a;
    END_TRANSITION

    TRANSITION T_b_done FROM BranchB TO WaitB := done_b;
    END_TRANSITION

    TRANSITION T_join FROM (WaitA, WaitB) TO (R5_Left, R5_Mid) := TRUE;
    END_TRANSITION

    TRANSITION T_r5_shift1 FROM R5_Left TO R5_Mid := shift_r5;
    END_TRANSITION

    TRANSITION T_r5_shift2 FROM R5_Mid TO R5_Right := shift_r5;
    END_TRANSITION

    TRANSITION T_restart FROM (R5_Mid, R5_Right) TO Idle := reset_all;
    END_TRANSITION
END_SFC_CHART
"#;

    let sfc_parsed = sfc_fe.parse(sfc_src, &plc_ctx).expect("parse SFC chart");
    assert!(
        !format!("{:#?}", sfc_parsed.value.program).contains("__sfc_"),
        "ASL PouBody::Chart MUST NOT persist synthetic __sfc_* variables"
    );
    verify_frontend_roundtrip(&sfc_fe, &sfc_parsed.value, &plc_ctx)
        .expect("SFC LOSSLESS_ASL roundtrip");

    let native_board = make_plc_board("plc_sfc_native", AuthoringLanguage::Sfc, &sfc_parsed.value);
    assert_valid_against_live_catalog(&native_board, &catalog);

    // Verify direct StFrontend::emit on PouBody::Chart produces clean parseable ST preview
    let direct_st_preview = st_fe
        .emit(&sfc_parsed.value, &plc_ctx)
        .expect("direct ST preview from PouBody::Chart")
        .value;
    assert!(!contains_banned_magic_comment(&direct_st_preview));
    assert!(
        st_fe.parse(&direct_st_preview, &plc_ctx).is_ok(),
        "direct ST preview from PouBody::Chart must parse cleanly"
    );

    // Build full Rule-5 derived ST program via `project_chart_state_decls` + `project_chart_to_statements`
    // -> `StFrontend::emit` -> `StFrontend::parse` -> `dendri-validate`
    let chart = match &sfc_parsed.value.program.pous[0].body {
        PouBody::Chart(c) => c.clone(),
        PouBody::Statements(_) | PouBody::Network(_) => panic!("expected PouBody::Chart"),
    };
    let step_names: Vec<String> = chart.steps.iter().map(|s| s.name.clone()).collect();

    let mut projected_bundle = sfc_parsed.value.clone();
    projected_bundle
        .program
        .data
        .extend(project_chart_state_decls(&chart));
    projected_bundle.program.pous[0].body =
        PouBody::Statements(project_chart_to_statements(&chart));
    projected_bundle.program.pous[0].preferred_view = Some(AuthoringLanguage::St);

    let derived_st_src = st_fe
        .emit(&projected_bundle, &plc_ctx)
        .expect("StFrontend::emit on projected SFC")
        .value;
    assert!(!contains_banned_magic_comment(&derived_st_src));
    assert!(!contains_analog_full_scale_literal(&derived_st_src));

    let derived_st_parsed = st_fe
        .parse(&derived_st_src, &plc_ctx)
        .expect("StFrontend::parse on derived SFC ST");
    let st_board = make_plc_board(
        "plc_sfc_derived_st",
        AuthoringLanguage::St,
        &derived_st_parsed.value,
    );
    assert_valid_against_live_catalog(&st_board, &catalog);

    // Stimulus schedule: (go_fast, go_slow, fork_now, done_a, done_b, shift_r5, reset_all)
    let schedule = vec![
        // Scan 0: Initial boot in Idle
        (false, false, false, false, false, false, false),
        // Scan 1: BOTH go_fast & go_slow true -> OR priority MUST pick T_prio1 (SyncStart, NOT SlowAlt!)
        (true, true, false, false, false, false, false),
        // Scan 2: Stay in SyncStart (t = 10ms -> L=true, D=false, N=true)
        (false, false, false, false, false, false, false),
        // Scan 3: Stay in SyncStart (t = 20ms -> L=false, D=true, N=true)
        (false, false, false, false, false, false, false),
        // Scan 4: AND divergence SyncStart -> (BranchA, BranchB) (N auto-resets, S latches, P fires once)
        (false, false, true, false, false, false, false),
        // Scan 5: BranchB remains active -> P MUST NOT re-fire! BranchA completes -> WaitA
        (false, false, false, true, false, false, false),
        // Scan 6: WaitA waits at AND convergence barrier until BranchB finishes -> done_b=true -> WaitB
        (false, false, false, false, true, false, false),
        // Scan 7: AND convergence (WaitA, WaitB) -> (R5_Left, R5_Mid) fires!
        (false, false, false, false, false, false, false),
        // Scan 8: Rule 5 two-phase simultaneous deactivate+activate:
        // R5_Left -> R5_Mid AND R5_Mid -> R5_Right fire simultaneously ->
        // R5_Mid MUST REMAIN ACTIVE alongside R5_Right, and R5_Right resets coil_latch & AlarmOut!
        (false, false, false, false, false, true, false),
        // Scan 9: Verify R5_Mid & R5_Right stable, coil_latch stays FALSE
        (false, false, false, false, false, false, false),
        // Scan 10: Reset back to Idle
        (false, false, false, false, false, false, true),
    ];

    let mut native_vm =
        BoardVm::new(&native_board, vm_config_with_codecs(&plc_ctx)).expect("compile native SFC");
    let mut st_vm =
        BoardVm::new(&st_board, vm_config_with_codecs(&plc_ctx)).expect("compile derived ST SFC");
    let mut native_io = MemoryBoardIo::default();
    let mut st_io = MemoryBoardIo::default();

    let mut native_trace = Vec::with_capacity(schedule.len());
    let mut st_trace = Vec::with_capacity(schedule.len());

    let read_bool_global = |vm: &BoardVm, name: &str| -> bool {
        match vm.get_global(name) {
            Some(Value::Bool(b)) => *b,
            other => panic!("expected Bool global `{name}`, got {other:?}"),
        }
    };
    let read_dint_global = |vm: &BoardVm, name: &str| -> i32 {
        match vm.get_global(name) {
            Some(Value::Dint(n)) => *n,
            other => panic!("expected Dint global `{name}`, got {other:?}"),
        }
    };

    for (scan_idx, &(gf, gs, fk, da, db, sr, rst)) in schedule.iter().enumerate() {
        for io in [&mut native_io, &mut st_io] {
            io.set_digital_in("Ia.0", gf);
            io.set_digital_in("Ia.1", gs);
            io.set_digital_in("Ia.2", fk);
            io.set_digital_in("Ia.3", da);
            io.set_digital_in("Ia.4", db);
            io.set_digital_in("Ia.5", sr);
            io.set_digital_in("Ia.6", rst);
        }

        native_vm
            .run_plc_scan(10_000, &mut native_io)
            .expect("native SFC scan");
        st_vm
            .run_plc_scan(10_000, &mut st_io)
            .expect("derived ST SFC scan");

        let native_active = native_vm
            .active_sfc_steps("OB_MAIN")
            .cloned()
            .expect("OB_MAIN chart active steps");
        let mut st_active = BTreeSet::new();
        for sname in &step_names {
            let var_name = format!("sfc_step_{sname}_x");
            if read_bool_global(&st_vm, &var_name) {
                st_active.insert(sname.clone());
            }
        }

        let native_sample = SfcScanSample {
            active_steps: native_active,
            coil_n: read_bool_global(&native_vm, "coil_n"),
            coil_latch: read_bool_global(&native_vm, "coil_latch"),
            coil_limited: read_bool_global(&native_vm, "coil_limited"),
            coil_delayed: read_bool_global(&native_vm, "coil_delayed"),
            pulse_hits: read_dint_global(&native_vm, "pulse_hits"),
            out_qa0: read_digital_out(&native_io, "Qa.0"),
            out_qa1: read_digital_out(&native_io, "Qa.1"),
        };
        let st_sample = SfcScanSample {
            active_steps: st_active,
            coil_n: read_bool_global(&st_vm, "coil_n"),
            coil_latch: read_bool_global(&st_vm, "coil_latch"),
            coil_limited: read_bool_global(&st_vm, "coil_limited"),
            coil_delayed: read_bool_global(&st_vm, "coil_delayed"),
            pulse_hits: read_dint_global(&st_vm, "pulse_hits"),
            out_qa0: read_digital_out(&st_io, "Qa.0"),
            out_qa1: read_digital_out(&st_io, "Qa.1"),
        };

        assert_eq!(
            native_sample, st_sample,
            "SFC native vs derived ST diverged at scan {scan_idx}"
        );
        native_trace.push(native_sample);
        st_trace.push(st_sample);
    }

    // Explicit semantic checkpoints confirming every SFC feature genuinely fired:
    // Scan 1: OR priority picked SyncStart (NOT SlowAlt)
    assert!(native_trace[1].active_steps.contains("SyncStart"));
    assert!(!native_trace[1].active_steps.contains("SlowAlt"));
    assert!(native_trace[1].coil_n);
    assert!(native_trace[1].coil_limited);
    assert!(!native_trace[1].coil_delayed);

    // Scan 3: 20ms elapsed in SyncStart -> L timed out, D activated
    assert!(!native_trace[3].coil_limited);
    assert!(native_trace[3].coil_delayed);

    // Scan 4: AND divergence -> BranchA & BranchB active; N auto-reset; S latched; P pulsed once
    assert_eq!(
        native_trace[4].active_steps,
        BTreeSet::from(["BranchA".to_owned(), "BranchB".to_owned()])
    );
    assert!(!native_trace[4].coil_n);
    assert!(native_trace[4].coil_latch);
    assert_eq!(native_trace[4].pulse_hits, 1);

    // Scan 5: BranchB still active -> P must stay 1; WaitA waits for BranchB
    assert_eq!(
        native_trace[5].active_steps,
        BTreeSet::from(["WaitA".to_owned(), "BranchB".to_owned()])
    );
    assert_eq!(native_trace[5].pulse_hits, 1);
    assert!(native_trace[5].coil_latch);

    // Scan 7: AND convergence -> (R5_Left, R5_Mid)
    assert_eq!(
        native_trace[7].active_steps,
        BTreeSet::from(["R5_Left".to_owned(), "R5_Mid".to_owned()])
    );

    // Scan 8: IEC 60848 Rule 5 -> R5_Mid deactivated & reactivated simultaneously -> REMAINS ACTIVE!
    // R5_Right activates and resets S-latched coil_latch & AlarmOut!
    assert_eq!(
        native_trace[8].active_steps,
        BTreeSet::from(["R5_Mid".to_owned(), "R5_Right".to_owned()])
    );
    assert!(!native_trace[8].coil_latch);
    assert!(!native_trace[8].out_qa1);
    assert!(native_trace[8].out_qa0);

    // Scan 10: Back to Idle, N coil on R5_Mid auto-reset
    assert_eq!(
        native_trace[10].active_steps,
        BTreeSet::from(["Idle".to_owned()])
    );
    assert!(!native_trace[10].out_qa0);
    assert_eq!(native_trace, st_trace);
}

// ---------------------------------------------------------------------------
// Scenario 4: Numeric Contract T-NUM-* & Reduced Rational Scaling (Uno 1024 <-> PLC 27648)
// ---------------------------------------------------------------------------

#[test]
fn scenario_4_numeric_contract_t_num_and_reduced_rational_scaling() {
    let catalog = load_live_catalog();
    let st_fe = StFrontend::new();
    let cpp_fe = CppFrontend::new();

    let uno_ctx = BoardContext::arduino_uno_r3("uno_adc");
    let plc_ctx = BoardContext::plc_1200_open_1214("plc_adc");

    // Fetch live codecs directly from certified board profiles in AssetCatalog
    let uno_profile = catalog
        .boards
        .get("arduino-uno-r3")
        .expect("arduino-uno-r3 in live catalog");
    let plc_profile = catalog
        .boards
        .get("plc-1200-open-1214")
        .expect("plc-1200-open-1214 in live catalog");

    let uno_spec = uno_profile
        .codecs
        .iter()
        .find(|c| c.id == "uno_10bit")
        .expect("uno_10bit codec");
    let plc_unipolar_spec = plc_profile
        .codecs
        .iter()
        .find(|c| c.id == "plc_span_27648")
        .expect("plc_span_27648 codec");
    let plc_bipolar_spec = plc_profile
        .codecs
        .iter()
        .find(|c| c.id == "plc_bipolar_27648")
        .expect("plc_bipolar_27648 codec");

    let uno_10bit = Codec::new(&uno_spec.id, uno_spec.kind, uno_spec.d, uno_spec.n_max)
        .expect("valid uno_10bit");
    let plc_span = Codec::new(
        &plc_unipolar_spec.id,
        plc_unipolar_spec.kind,
        plc_unipolar_spec.d,
        plc_unipolar_spec.n_max,
    )
    .expect("valid plc_span_27648");
    let plc_bipolar = Codec::new(
        &plc_bipolar_spec.id,
        plc_bipolar_spec.kind,
        plc_bipolar_spec.d,
        plc_bipolar_spec.n_max,
    )
    .expect("valid plc_bipolar_27648");

    assert_eq!(uno_10bit.kind, CodecKind::Binned);
    assert_eq!(uno_10bit.d, 1024);
    assert_eq!(plc_span.kind, CodecKind::Span);
    assert_eq!(plc_span.d, 27_648);
    assert_eq!(plc_bipolar.kind, CodecKind::SpanBipolar);
    assert_eq!(plc_bipolar.d, 27_648);

    // 4a. Exhaustive T-NUM-2 composition & ReducedRationalPlan parity (1024 <-> 27648)
    let plan_uno_to_plc =
        ReducedRationalPlan::compute(&uno_10bit, &plc_span).expect("plan uno->plc");
    assert_eq!(plan_uno_to_plc.gcd, 1024);
    assert_eq!(plan_uno_to_plc.d1_red, 1);
    assert_eq!(plan_uno_to_plc.d2_red, 27);
    assert!(!plan_uno_to_plc.needs_clamp);
    assert!(plan_uno_to_plc.lcm_satisfies_lemma2);
    for n in 0..=uno_10bit.n_max {
        let via_asl = plc_span.to_code_unipolar(uno_10bit.to_analog_unipolar(n).expect("in range"));
        let via_lemma2 = composed_clamped_rational(n, uno_10bit.d, plc_span.d, plc_span.n_max);
        let via_plan = plan_uno_to_plc.eval_unipolar(n);
        assert_eq!(via_asl, via_lemma2, "T-NUM-2 mismatch uno->plc at n={n}");
        assert_eq!(via_plan, via_asl, "ReducedRationalPlan mismatch at n={n}");
    }

    let plan_plc_to_uno =
        ReducedRationalPlan::compute(&plc_span, &uno_10bit).expect("plan plc->uno");
    assert_eq!(plan_plc_to_uno.gcd, 1024);
    assert_eq!(plan_plc_to_uno.d1_red, 27);
    assert_eq!(plan_plc_to_uno.d2_red, 1);
    assert!(
        plan_plc_to_uno.needs_clamp,
        "span -> binned MUST require mandatory upper clamp"
    );
    assert_eq!(
        plan_plc_to_uno.eval_unipolar(27_648),
        1023,
        "top span code 27648 MUST clamp to binned n_max = 1023"
    );
    for n in 0..=plc_span.n_max {
        let via_asl = uno_10bit.to_code_unipolar(plc_span.to_analog_unipolar(n).expect("in range"));
        let via_lemma2 = composed_clamped_rational(n, plc_span.d, uno_10bit.d, uno_10bit.n_max);
        let via_plan = plan_plc_to_uno.eval_unipolar(n);
        assert_eq!(via_asl, via_lemma2, "T-NUM-2 mismatch plc->uno at n={n}");
        assert_eq!(via_plan, via_asl, "ReducedRationalPlan mismatch at n={n}");
    }

    // 4b. T-NUM-5 exact worked codes across unipolar and bipolar channels
    assert_eq!(plc_span.to_code_unipolar(200_000_000), 5_529);
    assert_eq!(uno_10bit.to_code_unipolar(200_000_000), 204);
    assert_eq!(plc_bipolar.to_code_signed(200_000_000), 5_529);
    assert_eq!(plc_bipolar.to_code_signed(-200_000_000), -5_529);
    assert_eq!(
        plc_bipolar.to_code_signed(i64::try_from(ASL_ANALOG_FULL_SCALE).expect("M fits i64")),
        27_648
    );
    assert_eq!(
        plc_bipolar.to_code_signed(-i64::try_from(ASL_ANALOG_FULL_SCALE).expect("M fits i64")),
        -27_648
    );

    // Bipolar -> Unipolar clamping for negative codes
    let plan_bipolar_to_uno =
        ReducedRationalPlan::compute(&plc_bipolar, &uno_10bit).expect("plan bipolar->uno");
    assert!(plan_bipolar_to_uno.needs_clamp);
    assert!(plan_bipolar_to_uno.is_signed);

    // Validate live PLC channel bands (IW64 unipolar & IW66 bipolar)
    for ch in &plc_profile.channels {
        if let Some(band) = &ch.band {
            let codec = if ch.codec_id == "plc_span_27648" {
                &plc_span
            } else {
                &plc_bipolar
            };
            let ast_band = dendri_ast::ChannelBand {
                underrange_min: band.underrange_min,
                overrange_max: band.overrange_max,
                overflow_code: band.overflow_code,
                wire_break_code: band.wire_break_code,
                source_ref: band.source_ref.clone(),
            };
            ast_band
                .validate_against(codec)
                .expect("live PLC channel band must validate within signed i32 ANALOG");
        }
    }

    // 4c. StFrontend & CppFrontend reduced-rational emitters NEVER emit M = 10^9
    let st_uno_to_plc =
        emit_scaled_code_expr("raw_uno", &uno_10bit, &plc_span).expect("ST uno->plc expr");
    let st_plc_to_uno =
        emit_scaled_code_expr("raw_plc", &plc_span, &uno_10bit).expect("ST plc->uno expr");
    let cpp_uno_to_plc =
        emit_codec_conversion_cpp(&uno_10bit, &plc_span, "raw_uno").expect("C++ uno->plc expr");
    let cpp_plc_to_uno =
        emit_codec_conversion_cpp(&plc_span, &uno_10bit, "raw_plc").expect("C++ plc->uno expr");
    let st_bip_to_uno =
        emit_scaled_code_expr("raw_bip", &plc_bipolar, &uno_10bit).expect("ST bip->uno expr");
    let cpp_bip_to_uno =
        emit_codec_conversion_cpp(&plc_bipolar, &uno_10bit, "raw_bip").expect("C++ bip->uno expr");

    for expr_text in [
        &st_uno_to_plc,
        &st_plc_to_uno,
        &cpp_uno_to_plc,
        &cpp_plc_to_uno,
        &st_bip_to_uno,
        &cpp_bip_to_uno,
    ] {
        assert!(
            !contains_analog_full_scale_literal(expr_text),
            "reduced-rational emitter MUST NEVER emit M = 10^9: `{expr_text}`"
        );
    }
    assert!(st_uno_to_plc.contains("27"));
    assert!(st_plc_to_uno.contains("MIN(") && st_plc_to_uno.contains("1023"));
    assert!(cpp_plc_to_uno.contains("1023"));
    assert!(cpp_uno_to_plc.contains("27"));

    // 4d. Full ST & C++ programs with cross-platform codecs: LOSSLESS_ASL + zero M + live VM execution
    let mut cross_plc_ctx = plc_ctx.clone();
    cross_plc_ctx
        .codecs
        .insert(uno_10bit.id.clone(), uno_10bit.clone());

    let st_scale_src = r#"VAR_GLOBAL
    PressRaw AT %IW64 : INT;
    BipRaw AT %IW66 : INT;
    scaledToUno : CODE<uno_10bit>;
    plcEcho : CODE<plc_span_27648>;
    bipEcho : CODE<plc_bipolar_27648>;
END_VAR

PROGRAM OB_MAIN
    plcEcho := PressRaw;
    scaledToUno := ANALOG_TO_CODE(CODE_TO_ANALOG(PressRaw, 'plc_span_27648'), 'uno_10bit');
    bipEcho := ANALOG_TO_CODE(CODE_TO_ANALOG(BipRaw, 'plc_bipolar_27648'), 'plc_bipolar_27648');
END_PROGRAM
"#;
    let st_scale_parsed = st_fe
        .parse(st_scale_src, &cross_plc_ctx)
        .expect("parse ST cross-codec program");
    verify_frontend_roundtrip(&st_fe, &st_scale_parsed.value, &cross_plc_ctx)
        .expect("ST cross-codec LOSSLESS_ASL");

    let st_reduced_emit = st_fe
        .emit_with_reduced_rationals(&st_scale_parsed.value, &cross_plc_ctx)
        .expect("ST reduced-rational emit")
        .value;
    assert!(
        !contains_analog_full_scale_literal(&st_reduced_emit),
        "ST reduced-rational output MUST NEVER emit M = 10^9:\n{st_reduced_emit}"
    );
    assert!(
        st_reduced_emit.contains("MIN(") && st_reduced_emit.contains("1023"),
        "ST reduced-rational output must include mandatory span->binned clamp:\n{st_reduced_emit}"
    );

    // Execute ST cross-codec board in dendri-vm to verify live T-NUM-5 + span->binned clamp
    let st_scale_board = make_plc_board(
        "plc_scale_vm",
        AuthoringLanguage::St,
        &st_scale_parsed.value,
    );
    let mut vm = BoardVm::new(&st_scale_board, vm_config_with_codecs(&cross_plc_ctx))
        .expect("compile ST scale VM");
    let mut io = MemoryBoardIo::default();

    // Test 2V unipolar (200_000_000 ppb -> 5529 PLC -> 204 Uno) & -2V bipolar (-200_000_000 ppb -> -5529)
    io.set_analog_in("IW64", 200_000_000, dendri_ast::AnalogStatus::Ok);
    io.set_analog_in("IW66", -200_000_000, dendri_ast::AnalogStatus::Ok);
    vm.run_plc_scan(10_000, &mut io).expect("run 2V scan");
    assert_eq!(
        vm.get_global("plcEcho"),
        Some(&Value::Code {
            value: 5_529,
            codec_id: "plc_span_27648".to_owned(),
        })
    );
    assert_eq!(
        vm.get_global("scaledToUno"),
        Some(&Value::Code {
            value: 204,
            codec_id: "uno_10bit".to_owned(),
        })
    );
    assert_eq!(
        vm.get_global("bipEcho"),
        Some(&Value::Code {
            value: -5_529,
            codec_id: "plc_bipolar_27648".to_owned(),
        })
    );

    // Test full-scale 10V (1_000_000_000 ppb -> 27648 PLC -> clamped 1023 Uno) & -10V bipolar (-27648)
    io.set_analog_in("IW64", 1_000_000_000, dendri_ast::AnalogStatus::Ok);
    io.set_analog_in("IW66", -1_000_000_000, dendri_ast::AnalogStatus::Ok);
    vm.run_plc_scan(10_000, &mut io)
        .expect("run full-scale scan");
    assert_eq!(
        vm.get_global("plcEcho"),
        Some(&Value::Code {
            value: 27_648,
            codec_id: "plc_span_27648".to_owned(),
        })
    );
    assert_eq!(
        vm.get_global("scaledToUno"),
        Some(&Value::Code {
            value: 1_023,
            codec_id: "uno_10bit".to_owned(),
        })
    );
    assert_eq!(
        vm.get_global("bipEcho"),
        Some(&Value::Code {
            value: -27_648,
            codec_id: "plc_bipolar_27648".to_owned(),
        })
    );

    // C++ Arduino analog program: LOSSLESS_ASL + zero M + live VM execution on arduino-uno-r3
    // (a) Strict ASL-typed analogWrite(PWM_PIN, analogRead(POT_PIN)) validates cleanly against live AssetCatalog
    let cpp_strict_pwm_src = r#"#include <Arduino.h>
#include <stdint.h>

#define POT_PIN A0
#define PWM_PIN 5

void setup() {
  pinMode(PWM_PIN, OUTPUT);
}

void loop() {
  analogWrite(PWM_PIN, analogRead(POT_PIN));
}
"#;
    let cpp_strict_parsed = cpp_fe
        .parse(cpp_strict_pwm_src, &uno_ctx)
        .expect("parse strict C++ analogWrite/analogRead sketch");
    let cpp_strict_emitted = cpp_fe
        .emit(&cpp_strict_parsed.value, &uno_ctx)
        .expect("emit strict C++ analog sketch")
        .value;
    assert!(!contains_analog_full_scale_literal(&cpp_strict_emitted));
    assert!(!contains_banned_magic_comment(&cpp_strict_emitted));
    verify_frontend_roundtrip(&cpp_fe, &cpp_strict_parsed.value, &uno_ctx)
        .expect("strict C++ analog sketch LOSSLESS_ASL");
    let cpp_strict_board = make_uno_board("uno_strict_pwm", &cpp_strict_parsed.value);
    assert_valid_against_live_catalog(&cpp_strict_board, &catalog);

    // (b) Idiomatic Arduino `rawAdc = analogRead(POT_PIN)` executes exact T-NUM-5 code (204 at 20%) in dendri-vm
    let cpp_adc_src = r#"#include <Arduino.h>
#include <stdint.h>

#define POT_PIN A0
#define PWM_PIN 5

int16_t rawAdc = 0;

void setup() {
  pinMode(PWM_PIN, OUTPUT);
}

void loop() {
  rawAdc = analogRead(POT_PIN);
  analogWrite(PWM_PIN, rawAdc >> 2);
}
"#;
    let cpp_adc_parsed = cpp_fe
        .parse(cpp_adc_src, &uno_ctx)
        .expect("parse C++ analog sketch");
    let cpp_adc_emitted = cpp_fe
        .emit(&cpp_adc_parsed.value, &uno_ctx)
        .expect("emit C++ analog sketch")
        .value;
    assert!(!contains_analog_full_scale_literal(&cpp_adc_emitted));
    assert!(!contains_banned_magic_comment(&cpp_adc_emitted));
    verify_frontend_roundtrip(&cpp_fe, &cpp_adc_parsed.value, &uno_ctx)
        .expect("C++ analog sketch LOSSLESS_ASL");
    let cpp_adc_board = make_uno_board("uno_adc_vm", &cpp_adc_parsed.value);

    let mut uno_vm =
        BoardVm::new(&cpp_adc_board, vm_config_with_codecs(&uno_ctx)).expect("compile Uno VM");
    let mut uno_io = MemoryBoardIo::default();
    uno_io.set_analog_in("A0", 200_000_000, dendri_ast::AnalogStatus::Ok);
    let _ = uno_vm
        .step_cooperative(10_000, &mut uno_io)
        .expect("step Uno ADC");
    assert_eq!(uno_vm.get_global("rawAdc"), Some(&Value::Int(204)));
    // Full scale (10^9 ppb) clamps to 1023 on binned 10-bit Uno ADC (T-NUM-3)
    uno_io.set_analog_in("A0", 1_000_000_000, dendri_ast::AnalogStatus::Ok);
    let _ = uno_vm
        .step_cooperative(10_000, &mut uno_io)
        .expect("step Uno full-scale ADC");
    assert_eq!(uno_vm.get_global("rawAdc"), Some(&Value::Int(1_023)));
}

// ---------------------------------------------------------------------------
// Scenario 5: Live dendri-sync Multi-View Workflow + C1-C5 + Foreign + Assumptions + Undo/Redo
// ---------------------------------------------------------------------------

#[test]
fn scenario_5_live_sync_engine_multi_view_comments_foreign_assumptions_undo_redo() {
    let catalog = load_live_catalog();
    let engine = SyncEngine::new();
    let st_fe = StFrontend::new();
    let cpp_fe = CppFrontend::new();
    let lad_fe = LadderFrontend::new();
    let sfc_fe = SfcFrontend::new();

    let plc_ctx = BoardContext::plc_1200_open_1214("plc_sync");
    let uno_ctx = BoardContext::arduino_uno_r3("uno_sync");

    // 5a. Verify full C3 hostile comment corpus across ALL 4 real Wave 1 frontends
    //     and ALL 4 DEF-006 ST dialects!
    assert_eq!(verify_hostile_corpus_on_frontend(&cpp_fe, &uno_ctx), Ok(()));
    assert_eq!(verify_hostile_corpus_on_frontend(&lad_fe, &plc_ctx), Ok(()));
    assert_eq!(verify_hostile_corpus_on_frontend(&sfc_fe, &plc_ctx), Ok(()));
    for &dialect in StCommentDialect::ALL {
        let mut d_ctx = plc_ctx.clone();
        d_ctx.st_dialect = dialect;
        let d_st = StFrontend::with_dialect(dialect);
        assert_eq!(
            verify_hostile_corpus_on_frontend(&d_st, &d_ctx),
            Ok(()),
            "C3 hostile corpus failed on real StFrontend dialect {dialect:?}"
        );
    }

    // 5b. Construct a rich multi-POU BoardDoc containing:
    //     OB_STARTUP, OB_MAIN (ST), OB_HW_INT (ISR), MotorLadder (Network), BatchChart (Chart),
    //     CppHelper (FC), Foreign nodes (Decl + Stmt + Expr), Assumption, and hostile_comment_corpus()!
    let ladder_pou_bundle = lad_fe
        .parse_and_validate(
            r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
END_VAR
NETWORK MotorLadder (n_lad_pou) KIND FB
    RUNG 0 (n_lad_r0): | | StartBtn @(0, 0, n_lad_c0) -- ( ) MotorOut @(0, 1, n_lad_c1);
END_NETWORK
"#,
            &plc_ctx,
        )
        .expect("parse initial MotorLadder");
    let ladder_pou = ladder_pou_bundle.value.program.pous[0].clone();

    let sfc_pou_bundle = sfc_fe
        .parse(
            r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    ValveOut AT %QX0.1 : BOOL;
END_VAR
CHART BatchChart (KIND := FB)
    INITIAL_STEP Idle:
    END_STEP
    STEP Active:
        N: ValveOut;
    END_STEP
    TRANSITION T_go FROM Idle TO Active := StartBtn;
    END_TRANSITION
END_CHART
"#,
            &plc_ctx,
        )
        .expect("parse initial BatchChart");
    let sfc_pou = sfc_pou_bundle.value.program.pous[0].clone();

    // Seed verbatim comments from hostile_comment_corpus() anchored onto surviving nodes
    let mut seeded_comments = vec![
        Comment {
            id: "c_startup".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_ob_startup"),
            },
            placement: AstCommentPlacement::Leading,
            token_ordinal: None,
            text: " Boot initialization — DO NOT CLOBBER".to_owned(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: CommentStyle::ParenBlock,
            },
            span: Span::default(),
        },
        Comment {
            id: "c_isr".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_ob_hw_int"),
            },
            placement: AstCommentPlacement::Leading,
            token_ordinal: None,
            text: " Emergency stop ISR handler".to_owned(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: CommentStyle::Line,
            },
            span: Span::default(),
        },
        Comment {
            id: "c_orphan_target".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_temp_flag"),
            },
            placement: AstCommentPlacement::Leading,
            token_ordinal: None,
            text: " Diagnostic flag comment preserved on deletion (C2)".to_owned(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: CommentStyle::Line,
            },
            span: Span::default(),
        },
    ];
    for (idx, hcase) in hostile_comment_corpus().into_iter().enumerate() {
        let (placement, token_ordinal) = match hcase.placement {
            dendri_frontend_core::CommentPlacement::Leading => (AstCommentPlacement::Leading, None),
            dendri_frontend_core::CommentPlacement::Trailing => {
                (AstCommentPlacement::Trailing, None)
            }
            dendri_frontend_core::CommentPlacement::Inner { token_ordinal } => {
                (AstCommentPlacement::Inner, Some(token_ordinal))
            }
            dendri_frontend_core::CommentPlacement::Header => (AstCommentPlacement::Header, None),
            dendri_frontend_core::CommentPlacement::Title => (AstCommentPlacement::Title, None),
            dendri_frontend_core::CommentPlacement::Body => (AstCommentPlacement::Body, None),
            dendri_frontend_core::CommentPlacement::Dangling => {
                (AstCommentPlacement::Dangling, None)
            }
        };
        seeded_comments.push(Comment {
            id: format!("c_hostile_{idx:02}"),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_ob_startup"),
            },
            placement,
            token_ordinal,
            text: hcase.text,
            origin: CommentOrigin {
                language: "cpp".to_owned(),
                style: hcase.style,
            },
            span: Span::default(),
        });
    }
    assert_eq!(assert_c5_comment_fields(&seeded_comments), Ok(()));

    let initial_assumption = Assumption {
        node: NodeId::new("n_foreign_stmt"),
        expr_ordinal: None,
        policy: AssumptionPolicy::NoEffect,
        value: None,
        note: "Direct C++ register stub modeled as no-op during PLC sync verification".to_owned(),
        span: Span::default(),
    };

    let board = Board {
        id: "plc_sync".to_owned(),
        profile: "plc-1200-open-1214".to_owned(),
        authoring_language: AuthoringLanguage::St,
        execution: ExecutionKind::PlcScan,
        scan_period_us: Some(10_000),
        layout: BoardLayout::default(),
        program: BoardProgram {
            imports: vec!["std.iec.*".to_owned()],
            hardware: vec![
                HardwareBinding {
                    id: NodeId::new("n_hw_start"),
                    label: "StartBtn".to_owned(),
                    pin: "Ia.0".to_owned(),
                    kind: HardwareKind::DigitalIn,
                    pull: PullMode::None,
                    active_low: false,
                    codec: None,
                    range: None,
                    span: Span::default(),
                },
                HardwareBinding {
                    id: NodeId::new("n_hw_estop"),
                    label: "EStopIn".to_owned(),
                    pin: "Ia.2".to_owned(),
                    kind: HardwareKind::DigitalIn,
                    pull: PullMode::None,
                    active_low: false,
                    codec: None,
                    range: None,
                    span: Span::default(),
                },
                HardwareBinding {
                    id: NodeId::new("n_hw_motor"),
                    label: "MotorOut".to_owned(),
                    pin: "Qa.0".to_owned(),
                    kind: HardwareKind::DigitalOut,
                    pull: PullMode::None,
                    active_low: false,
                    codec: None,
                    range: None,
                    span: Span::default(),
                },
                HardwareBinding {
                    id: NodeId::new("n_hw_valve"),
                    label: "ValveOut".to_owned(),
                    pin: "Qa.1".to_owned(),
                    kind: HardwareKind::DigitalOut,
                    pull: PullMode::None,
                    active_low: false,
                    codec: None,
                    range: None,
                    span: Span::default(),
                },
            ],
            types: vec![TypeDecl::Foreign {
                id: NodeId::new("n_foreign_decl"),
                name: Some("NativeFastReg".to_owned()),
                language: "cpp".to_owned(),
                fidelity: ForeignFidelity::F0,
                text: "struct NativeFastReg { uint8_t raw; void reset(); };".to_owned(),
                pack_hint: None,
                span: Span::default(),
            }],
            data: vec![
                DataDecl {
                    id: NodeId::new("n_cycle_cnt"),
                    name: "cycleCnt".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Dint),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(0)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("n_isr_hits"),
                    name: "isrHits".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Dint),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(0)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("n_temp_flag"),
                    name: "tempFlag".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Bool),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Bool(false)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
            ],
            modules: Vec::new(),
            pous: vec![
                Pou {
                    id: NodeId::new("n_ob_startup"),
                    name: "OB_STARTUP".to_owned(),
                    kind: PouKind::Ob {
                        ob: ObKind::Startup,
                    },
                    preferred_view: Some(AuthoringLanguage::St),
                    params: Vec::new(),
                    locals: Vec::new(),
                    body: PouBody::Statements(vec![
                        Stmt::new(
                            NodeId::new("n_boot_stmt"),
                            StmtKind::Assign {
                                target: LValue::Var {
                                    name: "cycleCnt".to_owned(),
                                    span: Span::default(),
                                },
                                value: Expr::new(
                                    ExprKind::Literal(Literal::Int(1)),
                                    Span::default(),
                                ),
                            },
                            Span::default(),
                        ),
                        Stmt::new(
                            NodeId::new("n_foreign_stmt"),
                            StmtKind::Foreign {
                                language: "cpp".to_owned(),
                                fidelity: ForeignFidelity::F0,
                                text: "PORTB |= (1 << 5);".to_owned(),
                                pack_hint: None,
                            },
                            Span::default(),
                        ),
                    ]),
                    span: Span::default(),
                },
                Pou {
                    id: NodeId::new("n_ob_main"),
                    name: "OB_MAIN".to_owned(),
                    kind: PouKind::Ob { ob: ObKind::Main },
                    preferred_view: Some(AuthoringLanguage::St),
                    params: Vec::new(),
                    locals: Vec::new(),
                    body: PouBody::Statements(vec![Stmt::new(
                        NodeId::new("n_main_s0"),
                        StmtKind::Assign {
                            target: LValue::Var {
                                name: "cycleCnt".to_owned(),
                                span: Span::default(),
                            },
                            value: Expr::new(
                                ExprKind::Binary {
                                    op: dendri_ast::BinaryOp::Add,
                                    left: Box::new(Expr::new(
                                        ExprKind::Ident("cycleCnt".to_owned()),
                                        Span::default(),
                                    )),
                                    right: Box::new(Expr::new(
                                        ExprKind::Literal(Literal::Int(1)),
                                        Span::default(),
                                    )),
                                },
                                Span::default(),
                            ),
                        },
                        Span::default(),
                    )]),
                    span: Span::default(),
                },
                Pou {
                    id: NodeId::new("n_ob_hw_int"),
                    name: "OB_HW_INT".to_owned(),
                    kind: PouKind::Ob {
                        ob: ObKind::HwInt {
                            pin_label: "EStopIn".to_owned(),
                            edge: InterruptEdge::Rising,
                            priority: 1,
                        },
                    },
                    preferred_view: Some(AuthoringLanguage::St),
                    params: Vec::new(),
                    locals: Vec::new(),
                    body: PouBody::Statements(vec![Stmt::new(
                        NodeId::new("n_isr_s0"),
                        StmtKind::Assign {
                            target: LValue::Var {
                                name: "isrHits".to_owned(),
                                span: Span::default(),
                            },
                            value: Expr::new(
                                ExprKind::Binary {
                                    op: dendri_ast::BinaryOp::Add,
                                    left: Box::new(Expr::new(
                                        ExprKind::Ident("isrHits".to_owned()),
                                        Span::default(),
                                    )),
                                    right: Box::new(Expr::new(
                                        ExprKind::Literal(Literal::Int(1)),
                                        Span::default(),
                                    )),
                                },
                                Span::default(),
                            ),
                        },
                        Span::default(),
                    )]),
                    span: Span::default(),
                },
                ladder_pou,
                sfc_pou,
                Pou {
                    id: NodeId::new("n_cpp_helper"),
                    name: "CppHelper".to_owned(),
                    kind: PouKind::Fc {
                        return_type: TypeRef::scalar(ScalarType::Dint),
                    },
                    preferred_view: Some(AuthoringLanguage::Cpp),
                    params: vec![ParamDecl {
                        name: "x".to_owned(),
                        direction: ParamDirection::In,
                        ty: TypeRef::scalar(ScalarType::Dint),
                    }],
                    locals: Vec::new(),
                    body: PouBody::Statements(vec![Stmt::new(
                        NodeId::new("n_cpp_ret"),
                        StmtKind::Return {
                            value: Some(Expr::new(
                                ExprKind::Binary {
                                    op: dendri_ast::BinaryOp::Add,
                                    left: Box::new(Expr::new(
                                        ExprKind::Ident("x".to_owned()),
                                        Span::default(),
                                    )),
                                    right: Box::new(Expr::new(
                                        ExprKind::Literal(Literal::Int(1)),
                                        Span::default(),
                                    )),
                                },
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    )]),
                    span: Span::default(),
                },
            ],
        },
        comments: seeded_comments.clone(),
        assumptions: vec![initial_assumption.clone()],
        presentation: BoardPresentation::default(),
        span: Span::default(),
    };

    let mut doc = BoardDoc::new(board);
    assert!(
        !doc.validate(&catalog, &ValidateOptions::default())
            .has_errors(),
        "initial multi-POU BoardDoc must pass dendri-validate against live AssetCatalog"
    );

    let orig_startup = doc
        .board
        .program
        .pous
        .iter()
        .find(|p| p.name == "OB_STARTUP")
        .cloned()
        .expect("OB_STARTUP");
    let orig_isr = doc
        .board
        .program
        .pous
        .iter()
        .find(|p| p.name == "OB_HW_INT")
        .cloned()
        .expect("OB_HW_INT");

    // 5c. Edit single POU (`OB_MAIN`) via real StFrontend
    let st_edit_src = r#"VAR_GLOBAL
    cycleCnt : DINT := 0;
END_VAR
PROGRAM OB_MAIN
    cycleCnt := cycleCnt + 5;
END_PROGRAM
"#;
    let r1 = engine
        .apply_view_edit(
            &mut doc,
            "st:OB_MAIN",
            st_edit_src,
            &st_fe,
            &plc_ctx,
            &catalog,
        )
        .expect("ST single-POU edit must succeed");
    assert_eq!(r1.revision, 1);
    assert_eq!(r1.updated_pous, vec!["OB_MAIN".to_owned()]);

    // 5d. Edit single POU (`MotorLadder`) via real LadderFrontend
    let lad_edit_src = r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    EStopIn AT %IX0.2 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
END_VAR
NETWORK MotorLadder (n_lad_pou) KIND FB
    RUNG 0 (n_lad_r0): | | StartBtn @(0, 0, n_lad_c0) -- |/| EStopIn @(0, 1, n_lad_c2) -- ( ) MotorOut @(0, 2, n_lad_c1);
END_NETWORK
"#;
    let r2 = engine
        .apply_view_edit(
            &mut doc,
            "ladder:MotorLadder",
            lad_edit_src,
            &lad_fe,
            &plc_ctx,
            &catalog,
        )
        .expect("Ladder single-POU edit must succeed");
    assert_eq!(r2.revision, 2);
    assert_eq!(r2.updated_pous, vec!["MotorLadder".to_owned()]);

    // 5e. Edit single POU (`BatchChart`) via real SfcFrontend
    let sfc_edit_src = r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    EStopIn AT %IX0.2 : BOOL;
    ValveOut AT %QX0.1 : BOOL;
END_VAR
CHART BatchChart (KIND := FB)
    INITIAL_STEP Idle:
    END_STEP
    STEP Active:
        N: ValveOut;
    END_STEP
    TRANSITION T_go FROM Idle TO Active := StartBtn;
    END_TRANSITION
    TRANSITION T_stop FROM Active TO Idle := EStopIn;
    END_TRANSITION
END_CHART
"#;
    let r3 = engine
        .apply_view_edit(
            &mut doc,
            "sfc:BatchChart",
            sfc_edit_src,
            &sfc_fe,
            &plc_ctx,
            &catalog,
        )
        .expect("SFC single-POU edit must succeed");
    assert_eq!(r3.revision, 3);
    assert_eq!(r3.updated_pous, vec!["BatchChart".to_owned()]);

    // 5f. Edit single POU (`CppHelper`) via real CppFrontend
    let cpp_edit_src = r#"int32_t CppHelper(int32_t x) {
  return x + 42;
}
"#;
    let r4 = engine
        .apply_view_edit(
            &mut doc,
            "cpp:CppHelper",
            cpp_edit_src,
            &cpp_fe,
            &plc_ctx,
            &catalog,
        )
        .expect("C++ single-POU edit must succeed");
    assert_eq!(r4.revision, 4);
    assert_eq!(r4.updated_pous, vec!["CppHelper".to_owned()]);

    // Verify ZERO silent clobbering across ALL four real frontend edits!
    assert_eq!(
        doc.board
            .program
            .pous
            .iter()
            .find(|p| p.name == "OB_STARTUP"),
        Some(&orig_startup),
        "OB_STARTUP (including Foreign stmt) must survive verbatim"
    );
    assert_eq!(
        doc.board
            .program
            .pous
            .iter()
            .find(|p| p.name == "OB_HW_INT"),
        Some(&orig_isr),
        "OB_HW_INT ISR must survive verbatim"
    );
    assert_eq!(doc.board.assumptions, vec![initial_assumption]);
    assert!(
        collect_foreign_sites(&doc.board.program).len() >= 2,
        "Foreign Decl and Foreign Stmt must both survive untouched"
    );
    for orig_c in &seeded_comments {
        let found = doc
            .board
            .comments
            .iter()
            .find(|c| c.id == orig_c.id)
            .expect("seeded hostile/verbatim comment must survive");
        assert_eq!(
            found.text, orig_c.text,
            "verbatim text of comment `{}` MUST NEVER be modified",
            orig_c.id
        );
    }

    // 5g. Syntax error preserves dirty buffer without clobbering authoritative ASL or revision
    let board_before_error = doc.board.clone();
    let rev_before_error = doc.revision();
    let broken_st = "PROGRAM OB_MAIN\n    cycleCnt := (1 + ;\nEND_PROGRAM\n";
    let err = engine
        .apply_view_edit(
            &mut doc,
            "st:OB_MAIN",
            broken_st,
            &st_fe,
            &plc_ctx,
            &catalog,
        )
        .expect_err("syntax error must be rejected");
    assert!(matches!(err, SyncEditError::ParseFailed { .. }));
    assert_eq!(doc.board, board_before_error);
    assert_eq!(doc.revision(), rev_before_error);
    match engine
        .project_view_or_dirty(
            &doc,
            "st:OB_MAIN",
            ExportScope::Pou("OB_MAIN"),
            &st_fe,
            &plc_ctx,
            false,
        )
        .expect("project_view_or_dirty")
    {
        ViewProjection::Dirty(buf) => {
            assert_eq!(buf.source, broken_st);
            assert!(buf.error_count() >= 1);
        }
        ViewProjection::Clean(_) => panic!("dirty buffer must be retained across tab switch"),
    }

    // 5h. Undo / Redo restores exact Board state monotonically while keeping dirty buffer intact
    let board_after_cpp_edit = doc.board.clone();
    let undo_out = engine.undo(&mut doc).expect("undo CppHelper edit");
    assert_eq!(undo_out.revision, rev_before_error + 1);
    assert!(doc.has_dirty_buffer("st:OB_MAIN"));
    assert_ne!(doc.board, board_after_cpp_edit);

    let redo_out = engine.redo(&mut doc).expect("redo CppHelper edit");
    assert_eq!(redo_out.revision, rev_before_error + 2);
    assert_eq!(doc.board, board_after_cpp_edit);
    assert!(doc.has_dirty_buffer("st:OB_MAIN"));

    // 5i. C2 orphan preservation:
    // (1) Removing a top-level global data decl relocates its comment onto the board scope (Body) + INFO_COMMENT_ORPHANED
    let orphan_report = engine
        .remove_data_decl(&mut doc, NodeId::new("n_temp_flag"), &plc_ctx, &catalog)
        .expect("remove unused data decl");
    assert!(
        orphan_report
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentOrphaned),
        "deleting anchored global node must emit INFO_COMMENT_ORPHANED"
    );
    let orphaned = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_orphan_target")
        .expect("orphaned comment MUST NOT be deleted");
    assert_eq!(
        orphaned.anchor,
        CommentAnchor::Board {
            id: "plc_sync".to_owned(),
        }
    );
    assert_eq!(orphaned.placement, AstCommentPlacement::Body);
    assert_eq!(
        orphaned.text,
        " Diagnostic flag comment preserved on deletion (C2)"
    );

    // (2) Deleting a statement inside OB_MAIN transitions its comment to Dangling on OB_MAIN + INFO_COMMENT_ORPHANED
    let current_main_stmt_id = match &doc
        .board
        .program
        .pous
        .iter()
        .find(|p| p.name == "OB_MAIN")
        .expect("OB_MAIN")
        .body
    {
        PouBody::Statements(stmts) => stmts[0].id.clone(),
        PouBody::Network(_) | PouBody::Chart(_) => panic!("expected OB_MAIN statements"),
    };
    engine
        .add_comment(
            &mut doc,
            Comment {
                id: "c_inner_stmt_orphan".to_owned(),
                anchor: CommentAnchor::Node {
                    id: current_main_stmt_id,
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " Child statement note preserved as Dangling on POU (C2)".to_owned(),
                origin: CommentOrigin {
                    language: "st".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            &plc_ctx,
            &catalog,
        )
        .expect("attach comment to OB_MAIN child statement");
    let empty_main_edit =
        "VAR_GLOBAL\n    cycleCnt : DINT := 0;\nEND_VAR\nPROGRAM OB_MAIN\nEND_PROGRAM\n";
    let child_orphan_report = engine
        .apply_view_edit(
            &mut doc,
            "st:OB_MAIN_clear",
            empty_main_edit,
            &st_fe,
            &plc_ctx,
            &catalog,
        )
        .expect("clear OB_MAIN body");
    assert!(
        child_orphan_report
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentOrphaned),
        "deleting child statement must emit INFO_COMMENT_ORPHANED"
    );
    let dangling_child = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_inner_stmt_orphan")
        .expect("child statement comment MUST NOT be deleted");
    assert_eq!(
        dangling_child.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_ob_main"),
        }
    );
    assert_eq!(dangling_child.placement, AstCommentPlacement::Dangling);
    assert_eq!(
        dangling_child.text,
        " Child statement note preserved as Dangling on POU (C2)"
    );

    // 5j. Cross-language export gate (`ERR_FOREIGN_EXPORT_BLOCKED` vs `allow_foreign = true`)
    //     vs same-language export (NEVER blocked!)
    let blocked_errs = engine
        .export_view(&doc, ExportScope::Board, &st_fe, &plc_ctx, false)
        .expect_err("cross-language export of cpp Foreign to ST MUST be blocked by default");
    assert!(
        blocked_errs
            .iter()
            .any(|d| d.code == ErrorCode::ErrForeignExportBlocked && is_four_part_diagnostic(d)),
        "expected four-part ERR_FOREIGN_EXPORT_BLOCKED, got {blocked_errs:?}"
    );

    let allowed_out = engine
        .export_view(&doc, ExportScope::Board, &st_fe, &plc_ctx, true)
        .expect("cross-language export with allow_foreign=true must succeed");
    assert!(
        allowed_out.value.contains("NOT TRANSLATED"),
        "allow_foreign=true must emit clearly demarcated NOT TRANSLATED block:\n{}",
        allowed_out.value
    );

    // Same-language C++ board containing cpp Foreign MUST NEVER be blocked even with allow_foreign=false!
    let cpp_foreign_parsed = cpp_fe
        .parse(
            r#"#include <Arduino.h>
void loop() {
  PORTB |= (1 << 5);
}
"#,
            &uno_ctx,
        )
        .expect("parse C++ Foreign PORTB sketch");
    let cpp_foreign_doc = BoardDoc::new(make_uno_board("uno_foreign", &cpp_foreign_parsed.value));
    let same_lang_out = engine
        .export_view(
            &cpp_foreign_doc,
            ExportScope::Board,
            &cpp_fe,
            &uno_ctx,
            false,
        )
        .expect("same-language Foreign export MUST NEVER be blocked");
    assert!(same_lang_out.value.contains("PORTB"));
    assert!(!same_lang_out.value.contains("NOT TRANSLATED"));

    // Final validation of the full multi-view BoardDoc against live AssetCatalog
    assert!(
        !doc.validate(&catalog, &ValidateOptions::default())
            .has_errors(),
        "final synced BoardDoc must pass dendri-validate against live AssetCatalog"
    );
}

// ---------------------------------------------------------------------------
// Scenario 6: Real assets/packs/*.pack.toml + checksums.toml (DEF-008) End-to-End
// ---------------------------------------------------------------------------

#[test]
fn scenario_6_real_assets_packs_and_checksums_end_to_end() {
    let root = workspace_root();
    let packs_dir = root.join("assets").join("packs");
    let catalog = load_live_catalog();

    // 6a. Parse & verify checksums.toml + all 4 real Wave 1 packs
    let checksums_src = fs::read_to_string(packs_dir.join("checksums.toml"))
        .expect("read assets/packs/checksums.toml");
    let checksums = PackChecksums::parse_str(&checksums_src).expect("parse checksums.toml");
    assert_eq!(checksums.packs.len(), 4, "expected 4 Wave 1 packs");

    let expected_ids = [
        "arduino-servo",
        "arduino-spi",
        "arduino-wire",
        "plc-modbus-tcp",
    ];
    let mut manifests: BTreeMap<String, PackManifest> = BTreeMap::new();
    for id in expected_ids {
        let lock = checksums
            .lock_for_id(id)
            .unwrap_or_else(|| panic!("missing lock entry for pack `{id}`"));
        let pack_path = root.join(&lock.path);
        let manifest = PackManifest::from_file_verified(&pack_path, lock)
            .unwrap_or_else(|e| panic!("failed SHA-256 verification for `{id}`: {e}"));
        assert_eq!(manifest.pack.id, id);
        assert!(!manifest.symbols.is_empty());
        manifests.insert(id.to_owned(), manifest);
    }

    // Tampered content MUST fail dynamic SHA-256 checksum verification!
    let wire_lock = checksums.lock_for_id("arduino-wire").expect("wire lock");
    let wire_raw =
        fs::read_to_string(root.join(&wire_lock.path)).expect("read arduino-wire.pack.toml");
    let tampered = format!("{wire_raw}\n# tampered\n");
    assert!(matches!(
        PackManifest::parse_str_verified(&tampered, &wire_lock.sha256),
        Err(PackError::ChecksumMismatch { .. })
    ));

    // 6b. Exercise Wire, SPI, and Servo F1 pack calls in CppFrontend
    let cpp_fe = CppFrontend::new();
    let mut uno_ctx = BoardContext::arduino_uno_r3("uno_packs");
    uno_ctx.loaded_packs = vec![
        manifests["arduino-wire"].clone(),
        manifests["arduino-spi"].clone(),
        manifests["arduino-servo"].clone(),
    ];

    let cpp_pack_src = r#"#include <Arduino.h>
#include <Wire.h>
#include <SPI.h>
#include <Servo.h>

Servo gripper;
uint8_t spiRx = 0;
int16_t i2cByte = 0;

void setup() {
  Wire.begin();
  SPI.begin();
  gripper.attach(9);
}

void loop() {
  Wire.beginTransmission(0x50);
  Wire.write(0x10);
  Wire.endTransmission(true);
  uint8_t n = Wire.requestFrom(0x50, 1);
  if (n > 0) {
    i2cByte = Wire.read();
  }
  SPI.beginTransaction(1000000UL, MSBFIRST, SPI_MODE0);
  spiRx = SPI.transfer(0xA5);
  SPI.endTransaction();
  gripper.write(90);
  gripper.writeMicroseconds(1500);
  gripper.detach();
}
"#;
    let cpp_parsed = cpp_fe
        .parse(cpp_pack_src, &uno_ctx)
        .expect("parse C++ F1 pack sketch");
    let cpp_sites = collect_foreign_sites(&cpp_parsed.value.program);
    assert!(
        cpp_sites
            .iter()
            .any(|s| s.fidelity == ForeignFidelity::F1 && s.pack_hint == Some("arduino-wire")),
        "expected F1 arduino-wire foreign sites"
    );
    assert!(
        cpp_sites
            .iter()
            .any(|s| s.fidelity == ForeignFidelity::F1 && s.pack_hint == Some("arduino-spi")),
        "expected F1 arduino-spi foreign sites"
    );
    assert!(
        cpp_sites
            .iter()
            .any(|s| s.fidelity == ForeignFidelity::F1 && s.pack_hint == Some("arduino-servo")),
        "expected F1 arduino-servo foreign sites"
    );
    verify_frontend_roundtrip(&cpp_fe, &cpp_parsed.value, &uno_ctx)
        .expect("C++ F1 pack sketch LOSSLESS_ASL");

    // Wrong arity on F1 pack symbol MUST trigger ERR_ARITY_MISMATCH
    let bad_cpp_arity = "#include <SPI.h>\nvoid loop() { SPI.transfer(1, 2); }\n";
    let cpp_errs = cpp_fe
        .parse(bad_cpp_arity, &uno_ctx)
        .expect_err("wrong arity on SPI.transfer must error");
    assert!(
        cpp_errs
            .iter()
            .any(|d| d.code == ErrorCode::ErrArityMismatch),
        "expected ERR_ARITY_MISMATCH, got {cpp_errs:?}"
    );

    // 6c. Exercise MB_CLIENT and MB_SERVER F1 pack calls in StFrontend & LadderFrontend
    let st_fe = StFrontend::new();
    let lad_fe = LadderFrontend::new();
    let mut plc_ctx = BoardContext::plc_1200_open_1214("plc_modbus");
    plc_ctx.loaded_packs = vec![manifests["plc-modbus-tcp"].clone()];

    let st_modbus_src = r#"VAR_GLOBAL
    PollReq AT %IX0.0 : BOOL;
    SrvEnable AT %IX0.1 : BOOL;
    CommOk AT %QX0.0 : BOOL;
    SrvReady AT %QX0.1 : BOOL;
    mbClient : MB_CLIENT;
    mbServer : MB_SERVER;
    lastStatus : WORD := 16#0000;
END_VAR

PROGRAM OB_MAIN
    mbClient(
        REQ := PollReq,
        DISCONNECT := FALSE,
        REMOTE_IP := '192.168.1.50',
        REMOTE_PORT := 502,
        UNIT_ID := 1,
        FC := 3,
        ADDR := 0,
        QTY := 2,
        DATA_WR := 0
    );
    CommOk := mbClient.DONE AND NOT mbClient.ERR;
    lastStatus := mbClient.STATUS;

    mbServer(
        ENABLE := SrvEnable,
        BIND_IP := '0.0.0.0',
        PORT := 502,
        UNIT_ID := 1,
        DISCONNECT := FALSE
    );
    SrvReady := mbServer.NDR AND NOT mbServer.ERR;
END_PROGRAM
"#;
    let st_mb_parsed = st_fe
        .parse(st_modbus_src, &plc_ctx)
        .expect("parse ST MB_CLIENT + MB_SERVER");
    verify_frontend_roundtrip(&st_fe, &st_mb_parsed.value, &plc_ctx)
        .expect("ST Modbus F1 pack LOSSLESS_ASL");
    let st_mb_board = make_plc_board("plc_st_modbus", AuthoringLanguage::St, &st_mb_parsed.value);
    assert_valid_against_live_catalog(&st_mb_board, &catalog);

    // LadderFrontend exercising MB_CLIENT & MB_SERVER FB boxes
    let lad_modbus_src = r#"VAR_GLOBAL
    PollReq AT %IX0.0 : BOOL;
    SrvEnable AT %IX0.1 : BOOL;
    CommOk AT %QX0.0 : BOOL;
    SrvReady AT %QX0.1 : BOOL;
    mbClient : MB_CLIENT;
    mbServer : MB_SERVER;
END_VAR

NETWORK OB_MAIN
    RUNG 0 (n_mb_r0): | | PollReq @(0, 0, n_req) -- FB MB_CLIENT mbClient(DISCONNECT := FALSE, REMOTE_IP := "192.168.1.50", REMOTE_PORT := 502, UNIT_ID := 1, FC := 3, ADDR := 0, QTY := 2, DATA_WR := 0) @(0, 1, n_mbc) -- ( ) CommOk @(0, 2, n_ok);
    RUNG 1 (n_mb_r1): | | SrvEnable @(0, 0, n_en) -- FB MB_SERVER mbServer(BIND_IP := "0.0.0.0", PORT := 502, UNIT_ID := 1, DISCONNECT := FALSE) @(0, 1, n_mbs) -- ( ) SrvReady @(0, 2, n_ndr);
END_NETWORK
"#;
    let lad_mb_parsed = lad_fe
        .parse_and_validate(lad_modbus_src, &plc_ctx)
        .expect("parse & validate Ladder MB_CLIENT + MB_SERVER");
    verify_frontend_roundtrip(&lad_fe, &lad_mb_parsed.value, &plc_ctx)
        .expect("Ladder Modbus F1 pack LOSSLESS_ASL");
    assert!(
        lad_mb_parsed.value.program.data.iter().any(|d| matches!(
            d.ty,
            TypeRef::Fb {
                fb_kind: FbKind::MbClient
            }
        )),
        "Ladder bundle must declare MB_CLIENT FB"
    );
    assert!(
        lad_mb_parsed.value.program.data.iter().any(|d| matches!(
            d.ty,
            TypeRef::Fb {
                fb_kind: FbKind::MbServer
            }
        )),
        "Ladder bundle must declare MB_SERVER FB"
    );
    let lad_mb_board = make_plc_board(
        "plc_lad_modbus",
        AuthoringLanguage::Ladder,
        &lad_mb_parsed.value,
    );
    assert_valid_against_live_catalog(&lad_mb_board, &catalog);
}

// ---------------------------------------------------------------------------
// Scenario 7: Auto-Generated Feature Matrix Drift Gate (docs/lbc/feature-matrix.md)
// ---------------------------------------------------------------------------

fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn crate_contains_fn(crate_dir: &Path, fn_name: &str) -> bool {
    let mut files = Vec::new();
    collect_rust_sources(&crate_dir.join("src"), &mut files);
    collect_rust_sources(&crate_dir.join("tests"), &mut files);

    let needle_paren = format!("fn {fn_name}(");
    let needle_space = format!("fn {fn_name} ");
    let needle_generic = format!("fn {fn_name}<");

    for file in files {
        let Ok(src) = fs::read_to_string(&file) else {
            continue;
        };
        if src.contains(&needle_paren)
            || src.contains(&needle_space)
            || src.contains(&needle_generic)
        {
            return true;
        }
    }
    false
}

fn render_feature_matrix_document(collector: &FeatureMatrixCollector) -> String {
    let mut doc = String::new();
    doc.push_str("# Wave 1 Language Feature Matrix\n\n");
    doc.push_str(
        "> **Auto-Generated Normative Reference** — Generated directly from `FeatureMatrixCollector`\n",
    );
    doc.push_str(
        "> across `dendri-lang-cpp`, `dendri-lang-st`, `dendri-lang-ladder`, and `dendri-lang-sfc`\n",
    );
    doc.push_str(
        "> per [`docs/architecture/04-frontends-visual-and-sync.md`](../architecture/04-frontends-visual-and-sync.md) §2.\n",
    );
    doc.push_str(
        "> MUST NOT be hand-edited. Regenerate with `DENDRI_UPDATE_FEATURE_MATRIX=1 cargo test -p dendri-sync --test wave1_golden_equivalence scenario_7`.\n\n",
    );
    doc.push_str(&collector.to_markdown());
    doc
}

#[test]
fn scenario_7_feature_matrix_auto_generation_and_drift_gate() {
    let root = workspace_root();
    let mut collector = FeatureMatrixCollector::new();

    let crate_sources: &[(
        LanguageId,
        &str,
        Vec<dendri_frontend_core::FeatureMatrixEntry>,
    )] = &[
        (
            LanguageId::Cpp,
            "crates/dendri-lang-cpp",
            dendri_lang_cpp::feature_matrix_entries(),
        ),
        (
            LanguageId::St,
            "crates/dendri-lang-st",
            dendri_lang_st::feature_matrix_entries(),
        ),
        (
            LanguageId::Ladder,
            "crates/dendri-lang-ladder",
            dendri_lang_ladder::feature_matrix_entries(),
        ),
        (
            LanguageId::Sfc,
            "crates/dendri-lang-sfc",
            dendri_lang_sfc::feature_matrix_entries(),
        ),
    ];

    for (expected_lang, rel_crate_dir, entries) in crate_sources {
        assert!(
            !entries.is_empty(),
            "language `{expected_lang}` must publish non-empty feature_matrix_entries()"
        );
        let crate_dir = root.join(rel_crate_dir);
        for entry in entries {
            assert_eq!(
                entry.language, *expected_lang,
                "mismatched language tag in {rel_crate_dir}"
            );
            assert!(
                !entry.category.trim().is_empty() && !entry.feature.trim().is_empty(),
                "feature matrix entry must have non-empty category and feature"
            );
            assert!(
                !entry.evidence_test.trim().is_empty(),
                "feature matrix entry `{}` must name a live evidence_test",
                entry.feature
            );
            assert!(
                crate_contains_fn(&crate_dir, &entry.evidence_test),
                "evidence_test `fn {}` declared by `{rel_crate_dir}` does not exist in src/ or tests/",
                entry.evidence_test
            );
            collector.record(entry.clone());
        }
    }

    let rendered = render_feature_matrix_document(&collector);
    assert!(!contains_banned_magic_comment(&rendered));
    assert!(
        rendered.contains("| Category | Feature | C++ | ST | Ladder | SFC | Evidence / Notes |")
    );

    // Guard against fragmented per-language taxonomy drift: cross-cutting Wave 1 contracts
    // (Comments, Conformance LOSSLESS_ASL, Negative Fixtures, Foreign F0, Hardware I/O, OBs/POUs)
    // MUST align into unified rows across all four Wave 1 language columns.
    let four_lang_rows = rendered
        .lines()
        .filter(|line| {
            let cols: Vec<&str> = line.split('|').map(str::trim).collect();
            cols.len() >= 7
                && !matches!(cols[1], "" | "Category" | "---")
                && cols[3] != "—"
                && cols[4] != "—"
                && cols[5] != "—"
                && cols[6] != "—"
        })
        .count();
    assert!(
        four_lang_rows >= 6,
        "expected >= 6 unified 4-language comparison rows in docs/lbc/feature-matrix.md, found {four_lang_rows}"
    );

    let matrix_path = root.join("docs").join("lbc").join("feature-matrix.md");
    if std::env::var("DENDRI_UPDATE_FEATURE_MATRIX").as_deref() == Ok("1") {
        fs::write(&matrix_path, &rendered).expect("write docs/lbc/feature-matrix.md");
    }

    let committed = fs::read_to_string(&matrix_path).unwrap_or_else(|e| {
        panic!(
            "failed to read `{}` (regenerate with DENDRI_UPDATE_FEATURE_MATRIX=1): {e}",
            matrix_path.display()
        )
    });
    assert_eq!(
        committed, rendered,
        "docs/lbc/feature-matrix.md drifted from live FeatureMatrixCollector output; regenerate with DENDRI_UPDATE_FEATURE_MATRIX=1"
    );
}

// ---------------------------------------------------------------------------
// Scenario 8: Gate 8 (C1) — Full Wave 1 Cross-Language Translation Chain
//             (C++ -> ST -> Ladder -> SFC -> ST -> C++)
// ---------------------------------------------------------------------------

#[test]
fn scenario_8_c1_comment_multiset_invariant_across_full_wave1_chain() {
    let cpp_fe = CppFrontend::new();
    let st_fe = StFrontend::new();
    let lad_fe = LadderFrontend::new();
    let sfc_fe = SfcFrontend::new();

    // Pure algorithmic program (zero hardware bindings) authored in C++ with verbatim comments
    // covering line, block, Unicode/accented text, emoji, and delimiter-hazard sequences.
    let cpp_src = r#"// C1 Chain Header: safety counter
#include <stdint.h>

/* Leading block note: keep >= 0 (café / π / ⚡) */
int32_t acc = 0;

// Compute next step with hazard tokens: *) and backslash \
int32_t step_calc(int32_t v) {
  // Trailing backslash hazard \
  return v + 1;
}

void loop() {
  /* Update accumulator cleanly */
  acc = step_calc(acc);
}
"#;

    let uno_ctx = BoardContext::arduino_uno_r3("chain_board");
    let plc_ctx = BoardContext::plc_1200_open_1214("chain_board");

    // Step 1: Parse C++
    let hop0_cpp = cpp_fe
        .parse(cpp_src, &uno_ctx)
        .expect("parse initial C++")
        .value;
    assert!(
        hop0_cpp.comments.len() >= 4,
        "expected >= 4 comments in initial C++ program, found {}",
        hop0_cpp.comments.len()
    );
    assert_c5_comment_fields(&hop0_cpp.comments).expect("C5 on initial C++");

    // Step 2: C++ -> ST
    let st_text_1 = st_fe
        .emit(&hop0_cpp, &plc_ctx)
        .expect("emit ST from C++")
        .value;
    assert!(!contains_banned_magic_comment(&st_text_1));
    let hop1_st = st_fe
        .parse(&st_text_1, &plc_ctx)
        .expect("reparse ST hop 1")
        .value;
    assert_c1_multiset_invariant(&hop0_cpp.comments, &hop1_st.comments)
        .expect("C1 multiset invariant violated on C++ -> ST");

    // Step 3: ST -> Ladder (intact STRUCTURED_POU preserves algorithmic code & comments)
    let lad_text = lad_fe
        .emit(&hop1_st, &plc_ctx)
        .expect("emit Ladder from ST")
        .value;
    assert!(!contains_banned_magic_comment(&lad_text));
    let hop2_lad = lad_fe
        .parse(&lad_text, &plc_ctx)
        .expect("reparse Ladder hop 2")
        .value;
    assert_c1_multiset_invariant(&hop0_cpp.comments, &hop2_lad.comments)
        .expect("C1 multiset invariant violated on ST -> Ladder");

    // Step 4: Ladder -> SFC (intact POU preserves code & comments verbatim)
    let sfc_text = sfc_fe
        .emit(&hop2_lad, &plc_ctx)
        .expect("emit SFC from Ladder")
        .value;
    assert!(!contains_banned_magic_comment(&sfc_text));
    let hop3_sfc = sfc_fe
        .parse(&sfc_text, &plc_ctx)
        .expect("reparse SFC hop 3")
        .value;
    assert_c1_multiset_invariant(&hop0_cpp.comments, &hop3_sfc.comments)
        .expect("C1 multiset invariant violated on Ladder -> SFC");

    // Step 5: SFC -> ST
    let st_text_2 = st_fe
        .emit(&hop3_sfc, &plc_ctx)
        .expect("emit ST from SFC")
        .value;
    assert!(!contains_banned_magic_comment(&st_text_2));
    let hop4_st = st_fe
        .parse(&st_text_2, &plc_ctx)
        .expect("reparse ST hop 4")
        .value;
    assert_c1_multiset_invariant(&hop0_cpp.comments, &hop4_st.comments)
        .expect("C1 multiset invariant violated on SFC -> ST");

    // Step 6: ST -> C++
    let cpp_text_2 = cpp_fe
        .emit(&hop4_st, &uno_ctx)
        .expect("emit C++ from ST")
        .value;
    assert!(!contains_banned_magic_comment(&cpp_text_2));
    let hop5_cpp = cpp_fe
        .parse(&cpp_text_2, &uno_ctx)
        .expect("reparse C++ hop 5")
        .value;
    assert_c1_multiset_invariant(&hop0_cpp.comments, &hop5_cpp.comments)
        .expect("C1 multiset invariant violated on ST -> C++");
    assert_c5_comment_fields(&hop5_cpp.comments).expect("C5 on final C++");
}

// ---------------------------------------------------------------------------
// Scenario 9: Gate 9 — Heterogeneous Multi-Board Co-Simulation Across All 4
//             Wave 1 Frontends (C++, ST, Ladder, GRAFCET/SFC) in SimKernel
// ---------------------------------------------------------------------------

#[test]
fn scenario_9_multi_board_heterogeneous_all_four_frontends_golden_equivalence() {
    let catalog = load_live_catalog();
    let cpp_fe = CppFrontend::new();
    let st_fe = StFrontend::new();
    let lad_fe = LadderFrontend::new();
    let sfc_fe = SfcFrontend::new();

    // Board 1 (5V Cooperative): Arduino Uno R3 authored in native Arduino C++
    // Reads sensor pushbutton on D2 and drives 5V interlock on D8.
    let uno_src = r#"#include <Arduino.h>

#define SENSOR_BTN 2
#define INTERLOCK_TX 8

void setup() {
  pinMode(SENSOR_BTN, INPUT);
  pinMode(INTERLOCK_TX, OUTPUT);
}

void loop() {
  if (digitalRead(SENSOR_BTN)) {
    digitalWrite(INTERLOCK_TX, HIGH);
  } else {
    digitalWrite(INTERLOCK_TX, LOW);
  }
}
"#;
    let uno_ctx = BoardContext::arduino_uno_r3("uno_sensor");
    let uno_bundle = cpp_fe
        .parse(uno_src, &uno_ctx)
        .expect("parse Uno C++")
        .value;

    // Board 2 (24V PlcScan, Variant A = ST, Variant B = Ladder):
    // Leader PLC latches Qa.0 on Start (Ia.0) until Stop (Ia.1), wired at 24V -> plc_follower.Ia.0.
    let plc_st_src = r#"VAR_GLOBAL
    StartCmd AT %IX0.0 : BOOL;
    StopCmd AT %IX0.1 : BOOL;
    RunPermissive AT %QX0.0 : BOOL;
END_VAR

PROGRAM OB_MAIN
    RunPermissive := (StartCmd OR RunPermissive) AND NOT StopCmd;
END_PROGRAM
"#;
    let plc_leader_ctx = BoardContext::plc_1200_open_1214("plc_leader");
    let plc_st_bundle = st_fe
        .parse(plc_st_src, &plc_leader_ctx)
        .expect("parse PLC leader ST")
        .value;

    let plc_lad_src = r#"VAR_GLOBAL
    StartCmd AT %IX0.0 : BOOL;
    StopCmd AT %IX0.1 : BOOL;
    RunPermissive AT %QX0.0 : BOOL;
END_VAR

NETWORK OB_MAIN
    RUNG 0 (n_mb_r0): BRANCH @(0, 0, n_mb_or) { | | StartCmd @(0, 0, n_mb_c0) || | | RunPermissive @(1, 0, n_mb_c1) } -- |/| StopCmd @(0, 1, n_mb_stop) -- ( ) RunPermissive @(0, 2, n_mb_lamp);
END_NETWORK
"#;
    let plc_lad_bundle = lad_fe
        .parse_and_validate(plc_lad_src, &plc_leader_ctx)
        .expect("parse PLC leader Ladder")
        .value;

    // Board 3 (24V PlcScan): Follower PLC authored in structural GRAFCET/SFC (`PouBody::Chart`).
    // Waits in S0_IDLE until 24V permissive from plc_leader arrives on %IX0.0 (Ia.0),
    // transitions to S1_ACTIVE (driving PilotLamp %QX0.0 with qualifier N), returns to S0_IDLE when permissive drops.
    let plc_sfc_src = r#"VAR_GLOBAL
    PermissiveIn AT %IX0.0 : BOOL;
    PilotLamp AT %QX0.0 : BOOL;
END_VAR

SFC_CHART OB_MAIN
    INITIAL_STEP S0_IDLE:
    END_STEP

    STEP S1_ACTIVE:
        N: PilotLamp;
    END_STEP

    TRANSITION T_START PRIORITY 0 FROM S0_IDLE TO S1_ACTIVE := PermissiveIn;
    END_TRANSITION

    TRANSITION T_STOP PRIORITY 0 FROM S1_ACTIVE TO S0_IDLE := NOT PermissiveIn;
    END_TRANSITION
END_SFC_CHART
"#;
    let plc_follower_ctx = BoardContext::plc_1200_open_1214("plc_follower");
    let plc_sfc_bundle = sfc_fe
        .parse(plc_sfc_src, &plc_follower_ctx)
        .expect("parse PLC follower SFC")
        .value;

    // 9a. Electrical safety contract check: directly wiring 5V Uno D8 to 24V PLC Ia.0 MUST fail
    // with `ErrUnsafeVoltageDomain` in `dendri-validate`.
    let unsafe_proj = Project {
        meta: ProjectMeta {
            schema_version: "2.0".to_owned(),
            asl_version: "0.1.0".to_owned(),
            requires: vec!["asl-core@1".to_owned()],
            name: "wave1_unsafe_voltage_domain".to_owned(),
            author: "DendriForge".to_owned(),
            description: "Negative voltage domain safety guard".to_owned(),
        },
        libraries: Vec::new(),
        packs: Vec::new(),
        comments: Vec::new(),
        boards: vec![
            make_uno_board("uno_sensor", &uno_bundle),
            make_plc_board("plc_leader", AuthoringLanguage::St, &plc_st_bundle),
        ],
        components: Vec::new(),
        wires: vec![Wire {
            id: "w_bad_5v_to_24v".to_owned(),
            from: "uno_sensor.D8".to_owned(),
            to: "plc_leader.Ia.0".to_owned(),
            signal: WireSignal::Digital,
            waypoints: Vec::new(),
            span: Span::default(),
        }],
    };
    let unsafe_bag = validate_project(&unsafe_proj, &catalog, &ValidateOptions::default());
    assert!(
        unsafe_bag
            .all()
            .iter()
            .any(|d| d.code == ErrorCode::ErrUnsafeVoltageDomain),
        "direct 5V Uno -> 24V PLC wire MUST trigger ErrUnsafeVoltageDomain"
    );

    // 9b. Valid heterogeneous 3-board project uniting ALL 4 Wave 1 frontends simultaneously:
    // - uno_sensor (5V Cooperative, Arduino C++)
    // - plc_leader (24V PlcScan, IEC ST in proj_st / Ladder in proj_lad)
    // - plc_follower (24V PlcScan, GRAFCET/SFC wired 24V <-> 24V from plc_leader.Qa.0 -> plc_follower.Ia.0)
    let build_three_board_project = |leader_board: Board| Project {
        meta: ProjectMeta {
            schema_version: "2.0".to_owned(),
            asl_version: "0.1.0".to_owned(),
            requires: vec!["asl-core@1".to_owned()],
            name: "wave1_multi_board_all_four_frontends".to_owned(),
            author: "DendriForge".to_owned(),
            description: "Gate 9 Multi-Board C++ + ST/Ladder + GRAFCET/SFC Equivalence".to_owned(),
        },
        libraries: Vec::new(),
        packs: Vec::new(),
        comments: Vec::new(),
        boards: vec![
            make_uno_board("uno_sensor", &uno_bundle),
            leader_board,
            make_plc_board("plc_follower", AuthoringLanguage::Sfc, &plc_sfc_bundle),
        ],
        components: Vec::new(),
        wires: vec![Wire {
            id: "w_24v_permissive".to_owned(),
            from: "plc_leader.Qa.0".to_owned(),
            to: "plc_follower.Ia.0".to_owned(),
            signal: WireSignal::Digital,
            waypoints: Vec::new(),
            span: Span::default(),
        }],
    };

    let proj_st = build_three_board_project(make_plc_board(
        "plc_leader",
        AuthoringLanguage::St,
        &plc_st_bundle,
    ));
    let proj_lad = build_three_board_project(make_plc_board(
        "plc_leader",
        AuthoringLanguage::Ladder,
        &plc_lad_bundle,
    ));

    let bag_st = validate_project(&proj_st, &catalog, &ValidateOptions::default());
    assert!(
        !bag_st.has_errors(),
        "3-board C++ + ST + SFC validation errors: {:#?}",
        bag_st.all()
    );
    let bag_lad = validate_project(&proj_lad, &catalog, &ValidateOptions::default());
    assert!(
        !bag_lad.has_errors(),
        "3-board C++ + Ladder + SFC validation errors: {:#?}",
        bag_lad.all()
    );

    // Deterministic timeline across all 3 heterogeneous boards:
    // t=0..15ms:  all inputs LOW -> (uno.D8=false, leader.Qa.0=false, follower.Qa.0=false)
    // t=20ms:     uno D2=HIGH, leader Ia.0=HIGH -> uno.D8=true, leader latches Qa.0=true -> wire -> follower enters S1 -> Qa.0=true
    // t=50ms:     uno D2=LOW,  leader Ia.0=LOW  -> uno.D8=false, leader stays latched Qa.0=true -> follower stays in S1 Qa.0=true
    // t=80ms:     leader Ia.1=HIGH (Stop) -> leader unlatches Qa.0=false -> wire -> follower returns to S0_IDLE (N auto-reset) -> Qa.0=false
    let sample_times_us: &[u64] = &[15_000, 45_000, 75_000, 120_000];
    let run_three_board = |proj: &Project| -> Vec<(bool, bool, bool)> {
        let mut timeline = InputTimeline::new();
        timeline.push_digital(0, "uno_sensor", "D2", ElectricalDigitalLevel::Low);
        timeline.push_digital(0, "plc_leader", "Ia.0", ElectricalDigitalLevel::Low);
        timeline.push_digital(0, "plc_leader", "Ia.1", ElectricalDigitalLevel::Low);
        timeline.push_digital(20_000, "uno_sensor", "D2", ElectricalDigitalLevel::High);
        timeline.push_digital(20_000, "plc_leader", "Ia.0", ElectricalDigitalLevel::High);
        timeline.push_digital(50_000, "uno_sensor", "D2", ElectricalDigitalLevel::Low);
        timeline.push_digital(50_000, "plc_leader", "Ia.0", ElectricalDigitalLevel::Low);
        timeline.push_digital(80_000, "plc_leader", "Ia.1", ElectricalDigitalLevel::High);
        timeline.push_digital(100_000, "plc_leader", "Ia.1", ElectricalDigitalLevel::Low);

        let mut kernel = SimKernel::new(proj, KernelConfig::default(), timeline)
            .expect("construct 3-board SimKernel");
        kernel.boot().expect("boot 3-board SimKernel");

        let mut trace = Vec::with_capacity(sample_times_us.len());
        for &t_us in sample_times_us {
            kernel.run_until(t_us).expect("advance 3-board kernel");
            let snap = kernel.snapshot();
            let uno_tx = snap
                .boards
                .get("uno_sensor")
                .and_then(|b| b.digital_outputs.get("D8"))
                .copied()
                .unwrap_or(false);
            let leader_out = snap
                .boards
                .get("plc_leader")
                .and_then(|b| b.digital_outputs.get("Qa.0"))
                .copied()
                .unwrap_or(false);
            let follower_lamp = snap
                .boards
                .get("plc_follower")
                .and_then(|b| b.digital_outputs.get("Qa.0"))
                .copied()
                .unwrap_or(false);
            trace.push((uno_tx, leader_out, follower_lamp));
        }
        trace
    };

    // Independent analytical oracle: (uno_sensor.D8, plc_leader.Qa.0, plc_follower.Qa.0)
    let expected_oracle = vec![
        (false, false, false),
        (true, true, true),
        (false, true, true),
        (false, false, false),
    ];

    let trace_st = run_three_board(&proj_st);
    let trace_lad = run_three_board(&proj_lad);

    assert_eq!(
        trace_st, expected_oracle,
        "3-board C++ + ST + SFC trace must match independent analytical oracle"
    );
    assert_eq!(
        trace_lad, expected_oracle,
        "3-board C++ + Ladder + SFC trace must match independent analytical oracle"
    );
    assert_eq!(
        trace_st, trace_lad,
        "swapping plc_leader ST <-> Ladder must produce bit-identical multi-board traces"
    );
}
