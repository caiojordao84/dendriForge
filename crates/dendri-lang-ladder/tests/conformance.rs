//! Comprehensive conformance, `LOSSLESS_ASL` proptest, `C1–C5` hostile comment corpus,
//! conservative lifting, derived projection VM-parity, `Foreign` box preservation,
//! feature-matrix, and negative `ErrorCode` fixture tests for `dendri-lang-ladder`.

use dendri_ast::{
    AuthoringLanguage, BinaryOp, Board, BoardLayout, BoardPresentation, BoardProgram, CallArg,
    CasePattern, ChannelRange, Comment, CommentAnchor, CommentEncoding, CommentOrigin,
    CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, ElifArm, EnumVariant,
    EolStyle, ExecutionKind, Expr, ExprKind, FbKind, ForeignFidelity, HardwareBinding,
    HardwareKind, LValue, LadderBoxKind, LadderCell, LadderCellKind, LadderCoilKind,
    LadderCompareOp, LadderContactKind, LadderNetwork, LadderRung, Literal, ModuleDecl, NodeId,
    ObKind, ParamDecl, ParamDirection, Pou, PouBody, PouKind, PullMode, ScalarType, SignalKind,
    Stmt, StmtKind, StorageScope, StructField, SwitchCase, TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, FeatureMatrixCollector, LanguageFrontend, LanguageId,
    Paradigm, assert_c1_multiset_invariant, assert_c2_orphan_preservation,
    assert_c4_reimport_idempotence, assert_c5_comment_fields, assert_lossless_asl_exact_ids,
    contains_banned_magic_comment, is_four_part_diagnostic, verify_frontend_roundtrip,
    verify_hostile_corpus_on_frontend,
};
use dendri_lang_ladder::{
    LadderFrontend, feature_matrix_entries, lift_statements_to_network,
    lift_statements_to_network_with_fb_kinds, project_network_for_export,
    project_network_to_statements, project_network_with_decls, validate_ladder_bundle,
};
use dendri_span::{ErrorCode, Span};
use dendri_vm::{BoardVm, MemoryBoardIo, Value, VmConfig};
use proptest::prelude::*;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Test AST Helpers
// ---------------------------------------------------------------------------

fn lv(name: &str) -> LValue {
    LValue::Var {
        name: name.to_owned(),
        span: Span::default(),
    }
}

fn id_expr(name: &str) -> Expr {
    Expr::new(ExprKind::Ident(name.to_owned()), Span::default())
}

fn bool_expr(v: bool) -> Expr {
    Expr::new(ExprKind::Literal(Literal::Bool(v)), Span::default())
}

fn int_expr(v: i128) -> Expr {
    Expr::new(ExprKind::Literal(Literal::Int(v)), Span::default())
}

fn time_expr(us: i64) -> Expr {
    Expr::new(ExprKind::Literal(Literal::TimeUs(us)), Span::default())
}

fn bool_var(id: &str, name: &str, init: bool) -> DataDecl {
    DataDecl {
        id: NodeId::new(id),
        name: name.to_owned(),
        scope: StorageScope::Var,
        ty: TypeRef::scalar(ScalarType::Bool),
        initial: Some(bool_expr(init)),
        span: Span::default(),
    }
}

fn dint_var(id: &str, name: &str, init: i128) -> DataDecl {
    DataDecl {
        id: NodeId::new(id),
        name: name.to_owned(),
        scope: StorageScope::Var,
        ty: TypeRef::scalar(ScalarType::Dint),
        initial: Some(int_expr(init)),
        span: Span::default(),
    }
}

fn fb_var(id: &str, name: &str, fb_kind: FbKind) -> DataDecl {
    DataDecl {
        id: NodeId::new(id),
        name: name.to_owned(),
        scope: StorageScope::Var,
        ty: TypeRef::Fb { fb_kind },
        initial: None,
        span: Span::default(),
    }
}

