//! Comprehensive conformance, `LOSSLESS_ASL` proptest, `DEF-006` dialect, `C1–C5` hostile
//! comment, numeric contract, `iec_address`, three-level `Foreign`, `F1` pack, and negative
//! fixture test suite for `dendri-lang-st` (`docs/lbc/st.md` §7–§8).

use dendri_ast::{
    ActionQualifier, BinaryOp, BoardPresentation, BoardProgram, CallArg, CasePattern, Codec,
    CodecKind, Comment, CommentAnchor, CommentEncoding, CommentOrigin,
    CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, ElifArm, EolStyle, Expr,
    ExprKind, FbKind, ForeignFidelity, HardwareBinding, HardwareKind, InterruptEdge, LValue,
    LadderBoxKind, LadderCell, LadderCellKind, LadderCoilKind, LadderCompareOp, LadderContactKind,
    LadderNetwork, LadderRung, Literal, NodeId, ObKind, Pou, PouBody, PouKind, PullMode,
    ScalarType, SfcAction, SfcChart, SfcStep, SfcTransition, Stmt, StmtKind, StorageScope,
    SwitchCase, TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, FeatureMatrixCollector, LanguageFrontend, LanguageId,
    PackManifest, Paradigm, StCommentDialect, assert_c1_multiset_invariant,
    assert_c2_orphan_preservation, assert_c4_reimport_idempotence, assert_c5_comment_fields,
    assert_lossless_asl, contains_analog_full_scale_literal, contains_banned_magic_comment,
    is_four_part_diagnostic, verify_frontend_roundtrip, verify_hostile_corpus_on_frontend,
};
use dendri_lang_st::{StFrontend, emit_scaled_code_expr, feature_matrix_entries};
use dendri_span::{ErrorCode, Span};
use proptest::prelude::*;

fn plc_ctx() -> BoardContext {
    BoardContext::plc_1200_open_1214("plc_main")
}

fn assert_parse_error(src: &str, ctx: &BoardContext, expected: ErrorCode) {
    let frontend = StFrontend::with_config(ctx.st_dialect, ctx.comment_encoding);
    let res = frontend.parse(src, ctx);
    assert!(res.is_err(), "expected error {expected} for:\n{src}");
    if let Err(errs) = res {
        let matched = errs.iter().find(|d| d.code == expected);
        assert!(
            matched.is_some(),
            "expected {expected}, got {:?} for:\n{src}",
            errs.iter()
                .map(|d| (d.code, &d.message))
                .collect::<Vec<_>>()
        );
        if let Some(diag) = matched {
            assert!(
                diag.span.is_some(),
                "diagnostic {expected} must carry a byte Span: {diag:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 1. POUs, Organization Blocks & Separate VAR Blocks (`LOSSLESS_ASL`)
// ---------------------------------------------------------------------------

#[test]
fn st_pous_and_var_blocks_roundtrip() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();
    assert_eq!(frontend.id(), LanguageId::St);
    assert_eq!(frontend.paradigm(), Paradigm::Textual);

    let src = r#"
USING std.plc.scale;

VAR_GLOBAL
    g_counter : DINT := 0;
END_VAR

VAR_GLOBAL CONSTANT
    MAX_SPEED : DINT := 1500;
END_VAR

VAR_GLOBAL RETAIN
    g_total_cycles : DINT := 0;
END_VAR

VAR_GLOBAL PERSISTENT
    g_cal_offset : INT := 4;
END_VAR

NAMESPACE Helpers
    FUNCTION DoubleIt : DINT
        VAR_INPUT
            x : DINT;
        END_VAR
        DoubleIt := x * 2;
    END_FUNCTION
END_NAMESPACE

FUNCTION_BLOCK MotorCtrl
    VAR_INPUT
        run_cmd : BOOL;
        setpoint : DINT;
    END_VAR
    VAR_OUTPUT
        active : BOOL;
        clamped_sp : DINT;
    END_VAR
    VAR_IN_OUT
        cycle_count : DINT;
    END_VAR
    VAR
        prev_cmd : BOOL := FALSE;
    END_VAR
    VAR CONSTANT
        MIN_SP : DINT := 0;
    END_VAR
    VAR RETAIN
        run_hours : DINT := 0;
    END_VAR
    VAR_TEMP
        tmp : DINT;
    END_VAR
    tmp := LIMIT(MIN_SP, setpoint, MAX_SPEED);
    clamped_sp := tmp;
    active := run_cmd;
    IF run_cmd AND NOT prev_cmd THEN
        cycle_count := cycle_count + 1;
        run_hours := run_hours + 1;
    END_IF;
    prev_cmd := run_cmd;
END_FUNCTION_BLOCK

FUNCTION CalcScaled : REAL
    VAR_INPUT
        v : REAL;
    END_VAR
    VAR_TEMP
        n : REAL;
    END_VAR
    n := NORM_X(0.0, v, 100.0);
    CalcScaled := SCALE_X(0.0, n, 10.0);
END_FUNCTION

ORGANIZATION_BLOCK OB_STARTUP
    g_counter := 0;
END_ORGANIZATION_BLOCK

program ob_main
    var
        ctrl : MotorCtrl;
        sp_out : DINT;
        scaled : REAL;
    end_var
    ctrl(run_cmd := TRUE, setpoint := 500, cycle_count := g_counter);
    sp_out := ctrl.clamped_sp;
    scaled := CalcScaled(50.0);
    g_total_cycles := g_total_cycles + 1;
end_program

PROGRAM OB_CYCLIC_1000
    VAR
        tick : DINT := 0;
    END_VAR
    tick := tick + 1;
END_PROGRAM

PROGRAM OB_COMM_ETH0
    RETURN;
END_PROGRAM
"#;

    let parsed = frontend.parse(src, &ctx).expect("valid ST should parse");
    let prog = &parsed.value.program;

    assert_eq!(prog.imports, vec!["std.plc.scale"]);
    assert_eq!(prog.modules.len(), 1);
    assert_eq!(prog.modules[0].name, "Helpers");
    assert_eq!(prog.data.len(), 4);
    assert_eq!(prog.data[0].scope, StorageScope::Var);
    assert_eq!(prog.data[1].scope, StorageScope::Const);
    assert_eq!(prog.data[2].scope, StorageScope::Retain);
    assert_eq!(prog.data[3].scope, StorageScope::Persistent);

    // Verify OB classification & zero invented SETUP/LOOP
    assert_eq!(prog.pous.len(), 6);
    assert!(matches!(
        prog.pous[2].kind,
        PouKind::Ob {
            ob: ObKind::Startup
        }
    ));
    assert!(matches!(
        prog.pous[3].kind,
        PouKind::Ob { ob: ObKind::Main }
    ));
    assert!(matches!(
        prog.pous[4].kind,
        PouKind::Ob {
            ob: ObKind::CyclicInt { period_us: 1000 }
        }
    ));
    assert!(matches!(
        prog.pous[5].kind,
        PouKind::Ob {
            ob: ObKind::CommEvent { ref bus_id }
        } if bus_id == "ETH0"
    ));

    // Verify canonical emit has uppercase IEC keywords, zero SETUP/LOOP, zero magic comments
    let emitted = frontend.emit(&parsed.value, &ctx).unwrap().value;
    assert!(emitted.contains("PROGRAM OB_STARTUP"));
    assert!(emitted.contains("PROGRAM ob_main"));
    assert!(emitted.contains("VAR_GLOBAL CONSTANT"));
    assert!(emitted.contains("VAR_GLOBAL RETAIN"));
    assert!(emitted.contains("VAR_GLOBAL PERSISTENT"));
    assert!(emitted.contains("VAR_INPUT"));
    assert!(emitted.contains("VAR_OUTPUT"));
    assert!(emitted.contains("VAR_IN_OUT"));
    assert!(emitted.contains("VAR_TEMP"));
    assert!(!emitted.contains("SETUP "));
    assert!(!emitted.contains("LOOP "));
    assert!(!contains_banned_magic_comment(&emitted));

    // Verify full LOSSLESS_ASL roundtrip
    let _reparsed =
        verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).expect("LOSSLESS_ASL roundtrip");
}

// ---------------------------------------------------------------------------
// 2. Hardware `iec_address` Bidirectional Mapping (`%IX/%QX/%IW`)
// ---------------------------------------------------------------------------

#[test]
fn st_iec_address_hardware_roundtrip_zero_magic_comments() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    let src = r#"
VAR_GLOBAL
    StartBtn AT %ix0.0 : BOOL;
    StopBtn AT %IX0.1 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
    LampOut AT %QX0.2 : BOOL;
    PressRaw AT %IW64 : INT;
    BipolarRaw AT %IW66 : INT;
END_VAR

PROGRAM OB_MAIN
    VAR
        press_norm : ANALOG;
        bip_norm : ANALOG;
    END_VAR
    MotorOut := (StartBtn OR MotorOut) AND NOT StopBtn;
    LampOut := MotorOut;
    press_norm := CODE_TO_ANALOG(PressRaw, 'plc_span_27648');
    bip_norm := CODE_TO_ANALOG(BipolarRaw, 'plc_bipolar_27648');
END_PROGRAM
"#;

    let parsed = frontend.parse(src, &ctx).unwrap();
    let hw = &parsed.value.program.hardware;
    assert_eq!(hw.len(), 6);
    assert_eq!(
        (hw[0].label.as_str(), hw[0].pin.as_str(), hw[0].kind),
        ("StartBtn", "Ia.0", HardwareKind::DigitalIn)
    );
    assert_eq!(
        (hw[1].label.as_str(), hw[1].pin.as_str(), hw[1].kind),
        ("StopBtn", "Ia.1", HardwareKind::DigitalIn)
    );
    assert_eq!(
        (hw[2].label.as_str(), hw[2].pin.as_str(), hw[2].kind),
        ("MotorOut", "Qa.0", HardwareKind::DigitalOut)
    );
    assert_eq!(
        (hw[3].label.as_str(), hw[3].pin.as_str(), hw[3].kind),
        ("LampOut", "Qa.2", HardwareKind::DigitalOut)
    );
    assert_eq!(
        (hw[4].label.as_str(), hw[4].pin.as_str(), hw[4].kind),
        ("PressRaw", "IW64", HardwareKind::AnalogIn)
    );
    assert_eq!(hw[4].codec.as_deref(), Some("plc_span_27648"));
    assert_eq!(
        (hw[5].label.as_str(), hw[5].pin.as_str(), hw[5].kind),
        ("BipolarRaw", "IW66", HardwareKind::AnalogIn)
    );
    assert_eq!(hw[5].codec.as_deref(), Some("plc_bipolar_27648"));

    let emitted = frontend.emit(&parsed.value, &ctx).unwrap().value;
    assert!(emitted.contains("StartBtn AT %IX0.0 : BOOL;"));
    assert!(emitted.contains("MotorOut AT %QX0.0 : BOOL;"));
    assert!(emitted.contains("PressRaw AT %IW64 : INT;"));
    assert!(emitted.contains("BipolarRaw AT %IW66 : INT;"));
    assert!(!contains_banned_magic_comment(&emitted));

    let _reparsed =
        verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).expect("iec_address roundtrip");

    // On an MCU board without `iec_address` (arduino-uno-r3), hardware emits cleanly with zero magic comments
    let uno_ctx = BoardContext::arduino_uno_r3("mcu_1");
    let uno_bundle = BoardProgramBundle::new(BoardProgram {
        hardware: vec![HardwareBinding {
            id: NodeId::new("hw_led"),
            label: "LedOut".to_owned(),
            pin: "D13".to_owned(),
            kind: HardwareKind::DigitalOut,
            pull: PullMode::None,
            active_low: false,
            codec: None,
            range: None,
            span: Span::default(),
        }],
        pous: vec![Pou {
            id: NodeId::new("pou_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Statements(vec![Stmt::new(
                NodeId::new("s1"),
                StmtKind::Assign {
                    target: LValue::Var {
                        name: "LedOut".to_owned(),
                        span: Span::default(),
                    },
                    value: Expr::new(ExprKind::Literal(Literal::Bool(true)), Span::default()),
                },
                Span::default(),
            )]),
            span: Span::default(),
        }],
        ..BoardProgram::default()
    });
    let uno_st = frontend.emit(&uno_bundle, &uno_ctx).unwrap().value;
    assert!(uno_st.contains("LedOut : BOOL;"));
    assert!(!contains_banned_magic_comment(&uno_st));
}

// ---------------------------------------------------------------------------
// 3. IEC Literals, Types, Escapes & Precedence (`LOSSLESS_ASL`)
// ---------------------------------------------------------------------------

#[test]
fn st_literals_types_and_operators_roundtrip() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    let src = r#"
TYPE
    AxisState : STRUCT
        enabled : BOOL;
        pos : DINT;
    END_STRUCT;
    OpMode : USINT (
        IDLE := 0,
        AUTO := 1,
        FAULT := 9
    );
END_TYPE

