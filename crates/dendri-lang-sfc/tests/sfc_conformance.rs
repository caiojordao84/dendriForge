//! Comprehensive conformance, property-based round-trip, VM differential,
//! comment conservation (`C1–C5`), `Foreign`, and negative-fixture test suite
//! for `dendri-lang-sfc` (`docs/lbc/sfc.md`, `04-frontends-visual-and-sync.md` §5.2).

use dendri_ast::{
    ActionQualifier, AuthoringLanguage, BinaryOp, Board, BoardLayout, BoardPresentation,
    BoardProgram, ChannelRange, Comment, CommentAnchor, CommentEncoding, CommentOrigin,
    CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, EolStyle, ExecutionKind, Expr,
    ExprKind, ForeignFidelity, HardwareBinding, HardwareKind, LValue, Literal, NodeId, ObKind, Pou,
    PouBody, PouKind, PullMode, ScalarType, SfcAction, SfcChart, SfcStep, SfcTransition,
    SignalKind, Stmt, StmtKind, StorageScope, TypeDecl, TypeRef,
};
use dendri_frontend_core::conformance::{
    FeatureMatrixCollector, assert_c1_multiset_invariant, assert_c2_orphan_preservation,
    assert_c4_reimport_idempotence, assert_c5_comment_fields, assert_lossless_asl,
    contains_banned_magic_comment, verify_frontend_roundtrip, verify_hostile_corpus_on_frontend,
};
use dendri_frontend_core::foreign::is_four_part_diagnostic;
use dendri_frontend_core::traits::{
    BoardContext, BoardProgramBundle, LanguageFrontend, LanguageId, Paradigm,
};
use dendri_lang_sfc::{
    MAX_STABILITY_ITERS, SfcFrontend, feature_matrix_entries, project_chart_state_decls,
    project_chart_to_statements,
};
use dendri_span::{ErrorCode, Span};
use dendri_vm::{BoardVm, MemoryBoardIo, RuntimeError, Value, VmConfig};
use proptest::prelude::*;

fn plc_ctx() -> BoardContext {
    BoardContext::plc_1200_open_1214("plc_main")
}

fn board_from_program(program: BoardProgram) -> Board {
    Board {
        id: "plc_main".to_owned(),
        profile: "plc-1200-open-1214".to_owned(),
        authoring_language: AuthoringLanguage::Sfc,
        execution: ExecutionKind::PlcScan,
        scan_period_us: Some(10_000),
        layout: BoardLayout::default(),
        program,
        comments: Vec::new(),
        assumptions: Vec::new(),
        presentation: BoardPresentation::default(),
        span: Span::default(),
    }
}

/// Builds a companion `Board` where the target SFC POU is replaced by its derived
/// `project_chart_to_statements` + `project_chart_state_decls` statement representation
/// so we can compare `dendri-vm` execution traces bit-for-bit!
fn projected_board_from_chart_program(program: &BoardProgram, pou_name: &str) -> Board {
    let mut cloned = program.clone();
    let mut added_decls = Vec::new();
    for pou in &mut cloned.pous {
        if pou.name == pou_name
            && let PouBody::Chart(chart) = &pou.body
        {
            added_decls.extend(project_chart_state_decls(chart));
            let stmts = project_chart_to_statements(chart);
            pou.body = PouBody::Statements(stmts);
        }
    }
    cloned.data.extend(added_decls);
    board_from_program(cloned)
}

fn contains_synthetic_sfc_mangling(program: &BoardProgram) -> bool {
    let dbg = format!("{program:#?}");
    dbg.contains("__sfc_")
}

#[test]
fn frontend_identity_and_capabilities() {
    let fe = SfcFrontend::new();
    assert_eq!(fe.id(), LanguageId::Sfc);
    assert_eq!(fe.paradigm(), Paradigm::Grafcet);

    let caps = fe.capabilities();
    assert_eq!(
        caps.supported_comment_styles,
        vec![
            CommentStyle::Visual,
            CommentStyle::Line,
            CommentStyle::ParenBlock,
        ]
    );
    assert!(caps.supports_nested_paren_comments);
    assert!(caps.supports_line_comments);
    assert_eq!(caps.comment_encoding, CommentEncoding::Utf8Passthrough);
    assert_eq!(caps.capabilities, vec!["asl-core@1".to_owned()]);
    assert_eq!(caps.st_dialect, None);

    let fe_ascii = SfcFrontend::with_comment_encoding(CommentEncoding::AsciiFold);
    assert_eq!(
        fe_ascii.capabilities().comment_encoding,
        CommentEncoding::AsciiFold
    );
}

#[test]
fn canonical_and_alternate_textual_sfc_syntax_parsing() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    // Exercises CHART..END_CHART, EVOLVE_TO_STABILITY, MACRO_STEP, PRIORITY n,
    // verbose ACTION..END_ACTION, shorthand N(target), single-quoted IEC strings,
    // IEC radix literals, and CRLF line-ending preservation.
    let src = "VAR_GLOBAL\r\n\
    StartBtn AT %IX0.0 : BOOL;\r\n\
    ValveA AT %QX0.0 : BOOL;\r\n\
    Alarm AT %QX0.1 : BOOL;\r\n\
    count : DINT := 16#10;\r\n\
    mask : DINT := 2#1010_0101;\r\n\
    tag : STRING[32] := 'plc_span_27648';\r\n\
END_VAR\r\n\
\r\n\
CHART SubRoutine (KIND := FB)\r\n\
    INITIAL_STEP Entry:\r\n\
        N(ValveA);\r\n\
    END_STEP\r\n\
    STEP Exit:\r\n\
        ACTION DoneAct (P1): { count := count + 1; } END_ACTION\r\n\
    END_STEP\r\n\
    TRANSITION T_sub FROM Entry TO Exit := TRUE;\r\n\
    END_TRANSITION\r\n\
END_CHART\r\n\
\r\n\
CHART MainSeq\r\n\
    EVOLVE_TO_STABILITY;\r\n\
    INITIAL_STEP Idle:\r\n\
        L(T#250ms, Alarm);\r\n\
    END_STEP\r\n\
    MACRO_STEP RunSub (SubRoutine):\r\n\
    END_STEP\r\n\
    TRANSITION T_go PRIORITY 2 FROM Idle TO RunSub := StartBtn;\r\n\
    END_TRANSITION\r\n\
    TRANSITION T_ret FROM RunSub TO Idle := RunSub.DONE;\r\n\
    END_TRANSITION\r\n\
END_CHART\r\n";

    let parsed = fe.parse(src, &ctx).expect("parse alternate SFC syntax");
    assert_eq!(parsed.value.presentation.eol_style, EolStyle::Crlf);
    assert!(!contains_synthetic_sfc_mangling(&parsed.value.program));
    assert_eq!(parsed.value.program.hardware.len(), 3);
    assert_eq!(parsed.value.program.hardware[0].pin, "Ia.0");
    assert_eq!(parsed.value.program.hardware[1].pin, "Qa.0");
    assert_eq!(parsed.value.program.pous.len(), 2);

    let PouBody::Chart(main_chart) = &parsed.value.program.pous[1].body else {
        panic!("expected PouBody::Chart");
    };
    assert!(main_chart.evolve_to_stability);
    assert_eq!(
        main_chart.steps[1].macro_chart.as_deref(),
        Some("SubRoutine")
    );
    assert_eq!(main_chart.transitions[0].priority, 2);

    // Emit preserves CRLF and round-trips losslessly
    let emitted = fe.emit(&parsed.value, &ctx).expect("emit canonical SFC");
    assert!(emitted.value.contains("\r\n"));
    assert!(!contains_banned_magic_comment(&emitted.value));

    let _reparsed = verify_frontend_roundtrip(&fe, &parsed.value, &ctx)
        .expect("LOSSLESS_ASL roundtrip after alternate parse");
}

#[test]
fn all_eight_table47_action_qualifiers_roundtrip_and_vm_projection_match() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    let src = r#"
VAR_GLOBAL
    q_n : BOOL := FALSE;
    q_s : BOOL := FALSE;
    q_l : BOOL := FALSE;
    q_d : BOOL := FALSE;
    q_p : BOOL := FALSE;
    q_p1 : BOOL := FALSE;
    q_p0 : BOOL := FALSE;
    p_cnt : DINT := 0;
    p1_cnt : DINT := 0;
    p0_cnt : DINT := 0;
END_VAR