fn make_board(program: BoardProgram) -> Board {
    Board {
        id: "plc_main".to_owned(),
        profile: "plc-1200-open-1214".to_owned(),
        authoring_language: AuthoringLanguage::Ladder,
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

fn make_projected_board(base_program: &BoardProgram, net: &LadderNetwork) -> Board {
    let proj = project_network_with_decls(net);
    let mut program = base_program.clone();
    program.data.extend(proj.edge_decls);
    if let Some(main_pou) = program.pous.first_mut() {
        main_pou.locals.extend(proj.rail_locals);
        main_pou.body = PouBody::Statements(proj.stmts);
    }
    make_board(program)
}

fn assert_no_synthetic_rail_temps_in_asl(emitted: &str) {
    assert!(
        !emitted.contains("__rail_"),
        "synthetic __rail_N temporary leaked into Ladder ASL:\n{emitted}"
    );
    assert!(
        !contains_banned_magic_comment(emitted),
        "ADR-020 banned magic comment detected:\n{emitted}"
    );
}

// ---------------------------------------------------------------------------
// 1. Structural Round-Trip (`LOSSLESS_ASL` + Exact NodeIds)
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_all_contacts_coils_wires_empty_and_nested_branches() {
    let fe = LadderFrontend::new();
    assert_eq!(fe.id(), LanguageId::Ladder);
    assert_eq!(fe.paradigm(), Paradigm::Ladder);
    let ctx = BoardContext::plc_1200_open_1214("plc_main");

    let net = LadderNetwork {
        rungs: vec![
            // Intentionally non-monotonic `order` to verify order preservation!
            LadderRung {
                id: NodeId::new("n_rung_10"),
                order: 10,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("n_br_outer"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::ParallelOr {
                            branches: vec![
                                vec![
                                    LadderCell {
                                        id: NodeId::new("n_no"),
                                        row: 0,
                                        col: 0,
                                        kind: LadderCellKind::Contact {
                                            kind: LadderContactKind::NormallyOpen,
                                            target: lv("start"),
                                        },
                                    },
                                    LadderCell {
                                        id: NodeId::new("n_nc"),
                                        row: 0,
                                        col: 1,
                                        kind: LadderCellKind::Contact {
                                            kind: LadderContactKind::NormallyClosed,
                                            target: lv("stop"),
                                        },
                                    },
                                ],
                                vec![LadderCell {
                                    id: NodeId::new("n_br_inner"),
                                    row: 1,
                                    col: 0,
                                    kind: LadderCellKind::ParallelOr {
                                        branches: vec![
                                            vec![LadderCell {
                                                id: NodeId::new("n_pos"),
                                                row: 1,
                                                col: 0,
                                                kind: LadderCellKind::Contact {
                                                    kind: LadderContactKind::PositiveTransition,
                                                    target: lv("pulse_in"),
                                                },
                                            }],
                                            vec![LadderCell {
                                                id: NodeId::new("n_neg"),
                                                row: 2,
                                                col: 0,
                                                kind: LadderCellKind::Contact {
                                                    kind: LadderContactKind::NegativeTransition,
                                                    target: lv("fall_in"),
                                                },
                                            }],
                                        ],
                                    },
                                }],
                            ],
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_wire"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Wire,
                    },
                    LadderCell {
                        id: NodeId::new("n_c_norm"),
                        row: 0,
                        col: 3,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("out_norm"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_c_neg"),
                        row: 0,
                        col: 4,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Negated,
                            target: lv("out_neg"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("n_rung_2"),
                order: 2,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("n_empty"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Empty,
                    },
                    LadderCell {
                        id: NodeId::new("n_c_set"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Set,
                            target: lv("out_set"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_c_rst"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Reset,
                            target: lv("out_rst"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_c_pos"),
                        row: 0,
                        col: 3,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::PositiveTransition,
                            target: lv("out_p"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_c_neg_t"),
                        row: 0,
                        col: 4,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::NegativeTransition,
                            target: lv("out_n"),
                        },
                    },
                ],
            },
        ],
    };

    let bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            bool_var("n_d0", "start", false),
            bool_var("n_d1", "stop", false),
            bool_var("n_d2", "pulse_in", false),
            bool_var("n_d3", "fall_in", false),
            bool_var("n_d4", "out_norm", false),
            bool_var("n_d5", "out_neg", false),
            bool_var("n_d6", "out_set", false),
            bool_var("n_d7", "out_rst", false),
            bool_var("n_d8", "out_p", false),
            bool_var("n_d9", "out_n", false),
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("n_ob_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: Some(AuthoringLanguage::Ladder),
            params: vec![],
            locals: vec![],
            body: PouBody::Network(net),
            span: Span::default(),
        }],
    });

    let emitted = fe.emit(&bundle, &ctx).expect("emit succeeded");
    assert_no_synthetic_rail_temps_in_asl(&emitted.value);

    let reparsed = verify_frontend_roundtrip(&fe, &bundle, &ctx).expect("LOSSLESS_ASL passed");
    assert_lossless_asl_exact_ids(&bundle.program, &reparsed.program)
        .expect("exact NodeIds preserved");
    assert!(
        validate_ladder_bundle(&reparsed, &ctx).is_empty(),
        "reparsed bundle must validate cleanly"
    );
}

#[test]
fn roundtrip_all_fb_compare_call_and_foreign_boxes() {
    let fe = LadderFrontend::new();
    let ctx = BoardContext::plc_1200_open_1214("plc_main");

    type FbCase = (&'static str, FbKind, Vec<(String, Expr)>);
    let all_fbs: Vec<FbCase> = vec![
        (
            "t_on",
            FbKind::Ton,
            vec![("PT".to_owned(), time_expr(500_000))],
        ),
        (
            "t_off",
            FbKind::Tof,
            vec![("PT".to_owned(), time_expr(250_000))],
        ),
        (
            "t_p",
            FbKind::Tp,
            vec![("PT".to_owned(), time_expr(100_000))],
        ),
        (
            "c_up",
            FbKind::Ctu,
            vec![
                ("R".to_owned(), bool_expr(false)),
                ("PV".to_owned(), int_expr(5)),
            ],
        ),
        (
            "c_dn",
            FbKind::Ctd,
            vec![
                ("LD".to_owned(), bool_expr(false)),
                ("PV".to_owned(), int_expr(3)),
            ],
        ),
        (
            "c_ud",
            FbKind::Ctud,
            vec![
                ("CD".to_owned(), bool_expr(false)),
                ("PV".to_owned(), int_expr(10)),
            ],
        ),
        ("rt", FbKind::RTrig, vec![]),
        ("ft", FbKind::FTrig, vec![]),
        (
            "latch_sr",
            FbKind::Sr,
            vec![("R".to_owned(), bool_expr(false))],
        ),
        (
            "latch_rs",
            FbKind::Rs,
            vec![("R1".to_owned(), bool_expr(false))],
        ),
        (
            "tx_u",
            FbKind::UartSend,
            vec![(
                "DATA".to_owned(),
                Expr::new(
                    ExprKind::Literal(Literal::String("OK".to_owned())),
                    Span::default(),
                ),
            )],
        ),
        ("rx_u", FbKind::UartRecv, vec![]),
        (
            "i2c_b",
            FbKind::I2cXfer,
            vec![("ADDR".to_owned(), int_expr(0x48))],
        ),
        (
            "spi_b",
            FbKind::SpiXfer,
            vec![("TX".to_owned(), int_expr(0xAA))],
        ),
        (
            "mb_c",
            FbKind::MbClient,
            vec![
                ("FC".to_owned(), int_expr(3)),
                ("ADDR".to_owned(), int_expr(40001)),
            ],
        ),
        ("mb_s", FbKind::MbServer, vec![]),
        (
            "user_ctrl",
            FbKind::User {
                fb_name: "MotorCtrl".to_owned(),
            },
            vec![],
        ),
    ];

    let mut data = vec![
        bool_var("n_flag", "en_flag", true),
        dint_var("n_a", "val_a", 10),
        dint_var("n_b", "val_b", 20),
        dint_var("n_res", "result", 0),
    ];
    let mut fb_cells = Vec::new();
    for (idx, (inst, kind, inputs)) in all_fbs.into_iter().enumerate() {
        data.push(fb_var(&format!("n_fb_decl_{idx}"), inst, kind.clone()));
        fb_cells.push(LadderCell {
            id: NodeId::new(format!("n_fb_cell_{idx}")),
            row: 0,
            col: u32::try_from(idx).unwrap(),
            kind: LadderCellKind::Box(LadderBoxKind::Fb {
                kind,
                instance: lv(inst),
                inputs,
            }),
        });
    }

    let cmp_ops = [
        LadderCompareOp::Eq,
        LadderCompareOp::Ne,
        LadderCompareOp::Lt,
        LadderCompareOp::Le,
        LadderCompareOp::Gt,
        LadderCompareOp::Ge,
    ];
    let mut cmp_cells = Vec::new();
    for (idx, op) in cmp_ops.into_iter().enumerate() {
        cmp_cells.push(LadderCell {
            id: NodeId::new(format!("n_cmp_{idx}")),
            row: 0,
            col: u32::try_from(idx).unwrap(),
            kind: LadderCellKind::Box(LadderBoxKind::Compare {
                op,
                left: id_expr("val_a"),
                right: id_expr("val_b"),
            }),
        });
    }

    let call_and_foreign_cells = vec![
        LadderCell {
            id: NodeId::new("n_call_add"),
            row: 0,
            col: 0,
            kind: LadderCellKind::Box(LadderBoxKind::Call {
                callee: "ADD".to_owned(),
                args: vec![(None, id_expr("val_a")), (None, id_expr("val_b"))],
                output: Some(lv("result")),
            }),
        },
        LadderCell {
            id: NodeId::new("n_call_fc"),
            row: 0,
            col: 1,
            kind: LadderCellKind::Box(LadderBoxKind::Call {
                callee: "HelperCalc".to_owned(),
                args: vec![(Some("x".to_owned()), id_expr("val_a"))],
                output: Some(lv("result")),
            }),
        },
        LadderCell {
            id: NodeId::new("n_call_void"),
            row: 0,
            col: 2,
            kind: LadderCellKind::Box(LadderBoxKind::Call {
                callee: "SubSheet".to_owned(),
                args: vec![],
                output: None,
            }),
        },
        LadderCell {
            id: NodeId::new("n_foreign_f0"),
            row: 0,
            col: 3,
            kind: LadderCellKind::Box(LadderBoxKind::Foreign {
                language: "cpp".to_owned(),
                fidelity: ForeignFidelity::F0,
                text: "PORTB |= (1 << 5);".to_owned(),
                pack_hint: None,
            }),
        },
        LadderCell {
            id: NodeId::new("n_foreign_f1"),
            row: 0,
            col: 4,
            kind: LadderCellKind::Box(LadderBoxKind::Foreign {
                language: "st".to_owned(),
                fidelity: ForeignFidelity::F1,
                text: "VENDOR_DIAG(1);".to_owned(),
                pack_hint: Some("plc-modbus-tcp".to_owned()),
            }),
        },
        LadderCell {
            id: NodeId::new("n_foreign_f2"),
            row: 0,
            col: 5,
            kind: LadderCellKind::Box(LadderBoxKind::Foreign {
                language: "ladder".to_owned(),
                fidelity: ForeignFidelity::F2,
                text: "MODELED_CUSTOM()".to_owned(),
                pack_hint: Some("custom-pack".to_owned()),
            }),
        },
    ];

    let bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![],
        types: vec![
            TypeDecl::Struct {
                id: NodeId::new("n_struct_cfg"),
                name: "MotorCfg".to_owned(),
                fields: vec![StructField {
                    name: "enabled".to_owned(),
                    ty: TypeRef::scalar(ScalarType::Bool),
                }],
                span: Span::default(),
            },
            TypeDecl::Enum {
                id: NodeId::new("n_enum_mode"),
                name: "RunMode".to_owned(),
                backing: ScalarType::Dint,
                variants: vec![
                    EnumVariant {
                        name: "Idle".to_owned(),
                        value: 0,
                    },
                    EnumVariant {
                        name: "Auto".to_owned(),
                        value: 1,
                    },
                ],
                span: Span::default(),
            },
        ],
        data,
        modules: vec![ModuleDecl {
            id: NodeId::new("n_mod_util"),
            name: "utils".to_owned(),
            imports: vec!["std.iec.math".to_owned()],
            exports: vec!["SubSheet".to_owned()],
            types: vec![],
            data: vec![],
            pous: vec![Pou {
                id: NodeId::new("n_sub_sheet"),
                name: "SubSheet".to_owned(),
                kind: PouKind::Fc {
                    return_type: TypeRef::Void,
                },
                preferred_view: Some(AuthoringLanguage::Ladder),
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork { rungs: vec![] }),
                span: Span::default(),
            }],
            span: Span::default(),
        }],
        pous: vec![
            Pou {
                id: NodeId::new("n_ob_main"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: Some(AuthoringLanguage::Ladder),
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork {
                    rungs: vec![
                        LadderRung {
                            id: NodeId::new("n_r_fbs"),
                            order: 0,
                            cells: fb_cells,
                        },
                        LadderRung {
                            id: NodeId::new("n_r_cmps"),
                            order: 1,
                            cells: cmp_cells,
                        },
                        LadderRung {
                            id: NodeId::new("n_r_calls"),
                            order: 2,
                            cells: call_and_foreign_cells,
                        },
                    ],
                }),
                span: Span::default(),
            },
            // Intact non-liftable `Structured POU` helper FC callable from Ladder!
            Pou {
                id: NodeId::new("n_fc_helper"),
                name: "HelperCalc".to_owned(),
                kind: PouKind::Fc {
                    return_type: TypeRef::scalar(ScalarType::Dint),
                },
                preferred_view: Some(AuthoringLanguage::St),
                params: vec![ParamDecl {
                    name: "x".to_owned(),
                    direction: ParamDirection::In,
                    ty: TypeRef::scalar(ScalarType::Dint),
                }],
                locals: vec![dint_var("n_loc_acc", "acc", 0)],
                body: PouBody::Statements(vec![
                    Stmt::new(
                        NodeId::new("n_st_while"),
                        StmtKind::While {
                            condition: Expr::new(
                                ExprKind::Binary {
                                    op: BinaryOp::Lt,
                                    left: Box::new(id_expr("acc")),
                                    right: Box::new(id_expr("x")),
                                },
                                Span::default(),
                            ),
                            body: vec![Stmt::new(
                                NodeId::new("n_st_inc"),
                                StmtKind::Assign {
                                    target: lv("acc"),
                                    value: Expr::new(
                                        ExprKind::Binary {
                                            op: BinaryOp::Add,
                                            left: Box::new(id_expr("acc")),
                                            right: Box::new(int_expr(1)),
                                        },
                                        Span::default(),
                                    ),
                                },
                                Span::default(),
                            )],
                        },
                        Span::default(),
                    ),
                    Stmt::new(
                        NodeId::new("n_st_ret"),
                        StmtKind::Return {
                            value: Some(id_expr("acc")),
                        },
                        Span::default(),
                    ),
                ]),
                span: Span::default(),
            },
            Pou {
                id: NodeId::new("n_fb_user_def"),
                name: "MotorCtrl".to_owned(),
                kind: PouKind::Fb,
                preferred_view: Some(AuthoringLanguage::Ladder),
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork { rungs: vec![] }),
                span: Span::default(),
            },
        ],
    });

    let reparsed = verify_frontend_roundtrip(&fe, &bundle, &ctx).expect("LOSSLESS_ASL passed");
    assert_lossless_asl_exact_ids(&bundle.program, &reparsed.program)
        .expect("exact NodeIds preserved");
    let diags = validate_ladder_bundle(&reparsed, &ctx);
    assert!(diags.is_empty(), "expected 0 diagnostics, got {diags:?}");
}

#[test]
fn handwritten_ebnf_minimal_grammar_parses_cleanly() {
    let fe = LadderFrontend::new();
    let ctx = BoardContext::plc_1200_open_1214("plc_main");

    // Strictly follows docs/lbc/ladder.md §1.1 EBNF (bare @(row, col) without explicit cell IDs)
    let src = r#"
// Main conveyor seal-in network
NETWORK OB_MAIN
VAR
    Start AT %IX0.0 : BOOL;
    Stop AT %IX0.1 : BOOL;
    Motor AT %QX0.0 : BOOL;
    t_delay : TON;
    count_val : DINT := 5;
END_VAR
// Rung 0 title
RUNG 0 (r_seal): BRANCH @(0, 0) { | | Start @(0, 0) || | | Motor @(1, 0) } -- |/| Stop @(0, 1) -- FB TON t_delay(PT := T#500ms) @(0, 2) -- CMP GE(count_val, 3) @(0, 3) -- ( ) Motor @(0, 4);
RUNG 1 (r_open): EMPTY @(0, 0) -- WIRE @(0, 1) -- FOREIGN("cpp", F0, "PORTB |= 1;") @(0, 2);
END_NETWORK
"#;
    let parsed = fe
        .parse_and_validate(src, &ctx)
        .expect("handwritten EBNF parses & validates");
    assert_eq!(parsed.value.program.hardware.len(), 3);
    assert_eq!(parsed.value.program.hardware[0].pin, "Ia.0");
    assert_eq!(parsed.value.program.hardware[2].pin, "Qa.0");
    assert_eq!(parsed.value.program.pous.len(), 1);
    let PouBody::Network(net) = &parsed.value.program.pous[0].body else {
        panic!("expected PouBody::Network");
    };
    assert_eq!(net.rungs.len(), 2);
    assert_eq!(net.rungs[0].id, NodeId::new("r_seal"));
    assert_eq!(net.rungs[0].order, 0);
    assert_eq!(net.rungs[1].order, 1);

    // Round-trip the parsed bundle cleanly
    let _ = verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("round-trip lossless");
}

// ---------------------------------------------------------------------------
// 2. Proptest `LOSSLESS_ASL` Verification
// ---------------------------------------------------------------------------

fn ident_strat() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("a".to_owned()),
        Just("b".to_owned()),
        Just("c".to_owned()),
        Just("start".to_owned()),
        Just("stop".to_owned()),
        Just("motor".to_owned()),
        Just("alarm".to_owned()),
    ]
}

fn leaf_cell_strat() -> impl Strategy<Value = LadderCell> {
    let contact = (
        0u32..4,
        0u32..8,
        prop_oneof![
            Just(LadderContactKind::NormallyOpen),
            Just(LadderContactKind::NormallyClosed),
            Just(LadderContactKind::PositiveTransition),
            Just(LadderContactKind::NegativeTransition),
        ],
        ident_strat(),
    )
        .prop_map(|(row, col, kind, name)| LadderCell {
            id: NodeId::new("tmp"),
            row,
            col,
            kind: LadderCellKind::Contact {
                kind,
                target: lv(&name),
            },
        });

    let coil = (
        0u32..4,
        0u32..8,
        prop_oneof![
            Just(LadderCoilKind::Normal),
            Just(LadderCoilKind::Negated),
            Just(LadderCoilKind::Set),
            Just(LadderCoilKind::Reset),
            Just(LadderCoilKind::PositiveTransition),
            Just(LadderCoilKind::NegativeTransition),
        ],
        ident_strat(),
    )
        .prop_map(|(row, col, kind, name)| LadderCell {
            id: NodeId::new("tmp"),
            row,
            col,
            kind: LadderCellKind::Coil {
                kind,
                target: lv(&name),
            },
        });

    let wire_or_empty =
        (0u32..4, 0u32..8, any::<bool>()).prop_map(|(row, col, is_wire)| LadderCell {
            id: NodeId::new("tmp"),
            row,
            col,
            kind: if is_wire {
                LadderCellKind::Wire
            } else {
                LadderCellKind::Empty
            },
        });

    let cmp_box = (
        0u32..4,
        0u32..8,
        prop_oneof![
            Just(LadderCompareOp::Eq),
            Just(LadderCompareOp::Ne),
            Just(LadderCompareOp::Lt),
            Just(LadderCompareOp::Le),
            Just(LadderCompareOp::Gt),
            Just(LadderCompareOp::Ge),
        ],
        0i128..100,
    )
        .prop_map(|(row, col, op, rhs)| LadderCell {
            id: NodeId::new("tmp"),
            row,
            col,
            kind: LadderCellKind::Box(LadderBoxKind::Compare {
                op,
                left: id_expr("cnt"),
                right: int_expr(rhs),
            }),
        });

    let fb_box = (
        0u32..4,
        0u32..8,
        prop_oneof![
            Just((
                FbKind::Ton,
                "ton1",
                vec![("PT".to_owned(), time_expr(100_000))]
            )),
            Just((FbKind::Ctu, "ctu1", vec![("PV".to_owned(), int_expr(5))])),
            Just((FbKind::RTrig, "rt1", vec![])),
            Just((FbKind::Sr, "sr1", vec![("R".to_owned(), id_expr("stop"))])),
        ],
    )
        .prop_map(|(row, col, (kind, inst, inputs))| LadderCell {
            id: NodeId::new("tmp"),
            row,
            col,
            kind: LadderCellKind::Box(LadderBoxKind::Fb {
                kind,
                instance: lv(inst),
                inputs,
            }),
        });

    let call_box = (0u32..4, 0u32..8, any::<bool>()).prop_map(|(row, col, with_out)| LadderCell {
        id: NodeId::new("tmp"),
        row,
        col,
        kind: LadderCellKind::Box(LadderBoxKind::Call {
            callee: "ADD".to_owned(),
            args: vec![(None, id_expr("cnt")), (None, int_expr(1))],
            output: if with_out { Some(lv("cnt")) } else { None },
        }),
    });

    let foreign_box = (
        0u32..4,
        0u32..8,
        prop_oneof![
            Just(ForeignFidelity::F0),
            Just(ForeignFidelity::F1),
            Just(ForeignFidelity::F2),
        ],
        prop::option::of(Just("pack-a".to_owned())),
    )
        .prop_map(|(row, col, fidelity, pack_hint)| LadderCell {
            id: NodeId::new("tmp"),
            row,
            col,
            kind: LadderCellKind::Box(LadderBoxKind::Foreign {
                language: "cpp".to_owned(),
                fidelity,
                text: "NOP();".to_owned(),
                pack_hint,
            }),
        });

    prop_oneof![
        contact,
        coil,
        wire_or_empty,
        cmp_box,
        fb_box,
        call_box,
        foreign_box
    ]
}

fn cell_strat() -> impl Strategy<Value = LadderCell> {
    leaf_cell_strat().prop_recursive(3, 16, 3, |inner| {
        (
            0u32..4,
            0u32..8,
            prop::collection::vec(prop::collection::vec(inner, 1..=3), 1..=3),
        )
            .prop_map(|(row, col, branches)| LadderCell {
                id: NodeId::new("tmp"),
                row,
                col,
                kind: LadderCellKind::ParallelOr { branches },
            })
    })
}

fn assign_unique_cell_ids(cells: &mut [LadderCell], counter: &mut u32) {
    for c in cells {
        c.id = NodeId::new(format!("n_p_c{}", *counter));
        *counter = counter.saturating_add(1);
        if let LadderCellKind::ParallelOr { branches } = &mut c.kind {
            for br in branches {
                assign_unique_cell_ids(br, counter);
            }
        }
    }
}

fn network_strat() -> impl Strategy<Value = LadderNetwork> {
    prop::collection::vec(
        (0u32..50, prop::collection::vec(cell_strat(), 0..=4)),
        0..=4,
    )
    .prop_map(|raw_rungs| {
        let mut cell_counter = 0u32;
        let rungs = raw_rungs
            .into_iter()
            .enumerate()
            .map(|(idx, (order, mut cells))| {
                assign_unique_cell_ids(&mut cells, &mut cell_counter);
                LadderRung {
                    id: NodeId::new(format!("n_p_r{idx}")),
                    order,
                    cells,
                }
            })
            .collect();
        LadderNetwork { rungs }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn proptest_lossless_asl_roundtrip_all_ladder_constructs(
        net in network_strat(),
        rung_note in "[a-zA-Z0-9 _()/*\\\\çãé]{0,24}",
    ) {
        let fe = LadderFrontend::new();
        let ctx = BoardContext::plc_1200_open_1214("plc_prop");
        let mut comments = Vec::new();
        if let Some(first_rung) = net.rungs.first() {
            comments.push(Comment {
                id: "c_prop_0".to_owned(),
                anchor: CommentAnchor::Node { id: first_rung.id.clone() },
                placement: AstCommentPlacement::Title,
                token_ordinal: None,
                text: rung_note,
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            });
        }
        let bundle = BoardProgramBundle {
            program: BoardProgram {
                imports: vec!["std.iec.*".to_owned()],
                hardware: vec![],
                types: vec![],
                data: vec![
                    bool_var("n_da", "a", false),
                    bool_var("n_db", "b", false),
                    bool_var("n_dc", "c", false),
                    bool_var("n_dstart", "start", false),
                    bool_var("n_dstop", "stop", false),
                    bool_var("n_dmotor", "motor", false),
                    bool_var("n_dalarm", "alarm", false),
                    dint_var("n_dcnt", "cnt", 0),
                    fb_var("n_dton", "ton1", FbKind::Ton),
                    fb_var("n_dctu", "ctu1", FbKind::Ctu),
                    fb_var("n_drt", "rt1", FbKind::RTrig),
                    fb_var("n_dsr", "sr1", FbKind::Sr),
                ],
                modules: vec![],
                pous: vec![Pou {
                    id: NodeId::new("n_pou_main"),
                    name: "OB_MAIN".to_owned(),
                    kind: PouKind::Ob { ob: ObKind::Main },
                    preferred_view: Some(AuthoringLanguage::Ladder),
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Network(net),
                    span: Span::default(),
                }],
            },
            comments,
            assumptions: Vec::new(),
            presentation: BoardPresentation::default(),
        };

        let reparsed = verify_frontend_roundtrip(&fe, &bundle, &ctx)
            .map_err(TestCaseError::fail)?;
        assert_lossless_asl_exact_ids(&bundle.program, &reparsed.program)
            .map_err(TestCaseError::fail)?;
    }
}

// ---------------------------------------------------------------------------
// 3. Comments (`C1–C5`) & Hostile Comment Corpus (`C3`)
// ---------------------------------------------------------------------------

#[test]
fn comments_c1_c5_and_hostile_corpus_all_encodings() {
    for encoding in [
        CommentEncoding::Utf8Passthrough,
        CommentEncoding::AsciiFold,
        CommentEncoding::WarnOnly,
    ] {
        let fe = LadderFrontend::with_comment_encoding(encoding);
        let caps = fe.capabilities();
        assert_eq!(
            caps.supported_comment_styles,
            vec![
                CommentStyle::Visual,
                CommentStyle::Line,
                CommentStyle::ParenBlock
            ]
        );
        assert!(caps.supports_nested_paren_comments);
        assert!(caps.supports_line_comments);
        assert_eq!(caps.comment_encoding, encoding);
        assert_eq!(caps.capabilities, vec!["asl-core@1".to_owned()]);
        assert_eq!(caps.st_dialect, None);

        let mut ctx = BoardContext::plc_1200_open_1214("plc_main");
        ctx.comment_encoding = encoding;
        assert_eq!(
            verify_hostile_corpus_on_frontend(&fe, &ctx),
            Ok(()),
            "hostile C3 corpus failed for Ladder under {encoding:?}"
        );
    }

    // Full C1/C2/C4/C5 bundle round-trip including Header, Title, Body, Leading, Trailing,
    // Dangling, Inner relocation (INFO_COMMENT_RELOCATED), nested (* *), and CRLF preservation!
    let fe = LadderFrontend::new();
    let ctx = BoardContext::plc_1200_open_1214("plc_main");

    let bundle = BoardProgramBundle {
        program: BoardProgram {
            imports: vec!["std.iec.*".to_owned()],
            hardware: vec![],
            types: vec![],
            data: vec![
                bool_var("n_d_s", "start", false),
                bool_var("n_d_m", "motor", false),
            ],
            modules: vec![],
            pous: vec![Pou {
                id: NodeId::new("n_pou_0"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: Some(AuthoringLanguage::Ladder),
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork {
                    rungs: vec![LadderRung {
                        id: NodeId::new("n_r0"),
                        order: 0,
                        cells: vec![
                            LadderCell {
                                id: NodeId::new("n_c0"),
                                row: 0,
                                col: 0,
                                kind: LadderCellKind::Contact {
                                    kind: LadderContactKind::NormallyOpen,
                                    target: lv("start"),
                                },
                            },
                            LadderCell {
                                id: NodeId::new("n_c1"),
                                row: 0,
                                col: 1,
                                kind: LadderCellKind::Coil {
                                    kind: LadderCoilKind::Normal,
                                    target: lv("motor"),
                                },
                            },
                        ],
                    }],
                }),
                span: Span::default(),
            }],
        },
        comments: vec![
            Comment {
                id: "c_hdr".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_pou_0"),
                },
                placement: AstCommentPlacement::Header,
                token_ordinal: None,
                text: " POU Header — Motor 24V ±5% 🚀".to_owned(),
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_title".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_r0"),
                },
                placement: AstCommentPlacement::Title,
                token_ordinal: None,
                text: "   exact leading/trailing spaces   ".to_owned(),
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_body".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_r0"),
                },
                placement: AstCommentPlacement::Body,
                token_ordinal: None,
                text: " outer (* nested (* deep *) *) outer ".to_owned(),
                origin: CommentOrigin {
                    language: "st".to_owned(),
                    style: CommentStyle::ParenBlock,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_cell_lead".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_c0"),
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " line 1 with */\n line 2 with *) and \\".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_cell_trail".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_c1"),
                },
                placement: AstCommentPlacement::Trailing,
                token_ordinal: None,
                text: " motor coil trailing note".to_owned(),
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_inner".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_c0"),
                },
                placement: AstCommentPlacement::Inner,
                token_ordinal: Some(1),
                text: " relocated inner note".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Block,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_dangling".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_pou_0"),
                },
                placement: AstCommentPlacement::Dangling,
                token_ordinal: None,
                text: " orphaned from deleted rung".to_owned(),
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
        ],
        assumptions: Vec::new(),
        presentation: BoardPresentation {
            eol_style: EolStyle::Crlf,
            ..BoardPresentation::default()
        },
    };

    assert_eq!(assert_c5_comment_fields(&bundle.comments), Ok(()));
    assert_eq!(
        assert_c4_reimport_idempotence(&fe, &bundle.comments, &ctx),
        Ok(())
    );
    assert_eq!(
        assert_c2_orphan_preservation(
            &bundle.comments,
            &NodeId::new("n_r0"),
            &NodeId::new("n_pou_0"),
            Some("plc_main"),
        ),
        Ok(2)
    );

    let emitted = fe
        .emit(&bundle, &ctx)
        .expect("emit with comments succeeded");
    assert!(emitted.value.contains("\r\n"), "CRLF EOL must be restored");
    assert!(
        emitted
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentRelocated),
        "Inner comment must emit INFO_COMMENT_RELOCATED"
    );

    let reparsed = fe.parse(&emitted.value, &ctx).expect("reparse succeeded");
    assert_eq!(reparsed.value.presentation.eol_style, EolStyle::Crlf);
    assert_c1_multiset_invariant(&bundle.comments, &reparsed.value.comments)
        .expect("C1 comment multiset invariant preserved");
    assert_eq!(assert_c5_comment_fields(&reparsed.value.comments), Ok(()));

    // C4 matcher handles textual Ladder `//` and `(* ... *)` AND rejects genuine human edits!
    assert!(fe.matches_comment("// motor coil", " motor coil"));
    assert!(fe.matches_comment("(* outer (* inner *) *)", " outer (* inner *) "));
    assert!(!fe.matches_comment("// human changed this", " motor coil"));
}