PROGRAM OB_MAIN
    VAR
        hex_val : WORD := WORD#16#FF00;
        bin_val : BYTE := BYTE#2#1010_0101;
        oct_val : DINT := 8#77;
        neg_i16 : INT := INT#-10;
        flt_val : REAL := REAL#3.14;
        dur_a : TIME := T#1s500ms;
        dur_b : TIME := TIME#2m;
        dur_neg : TIME := t#-250us;
        msg : STRING[64] := 'Line1$LLine2$T$'$41$$olá';
        flag : BOOL := BOOL#TRUE;
        cmp_eq : BOOL;
        bits : DWORD := 16#00FF;
        arr : ARRAY[0..3] OF DINT;
        ax : AxisState;
        mode : OpMode := IDLE;
        math_res : DINT;
    END_VAR
    arr[0] := oct_val MOD 10;
    arr[1] := (arr[0] + 5) * 3 - 2 / 1;
    ax.enabled := flag AND NOT FALSE OR (TRUE XOR FALSE);
    ax.pos := arr[1];
    cmp_eq := arr[0] < arr[1] = (1 < 2);
    bits := (bits SHL 4) BIT_OR (SHR(bits, 2) BIT_AND DWORD#16#00FF);
    bits := BIT_NOT bits BIT_XOR DWORD#16#000F;
    mode := OpMode.AUTO;
    math_res := ABS(-ax.pos);
    dur_a := dur_a + T#500ms;
END_PROGRAM
"#;

    let parsed = frontend
        .parse(src, &ctx)
        .expect("literals & ops should parse");
    // Check decoded values
    let ob = &parsed.value.program.pous[0];
    assert_eq!(
        ob.locals[0].initial.as_ref().unwrap().kind,
        ExprKind::Literal(Literal::Int(0xFF00))
    );
    assert_eq!(
        ob.locals[1].initial.as_ref().unwrap().kind,
        ExprKind::Literal(Literal::Int(0b1010_0101))
    );
    assert_eq!(
        ob.locals[2].initial.as_ref().unwrap().kind,
        ExprKind::Literal(Literal::Int(63))
    );
    assert_eq!(
        ob.locals[3].initial.as_ref().unwrap().kind,
        ExprKind::Literal(Literal::Int(-10))
    );
    assert_eq!(
        ob.locals[5].initial.as_ref().unwrap().kind,
        ExprKind::Literal(Literal::TimeUs(1_500_000))
    );
    assert_eq!(
        ob.locals[6].initial.as_ref().unwrap().kind,
        ExprKind::Literal(Literal::TimeUs(120_000_000))
    );
    assert_eq!(
        ob.locals[7].initial.as_ref().unwrap().kind,
        ExprKind::Literal(Literal::TimeUs(-250))
    );
    assert_eq!(
        ob.locals[8].initial.as_ref().unwrap().kind,
        ExprKind::Literal(Literal::String("Line1\nLine2\t'A$olá".to_owned()))
    );

    // Verify `arr[0] < arr[1] = (1 < 2)` parsed as Eq(Lt(..), Lt(..))
    if let PouBody::Statements(stmts) = &ob.body {
        if let StmtKind::Assign { value, .. } = &stmts[4].kind {
            assert!(matches!(
                value.kind,
                ExprKind::Binary {
                    op: BinaryOp::Eq,
                    ..
                }
            ));
        } else {
            panic!("expected Assign stmt at index 4");
        }
    }

    let _reparsed =
        verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).expect("literals LOSSLESS_ASL");
}

// ---------------------------------------------------------------------------
// 4. Control Flow, FB Invocations, Member Access & SFC Step Attrs
// ---------------------------------------------------------------------------

#[test]
fn st_control_flow_fb_and_sfc_step_attrs() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    let src = r#"
FUNCTION ClampStep : DINT
    VAR_INPUT
        v : DINT;
    END_VAR
    IF v < 0 THEN
        ClampStep := 0;
        RETURN;
    ELSIF v > 100 THEN
        ClampStep := 100;
        RETURN;
    END_IF;
    ClampStep := v;
END_FUNCTION

PROGRAM OB_MAIN
    VAR
        Ton1 : TON;
        Ctu1 : CTU;
        R1 : R_TRIG;
        Latch1 : SR;
        i : DINT;
        acc : DINT := 0;
        mode : DINT := 2;
        done : BOOL := FALSE;
        elapsed : TIME := T#0us;
        cnt : DINT := 0;
        step_active : BOOL;
        step_time : TIME;
        macro_done : BOOL;
    END_VAR

    Ton1(IN := TRUE, PT := T#500ms);
    done := Ton1.Q;
    elapsed := Ton1.ET;

    R1(CLK := done);
    Ctu1(CU := R1.Q, R := FALSE, PV := 10);
    cnt := Ctu1.CV;
    Latch1(S1 := Ctu1.Q, R := FALSE);

    CASE mode OF
        0:
            acc := 0;
        1..5, 8:
            acc := ClampStep(cnt);
        ELSE
            acc := -1;
    END_CASE;

    FOR i := 0 TO 10 BY 2 DO
        IF i = 4 THEN
            CONTINUE;
        END_IF;
        IF i = 8 THEN
            EXIT;
        END_IF;
        acc := acc + i;
    END_FOR;

    WHILE acc > 0 DO
        acc := acc - 1;
    END_WHILE;

    REPEAT
        acc := acc + 1;
    UNTIL acc >= 3
    END_REPEAT;

    step_active := Step1.X;
    step_time := Step1.T;
    macro_done := MacroFill.DONE;
    IF NOT step_active AND (step_time = T#0us) AND NOT macro_done THEN
        RETURN;
    END_IF;
END_PROGRAM
"#;

    let parsed = frontend
        .parse(src, &ctx)
        .expect("control flow & FBs should parse");
    // Verify Ton1/Ctu1/R1/Latch1 lifted to FbInvoke
    if let PouBody::Statements(stmts) = &parsed.value.program.pous[1].body {
        assert!(matches!(stmts[0].kind, StmtKind::FbInvoke { .. }));
        assert!(matches!(stmts[3].kind, StmtKind::FbInvoke { .. }));
        assert!(matches!(stmts[4].kind, StmtKind::FbInvoke { .. }));
        assert!(matches!(stmts[6].kind, StmtKind::FbInvoke { .. }));
        // Verify REPEAT..UNTIL lowered into post-test While(TRUE) with trailing If(cond){Break}
        let repeat_while = &stmts[10];
        if let StmtKind::While { condition, body } = &repeat_while.kind {
            assert_eq!(condition.kind, ExprKind::Literal(Literal::Bool(true)));
            assert!(matches!(
                body.last().unwrap().kind,
                StmtKind::If {
                    ref then_body,
                    ..
                } if matches!(then_body.first().unwrap().kind, StmtKind::Break)
            ));
        } else {
            panic!("expected lowered While from REPEAT..UNTIL");
        }
    }

    // Verify Function early return fusion & roundtrip
    if let PouBody::Statements(fn_stmts) = &parsed.value.program.pous[0].body {
        assert_eq!(fn_stmts.len(), 2); // IF + trailing ClampStep := v
        if let StmtKind::If {
            then_body,
            elif_arms,
            ..
        } = &fn_stmts[0].kind
        {
            assert_eq!(then_body.len(), 1);
            assert!(matches!(
                then_body[0].kind,
                StmtKind::Return { value: Some(_) }
            ));
            assert_eq!(elif_arms[0].body.len(), 1);
            assert!(matches!(
                elif_arms[0].body[0].kind,
                StmtKind::Return { value: Some(_) }
            ));
        } else {
            panic!("expected IF stmt in ClampStep");
        }
    }

    let _reparsed =
        verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).expect("control flow roundtrip");
}

// ---------------------------------------------------------------------------
// 5. Numeric Contract: `ADC_CODE(IW64)` & `ReducedRationalPlan::emit_st` (NEVER `M`)
// ---------------------------------------------------------------------------

#[test]
fn st_numeric_contract_adc_code_and_reduced_rational_never_emits_m() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    // 5a. `PressRaw AT %IW64 : INT;` lifts to `ADC_CODE(PressRaw)` and round-trips to `PressRaw`
    let st_src = r#"
VAR_GLOBAL
    PressRaw AT %IW64 : INT;
END_VAR

PROGRAM OB_MAIN
    VAR
        norm : ANALOG;
        back : CODE<plc_span_27648>;
        pwm : DUTY;
    END_VAR
    norm := CODE_TO_ANALOG(PressRaw, 'plc_span_27648');
    back := ANALOG_TO_CODE(norm, 'plc_span_27648');
    pwm := ANALOG_TO_DUTY(norm);
END_PROGRAM
"#;
    let parsed = frontend.parse(st_src, &ctx).unwrap();
    if let PouBody::Statements(stmts) = &parsed.value.program.pous[0].body
        && let StmtKind::Assign { value, .. } = &stmts[0].kind
    {
        if let ExprKind::Call { callee, args } = &value.kind {
            assert_eq!(callee, "CODE_TO_ANALOG");
            assert!(matches!(
                &args[0].value.kind,
                ExprKind::Call { callee: c, .. } if c == "ADC_CODE"
            ));
        } else {
            panic!("expected CODE_TO_ANALOG call");
        }
    }
    let _reparsed = verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).unwrap();

    // 5b. ReducedRationalPlan::emit_st across catalog pairs — NEVER emits M = 10^9 and parses cleanly!
    let uno_10 = Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap();
    let adc_12 = Codec::new("adc_12bit", CodecKind::Binned, 4096, 4095).unwrap();
    let plc_span = Codec::new("plc_span_27648", CodecKind::Span, 27_648, 27_648).unwrap();
    let plc_bip = Codec::new("plc_bipolar_27648", CodecKind::SpanBipolar, 27_648, 27_648).unwrap();
    let adc_24 = Codec::new("adc_24bit", CodecKind::Binned, 1 << 24, (1 << 24) - 1).unwrap();
    let span_100k = Codec::new("span_100k", CodecKind::Span, 100_000, 100_000).unwrap();

    let uno_to_plc = emit_scaled_code_expr("raw", &uno_10, &plc_span).unwrap();
    assert_eq!(uno_to_plc, "(TO_UDINT(raw) * UDINT#27)");
    assert!(!contains_analog_full_scale_literal(&uno_to_plc));

    let plc_to_uno = emit_scaled_code_expr("PressRaw", &plc_span, &uno_10).unwrap();
    assert!(plc_to_uno.contains("MIN(UDINT#1023"));
    assert!(plc_to_uno.contains("UDINT#27"));
    assert!(!contains_analog_full_scale_literal(&plc_to_uno));

    let adc12_to_10 = emit_scaled_code_expr("raw", &adc_12, &uno_10).unwrap();
    assert_eq!(adc12_to_10, "SHR(TO_UDINT(raw), 2)");
    assert!(!contains_analog_full_scale_literal(&adc12_to_10));

    let bip_to_span = emit_scaled_code_expr("BipolarRaw", &plc_bip, &plc_span).unwrap();
    assert!(bip_to_span.contains("LIMIT(DINT#0,"));
    assert!(!contains_analog_full_scale_literal(&bip_to_span));

    let wide64 = emit_scaled_code_expr("raw", &adc_24, &span_100k).unwrap();
    assert!(wide64.contains("ULINT#3125"));
    assert!(!contains_analog_full_scale_literal(&wide64));

    // Verify every reduced-rational expression parses cleanly inside real IEC ST!
    let check_st = format!(
        "VAR_GLOBAL\n    PressRaw AT %IW64 : INT;\n    BipolarRaw AT %IW66 : INT;\nEND_VAR\n\
         PROGRAM OB_MAIN\n    VAR\n        raw : INT;\n        u32_a : UDINT;\n        u32_b : UDINT;\n        u32_c : UDINT;\n        s32_d : DINT;\n        u64_e : ULINT;\n    END_VAR\n\
             u32_a := {uno_to_plc};\n\
             u32_b := {plc_to_uno};\n\
             u32_c := {adc12_to_10};\n\
             s32_d := {bip_to_span};\n\
             u64_e := {wide64};\n\
         END_PROGRAM\n"
    );
    let parsed_scaled = frontend
        .parse(&check_st, &ctx)
        .expect("reduced-rational emitted ST must parse cleanly");
    assert_eq!(parsed_scaled.value.program.pous.len(), 1);

    // 5c. Cross-board AST (`uno_10bit` -> `plc_span_27648`) emits reduced rational automatically and never M!
    let cross_bundle = BoardProgramBundle::new(BoardProgram {
        pous: vec![Pou {
            id: NodeId::new("p1"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![
                DataDecl {
                    id: NodeId::new("d1"),
                    name: "raw".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Int),
                    initial: None,
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("d2"),
                    name: "scaled".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Udint),
                    initial: None,
                    span: Span::default(),
                },
            ],
            body: PouBody::Statements(vec![Stmt::new(
                NodeId::new("s1"),
                StmtKind::Assign {
                    target: LValue::Var {
                        name: "scaled".to_owned(),
                        span: Span::default(),
                    },
                    value: Expr::new(
                        ExprKind::Call {
                            callee: "ANALOG_TO_CODE".to_owned(),
                            args: vec![
                                CallArg {
                                    name: None,
                                    value: Expr::new(
                                        ExprKind::Call {
                                            callee: "CODE_TO_ANALOG".to_owned(),
                                            args: vec![
                                                CallArg {
                                                    name: None,
                                                    value: Expr::new(
                                                        ExprKind::Ident("raw".to_owned()),
                                                        Span::default(),
                                                    ),
                                                    span: Span::default(),
                                                },
                                                CallArg {
                                                    name: None,
                                                    value: Expr::new(
                                                        ExprKind::Literal(Literal::String(
                                                            "uno_10bit".to_owned(),
                                                        )),
                                                        Span::default(),
                                                    ),
                                                    span: Span::default(),
                                                },
                                            ],
                                        },
                                        Span::default(),
                                    ),
                                    span: Span::default(),
                                },
                                CallArg {
                                    name: None,
                                    value: Expr::new(
                                        ExprKind::Literal(Literal::String(
                                            "plc_span_27648".to_owned(),
                                        )),
                                        Span::default(),
                                    ),
                                    span: Span::default(),
                                },
                            ],
                        },
                        Span::default(),
                    ),
                },
                Span::default(),
            )]),
            span: Span::default(),
        }],
        ..BoardProgram::default()
    });
    let cross_emitted = frontend.emit(&cross_bundle, &ctx).unwrap().value;
    assert!(cross_emitted.contains("(TO_UDINT(raw) * UDINT#27)"));
    assert!(!contains_analog_full_scale_literal(&cross_emitted));
    assert!(frontend.parse(&cross_emitted, &ctx).is_ok());

    let forced_emitted = frontend
        .emit_with_reduced_rationals(&cross_bundle, &ctx)
        .unwrap()
        .value;
    assert!(forced_emitted.contains("(TO_UDINT(raw) * UDINT#27)"));
    assert!(!contains_analog_full_scale_literal(&forced_emitted));
}