SFC_CHART QualChart
    INITIAL_STEP S0:
        N: q_n;
        S: q_s;
        L(T#20ms): q_l;
        D(T#20ms): q_d;
        P: q_p { p_cnt := p_cnt + 1; }
        P1: q_p1 { p1_cnt := p1_cnt + 1; }
        P0: q_p0 { p0_cnt := p0_cnt + 1; }
    END_STEP
    STEP S1:
        N: q_n;
        R: q_n;
        R: q_s;
    END_STEP
    TRANSITION T0 FROM S0 TO S1 := S0.T >= T#30ms;
    END_TRANSITION
END_SFC_CHART
"#;

    let parsed = fe.parse(src, &ctx).expect("parse all 8 qualifiers");
    assert!(!contains_synthetic_sfc_mangling(&parsed.value.program));
    let _rt = verify_frontend_roundtrip(&fe, &parsed.value, &ctx)
        .expect("all 8 qualifiers must round-trip losslessly");

    // Verify native dendri-vm AND projected statements produce identical state on every scan!
    let native_board = board_from_program(parsed.value.program.clone());
    let proj_board = projected_board_from_chart_program(&parsed.value.program, "QualChart");

    let mut native_vm = BoardVm::new(&native_board, VmConfig::default()).expect("native vm");
    let mut proj_vm = BoardVm::new(&proj_board, VmConfig::default()).expect("projected vm");
    let mut native_io = MemoryBoardIo::default();
    let mut proj_io = MemoryBoardIo::default();

    let tracked = [
        "q_n", "q_s", "q_l", "q_d", "q_p", "q_p1", "q_p0", "p_cnt", "p1_cnt", "p0_cnt",
    ];

    // Scan 1 (t=0ms): S0 active -> N=true, S=true, L=true, D=false, P=1, P1=1, P0=0
    native_vm.run_plc_scan(10_000, &mut native_io).unwrap();
    proj_vm.run_plc_scan(10_000, &mut proj_io).unwrap();
    assert_eq!(native_vm.get_global("q_n"), Some(&Value::Bool(true)));
    assert_eq!(native_vm.get_global("q_s"), Some(&Value::Bool(true)));
    assert_eq!(native_vm.get_global("q_l"), Some(&Value::Bool(true)));
    assert_eq!(native_vm.get_global("q_d"), Some(&Value::Bool(false)));
    assert_eq!(native_vm.get_global("p_cnt"), Some(&Value::Dint(1)));
    assert_eq!(native_vm.get_global("p1_cnt"), Some(&Value::Dint(1)));
    assert_eq!(native_vm.get_global("p0_cnt"), Some(&Value::Dint(0)));
    for var in tracked {
        assert_eq!(
            native_vm.get_global(var),
            proj_vm.get_global(var),
            "scan 1 mismatch on `{var}`"
        );
    }

    // Scan 2 (t=10ms): S0 still active -> L still true, D still false, P/P1 do NOT re-fire!
    native_vm.run_plc_scan(10_000, &mut native_io).unwrap();
    proj_vm.run_plc_scan(10_000, &mut proj_io).unwrap();
    assert_eq!(native_vm.get_global("q_l"), Some(&Value::Bool(true)));
    assert_eq!(native_vm.get_global("q_d"), Some(&Value::Bool(false)));
    assert_eq!(native_vm.get_global("p_cnt"), Some(&Value::Dint(1)));
    assert_eq!(native_vm.get_global("p1_cnt"), Some(&Value::Dint(1)));
    for var in tracked {
        assert_eq!(
            native_vm.get_global(var),
            proj_vm.get_global(var),
            "scan 2 mismatch on `{var}`"
        );
    }

    // Scan 3 (t=20ms): L expires -> auto-resets FALSE; D triggers -> TRUE!
    native_vm.run_plc_scan(10_000, &mut native_io).unwrap();
    proj_vm.run_plc_scan(10_000, &mut proj_io).unwrap();
    assert_eq!(native_vm.get_global("q_l"), Some(&Value::Bool(false)));
    assert_eq!(native_vm.get_global("q_d"), Some(&Value::Bool(true)));
    for var in tracked {
        assert_eq!(
            native_vm.get_global(var),
            proj_vm.get_global(var),
            "scan 3 mismatch on `{var}`"
        );
    }

    // Scan 4 (t=30ms): T0 fires -> S0 exits (P0 fires once, D auto-resets FALSE);
    // S1 enters -> R overrides BOTH N (q_n) AND S (q_s) on the SAME scan!
    native_vm.run_plc_scan(10_000, &mut native_io).unwrap();
    proj_vm.run_plc_scan(10_000, &mut proj_io).unwrap();
    assert_eq!(native_vm.get_global("q_n"), Some(&Value::Bool(false)));
    assert_eq!(native_vm.get_global("q_s"), Some(&Value::Bool(false)));
    assert_eq!(native_vm.get_global("q_d"), Some(&Value::Bool(false)));
    assert_eq!(native_vm.get_global("q_p0"), Some(&Value::Bool(true)));
    assert_eq!(native_vm.get_global("p0_cnt"), Some(&Value::Dint(1)));
    for var in tracked {
        assert_eq!(
            native_vm.get_global(var),
            proj_vm.get_global(var),
            "scan 4 mismatch on `{var}`"
        );
    }

    // Scan 5: S1 remains active, P0 does NOT re-fire
    native_vm.run_plc_scan(10_000, &mut native_io).unwrap();
    proj_vm.run_plc_scan(10_000, &mut proj_io).unwrap();
    assert_eq!(native_vm.get_global("p0_cnt"), Some(&Value::Dint(1)));
    for var in tracked {
        assert_eq!(
            native_vm.get_global(var),
            proj_vm.get_global(var),
            "scan 5 mismatch on `{var}`"
        );
    }
}

#[test]
fn rule5_simultaneous_deactivate_activate_and_frozen_receptivity_vm_equivalence() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    // S0 and S1 BOTH start active!
    // T_a: S0 -> S1 (receptivity TRUE)
    // T_b: S1 -> S2 (receptivity S0.X — MUST see FROZEN S0.X == TRUE even though T_a clears S0!)
    // Rule 5: Phase 2a clears S0 and S1; Phase 2b activates S1 and S2 -> S1 MUST REMAIN ACTIVE!
    // Then T_self: S2 -> S2 keeps S2 ACTIVE on subsequent scans without re-firing P1!
    let src = r#"
VAR_GLOBAL
    s1_lamp : BOOL := FALSE;
    s2_lamp : BOOL := FALSE;
    s2_enter_cnt : DINT := 0;
    go : BOOL := TRUE;
END_VAR

SFC_CHART Rule5Chart
    INITIAL_STEP S0:
    END_STEP
    INITIAL_STEP S1:
        N: s1_lamp;
    END_STEP
    STEP S2:
        N: s2_lamp;
        P1: { s2_enter_cnt := s2_enter_cnt + 1; }
    END_STEP
    TRANSITION T_a FROM S0 TO S1 := go;
    END_TRANSITION
    TRANSITION T_b FROM S1 TO S2 := S0.X;
    END_TRANSITION
    TRANSITION T_self FROM S2 TO S2 := S2.X;
    END_TRANSITION
END_SFC_CHART
"#;

    let parsed = fe.parse(src, &ctx).expect("parse Rule 5 chart");
    let _rt = verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("roundtrip Rule 5");

    let native_board = board_from_program(parsed.value.program.clone());
    let proj_board = projected_board_from_chart_program(&parsed.value.program, "Rule5Chart");

    let mut native_vm = BoardVm::new(&native_board, VmConfig::default()).unwrap();
    let mut proj_vm = BoardVm::new(&proj_board, VmConfig::default()).unwrap();
    let mut native_io = MemoryBoardIo::default();
    let mut proj_io = MemoryBoardIo::default();

    // Scan 1: S0 & S1 initialize active -> T_a (S0->S1) AND T_b (S1->S2, checking frozen S0.X) BOTH fire!
    // Rule 5 keeps S1 ACTIVE and activates S2!
    native_vm.run_plc_scan(10_000, &mut native_io).unwrap();
    proj_vm.run_plc_scan(10_000, &mut proj_io).unwrap();

    let active1 = native_vm.active_sfc_steps("Rule5Chart").unwrap();
    assert!(!active1.contains("S0"));
    assert!(
        active1.contains("S1"),
        "Rule 5 violated: S1 simultaneously cleared and activated must remain ACTIVE!"
    );
    assert!(
        active1.contains("S2"),
        "Frozen Phase 1 violated: T_b must observe frozen S0.X == true!"
    );
    assert_eq!(native_vm.get_global("s1_lamp"), Some(&Value::Bool(true)));
    assert_eq!(native_vm.get_global("s2_lamp"), Some(&Value::Bool(true)));
    assert_eq!(native_vm.get_global("s2_enter_cnt"), Some(&Value::Dint(1)));

    assert_eq!(
        proj_vm.get_global("sfc_step_S0_x"),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        proj_vm.get_global("sfc_step_S1_x"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        proj_vm.get_global("sfc_step_S2_x"),
        Some(&Value::Bool(true))
    );
    assert_eq!(proj_vm.get_global("s1_lamp"), Some(&Value::Bool(true)));
    assert_eq!(proj_vm.get_global("s2_lamp"), Some(&Value::Bool(true)));
    assert_eq!(proj_vm.get_global("s2_enter_cnt"), Some(&Value::Dint(1)));

    // Scan 2: T_self fires S2 -> S2; S2 stays ACTIVE and P1 does NOT re-fire!
    native_vm.run_plc_scan(10_000, &mut native_io).unwrap();
    proj_vm.run_plc_scan(10_000, &mut proj_io).unwrap();
    assert!(
        native_vm
            .active_sfc_steps("Rule5Chart")
            .unwrap()
            .contains("S2")
    );
    assert_eq!(
        proj_vm.get_global("sfc_step_S2_x"),
        Some(&Value::Bool(true))
    );
    assert_eq!(native_vm.get_global("s2_enter_cnt"), Some(&Value::Dint(1)));
    assert_eq!(proj_vm.get_global("s2_enter_cnt"), Some(&Value::Dint(1)));
}

#[test]
fn or_divergence_priority_and_and_divergence_convergence_roundtrip_and_vm() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    let src = r#"
VAR_GLOBAL
    branch_low : BOOL := FALSE;
    branch_high : BOOL := FALSE;
    par_a : BOOL := FALSE;
    par_b : BOOL := FALSE;
    merged : BOOL := FALSE;
END_VAR

SFC_CHART OrAndChart
    INITIAL_STEP Start:
    END_STEP
    STEP LowPrio:
        N: branch_low;
    END_STEP
    STEP HighPrio:
        N: branch_high;
    END_STEP
    STEP ParA:
        N: par_a;
    END_STEP
    STEP ParB:
        N: par_b;
    END_STEP
    STEP JoinDone:
        N: merged;
    END_STEP

    // Declare LOWER priority (5) BEFORE HIGHER priority (1) to prove ascending priority sorting!
    TRANSITION T_low (PRIORITY := 5) FROM Start TO LowPrio := Start.T >= T#10ms;
    END_TRANSITION
    TRANSITION T_high (PRIORITY := 1) FROM Start TO HighPrio := Start.T >= T#10ms;
    END_TRANSITION

    // AND divergence: HighPrio -> (ParA, ParB)
    TRANSITION T_div FROM HighPrio TO (ParA, ParB) := HighPrio.T >= T#10ms;
    END_TRANSITION

    // AND convergence: (ParA, ParB) -> JoinDone
    TRANSITION T_conv FROM (ParA, ParB) TO JoinDone := ParA.X AND ParB.X AND ParA.T >= T#10ms;
    END_TRANSITION
END_SFC_CHART
"#;

    let parsed = fe.parse(src, &ctx).expect("parse OR/AND chart");
    let _rt = verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("roundtrip OR/AND");

    let native_board = board_from_program(parsed.value.program.clone());
    let proj_board = projected_board_from_chart_program(&parsed.value.program, "OrAndChart");

    let mut native_vm = BoardVm::new(&native_board, VmConfig::default()).unwrap();
    let mut proj_vm = BoardVm::new(&proj_board, VmConfig::default()).unwrap();
    let mut native_io = MemoryBoardIo::default();
    let mut proj_io = MemoryBoardIo::default();

    let vars = ["branch_low", "branch_high", "par_a", "par_b", "merged"];

    for scan in 1..=5 {
        native_vm.run_plc_scan(10_000, &mut native_io).unwrap();
        proj_vm.run_plc_scan(10_000, &mut proj_io).unwrap();
        for v in vars {
            assert_eq!(
                native_vm.get_global(v),
                proj_vm.get_global(v),
                "OR/AND mismatch at scan {scan} on `{v}`"
            );
        }
        // LowPrio must NEVER activate!
        assert_eq!(
            native_vm.get_global("branch_low"),
            Some(&Value::Bool(false))
        );
    }

    // By scan 5, JoinDone is active and ParA/ParB have auto-reset!
    assert_eq!(native_vm.get_global("merged"), Some(&Value::Bool(true)));
    assert_eq!(native_vm.get_global("par_a"), Some(&Value::Bool(false)));
    assert_eq!(native_vm.get_global("par_b"), Some(&Value::Bool(false)));
}