// ---------------------------------------------------------------------------
// 4. Hardware Tag Table Mapping (PLC 1200 & Arduino Uno R3, Zero Magic Comments)
// ---------------------------------------------------------------------------

#[test]
fn hardware_tag_table_plc_and_arduino_roundtrip_without_magic_comments() {
    let fe = LadderFrontend::new();

    // PLC 1200: Ia.0 <-> %IX0.0, Qa.0 <-> %QX0.0, IW64 <-> %IW64
    let plc_ctx = BoardContext::plc_1200_open_1214("plc_1");
    let plc_bundle = BoardProgramBundle::new(BoardProgram {
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
                id: NodeId::new("n_hw_press"),
                label: "PressRaw".to_owned(),
                pin: "IW64".to_owned(),
                kind: HardwareKind::AnalogIn,
                pull: PullMode::None,
                active_low: false,
                codec: Some("plc_span_27648".to_owned()),
                range: Some(ChannelRange {
                    signal: SignalKind::Current,
                    low: 4.0,
                    high: 20.0,
                    live_zero: true,
                    hardware_normalizes_live_zero: true,
                }),
                span: Span::default(),
            },
        ],
        types: vec![],
        data: vec![],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("n_plc_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: Some(AuthoringLanguage::Ladder),
            params: vec![],
            locals: vec![],
            body: PouBody::Network(LadderNetwork {
                rungs: vec![LadderRung {
                    id: NodeId::new("n_r0"),
                    order: 0,
                    cells: vec![
                        LadderCell {
                            id: NodeId::new("n_c0"),
                            row: 0,
                            col: 0,
                            kind: LadderCellKind::Contact {
                                kind: LadderContactKind::NormallyOpen,
                                target: lv("StartBtn"),
                            },
                        },
                        LadderCell {
                            id: NodeId::new("n_c1"),
                            row: 0,
                            col: 1,
                            kind: LadderCellKind::Coil {
                                kind: LadderCoilKind::Normal,
                                target: lv("MotorOut"),
                            },
                        },
                    ],
                }],
            }),
            span: Span::default(),
        }],
    });

    let emitted_plc = fe.emit(&plc_bundle, &plc_ctx).expect("emit PLC succeeded");
    assert!(emitted_plc.value.contains("%IX0.0"));
    assert!(emitted_plc.value.contains("%QX0.0"));
    assert!(emitted_plc.value.contains("%IW64"));
    assert_no_synthetic_rail_temps_in_asl(&emitted_plc.value);
    let reparsed_plc =
        verify_frontend_roundtrip(&fe, &plc_bundle, &plc_ctx).expect("PLC roundtrip");
    assert_eq!(reparsed_plc.program.hardware[0].pin, "Ia.0");
    assert_eq!(reparsed_plc.program.hardware[1].pin, "Qa.0");
    assert_eq!(reparsed_plc.program.hardware[2].pin, "IW64");
    assert!(validate_ladder_bundle(&reparsed_plc, &plc_ctx).is_empty());

    // Arduino Uno R3: D2 (PULL_UP, ACTIVE_LOW), D13 (DIGITAL_OUT), A0 (ANALOG_IN, uno_10bit)
    let uno_ctx = BoardContext::arduino_uno_r3("mcu_uno");
    let uno_bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec!["std.iec.*".to_owned(), "std.arduino.*".to_owned()],
        hardware: vec![
            HardwareBinding {
                id: NodeId::new("n_u_btn"),
                label: "BtnPin".to_owned(),
                pin: "D2".to_owned(),
                kind: HardwareKind::DigitalIn,
                pull: PullMode::PullUp,
                active_low: true,
                codec: None,
                range: None,
                span: Span::default(),
            },
            HardwareBinding {
                id: NodeId::new("n_u_led"),
                label: "LedPin".to_owned(),
                pin: "D13".to_owned(),
                kind: HardwareKind::DigitalOut,
                pull: PullMode::None,
                active_low: false,
                codec: None,
                range: None,
                span: Span::default(),
            },
            HardwareBinding {
                id: NodeId::new("n_u_pot"),
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
        types: vec![],
        data: vec![],
        modules: vec![],
        pous: vec![],
    });
    let reparsed_uno =
        verify_frontend_roundtrip(&fe, &uno_bundle, &uno_ctx).expect("Uno roundtrip");
    assert!(validate_ladder_bundle(&reparsed_uno, &uno_ctx).is_empty());
}