// ---------------------------------------------------------------------------
// 6. `DEF-006` All 4 ST Comment Dialects & `C1–C5` Hostile Corpus Conservation
// ---------------------------------------------------------------------------

#[test]
fn st_def_006_all_four_dialects_and_hostile_c1_c5_corpus() {
    // 6a. Verify exact DEF-006 capability table & C3 hostile corpus across all 4 dialects × 3 encodings
    for &dialect in StCommentDialect::ALL {
        for encoding in [
            CommentEncoding::Utf8Passthrough,
            CommentEncoding::AsciiFold,
            CommentEncoding::WarnOnly,
        ] {
            let mut ctx = plc_ctx();
            ctx.st_dialect = dialect;
            ctx.comment_encoding = encoding;
            let frontend = StFrontend::with_config(dialect, encoding);
            let caps = frontend.capabilities();
            assert_eq!(caps.st_dialect, Some(dialect));
            assert_eq!(caps.comment_encoding, encoding);
            assert_eq!(
                caps.supports_nested_paren_comments,
                matches!(
                    dialect,
                    StCommentDialect::Iec61131Ed3 | StCommentDialect::RuSTy
                )
            );
            assert_eq!(
                caps.supports_line_comments,
                dialect != StCommentDialect::StrictEd2
            );
            assert_eq!(
                verify_hostile_corpus_on_frontend(&frontend, &ctx),
                Ok(()),
                "C3 hostile corpus failed for {dialect:?} / {encoding:?}"
            );
        }
    }

    // 6b. Nested `(* outer (* inner *) outer *)` + mixed styles + CRLF + C1/C2/C4/C5 end-to-end
    let src_crlf = "\
(* File header comment *)\r\n\
\r\n\
PROGRAM OB_MAIN\r\n\
    VAR\r\n\
        // Leading line 1\r\n\
        // Leading line 2\r\n\
        x : DINT := 0; // trailing decl\r\n\
        y : DINT := 1;\r\n\
    END_VAR\r\n\
    (* outer (* inner (* deep *) inner *) outer *)\r\n\
    x := (* gain *) y + 1;\r\n\
    /* C-style block comment */\r\n\
\r\n\
    // Separate paragraph 1\r\n\
\r\n\
    // Separate paragraph 2\r\n\
    y := x;\r\n\
    // Dangling before END_PROGRAM\r\n\
END_PROGRAM\r\n\
// Trailing file note\r\n";

    let ctx_ed3 = plc_ctx();
    let frontend_ed3 = StFrontend::with_dialect(StCommentDialect::RuSTy);
    let parsed = frontend_ed3.parse(src_crlf, &ctx_ed3).unwrap();
    assert_eq!(parsed.value.presentation.eol_style, EolStyle::Crlf);
    assert_eq!(assert_c5_comment_fields(&parsed.value.comments), Ok(()));
    assert_eq!(
        assert_c4_reimport_idempotence(&frontend_ed3, &parsed.value.comments, &ctx_ed3),
        Ok(())
    );

    // Check verbatim merged leading, nested paren, inner token_ordinal, C-block, separate paragraphs, dangling
    let texts: Vec<&str> = parsed
        .value
        .comments
        .iter()
        .map(|c| c.text.as_str())
        .collect();
    assert!(texts.contains(&" File header comment "));
    assert!(texts.contains(&" Leading line 1\n Leading line 2"));
    assert!(texts.contains(&" trailing decl"));
    assert!(texts.contains(&" outer (* inner (* deep *) inner *) outer "));
    assert!(texts.contains(&" gain "));
    assert!(texts.contains(&" C-style block comment "));
    assert!(texts.contains(&" Separate paragraph 1"));
    assert!(texts.contains(&" Separate paragraph 2"));
    assert!(texts.contains(&" Dangling before END_PROGRAM"));
    assert!(texts.contains(&" Trailing file note"));

    assert!(parsed.value.comments.iter().any(|c| {
        c.placement == AstCommentPlacement::Inner && c.token_ordinal.is_some() && c.text == " gain "
    }));
    assert!(parsed.value.comments.iter().any(|c| {
        c.placement == AstCommentPlacement::Dangling && c.text == " Dangling before END_PROGRAM"
    }));

    // Same-language round-trip preserves CRLF, exact AST, and exact C1 multiset!
    let emitted_crlf = frontend_ed3.emit(&parsed.value, &ctx_ed3).unwrap().value;
    assert!(emitted_crlf.contains("\r\n"));
    let reparsed = verify_frontend_roundtrip(&frontend_ed3, &parsed.value, &ctx_ed3).unwrap();
    assert_eq!(assert_c5_comment_fields(&reparsed.comments), Ok(()));

    // C2 orphan preservation test
    let first_stmt_id = match &parsed.value.program.pous[0].body {
        PouBody::Statements(stmts) => stmts[0].id.clone(),
        PouBody::Network(_) | PouBody::Chart(_) => panic!("expected statements"),
    };
    let pou_id = parsed.value.program.pous[0].id.clone();
    let orphaned = assert_c2_orphan_preservation(
        &parsed.value.comments,
        &first_stmt_id,
        &pou_id,
        Some("plc_main"),
    )
    .expect("C2 orphan preservation");
    assert!(orphaned >= 1);

    // Emit the same bundle under StrictEd2 (block-only, non-nested): zero `//` or `/*`, re-parses cleanly!
    let mut ctx_ed2 = plc_ctx();
    ctx_ed2.st_dialect = StCommentDialect::StrictEd2;
    let frontend_ed2 = StFrontend::with_dialect(StCommentDialect::StrictEd2);
    let emitted_ed2 = frontend_ed2.emit(&parsed.value, &ctx_ed2).unwrap().value;
    assert!(!emitted_ed2.contains("//"));
    assert!(!emitted_ed2.contains("/*"));
    let reparsed_ed2 = frontend_ed2
        .parse(&emitted_ed2, &ctx_ed2)
        .expect("StrictEd2 emission must re-parse cleanly under StrictEd2");
    assert_eq!(
        assert_lossless_asl(&parsed.value.program, &reparsed_ed2.value.program),
        Ok(())
    );
    // Every reparsed StrictEd2 comment matches its stored verbatim original via C4!
    assert_eq!(
        reparsed_ed2.value.comments.len(),
        parsed.value.comments.len()
    );
    for (orig, degraded) in parsed
        .value
        .comments
        .iter()
        .zip(reparsed_ed2.value.comments.iter())
    {
        assert!(
            frontend_ed2.matches_comment(&degraded.text, &orig.text),
            "C4 failed on StrictEd2 re-import: degraded={:?}, orig={:?}",
            degraded.text,
            orig.text
        );
    }

    // Cross-language Inner comment relocation (`origin.language = "cpp"`) + Trojan-Source bidi warning
    let mut cross_bundle = parsed.value.clone();
    cross_bundle.comments.push(Comment {
        id: "c_cpp_inner".to_owned(),
        anchor: CommentAnchor::Node { id: first_stmt_id },
        placement: AstCommentPlacement::Inner,
        token_ordinal: Some(1),
        text: " bidi \u{202E}hazard".to_owned(),
        origin: CommentOrigin {
            language: "cpp".to_owned(),
            style: CommentStyle::Block,
        },
        span: Span::default(),
    });
    let out_cross = frontend_ed3.emit(&cross_bundle, &ctx_ed3).unwrap();
    assert!(
        out_cross
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentRelocated)
    );
    assert!(
        out_cross
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::WarnCommentBidi)
    );
    assert!(!out_cross.value.contains('\u{202E}'));
    assert!(out_cross.value.contains("\\u{202E}"));
    // C1 multiset preserved on relocation (stored comments untouched)
    assert_eq!(
        assert_c1_multiset_invariant(&parsed.value.comments, &reparsed.comments),
        Ok(())
    );
}

// ---------------------------------------------------------------------------
// 7. Three-Level `Foreign` Preservation & Cross-Language Export Guard
// ---------------------------------------------------------------------------

#[test]
fn st_three_level_foreign_and_cross_language_export_guard() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    // 7a. Native ST 3-level Foreign (Decl, Stmt, Expr) round-trips losslessly with allow_foreign=false!
    let st_foreign_src = r#"
{attribute 'qualified_only'}

INTERFACE IMotor
    METHOD Start : BOOL
    END_METHOD
END_INTERFACE

TYPE
    IntPtr : POINTER TO INT;
END_TYPE

FUNCTION_BLOCK ServoBase EXTENDS BaseObj
    VAR
        pos : DINT;
    END_VAR
    METHOD Move : BOOL
        Move := TRUE;
    END_METHOD
END_FUNCTION_BLOCK

PROGRAM OB_MAIN
    VAR
        x : DINT := 2;
        y : DINT := 3;
        res : DINT := 0;
        p : IntPtr;
        addr_val : DWORD := 0;
    END_VAR
    {attribute 'no_check'}
    p^ := 42;
    addr_val := ADR(x);
    res := 1 + x ** y;
    res := p^;
END_PROGRAM
"#;

    let parsed = frontend
        .parse(st_foreign_src, &ctx)
        .expect("ST Foreign constructs should preserve cleanly");
    assert_eq!(parsed.value.program.types.len(), 4);
    assert!(
        parsed
            .value
            .program
            .types
            .iter()
            .all(|t| matches!(t, TypeDecl::Foreign { language, .. } if language == "st"))
    );

    if let PouBody::Statements(stmts) = &parsed.value.program.pous[0].body {
        assert!(matches!(
            stmts[0].kind,
            StmtKind::Foreign { ref language, fidelity: ForeignFidelity::F0, .. } if language == "st"
        ));
        assert!(matches!(
            stmts[1].kind,
            StmtKind::Foreign { ref language, ref text, .. } if language == "st" && text == "p^ := 42;"
        ));
        // `addr_val := ADR(x);`
        assert!(matches!(
            stmts[2].kind,
            StmtKind::Assign {
                value: Expr { kind: ExprKind::Foreign { ref text, .. }, .. },
                ..
            } if text == "ADR(x)"
        ));
        // `res := 1 + x ** y;` -> Binary(Add, 1, Foreign("st", "x ** y"))
        if let StmtKind::Assign { value, .. } = &stmts[3].kind {
            if let ExprKind::Binary { op, right, .. } = &value.kind {
                assert_eq!(*op, BinaryOp::Add);
                assert!(matches!(
                    right.kind,
                    ExprKind::Foreign { ref text, .. } if text == "x ** y"
                ));
            } else {
                panic!("expected Add with right Foreign Pow");
            }
        }
        // `res := p^;`
        assert!(matches!(
            stmts[4].kind,
            StmtKind::Assign {
                value: Expr { kind: ExprKind::Foreign { ref text, .. }, .. },
                ..
            } if text == "p^"
        ));
    }

    // Same-language round-trip MUST succeed even with allow_foreign_export = false!
    let _reparsed =
        verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).expect("ST Foreign roundtrip");

    // 7b. Cross-language (`cpp`) Foreign nodes MUST block export unless allow_foreign_export = true!
    let cpp_bundle = BoardProgramBundle::new(BoardProgram {
        types: vec![TypeDecl::Foreign {
            id: NodeId::new("f_decl"),
            name: Some("Servo".to_owned()),
            language: "cpp".to_owned(),
            fidelity: ForeignFidelity::F1,
            text: "Servo s;".to_owned(),
            pack_hint: Some("arduino-servo".to_owned()),
            span: Span::new(0, 8),
        }],
        pous: vec![Pou {
            id: NodeId::new("f_ob"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![DataDecl {
                id: NodeId::new("f_var"),
                name: "v".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Int),
                initial: None,
                span: Span::default(),
            }],
            body: PouBody::Statements(vec![
                Stmt::new(
                    NodeId::new("f_stmt"),
                    StmtKind::Foreign {
                        language: "cpp".to_owned(),
                        fidelity: ForeignFidelity::F0,
                        text: "PORTB |= (1 << 5);".to_owned(),
                        pack_hint: None,
                    },
                    Span::new(10, 28),
                ),
                Stmt::new(
                    NodeId::new("f_expr_stmt"),
                    StmtKind::Assign {
                        target: LValue::Var {
                            name: "v".to_owned(),
                            span: Span::new(30, 31),
                        },
                        value: Expr::new(
                            ExprKind::Foreign {
                                language: "cpp".to_owned(),
                                text: "s.read()".to_owned(),
                            },
                            Span::new(34, 42),
                        ),
                    },
                    Span::new(30, 43),
                ),
            ]),
            span: Span::new(0, 45),
        }],
        ..BoardProgram::default()
    });

    let blocked = frontend
        .emit(&cpp_bundle, &ctx)
        .expect_err("cross-language Foreign MUST be blocked by default");
    assert_eq!(blocked.len(), 3);
    for d in &blocked {
        assert_eq!(d.code, ErrorCode::ErrForeignExportBlocked);
        assert!(is_four_part_diagnostic(d), "expected 4-part diag: {d:?}");
    }

    let mut allow_ctx = plc_ctx();
    allow_ctx.allow_foreign_export = true;
    let exported = frontend
        .emit(&cpp_bundle, &allow_ctx)
        .expect("allow_foreign_export=true should emit demarcated blocks");
    assert!(exported.value.contains("NOT TRANSLATED"));
    assert!(exported.value.contains("Servo s;"));
    assert!(exported.value.contains("PORTB |= (1 << 5);"));
    assert!(exported.value.contains("s.read()"));
}