#[test]
fn native_step_attributes_and_macro_step_entry_exit_roundtrip() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    // Initial macro-step `M0` initializes sub-chart on Scan 1 (`04` §5.2),
    // advances when `M0.DONE` becomes true, then cycles back to `M0` (re-entry reset!).
    let src = r#"
VAR_GLOBAL
    sub_entry_count : DINT := 0;
    sub_exit_lamp : BOOL := FALSE;
    parent_done_count : DINT := 0;
END_VAR

SFC_CHART SubCycle (KIND := FB)
    INITIAL_STEP SubEntry:
        P1: { sub_entry_count := sub_entry_count + 1; }
    END_STEP
    STEP SubExit:
        N: sub_exit_lamp;
    END_STEP
    TRANSITION T_sub_step FROM SubEntry TO SubExit := SubEntry.T >= T#20ms;
    END_TRANSITION
END_SFC_CHART

SFC_CHART ParentChart
    INITIAL_STEP M0 MACRO SubCycle:
    END_STEP
    STEP Done:
        P1: { parent_done_count := parent_done_count + 1; }
    END_STEP
    TRANSITION T_leave_macro FROM M0 TO Done := M0.DONE;
    END_TRANSITION
    TRANSITION T_reenter_macro FROM Done TO M0 := Done.T >= T#10ms;
    END_TRANSITION
END_SFC_CHART
"#;

    let parsed1 = fe.parse(src, &ctx).expect("parse macro-step chart");
    assert!(!contains_synthetic_sfc_mangling(&parsed1.value.program));

    // Verify zero duplicate setup statements across 3 full emit -> parse round-trips!
    let rt1 = verify_frontend_roundtrip(&fe, &parsed1.value, &ctx).expect("roundtrip 1");
    let rt2 = verify_frontend_roundtrip(&fe, &rt1, &ctx).expect("roundtrip 2");
    let rt3 = verify_frontend_roundtrip(&fe, &rt2, &ctx).expect("roundtrip 3");
    assert_lossless_asl(&parsed1.value.program, &rt3.program).expect("3x macro roundtrip exact");

    // Verify exact VM execution of Scan-1 initial macro-step, <Macro>.DONE, and re-entry reset!
    let board = board_from_program(rt3.program);
    let mut vm = BoardVm::new(&board, VmConfig::default()).unwrap();
    let mut io = MemoryBoardIo::default();

    // Scan 1: M0 is initial -> SubCycle resets & initializes SubEntry on Scan 1 -> sub_entry_count = 1
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(vm.active_sfc_steps("ParentChart").unwrap().contains("M0"));
    assert!(
        vm.active_sfc_steps("SubCycle")
            .unwrap()
            .contains("SubEntry")
    );
    assert_eq!(vm.get_global("sub_entry_count"), Some(&Value::Dint(1)));
    assert_eq!(vm.get_global("parent_done_count"), Some(&Value::Dint(0)));

    // Scan 2: SubCycle advances SubEntry -> SubExit (M0.DONE becomes true AFTER SubCycle ticks)
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(vm.active_sfc_steps("SubCycle").unwrap().contains("SubExit"));
    assert_eq!(vm.get_global("sub_exit_lamp"), Some(&Value::Bool(true)));

    // Scan 3: Parent evaluates M0.DONE == true -> transitions M0 -> Done!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(vm.active_sfc_steps("ParentChart").unwrap().contains("Done"));
    assert_eq!(vm.get_global("parent_done_count"), Some(&Value::Dint(1)));

    // Scan 4: Done.T == 10ms -> transitions Done -> M0 (re-entry cleanly resets SubCycle to SubEntry!)
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(vm.active_sfc_steps("ParentChart").unwrap().contains("M0"));
    assert!(
        vm.active_sfc_steps("SubCycle")
            .unwrap()
            .contains("SubEntry")
    );
    assert_eq!(
        vm.get_global("sub_entry_count"),
        Some(&Value::Dint(2)),
        "macro re-entry must cleanly reset sub-chart and fire SubEntry P1 again!"
    );
}

#[test]
fn evolve_to_stability_roundtrip_and_projection_bound() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    // 3-step chain that collapses S0 -> S1 -> S2 in ONE scan when STABILITY := TRUE
    let chain_src = r#"
VAR_GLOBAL
    final_out : BOOL := FALSE;
END_VAR

SFC_CHART FastChain (STABILITY := TRUE)
    INITIAL_STEP S0:
    END_STEP
    STEP S1:
    END_STEP
    STEP S2:
        N: final_out;
    END_STEP
    TRANSITION T0 FROM S0 TO S1 := TRUE;
    END_TRANSITION
    TRANSITION T1 FROM S1 TO S2 := TRUE;
    END_TRANSITION
END_SFC_CHART
"#;
    let parsed = fe.parse(chain_src, &ctx).expect("parse stability chain");
    let _rt = verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("roundtrip stability");

    let native_board = board_from_program(parsed.value.program.clone());
    let proj_board = projected_board_from_chart_program(&parsed.value.program, "FastChain");
    let mut native_vm = BoardVm::new(&native_board, VmConfig::default()).unwrap();
    let mut proj_vm = BoardVm::new(&proj_board, VmConfig::default()).unwrap();
    let mut io1 = MemoryBoardIo::default();
    let mut io2 = MemoryBoardIo::default();

    native_vm.run_plc_scan(10_000, &mut io1).unwrap();
    proj_vm.run_plc_scan(10_000, &mut io2).unwrap();
    assert!(
        native_vm
            .active_sfc_steps("FastChain")
            .unwrap()
            .contains("S2")
    );
    assert_eq!(native_vm.get_global("final_out"), Some(&Value::Bool(true)));
    assert_eq!(proj_vm.get_global("final_out"), Some(&Value::Bool(true)));
    assert_eq!(
        proj_vm.get_global("sfc_step_S2_x"),
        Some(&Value::Bool(true))
    );

    // Unconditional zero-time cycle: native VM trips SfcInstabilityLoop, projection halts at MAX_STABILITY_ITERS!
    let cycle_src = r#"
SFC_CHART CycleGuard (STABILITY := TRUE)
    INITIAL_STEP A:
    END_STEP
    STEP B:
    END_STEP
    TRANSITION T0 FROM A TO B := TRUE;
    END_TRANSITION
    TRANSITION T1 FROM B TO A := TRUE;
    END_TRANSITION
END_SFC_CHART
"#;
    let cyc = fe.parse(cycle_src, &ctx).expect("parse cycle chart");
    let native_cyc = board_from_program(cyc.value.program.clone());
    let proj_cyc = projected_board_from_chart_program(&cyc.value.program, "CycleGuard");

    let mut native_cyc_vm = BoardVm::new(&native_cyc, VmConfig::default()).unwrap();
    assert!(matches!(
        native_cyc_vm.run_plc_scan(10_000, &mut io1),
        Err(RuntimeError::SfcInstabilityLoop { .. })
    ));

    let mut proj_cyc_vm = BoardVm::new(&proj_cyc, VmConfig::default()).unwrap();
    proj_cyc_vm
        .run_plc_scan(10_000, &mut io2)
        .expect("bounded projection must terminate cleanly without hanging");
    assert_eq!(
        proj_cyc_vm.get_global("sfc_stab_iter"),
        Some(&Value::Dint(i32::try_from(MAX_STABILITY_ITERS).unwrap()))
    );
}

#[test]
fn projection_matches_dendri_vm_native_sfc_bit_for_bit() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    let src = r#"
VAR_GLOBAL
    pump : BOOL := FALSE;
    heater : BOOL := FALSE;
    alarm_latch : BOOL := FALSE;
    pulse_log : DINT := 0;
    cycles : DINT := 0;
END_VAR