// ---------------------------------------------------------------------------
// 5. Conservative Structural Lifting (Positive Canonical + Negative Non-Liftable)
// ---------------------------------------------------------------------------

#[test]
fn conservative_lifting_canonical_patterns_and_non_liftable_rejection() {
    // Pattern 1: Seal-in latch `out := (start OR out) AND NOT stop;`
    let seal_in_stmt = Stmt::new(
        NodeId::new("s_seal"),
        StmtKind::Assign {
            target: lv("out"),
            value: Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::And,
                    left: Box::new(Expr::new(
                        ExprKind::Binary {
                            op: BinaryOp::Or,
                            left: Box::new(id_expr("start")),
                            right: Box::new(id_expr("out")),
                        },
                        Span::default(),
                    )),
                    right: Box::new(Expr::new(
                        ExprKind::Unary {
                            op: UnaryOp::Not,
                            operand: Box::new(id_expr("stop")),
                        },
                        Span::default(),
                    )),
                },
                Span::default(),
            ),
        },
        Span::default(),
    );

    // Pattern 2: Guarded Set / Reset
    let set_stmt = Stmt::new(
        NodeId::new("s_set"),
        StmtKind::If {
            condition: id_expr("set_cmd"),
            then_body: vec![Stmt::new(
                NodeId::new("s_set_b"),
                StmtKind::Assign {
                    target: lv("latched"),
                    value: bool_expr(true),
                },
                Span::default(),
            )],
            elif_arms: vec![],
            else_body: vec![],
        },
        Span::default(),
    );
    let rst_stmt = Stmt::new(
        NodeId::new("s_rst"),
        StmtKind::If {
            condition: id_expr("rst_cmd"),
            then_body: vec![Stmt::new(
                NodeId::new("s_rst_b"),
                StmtKind::Assign {
                    target: lv("latched"),
                    value: bool_expr(false),
                },
                Span::default(),
            )],
            elif_arms: vec![],
            else_body: vec![],
        },
        Span::default(),
    );

    // Pattern 3: Canonical FB call + output wire (`t1(IN := out, PT := T#500ms); timer_done := t1.Q;`)
    let fb_call = Stmt::new(
        NodeId::new("s_ton"),
        StmtKind::FbInvoke {
            instance: lv("t1"),
            inputs: vec![
                ("IN".to_owned(), id_expr("out")),
                ("PT".to_owned(), time_expr(500_000)),
            ],
        },
        Span::default(),
    );
    let fb_wire = Stmt::new(
        NodeId::new("s_ton_q"),
        StmtKind::Assign {
            target: lv("timer_done"),
            value: Expr::new(
                ExprKind::Field {
                    base: Box::new(id_expr("t1")),
                    field: "Q".to_owned(),
                },
                Span::default(),
            ),
        },
        Span::default(),
    );

    // Pattern 4: EN-gated comparison (`alarm := timer_done AND (temp >= 100);`)
    let cmp_stmt = Stmt::new(
        NodeId::new("s_cmp"),
        StmtKind::Assign {
            target: lv("alarm"),
            value: Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::And,
                    left: Box::new(id_expr("timer_done")),
                    right: Box::new(Expr::new(
                        ExprKind::Binary {
                            op: BinaryOp::Ge,
                            left: Box::new(id_expr("temp")),
                            right: Box::new(int_expr(100)),
                        },
                        Span::default(),
                    )),
                },
                Span::default(),
            ),
        },
        Span::default(),
    );

    // Pattern 5: EN-gated math, move, and stateless call
    let math_stmt = Stmt::new(
        NodeId::new("s_math"),
        StmtKind::If {
            condition: id_expr("en"),
            then_body: vec![Stmt::new(
                NodeId::new("s_math_b"),
                StmtKind::Assign {
                    target: lv("total"),
                    value: Expr::new(
                        ExprKind::Binary {
                            op: BinaryOp::Add,
                            left: Box::new(id_expr("a")),
                            right: Box::new(id_expr("b")),
                        },
                        Span::default(),
                    ),
                },
                Span::default(),
            )],
            elif_arms: vec![],
            else_body: vec![],
        },
        Span::default(),
    );
    let move_stmt = Stmt::new(
        NodeId::new("s_move"),
        StmtKind::If {
            condition: id_expr("en"),
            then_body: vec![Stmt::new(
                NodeId::new("s_move_b"),
                StmtKind::Assign {
                    target: lv("copy_val"),
                    value: id_expr("total"),
                },
                Span::default(),
            )],
            elif_arms: vec![],
            else_body: vec![],
        },
        Span::default(),
    );
    let call_stmt = Stmt::new(
        NodeId::new("s_call"),
        StmtKind::If {
            condition: id_expr("en"),
            then_body: vec![Stmt::new(
                NodeId::new("s_call_b"),
                StmtKind::Assign {
                    target: lv("abs_val"),
                    value: Expr::new(
                        ExprKind::Call {
                            callee: "ABS".to_owned(),
                            args: vec![CallArg {
                                name: None,
                                value: id_expr("temp"),
                                span: Span::default(),
                            }],
                        },
                        Span::default(),
                    ),
                },
                Span::default(),
            )],
            elif_arms: vec![],
            else_body: vec![],
        },
        Span::default(),
    );

    let all_canonical = vec![
        seal_in_stmt.clone(),
        set_stmt,
        rst_stmt,
        fb_call,
        fb_wire,
        cmp_stmt,
        math_stmt,
        move_stmt,
        call_stmt,
    ];
    let lifted = lift_statements_to_network(&all_canonical).expect("canonical patterns must lift");
    assert_eq!(lifted.rungs.len(), 8); // FB call + output wire merged into 1 rung!

    // Check Rung 0: Seal-in latch structure
    assert!(matches!(
        lifted.rungs[0].cells[0].kind,
        LadderCellKind::ParallelOr { ref branches } if branches.len() == 2
    ));
    assert!(matches!(
        lifted.rungs[0].cells[1].kind,
        LadderCellKind::Contact {
            kind: LadderContactKind::NormallyClosed,
            ..
        }
    ));
    assert!(matches!(
        lifted.rungs[0].cells[2].kind,
        LadderCellKind::Coil {
            kind: LadderCoilKind::Normal,
            ..
        }
    ));
    // Check Rung 1 & 2: Set & Reset coils
    assert!(matches!(
        lifted.rungs[1].cells[1].kind,
        LadderCellKind::Coil {
            kind: LadderCoilKind::Set,
            ..
        }
    ));
    assert!(matches!(
        lifted.rungs[2].cells[1].kind,
        LadderCellKind::Coil {
            kind: LadderCoilKind::Reset,
            ..
        }
    ));
    // Check Rung 3: TON FB box wired directly to `timer_done` coil
    assert!(matches!(
        lifted.rungs[3].cells[1].kind,
        LadderCellKind::Box(LadderBoxKind::Fb {
            kind: FbKind::Ton,
            ..
        })
    ));
    assert!(matches!(
        lifted.rungs[3].cells[2].kind,
        LadderCellKind::Coil {
            kind: LadderCoilKind::Normal,
            ..
        }
    ));
    // Check Rung 4: Compare GE box
    assert!(matches!(
        lifted.rungs[4].cells[1].kind,
        LadderCellKind::Box(LadderBoxKind::Compare {
            op: LadderCompareOp::Ge,
            ..
        })
    ));
    // Check Rungs 5, 6, 7: ADD, MOVE, ABS Call boxes
    assert!(matches!(
        &lifted.rungs[5].cells[1].kind,
        LadderCellKind::Box(LadderBoxKind::Call { callee, .. }) if callee == "ADD"
    ));
    assert!(matches!(
        &lifted.rungs[6].cells[1].kind,
        LadderCellKind::Box(LadderBoxKind::Call { callee, .. }) if callee == "MOVE"
    ));
    assert!(matches!(
        &lifted.rungs[7].cells[1].kind,
        LadderCellKind::Box(LadderBoxKind::Call { callee, .. }) if callee == "ABS"
    ));

    // Also test explicit `fb_kinds` override for `SR` / `RS` / `CTU`
    let mut fb_map = BTreeMap::new();
    fb_map.insert("my_counter".to_owned(), FbKind::Ctu);
    let ctu_stmts = vec![Stmt::new(
        NodeId::new("s_ctu"),
        StmtKind::FbInvoke {
            instance: lv("my_counter"),
            inputs: vec![
                ("CU".to_owned(), id_expr("pulse")),
                ("PV".to_owned(), int_expr(10)),
            ],
        },
        Span::default(),
    )];
    let lifted_ctu = lift_statements_to_network_with_fb_kinds(&ctu_stmts, &fb_map)
        .expect("CTU lifts with explicit fb_kinds");
    assert!(matches!(
        lifted_ctu.rungs[0].cells[1].kind,
        LadderCellKind::Box(LadderBoxKind::Fb {
            kind: FbKind::Ctu,
            ..
        })
    ));

    // Verify lifted seal-in network executes identically to original statement in `dendri-vm`!
    let lifted_seal =
        lift_statements_to_network(std::slice::from_ref(&seal_in_stmt)).expect("seal lifts");
    let make_seal_prog = |body: PouBody| BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![
            HardwareBinding {
                id: NodeId::new("h0"),
                label: "start".to_owned(),
                pin: "Ia.0".to_owned(),
                kind: HardwareKind::DigitalIn,
                pull: PullMode::None,
                active_low: false,
                codec: None,
                range: None,
                span: Span::default(),
            },
            HardwareBinding {
                id: NodeId::new("h1"),
                label: "stop".to_owned(),
                pin: "Ia.1".to_owned(),
                kind: HardwareKind::DigitalIn,
                pull: PullMode::None,
                active_low: false,
                codec: None,
                range: None,
                span: Span::default(),
            },
        ],
        types: vec![],
        data: vec![bool_var("d_out", "out", false)],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("p_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body,
            span: Span::default(),
        }],
    };
    let mut vm_stmt = BoardVm::new(
        &make_board(make_seal_prog(PouBody::Statements(vec![seal_in_stmt]))),
        VmConfig::default(),
    )
    .unwrap();
    let mut vm_net = BoardVm::new(
        &make_board(make_seal_prog(PouBody::Network(lifted_seal))),
        VmConfig::default(),
    )
    .unwrap();
    let mut io_a = MemoryBoardIo::default();
    let mut io_b = MemoryBoardIo::default();

    // Scan 1: start=true, stop=false -> out latches true
    // Scan 2: start=false, stop=false -> out stays latched true!
    // Scan 3: start=false, stop=true -> out drops false!
    for (s, st, expected) in [
        (true, false, true),
        (false, false, true),
        (false, true, false),
    ] {
        io_a.set_digital_in("Ia.0", s);
        io_a.set_digital_in("Ia.1", st);
        io_b.set_digital_in("Ia.0", s);
        io_b.set_digital_in("Ia.1", st);
        vm_stmt.run_plc_scan(10_000, &mut io_a).unwrap();
        vm_net.run_plc_scan(10_000, &mut io_b).unwrap();
        assert_eq!(vm_stmt.get_global("out"), Some(&Value::Bool(expected)));
        assert_eq!(vm_net.get_global("out"), Some(&Value::Bool(expected)));
    }

    // --- NEGATIVE: Non-liftable POUs MUST return `None` cleanly (NEVER flattened!) ---
    // 1. String tokenizer WHILE loop
    let while_loop = vec![Stmt::new(
        NodeId::new("s_tok"),
        StmtKind::While {
            condition: Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::Lt,
                    left: Box::new(id_expr("pos")),
                    right: Box::new(id_expr("len")),
                },
                Span::default(),
            ),
            body: vec![Stmt::new(
                NodeId::new("s_tok_step"),
                StmtKind::Assign {
                    target: lv("pos"),
                    value: Expr::new(
                        ExprKind::Binary {
                            op: BinaryOp::Add,
                            left: Box::new(id_expr("pos")),
                            right: Box::new(int_expr(1)),
                        },
                        Span::default(),
                    ),
                },
                Span::default(),
            )],
        },
        Span::default(),
    )];
    assert_eq!(lift_statements_to_network(&while_loop), None);

    // 2. FOR loop
    let for_loop = vec![Stmt::new(
        NodeId::new("s_for"),
        StmtKind::For {
            counter: "i".to_owned(),
            from: int_expr(0),
            to: int_expr(10),
            step: int_expr(1),
            inclusive: true,
            body: vec![],
        },
        Span::default(),
    )];
    assert_eq!(lift_statements_to_network(&for_loop), None);

    // 3. Switch / CASE
    let switch_stmt = vec![Stmt::new(
        NodeId::new("s_sw"),
        StmtKind::Switch {
            scrutinee: id_expr("state"),
            cases: vec![SwitchCase {
                patterns: vec![CasePattern::Value(int_expr(1))],
                body: vec![],
            }],
            default: None,
        },
        Span::default(),
    )];
    assert_eq!(lift_statements_to_network(&switch_stmt), None);

    // 4. IF with ELSIF / ELSE
    let if_else = vec![Stmt::new(
        NodeId::new("s_ife"),
        StmtKind::If {
            condition: id_expr("cond"),
            then_body: vec![Stmt::new(
                NodeId::new("s_t"),
                StmtKind::Assign {
                    target: lv("out"),
                    value: bool_expr(true),
                },
                Span::default(),
            )],
            elif_arms: vec![ElifArm {
                condition: id_expr("other"),
                body: vec![],
            }],
            else_body: vec![],
        },
        Span::default(),
    )];
    assert_eq!(lift_statements_to_network(&if_else), None);

    // 5. Foreign statement
    let foreign_stmt = vec![Stmt::new(
        NodeId::new("s_f"),
        StmtKind::Foreign {
            language: "cpp".to_owned(),
            fidelity: ForeignFidelity::F0,
            text: "PORTB = 0;".to_owned(),
            pack_hint: None,
        },
        Span::default(),
    )];
    assert_eq!(lift_statements_to_network(&foreign_stmt), None);

    // 6. Unconditional arithmetic mutation (`idx := idx + 1;`)
    let raw_arith = vec![Stmt::new(
        NodeId::new("s_inc"),
        StmtKind::Assign {
            target: lv("idx"),
            value: Expr::new(
                ExprKind::Binary {
                    op: BinaryOp::Add,
                    left: Box::new(id_expr("idx")),
                    right: Box::new(int_expr(1)),
                },
                Span::default(),
            ),
        },
        Span::default(),
    )];
    assert_eq!(lift_statements_to_network(&raw_arith), None);
}