// ---------------------------------------------------------------------------
// 8. `F1` Pack Symbol Recognition (`plc-modbus-tcp` & Vendor Pack)
// ---------------------------------------------------------------------------

#[test]
fn st_f1_pack_symbol_recognition_modbus_and_vendor() {
    let modbus_toml = r#"
schema_version = "2.0"

[pack]
id = "plc-modbus-tcp"
name = "IEC 61131-3 Modbus TCP Communication Pack"
version = "1.0.0"
language = "st"
fidelity = "f1_declared"
standards = ["IEC 61131-3", "Modbus TCP"]
description = "F1 declared signatures for MB_CLIENT and MB_SERVER."

[[symbols]]
name = "MB_CLIENT"
kind = "fb"
params = [
    { name = "REQ", ty = "BOOL" },
    { name = "REMOTE_IP", ty = "STRING" },
    { name = "REMOTE_PORT", ty = "UINT" },
    { name = "UNIT_ID", ty = "USINT" },
    { name = "FC", ty = "USINT" },
    { name = "ADDR", ty = "UINT" },
    { name = "QTY", ty = "UINT" },
    { name = "DATA_RD", ty = "UINT" },
    { name = "DONE", ty = "BOOL" },
    { name = "BUSY", ty = "BOOL" },
    { name = "ERR", ty = "BOOL" },
    { name = "STATUS", ty = "WORD" },
]
return_ty = "VOID"
doc = "Modbus TCP client FB."
"#;

    let vendor_toml = r#"
schema_version = "2.0"

[pack]
id = "plc-vendor-ext"
name = "Vendor PLC Extension Pack"
version = "1.0.0"
language = "st"
fidelity = "f1_declared"
standards = ["IEC 61131-3"]
description = "F1 declared signatures for vendor checksum, logger, and PID FB."

[[symbols]]
name = "CRC16_CALC"
kind = "function"
params = [{ name = "DATA", ty = "WORD" }]
return_ty = "WORD"
doc = "Computes CRC16 over a word."

[[symbols]]
name = "LOG_EVENT"
kind = "function"
params = [{ name = "CODE", ty = "UINT" }]
return_ty = "VOID"
doc = "Records diagnostic event code."

[[symbols]]
name = "VENDOR_PID"
kind = "fb"
params = [
    { name = "SP", ty = "REAL" },
    { name = "PV", ty = "REAL" },
    { name = "OUT", ty = "REAL" },
]
return_ty = "VOID"
doc = "Vendor F1 PID FB."
"#;

    let mut ctx = plc_ctx();
    ctx.loaded_packs
        .push(PackManifest::parse_str(modbus_toml).unwrap());
    ctx.loaded_packs
        .push(PackManifest::parse_str(vendor_toml).unwrap());

    let frontend = StFrontend::new();
    let src = r#"
PROGRAM OB_MAIN
    VAR
        mb : MB_CLIENT;
        pid : VENDOR_PID;
        ok : BOOL;
        status_word : WORD;
        crc : WORD := WORD#16#1234;
        pid_out : REAL;
    END_VAR
    mb(REQ := TRUE, REMOTE_IP := '192.168.1.10', REMOTE_PORT := 502, FC := 3, ADDR := 100, QTY := 1);
    ok := mb.DONE AND NOT mb.BUSY;
    status_word := mb.STATUS;
    LOG_EVENT(42);
    IF CRC16_CALC(crc) = WORD#16#FFFF THEN
        status_word := CRC16_CALC(crc);
    END_IF;
    pid(SP := 50.0, PV := 48.5);
    pid_out := pid.OUT;
END_PROGRAM
"#;

    let parsed = frontend
        .parse(src, &ctx)
        .expect("F1 pack symbols should parse and type-check cleanly");

    if let PouBody::Statements(stmts) = &parsed.value.program.pous[0].body {
        assert!(matches!(
            stmts[3].kind,
            StmtKind::Foreign {
                fidelity: ForeignFidelity::F1,
                pack_hint: Some(ref hint),
                ..
            } if hint == "plc-vendor-ext"
        ));
    }

    let _reparsed =
        verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).expect("F1 pack roundtrip");
}

// ---------------------------------------------------------------------------
// 9. Negative Fixtures Covering Every Frozen `ErrorCode` (`docs/lbc/st.md` §1.4)
// ---------------------------------------------------------------------------

#[test]
fn st_negative_fixtures_cover_all_error_codes() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    // 1. ERR_TOML_SYNTAX (unterminated string, bad radix, chained cmp, invented SETUP/LOOP, StrictEd2 `//`)
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR s : STRING := 'unterminated;\n END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrTomlSyntax,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR x : DINT := 2#102;\n END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrTomlSyntax,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR a : DINT; b : DINT; c : DINT; ok : BOOL; END_VAR\n ok := a < b < c;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrTomlSyntax,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR a : DINT; b : DINT; c : DINT; ok : BOOL; END_VAR\n ok := a = b <> c;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrTomlSyntax,
    );
    assert_parse_error("SETUP\nEND_SETUP", &ctx, ErrorCode::ErrTomlSyntax);
    assert_parse_error("LOOP\nEND_LOOP", &ctx, ErrorCode::ErrTomlSyntax);
    let mut ctx_ed2 = plc_ctx();
    ctx_ed2.st_dialect = StCommentDialect::StrictEd2;
    assert_parse_error(
        "// forbidden in StrictEd2\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx_ed2,
        ErrorCode::ErrTomlSyntax,
    );

    // 2. ERR_COMMENT_FIELD (unterminated nested comment + invalid Inner comment on emit)
    assert_parse_error(
        "(* outer (* inner *)\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrCommentField,
    );
    assert_parse_error(
        "/* unterminated block\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrCommentField,
    );
    let mut bad_comment_bundle = BoardProgramBundle::default();
    bad_comment_bundle.comments.push(Comment {
        id: "c_bad".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n1"),
        },
        placement: AstCommentPlacement::Inner,
        token_ordinal: None,
        text: "missing ordinal".to_owned(),
        origin: CommentOrigin {
            language: "st".to_owned(),
            style: CommentStyle::ParenBlock,
        },
        span: Span::new(0, 5),
    });
    let emit_err = frontend.emit(&bad_comment_bundle, &ctx).unwrap_err();
    assert_eq!(emit_err[0].code, ErrorCode::ErrCommentField);

    // 3. ERR_UNKNOWN_TYPE (unknown identifier, WSTRING, DATE, double-quoted literal, unknown CODE codec)
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR x : UnknownCustomType; END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnknownType,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR s : WSTRING; END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnknownType,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR d : DATE; END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnknownType,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR s : STRING := \"double\"; END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnknownType,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR c : CODE<nonexistent_codec>; END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnknownType,
    );

    // 4. ERR_TYPE_MISMATCH (INT + UINT, DINT := REAL, IF non-BOOL, ANALOG + DINT)
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR a : INT; b : UINT; c : INT; END_VAR\n c := a + b;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR a : DINT; r : REAL; END_VAR\n a := r;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR n : DINT; END_VAR\n IF n THEN n := 1; END_IF;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR a : ANALOG; n : DINT; END_VAR\n a := a + n;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrTypeMismatch,
    );

    // 5. ERR_INVALID_CAST (`BOOL_TO_REAL`, bad codec in `CODE_TO_ANALOG`, wrong input to `INT_TO_DINT`)
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR b : BOOL; r : REAL; END_VAR\n r := BOOL_TO_REAL(b);\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrInvalidCast,
    );
    assert_parse_error(
        "VAR_GLOBAL PressRaw AT %IW64 : INT; END_VAR\nPROGRAM OB_MAIN\n VAR a : ANALOG; END_VAR\n a := CODE_TO_ANALOG(PressRaw, 'bogus_codec');\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrInvalidCast,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR b : BOOL; d : DINT; END_VAR\n d := INT_TO_DINT(b);\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrInvalidCast,
    );

    // 6. ERR_ARRAY_BOUNDS_CONST (non-zero lower bound, constant out-of-bounds index)
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR arr : ARRAY[1..5] OF DINT; END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrArrayBoundsConst,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR arr : ARRAY[0..3] OF DINT; x : DINT; END_VAR\n x := arr[4];\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrArrayBoundsConst,
    );

    // 7. ERR_UNKNOWN_FIELD_ACCESS (FB & struct nonexistent fields)
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR t1 : TON; b : BOOL; END_VAR\n b := t1.DOES_NOT_EXIST;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnknownFieldAccess,
    );
    assert_parse_error(
        "TYPE S : STRUCT a : DINT; END_STRUCT; END_TYPE\nPROGRAM OB_MAIN\n VAR s : S; x : DINT; END_VAR\n x := s.missing;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnknownFieldAccess,
    );

    // 8. ERR_UNDECLARED_SYMBOL
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR x : DINT; END_VAR\n x := ghost_var + 1;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUndeclaredSymbol,
    );

    // 9. ERR_CONST_WRITE (`VAR CONSTANT`, `VAR_INPUT`, read-only SFC `<Step>.X`)
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR CONSTANT LIMIT_V : DINT := 10; END_VAR\n LIMIT_V := 20;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrConstWrite,
    );
    assert_parse_error(
        "FUNCTION BadFn : DINT\n VAR_INPUT inp : DINT; END_VAR\n inp := 5;\n BadFn := inp;\nEND_FUNCTION",
        &ctx,
        ErrorCode::ErrConstWrite,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n Step1.X := TRUE;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrConstWrite,
    );

    // 10. ERR_HARDWARE_LABEL_WRITE (assigning to `%IX0.0` or `%IW64`)
    assert_parse_error(
        "VAR_GLOBAL StartBtn AT %IX0.0 : BOOL; END_VAR\nPROGRAM OB_MAIN\n StartBtn := TRUE;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrHardwareLabelWrite,
    );
    assert_parse_error(
        "VAR_GLOBAL PressRaw AT %IW64 : INT; END_VAR\nPROGRAM OB_MAIN\n PressRaw := 100;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrHardwareLabelWrite,
    );

    // 11. ERR_DUPLICATE_DECL (case-insensitive duplicate local & POU)
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR cnt : DINT; CNT : DINT; END_VAR\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrDuplicateDecl,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\nEND_PROGRAM\nPROGRAM ob_main\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrDuplicateDecl,
    );

    // 12. ERR_NAME_COLLISION (hardware label vs global data)
    assert_parse_error(
        "VAR_GLOBAL\n StartBtn AT %IX0.0 : BOOL;\n startbtn : BOOL;\nEND_VAR\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrNameCollision,
    );

    // 13. ERR_UNRESOLVED_FUNCTION
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR x : DINT; END_VAR\n x := DoesNotExist(1);\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnresolvedFunction,
    );

    // 14. ERR_UNRESOLVED_IMPORT (unimported `std.plc.scale`, isolated `std.arduino.*`, unknown `USING`)
    assert_parse_error(
        "USING std.no_such_module;\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnresolvedImport,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR r : REAL; END_VAR\n r := NORM_X(0.0, 5.0, 10.0);\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnresolvedImport,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n VAR t : UDINT; END_VAR\n t := MILLIS();\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnresolvedImport,
    );

    // 15. ERR_ARITY_MISMATCH
    assert_parse_error(
        "FUNCTION AddTwo : DINT\n VAR_INPUT a : DINT; b : DINT; END_VAR\n AddTwo := a + b;\nEND_FUNCTION\nPROGRAM OB_MAIN\n VAR x : DINT; END_VAR\n x := AddTwo(1);\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrArityMismatch,
    );

    // 16. ERR_RETURN_TYPE_MISMATCH
    assert_parse_error(
        "FUNCTION RetBad : DINT\n RetBad := TRUE;\nEND_FUNCTION",
        &ctx,
        ErrorCode::ErrReturnTypeMismatch,
    );

    // 17. ERR_BREAK_OUTSIDE_LOOP & 18. ERR_CONTINUE_OUTSIDE_LOOP
    assert_parse_error(
        "PROGRAM OB_MAIN\n EXIT;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrBreakOutsideLoop,
    );
    assert_parse_error(
        "PROGRAM OB_MAIN\n CONTINUE;\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrContinueOutsideLoop,
    );

    // 19. ERR_ISR_FORBIDDEN_OP (`WAIT` inside `OB_HW_*` and `OB_CYCLIC_*`)
    assert_parse_error(
        "PROGRAM OB_HW_StartBtn_RISING_1\n WAIT(T#10ms);\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrIsrForbiddenOp,
    );
    assert_parse_error(
        "PROGRAM OB_CYCLIC_1000\n WAIT(T#5ms);\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrIsrForbiddenOp,
    );

    // 20. ERR_UNKNOWN_PORT
    assert_parse_error(
        "VAR_GLOBAL Ghost AT %IX9.9 : BOOL; END_VAR\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrUnknownPort,
    );

    // 21. ERR_PIN_CAPABILITY_MISMATCH
    assert_parse_error(
        "VAR_GLOBAL Bad AT %IX0.0 : INT; END_VAR\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrPinCapabilityMismatch,
    );
    assert_parse_error(
        "VAR_GLOBAL Bad AT %IW64 : BOOL; END_VAR\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrPinCapabilityMismatch,
    );
    assert_parse_error(
        "VAR_GLOBAL BadPwm AT %QX0.2 : DUTY; END_VAR\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrPinCapabilityMismatch,
    );

    // 22. ERR_HARDWARE_PIN_CONFLICT
    assert_parse_error(
        "VAR_GLOBAL\n A AT %IX0.0 : BOOL;\n B AT %IX0.0 : BOOL;\nEND_VAR\nPROGRAM OB_MAIN\nEND_PROGRAM",
        &ctx,
        ErrorCode::ErrHardwarePinConflict,
    );

    // 23. ERR_IMPORT_CYCLE
    assert_parse_error(
        "NAMESPACE ModA\n USING ModB;\nEND_NAMESPACE\nNAMESPACE ModB\n USING ModA;\nEND_NAMESPACE",
        &ctx,
        ErrorCode::ErrImportCycle,
    );

    // 24. ERR_FOREIGN_UNTYPED (strict 4-part diagnostic)
    let untyped_src = "PROGRAM OB_MAIN\n VAR x : DINT; y : DINT; END_VAR\n IF ADR(x) = ADR(y) THEN x := 1; END_IF;\nEND_PROGRAM";
    let untyped_errs = frontend.parse(untyped_src, &ctx).unwrap_err();
    let untyped_diag = untyped_errs
        .iter()
        .find(|d| d.code == ErrorCode::ErrForeignUntyped)
        .expect("expected ERR_FOREIGN_UNTYPED");
    assert!(is_four_part_diagnostic(untyped_diag));

    // 25. ERR_RECURSION_DEPTH (Pratt recursion guard > 128)
    let deep = format!(
        "PROGRAM OB_MAIN\n VAR b : BOOL; END_VAR\n b := {}TRUE;\nEND_PROGRAM",
        "NOT ".repeat(140)
    );
    assert_parse_error(&deep, &ctx, ErrorCode::ErrRecursionDepth);
}