SFC_CHART BatchProcess
    INITIAL_STEP Idle:
        N: pump;
    END_STEP
    STEP HeatA:
        N: pump;
        D(T#10ms): heater;
        S: alarm_latch;
    END_STEP
    STEP FillB:
        L(T#20ms): heater;
        P: { pulse_log := pulse_log + 1; }
    END_STEP
    STEP Drain:
        R: alarm_latch;
        P0: { cycles := cycles + 1; }
    END_STEP

    TRANSITION T_start (PRIORITY := 1) FROM Idle TO (HeatA, FillB) := Idle.T >= T#10ms;
    END_TRANSITION
    TRANSITION T_join FROM (HeatA, FillB) TO Drain := HeatA.T >= T#20ms AND FillB.T >= T#20ms;
    END_TRANSITION
    TRANSITION T_restart FROM Drain TO Idle := Drain.T >= T#10ms;
    END_TRANSITION
END_SFC_CHART
"#;

    let parsed = fe.parse(src, &ctx).expect("parse batch process");
    let native_board = board_from_program(parsed.value.program.clone());
    let proj_board = projected_board_from_chart_program(&parsed.value.program, "BatchProcess");

    let mut native_vm = BoardVm::new(&native_board, VmConfig::default()).unwrap();
    let mut proj_vm = BoardVm::new(&proj_board, VmConfig::default()).unwrap();
    let mut io1 = MemoryBoardIo::default();
    let mut io2 = MemoryBoardIo::default();

    let vars = ["pump", "heater", "alarm_latch", "pulse_log", "cycles"];
    for scan in 1..=12 {
        native_vm.run_plc_scan(10_000, &mut io1).unwrap();
        proj_vm.run_plc_scan(10_000, &mut io2).unwrap();
        for v in vars {
            assert_eq!(
                native_vm.get_global(v),
                proj_vm.get_global(v),
                "differential mismatch at scan {scan} for `{v}`"
            );
        }
    }
    assert!(matches!(
        native_vm.get_global("cycles"),
        Some(&Value::Dint(c)) if c >= 1
    ));
}

#[test]
fn electro_pneumatic_a_plus_b_plus_a_minus_b_minus_roundtrip_and_vm() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    // Industrial A+ B+ A- B- electro-pneumatic sequence driving two 5/2 valves with limit switches
    // and a 4-20 mA supply air pressure transmitter on %IW64!
    let src = r#"
// Electro-pneumatic A+ B+ A- B- sequence on plc-1200-open-1214
VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    LimA0 AT %IX0.1 : BOOL;
    LimA1 AT %IX0.2 : BOOL;
    LimB0 AT %IX0.3 : BOOL;
    LimB1 AT %IX0.4 : BOOL;
    ValveAPlus AT %QX0.0 : BOOL;
    ValveAMinus AT %QX0.1 : BOOL;
    ValveBPlus AT %QX0.2 : BOOL;
    ValveBMinus AT %QX0.3 : BOOL;
    AirPress AT %IW64 : CODE<plc_span_27648> (RANGE := CURRENT(4.0 .. 20.0, LIVE_ZERO));
END_VAR

VAR_GLOBAL
    completed_cycles : DINT := 0;
END_VAR

SFC_CHART PneumaticAB
    // Home position: both cylinders retracted
    INITIAL_STEP S0_Home:
    END_STEP

    // A+ : extend cylinder A
    STEP S1_APlus:
        N: ValveAPlus;
    END_STEP

    // B+ : extend cylinder B
    STEP S2_BPlus:
        N: ValveBPlus;
    END_STEP

    // A- : retract cylinder A
    STEP S3_AMinus:
        N: ValveAMinus;
    END_STEP

    // B- : retract cylinder B
    STEP S4_BMinus:
        N: ValveBMinus;
        P0: { completed_cycles := completed_cycles + 1; }
    END_STEP

    TRANSITION T0 FROM S0_Home TO S1_APlus := StartBtn AND LimA0 AND LimB0;
    END_TRANSITION
    TRANSITION T1 FROM S1_APlus TO S2_BPlus := LimA1;
    END_TRANSITION
    TRANSITION T2 FROM S2_BPlus TO S3_AMinus := LimB1;
    END_TRANSITION
    TRANSITION T3 FROM S3_AMinus TO S4_BMinus := LimA0;
    END_TRANSITION
    TRANSITION T4 FROM S4_BMinus TO S0_Home := LimB0;
    END_TRANSITION
END_SFC_CHART
"#;

    let parsed = fe.parse(src, &ctx).expect("parse A+ B+ A- B- chart");
    assert!(!contains_synthetic_sfc_mangling(&parsed.value.program));
    assert_eq!(parsed.value.program.hardware.len(), 10);
    assert_eq!(parsed.value.program.hardware[0].pin, "Ia.0");
    assert_eq!(parsed.value.program.hardware[5].pin, "Qa.0");
    assert_eq!(parsed.value.program.hardware[9].pin, "IW64");
    assert_eq!(
        parsed.value.program.hardware[9].range,
        Some(ChannelRange {
            signal: SignalKind::Current,
            low: 4.0,
            high: 20.0,
            live_zero: true,
            hardware_normalizes_live_zero: false,
        })
    );

    let _rt = verify_frontend_roundtrip(&fe, &parsed.value, &ctx)
        .expect("A+ B+ A- B- LOSSLESS_ASL roundtrip");

    // Simulate full A+ -> B+ -> A- -> B- physical limit-switch progression in dendri-vm!
    let board = board_from_program(parsed.value.program);
    let mut vm = BoardVm::new(&board, VmConfig::default()).unwrap();
    let mut io = MemoryBoardIo::default();

    // Initial state: retracted (A0=true, B0=true), StartBtn=false -> stays in S0_Home
    io.set_digital_in("Ia.0", false);
    io.set_digital_in("Ia.1", true);
    io.set_digital_in("Ia.2", false);
    io.set_digital_in("Ia.3", true);
    io.set_digital_in("Ia.4", false);
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(
        vm.active_sfc_steps("PneumaticAB")
            .unwrap()
            .contains("S0_Home")
    );

    // Press StartBtn -> S1_APlus (Qa.0 = true)
    io.set_digital_in("Ia.0", true);
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(
        vm.active_sfc_steps("PneumaticAB")
            .unwrap()
            .contains("S1_APlus")
    );
    assert_eq!(io.digital_outputs.get("Qa.0"), Some(&true));

    // Cylinder A reaches A1 -> S2_BPlus (Qa.0 auto-resets false, Qa.2 = true)
    io.set_digital_in("Ia.0", false);
    io.set_digital_in("Ia.1", false);
    io.set_digital_in("Ia.2", true);
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(
        vm.active_sfc_steps("PneumaticAB")
            .unwrap()
            .contains("S2_BPlus")
    );
    assert_eq!(io.digital_outputs.get("Qa.0"), Some(&false));
    assert_eq!(io.digital_outputs.get("Qa.2"), Some(&true));

    // Cylinder B reaches B1 -> S3_AMinus (Qa.2 = false, Qa.1 = true)
    io.set_digital_in("Ia.3", false);
    io.set_digital_in("Ia.4", true);
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(
        vm.active_sfc_steps("PneumaticAB")
            .unwrap()
            .contains("S3_AMinus")
    );
    assert_eq!(io.digital_outputs.get("Qa.2"), Some(&false));
    assert_eq!(io.digital_outputs.get("Qa.1"), Some(&true));

    // Cylinder A retracts to A0 -> S4_BMinus (Qa.1 = false, Qa.3 = true)
    io.set_digital_in("Ia.2", false);
    io.set_digital_in("Ia.1", true);
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(
        vm.active_sfc_steps("PneumaticAB")
            .unwrap()
            .contains("S4_BMinus")
    );
    assert_eq!(io.digital_outputs.get("Qa.1"), Some(&false));
    assert_eq!(io.digital_outputs.get("Qa.3"), Some(&true));

    // Cylinder B retracts to B0 -> S0_Home (Qa.3 = false, completed_cycles = 1)
    io.set_digital_in("Ia.4", false);
    io.set_digital_in("Ia.3", true);
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(
        vm.active_sfc_steps("PneumaticAB")
            .unwrap()
            .contains("S0_Home")
    );
    assert_eq!(io.digital_outputs.get("Qa.3"), Some(&false));
    assert_eq!(vm.get_global("completed_cycles"), Some(&Value::Dint(1)));
}