// ---------------------------------------------------------------------------
// 6. Deterministic Derived Projection & Live `dendri-vm` Parity
// ---------------------------------------------------------------------------

#[test]
fn derived_projection_matches_vm_mid_rung_reset_and_unconditional_normal_coil() {
    // Rung 0: `[a] --(R a)-- (b)` (Mid-rung mutation hazard!)
    //   When `a` starts `true`, `--(R a)--` resets `a := false`, YET downstream `(b)` MUST observe
    //   the pre-mutation materialized rail (`true`), NEVER the post-reset `false` value of `a`!
    // Rung 1: `[ctrl] --( ) norm_coil` (Normal coil writes unconditionally on BOTH `true` and `false`!)
    //   `norm_coil` starts `true`, `ctrl` is `false` -> MUST become `false`!
    let net = LadderNetwork {
        rungs: vec![
            LadderRung {
                id: NodeId::new("n_r_hazard"),
                order: 0,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("n_c_a"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::NormallyOpen,
                            target: lv("a"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_c_rst_a"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Reset,
                            target: lv("a"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_c_b"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("b"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("n_r_uncond"),
                order: 1,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("n_c_ctrl"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::NormallyOpen,
                            target: lv("ctrl"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_c_norm"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("norm_coil"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("n_c_neg"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Negated,
                            target: lv("neg_coil"),
                        },
                    },
                ],
            },
        ],
    };

    // Confirm public convenience API returns identical statements
    let stmts_only = project_network_to_statements(&net);
    let with_decls = project_network_with_decls(&net);
    assert_eq!(stmts_only, with_decls.stmts);
    assert!(!stmts_only.is_empty());

    let base_program = BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            bool_var("d_a", "a", true),
            bool_var("d_b", "b", false),
            bool_var("d_ctrl", "ctrl", false),
            bool_var("d_norm", "norm_coil", true),
            bool_var("d_neg", "neg_coil", false),
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("p_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: Some(AuthoringLanguage::Ladder),
            params: vec![],
            locals: vec![],
            body: PouBody::Network(net.clone()),
            span: Span::default(),
        }],
    };

    let mut vm_native =
        BoardVm::new(&make_board(base_program.clone()), VmConfig::default()).unwrap();
    let mut vm_proj = BoardVm::new(
        &make_projected_board(&base_program, &net),
        VmConfig::default(),
    )
    .unwrap();
    let mut io1 = MemoryBoardIo::default();
    let mut io2 = MemoryBoardIo::default();

    // Scan 1: `a` was true -> reset to false, `b` MUST be true! `norm_coil` MUST drop false! `neg_coil` MUST become true!
    vm_native.run_plc_scan(10_000, &mut io1).unwrap();
    vm_proj.run_plc_scan(10_000, &mut io2).unwrap();

    for (var, expected) in [
        ("a", false),
        ("b", true),
        ("norm_coil", false),
        ("neg_coil", true),
    ] {
        assert_eq!(
            vm_native.get_global(var),
            Some(&Value::Bool(expected)),
            "native VM mismatch on {var} (scan 1)"
        );
        assert_eq!(
            vm_proj.get_global(var),
            Some(&Value::Bool(expected)),
            "projected statements VM mismatch on {var} (scan 1)"
        );
    }

    // Scan 2: now `a` is false -> `b` unconditionally drops to false!
    vm_native.run_plc_scan(10_000, &mut io1).unwrap();
    vm_proj.run_plc_scan(10_000, &mut io2).unwrap();
    assert_eq!(vm_native.get_global("b"), Some(&Value::Bool(false)));
    assert_eq!(vm_proj.get_global("b"), Some(&Value::Bool(false)));
}