// ---------------------------------------------------------------------------
// 10. Visual POU Derived ST Projection (`PouBody::Network` & `PouBody::Chart`)
// ---------------------------------------------------------------------------

#[test]
fn st_visual_pou_derived_projection() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    let bundle = BoardProgramBundle::new(BoardProgram {
        hardware: vec![
            HardwareBinding {
                id: NodeId::new("hw1"),
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
                id: NodeId::new("hw2"),
                label: "MotorOut".to_owned(),
                pin: "Qa.0".to_owned(),
                kind: HardwareKind::DigitalOut,
                pull: PullMode::None,
                active_low: false,
                codec: None,
                range: None,
                span: Span::default(),
            },
        ],
        pous: vec![
            Pou {
                id: NodeId::new("lad_pou"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: None,
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork {
                    rungs: vec![LadderRung {
                        id: NodeId::new("r1"),
                        order: 0,
                        cells: vec![
                            LadderCell {
                                id: NodeId::new("c1"),
                                row: 0,
                                col: 0,
                                kind: LadderCellKind::Contact {
                                    kind: LadderContactKind::NormallyOpen,
                                    target: LValue::Var {
                                        name: "StartBtn".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                            LadderCell {
                                id: NodeId::new("c2"),
                                row: 0,
                                col: 1,
                                kind: LadderCellKind::Box(LadderBoxKind::Compare {
                                    op: LadderCompareOp::Ge,
                                    left: Expr::new(
                                        ExprKind::Literal(Literal::Int(10)),
                                        Span::default(),
                                    ),
                                    right: Expr::new(
                                        ExprKind::Literal(Literal::Int(5)),
                                        Span::default(),
                                    ),
                                }),
                            },
                            LadderCell {
                                id: NodeId::new("c3"),
                                row: 0,
                                col: 2,
                                kind: LadderCellKind::Coil {
                                    kind: LadderCoilKind::Normal,
                                    target: LValue::Var {
                                        name: "MotorOut".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                        ],
                    }],
                }),
                span: Span::default(),
            },
            Pou {
                id: NodeId::new("sfc_pou"),
                name: "SeqCtrl".to_owned(),
                kind: PouKind::Fb,
                preferred_view: None,
                params: vec![],
                locals: vec![],
                body: PouBody::Chart(SfcChart {
                    steps: vec![SfcStep {
                        id: NodeId::new("s0"),
                        name: "S0".to_owned(),
                        initial: true,
                        actions: vec![SfcAction {
                            id: NodeId::new("a0"),
                            qualifier: ActionQualifier::N,
                            duration: None,
                            target: Some(LValue::Var {
                                name: "MotorOut".to_owned(),
                                span: Span::default(),
                            }),
                            body: vec![],
                        }],
                        macro_chart: None,
                    }],
                    transitions: vec![SfcTransition {
                        id: NodeId::new("t0"),
                        name: "T0".to_owned(),
                        from_steps: vec!["S0".to_owned()],
                        to_steps: vec!["S0".to_owned()],
                        priority: 0,
                        receptivity: Expr::new(
                            ExprKind::Literal(Literal::Bool(true)),
                            Span::default(),
                        ),
                    }],
                    evolve_to_stability: false,
                }),
                span: Span::default(),
            },
        ],
        ..BoardProgram::default()
    });

    let emitted = frontend.emit(&bundle, &ctx).unwrap().value;
    assert!(!contains_banned_magic_comment(&emitted));
    assert!(emitted.contains("_rail : BOOL;"));
    assert!(emitted.contains("_rail := _rail AND StartBtn;"));
    assert!(emitted.contains("_rail := _rail AND (10 >= 5);"));
    assert!(emitted.contains("MotorOut := _rail;"));
    assert!(emitted.contains("IF S0.X THEN"));
    // Derived ST parses back cleanly as valid IEC 61131-3 ST!
    assert!(frontend.parse(&emitted, &ctx).is_ok());
}

// ---------------------------------------------------------------------------
// 11. Auto-Generated Feature Matrix (`04` §2, `docs/lbc/st.md` §8)
// ---------------------------------------------------------------------------

#[test]
fn st_feature_matrix_collector_generates_markdown() {
    let entries = feature_matrix_entries();
    assert!(entries.len() >= 10);
    let mut collector = FeatureMatrixCollector::new();
    for entry in entries {
        assert_eq!(entry.language, LanguageId::St);
        assert!(!entry.evidence_test.is_empty());
        collector.record(entry);
    }
    let md = collector.to_markdown();
    assert!(md.contains("| Category | Feature | C++ | ST | Ladder | SFC | Evidence / Notes |"));
    assert!(md.contains("st_lossless_asl_proptest"));
    assert!(md.contains("st_def_006_all_four_dialects_and_hostile_c1_c5_corpus"));
}

// ---------------------------------------------------------------------------
// 12. `LOSSLESS_ASL` Property-Based Testing (`proptest`)
// ---------------------------------------------------------------------------

fn dint_expr_strategy() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        (0i128..=50_000i128)
            .prop_map(|n| Expr::new(ExprKind::Literal(Literal::Int(n)), Span::default())),
        prop_oneof![Just("a"), Just("b"), Just("c")]
            .prop_map(|s| Expr::new(ExprKind::Ident(s.to_owned()), Span::default())),
    ];
    leaf.prop_recursive(3, 16, 2, |inner| {
        prop_oneof![
            (
                prop_oneof![
                    Just(BinaryOp::Add),
                    Just(BinaryOp::Sub),
                    Just(BinaryOp::Mul),
                    Just(BinaryOp::Div),
                    Just(BinaryOp::Mod),
                    Just(BinaryOp::BitAnd),
                    Just(BinaryOp::BitXor),
                    Just(BinaryOp::BitOr),
                ],
                inner.clone(),
                inner.clone(),
            )
                .prop_map(|(op, l, r)| Expr::new(
                    ExprKind::Binary {
                        op,
                        left: Box::new(l),
                        right: Box::new(r),
                    },
                    Span::default(),
                )),
            (
                prop_oneof![
                    Just(UnaryOp::Neg),
                    Just(UnaryOp::Pos),
                    Just(UnaryOp::BitNot)
                ],
                inner
            )
                .prop_map(|(op, operand)| Expr::new(
                    ExprKind::Unary {
                        op,
                        operand: Box::new(operand),
                    },
                    Span::default(),
                )),
        ]
    })
}

fn bool_expr_strategy() -> impl Strategy<Value = Expr> {
    let cmp = (
        prop_oneof![
            Just(BinaryOp::Lt),
            Just(BinaryOp::Le),
            Just(BinaryOp::Gt),
            Just(BinaryOp::Ge),
            Just(BinaryOp::Eq),
            Just(BinaryOp::Ne),
        ],
        dint_expr_strategy(),
        dint_expr_strategy(),
    )
        .prop_map(|(op, l, r)| {
            Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(l),
                    right: Box::new(r),
                },
                Span::default(),
            )
        });
    let leaf = prop_oneof![
        any::<bool>().prop_map(|b| Expr::new(ExprKind::Literal(Literal::Bool(b)), Span::default())),
        Just(Expr::new(
            ExprKind::Ident("flag".to_owned()),
            Span::default()
        )),
        Just(Expr::new(
            ExprKind::Ident("StartBtn".to_owned()),
            Span::default()
        )),
        cmp,
    ];
    leaf.prop_recursive(2, 10, 2, |inner| {
        prop_oneof![
            (
                prop_oneof![Just(BinaryOp::And), Just(BinaryOp::Xor), Just(BinaryOp::Or)],
                inner.clone(),
                inner.clone(),
            )
                .prop_map(|(op, l, r)| Expr::new(
                    ExprKind::Binary {
                        op,
                        left: Box::new(l),
                        right: Box::new(r),
                    },
                    Span::default(),
                )),
            inner.prop_map(|operand| Expr::new(
                ExprKind::Unary {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                },
                Span::default(),
            )),
        ]
    })
}

fn assign_dint_strategy() -> impl Strategy<Value = Stmt> {
    (
        prop_oneof![Just("a"), Just("b"), Just("c")],
        dint_expr_strategy(),
    )
        .prop_map(|(target, value)| {
            Stmt::new(
                NodeId::new("s"),
                StmtKind::Assign {
                    target: LValue::Var {
                        name: target.to_owned(),
                        span: Span::default(),
                    },
                    value,
                },
                Span::default(),
            )
        })
}

fn assign_bool_strategy() -> impl Strategy<Value = Stmt> {
    (
        prop_oneof![Just("flag"), Just("MotorOut")],
        bool_expr_strategy(),
    )
        .prop_map(|(target, value)| {
            Stmt::new(
                NodeId::new("s"),
                StmtKind::Assign {
                    target: LValue::Var {
                        name: target.to_owned(),
                        span: Span::default(),
                    },
                    value,
                },
                Span::default(),
            )
        })
}

fn stmt_strategy() -> impl Strategy<Value = Stmt> {
    let assign_dint = assign_dint_strategy();
    let assign_bool = assign_bool_strategy();
    let fb_invoke = (bool_expr_strategy(), 1i64..=10_000_000i64).prop_map(|(inp, pt_us)| {
        Stmt::new(
            NodeId::new("s"),
            StmtKind::FbInvoke {
                instance: LValue::Var {
                    name: "Ton1".to_owned(),
                    span: Span::default(),
                },
                inputs: vec![
                    ("IN".to_owned(), inp),
                    (
                        "PT".to_owned(),
                        Expr::new(ExprKind::Literal(Literal::TimeUs(pt_us)), Span::default()),
                    ),
                ],
            },
            Span::default(),
        )
    });
    let if_stmt = (
        bool_expr_strategy(),
        prop::collection::vec(assign_dint_strategy(), 0..=2),
        prop::collection::vec(assign_bool_strategy(), 0..=2),
    )
        .prop_map(|(condition, then_body, else_body)| {
            Stmt::new(
                NodeId::new("s"),
                StmtKind::If {
                    condition,
                    then_body,
                    elif_arms: Vec::new(),
                    else_body,
                },
                Span::default(),
            )
        });
    let case_stmt = (
        0i128..=5i128,
        6i128..=10i128,
        assign_dint_strategy(),
        assign_dint_strategy(),
    )
        .prop_map(|(lo, hi, b1, b2)| {
            Stmt::new(
                NodeId::new("s"),
                StmtKind::Switch {
                    scrutinee: Expr::new(ExprKind::Ident("a".to_owned()), Span::default()),
                    cases: vec![
                        SwitchCase {
                            patterns: vec![CasePattern::Value(Expr::new(
                                ExprKind::Literal(Literal::Int(lo)),
                                Span::default(),
                            ))],
                            body: vec![b1],
                        },
                        SwitchCase {
                            patterns: vec![CasePattern::Range {
                                start: Expr::new(
                                    ExprKind::Literal(Literal::Int(lo + 1)),
                                    Span::default(),
                                ),
                                end: Expr::new(
                                    ExprKind::Literal(Literal::Int(hi)),
                                    Span::default(),
                                ),
                            }],
                            body: vec![b2],
                        },
                    ],
                    default: None,
                },
                Span::default(),
            )
        });
    let for_stmt = (1i128..=4i128, assign_dint_strategy()).prop_map(|(step, body_stmt)| {
        Stmt::new(
            NodeId::new("s"),
            StmtKind::For {
                counter: "i".to_owned(),
                from: Expr::new(ExprKind::Literal(Literal::Int(0)), Span::default()),
                to: Expr::new(ExprKind::Literal(Literal::Int(10)), Span::default()),
                step: Expr::new(ExprKind::Literal(Literal::Int(step)), Span::default()),
                inclusive: true,
                body: vec![body_stmt],
            },
            Span::default(),
        )
    });
    let while_stmt =
        (bool_expr_strategy(), assign_dint_strategy()).prop_map(|(cond, body_stmt)| {
            Stmt::new(
                NodeId::new("s"),
                StmtKind::While {
                    condition: cond,
                    body: vec![
                        body_stmt,
                        Stmt::new(NodeId::new("sb"), StmtKind::Break, Span::default()),
                    ],
                },
                Span::default(),
            )
        });
    prop_oneof![
        assign_dint,
        assign_bool,
        fb_invoke,
        if_stmt,
        case_stmt,
        for_stmt,
        while_stmt,
    ]
}

fn comment_text_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just('a'),
            Just('b'),
            Just('Z'),
            Just('0'),
            Just('9'),
            Just(' '),
            Just('_'),
            Just('-'),
            Just('.'),
            Just(':'),
            Just(','),
        ],
        1..=24,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn st_bundle_strategy() -> impl Strategy<Value = BoardProgramBundle> {
    (
        prop::collection::vec(stmt_strategy(), 1..=5),
        prop::collection::vec(
            (
                comment_text_strategy(),
                prop_oneof![
                    Just(CommentStyle::Line),
                    Just(CommentStyle::ParenBlock),
                    Just(CommentStyle::Block),
                ],
            ),
            0..=3,
        ),
        prop_oneof![Just(EolStyle::Lf), Just(EolStyle::Crlf)],
    )
        .prop_map(|(mut stmts, raw_comments, eol_style)| {
            for (idx, s) in stmts.iter_mut().enumerate() {
                s.id = NodeId::new(format!("stmt_{idx}"));
            }
            let pou_id = NodeId::new("pou_main");
            let comments: Vec<Comment> = raw_comments
                .into_iter()
                .enumerate()
                .map(|(idx, (text, style))| {
                    let (target_id, placement) = match stmts.get(idx) {
                        Some(stmt) => (stmt.id.clone(), AstCommentPlacement::Leading),
                        None => (pou_id.clone(), AstCommentPlacement::Dangling),
                    };
                    Comment {
                        id: format!("c_{idx}"),
                        anchor: CommentAnchor::Node { id: target_id },
                        placement,
                        token_ordinal: None,
                        text: format!(" {text} "),
                        origin: CommentOrigin {
                            language: "st".to_owned(),
                            style,
                        },
                        span: Span::default(),
                    }
                })
                .collect();

            BoardProgramBundle {
                program: BoardProgram {
                    imports: vec![],
                    hardware: vec![
                        HardwareBinding {
                            id: NodeId::new("hw_in"),
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
                            id: NodeId::new("hw_out"),
                            label: "MotorOut".to_owned(),
                            pin: "Qa.0".to_owned(),
                            kind: HardwareKind::DigitalOut,
                            pull: PullMode::None,
                            active_low: false,
                            codec: None,
                            range: None,
                            span: Span::default(),
                        },
                    ],
                    types: vec![],
                    data: vec![],
                    modules: vec![],
                    pous: vec![Pou {
                        id: pou_id,
                        name: "OB_MAIN".to_owned(),
                        kind: PouKind::Ob { ob: ObKind::Main },
                        preferred_view: None,
                        params: vec![],
                        locals: vec![
                            DataDecl {
                                id: NodeId::new("d_a"),
                                name: "a".to_owned(),
                                scope: StorageScope::Var,
                                ty: TypeRef::scalar(ScalarType::Dint),
                                initial: Some(Expr::new(
                                    ExprKind::Literal(Literal::Int(1)),
                                    Span::default(),
                                )),
                                span: Span::default(),
                            },
                            DataDecl {
                                id: NodeId::new("d_b"),
                                name: "b".to_owned(),
                                scope: StorageScope::Var,
                                ty: TypeRef::scalar(ScalarType::Dint),
                                initial: Some(Expr::new(
                                    ExprKind::Literal(Literal::Int(2)),
                                    Span::default(),
                                )),
                                span: Span::default(),
                            },
                            DataDecl {
                                id: NodeId::new("d_c"),
                                name: "c".to_owned(),
                                scope: StorageScope::Var,
                                ty: TypeRef::scalar(ScalarType::Dint),
                                initial: Some(Expr::new(
                                    ExprKind::Literal(Literal::Int(3)),
                                    Span::default(),
                                )),
                                span: Span::default(),
                            },
                            DataDecl {
                                id: NodeId::new("d_i"),
                                name: "i".to_owned(),
                                scope: StorageScope::Temp,
                                ty: TypeRef::scalar(ScalarType::Dint),
                                initial: None,
                                span: Span::default(),
                            },
                            DataDecl {
                                id: NodeId::new("d_flag"),
                                name: "flag".to_owned(),
                                scope: StorageScope::Var,
                                ty: TypeRef::scalar(ScalarType::Bool),
                                initial: Some(Expr::new(
                                    ExprKind::Literal(Literal::Bool(false)),
                                    Span::default(),
                                )),
                                span: Span::default(),
                            },
                            DataDecl {
                                id: NodeId::new("d_ton"),
                                name: "Ton1".to_owned(),
                                scope: StorageScope::Var,
                                ty: TypeRef::Fb {
                                    fb_kind: FbKind::Ton,
                                },
                                initial: None,
                                span: Span::default(),
                            },
                        ],
                        body: PouBody::Statements(stmts),
                        span: Span::default(),
                    }],
                },
                comments,
                assumptions: vec![],
                presentation: BoardPresentation {
                    eol_style,
                    blank_lines_before: std::collections::BTreeMap::new(),
                    origin_files: std::collections::BTreeMap::new(),
                    node_coords: std::collections::BTreeMap::new(),
                },
            }
        })
}