#[test]
fn hostile_comment_corpus_and_c1_c5_conservation() {
    // 1. C3 hostile corpus across all 3 CommentEncoding modes
    for encoding in [
        CommentEncoding::Utf8Passthrough,
        CommentEncoding::AsciiFold,
        CommentEncoding::WarnOnly,
    ] {
        let mut ctx = plc_ctx();
        ctx.comment_encoding = encoding;
        let fe = SfcFrontend::with_comment_encoding(encoding);
        assert_eq!(
            verify_hostile_corpus_on_frontend(&fe, &ctx),
            Ok(()),
            "C3 hostile corpus failed for {encoding:?}"
        );
    }

    // 2. Full C1, C2, C4, C5 verification on a rich annotated SFC bundle
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();
    let pou_id = NodeId::new("n_pou");
    let s0_id = NodeId::new("n_s0");
    let act_id = NodeId::new("n_act");
    let tr_id = NodeId::new("n_tr");

    let bundle = BoardProgramBundle {
        program: BoardProgram {
            imports: Vec::new(),
            hardware: Vec::new(),
            types: Vec::new(),
            data: vec![DataDecl {
                id: NodeId::new("n_motor"),
                name: "Motor".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Bool),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Bool(false)),
                    Span::default(),
                )),
                span: Span::default(),
            }],
            modules: Vec::new(),
            pous: vec![Pou {
                id: pou_id.clone(),
                name: "CommentedChart".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: Some(AuthoringLanguage::Sfc),
                params: Vec::new(),
                locals: Vec::new(),
                body: PouBody::Chart(SfcChart {
                    steps: vec![SfcStep {
                        id: s0_id.clone(),
                        name: "S0".to_owned(),
                        initial: true,
                        actions: vec![SfcAction {
                            id: act_id.clone(),
                            qualifier: ActionQualifier::N,
                            duration: None,
                            target: Some(LValue::Var {
                                name: "Motor".to_owned(),
                                span: Span::default(),
                            }),
                            body: Vec::new(),
                        }],
                        macro_chart: None,
                    }],
                    transitions: vec![SfcTransition {
                        id: tr_id.clone(),
                        name: "T0".to_owned(),
                        from_steps: vec!["S0".to_owned()],
                        to_steps: vec!["S0".to_owned()],
                        priority: 0,
                        receptivity: Expr::new(
                            ExprKind::Literal(Literal::Bool(false)),
                            Span::default(),
                        ),
                    }],
                    evolve_to_stability: false,
                }),
                span: Span::default(),
            }],
        },
        comments: vec![
            Comment {
                id: "c_hdr".to_owned(),
                anchor: CommentAnchor::Node { id: pou_id.clone() },
                placement: AstCommentPlacement::Header,
                token_ordinal: None,
                text: " Chart header: Motor OK \u{2699}\u{FE0F} caf\u{00E9} vs cafe\u{0301}"
                    .to_owned(),
                origin: CommentOrigin {
                    language: "sfc".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_step_1".to_owned(),
                anchor: CommentAnchor::Node { id: s0_id.clone() },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " First step comment (must stay distinct)".to_owned(),
                origin: CommentOrigin {
                    language: "sfc".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_step_2".to_owned(),
                anchor: CommentAnchor::Node { id: s0_id.clone() },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " Second step comment with *) hazard and \\".to_owned(),
                origin: CommentOrigin {
                    language: "sfc".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_act_visual".to_owned(),
                anchor: CommentAnchor::Node { id: act_id.clone() },
                placement: AstCommentPlacement::Body,
                token_ordinal: None,
                text: " Visual note (* nested *) preserved ".to_owned(),
                origin: CommentOrigin {
                    language: "sfc".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_tr_inner".to_owned(),
                anchor: CommentAnchor::Node { id: tr_id.clone() },
                placement: AstCommentPlacement::Inner,
                token_ordinal: Some(2),
                text: " inner receptivity note ".to_owned(),
                origin: CommentOrigin {
                    language: "st".to_owned(),
                    style: CommentStyle::ParenBlock,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_dangling".to_owned(),
                anchor: CommentAnchor::Node { id: pou_id.clone() },
                placement: AstCommentPlacement::Dangling,
                token_ordinal: None,
                text: " Previously deleted step note".to_owned(),
                origin: CommentOrigin {
                    language: "sfc".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
        ],
        assumptions: Vec::new(),
        presentation: BoardPresentation::default(),
    };

    assert_eq!(assert_c5_comment_fields(&bundle.comments), Ok(()));
    assert_eq!(
        assert_c4_reimport_idempotence(&fe, &bundle.comments, &ctx),
        Ok(())
    );
    assert_eq!(
        assert_c2_orphan_preservation(&bundle.comments, &s0_id, &pou_id, Some(&ctx.board_id)),
        Ok(2)
    );

    // Emit records INFO_COMMENT_RELOCATED for c_tr_inner and preserves exact C1 multiset!
    let emitted = fe.emit(&bundle, &ctx).expect("emit commented SFC");
    assert!(
        emitted
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentRelocated)
    );

    let reparsed = verify_frontend_roundtrip(&fe, &bundle, &ctx).expect("C1 + LOSSLESS_ASL");
    assert_eq!(
        assert_c1_multiset_invariant(&bundle.comments, &reparsed.comments),
        Ok(())
    );
    assert_eq!(assert_c5_comment_fields(&reparsed.comments), Ok(()));
}

#[test]
fn three_level_foreign_roundtrip_and_cross_language_export_guard() {
    let fe = SfcFrontend::new();
    let mut ctx = plc_ctx();

    // 1. Same-language ("sfc") Foreign at all 3 AST levels (Decl, Stmt, Expr) round-trips losslessly!
    let same_lang = BoardProgramBundle::new(BoardProgram {
        imports: Vec::new(),
        hardware: Vec::new(),
        types: vec![TypeDecl::Foreign {
            id: NodeId::new("n_fdecl"),
            name: Some("CustomBlock".to_owned()),
            language: "sfc".to_owned(),
            fidelity: ForeignFidelity::F1,
            text: "EXT_BLOCK CustomBlock;".to_owned(),
            pack_hint: Some("plc-modbus-tcp".to_owned()),
            span: Span::default(),
        }],
        data: vec![DataDecl {
            id: NodeId::new("n_flag"),
            name: "flag".to_owned(),
            scope: StorageScope::Var,
            ty: TypeRef::scalar(ScalarType::Bool),
            initial: None,
            span: Span::default(),
        }],
        modules: Vec::new(),
        pous: vec![Pou {
            id: NodeId::new("n_pou"),
            name: "ForeignSameLang".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: Vec::new(),
            locals: Vec::new(),
            body: PouBody::Chart(SfcChart {
                steps: vec![SfcStep {
                    id: NodeId::new("n_s0"),
                    name: "S0".to_owned(),
                    initial: true,
                    actions: vec![SfcAction {
                        id: NodeId::new("n_a0"),
                        qualifier: ActionQualifier::N,
                        duration: None,
                        target: None,
                        body: vec![
                            Stmt::new(
                                NodeId::new("n_fstmt"),
                                StmtKind::Foreign {
                                    language: "sfc".to_owned(),
                                    fidelity: ForeignFidelity::F1,
                                    text: "MB_CLIENT(REQ := TRUE);".to_owned(),
                                    pack_hint: Some("plc-modbus-tcp".to_owned()),
                                },
                                Span::default(),
                            ),
                            Stmt::new(
                                NodeId::new("n_fexpr_stmt"),
                                StmtKind::Assign {
                                    target: LValue::Var {
                                        name: "flag".to_owned(),
                                        span: Span::default(),
                                    },
                                    value: Expr::new(
                                        ExprKind::Foreign {
                                            language: "sfc".to_owned(),
                                            text: "VENDOR_READY()".to_owned(),
                                        },
                                        Span::default(),
                                    ),
                                },
                                Span::default(),
                            ),
                        ],
                    }],
                    macro_chart: None,
                }],
                transitions: vec![SfcTransition {
                    id: NodeId::new("n_tr0"),
                    name: "T0".to_owned(),
                    from_steps: vec!["S0".to_owned()],
                    to_steps: vec!["S0".to_owned()],
                    priority: 0,
                    receptivity: Expr::new(
                        ExprKind::Foreign {
                            language: "sfc".to_owned(),
                            text: "EXT_TRIGGER()".to_owned(),
                        },
                        Span::default(),
                    ),
                }],
                evolve_to_stability: false,
            }),
            span: Span::default(),
        }],
    });

    let _rt = verify_frontend_roundtrip(&fe, &same_lang, &ctx)
        .expect("same-language 3-level Foreign must round-trip losslessly");

    // 2. Cross-language ("cpp") Foreign inside an SFC action is BLOCKED unless allow_foreign_export = true!
    let mut cross_lang = same_lang;
    cross_lang.program.types[0] = TypeDecl::Foreign {
        id: NodeId::new("n_fdecl"),
        name: Some("Servo".to_owned()),
        language: "cpp".to_owned(),
        fidelity: ForeignFidelity::F0,
        text: "Servo s;".to_owned(),
        pack_hint: None,
        span: Span::new(0, 8),
    };

    ctx.allow_foreign_export = false;
    let err_diags = fe
        .emit(&cross_lang, &ctx)
        .expect_err("cross-language Foreign export must be blocked without allow_foreign_export");
    assert_eq!(err_diags.len(), 1);
    assert_eq!(err_diags[0].code, ErrorCode::ErrForeignExportBlocked);
    assert!(
        is_four_part_diagnostic(&err_diags[0]),
        "Foreign diagnostic must satisfy strict four-part contract: {:?}",
        err_diags[0]
    );

    ctx.allow_foreign_export = true;
    let allowed = fe
        .emit(&cross_lang, &ctx)
        .expect("cross-language Foreign permitted with allow_foreign_export");
    assert!(
        allowed.value.contains("NOT TRANSLATED"),
        "demarcated NOT TRANSLATED block missing: {}",
        allowed.value
    );
}

#[test]
fn negative_fixtures_cover_every_sfc_error_code() {
    let fe = SfcFrontend::new();
    let ctx = plc_ctx();

    let assert_parse_code = |name: &str, src: &str, expected: ErrorCode| {
        let diags = fe
            .parse(src, &ctx)
            .expect_err(&format!("fixture `{name}` must fail with {expected}"));
        assert!(
            diags.iter().any(|d| d.code == expected),
            "fixture `{name}` expected {expected}, got: {diags:?}"
        );
    };

    // 1. ERR_TOML_SYNTAX: malformed SFC syntax, bad qualifier, chained comparisons
    assert_parse_code(
        "missing_end_step",
        "SFC_CHART Bad INITIAL_STEP S0: END_SFC_CHART",
        ErrorCode::ErrTomlSyntax,
    );
    assert_parse_code(
        "invalid_qualifier",
        "SFC_CHART Bad INITIAL_STEP S0: Z: Motor; END_STEP END_SFC_CHART",
        ErrorCode::ErrTomlSyntax,
    );
    assert_parse_code(
        "chained_comparison",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 FROM S0 TO S0 := 1 < 2 < 3; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrTomlSyntax,
    );

    // 2. ERR_UNKNOWN_FIELD: unknown header option, transition option, hardware attribute
    assert_parse_code(
        "unknown_chart_option",
        "SFC_CHART Bad (BOGUS := TRUE) INITIAL_STEP S0: END_STEP END_SFC_CHART",
        ErrorCode::ErrUnknownField,
    );
    assert_parse_code(
        "unknown_transition_option",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 (WEIGHT := 1) FROM S0 TO S0 := TRUE; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrUnknownField,
    );
    assert_parse_code(
        "unknown_hw_attr",
        "VAR_GLOBAL Btn AT %IX0.0 : BOOL (BOGUS_ATTR); END_VAR",
        ErrorCode::ErrUnknownField,
    );

    // 3. ERR_DUPLICATE_DECL: duplicate step, duplicate transition, duplicate POU
    assert_parse_code(
        "duplicate_step",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP STEP S0: END_STEP END_SFC_CHART",
        ErrorCode::ErrDuplicateDecl,
    );
    assert_parse_code(
        "duplicate_transition",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 FROM S0 TO S0 := TRUE; END_TRANSITION TRANSITION T0 FROM S0 TO S0 := FALSE; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrDuplicateDecl,
    );

    // 4. ERR_UNDECLARED_SYMBOL: transition FROM/TO undeclared step
    assert_parse_code(
        "undeclared_to_step",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 FROM S0 TO GhostStep := TRUE; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrUndeclaredSymbol,
    );
    assert_parse_code(
        "undeclared_from_step",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 FROM MissingStep TO S0 := TRUE; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrUndeclaredSymbol,
    );

    // 5. ERR_TYPE_MISMATCH: non-BOOL receptivity, L/D missing or non-TIME duration, N with duration, non-BOOL coil
    assert_parse_code(
        "non_bool_receptivity_int",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 FROM S0 TO S0 := 42; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_code(
        "non_bool_receptivity_time",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 FROM S0 TO S0 := S0.T; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_code(
        "l_missing_duration",
        "VAR_GLOBAL q : BOOL; END_VAR SFC_CHART Bad INITIAL_STEP S0: L: q; END_STEP END_SFC_CHART",
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_code(
        "d_non_time_duration",
        "VAR_GLOBAL q : BOOL; END_VAR SFC_CHART Bad INITIAL_STEP S0: D(123): q; END_STEP END_SFC_CHART",
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_code(
        "n_forbids_duration",
        "VAR_GLOBAL q : BOOL; END_VAR SFC_CHART Bad INITIAL_STEP S0: N(T#10ms): q; END_STEP END_SFC_CHART",
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_code(
        "non_bool_coil_target",
        "VAR_GLOBAL cnt : DINT; END_VAR SFC_CHART Bad INITIAL_STEP S0: N: cnt; END_STEP END_SFC_CHART",
        ErrorCode::ErrTypeMismatch,
    );

    // 6. ERR_CONST_WRITE: direct write to <Step>.X, <Step>.T, <Macro>.DONE, or CONST var
    assert_parse_code(
        "write_step_x",
        "SFC_CHART Bad INITIAL_STEP S0: N: { S0.X := TRUE; } END_STEP END_SFC_CHART",
        ErrorCode::ErrConstWrite,
    );
    assert_parse_code(
        "write_step_t",
        "SFC_CHART Bad INITIAL_STEP S0: N: { S0.T := T#0us; } END_STEP END_SFC_CHART",
        ErrorCode::ErrConstWrite,
    );
    assert_parse_code(
        "write_macro_done",
        "SFC_CHART Sub (KIND := FB) INITIAL_STEP E: END_STEP END_SFC_CHART SFC_CHART Bad INITIAL_STEP M0 MACRO Sub: N: { M0.DONE := FALSE; } END_STEP END_SFC_CHART",
        ErrorCode::ErrConstWrite,
    );
    assert_parse_code(
        "write_const_coil",
        "VAR_GLOBAL CONSTANT C_FLAG : BOOL := TRUE; END_VAR SFC_CHART Bad INITIAL_STEP S0: N: C_FLAG; END_STEP END_SFC_CHART",
        ErrorCode::ErrConstWrite,
    );

    // 7. ERR_HARDWARE_LABEL_WRITE: coil target bound to read-only DigitalIn (%IX0.0)
    assert_parse_code(
        "coil_on_digital_input",
        "VAR_GLOBAL Btn AT %IX0.0 : BOOL; END_VAR SFC_CHART Bad INITIAL_STEP S0: N: Btn; END_STEP END_SFC_CHART",
        ErrorCode::ErrHardwareLabelWrite,
    );

    // 8. ERR_UNKNOWN_FIELD_ACCESS: invalid attribute on step & .DONE on non-macro step
    assert_parse_code(
        "unknown_step_field",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 FROM S0 TO S0 := S0.BOGUS; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrUnknownFieldAccess,
    );
    assert_parse_code(
        "done_on_non_macro_step",
        "SFC_CHART Bad INITIAL_STEP S0: END_STEP TRANSITION T0 FROM S0 TO S0 := S0.DONE; END_TRANSITION END_SFC_CHART",
        ErrorCode::ErrUnknownFieldAccess,
    );

    // 9. ERR_UNRESOLVED_FUNCTION: macro-step referencing nonexistent sub-chart
    assert_parse_code(
        "missing_macro_subchart",
        "SFC_CHART Bad INITIAL_STEP M0 MACRO DoesNotExist: END_STEP END_SFC_CHART",
        ErrorCode::ErrUnresolvedFunction,
    );

    // 10. ERR_RECURSION_DEPTH: direct & mutual macro-step cycles
    assert_parse_code(
        "self_recursive_macro",
        "SFC_CHART SelfRec INITIAL_STEP M0 MACRO SelfRec: END_STEP END_SFC_CHART",
        ErrorCode::ErrRecursionDepth,
    );
    assert_parse_code(
        "mutual_recursive_macro",
        "SFC_CHART A INITIAL_STEP MA MACRO B: END_STEP END_SFC_CHART SFC_CHART B (KIND := FB) INITIAL_STEP MB MACRO A: END_STEP END_SFC_CHART",
        ErrorCode::ErrRecursionDepth,
    );

    // 11. ERR_ISR_FORBIDDEN_OP: blocking WAIT inside OB_HW_INT or OB_CYCLIC_INT
    assert_parse_code(
        "wait_in_hw_int",
        "SFC_CHART IsrChart (KIND := HW_INT(Btn, RISING, 1)) INITIAL_STEP S0: N: { WAIT(T#5ms); } END_STEP END_SFC_CHART",
        ErrorCode::ErrIsrForbiddenOp,
    );
    assert_parse_code(
        "wait_in_cyclic_int",
        "SFC_CHART CycChart (KIND := CYCLIC_INT(1000)) INITIAL_STEP S0: N: { WAIT(T#1ms); } END_STEP END_SFC_CHART",
        ErrorCode::ErrIsrForbiddenOp,
    );

    // 12. ERR_COMMENT_FIELD: unterminated paren comment on parse + invalid token_ordinal on emit
    assert_parse_code(
        "unterminated_paren_comment",
        "(* outer (* inner *) SFC_CHART Bad END_SFC_CHART",
        ErrorCode::ErrCommentField,
    );
    let mut bad_comment_bundle = BoardProgramBundle::new(BoardProgram {
        pous: vec![Pou {
            id: NodeId::new("n_p1"),
            name: "Main".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: Vec::new(),
            locals: Vec::new(),
            body: PouBody::Chart(SfcChart::default()),
            span: Span::default(),
        }],
        ..BoardProgram::default()
    });
    bad_comment_bundle.comments.push(Comment {
        id: "c_bad".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_p1"),
        },
        placement: AstCommentPlacement::Inner,
        token_ordinal: None, // Invalid: Inner requires token_ordinal!
        text: "bad inner".to_owned(),
        origin: CommentOrigin {
            language: "sfc".to_owned(),
            style: CommentStyle::ParenBlock,
        },
        span: Span::default(),
    });
    let err_cf = fe
        .emit(&bad_comment_bundle, &ctx)
        .expect_err("Inner without token_ordinal must fail");
    assert!(err_cf.iter().any(|d| d.code == ErrorCode::ErrCommentField));

    // 13. ERR_COMMENT_ANCHOR: comment pointing to nonexistent NodeId
    bad_comment_bundle.comments[0].placement = AstCommentPlacement::Leading;
    bad_comment_bundle.comments[0].anchor = CommentAnchor::Node {
        id: NodeId::new("n_does_not_exist"),
    };
    let err_ca = fe
        .emit(&bad_comment_bundle, &ctx)
        .expect_err("unresolved comment anchor must fail");
    assert!(err_ca.iter().any(|d| d.code == ErrorCode::ErrCommentAnchor));

    // 14. ERR_DUPLICATE_ID: duplicate AST NodeId on emit
    let dup_id_bundle = BoardProgramBundle::new(BoardProgram {
        pous: vec![Pou {
            id: NodeId::new("n_dup"),
            name: "Main".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: Vec::new(),
            locals: Vec::new(),
            body: PouBody::Chart(SfcChart {
                steps: vec![SfcStep {
                    id: NodeId::new("n_dup"), // Collides with POU NodeId!
                    name: "S0".to_owned(),
                    initial: true,
                    actions: Vec::new(),
                    macro_chart: None,
                }],
                transitions: Vec::new(),
                evolve_to_stability: false,
            }),
            span: Span::default(),
        }],
        ..BoardProgram::default()
    });
    let err_dup_id = fe
        .emit(&dup_id_bundle, &ctx)
        .expect_err("duplicate NodeId must fail");
    assert!(
        err_dup_id
            .iter()
            .any(|d| d.code == ErrorCode::ErrDuplicateId)
    );
}

#[test]
fn feature_matrix_entries_generate_valid_markdown_table() {
    let entries = feature_matrix_entries();
    assert!(entries.len() >= 10);
    assert!(entries.iter().all(|e| e.language == LanguageId::Sfc));

    let mut collector = FeatureMatrixCollector::new();
    for entry in entries {
        collector.record(entry);
    }
    let md = collector.to_markdown();
    assert!(md.contains("| Category | Feature | C++ | ST | Ladder | SFC | Evidence / Notes |"));
    assert!(md.contains("IEC 60848 Rule 5 Two-Phase Evolution"));
    assert!(md.contains("IEC 61131-3 Table 47 Action Qualifiers"));
    assert!(md.contains("proptest_lossless_asl_roundtrip_across_topologies"));
}

#[test]
fn full_asl_constructs_mcu_hardware_modules_and_validator_integration() {
    use dendri_ast::{
        CasePattern, ElifArm, EnumVariant, FbKind, InterruptEdge, ModuleDecl, ParamDecl,
        ParamDirection, Project, ProjectMeta, StructField, SwitchCase,
    };
    use dendri_validate::{MemoryProfileCatalog, ValidateOptions, validate_project};

    let fe = SfcFrontend::new();
    let uno_ctx = BoardContext::arduino_uno_r3("uno_1");

    let bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec!["std.iec.timers".to_owned(), "project.helpers".to_owned()],
        hardware: vec![
            HardwareBinding {
                id: NodeId::new("n_hw_btn"),
                label: "Btn".to_owned(),
                pin: "D2".to_owned(),
                kind: HardwareKind::DigitalIn,
                pull: PullMode::PullUp,
                active_low: true,
                codec: None,
                range: None,
                span: Span::default(),
            },
            HardwareBinding {
                id: NodeId::new("n_hw_led"),
                label: "Led".to_owned(),
                pin: "D13".to_owned(),
                kind: HardwareKind::DigitalOut,
                pull: PullMode::None,
                active_low: false,
                codec: None,
                range: None,
                span: Span::default(),
            },
            HardwareBinding {
                id: NodeId::new("n_hw_pwm"),
                label: "PwmOutPin".to_owned(),
                pin: "D9".to_owned(),
                kind: HardwareKind::PwmOut,
                pull: PullMode::None,
                active_low: false,
                codec: Some("pwm_8bit".to_owned()),
                range: None,
                span: Span::default(),
            },
            HardwareBinding {
                id: NodeId::new("n_hw_pot"),
                label: "PotPin".to_owned(),
                pin: "A0".to_owned(),
                kind: HardwareKind::AnalogIn,
                pull: PullMode::None,
                active_low: false,
                codec: Some("uno_10bit".to_owned()),
                range: Some(ChannelRange {
                    signal: SignalKind::Voltage,
                    low: 0.0,
                    high: 5.0,
                    live_zero: false,
                    hardware_normalizes_live_zero: false,
                }),
                span: Span::default(),
            },
        ],
        types: vec![
            TypeDecl::Struct {
                id: NodeId::new("n_ty_st"),
                name: "MotorCfg".to_owned(),
                fields: vec![
                    StructField {
                        name: "speed".to_owned(),
                        ty: TypeRef::scalar(ScalarType::Int),
                    },
                    StructField {
                        name: "enabled".to_owned(),
                        ty: TypeRef::scalar(ScalarType::Bool),
                    },
                ],
                span: Span::default(),
            },
            TypeDecl::Enum {
                id: NodeId::new("n_ty_en"),
                name: "RunMode".to_owned(),
                backing: ScalarType::Int,
                variants: vec![
                    EnumVariant {
                        name: "STOPPED".to_owned(),
                        value: 0,
                    },
                    EnumVariant {
                        name: "RUNNING".to_owned(),
                        value: 1,
                    },
                ],
                span: Span::default(),
            },
        ],
        data: vec![
            DataDecl {
                id: NodeId::new("n_d_lim"),
                name: "LIMIT_CNT".to_owned(),
                scope: StorageScope::Const,
                ty: TypeRef::scalar(ScalarType::Dint),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(10)),
                    Span::default(),
                )),
                span: Span::default(),
            },
            DataDecl {
                id: NodeId::new("n_d_ret"),
                name: "retained_cnt".to_owned(),
                scope: StorageScope::Retain,
                ty: TypeRef::scalar(ScalarType::Dint),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(0)),
                    Span::default(),
                )),
                span: Span::default(),
            },
            DataDecl {
                id: NodeId::new("n_d_ton"),
                name: "t_hold".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::Fb {
                    fb_kind: FbKind::Ton,
                },
                initial: None,
                span: Span::default(),
            },
        ],
        modules: vec![ModuleDecl {
            id: NodeId::new("n_mod"),
            name: "helpers".to_owned(),
            imports: Vec::new(),
            exports: vec!["AddOne".to_owned()],
            types: Vec::new(),
            data: Vec::new(),
            pous: vec![Pou {
                id: NodeId::new("n_fc"),
                name: "AddOne".to_owned(),
                kind: PouKind::Fc {
                    return_type: TypeRef::scalar(ScalarType::Dint),
                },
                preferred_view: Some(AuthoringLanguage::St),
                params: vec![ParamDecl {
                    name: "x".to_owned(),
                    direction: ParamDirection::In,
                    ty: TypeRef::scalar(ScalarType::Dint),
                }],
                locals: Vec::new(),
                body: PouBody::Statements(vec![Stmt::new(
                    NodeId::new("n_ret"),
                    StmtKind::Return {
                        value: Some(Expr::new(
                            ExprKind::Binary {
                                op: BinaryOp::Add,
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
            }],
            span: Span::default(),
        }],
        pous: vec![
            Pou {
                id: NodeId::new("n_isr"),
                name: "OnBtnEdge".to_owned(),
                kind: PouKind::Ob {
                    ob: ObKind::HwInt {
                        pin_label: "Btn".to_owned(),
                        edge: InterruptEdge::Rising,
                        priority: 1,
                    },
                },
                preferred_view: None,
                params: Vec::new(),
                locals: Vec::new(),
                body: PouBody::Statements(vec![Stmt::new(
                    NodeId::new("n_isr_a"),
                    StmtKind::Assign {
                        target: LValue::Var {
                            name: "retained_cnt".to_owned(),
                            span: Span::default(),
                        },
                        value: Expr::new(
                            ExprKind::Binary {
                                op: BinaryOp::Add,
                                left: Box::new(Expr::new(
                                    ExprKind::Ident("retained_cnt".to_owned()),
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
                id: NodeId::new("n_main"),
                name: "MainChart".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: Some(AuthoringLanguage::Sfc),
                params: Vec::new(),
                locals: vec![DataDecl {
                    id: NodeId::new("n_loc_i"),
                    name: "i".to_owned(),
                    scope: StorageScope::Temp,
                    ty: TypeRef::scalar(ScalarType::Dint),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(0)),
                        Span::default(),
                    )),
                    span: Span::default(),
                }],
                body: PouBody::Chart(SfcChart {
                    steps: vec![SfcStep {
                        id: NodeId::new("n_s0"),
                        name: "S0".to_owned(),
                        initial: true,
                        actions: vec![SfcAction {
                            id: NodeId::new("n_a0"),
                            qualifier: ActionQualifier::N,
                            duration: None,
                            target: Some(LValue::Var {
                                name: "Led".to_owned(),
                                span: Span::default(),
                            }),
                            body: vec![
                                Stmt::new(
                                    NodeId::new("n_fb"),
                                    StmtKind::FbInvoke {
                                        instance: LValue::Var {
                                            name: "t_hold".to_owned(),
                                            span: Span::default(),
                                        },
                                        inputs: vec![
                                            (
                                                "IN".to_owned(),
                                                Expr::new(
                                                    ExprKind::Ident("Btn".to_owned()),
                                                    Span::default(),
                                                ),
                                            ),
                                            (
                                                "PT".to_owned(),
                                                Expr::new(
                                                    ExprKind::Literal(Literal::TimeUs(500_000)),
                                                    Span::default(),
                                                ),
                                            ),
                                        ],
                                    },
                                    Span::default(),
                                ),
                                Stmt::new(
                                    NodeId::new("n_if"),
                                    StmtKind::If {
                                        condition: Expr::new(
                                            ExprKind::Binary {
                                                op: BinaryOp::Lt,
                                                left: Box::new(Expr::new(
                                                    ExprKind::Ident("retained_cnt".to_owned()),
                                                    Span::default(),
                                                )),
                                                right: Box::new(Expr::new(
                                                    ExprKind::Ident("LIMIT_CNT".to_owned()),
                                                    Span::default(),
                                                )),
                                            },
                                            Span::default(),
                                        ),
                                        then_body: vec![Stmt::new(
                                            NodeId::new("n_for"),
                                            StmtKind::For {
                                                counter: "i".to_owned(),
                                                from: Expr::new(
                                                    ExprKind::Literal(Literal::Int(0)),
                                                    Span::default(),
                                                ),
                                                to: Expr::new(
                                                    ExprKind::Literal(Literal::Int(3)),
                                                    Span::default(),
                                                ),
                                                step: Expr::new(
                                                    ExprKind::Literal(Literal::Int(1)),
                                                    Span::default(),
                                                ),
                                                inclusive: false,
                                                body: vec![Stmt::new(
                                                    NodeId::new("n_sw"),
                                                    StmtKind::Switch {
                                                        scrutinee: Expr::new(
                                                            ExprKind::Ident("i".to_owned()),
                                                            Span::default(),
                                                        ),
                                                        cases: vec![SwitchCase {
                                                            patterns: vec![
                                                                CasePattern::Value(Expr::new(
                                                                    ExprKind::Literal(
                                                                        Literal::Int(0),
                                                                    ),
                                                                    Span::default(),
                                                                )),
                                                                CasePattern::Range {
                                                                    start: Expr::new(
                                                                        ExprKind::Literal(
                                                                            Literal::Int(1),
                                                                        ),
                                                                        Span::default(),
                                                                    ),
                                                                    end: Expr::new(
                                                                        ExprKind::Literal(
                                                                            Literal::Int(2),
                                                                        ),
                                                                        Span::default(),
                                                                    ),
                                                                },
                                                            ],
                                                            body: vec![Stmt::new(
                                                                NodeId::new("n_cont"),
                                                                StmtKind::Continue,
                                                                Span::default(),
                                                            )],
                                                        }],
                                                        default: Some(vec![Stmt::new(
                                                            NodeId::new("n_brk"),
                                                            StmtKind::Break,
                                                            Span::default(),
                                                        )]),
                                                    },
                                                    Span::default(),
                                                )],
                                            },
                                            Span::default(),
                                        )],
                                        elif_arms: vec![ElifArm {
                                            condition: Expr::new(
                                                ExprKind::Literal(Literal::Bool(false)),
                                                Span::default(),
                                            ),
                                            body: vec![Stmt::new(
                                                NodeId::new("n_wh"),
                                                StmtKind::While {
                                                    condition: Expr::new(
                                                        ExprKind::Literal(Literal::Bool(false)),
                                                        Span::default(),
                                                    ),
                                                    body: Vec::new(),
                                                },
                                                Span::default(),
                                            )],
                                        }],
                                        else_body: Vec::new(),
                                    },
                                    Span::default(),
                                ),
                            ],
                        }],
                        macro_chart: None,
                    }],
                    transitions: Vec::new(),
                    evolve_to_stability: false,
                }),
                span: Span::default(),
            },
        ],
    });

    let reparsed = verify_frontend_roundtrip(&fe, &bundle, &uno_ctx)
        .expect("full MCU + module + stmt + SFC LOSSLESS_ASL roundtrip");

    // Verify parsed SFC board passes `dendri-validate` cleanly with zero errors!
    let plc_src = r#"
VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
    cycles : DINT := 0;
END_VAR

SFC_CHART ValidatedChart
    INITIAL_STEP S0:
        N: MotorOut;
        P1: { cycles := cycles + 1; }
    END_STEP
    STEP S1:
    END_STEP
    TRANSITION T0 FROM S0 TO S1 := StartBtn AND S0.T >= T#10ms;
    END_TRANSITION
    TRANSITION T1 FROM S1 TO S0 := NOT StartBtn;
    END_TRANSITION
END_SFC_CHART
"#;
    let parsed_plc = fe.parse(plc_src, &plc_ctx()).expect("parse plc sfc");
    let proj = Project {
        meta: ProjectMeta {
            schema_version: "2.0".to_owned(),
            asl_version: "0.1.0".to_owned(),
            requires: vec!["asl-core@1".to_owned()],
            name: "SfcValidated".to_owned(),
            author: String::new(),
            description: String::new(),
        },
        libraries: Vec::new(),
        packs: Vec::new(),
        comments: Vec::new(),
        boards: vec![board_from_program(parsed_plc.value.program)],
        components: Vec::new(),
        wires: Vec::new(),
    };
    let catalog = MemoryProfileCatalog::default();
    let diag_bag = validate_project(&proj, &catalog, &ValidateOptions::default());
    // Ignore ErrUnknownProfile since empty MemoryProfileCatalog has no profiles registered
    let non_profile_errors: Vec<_> = diag_bag
        .all()
        .iter()
        .filter(|d| {
            d.severity == dendri_span::Severity::Error && d.code != ErrorCode::ErrUnknownProfile
        })
        .collect();
    assert!(
        non_profile_errors.is_empty(),
        "parsed SFC chart must pass dendri-validate with 0 semantic errors: {non_profile_errors:?}"
    );
    assert!(!contains_synthetic_sfc_mangling(&reparsed.program));
}

// ---------------------------------------------------------------------------
// Property-Based `LOSSLESS_ASL` Round-Trip Suite (`04` §2, `docs/lbc/sfc.md` §7)
// ---------------------------------------------------------------------------

fn qualifier_strategy() -> impl Strategy<Value = ActionQualifier> {
    prop_oneof![
        Just(ActionQualifier::N),
        Just(ActionQualifier::S),
        Just(ActionQualifier::R),
        Just(ActionQualifier::L),
        Just(ActionQualifier::D),
        Just(ActionQualifier::P),
        Just(ActionQualifier::P1),
        Just(ActionQualifier::P0),
    ]
}

fn sfc_bundle_strategy() -> impl Strategy<Value = BoardProgramBundle> {
    (
        2usize..=5,
        any::<bool>(),
        any::<bool>(),
        prop::collection::vec(
            (
                qualifier_strategy(),
                1i64..=5000,
                any::<bool>(),
                any::<bool>(),
            ),
            0..=4,
        ),
        prop::collection::vec((0u32..=10, any::<bool>(), any::<bool>()), 1..=5),
    )
        .prop_map(
            |(num_steps, evolve_to_stability, include_macro, action_specs, tr_specs)| {
                let mut next_id = 1u32;
                let mut nid = || {
                    let id = NodeId::new(format!("n_{next_id:04}"));
                    next_id = next_id.saturating_add(1);
                    id
                };

                let mut pous = Vec::new();
                let sub_name = "SubAuto";
                if include_macro {
                    pous.push(Pou {
                        id: nid(),
                        name: sub_name.to_owned(),
                        kind: PouKind::Fb,
                        preferred_view: None,
                        params: Vec::new(),
                        locals: Vec::new(),
                        body: PouBody::Chart(SfcChart {
                            steps: vec![
                                SfcStep {
                                    id: nid(),
                                    name: "SubIn".to_owned(),
                                    initial: true,
                                    actions: Vec::new(),
                                    macro_chart: None,
                                },
                                SfcStep {
                                    id: nid(),
                                    name: "SubOut".to_owned(),
                                    initial: false,
                                    actions: Vec::new(),
                                    macro_chart: None,
                                },
                            ],
                            transitions: vec![SfcTransition {
                                id: nid(),
                                name: "T_sub".to_owned(),
                                from_steps: vec!["SubIn".to_owned()],
                                to_steps: vec!["SubOut".to_owned()],
                                priority: 0,
                                receptivity: Expr::new(
                                    ExprKind::Literal(Literal::Bool(true)),
                                    Span::default(),
                                ),
                            }],
                            evolve_to_stability: false,
                        }),
                        span: Span::default(),
                    });
                }

                let step_names: Vec<String> = (0..num_steps).map(|i| format!("S{i}")).collect();
                let mut steps = Vec::with_capacity(num_steps);
                let mut comments = Vec::new();

                for (idx, sname) in step_names.iter().enumerate() {
                    let step_id = nid();
                    let mut actions = Vec::new();
                    if idx == 0 {
                        for (a_idx, &(qual, ms, has_coil, has_body)) in
                            action_specs.iter().enumerate()
                        {
                            let duration = match qual {
                                ActionQualifier::L | ActionQualifier::D => Some(Expr::new(
                                    ExprKind::Literal(Literal::TimeUs(ms * 1_000)),
                                    Span::default(),
                                )),
                                ActionQualifier::N
                                | ActionQualifier::S
                                | ActionQualifier::R
                                | ActionQualifier::P
                                | ActionQualifier::P1
                                | ActionQualifier::P0 => None,
                            };
                            let target = if has_coil || !has_body {
                                Some(LValue::Var {
                                    name: format!("coil_{}", a_idx % 2),
                                    span: Span::default(),
                                })
                            } else {
                                None
                            };
                            let body = if has_body {
                                vec![Stmt::new(
                                    nid(),
                                    StmtKind::Assign {
                                        target: LValue::Var {
                                            name: "acc".to_owned(),
                                            span: Span::default(),
                                        },
                                        value: Expr::new(
                                            ExprKind::Binary {
                                                op: BinaryOp::Add,
                                                left: Box::new(Expr::new(
                                                    ExprKind::Ident("acc".to_owned()),
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
                                )]
                            } else {
                                Vec::new()
                            };
                            actions.push(SfcAction {
                                id: nid(),
                                qualifier: qual,
                                duration,
                                target,
                                body,
                            });
                        }
                    }

                    let macro_chart = if include_macro && idx + 1 == num_steps {
                        Some(sub_name.to_owned())
                    } else {
                        None
                    };

                    if idx == 0 {
                        comments.push(Comment {
                            id: "c_step0".to_owned(),
                            anchor: CommentAnchor::Node {
                                id: step_id.clone(),
                            },
                            placement: AstCommentPlacement::Leading,
                            token_ordinal: None,
                            text: format!(" Note for {sname}"),
                            origin: CommentOrigin {
                                language: "sfc".to_owned(),
                                style: CommentStyle::Line,
                            },
                            span: Span::default(),
                        });
                    }

                    steps.push(SfcStep {
                        id: step_id,
                        name: sname.clone(),
                        initial: idx == 0,
                        actions,
                        macro_chart,
                    });
                }

                let mut transitions = Vec::new();
                let step_at = |i: usize| -> String {
                    step_names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| "S0".to_owned())
                };
                for (t_idx, &(prio, and_from, and_to)) in tr_specs.iter().enumerate() {
                    let from_idx = t_idx % num_steps;
                    let to_idx = (t_idx + 1) % num_steps;
                    let from_steps = if and_from && num_steps >= 2 {
                        let other = (from_idx + 1) % num_steps;
                        vec![step_at(from_idx), step_at(other)]
                    } else {
                        vec![step_at(from_idx)]
                    };
                    let to_steps = if and_to && num_steps >= 2 {
                        let other = (to_idx + 1) % num_steps;
                        vec![step_at(to_idx), step_at(other)]
                    } else {
                        vec![step_at(to_idx)]
                    };

                    let base_step = step_at(from_idx);
                    let receptivity = if include_macro && from_idx + 1 == num_steps {
                        Expr::new(
                            ExprKind::Field {
                                base: Box::new(Expr::new(
                                    ExprKind::Ident(base_step),
                                    Span::default(),
                                )),
                                field: "DONE".to_owned(),
                            },
                            Span::default(),
                        )
                    } else {
                        Expr::new(
                            ExprKind::Binary {
                                op: BinaryOp::And,
                                left: Box::new(Expr::new(
                                    ExprKind::Field {
                                        base: Box::new(Expr::new(
                                            ExprKind::Ident(base_step.clone()),
                                            Span::default(),
                                        )),
                                        field: "X".to_owned(),
                                    },
                                    Span::default(),
                                )),
                                right: Box::new(Expr::new(
                                    ExprKind::Binary {
                                        op: BinaryOp::Ge,
                                        left: Box::new(Expr::new(
                                            ExprKind::Field {
                                                base: Box::new(Expr::new(
                                                    ExprKind::Ident(base_step),
                                                    Span::default(),
                                                )),
                                                field: "T".to_owned(),
                                            },
                                            Span::default(),
                                        )),
                                        right: Box::new(Expr::new(
                                            ExprKind::Literal(Literal::TimeUs(10_000)),
                                            Span::default(),
                                        )),
                                    },
                                    Span::default(),
                                )),
                            },
                            Span::default(),
                        )
                    };

                    transitions.push(SfcTransition {
                        id: nid(),
                        name: format!("T{t_idx}"),
                        from_steps,
                        to_steps,
                        priority: prio,
                        receptivity,
                    });
                }

                pous.push(Pou {
                    id: nid(),
                    name: "PropChart".to_owned(),
                    kind: PouKind::Ob { ob: ObKind::Main },
                    preferred_view: Some(AuthoringLanguage::Sfc),
                    params: Vec::new(),
                    locals: Vec::new(),
                    body: PouBody::Chart(SfcChart {
                        steps,
                        transitions,
                        evolve_to_stability,
                    }),
                    span: Span::default(),
                });

                BoardProgramBundle {
                    program: BoardProgram {
                        imports: vec!["std.iec.timers".to_owned()],
                        hardware: vec![
                            HardwareBinding {
                                id: nid(),
                                label: "Btn0".to_owned(),
                                pin: "Ia.0".to_owned(),
                                kind: HardwareKind::DigitalIn,
                                pull: PullMode::None,
                                active_low: false,
                                codec: None,
                                range: None,
                                span: Span::default(),
                            },
                            HardwareBinding {
                                id: nid(),
                                label: "coil_0".to_owned(),
                                pin: "Qa.0".to_owned(),
                                kind: HardwareKind::DigitalOut,
                                pull: PullMode::None,
                                active_low: false,
                                codec: None,
                                range: None,
                                span: Span::default(),
                            },
                        ],
                        types: Vec::new(),
                        data: vec![
                            DataDecl {
                                id: nid(),
                                name: "coil_1".to_owned(),
                                scope: StorageScope::Var,
                                ty: TypeRef::scalar(ScalarType::Bool),
                                initial: Some(Expr::new(
                                    ExprKind::Literal(Literal::Bool(false)),
                                    Span::default(),
                                )),
                                span: Span::default(),
                            },
                            DataDecl {
                                id: nid(),
                                name: "acc".to_owned(),
                                scope: StorageScope::Var,
                                ty: TypeRef::scalar(ScalarType::Dint),
                                initial: Some(Expr::new(
                                    ExprKind::Literal(Literal::Int(0)),
                                    Span::default(),
                                )),
                                span: Span::default(),
                            },
                        ],
                        modules: Vec::new(),
                        pous,
                    },
                    comments,
                    assumptions: Vec::new(),
                    presentation: BoardPresentation::default(),
                }
            },
        )
}

proptest! {
    #[test]
    fn proptest_lossless_asl_roundtrip_across_topologies(bundle in sfc_bundle_strategy()) {
        let fe = SfcFrontend::new();
        let ctx = plc_ctx();
        let reparsed = verify_frontend_roundtrip(&fe, &bundle, &ctx)
            .map_err(TestCaseError::fail)?;
        prop_assert!(!contains_synthetic_sfc_mangling(&reparsed.program));
    }

    #[test]
    fn proptest_parser_never_panics_on_arbitrary_utf8(src in "\\PC{0,256}") {
        let fe = SfcFrontend::new();
        let ctx = plc_ctx();
        let _ = fe.parse(&src, &ctx);
    }
}