#[test]
fn derived_projection_empty_cell_is_open_circuit_and_never_conducts() {
    // Rung 0: `WIRE -- EMPTY -- ( ) out_series` -> Empty MUST break circuit (`out_series = false`)!
    // Rung 1: `BRANCH { EMPTY || EMPTY } -- ( ) out_both_empty` -> MUST be `false`!
    // Rung 2: `BRANCH { EMPTY || WIRE } -- ( ) out_or_wire` -> MUST be `true`!
    let net = LadderNetwork {
        rungs: vec![
            LadderRung {
                id: NodeId::new("r0"),
                order: 0,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c0"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Wire,
                    },
                    LadderCell {
                        id: NodeId::new("c1"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Empty,
                    },
                    LadderCell {
                        id: NodeId::new("c2"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("out_series"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("r1"),
                order: 1,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c3"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::ParallelOr {
                            branches: vec![
                                vec![LadderCell {
                                    id: NodeId::new("c4"),
                                    row: 0,
                                    col: 0,
                                    kind: LadderCellKind::Empty,
                                }],
                                vec![LadderCell {
                                    id: NodeId::new("c5"),
                                    row: 1,
                                    col: 0,
                                    kind: LadderCellKind::Empty,
                                }],
                            ],
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c6"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("out_both_empty"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("r2"),
                order: 2,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c7"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::ParallelOr {
                            branches: vec![
                                vec![LadderCell {
                                    id: NodeId::new("c8"),
                                    row: 0,
                                    col: 0,
                                    kind: LadderCellKind::Empty,
                                }],
                                vec![LadderCell {
                                    id: NodeId::new("c9"),
                                    row: 1,
                                    col: 0,
                                    kind: LadderCellKind::Wire,
                                }],
                            ],
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c10"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("out_or_wire"),
                        },
                    },
                ],
            },
        ],
    };

    let base_program = BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            bool_var("d0", "out_series", true),
            bool_var("d1", "out_both_empty", true),
            bool_var("d2", "out_or_wire", false),
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("p_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Network(net.clone()),
            span: Span::default(),
        }],
    };

    let mut vm_native =
        BoardVm::new(&make_board(base_program.clone()), VmConfig::default()).unwrap();
    let mut vm_proj = BoardVm::new(
        &make_projected_board(&base_program, &net),
        VmConfig::default(),
    )
    .unwrap();
    let mut io1 = MemoryBoardIo::default();
    let mut io2 = MemoryBoardIo::default();
    vm_native.run_plc_scan(10_000, &mut io1).unwrap();
    vm_proj.run_plc_scan(10_000, &mut io2).unwrap();

    for (var, expected) in [
        ("out_series", false),
        ("out_both_empty", false),
        ("out_or_wire", true),
    ] {
        assert_eq!(vm_native.get_global(var), Some(&Value::Bool(expected)));
        assert_eq!(vm_proj.get_global(var), Some(&Value::Bool(expected)));
    }
}

#[test]
fn derived_projection_compare_and_call_en_eno_parity_with_vm() {
    // Rung 0: `[en_low] -- CMP GE(x, 5) -- CALL ABS(neg_x) => res_skipped -- ( ) eno_low`
    //   `en_low == false`, so Compare outputs `false`, `ABS` MUST NOT execute (`res_skipped` stays 99), `eno_low == false`!
    // Rung 1: `[en_hi] -- CMP GE(x, 5) -- CALL ABS(neg_x) => res_ran -- ( ) eno_hi`
    //   `en_hi == true` and `10 >= 5`, so `ABS(-42) == 42` writes `res_ran := 42`, `eno_hi == true`!
    let net = LadderNetwork {
        rungs: vec![
            LadderRung {
                id: NodeId::new("r_lo"),
                order: 0,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c0"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::NormallyOpen,
                            target: lv("en_low"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c1"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Box(LadderBoxKind::Compare {
                            op: LadderCompareOp::Ge,
                            left: id_expr("x"),
                            right: int_expr(5),
                        }),
                    },
                    LadderCell {
                        id: NodeId::new("c2"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Box(LadderBoxKind::Call {
                            callee: "ABS".to_owned(),
                            args: vec![(None, id_expr("neg_x"))],
                            output: Some(lv("res_skipped")),
                        }),
                    },
                    LadderCell {
                        id: NodeId::new("c3"),
                        row: 0,
                        col: 3,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("eno_low"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("r_hi"),
                order: 1,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c4"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::NormallyOpen,
                            target: lv("en_hi"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c5"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Box(LadderBoxKind::Compare {
                            op: LadderCompareOp::Ge,
                            left: id_expr("x"),
                            right: int_expr(5),
                        }),
                    },
                    LadderCell {
                        id: NodeId::new("c6"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Box(LadderBoxKind::Call {
                            callee: "ABS".to_owned(),
                            args: vec![(None, id_expr("neg_x"))],
                            output: Some(lv("res_ran")),
                        }),
                    },
                    LadderCell {
                        id: NodeId::new("c7"),
                        row: 0,
                        col: 3,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("eno_hi"),
                        },
                    },
                ],
            },
        ],
    };

    let base_program = BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            bool_var("d0", "en_low", false),
            bool_var("d1", "en_hi", true),
            dint_var("d2", "x", 10),
            dint_var("d3", "neg_x", -42),
            dint_var("d4", "res_skipped", 99),
            dint_var("d5", "res_ran", 0),
            bool_var("d6", "eno_low", true),
            bool_var("d7", "eno_hi", false),
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("p_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Network(net.clone()),
            span: Span::default(),
        }],
    };

    let mut vm_native =
        BoardVm::new(&make_board(base_program.clone()), VmConfig::default()).unwrap();
    let mut vm_proj = BoardVm::new(
        &make_projected_board(&base_program, &net),
        VmConfig::default(),
    )
    .unwrap();
    let mut io1 = MemoryBoardIo::default();
    let mut io2 = MemoryBoardIo::default();
    vm_native.run_plc_scan(10_000, &mut io1).unwrap();
    vm_proj.run_plc_scan(10_000, &mut io2).unwrap();

    assert_eq!(vm_native.get_global("res_skipped"), Some(&Value::Dint(99)));
    assert_eq!(vm_proj.get_global("res_skipped"), Some(&Value::Dint(99)));
    assert_eq!(vm_native.get_global("res_ran"), Some(&Value::Dint(42)));
    assert_eq!(vm_proj.get_global("res_ran"), Some(&Value::Dint(42)));
    assert_eq!(vm_native.get_global("eno_low"), Some(&Value::Bool(false)));
    assert_eq!(vm_proj.get_global("eno_low"), Some(&Value::Bool(false)));
    assert_eq!(vm_native.get_global("eno_hi"), Some(&Value::Bool(true)));
    assert_eq!(vm_proj.get_global("eno_hi"), Some(&Value::Bool(true)));
}

#[test]
fn derived_projection_fb_and_edge_transitions_parity_with_vm() {
    // Rung 0: `[run_in] -- FB TON t1(PT := T#500ms) -- ( ) q_ton`
    // Rung 1: `|P| edge_in -- ( ) q_rise`
    // Rung 2: `| | edge_in -- (N) q_fall_coil`
    let net = LadderNetwork {
        rungs: vec![
            LadderRung {
                id: NodeId::new("r_ton"),
                order: 0,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c0"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::NormallyOpen,
                            target: lv("run_in"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c1"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Box(LadderBoxKind::Fb {
                            kind: FbKind::Ton,
                            instance: lv("t1"),
                            inputs: vec![("PT".to_owned(), time_expr(500_000))],
                        }),
                    },
                    LadderCell {
                        id: NodeId::new("c2"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("q_ton"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("r_rise"),
                order: 1,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c3"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::PositiveTransition,
                            target: lv("edge_in"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c4"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("q_rise"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("r_fall"),
                order: 2,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c5"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::NormallyOpen,
                            target: lv("edge_in"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c6"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::NegativeTransition,
                            target: lv("q_fall_coil"),
                        },
                    },
                ],
            },
        ],
    };

    let base_program = BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![
            HardwareBinding {
                id: NodeId::new("h0"),
                label: "run_in".to_owned(),
                pin: "Ia.0".to_owned(),
                kind: HardwareKind::DigitalIn,
                pull: PullMode::None,
                active_low: false,
                codec: None,
                range: None,
                span: Span::default(),
            },
            HardwareBinding {
                id: NodeId::new("h1"),
                label: "edge_in".to_owned(),
                pin: "Ia.1".to_owned(),
                kind: HardwareKind::DigitalIn,
                pull: PullMode::None,
                active_low: false,
                codec: None,
                range: None,
                span: Span::default(),
            },
        ],
        types: vec![],
        data: vec![
            fb_var("d_t1", "t1", FbKind::Ton),
            bool_var("d_qton", "q_ton", false),
            bool_var("d_qrise", "q_rise", false),
            bool_var("d_qfall", "q_fall_coil", false),
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("p_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Network(net.clone()),
            span: Span::default(),
        }],
    };

    let mut vm_native =
        BoardVm::new(&make_board(base_program.clone()), VmConfig::default()).unwrap();
    let mut vm_proj = BoardVm::new(
        &make_projected_board(&base_program, &net),
        VmConfig::default(),
    )
    .unwrap();
    let mut io1 = MemoryBoardIo::default();
    let mut io2 = MemoryBoardIo::default();

    // Multi-scan trace: 4 scans of 250ms each
    let timeline = [
        // (run_in, edge_in, exp_q_ton, exp_q_rise, exp_q_fall)
        (true, true, false, true, false), // t=250ms: TON counting, rising edge pulse
        (true, true, true, false, false), // t=500ms: TON reaches PT!, no rising pulse
        (true, false, true, false, true), // t=750ms: falling edge pulse on coil!
        (false, false, false, false, false), // t=1000ms: TON resets
    ];
    for (scan, &(run, edge, exp_ton, exp_rise, exp_fall)) in timeline.iter().enumerate() {
        io1.set_digital_in("Ia.0", run);
        io1.set_digital_in("Ia.1", edge);
        io2.set_digital_in("Ia.0", run);
        io2.set_digital_in("Ia.1", edge);
        vm_native.run_plc_scan(250_000, &mut io1).unwrap();
        vm_proj.run_plc_scan(250_000, &mut io2).unwrap();

        for (var, expected) in [
            ("q_ton", exp_ton),
            ("q_rise", exp_rise),
            ("q_fall_coil", exp_fall),
        ] {
            assert_eq!(
                vm_native.get_global(var),
                Some(&Value::Bool(expected)),
                "native VM scan {scan} var {var}"
            );
            assert_eq!(
                vm_proj.get_global(var),
                Some(&Value::Bool(expected)),
                "projected VM scan {scan} var {var}"
            );
        }
    }
}