proptest! {
    #[test]
    fn st_lossless_asl_proptest(bundle in st_bundle_strategy()) {
        let ctx = plc_ctx();
        let frontend = StFrontend::new();
        let reparsed = verify_frontend_roundtrip(&frontend, &bundle, &ctx)
            .unwrap_or_else(|e| panic!("LOSSLESS_ASL proptest failed: {e}"));
        prop_assert_eq!(reparsed.presentation.eol_style, bundle.presentation.eol_style);
        prop_assert_eq!(assert_c5_comment_fields(&reparsed.comments), Ok(()));
    }
}

#[test]
fn st_hw_int_ob_name_roundtrips_edge_and_priority() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();
    let src = r#"
VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    edge_cnt : DINT := 0;
END_VAR

PROGRAM OB_HW_StartBtn_FALLING_3
    edge_cnt := edge_cnt + 1;
END_PROGRAM
"#;
    let parsed = frontend.parse(src, &ctx).unwrap();
    assert_eq!(
        parsed.value.program.pous[0].kind,
        PouKind::Ob {
            ob: ObKind::HwInt {
                pin_label: "StartBtn".to_owned(),
                edge: InterruptEdge::Falling,
                priority: 3,
            }
        }
    );
    let _reparsed = verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).unwrap();
}

#[test]
fn st_elif_arm_and_case_with_default_roundtrip() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();
    let bundle = BoardProgramBundle::new(BoardProgram {
        pous: vec![Pou {
            id: NodeId::new("p1"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![
                DataDecl {
                    id: NodeId::new("d1"),
                    name: "x".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Dint),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(0)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("d2"),
                    name: "y".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Dint),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(0)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
            ],
            body: PouBody::Statements(vec![Stmt::new(
                NodeId::new("s_if"),
                StmtKind::If {
                    condition: Expr::new(
                        ExprKind::Binary {
                            op: BinaryOp::Eq,
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
                    ),
                    then_body: vec![Stmt::new(
                        NodeId::new("s_t"),
                        StmtKind::Assign {
                            target: LValue::Var {
                                name: "y".to_owned(),
                                span: Span::default(),
                            },
                            value: Expr::new(ExprKind::Literal(Literal::Int(10)), Span::default()),
                        },
                        Span::default(),
                    )],
                    elif_arms: vec![ElifArm {
                        condition: Expr::new(
                            ExprKind::Binary {
                                op: BinaryOp::Eq,
                                left: Box::new(Expr::new(
                                    ExprKind::Ident("x".to_owned()),
                                    Span::default(),
                                )),
                                right: Box::new(Expr::new(
                                    ExprKind::Literal(Literal::Int(2)),
                                    Span::default(),
                                )),
                            },
                            Span::default(),
                        ),
                        body: vec![Stmt::new(
                            NodeId::new("s_e"),
                            StmtKind::Assign {
                                target: LValue::Var {
                                    name: "y".to_owned(),
                                    span: Span::default(),
                                },
                                value: Expr::new(
                                    ExprKind::Literal(Literal::Int(20)),
                                    Span::default(),
                                ),
                            },
                            Span::default(),
                        )],
                    }],
                    else_body: vec![Stmt::new(
                        NodeId::new("s_el"),
                        StmtKind::Assign {
                            target: LValue::Var {
                                name: "y".to_owned(),
                                span: Span::default(),
                            },
                            value: Expr::new(ExprKind::Literal(Literal::Int(30)), Span::default()),
                        },
                        Span::default(),
                    )],
                },
                Span::default(),
            )]),
            span: Span::default(),
        }],
        ..BoardProgram::default()
    });
    let _reparsed = verify_frontend_roundtrip(&frontend, &bundle, &ctx).unwrap();
}

// ---------------------------------------------------------------------------
// 15. Cross-Crate `dendri-validate` Compatibility Verification
// ---------------------------------------------------------------------------

#[test]
fn st_parsed_program_passes_dendri_validate() {
    use dendri_ast::{AuthoringLanguage, Board, BoardLayout, ExecutionKind, Project, ProjectMeta};
    use dendri_validate::{MemoryProfileCatalog, ValidateOptions, validate_project};

    let ctx = plc_ctx();
    let frontend = StFrontend::new();
    let src = r#"
(* Conveyor control program for dendri-validate compatibility check *)
USING std.iec.*;

TYPE
    RunMode : (IDLE := 0, RUN := 1, FAULT := 2);
    Limits : STRUCT
        hi : INT;
        lo : INT;
    END_STRUCT;
END_TYPE

VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
    PressRaw AT %IW64 : INT;
END_VAR

VAR_GLOBAL RETAIN
    cycle_count : DINT := 0;
END_VAR

FUNCTION PickLimit : INT
VAR_INPUT
    mode : INT;
    lim : Limits;
END_VAR
    CASE mode OF
        0:
            PickLimit := lim.lo;
            RETURN;
        1..2:
            PickLimit := lim.hi;
            RETURN;
        ELSE
            PickLimit := 0;
    END_CASE;
END_FUNCTION

PROGRAM OB_MAIN
VAR
    ton_run : TON;
    lim : Limits;
    target : INT := 0;
    clamped : DINT := 0;
    motor_cmd : BOOL := FALSE;
END_VAR
    IF lim.hi > 0 THEN
        target := PickLimit(1, lim) + 1;
    END_IF;
    clamped := LIMIT(0, cycle_count, 1000);
    ton_run(IN := StartBtn, PT := T#250ms);
    motor_cmd := ton_run.Q;
    IF motor_cmd THEN
        cycle_count := clamped + 1;
    END_IF;
END_PROGRAM
"#;

    let parsed = frontend.parse(src, &ctx).unwrap();
    // Verify LOSSLESS_ASL including CASE-arm Function Return fusion!
    let _reparsed = verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).unwrap();

    // Wrap into a full v2 Project and run dendri-validate!
    let project = Project {
        meta: ProjectMeta {
            schema_version: "2.0".to_owned(),
            asl_version: "2.0".to_owned(),
            requires: vec!["asl-core@1".to_owned()],
            name: "st-validate-check".to_owned(),
            author: String::new(),
            description: String::new(),
        },
        libraries: Vec::new(),
        packs: Vec::new(),
        boards: vec![Board {
            id: "plc_main".to_owned(),
            profile: "plc-1200-open-1214".to_owned(),
            authoring_language: AuthoringLanguage::St,
            execution: ExecutionKind::PlcScan,
            scan_period_us: Some(10_000),
            layout: BoardLayout::default(),
            program: parsed.value.program,
            comments: parsed.value.comments,
            assumptions: Vec::new(),
            presentation: parsed.value.presentation,
            span: Span::default(),
        }],
        components: Vec::new(),
        wires: Vec::new(),
        comments: Vec::new(),
    };

    let catalog = MemoryProfileCatalog::builtin_reference();
    let bag = validate_project(&project, &catalog, &ValidateOptions::default());
    assert!(
        !bag.has_errors(),
        "parsed ST project must pass dendri-validate with 0 errors, got: {bag:?}",
    );
}

// ---------------------------------------------------------------------------
// 16. Dialect Rejection & Frontend Dialect Override Verification (`DEF-006`)
// ---------------------------------------------------------------------------

#[test]
fn st_dialect_rejection_and_frontend_override() {
    let default_ctx = plc_ctx(); // st_dialect = Iec61131Ed3

    // ClassicPlc accepts `//` and `(* ... *)`, rejects C-style `/* ... */`
    let classic = StFrontend::with_dialect(StCommentDialect::ClassicPlc);
    let ok_classic = "PROGRAM OB_MAIN\n    // ok\n    (* ok *)\n    RETURN;\nEND_PROGRAM\n";
    assert!(classic.parse(ok_classic, &default_ctx).is_ok());
    let bad_classic =
        "PROGRAM OB_MAIN\n    /* forbidden in ClassicPlc */\n    RETURN;\nEND_PROGRAM\n";
    assert!(
        classic.parse(bad_classic, &default_ctx).is_err(),
        "ClassicPlc must reject /* ... */"
    );

    // StrictEd2 accepts `(* ... *)`, rejects `//`, `/* ... */`, AND nested `(* (* *) *)`
    let strict = StFrontend::with_dialect(StCommentDialect::StrictEd2);
    let ok_strict = "PROGRAM OB_MAIN\n    (* ok ed2 *)\n    RETURN;\nEND_PROGRAM\n";
    assert!(strict.parse(ok_strict, &default_ctx).is_ok());

    let bad_line = "PROGRAM OB_MAIN\n    // forbidden in StrictEd2\n    RETURN;\nEND_PROGRAM\n";
    assert!(
        strict.parse(bad_line, &default_ctx).is_err(),
        "StrictEd2 must reject //"
    );

    let bad_c_block =
        "PROGRAM OB_MAIN\n    /* forbidden in StrictEd2 */\n    RETURN;\nEND_PROGRAM\n";
    assert!(
        strict.parse(bad_c_block, &default_ctx).is_err(),
        "StrictEd2 must reject /* ... */"
    );

    let bad_nested =
        "PROGRAM OB_MAIN\n    (* outer (* inner *) outer *)\n    RETURN;\nEND_PROGRAM\n";
    assert!(
        strict.parse(bad_nested, &default_ctx).is_err(),
        "StrictEd2 must not allow nested (* ... *)"
    );

    // Meanwhile RuSTy and Iec61131Ed3 DO accept nested `(* outer (* inner *) outer *)` verbatim!
    for dialect in [StCommentDialect::Iec61131Ed3, StCommentDialect::RuSTy] {
        let fe = StFrontend::with_dialect(dialect);
        let parsed = fe.parse(bad_nested, &default_ctx).unwrap();
        assert_eq!(parsed.value.comments.len(), 1);
        assert_eq!(parsed.value.comments[0].text, " outer (* inner *) outer ");
        let emitted = fe.emit(&parsed.value, &default_ctx).unwrap();
        assert!(emitted.value.contains("(* outer (* inner *) outer *)"));
    }

    // Symmetric untyped literal comparison & arithmetic (`0 < x`, `1 + x`) in ST sema
    let sym_src = r#"
PROGRAM OB_MAIN
VAR
    x : INT := 5;
    w : WORD := WORD#16#00FF;
    ok : BOOL := FALSE;
END_VAR
    IF 0 < x THEN
        x := 1 + x;
        w := 16#0F BIT_AND w;
        ok := TRUE;
    END_IF;
END_PROGRAM
"#;
    let fe = StFrontend::new();
    let parsed_sym = fe.parse(sym_src, &default_ctx).unwrap();
    let _ = verify_frontend_roundtrip(&fe, &parsed_sym.value, &default_ctx).unwrap();
}

// ---------------------------------------------------------------------------
// 17. Verifier Regressions (#43 Fixes 1–6)
// ---------------------------------------------------------------------------

#[test]
fn st_regression_1_negative_typed_literals_roundtrip_all_signed_and_float_types() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();
    let src = r#"
FUNCTION NegInt : INT
VAR_INPUT
    a : INT;
    r : REAL;
END_VAR
    IF a < INT#-5 THEN
        NegInt := INT#-9;
        RETURN;
    END_IF;
    IF REAL#-2.0 < r THEN
        NegInt := a + INT#-1;
        RETURN;
    END_IF;
    NegInt := INT#-3;
END_FUNCTION

FUNCTION NegReal : REAL
VAR_INPUT
    v : REAL;
END_VAR
    NegReal := v + REAL#-1.5;
END_FUNCTION

FUNCTION_BLOCK OffsetFb
VAR_INPUT
    delta : INT;
    gain : REAL;
END_VAR
VAR_OUTPUT
    scaled : REAL;
END_VAR
    scaled := gain + REAL#-0.25;
END_FUNCTION_BLOCK

PROGRAM OB_MAIN
VAR
    s : SINT := SINT#-4;
    i : INT := INT#-10;
    r : REAL := REAL#-0.5;
    c : CTU;
    off : OffsetFb;
    flag : BOOL := FALSE;
END_VAR
    s := SINT#-7;
    i := INT#-5;
    r := REAL#-1.5;
    i := NegInt(INT#-2, REAL#-0.25);
    r := NegReal(REAL#-3.5);
    c(CU := flag, R := FALSE, PV := DINT#-1);
    off(delta := INT#-3, gain := REAL#-0.75);
    CASE i OF
        INT#-10..INT#-1:
            s := SINT#-2;
        ELSE
            s := SINT#-1;
    END_CASE;
    FOR i := INT#10 TO INT#-2 BY INT#-2 DO
        r := r + REAL#-0.5;
    END_FOR;
    FOR s := SINT#4 TO SINT#-4 BY SINT#-1 DO
        flag := TRUE;
    END_FOR;
END_PROGRAM
"#;

    let parsed = frontend.parse(src, &ctx).unwrap();
    let emitted = frontend.emit(&parsed.value, &ctx).unwrap().value;
    assert!(emitted.contains("s := SINT#-7;"), "emitted:\n{emitted}");
    assert!(emitted.contains("i := INT#-5;"), "emitted:\n{emitted}");
    assert!(emitted.contains("r := REAL#-1.5;"), "emitted:\n{emitted}");
    assert!(emitted.contains("BY INT#-2 DO"), "emitted:\n{emitted}");
    assert!(emitted.contains("BY SINT#-1 DO"), "emitted:\n{emitted}");
    assert!(emitted.contains("NegInt := INT#-9;"), "emitted:\n{emitted}");
    assert!(
        emitted.contains("off(delta := INT#-3, gain := REAL#-0.75);"),
        "emitted:\n{emitted}"
    );
    let _reparsed = verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).unwrap();
}

#[test]
fn st_regression_2_repeat_until_continue_checks_exit_and_preserves_nested_continue() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();
    let src = r#"
PROGRAM OB_MAIN
VAR
    x : DINT := 0;
    j : DINT := 0;
END_VAR
    REPEAT
        IF x = 0 THEN
            CONTINUE;
        END_IF;
        FOR j := 0 TO 2 DO
            IF j = 1 THEN
                CONTINUE;
            END_IF;
        END_FOR;
        x := x + 1;
    UNTIL TRUE
    END_REPEAT;
END_PROGRAM
"#;

    let parsed = frontend.parse(src, &ctx).unwrap();
    let PouBody::Statements(stmts) = &parsed.value.program.pous[0].body else {
        panic!("expected statements");
    };
    let StmtKind::While { body, .. } = &stmts[0].kind else {
        panic!("REPEAT must lower to While");
    };
    // Outer `IF x = 0 THEN CONTINUE; END_IF;` must have `IF TRUE THEN EXIT; END_IF;` immediately before `CONTINUE;`
    let StmtKind::If { then_body, .. } = &body[0].kind else {
        panic!("expected outer IF");
    };
    assert_eq!(
        then_body.len(),
        2,
        "outer CONTINUE must be preceded by UNTIL guard"
    );
    assert!(matches!(
        &then_body[0].kind,
        StmtKind::If {
            condition: Expr { kind: ExprKind::Literal(Literal::Bool(true)), .. },
            then_body: inner_then,
            ..
        } if matches!(inner_then.first().map(|s| &s.kind), Some(StmtKind::Break))
    ));
    assert!(matches!(then_body[1].kind, StmtKind::Continue));

    // Nested `FOR j := 0 TO 2` CONTINUE must remain untouched (1 stmt, NOT rewritten with outer UNTIL)
    let StmtKind::For { body: for_body, .. } = &body[1].kind else {
        panic!("expected nested FOR");
    };
    let StmtKind::If {
        then_body: inner_for_then,
        ..
    } = &for_body[0].kind
    else {
        panic!("expected inner IF");
    };
    assert_eq!(
        inner_for_then.len(),
        1,
        "nested loop CONTINUE must NOT be rewritten with outer REPEAT UNTIL check"
    );
    assert!(matches!(inner_for_then[0].kind, StmtKind::Continue));

    // Full LOSSLESS_ASL roundtrip of lowered While form
    let _reparsed = verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).unwrap();
}

#[test]
fn st_regression_3_for_exclusive_negative_and_nonconst_step() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    let bundle = BoardProgramBundle::new(BoardProgram {
        pous: vec![Pou {
            id: NodeId::new("pou_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![
                DataDecl {
                    id: NodeId::new("v_i"),
                    name: "i".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Int),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(0)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("v_s"),
                    name: "s".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Int),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(-2)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("v_acc"),
                    name: "acc".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Int),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(0)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
            ],
            body: PouBody::Statements(vec![
                // 1. Exclusive negative const step: 10 down to 0 (exclusive) -> TO (0 + 1) BY INT#-1
                Stmt::new(
                    NodeId::new("for_neg"),
                    StmtKind::For {
                        counter: "i".to_owned(),
                        from: Expr::new(ExprKind::Literal(Literal::Int(10)), Span::default()),
                        to: Expr::new(ExprKind::Literal(Literal::Int(0)), Span::default()),
                        step: Expr::new(ExprKind::Literal(Literal::Int(-1)), Span::default()),
                        inclusive: false,
                        body: vec![Stmt::new(
                            NodeId::new("s1"),
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
                                            ExprKind::Ident("i".to_owned()),
                                            Span::default(),
                                        )),
                                    },
                                    Span::default(),
                                ),
                            },
                            Span::default(),
                        )],
                    },
                    Span::default(),
                ),
                // 2. Exclusive positive const step: 0 to 10 (exclusive) BY 2 -> TO (10 - 1) BY 2
                Stmt::new(
                    NodeId::new("for_pos"),
                    StmtKind::For {
                        counter: "i".to_owned(),
                        from: Expr::new(ExprKind::Literal(Literal::Int(0)), Span::default()),
                        to: Expr::new(ExprKind::Literal(Literal::Int(10)), Span::default()),
                        step: Expr::new(ExprKind::Literal(Literal::Int(2)), Span::default()),
                        inclusive: false,
                        body: vec![],
                    },
                    Span::default(),
                ),
                // 3. Exclusive unknown-sign non-const step `s` with CONTINUE -> WHILE loop
                Stmt::new(
                    NodeId::new("for_dyn"),
                    StmtKind::For {
                        counter: "i".to_owned(),
                        from: Expr::new(ExprKind::Literal(Literal::Int(10)), Span::default()),
                        to: Expr::new(ExprKind::Literal(Literal::Int(0)), Span::default()),
                        step: Expr::new(ExprKind::Ident("s".to_owned()), Span::default()),
                        inclusive: false,
                        body: vec![Stmt::new(
                            NodeId::new("s_if"),
                            StmtKind::If {
                                condition: Expr::new(
                                    ExprKind::Binary {
                                        op: BinaryOp::Eq,
                                        left: Box::new(Expr::new(
                                            ExprKind::Ident("i".to_owned()),
                                            Span::default(),
                                        )),
                                        right: Box::new(Expr::new(
                                            ExprKind::Literal(Literal::Int(4)),
                                            Span::default(),
                                        )),
                                    },
                                    Span::default(),
                                ),
                                then_body: vec![Stmt::new(
                                    NodeId::new("s_cont"),
                                    StmtKind::Continue,
                                    Span::default(),
                                )],
                                elif_arms: vec![],
                                else_body: vec![],
                            },
                            Span::default(),
                        )],
                    },
                    Span::default(),
                ),
            ]),
            span: Span::default(),
        }],
        ..BoardProgram::default()
    });

    let emitted = frontend.emit(&bundle, &ctx).unwrap().value;
    assert!(
        emitted.contains("FOR i := 10 TO (0 + 1) BY INT#-1 DO"),
        "expected `(0 + 1) BY INT#-1`, got:\n{emitted}"
    );
    assert!(
        emitted.contains("FOR i := 0 TO (10 - 1) BY 2 DO"),
        "expected `(10 - 1) BY 2`, got:\n{emitted}"
    );
    assert!(
        emitted.contains("WHILE ((s > 0) AND (i < 0)) OR ((s < 0) AND (i > 0)) DO"),
        "expected signed-direction WHILE lowering, got:\n{emitted}"
    );
    assert!(
        emitted.contains("i := i + s;\n            CONTINUE;"),
        "expected counter update before CONTINUE in WHILE lowering, got:\n{emitted}"
    );
    assert!(
        frontend.parse(&emitted, &ctx).is_ok(),
        "emitted ST must parse cleanly:\n{emitted}"
    );
}