#[test]
fn derived_projection_honors_explicit_rung_order_and_grid_col_row_sorting() {
    // Rung with order=10 placed FIRST in vec reads `step1`; rung with order=0 placed SECOND sets `step1`!
    // Inside order=0 rung, the coil at col=1 is placed BEFORE the wire at col=0 in vec!
    let net = LadderNetwork {
        rungs: vec![
            LadderRung {
                id: NodeId::new("r_second"),
                order: 10,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c_out"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("step2"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c_read"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::NormallyOpen,
                            target: lv("step1"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("r_first"),
                order: 0,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c_set1"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("step1"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c_wire"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Wire,
                    },
                ],
            },
        ],
    };

    let base_program = BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            bool_var("d0", "step1", false),
            bool_var("d1", "step2", false),
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("p_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Network(net.clone()),
            span: Span::default(),
        }],
    };

    let mut vm_native =
        BoardVm::new(&make_board(base_program.clone()), VmConfig::default()).unwrap();
    let mut vm_proj = BoardVm::new(
        &make_projected_board(&base_program, &net),
        VmConfig::default(),
    )
    .unwrap();
    let mut io1 = MemoryBoardIo::default();
    let mut io2 = MemoryBoardIo::default();
    vm_native.run_plc_scan(10_000, &mut io1).unwrap();
    vm_proj.run_plc_scan(10_000, &mut io2).unwrap();

    // Both `step1` AND `step2` must become true on Scan 1 because order=0 runs BEFORE order=10!
    assert_eq!(vm_native.get_global("step1"), Some(&Value::Bool(true)));
    assert_eq!(vm_native.get_global("step2"), Some(&Value::Bool(true)));
    assert_eq!(vm_proj.get_global("step1"), Some(&Value::Bool(true)));
    assert_eq!(vm_proj.get_global("step2"), Some(&Value::Bool(true)));
}

// ---------------------------------------------------------------------------
// 7. `Foreign` Box Preservation & Cross-Language Export Guard
// ---------------------------------------------------------------------------

#[test]
fn foreign_box_preservation_and_cross_language_export_guard() {
    let fe = LadderFrontend::new();
    let mut ctx = BoardContext::plc_1200_open_1214("plc_main");

    let net = LadderNetwork {
        rungs: vec![LadderRung {
            id: NodeId::new("r_f"),
            order: 0,
            cells: vec![LadderCell {
                id: NodeId::new("c_f"),
                row: 0,
                col: 0,
                kind: LadderCellKind::Box(LadderBoxKind::Foreign {
                    language: "cpp".to_owned(),
                    fidelity: ForeignFidelity::F0,
                    text: "PORTB |= (1 << 5);".to_owned(),
                    pack_hint: Some("avr-regs".to_owned()),
                }),
            }],
        }],
    };

    // 1. Same-paradigm Ladder round-trip preserves locked Foreign box losslessly
    let bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("p_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: Some(AuthoringLanguage::Ladder),
            params: vec![],
            locals: vec![],
            body: PouBody::Network(net.clone()),
            span: Span::default(),
        }],
    });
    let reparsed = verify_frontend_roundtrip(&fe, &bundle, &ctx).expect("Foreign box round-trips");
    assert_lossless_asl_exact_ids(&bundle.program, &reparsed.program).expect("exact IDs");

    // 2. Exporting Ladder network with C++ ForeignBox to ST WITHOUT `--allow-foreign` MUST fail
    //    with a strict four-part `ERR_FOREIGN_EXPORT_BLOCKED` diagnostic!
    let blocked = project_network_for_export(&net, LanguageId::St, &ctx).unwrap_err();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].code, ErrorCode::ErrForeignExportBlocked);
    assert!(
        is_four_part_diagnostic(&blocked[0]),
        "must satisfy 4-part contract: {:?}",
        blocked[0]
    );

    // 3. Exporting to C++ (same language as ForeignBox) is NEVER blocked!
    let same_lang = project_network_for_export(&net, LanguageId::Cpp, &ctx)
        .expect("same-language export never blocked");
    assert!(same_lang.value.foreign_comments.is_empty());

    // 4. Enabling `allow_foreign_export` permits ST export with demarcated "NOT TRANSLATED" comment!
    ctx.allow_foreign_export = true;
    let allowed = project_network_for_export(&net, LanguageId::St, &ctx)
        .expect("export with --allow-foreign succeeds");
    assert_eq!(allowed.value.foreign_comments.len(), 1);
    assert!(
        allowed.value.foreign_comments[0]
            .rendered
            .contains("NOT TRANSLATED")
    );
    assert!(
        allowed.value.foreign_comments[0]
            .rendered
            .contains("PORTB |= (1 << 5);")
    );
}

// ---------------------------------------------------------------------------
// 8. Genuine Negative Fixture for Every `ErrorCode` (`docs/lbc/ladder.md` §1.4)
// ---------------------------------------------------------------------------

#[test]
fn negative_fixtures_cover_every_ladder_error_code() {
    let fe = LadderFrontend::new();
    let plc_ctx = BoardContext::plc_1200_open_1214("plc_main");
    let uno_ctx = BoardContext::arduino_uno_r3("mcu_uno");

    let assert_parse_err = |src: &str, ctx: &BoardContext, expected: ErrorCode| {
        let errs = fe
            .parse_and_validate(src, ctx)
            .expect_err(&format!("expected error {expected} for:\n{src}"));
        assert!(
            errs.iter().any(|d| d.code == expected),
            "expected {expected}, got: {errs:?}"
        );
    };

    // 1. ERR_TOML_SYNTAX: malformed LD text syntax
    assert_parse_err(
        "NETWORK OB_MAIN RUNG bad_order (r0): | | a @(0, 0); END_NETWORK",
        &plc_ctx,
        ErrorCode::ErrTomlSyntax,
    );

    // 2. ERR_UNKNOWN_FIELD: unknown directive / attribute keyword
    assert_parse_err(
        "TAG_TABLE btn AT %IX0.0 : DIGITAL_IN BOGUS_ATTR; END_TAG_TABLE",
        &plc_ctx,
        ErrorCode::ErrUnknownField,
    );

    // 3. ERR_DUPLICATE_ID: duplicate NodeId across rungs / cells
    assert_parse_err(
        r#"
        NETWORK OB_MAIN
        VAR a : BOOL; END_VAR
        RUNG 0 (dup_id): | | a @(0, 0, c0);
        RUNG 1 (dup_id): | | a @(0, 0, c1);
        END_NETWORK
        "#,
        &plc_ctx,
        ErrorCode::ErrDuplicateId,
    );

    // 4. ERR_TYPE_MISMATCH: contact on non-BOOL (`DINT`), FB mismatch (`TON` box on `CTU`),
    //    and incompatible `Compare` operands (`INT` vs `UINT`)
    assert_parse_err(
        r#"
        NETWORK OB_MAIN
        VAR speed : DINT; END_VAR
        RUNG 0 (r0): | | speed @(0, 0);
        END_NETWORK
        "#,
        &plc_ctx,
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_err(
        r#"
        NETWORK OB_MAIN
        VAR c1 : CTU; END_VAR
        RUNG 0 (r0): FB TON c1(PT := T#100ms) @(0, 0);
        END_NETWORK
        "#,
        &plc_ctx,
        ErrorCode::ErrTypeMismatch,
    );
    assert_parse_err(
        r#"
        NETWORK OB_MAIN
        VAR s_val : INT; u_val : UINT; END_VAR
        RUNG 0 (r0): CMP LT(s_val, u_val) @(0, 0);
        END_NETWORK
        "#,
        &plc_ctx,
        ErrorCode::ErrTypeMismatch,
    );

    // 5. ERR_CONST_WRITE: coil writing to CONSTANT variable
    assert_parse_err(
        r#"
        NETWORK OB_MAIN
        VAR CONSTANT flag : BOOL := true; END_VAR
        RUNG 0 (r0): WIRE @(0, 0) -- ( ) flag @(0, 1);
        END_NETWORK
        "#,
        &plc_ctx,
        ErrorCode::ErrConstWrite,
    );

    // 6. ERR_HARDWARE_LABEL_WRITE: coil writing to input-only `%IX0.0` tag
    assert_parse_err(
        r#"
        TAG_TABLE
            StartBtn (h0) AT %IX0.0 : DIGITAL_IN;
        END_TAG_TABLE
        NETWORK OB_MAIN
        RUNG 0 (r0): WIRE @(0, 0) -- ( ) StartBtn @(0, 1);
        END_NETWORK
        "#,
        &plc_ctx,
        ErrorCode::ErrHardwareLabelWrite,
    );

    // 7. ERR_UNDECLARED_SYMBOL: contact referencing undeclared variable
    assert_parse_err(
        "NETWORK OB_MAIN RUNG 0 (r0): | | ghost_var @(0, 0); END_NETWORK",
        &plc_ctx,
        ErrorCode::ErrUndeclaredSymbol,
    );

    // 8. ERR_UNRESOLVED_FUNCTION: CALL box invoking nonexistent function
    assert_parse_err(
        "NETWORK OB_MAIN RUNG 0 (r0): CALL NON_EXISTENT_FN(1) @(0, 0); END_NETWORK",
        &plc_ctx,
        ErrorCode::ErrUnresolvedFunction,
    );

    // 9. ERR_ARITY_MISMATCH: CALL box passing wrong arity to `ABS`
    assert_parse_err(
        r#"
        IMPORT std.iec.math;
        NETWORK OB_MAIN
        VAR x : DINT; y : DINT; END_VAR
        RUNG 0 (r0): CALL ABS(x, y) => y @(0, 0);
        END_NETWORK
        "#,
        &plc_ctx,
        ErrorCode::ErrArityMismatch,
    );

    // 10. ERR_RETURN_TYPE_MISMATCH: CALL box binding `DINT` return into `BOOL` output
    assert_parse_err(
        r#"
        IMPORT std.iec.math;
        NETWORK OB_MAIN
        VAR x : DINT; b_out : BOOL; END_VAR
        RUNG 0 (r0): CALL ABS(x) => b_out @(0, 0);
        END_NETWORK
        "#,
        &plc_ctx,
        ErrorCode::ErrReturnTypeMismatch,
    );

    // 11. ERR_ISR_FORBIDDEN_OP: blocking `WAIT_TIME` inside `OB_HW_INT` Ladder network
    assert_parse_err(
        r#"
        IMPORT std.arduino.*;
        NETWORK ISR_Handler KIND OB_HW_INT(Btn, RISING, 1)
        RUNG 0 (r0): CALL WAIT_TIME(100) @(0, 0);
        END_NETWORK
        "#,
        &uno_ctx,
        ErrorCode::ErrIsrForbiddenOp,
    );

    // 12. ERR_UNKNOWN_PORT: unknown IEC direct address on PLC & unknown pin on Arduino Uno
    assert_parse_err(
        "TAG_TABLE bad AT %IX99.0 : DIGITAL_IN; END_TAG_TABLE",
        &plc_ctx,
        ErrorCode::ErrUnknownPort,
    );
    assert_parse_err(
        "TAG_TABLE bad AT D99 : DIGITAL_IN; END_TAG_TABLE",
        &uno_ctx,
        ErrorCode::ErrUnknownPort,
    );

    // 13. ERR_PIN_CAPABILITY_MISMATCH: output bound to input-only `%IX0.0`, or PWM on Uno `D2`
    assert_parse_err(
        "TAG_TABLE bad AT %IX0.0 : DIGITAL_OUT; END_TAG_TABLE",
        &plc_ctx,
        ErrorCode::ErrPinCapabilityMismatch,
    );
    assert_parse_err(
        "TAG_TABLE bad AT D2 : PWM_OUT; END_TAG_TABLE",
        &uno_ctx,
        ErrorCode::ErrPinCapabilityMismatch,
    );

    // 14. ERR_COMMENT_ANCHOR: comment pointing to deleted/nonexistent rung NodeId
    let bad_anchor_bundle = BoardProgramBundle {
        program: BoardProgram::default(),
        comments: vec![Comment {
            id: "c_ghost".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_deleted_rung_999"),
            },
            placement: AstCommentPlacement::Title,
            token_ordinal: None,
            text: " orphaned improperly".to_owned(),
            origin: CommentOrigin {
                language: "ladder".to_owned(),
                style: CommentStyle::Visual,
            },
            span: Span::default(),
        }],
        assumptions: vec![],
        presentation: BoardPresentation::default(),
    };
    let anchor_diags = validate_ladder_bundle(&bad_anchor_bundle, &plc_ctx);
    assert!(
        anchor_diags
            .iter()
            .any(|d| d.code == ErrorCode::ErrCommentAnchor),
        "expected ERR_COMMENT_ANCHOR, got {anchor_diags:?}"
    );

    // 15. ERR_COMMENT_FIELD: invalid token_ordinal on non-Inner comment AND unterminated `(*`
    let bad_field_bundle = BoardProgramBundle {
        program: BoardProgram::default(),
        comments: vec![Comment {
            id: "c_bad_ord".to_owned(),
            anchor: CommentAnchor::Board {
                id: "plc_main".to_owned(),
            },
            placement: AstCommentPlacement::Header,
            token_ordinal: Some(3),
            text: " invalid ordinal".to_owned(),
            origin: CommentOrigin {
                language: "ladder".to_owned(),
                style: CommentStyle::Visual,
            },
            span: Span::default(),
        }],
        assumptions: vec![],
        presentation: BoardPresentation::default(),
    };
    let field_diags = validate_ladder_bundle(&bad_field_bundle, &plc_ctx);
    assert!(
        field_diags
            .iter()
            .any(|d| d.code == ErrorCode::ErrCommentField),
        "expected ERR_COMMENT_FIELD, got {field_diags:?}"
    );
    assert_parse_err(
        "(* unterminated comment NETWORK OB_MAIN END_NETWORK",
        &plc_ctx,
        ErrorCode::ErrCommentField,
    );

    // 16. ERR_FOREIGN_EXPORT_BLOCKED: emitting non-Ladder `TypeDecl::Foreign` without `--allow-foreign`
    let foreign_decl_bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec![],
        hardware: vec![],
        types: vec![TypeDecl::Foreign {
            id: NodeId::new("n_f_decl"),
            name: Some("Servo".to_owned()),
            language: "cpp".to_owned(),
            fidelity: ForeignFidelity::F1,
            text: "Servo s;".to_owned(),
            pack_hint: Some("arduino-servo".to_owned()),
            span: Span::new(0, 8),
        }],
        data: vec![],
        modules: vec![],
        pous: vec![],
    });
    let export_errs = fe.emit(&foreign_decl_bundle, &plc_ctx).unwrap_err();
    assert!(
        export_errs
            .iter()
            .any(|d| d.code == ErrorCode::ErrForeignExportBlocked && is_four_part_diagnostic(d)),
        "expected four-part ERR_FOREIGN_EXPORT_BLOCKED, got {export_errs:?}"
    );
}