#[test]
fn st_regression_4_nontail_function_return_assignments_and_bare_return() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();
    let src = r#"
FUNCTION ClampPos : INT
VAR_INPUT
    x : INT;
END_VAR
    ClampPos := 0;
    IF x > 0 THEN
        ClampPos := x;
    END_IF;
END_FUNCTION

FUNCTION EarlyExit : INT
VAR_INPUT
    x : INT;
END_VAR
    EarlyExit := 42;
    IF x = 0 THEN
        RETURN;
    END_IF;
    EarlyExit := x;
END_FUNCTION

FUNCTION SumTo : INT
VAR_INPUT
    n : INT;
END_VAR
VAR
    _ret : INT := 0;
    i : INT := 0;
END_VAR
    SumTo := 0;
    FOR i := 1 TO n DO
        SumTo := SumTo + i;
    END_FOR;
END_FUNCTION

PROGRAM OB_MAIN
VAR
    a : INT := 0;
    b : INT := 0;
    c : INT := 0;
END_VAR
    a := ClampPos(5);
    b := EarlyExit(0);
    c := SumTo(4);
END_PROGRAM
"#;

    let parsed = frontend.parse(src, &ctx).unwrap();
    let emitted = frontend.emit(&parsed.value, &ctx).unwrap().value;

    // Must NOT emit a premature `ClampPos := 0;\n    RETURN;` before `IF x > 0`!
    assert!(
        !emitted.contains("ClampPos := 0;\n    RETURN;"),
        "non-tail return assignment must not insert premature RETURN:\n{emitted}"
    );
    assert!(
        emitted.contains("_ret := 0;\n    IF x > 0 THEN\n        _ret := x;"),
        "emitted:\n{emitted}"
    );
    assert!(emitted.contains("ClampPos := _ret;"), "emitted:\n{emitted}");
    // Bare `RETURN;` inside EarlyExit must return the slot `_ret`!
    assert!(
        emitted.contains("EarlyExit := _ret;\n        RETURN;"),
        "emitted:\n{emitted}"
    );
    // Collision-avoidance: SumTo already declared `_ret`, so slot must be `_ret_1`!
    assert!(emitted.contains("_ret_1 : INT;"), "emitted:\n{emitted}");
    assert!(emitted.contains("SumTo := _ret_1;"), "emitted:\n{emitted}");

    // Full LOSSLESS_ASL round-trip on the lowered program!
    let _reparsed = verify_frontend_roundtrip(&frontend, &parsed.value, &ctx).unwrap();
}

#[test]
fn st_regression_5_f1_pack_expr_call_validates_arity_types_and_undeclared() {
    let mut ctx = plc_ctx();
    let custom_pack_toml = r#"
schema_version = "2.0"

[pack]
id = "vendor-crc-helper"
name = "Vendor CRC Helper"
version = "1.0.0"
language = "st"
fidelity = "f1_declared"
standards = ["IEC 61131-3"]

[[symbols]]
name = "CRC16_CALC"
kind = "function"
return_ty = "WORD"
params = [
    { name = "data", ty = "WORD" },
    { name = "len", ty = "UINT" },
]
"#;
    let pack = PackManifest::parse_str(custom_pack_toml).unwrap();
    ctx.loaded_packs.push(pack);

    let frontend = StFrontend::new();

    // 1. Arity mismatch + undeclared identifier in expression position
    let bad_arity_and_undeclared = r#"
PROGRAM OB_MAIN
VAR
    w : WORD := WORD#0;
END_VAR
    w := CRC16_CALC(TRUE, ghost_var, 123);
END_PROGRAM
"#;
    let errs = frontend.parse(bad_arity_and_undeclared, &ctx).unwrap_err();
    assert!(
        errs.iter().any(|d| d.code == ErrorCode::ErrArityMismatch),
        "expected ERR_ARITY_MISMATCH, got: {errs:?}"
    );
    assert!(
        errs.iter()
            .any(|d| d.code == ErrorCode::ErrUndeclaredSymbol),
        "expected ERR_UNDECLARED_SYMBOL for ghost_var, got: {errs:?}"
    );

    // 2. Argument type mismatch in expression position
    let bad_type = r#"
PROGRAM OB_MAIN
VAR
    w : WORD := WORD#0;
END_VAR
    w := CRC16_CALC(TRUE, UINT#4);
END_PROGRAM
"#;
    assert_parse_error(bad_type, &ctx, ErrorCode::ErrTypeMismatch);

    // 3. Undeclared symbol with matching arity
    let bad_undecl = r#"
PROGRAM OB_MAIN
VAR
    w : WORD := WORD#0;
END_VAR
    w := CRC16_CALC(ghost_var, UINT#4);
END_PROGRAM
"#;
    assert_parse_error(bad_undecl, &ctx, ErrorCode::ErrUndeclaredSymbol);

    // 4. Valid call still succeeds cleanly
    let good = r#"
PROGRAM OB_MAIN
VAR
    w : WORD := WORD#0;
END_VAR
    w := CRC16_CALC(WORD#16#1234, UINT#4);
END_PROGRAM
"#;
    assert!(frontend.parse(good, &ctx).is_ok());
}

#[test]
fn st_regression_6_derived_ladder_sorting_midrung_reset_fb_pins_and_parallel_or() {
    let ctx = plc_ctx();
    let frontend = StFrontend::new();

    let bundle = BoardProgramBundle::new(BoardProgram {
        hardware: vec![
            HardwareBinding {
                id: NodeId::new("hw1"),
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
                id: NodeId::new("hw2"),
                label: "MotorOut".to_owned(),
                pin: "Qa.0".to_owned(),
                kind: HardwareKind::DigitalOut,
                pull: PullMode::None,
                active_low: false,
                codec: None,
                range: None,
                span: Span::default(),
            },
        ],
        pous: vec![Pou {
            id: NodeId::new("lad_pou"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![
                // Deliberately declare `_rail` to verify collision-guarded VAR_TEMP naming!
                DataDecl {
                    id: NodeId::new("v_rail"),
                    name: "_rail".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Bool),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Bool(false)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("v_a"),
                    name: "a".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Bool),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Bool(true)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("v_b"),
                    name: "b".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Bool),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Bool(false)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("v_x"),
                    name: "x".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::scalar(ScalarType::Int),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Int(10)),
                        Span::default(),
                    )),
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("v_cnt"),
                    name: "cnt1".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::Fb {
                        fb_kind: FbKind::Ctu,
                    },
                    initial: None,
                    span: Span::default(),
                },
                DataDecl {
                    id: NodeId::new("v_sr"),
                    name: "latch1".to_owned(),
                    scope: StorageScope::Var,
                    ty: TypeRef::Fb {
                        fb_kind: FbKind::Sr,
                    },
                    initial: None,
                    span: Span::default(),
                },
            ],
            body: PouBody::Network(LadderNetwork {
                rungs: vec![
                    // Rung 0: Unsorted `[a] --(R a)-- (b)` — MUST sort by (col, row) and preserve pre-reset power flow!
                    LadderRung {
                        id: NodeId::new("r0"),
                        order: 0,
                        cells: vec![
                            LadderCell {
                                id: NodeId::new("r0_b"),
                                row: 0,
                                col: 2,
                                kind: LadderCellKind::Coil {
                                    kind: LadderCoilKind::Normal,
                                    target: LValue::Var {
                                        name: "b".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                            LadderCell {
                                id: NodeId::new("r0_reset"),
                                row: 0,
                                col: 1,
                                kind: LadderCellKind::Coil {
                                    kind: LadderCoilKind::Reset,
                                    target: LValue::Var {
                                        name: "a".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                            LadderCell {
                                id: NodeId::new("r0_contact"),
                                row: 0,
                                col: 0,
                                kind: LadderCellKind::Contact {
                                    kind: LadderContactKind::NormallyOpen,
                                    target: LValue::Var {
                                        name: "a".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                        ],
                    },
                    // Rung 1: ParallelOr with Compare + CTU + SR FB boxes (plus unsorted branch cells)
                    LadderRung {
                        id: NodeId::new("r1"),
                        order: 1,
                        cells: vec![
                            LadderCell {
                                id: NodeId::new("r1_par"),
                                row: 0,
                                col: 0,
                                kind: LadderCellKind::ParallelOr {
                                    branches: vec![
                                        // Branch 0: Compare box (`x > 5`)
                                        vec![LadderCell {
                                            id: NodeId::new("r1_cmp"),
                                            row: 0,
                                            col: 0,
                                            kind: LadderCellKind::Box(LadderBoxKind::Compare {
                                                op: LadderCompareOp::Gt,
                                                left: Expr::new(
                                                    ExprKind::Ident("x".to_owned()),
                                                    Span::default(),
                                                ),
                                                right: Expr::new(
                                                    ExprKind::Literal(Literal::Int(5)),
                                                    Span::default(),
                                                ),
                                            }),
                                        }],
                                        // Branch 1 (unsorted): Contact StartBtn (col 0) -> SR FB box (col 1)
                                        vec![
                                            LadderCell {
                                                id: NodeId::new("r1_sr"),
                                                row: 1,
                                                col: 1,
                                                kind: LadderCellKind::Box(LadderBoxKind::Fb {
                                                    kind: FbKind::Sr,
                                                    instance: LValue::Var {
                                                        name: "latch1".to_owned(),
                                                        span: Span::default(),
                                                    },
                                                    inputs: vec![(
                                                        "R".to_owned(),
                                                        Expr::new(
                                                            ExprKind::Literal(Literal::Bool(false)),
                                                            Span::default(),
                                                        ),
                                                    )],
                                                }),
                                            },
                                            LadderCell {
                                                id: NodeId::new("r1_start"),
                                                row: 1,
                                                col: 0,
                                                kind: LadderCellKind::Contact {
                                                    kind: LadderContactKind::NormallyOpen,
                                                    target: LValue::Var {
                                                        name: "StartBtn".to_owned(),
                                                        span: Span::default(),
                                                    },
                                                },
                                            },
                                        ],
                                    ],
                                },
                            },
                            LadderCell {
                                id: NodeId::new("r1_ctu"),
                                row: 0,
                                col: 1,
                                kind: LadderCellKind::Box(LadderBoxKind::Fb {
                                    kind: FbKind::Ctu,
                                    instance: LValue::Var {
                                        name: "cnt1".to_owned(),
                                        span: Span::default(),
                                    },
                                    inputs: vec![
                                        (
                                            "R".to_owned(),
                                            Expr::new(
                                                ExprKind::Literal(Literal::Bool(false)),
                                                Span::default(),
                                            ),
                                        ),
                                        (
                                            "PV".to_owned(),
                                            Expr::new(
                                                ExprKind::Literal(Literal::Int(3)),
                                                Span::default(),
                                            ),
                                        ),
                                    ],
                                }),
                            },
                            LadderCell {
                                id: NodeId::new("r1_out"),
                                row: 0,
                                col: 2,
                                kind: LadderCellKind::Coil {
                                    kind: LadderCoilKind::Normal,
                                    target: LValue::Var {
                                        name: "MotorOut".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                        ],
                    },
                ],
            }),
            span: Span::default(),
        }],
        ..BoardProgram::default()
    });

    let emitted = frontend.emit(&bundle, &ctx).unwrap().value;

    // 1. Collision guard: user declared `_rail`, so materialized rail temp MUST be `_rail_1`!
    assert!(emitted.contains("_rail_1 : BOOL;"), "emitted:\n{emitted}");
    // 2. Sorting & pre-reset preservation: Contact(a) before Reset(a) before Normal(b), all using `_rail_1`
    let pos_contact = emitted.find("_rail_1 := _rail_1 AND a;").unwrap();
    let pos_reset = emitted.find("IF _rail_1 THEN a := FALSE; END_IF;").unwrap();
    let pos_coil = emitted.find("b := _rail_1;").unwrap();
    assert!(
        pos_contact < pos_reset && pos_reset < pos_coil,
        "emitted:\n{emitted}"
    );
    // 3. Correct FB rail pins & outputs: SR uses S1/.Q1, CTU uses CU/.Q
    assert!(
        emitted.contains("latch1(S1 := _rail_br, R := FALSE);"),
        "emitted:\n{emitted}"
    );
    assert!(
        emitted.contains("_rail_br := latch1.Q1;"),
        "emitted:\n{emitted}"
    );
    assert!(
        emitted.contains("cnt1(CU := _rail_1, R := FALSE, PV := 3);"),
        "emitted:\n{emitted}"
    );
    assert!(
        emitted.contains("_rail_1 := cnt1.Q;"),
        "emitted:\n{emitted}"
    );
    // 4. Compare box inside ParallelOr is emitted (not dropped!)
    assert!(
        emitted.contains("_rail_br := _rail_br AND (x > 5);"),
        "emitted:\n{emitted}"
    );
    // 5. Full emitted ST parses cleanly via StFrontend::parse with ZERO errors!
    let reparsed = frontend.parse(&emitted, &ctx);
    assert!(
        reparsed.is_ok(),
        "derived Ladder ST must parse cleanly via StFrontend::parse, got: {:?}\nEmitted:\n{emitted}",
        reparsed.err()
    );
}