// ---------------------------------------------------------------------------
// 9. Auto-Generated Feature Matrix Verification (`docs/lbc/ladder.md` §8)
// ---------------------------------------------------------------------------

#[test]
fn feature_matrix_entries_render_clean_markdown_table() {
    let entries = feature_matrix_entries();
    assert!(entries.len() >= 10, "expected rich Ladder feature matrix");
    let mut collector = FeatureMatrixCollector::new();
    for e in entries {
        assert_eq!(e.language, LanguageId::Ladder);
        assert!(!e.category.is_empty());
        assert!(!e.feature.is_empty());
        assert!(!e.evidence_test.is_empty());
        collector.record(e);
    }
    let md = collector.to_markdown();
    assert!(md.contains("| Category | Feature | C++ | ST | Ladder | SFC | Evidence / Notes |"));
    assert!(md.contains("`ladder`:"));
    assert!(md.contains("proptest_lossless_asl_roundtrip_all_ladder_constructs"));
    assert!(md.contains("negative_fixtures_cover_every_ladder_error_code"));
}

#[test]
fn derived_projection_short_circuits_compare_when_en_false_and_avoids_branch_edge_duplication() {
    // 1. `EMPTY -- CMP EQ(10 / 0, 1) -- ( ) div_out`: EN is false -> MUST short-circuit without DivisionByZero!
    // 2. `|P| pulse_btn -- BRANCH { (S) branch_a || (S) branch_b }`: single upstream edge pulse feeds BOTH branches!
    let net = LadderNetwork {
        rungs: vec![
            LadderRung {
                id: NodeId::new("r_short"),
                order: 0,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c_e"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Empty,
                    },
                    LadderCell {
                        id: NodeId::new("c_div"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::Box(LadderBoxKind::Compare {
                            op: LadderCompareOp::Eq,
                            left: Expr::new(
                                ExprKind::Binary {
                                    op: BinaryOp::Div,
                                    left: Box::new(int_expr(10)),
                                    right: Box::new(int_expr(0)),
                                },
                                Span::default(),
                            ),
                            right: int_expr(1),
                        }),
                    },
                    LadderCell {
                        id: NodeId::new("c_div_out"),
                        row: 0,
                        col: 2,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: lv("div_out"),
                        },
                    },
                ],
            },
            LadderRung {
                id: NodeId::new("r_edge_branch"),
                order: 1,
                cells: vec![
                    LadderCell {
                        id: NodeId::new("c_p"),
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Contact {
                            kind: LadderContactKind::PositiveTransition,
                            target: lv("pulse_btn"),
                        },
                    },
                    LadderCell {
                        id: NodeId::new("c_br"),
                        row: 0,
                        col: 1,
                        kind: LadderCellKind::ParallelOr {
                            branches: vec![
                                vec![LadderCell {
                                    id: NodeId::new("c_ba"),
                                    row: 0,
                                    col: 1,
                                    kind: LadderCellKind::Coil {
                                        kind: LadderCoilKind::Set,
                                        target: lv("branch_a"),
                                    },
                                }],
                                vec![LadderCell {
                                    id: NodeId::new("c_bb"),
                                    row: 1,
                                    col: 1,
                                    kind: LadderCellKind::Coil {
                                        kind: LadderCoilKind::Set,
                                        target: lv("branch_b"),
                                    },
                                }],
                            ],
                        },
                    },
                ],
            },
        ],
    };

    let base_program = BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![HardwareBinding {
            id: NodeId::new("h_btn"),
            label: "pulse_btn".to_owned(),
            pin: "Ia.0".to_owned(),
            kind: HardwareKind::DigitalIn,
            pull: PullMode::None,
            active_low: false,
            codec: None,
            range: None,
            span: Span::default(),
        }],
        types: vec![],
        data: vec![
            bool_var("d0", "div_out", true),
            bool_var("d1", "branch_a", false),
            bool_var("d2", "branch_b", false),
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("p_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Network(net.clone()),
            span: Span::default(),
        }],
    };

    let mut vm_native =
        BoardVm::new(&make_board(base_program.clone()), VmConfig::default()).unwrap();
    let mut vm_proj = BoardVm::new(
        &make_projected_board(&base_program, &net),
        VmConfig::default(),
    )
    .unwrap();
    let mut io1 = MemoryBoardIo::default();
    let mut io2 = MemoryBoardIo::default();
    io1.set_digital_in("Ia.0", true);
    io2.set_digital_in("Ia.0", true);

    vm_native.run_plc_scan(10_000, &mut io1).unwrap();
    vm_proj.run_plc_scan(10_000, &mut io2).unwrap();

    for (var, expected) in [("div_out", false), ("branch_a", true), ("branch_b", true)] {
        assert_eq!(vm_native.get_global(var), Some(&Value::Bool(expected)));
        assert_eq!(vm_proj.get_global(var), Some(&Value::Bool(expected)));
    }
}

#[test]
fn conservative_lifting_rejects_mixed_canonical_plus_loop_and_lifts_sr_rs_bistables() {
    // Mixed canonical assignment + WHILE loop MUST return None (never partially flatten!)
    let mixed = vec![
        Stmt::new(
            NodeId::new("s0"),
            StmtKind::Assign {
                target: lv("ok"),
                value: id_expr("a"),
            },
            Span::default(),
        ),
        Stmt::new(
            NodeId::new("s1"),
            StmtKind::While {
                condition: id_expr("running"),
                body: vec![],
            },
            Span::default(),
        ),
    ];
    assert_eq!(lift_statements_to_network(&mixed), None);

    // SR & RS bistables with output wires lift cleanly
    let bistables = vec![
        Stmt::new(
            NodeId::new("s_sr"),
            StmtKind::FbInvoke {
                instance: lv("b_sr"),
                inputs: vec![
                    ("S1".to_owned(), id_expr("set_in")),
                    ("R".to_owned(), id_expr("rst_in")),
                ],
            },
            Span::default(),
        ),
        Stmt::new(
            NodeId::new("s_sr_q"),
            StmtKind::Assign {
                target: lv("q_sr"),
                value: Expr::new(
                    ExprKind::Field {
                        base: Box::new(id_expr("b_sr")),
                        field: "Q1".to_owned(),
                    },
                    Span::default(),
                ),
            },
            Span::default(),
        ),
        Stmt::new(
            NodeId::new("s_rs"),
            StmtKind::FbInvoke {
                instance: lv("b_rs"),
                inputs: vec![
                    ("S".to_owned(), id_expr("set_in")),
                    ("R1".to_owned(), id_expr("rst_in")),
                ],
            },
            Span::default(),
        ),
        Stmt::new(
            NodeId::new("s_rs_q"),
            StmtKind::Assign {
                target: lv("q_rs"),
                value: Expr::new(
                    ExprKind::Field {
                        base: Box::new(id_expr("b_rs")),
                        field: "Q1".to_owned(),
                    },
                    Span::default(),
                ),
            },
            Span::default(),
        ),
    ];
    let lifted = lift_statements_to_network(&bistables).expect("SR/RS lift cleanly");
    assert_eq!(lifted.rungs.len(), 2);
    assert!(matches!(
        lifted.rungs[0].cells[1].kind,
        LadderCellKind::Box(LadderBoxKind::Fb {
            kind: FbKind::Sr,
            ..
        })
    ));
    assert!(matches!(
        lifted.rungs[1].cells[1].kind,
        LadderCellKind::Box(LadderBoxKind::Fb {
            kind: FbKind::Rs,
            ..
        })
    ));
}
