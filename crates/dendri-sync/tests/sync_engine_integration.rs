//! Comprehensive integration tests for `dendri-sync` using clean mock `LanguageFrontend`
//! implementations (zero circular dependency on individual language crates).

use dendri_ast::{
    ActionQualifier, Assumption, AssumptionPolicy, AuthoringLanguage, Board, BoardLayout,
    BoardPresentation, BoardProgram, Comment, CommentAnchor, CommentEncoding, CommentOrigin,
    CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, ExecutionKind, Expr, ExprKind,
    ForeignFidelity, HardwareBinding, HardwareKind, InterruptEdge, LValue, LadderBoxKind,
    LadderCell, LadderCellKind, LadderCoilKind, LadderContactKind, LadderNetwork, LadderRung,
    Literal, ModuleDecl, NodeId, ObKind, Pou, PouBody, PouKind, PullMode, ScalarType, SfcAction,
    SfcChart, SfcStep, SfcTransition, Stmt, StmtKind, StorageScope, StructField, TypeDecl, TypeRef,
    comment_multiset,
};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, CommentPlacement, ForeignExportDecision, FrontendOutput,
    FrontendResult, LanguageCapabilities, LanguageFrontend, LanguageId, Paradigm, RenderedComment,
    StCommentDialect, check_foreign_export, collect_foreign_sites, is_four_part_diagnostic,
    matches_comment_with_caps, render_comment_for_language, strip_program_spans,
};
use dendri_project::{load_project_str, to_toml_string};
use dendri_span::{Diagnostic, ErrorCode, Span};
use dendri_sync::{
    BoardDoc, EditDomain, ExportScope, SyncCommand, SyncEditError, SyncEngine, UndoStack,
    ViewProjection,
};
use dendri_validate::{MemoryProfileCatalog, ValidateOptions};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Clean Mock `LanguageFrontend` Implementations (No Language Crate Dependency)
// ---------------------------------------------------------------------------

type ParseFn = Box<dyn Fn(&str, &BoardContext) -> FrontendResult<BoardProgramBundle> + Send + Sync>;

struct MockFrontend {
    id: LanguageId,
    paradigm: Paradigm,
    caps: LanguageCapabilities,
    parser: ParseFn,
}

impl MockFrontend {
    fn new(
        id: LanguageId,
        paradigm: Paradigm,
        caps: LanguageCapabilities,
        parser: impl Fn(&str, &BoardContext) -> FrontendResult<BoardProgramBundle>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            id,
            paradigm,
            caps,
            parser: Box::new(parser),
        }
    }
}

impl LanguageFrontend for MockFrontend {
    fn id(&self) -> LanguageId {
        self.id
    }

    fn paradigm(&self) -> Paradigm {
        self.paradigm
    }

    fn parse(&self, src: &str, ctx: &BoardContext) -> FrontendResult<BoardProgramBundle> {
        (self.parser)(src, ctx)
    }

    fn emit(&self, bundle: &BoardProgramBundle, ctx: &BoardContext) -> FrontendResult<String> {
        let mut lines = Vec::new();
        let mut diags = Vec::new();

        for comment in &bundle.comments {
            let placement =
                CommentPlacement::from_comment(comment).unwrap_or(CommentPlacement::Leading);
            let rendered = self.render_comment(&comment.text, comment.origin.style, placement, ctx);
            diags.extend(rendered.diagnostics);
            lines.push(rendered.rendered);
        }

        for site in collect_foreign_sites(&bundle.program) {
            match check_foreign_export(&site, self.id, ctx) {
                Ok(ForeignExportDecision::VerbatimSameLanguage(raw)) => lines.push(raw),
                Ok(ForeignExportDecision::DemarcatedNotTranslated(rendered)) => {
                    diags.extend(rendered.diagnostics);
                    lines.push(rendered.rendered);
                }
                Err(diag) => return Err(vec![diag]),
            }
        }

        for pou in &bundle.program.pous {
            lines.push(format!("POU {} ({})", pou.name, self.id.as_str()));
        }

        Ok(FrontendOutput::with_diagnostics(lines.join("\n"), diags))
    }

    fn render_comment(
        &self,
        text: &str,
        style: CommentStyle,
        placement: CommentPlacement,
        ctx: &BoardContext,
    ) -> RenderedComment {
        render_comment_for_language(self.id, text, style, placement, ctx)
    }

    fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool {
        matches_comment_with_caps(self.id, &self.caps, rendered, stored_text)
    }

    fn capabilities(&self) -> LanguageCapabilities {
        self.caps.clone()
    }
}

// ---------------------------------------------------------------------------
// Rich Multi-POU / Multi-Paradigm Board Fixture
// ---------------------------------------------------------------------------

fn hw_digital_in(id: &str, label: &str, pin: &str) -> HardwareBinding {
    HardwareBinding {
        id: NodeId::new(id),
        label: label.to_owned(),
        pin: pin.to_owned(),
        kind: HardwareKind::DigitalIn,
        pull: PullMode::None,
        active_low: false,
        codec: None,
        range: None,
        span: Span::default(),
    }
}

fn hw_digital_out(id: &str, label: &str, pin: &str) -> HardwareBinding {
    HardwareBinding {
        id: NodeId::new(id),
        label: label.to_owned(),
        pin: pin.to_owned(),
        kind: HardwareKind::DigitalOut,
        pull: PullMode::None,
        active_low: false,
        codec: None,
        range: None,
        span: Span::default(),
    }
}

fn bool_data(id: &str, name: &str, init: bool) -> DataDecl {
    DataDecl {
        id: NodeId::new(id),
        name: name.to_owned(),
        scope: StorageScope::Var,
        ty: TypeRef::scalar(ScalarType::Bool),
        initial: Some(Expr::new(
            ExprKind::Literal(Literal::Bool(init)),
            Span::default(),
        )),
        span: Span::default(),
    }
}

fn dint_data(id: &str, name: &str, init: i128) -> DataDecl {
    DataDecl {
        id: NodeId::new(id),
        name: name.to_owned(),
        scope: StorageScope::Var,
        ty: TypeRef::scalar(ScalarType::Dint),
        initial: Some(Expr::new(
            ExprKind::Literal(Literal::Int(init)),
            Span::default(),
        )),
        span: Span::default(),
    }
}

fn assign_bool_stmt(id: &str, var: &str, val: bool) -> Stmt {
    Stmt::new(
        NodeId::new(id),
        StmtKind::Assign {
            target: LValue::Var {
                name: var.to_owned(),
                span: Span::default(),
            },
            value: Expr::new(ExprKind::Literal(Literal::Bool(val)), Span::default()),
        },
        Span::default(),
    )
}

fn make_rich_uno_board() -> Board {
    let mut blank_lines_before = BTreeMap::new();
    blank_lines_before.insert(NodeId::new("n_pou_startup"), 1);
    blank_lines_before.insert(NodeId::new("n_pou_main"), 2);

    let mut origin_files = BTreeMap::new();
    origin_files.insert(NodeId::new("n_pou_startup"), "startup.ino".to_owned());
    origin_files.insert(NodeId::new("n_pou_servo"), "servo_driver.cpp".to_owned());

    let mut node_coords = BTreeMap::new();
    node_coords.insert(NodeId::new("n_step_0"), [120.0, 48.0]);
    node_coords.insert(NodeId::new("n_step_1"), [120.0, 160.0]);

    Board {
        id: "mcu_cell".to_owned(),
        profile: "arduino-uno-r3".to_owned(),
        authoring_language: AuthoringLanguage::Cpp,
        execution: ExecutionKind::Cooperative,
        scan_period_us: None,
        layout: BoardLayout {
            x: 15.0,
            y: 25.0,
            rotation: 90,
            mirror_x: false,
            mirror_y: true,
        },
        program: BoardProgram {
            imports: vec!["std.arduino.*".to_owned(), "std.iec.timers".to_owned()],
            hardware: vec![
                hw_digital_in("n_hw_start", "START_BTN", "D2"),
                hw_digital_in("n_hw_estop", "ESTOP_ISR", "D3"),
                hw_digital_out("n_hw_motor", "MOTOR_OUT", "D13"),
            ],
            types: vec![
                TypeDecl::Struct {
                    id: NodeId::new("n_type_cfg"),
                    name: "ConveyorCfg".to_owned(),
                    fields: vec![StructField {
                        name: "speed".to_owned(),
                        ty: TypeRef::scalar(ScalarType::Dint),
                    }],
                    span: Span::default(),
                },
                TypeDecl::Foreign {
                    id: NodeId::new("n_type_servo"),
                    name: Some("Servo".to_owned()),
                    language: "cpp".to_owned(),
                    fidelity: ForeignFidelity::F1,
                    text: "Servo actuator;".to_owned(),
                    pack_hint: Some("arduino-servo".to_owned()),
                    span: Span::new(0, 15),
                },
            ],
            data: vec![
                bool_data("n_d_run", "motor_run", false),
                bool_data("n_d_fault", "fault_latched", false),
                dint_data("n_d_hits", "isr_hits", 0),
                dint_data("n_d_angle", "servo_angle", 90),
            ],
            modules: vec![ModuleDecl {
                id: NodeId::new("n_mod_util"),
                name: "safety_mod".to_owned(),
                imports: vec!["std.iec.timers".to_owned()],
                exports: vec!["ResetFault".to_owned()],
                types: vec![],
                data: vec![],
                pous: vec![Pou {
                    id: NodeId::new("n_pou_reset"),
                    name: "ResetFault".to_owned(),
                    kind: PouKind::Fc {
                        return_type: TypeRef::Void,
                    },
                    preferred_view: Some(AuthoringLanguage::St),
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Statements(vec![assign_bool_stmt(
                        "n_reset_stmt",
                        "fault_latched",
                        false,
                    )]),
                    span: Span::default(),
                }],
                span: Span::default(),
            }],
            pous: vec![
                // 1. OB_STARTUP
                Pou {
                    id: NodeId::new("n_pou_startup"),
                    name: "OB_STARTUP".to_owned(),
                    kind: PouKind::Ob {
                        ob: ObKind::Startup,
                    },
                    preferred_view: Some(AuthoringLanguage::Cpp),
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Statements(vec![assign_bool_stmt(
                        "n_init_stmt",
                        "motor_run",
                        false,
                    )]),
                    span: Span::default(),
                },
                // 2. OB_HW_INT (hardware edge ISR)
                Pou {
                    id: NodeId::new("n_pou_isr"),
                    name: "OB_HW_INT".to_owned(),
                    kind: PouKind::Ob {
                        ob: ObKind::HwInt {
                            pin_label: "ESTOP_ISR".to_owned(),
                            edge: InterruptEdge::Rising,
                            priority: 1,
                        },
                    },
                    preferred_view: Some(AuthoringLanguage::Cpp),
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Statements(vec![assign_bool_stmt(
                        "n_isr_stmt",
                        "fault_latched",
                        true,
                    )]),
                    span: Span::default(),
                },
                // 3. OB_MAIN (Ladder Network)
                Pou {
                    id: NodeId::new("n_pou_main"),
                    name: "OB_MAIN".to_owned(),
                    kind: PouKind::Ob { ob: ObKind::Main },
                    preferred_view: Some(AuthoringLanguage::Ladder),
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Network(LadderNetwork {
                        rungs: vec![
                            LadderRung {
                                id: NodeId::new("n_rung_0"),
                                order: 0,
                                cells: vec![
                                    LadderCell {
                                        id: NodeId::new("n_cell_0_0"),
                                        row: 0,
                                        col: 0,
                                        kind: LadderCellKind::Contact {
                                            kind: LadderContactKind::NormallyOpen,
                                            target: LValue::Var {
                                                name: "START_BTN".to_owned(),
                                                span: Span::default(),
                                            },
                                        },
                                    },
                                    LadderCell {
                                        id: NodeId::new("n_cell_0_1"),
                                        row: 0,
                                        col: 1,
                                        kind: LadderCellKind::Coil {
                                            kind: LadderCoilKind::Normal,
                                            target: LValue::Var {
                                                name: "motor_run".to_owned(),
                                                span: Span::default(),
                                            },
                                        },
                                    },
                                ],
                            },
                            LadderRung {
                                id: NodeId::new("n_rung_1"),
                                order: 1,
                                cells: vec![
                                    LadderCell {
                                        id: NodeId::new("n_cell_1_0"),
                                        row: 0,
                                        col: 0,
                                        kind: LadderCellKind::Contact {
                                            kind: LadderContactKind::NormallyClosed,
                                            target: LValue::Var {
                                                name: "fault_latched".to_owned(),
                                                span: Span::default(),
                                            },
                                        },
                                    },
                                    LadderCell {
                                        id: NodeId::new("n_cell_1_1"),
                                        row: 0,
                                        col: 1,
                                        kind: LadderCellKind::Coil {
                                            kind: LadderCoilKind::Set,
                                            target: LValue::Var {
                                                name: "motor_run".to_owned(),
                                                span: Span::default(),
                                            },
                                        },
                                    },
                                ],
                            },
                        ],
                    }),
                    span: Span::default(),
                },
                // 4. ServoHelper FC containing Stmt::Foreign and Expr::Foreign
                Pou {
                    id: NodeId::new("n_pou_servo"),
                    name: "ServoHelper".to_owned(),
                    kind: PouKind::Fc {
                        return_type: TypeRef::Void,
                    },
                    preferred_view: Some(AuthoringLanguage::Cpp),
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Statements(vec![
                        Stmt::new(
                            NodeId::new("n_foreign_stmt"),
                            StmtKind::Foreign {
                                language: "cpp".to_owned(),
                                fidelity: ForeignFidelity::F1,
                                text: "actuator.write(servo_angle);".to_owned(),
                                pack_hint: Some("arduino-servo".to_owned()),
                            },
                            Span::new(10, 38),
                        ),
                        Stmt::new(
                            NodeId::new("n_foreign_expr_stmt"),
                            StmtKind::Assign {
                                target: LValue::Var {
                                    name: "servo_angle".to_owned(),
                                    span: Span::new(40, 51),
                                },
                                value: Expr::new(
                                    ExprKind::Foreign {
                                        language: "cpp".to_owned(),
                                        text: "actuator.read()".to_owned(),
                                    },
                                    Span::new(54, 69),
                                ),
                            },
                            Span::new(40, 70),
                        ),
                    ]),
                    span: Span::default(),
                },
                // 5. CycleChart FB (GRAFCET / SFC)
                Pou {
                    id: NodeId::new("n_pou_sfc"),
                    name: "CycleChart".to_owned(),
                    kind: PouKind::Fb,
                    preferred_view: Some(AuthoringLanguage::Sfc),
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Chart(SfcChart {
                        steps: vec![
                            SfcStep {
                                id: NodeId::new("n_step_0"),
                                name: "S0".to_owned(),
                                initial: true,
                                actions: vec![SfcAction {
                                    id: NodeId::new("n_act_0"),
                                    qualifier: ActionQualifier::N,
                                    duration: None,
                                    target: Some(LValue::Var {
                                        name: "motor_run".to_owned(),
                                        span: Span::default(),
                                    }),
                                    body: vec![],
                                }],
                                macro_chart: None,
                            },
                            SfcStep {
                                id: NodeId::new("n_step_1"),
                                name: "S1".to_owned(),
                                initial: false,
                                actions: vec![SfcAction {
                                    id: NodeId::new("n_act_1"),
                                    qualifier: ActionQualifier::S,
                                    duration: None,
                                    target: Some(LValue::Var {
                                        name: "fault_latched".to_owned(),
                                        span: Span::default(),
                                    }),
                                    body: vec![],
                                }],
                                macro_chart: None,
                            },
                        ],
                        transitions: vec![SfcTransition {
                            id: NodeId::new("n_trans_0"),
                            name: "T0".to_owned(),
                            from_steps: vec!["S0".to_owned()],
                            to_steps: vec!["S1".to_owned()],
                            priority: 0,
                            receptivity: Expr::new(
                                ExprKind::Ident("START_BTN".to_owned()),
                                Span::default(),
                            ),
                        }],
                        evolve_to_stability: false,
                    }),
                    span: Span::default(),
                },
            ],
        },
        comments: vec![
            Comment {
                id: "c_hw_start".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_hw_start"),
                },
                placement: AstCommentPlacement::Body,
                token_ordinal: None,
                text: " S1 green pushbutton (NO)".to_owned(),
                origin: CommentOrigin {
                    language: "asl".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_startup".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_pou_startup"),
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " Initialize board safely on cold boot".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_isr".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_isr_stmt"),
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " Emergency stop ISR latches fault immediately".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_rung_0".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_rung_0"),
                },
                placement: AstCommentPlacement::Title,
                token_ordinal: None,
                text: " Motor start interlock".to_owned(),
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_rung_1".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_rung_1"),
                },
                placement: AstCommentPlacement::Title,
                token_ordinal: None,
                text: " Secondary latch rung".to_owned(),
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_cell_1_0".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_cell_1_0"),
                },
                placement: AstCommentPlacement::Body,
                token_ordinal: None,
                text: " NC fault guard contact".to_owned(),
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_servo_foreign".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_foreign_stmt"),
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " Drive external RC servo".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_step_1".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_step_1"),
                },
                placement: AstCommentPlacement::Body,
                token_ordinal: None,
                text: " Fault state in GRAFCET".to_owned(),
                origin: CommentOrigin {
                    language: "sfc".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
            Comment {
                id: "c_act_1".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_act_1"),
                },
                placement: AstCommentPlacement::Body,
                token_ordinal: None,
                text: " Latch fault output".to_owned(),
                origin: CommentOrigin {
                    language: "sfc".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            },
        ],
        assumptions: vec![Assumption {
            node: NodeId::new("n_foreign_stmt"),
            expr_ordinal: None,
            policy: AssumptionPolicy::NoEffect,
            value: None,
            note: "External servo mechanical load not yet modeled".to_owned(),
            span: Span::new(10, 38),
        }],
        presentation: BoardPresentation {
            eol_style: dendri_ast::EolStyle::Crlf,
            blank_lines_before,
            origin_files,
            node_coords,
        },
        span: Span::default(),
    }
}

// ---------------------------------------------------------------------------
// 1. Zero Silent Clobbering: Single-POU Edit Preserves Entire Untouched Board
// ---------------------------------------------------------------------------

#[test]
fn single_pou_edit_preserves_startup_isr_other_pous_foreign_assumptions_presentation_and_metadata()
{
    let catalog = MemoryProfileCatalog::builtin_reference();
    let ctx = BoardContext::arduino_uno_r3("mcu_cell");
    let engine = SyncEngine::new();
    let initial_board = make_rich_uno_board();
    let mut doc = BoardDoc::new(initial_board.clone());

    // Sanity: initial board validates cleanly (0 errors; only expected WARN_ASSUMPTION_ACTIVE).
    let pre_val = doc.validate(&catalog, &ValidateOptions::default());
    assert!(
        !pre_val.has_errors(),
        "pre-validation errors: {:?}",
        pre_val.all()
    );

    // Mock Ladder frontend returning ONLY an edited `OB_MAIN` (empty hardware, types, data, assumptions, presentation).
    let ladder_frontend = MockFrontend::new(
        LanguageId::Ladder,
        Paradigm::Ladder,
        LanguageCapabilities::ladder(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| {
            let edited_main = Pou {
                id: NodeId::new("n_pou_main"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                // Parser omits preferred_view: SyncEngine must preserve existing Some(Ladder)!
                preferred_view: None,
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork {
                    rungs: vec![LadderRung {
                        id: NodeId::new("n_rung_0"),
                        order: 0,
                        cells: vec![
                            LadderCell {
                                id: NodeId::new("n_cell_0_0"),
                                row: 0,
                                col: 0,
                                kind: LadderCellKind::Contact {
                                    kind: LadderContactKind::NormallyClosed,
                                    target: LValue::Var {
                                        name: "START_BTN".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                            LadderCell {
                                id: NodeId::new("n_cell_0_1"),
                                row: 0,
                                col: 1,
                                kind: LadderCellKind::Coil {
                                    kind: LadderCoilKind::Normal,
                                    target: LValue::Var {
                                        name: "motor_run".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                        ],
                    }],
                }),
                span: Span::default(),
            };
            let mut bundle = BoardProgramBundle::new(BoardProgram {
                pous: vec![edited_main],
                ..BoardProgram::default()
            });
            bundle.comments.push(Comment {
                id: "c_rung_0".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_rung_0"),
                },
                placement: AstCommentPlacement::Title,
                token_ordinal: None,
                text: " Motor start interlock".to_owned(),
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            });
            Ok(FrontendOutput::ok(bundle))
        },
    );

    let report = engine
        .apply_view_edit(
            &mut doc,
            "ladder:OB_MAIN",
            "RUNG 0: NC(START_BTN) -> COIL(motor_run)",
            &ladder_frontend,
            &ctx,
            &catalog,
        )
        .unwrap();

    assert_eq!(report.revision, 1);
    assert_eq!(doc.revision, 1);
    assert_eq!(report.domain, EditDomain::Ladder);
    assert_eq!(report.updated_pous, vec!["OB_MAIN".to_owned()]);

    // 1. OB_MAIN was updated AND kept its `preferred_view = Some(Ladder)`.
    let main_pou = doc
        .board
        .program
        .pous
        .iter()
        .find(|p| p.name == "OB_MAIN")
        .unwrap();
    assert_eq!(main_pou.preferred_view, Some(AuthoringLanguage::Ladder));
    if let PouBody::Network(net) = &main_pou.body {
        assert_eq!(net.rungs.len(), 1);
        assert!(matches!(
            net.rungs[0].cells[0].kind,
            LadderCellKind::Contact {
                kind: LadderContactKind::NormallyClosed,
                ..
            }
        ));
    } else {
        panic!("expected LadderNetwork body");
    }

    // 2. OB_STARTUP, OB_HW_INT, ServoHelper (with Foreign), CycleChart, and modules are 100% untouched!
    assert_eq!(doc.board.program.pous[0], initial_board.program.pous[0]); // OB_STARTUP
    assert_eq!(doc.board.program.pous[1], initial_board.program.pous[1]); // OB_HW_INT
    assert_eq!(doc.board.program.pous[3], initial_board.program.pous[3]); // ServoHelper (Foreign)
    assert_eq!(doc.board.program.pous[4], initial_board.program.pous[4]); // CycleChart
    assert_eq!(doc.board.program.modules, initial_board.program.modules);

    // 3. Hardware, Types (including TypeDecl::Foreign), Data, Imports, Assumptions, Presentation, Metadata 100% intact!
    assert_eq!(doc.board.program.imports, initial_board.program.imports);
    assert_eq!(doc.board.program.hardware, initial_board.program.hardware);
    assert_eq!(doc.board.program.types, initial_board.program.types);
    assert_eq!(doc.board.program.data, initial_board.program.data);
    assert_eq!(doc.board.assumptions, initial_board.assumptions);
    assert_eq!(doc.board.presentation, initial_board.presentation);
    assert_eq!(doc.board.layout, initial_board.layout);
    assert_eq!(
        doc.board.authoring_language,
        initial_board.authoring_language
    );
    assert_eq!(doc.board.profile, initial_board.profile);

    // 4. Comments on untouched nodes (including Tag Table Visual comment) are untouched, AND deleted rung 1 comments became dangling!
    let hw_comment = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_hw_start")
        .unwrap();
    assert_eq!(hw_comment, &initial_board.comments[0]);

    let rung1_comment = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_rung_1")
        .unwrap();
    assert_eq!(rung1_comment.placement, AstCommentPlacement::Dangling);
    assert_eq!(
        rung1_comment.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_pou_main")
        }
    );
    assert_eq!(rung1_comment.text, " Secondary latch rung");

    let cell_comment = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_cell_1_0")
        .unwrap();
    assert_eq!(cell_comment.placement, AstCommentPlacement::Dangling);
    assert_eq!(
        cell_comment.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_pou_main")
        }
    );
    assert_eq!(cell_comment.text, " NC fault guard contact");

    // C1/C2 multiset conservation holds identically!
    let before_multiset: BTreeMap<String, usize> = comment_multiset(&initial_board.comments)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    let after_multiset: BTreeMap<String, usize> = comment_multiset(&doc.board.comments)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    assert_eq!(before_multiset, after_multiset);

    // INFO_COMMENT_ORPHANED emitted for both newly orphaned comments
    let orphan_diags: Vec<&Diagnostic> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == ErrorCode::InfoCommentOrphaned)
        .collect();
    assert_eq!(orphan_diags.len(), 2);
}

// ---------------------------------------------------------------------------
// 2. Dirty Buffers: Invalid Syntax & Semantic Errors Never Clobber BoardDoc
// ---------------------------------------------------------------------------

#[test]
fn invalid_syntax_and_semantic_errors_retain_dirty_buffers_across_tab_switches_until_fixed_or_reverted()
 {
    let catalog = MemoryProfileCatalog::builtin_reference();
    let ctx = BoardContext::arduino_uno_r3("mcu_cell");
    let engine = SyncEngine::new();
    let initial_board = make_rich_uno_board();
    let mut doc = BoardDoc::new(initial_board.clone());

    let cpp_frontend = MockFrontend::new(
        LanguageId::Cpp,
        Paradigm::Textual,
        LanguageCapabilities::cpp(CommentEncoding::Utf8Passthrough),
        |src, _ctx| {
            if src.contains("SYNTAX_ERR") {
                return Err(vec![
                    Diagnostic::new(ErrorCode::ErrTomlSyntax, "unexpected token `{`")
                        .with_board_id("mcu_cell"),
                ]);
            }
            if src.contains("WAIT_IN_ISR") {
                // Valid AST syntax, but illegal Wait inside OB_HW_INT -> ERR_ISR_FORBIDDEN_OP!
                let bad_isr = Pou {
                    id: NodeId::new("n_pou_isr"),
                    name: "OB_HW_INT".to_owned(),
                    kind: PouKind::Ob {
                        ob: ObKind::HwInt {
                            pin_label: "ESTOP_ISR".to_owned(),
                            edge: InterruptEdge::Rising,
                            priority: 1,
                        },
                    },
                    preferred_view: Some(AuthoringLanguage::Cpp),
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Statements(vec![Stmt::new(
                        NodeId::new("n_bad_wait"),
                        StmtKind::Wait {
                            duration: Expr::new(
                                ExprKind::Literal(Literal::TimeUs(1000)),
                                Span::new(0, 10),
                            ),
                        },
                        Span::new(0, 12),
                    )]),
                    span: Span::default(),
                };
                return Ok(FrontendOutput::ok(BoardProgramBundle::new(BoardProgram {
                    pous: vec![bad_isr],
                    ..BoardProgram::default()
                })));
            }
            // Valid C++ edit to OB_STARTUP
            let good_startup = Pou {
                id: NodeId::new("n_pou_startup"),
                name: "OB_STARTUP".to_owned(),
                kind: PouKind::Ob {
                    ob: ObKind::Startup,
                },
                preferred_view: Some(AuthoringLanguage::Cpp),
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![assign_bool_stmt("n_init_stmt", "motor_run", true)]),
                span: Span::default(),
            };
            Ok(FrontendOutput::ok(BoardProgramBundle::new(BoardProgram {
                pous: vec![good_startup],
                ..BoardProgram::default()
            })))
        },
    );

    // 1. Parse error in `cpp:OB_STARTUP` -> rejected, board & revision untouched, dirty buffer stored!
    let broken_src = "void setup() { SYNTAX_ERR";
    let err = engine
        .apply_view_edit(
            &mut doc,
            "cpp:OB_STARTUP",
            broken_src,
            &cpp_frontend,
            &ctx,
            &catalog,
        )
        .unwrap_err();
    assert!(matches!(err, SyncEditError::ParseFailed { .. }));
    assert_eq!(doc.revision, 0);
    assert_eq!(doc.board, initial_board);
    assert!(!doc.can_undo());
    let dirty_startup = doc.dirty_buffer("cpp:OB_STARTUP").unwrap();
    assert_eq!(dirty_startup.source, broken_src);
    assert_eq!(dirty_startup.language, LanguageId::Cpp);
    assert_eq!(dirty_startup.base_revision, 0);
    assert_eq!(dirty_startup.error_count(), 1);

    // 2. Semantic validation error in `cpp:OB_HW_INT` (`WAIT_TIME` in ISR -> ERR_ISR_FORBIDDEN_OP)
    let bad_isr_src = "void isr() { WAIT_IN_ISR }";
    let val_err = engine
        .apply_view_edit(
            &mut doc,
            "cpp:OB_HW_INT",
            bad_isr_src,
            &cpp_frontend,
            &ctx,
            &catalog,
        )
        .unwrap_err();
    assert!(matches!(val_err, SyncEditError::ValidationFailed { .. }));
    assert!(
        val_err
            .diagnostics()
            .iter()
            .any(|d| d.code == ErrorCode::ErrIsrForbiddenOp)
    );
    assert_eq!(doc.revision, 0);
    assert_eq!(doc.board, initial_board);
    assert_eq!(
        doc.dirty_view_keys(),
        vec!["cpp:OB_HW_INT", "cpp:OB_STARTUP"]
    );

    // 3. Switch to another surface (I/O Tag Table) and commit a valid edit -> dirty buffers stay intact!
    engine
        .add_hardware_tag(
            &mut doc,
            hw_digital_out("n_hw_aux", "AUX_LAMP", "D8"),
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(doc.revision, 1);
    assert!(
        doc.dirty_buffer("cpp:OB_STARTUP")
            .unwrap()
            .is_stale(doc.revision)
    );
    assert!(
        doc.dirty_buffer("cpp:OB_HW_INT")
            .unwrap()
            .is_stale(doc.revision)
    );

    // Switching back to `cpp:OB_STARTUP` via `project_view_or_dirty` returns the dirty buffer, NOT overwritten!
    let proj = engine
        .project_view_or_dirty(
            &doc,
            "cpp:OB_STARTUP",
            ExportScope::Pou("OB_STARTUP"),
            &cpp_frontend,
            &ctx,
            false,
        )
        .unwrap();
    match proj {
        ViewProjection::Dirty(buf) => assert_eq!(buf.source, broken_src),
        ViewProjection::Clean(_) => panic!("dirty buffer was silently overwritten on tab switch!"),
    }

    // 4. Fixing `cpp:OB_STARTUP` clears ONLY `cpp:OB_STARTUP`, keeping `cpp:OB_HW_INT` dirty!
    engine
        .apply_view_edit(
            &mut doc,
            "cpp:OB_STARTUP",
            "void setup() { motor_run = true; }",
            &cpp_frontend,
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(doc.revision, 2);
    assert!(!doc.has_dirty_buffer("cpp:OB_STARTUP"));
    assert!(doc.has_dirty_buffer("cpp:OB_HW_INT"));

    // 5. Explicit `revert_dirty_buffer` discards `cpp:OB_HW_INT` and restores clean projection!
    let reverted = doc.revert_dirty_buffer("cpp:OB_HW_INT").unwrap();
    assert_eq!(reverted.source, bad_isr_src);
    assert!(!doc.has_any_dirty_buffers());
    let clean_proj = engine
        .project_view_or_dirty(
            &doc,
            "cpp:OB_HW_INT",
            ExportScope::Pou("OB_HW_INT"),
            &cpp_frontend,
            &ctx,
            false,
        )
        .unwrap();
    assert!(matches!(clean_proj, ViewProjection::Clean(_)));
}

// ---------------------------------------------------------------------------
// 3. C2 Orphan Preservation across Statement, Ladder Rung, and GRAFCET Step
//    (including Nested Statements and Sequential Parser Comment ID Reuse!)
// ---------------------------------------------------------------------------

#[test]
fn deleting_anchored_statement_nested_stmt_or_grafcet_step_orphans_comments_without_loss() {
    let catalog = MemoryProfileCatalog::builtin_reference();
    let ctx = BoardContext::arduino_uno_r3("mcu_cell");
    let engine = SyncEngine::new();
    let mut board = make_rich_uno_board();

    // Add a structured POU with nested If -> inner statement, each carrying comments.
    board.program.pous.push(Pou {
        id: NodeId::new("n_pou_ctrl"),
        name: "CtrlSt".to_owned(),
        kind: PouKind::Fc {
            return_type: TypeRef::Void,
        },
        preferred_view: Some(AuthoringLanguage::St),
        params: vec![],
        locals: vec![],
        body: PouBody::Statements(vec![
            Stmt::new(
                NodeId::new("n_if_1"),
                StmtKind::If {
                    condition: Expr::new(ExprKind::Ident("START_BTN".to_owned()), Span::default()),
                    then_body: vec![assign_bool_stmt("n_inner_stmt", "motor_run", true)],
                    elif_arms: vec![],
                    else_body: vec![],
                },
                Span::default(),
            ),
            assign_bool_stmt("n_after_stmt", "fault_latched", false),
        ]),
        span: Span::default(),
    });
    board.comments.push(Comment {
        id: "c_1".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_inner_stmt"),
        },
        placement: AstCommentPlacement::Leading,
        token_ordinal: None,
        text: " Safety inner statement comment".to_owned(),
        origin: CommentOrigin {
            language: "st".to_owned(),
            style: CommentStyle::ParenBlock,
        },
        span: Span::default(),
    });
    board.comments.push(Comment {
        id: "c_2".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_after_stmt"),
        },
        placement: AstCommentPlacement::Leading,
        token_ordinal: None,
        text: " Clear fault comment".to_owned(),
        origin: CommentOrigin {
            language: "st".to_owned(),
            style: CommentStyle::ParenBlock,
        },
        span: Span::default(),
    });

    let mut doc = BoardDoc::new(board.clone());

    // ST frontend deletes `n_inner_stmt` (keeping `n_if_1`) AND reuses sequential comment ID `"c_1"`
    // for the surviving `n_after_stmt`! SyncEngine MUST orphan `" Safety inner statement comment"`
    // onto `n_if_1` without letting the recycled `"c_1"` overwrite it!
    let st_delete_inner = MockFrontend::new(
        LanguageId::St,
        Paradigm::Textual,
        LanguageCapabilities::st(
            StCommentDialect::Iec61131Ed3,
            CommentEncoding::Utf8Passthrough,
        ),
        |_src, _ctx| {
            let pou = Pou {
                id: NodeId::new("n_pou_ctrl"),
                name: "CtrlSt".to_owned(),
                kind: PouKind::Fc {
                    return_type: TypeRef::Void,
                },
                preferred_view: Some(AuthoringLanguage::St),
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![
                    Stmt::new(
                        NodeId::new("n_if_1"),
                        StmtKind::If {
                            condition: Expr::new(
                                ExprKind::Ident("START_BTN".to_owned()),
                                Span::default(),
                            ),
                            then_body: vec![],
                            elif_arms: vec![],
                            else_body: vec![],
                        },
                        Span::default(),
                    ),
                    assign_bool_stmt("n_after_stmt", "fault_latched", false),
                ]),
                span: Span::default(),
            };
            let mut bundle = BoardProgramBundle::new(BoardProgram {
                pous: vec![pou],
                ..BoardProgram::default()
            });
            // Recycled sequential ID `"c_1"` on `n_after_stmt`!
            bundle.comments.push(Comment {
                id: "c_1".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_after_stmt"),
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " Clear fault comment".to_owned(),
                origin: CommentOrigin {
                    language: "st".to_owned(),
                    style: CommentStyle::ParenBlock,
                },
                span: Span::default(),
            });
            Ok(FrontendOutput::ok(bundle))
        },
    );

    let r1 = engine
        .apply_view_edit(
            &mut doc,
            "st:CtrlSt",
            "edited",
            &st_delete_inner,
            &ctx,
            &catalog,
        )
        .unwrap();
    assert!(
        r1.diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentOrphaned)
    );
    let orphaned_inner = doc.board.comments.iter().find(|c| c.id == "c_1").unwrap();
    assert_eq!(orphaned_inner.text, " Safety inner statement comment");
    assert_eq!(orphaned_inner.placement, AstCommentPlacement::Dangling);
    assert_eq!(
        orphaned_inner.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_if_1")
        }
    );
    let kept_after = doc.board.comments.iter().find(|c| c.id == "c_2").unwrap();
    assert_eq!(kept_after.text, " Clear fault comment");
    assert_eq!(kept_after.placement, AstCommentPlacement::Leading);

    // Next edit deletes the ENTIRE `n_if_1` -> the dangling comment on `n_if_1` bubbles up to `n_pou_ctrl`!
    let st_delete_if = MockFrontend::new(
        LanguageId::St,
        Paradigm::Textual,
        LanguageCapabilities::st(
            StCommentDialect::Iec61131Ed3,
            CommentEncoding::Utf8Passthrough,
        ),
        |_src, _ctx| {
            let pou = Pou {
                id: NodeId::new("n_pou_ctrl"),
                name: "CtrlSt".to_owned(),
                kind: PouKind::Fc {
                    return_type: TypeRef::Void,
                },
                preferred_view: Some(AuthoringLanguage::St),
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![assign_bool_stmt(
                    "n_after_stmt",
                    "fault_latched",
                    false,
                )]),
                span: Span::default(),
            };
            let mut bundle = BoardProgramBundle::new(BoardProgram {
                pous: vec![pou],
                ..BoardProgram::default()
            });
            bundle.comments.push(Comment {
                id: "c_2".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_after_stmt"),
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " Clear fault comment".to_owned(),
                origin: CommentOrigin {
                    language: "st".to_owned(),
                    style: CommentStyle::ParenBlock,
                },
                span: Span::default(),
            });
            Ok(FrontendOutput::ok(bundle))
        },
    );

    let _r2 = engine
        .apply_view_edit(
            &mut doc,
            "st:CtrlSt",
            "edited2",
            &st_delete_if,
            &ctx,
            &catalog,
        )
        .unwrap();
    let bubbled = doc.board.comments.iter().find(|c| c.id == "c_1").unwrap();
    assert_eq!(bubbled.text, " Safety inner statement comment");
    assert_eq!(bubbled.placement, AstCommentPlacement::Dangling);
    assert_eq!(
        bubbled.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_pou_ctrl")
        }
    );

    // Now delete GRAFCET step `S1` (which owns `n_step_1` + `n_act_1`) via SFC frontend -> BOTH become dangling on `n_pou_sfc`!
    let sfc_delete_step = MockFrontend::new(
        LanguageId::Sfc,
        Paradigm::Grafcet,
        LanguageCapabilities::sfc(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| {
            let chart_pou = Pou {
                id: NodeId::new("n_pou_sfc"),
                name: "CycleChart".to_owned(),
                kind: PouKind::Fb,
                preferred_view: Some(AuthoringLanguage::Sfc),
                params: vec![],
                locals: vec![],
                body: PouBody::Chart(SfcChart {
                    steps: vec![SfcStep {
                        id: NodeId::new("n_step_0"),
                        name: "S0".to_owned(),
                        initial: true,
                        actions: vec![SfcAction {
                            id: NodeId::new("n_act_0"),
                            qualifier: ActionQualifier::N,
                            duration: None,
                            target: Some(LValue::Var {
                                name: "motor_run".to_owned(),
                                span: Span::default(),
                            }),
                            body: vec![],
                        }],
                        macro_chart: None,
                    }],
                    transitions: vec![],
                    evolve_to_stability: false,
                }),
                span: Span::default(),
            };
            Ok(FrontendOutput::ok(BoardProgramBundle::new(BoardProgram {
                pous: vec![chart_pou],
                ..BoardProgram::default()
            })))
        },
    );

    let r3 = engine
        .apply_view_edit(
            &mut doc,
            "sfc:CycleChart",
            "S0 only",
            &sfc_delete_step,
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(
        r3.diagnostics
            .iter()
            .filter(|d| d.code == ErrorCode::InfoCommentOrphaned && d.message.contains("n_pou_sfc"))
            .count(),
        2
    );
    for cid in ["c_step_1", "c_act_1"] {
        let c = doc.board.comments.iter().find(|c| c.id == cid).unwrap();
        assert_eq!(c.placement, AstCommentPlacement::Dangling);
        assert_eq!(
            c.anchor,
            CommentAnchor::Node {
                id: NodeId::new("n_pou_sfc")
            }
        );
    }

    // Multiset invariant C1/C2 holds across ALL deletions!
    let before_ms: BTreeMap<String, usize> = comment_multiset(&board.comments)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    let after_ms: BTreeMap<String, usize> = comment_multiset(&doc.board.comments)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    assert_eq!(before_ms, after_ms);
}

// ---------------------------------------------------------------------------
// 4. C4 Re-Import Comment Idempotence (Degraded Projections Never Overwrite Verbatim)
// ---------------------------------------------------------------------------

#[test]
fn degraded_comment_reimport_preserves_verbatim_original_and_real_human_edit_updates_it() {
    let catalog = MemoryProfileCatalog::builtin_reference();
    let mut ctx = BoardContext::arduino_uno_r3("mcu_cell");
    ctx.comment_encoding = CommentEncoding::AsciiFold;
    ctx.st_dialect = StCommentDialect::StrictEd2;
    let engine = SyncEngine::new();

    let verbatim_bidi_utf8 = " Motor ação \u{202E}24V ±5% 🚀";
    let verbatim_nested_st = " outer (* inner *) and *)";
    let verbatim_inner_expr = " gain /* tricky */";

    let mut board = make_rich_uno_board();
    board.comments.push(Comment {
        id: "c_c4_utf8".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_init_stmt"),
        },
        placement: AstCommentPlacement::Leading,
        token_ordinal: None,
        text: verbatim_bidi_utf8.to_owned(),
        origin: CommentOrigin {
            language: "cpp".to_owned(),
            style: CommentStyle::Line,
        },
        span: Span::default(),
    });
    board.comments.push(Comment {
        id: "c_c4_nested".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_init_stmt"),
        },
        placement: AstCommentPlacement::Trailing,
        token_ordinal: None,
        text: verbatim_nested_st.to_owned(),
        origin: CommentOrigin {
            language: "st".to_owned(),
            style: CommentStyle::ParenBlock,
        },
        span: Span::default(),
    });
    board.comments.push(Comment {
        id: "c_c4_inner".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_init_stmt"),
        },
        placement: AstCommentPlacement::Inner,
        token_ordinal: Some(2),
        text: verbatim_inner_expr.to_owned(),
        origin: CommentOrigin {
            language: "cpp".to_owned(),
            style: CommentStyle::Block,
        },
        span: Span::default(),
    });

    let mut doc = BoardDoc::new(board);

    // Project comments through ST StrictEd2 + AsciiFold -> degraded strings!
    let rendered_utf8 = render_comment_for_language(
        LanguageId::St,
        verbatim_bidi_utf8,
        CommentStyle::Line,
        CommentPlacement::Leading,
        &ctx,
    );
    assert!(rendered_utf8.degraded);
    assert!(rendered_utf8.rendered.is_ascii());

    let rendered_nested = render_comment_for_language(
        LanguageId::St,
        verbatim_nested_st,
        CommentStyle::ParenBlock,
        CommentPlacement::Trailing,
        &ctx,
    );
    assert!(rendered_nested.degraded);

    let rendered_inner = render_comment_for_language(
        LanguageId::St,
        verbatim_inner_expr,
        CommentStyle::Block,
        CommentPlacement::Leading, // relocated from Inner on cross-language export!
        &ctx,
    );

    let st_caps = LanguageCapabilities::st(StCommentDialect::StrictEd2, CommentEncoding::AsciiFold);
    let r_utf8_text = rendered_utf8.rendered.clone();
    let r_nested_text = rendered_nested.rendered.clone();
    let r_inner_text = rendered_inner.rendered.clone();

    let st_reimport = MockFrontend::new(
        LanguageId::St,
        Paradigm::Textual,
        st_caps,
        move |src, _ctx| {
            let startup = Pou {
                id: NodeId::new("n_pou_startup"),
                name: "OB_STARTUP".to_owned(),
                kind: PouKind::Ob {
                    ob: ObKind::Startup,
                },
                preferred_view: Some(AuthoringLanguage::Cpp),
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![assign_bool_stmt("n_init_stmt", "motor_run", true)]),
                span: Span::default(),
            };
            let mut bundle = BoardProgramBundle::new(BoardProgram {
                pous: vec![startup],
                ..BoardProgram::default()
            });
            // Parser regenerates fresh comment IDs and returns degraded projections!
            let first_text = if src.contains("HUMAN_EDIT") {
                " genuinely edited by engineer".to_owned()
            } else {
                r_utf8_text.clone()
            };
            bundle.comments = vec![
                Comment {
                    id: "c_fresh_1".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_pou_startup"),
                    },
                    placement: AstCommentPlacement::Leading,
                    token_ordinal: None,
                    text: " Initialize board safely on cold boot".to_owned(),
                    origin: CommentOrigin {
                        language: "st".to_owned(),
                        style: CommentStyle::ParenBlock,
                    },
                    span: Span::default(),
                },
                Comment {
                    id: "c_fresh_2".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_init_stmt"),
                    },
                    placement: AstCommentPlacement::Leading,
                    token_ordinal: None,
                    text: first_text,
                    origin: CommentOrigin {
                        language: "st".to_owned(),
                        style: CommentStyle::ParenBlock,
                    },
                    span: Span::default(),
                },
                Comment {
                    id: "c_fresh_3".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_init_stmt"),
                    },
                    placement: AstCommentPlacement::Trailing,
                    token_ordinal: None,
                    text: r_nested_text.clone(),
                    origin: CommentOrigin {
                        language: "st".to_owned(),
                        style: CommentStyle::ParenBlock,
                    },
                    span: Span::default(),
                },
                // Relocated Inner comment parsed back as Leading on `n_init_stmt`:
                Comment {
                    id: "c_fresh_4".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_init_stmt"),
                    },
                    placement: AstCommentPlacement::Leading,
                    token_ordinal: None,
                    text: r_inner_text.clone(),
                    origin: CommentOrigin {
                        language: "st".to_owned(),
                        style: CommentStyle::ParenBlock,
                    },
                    span: Span::default(),
                },
            ];
            Ok(FrontendOutput::ok(bundle))
        },
    );

    // 1. Re-import degraded projection: ALL THREE verbatim originals & metadata MUST remain untouched!
    engine
        .apply_view_edit(
            &mut doc,
            "st:OB_STARTUP",
            "reimport_degraded",
            &st_reimport,
            &ctx,
            &catalog,
        )
        .unwrap();

    let stored_utf8 = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_c4_utf8")
        .unwrap();
    assert_eq!(stored_utf8.text, verbatim_bidi_utf8);
    assert_eq!(stored_utf8.origin.language, "cpp");
    assert_eq!(stored_utf8.origin.style, CommentStyle::Line);

    let stored_nested = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_c4_nested")
        .unwrap();
    assert_eq!(stored_nested.text, verbatim_nested_st);
    assert_eq!(stored_nested.origin.style, CommentStyle::ParenBlock);

    let stored_inner = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_c4_inner")
        .unwrap();
    assert_eq!(stored_inner.text, verbatim_inner_expr);
    assert_eq!(stored_inner.placement, AstCommentPlacement::Inner);
    assert_eq!(stored_inner.token_ordinal, Some(2));

    // 2. Genuine human edit on the first comment: `matches_comment` returns false -> stored text updates!
    engine
        .apply_view_edit(
            &mut doc,
            "st:OB_STARTUP",
            "HUMAN_EDIT",
            &st_reimport,
            &ctx,
            &catalog,
        )
        .unwrap();
    let edited_utf8 = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_c4_utf8")
        .unwrap();
    assert_eq!(edited_utf8.text, " genuinely edited by engineer");
    // Sibling degraded comments STILL stay verbatim!
    let still_nested = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_c4_nested")
        .unwrap();
    assert_eq!(still_nested.text, verbatim_nested_st);
}

// ---------------------------------------------------------------------------
// 5. Unified Multi-Domain Undo / Redo + Dirty-Buffer Safety + Monotonic Revision
// ---------------------------------------------------------------------------

#[test]
fn unified_multidomain_undo_redo_restores_exact_snapshots_monotonically_preserving_dirty_buffers() {
    let catalog = MemoryProfileCatalog::builtin_reference();
    let ctx = BoardContext::arduino_uno_r3("mcu_cell");
    let engine = SyncEngine::new();
    let mut doc = BoardDoc::new(make_rich_uno_board());

    let mut snapshots: Vec<Board> = vec![doc.board.clone()];

    // Step 1: Code view edit (C++)
    let cpp_fe = MockFrontend::new(
        LanguageId::Cpp,
        Paradigm::Textual,
        LanguageCapabilities::cpp(CommentEncoding::Utf8Passthrough),
        |src, _ctx| {
            if src == "BAD_BUFFER" {
                return Err(vec![Diagnostic::new(
                    ErrorCode::ErrTomlSyntax,
                    "unclosed paren",
                )]);
            }
            let pou = Pou {
                id: NodeId::new("n_pou_startup"),
                name: "OB_STARTUP".to_owned(),
                kind: PouKind::Ob {
                    ob: ObKind::Startup,
                },
                preferred_view: Some(AuthoringLanguage::Cpp),
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![assign_bool_stmt("n_init_stmt", "motor_run", true)]),
                span: Span::default(),
            };
            Ok(FrontendOutput::ok(BoardProgramBundle::new(BoardProgram {
                pous: vec![pou],
                ..BoardProgram::default()
            })))
        },
    );
    let r1 = engine
        .apply_view_edit(&mut doc, "cpp:OB_STARTUP", "ok", &cpp_fe, &ctx, &catalog)
        .unwrap();
    assert_eq!(r1.domain, EditDomain::Code);
    assert_eq!(doc.revision, 1);
    snapshots.push(doc.board.clone());

    // Step 2: Ladder view edit
    let ladder_fe = MockFrontend::new(
        LanguageId::Ladder,
        Paradigm::Ladder,
        LanguageCapabilities::ladder(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| {
            let pou = Pou {
                id: NodeId::new("n_pou_main"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: Some(AuthoringLanguage::Ladder),
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork {
                    rungs: vec![LadderRung {
                        id: NodeId::new("n_rung_0"),
                        order: 0,
                        cells: vec![LadderCell {
                            id: NodeId::new("n_cell_wire"),
                            row: 0,
                            col: 0,
                            kind: LadderCellKind::Wire,
                        }],
                    }],
                }),
                span: Span::default(),
            };
            Ok(FrontendOutput::ok(BoardProgramBundle::new(BoardProgram {
                pous: vec![pou],
                ..BoardProgram::default()
            })))
        },
    );
    let r2 = engine
        .apply_view_edit(
            &mut doc,
            "ladder:OB_MAIN",
            "wire",
            &ladder_fe,
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(r2.domain, EditDomain::Ladder);
    assert_eq!(doc.revision, 2);
    snapshots.push(doc.board.clone());

    // Step 3: GRAFCET view edit
    let sfc_fe = MockFrontend::new(
        LanguageId::Sfc,
        Paradigm::Grafcet,
        LanguageCapabilities::sfc(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| {
            let pou = Pou {
                id: NodeId::new("n_pou_sfc"),
                name: "CycleChart".to_owned(),
                kind: PouKind::Fb,
                preferred_view: Some(AuthoringLanguage::Sfc),
                params: vec![],
                locals: vec![],
                body: PouBody::Chart(SfcChart {
                    steps: vec![SfcStep {
                        id: NodeId::new("n_step_0"),
                        name: "S0".to_owned(),
                        initial: true,
                        actions: vec![],
                        macro_chart: None,
                    }],
                    transitions: vec![],
                    evolve_to_stability: true,
                }),
                span: Span::default(),
            };
            Ok(FrontendOutput::ok(BoardProgramBundle::new(BoardProgram {
                pous: vec![pou],
                ..BoardProgram::default()
            })))
        },
    );
    let r3 = engine
        .apply_view_edit(&mut doc, "sfc:CycleChart", "s0", &sfc_fe, &ctx, &catalog)
        .unwrap();
    assert_eq!(r3.domain, EditDomain::Grafcet);
    assert_eq!(doc.revision, 3);
    snapshots.push(doc.board.clone());

    // Step 4: I/O Tag Table edit
    let r4 = engine
        .add_hardware_tag(
            &mut doc,
            hw_digital_out("n_hw_lamp", "STATUS_LAMP", "D7"),
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(r4.domain, EditDomain::IoTagTable);
    assert_eq!(doc.revision, 4);
    snapshots.push(doc.board.clone());

    // Step 5: Verbatim comment edit
    let r5 = engine
        .update_comment_text(
            &mut doc,
            "c_hw_start",
            " S1 green start (24V)\r\n line 2",
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(r5.domain, EditDomain::Comments);
    assert_eq!(
        doc.board
            .comments
            .iter()
            .find(|c| c.id == "c_hw_start")
            .unwrap()
            .text,
        " S1 green start (24V)\n line 2"
    );
    assert_eq!(doc.revision, 5);
    snapshots.push(doc.board.clone());

    // Step 6: Simulation assumption edit
    let r6 = engine
        .record_assumption(
            &mut doc,
            Assumption {
                node: NodeId::new("n_foreign_expr_stmt"),
                expr_ordinal: Some(0),
                policy: AssumptionPolicy::ReturnValue,
                value: Some("90".to_owned()),
                note: "Assume centered servo readback".to_owned(),
                span: Span::default(),
            },
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(r6.domain, EditDomain::Assumptions);
    assert!(
        r6.diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::WarnAssumptionActive)
    );
    assert_eq!(doc.revision, 6);
    snapshots.push(doc.board.clone());

    // Step 7: PresentationStore edit (also tests clamping blank_lines > 2 to 2)
    let r7 = engine
        .set_blank_lines_before(&mut doc, NodeId::new("n_pou_main"), Some(9), &ctx, &catalog)
        .unwrap();
    assert_eq!(r7.domain, EditDomain::Presentation);
    assert_eq!(
        doc.board
            .presentation
            .blank_lines_before
            .get(&NodeId::new("n_pou_main")),
        Some(&2)
    );
    assert_eq!(doc.revision, 7);
    snapshots.push(doc.board.clone());

    // Step 8: Canvas layout edit
    let r8 = engine
        .update_board_layout(
            &mut doc,
            BoardLayout {
                x: 200.0,
                y: 300.0,
                rotation: 180,
                mirror_x: true,
                mirror_y: false,
            },
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(r8.domain, EditDomain::Canvas);
    assert_eq!(doc.revision, 8);
    snapshots.push(doc.board.clone());

    // Record an uncommitted dirty buffer in an unrelated view BEFORE undoing!
    assert!(
        engine
            .apply_view_edit(
                &mut doc,
                "cpp:scratch",
                "BAD_BUFFER",
                &cpp_fe,
                &ctx,
                &catalog
            )
            .is_err()
    );
    assert!(doc.has_dirty_buffer("cpp:scratch"));

    // Undo ALL 8 transactions back to snapshot 0 -> monotonic revisions 9..=16 + dirty buffer intact!
    let mut expected_rev = 8u64;
    for (step, expected_board) in snapshots.iter().enumerate().take(8).rev() {
        let outcome = engine.undo(&mut doc).unwrap();
        expected_rev += 1;
        assert_eq!(outcome.revision, expected_rev);
        assert_eq!(doc.revision, expected_rev);
        assert_eq!(
            &doc.board, expected_board,
            "mismatch after undoing to step {step}"
        );
        assert_eq!(
            doc.dirty_buffer("cpp:scratch").unwrap().source,
            "BAD_BUFFER"
        );
    }
    assert!(!doc.can_undo());
    assert!(engine.undo(&mut doc).is_none());

    // Redo ALL 8 transactions forward to snapshot 8 -> monotonic revisions 17..=24 + dirty buffer intact!
    for (step, expected_board) in snapshots.iter().enumerate().skip(1) {
        let outcome = engine.redo(&mut doc).unwrap();
        expected_rev += 1;
        assert_eq!(outcome.revision, expected_rev);
        assert_eq!(doc.revision, expected_rev);
        assert_eq!(
            &doc.board, expected_board,
            "mismatch after redoing to step {step}"
        );
        assert_eq!(
            doc.dirty_buffer("cpp:scratch").unwrap().source,
            "BAD_BUFFER"
        );
    }
    assert!(!doc.can_redo());

    // Undo 2 steps, then apply a new command -> redo branch must clear!
    assert!(engine.undo(&mut doc).is_some());
    assert!(engine.undo(&mut doc).is_some());
    assert!(doc.can_redo());
    engine
        .set_node_coords(
            &mut doc,
            NodeId::new("n_step_0"),
            Some([42.0, 84.0]),
            &ctx,
            &catalog,
        )
        .unwrap();
    assert!(!doc.can_redo());
}

// ---------------------------------------------------------------------------
// 6. Cross-Language Foreign Export Gate (`export_view`) & Same-Language Never Blocked
// ---------------------------------------------------------------------------

#[test]
fn cross_language_foreign_export_is_blocked_without_allow_foreign_and_permitted_with_it() {
    let ctx = BoardContext::arduino_uno_r3("mcu_cell");
    let engine = SyncEngine::new();
    let mut board = make_rich_uno_board();

    // Add an Inner comment on OB_STARTUP to verify INFO_COMMENT_RELOCATED on cross-language export!
    board.comments.push(Comment {
        id: "c_inner_startup".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_init_stmt"),
        },
        placement: AstCommentPlacement::Inner,
        token_ordinal: Some(1),
        text: " inline init hint".to_owned(),
        origin: CommentOrigin {
            language: "cpp".to_owned(),
            style: CommentStyle::Block,
        },
        span: Span::default(),
    });
    let doc = BoardDoc::new(board.clone());

    let cpp_fe = MockFrontend::new(
        LanguageId::Cpp,
        Paradigm::Textual,
        LanguageCapabilities::cpp(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| Ok(FrontendOutput::ok(BoardProgramBundle::default())),
    );
    let st_fe = MockFrontend::new(
        LanguageId::St,
        Paradigm::Textual,
        LanguageCapabilities::st(
            StCommentDialect::Iec61131Ed3,
            CommentEncoding::Utf8Passthrough,
        ),
        |_src, _ctx| Ok(FrontendOutput::ok(BoardProgramBundle::default())),
    );
    let ladder_fe = MockFrontend::new(
        LanguageId::Ladder,
        Paradigm::Ladder,
        LanguageCapabilities::ladder(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| Ok(FrontendOutput::ok(BoardProgramBundle::default())),
    );

    // 1. Same-language (C++) whole-board emit is NEVER blocked, even with allow_foreign = false!
    //    Also restores CRLF EOL style on same-language textual emit!
    let cpp_out = engine
        .export_view(&doc, ExportScope::Board, &cpp_fe, &ctx, false)
        .unwrap();
    assert!(cpp_out.value.contains("actuator.write(servo_angle);"));
    assert!(cpp_out.value.contains("actuator.read()"));
    assert!(cpp_out.value.contains("\r\n"));
    assert!(
        !cpp_out
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentRelocated)
    );

    // 2. Cross-language (ST & Ladder) whole-board export WITHOUT allow_foreign MUST fail with
    //    four-part `ERR_FOREIGN_EXPORT_BLOCKED` for all 3 cpp Foreign sites (Decl, Stmt, Expr)!
    let st_blocked = engine
        .export_view(&doc, ExportScope::Board, &st_fe, &ctx, false)
        .unwrap_err();
    assert_eq!(st_blocked.len(), 3);
    for diag in &st_blocked {
        assert_eq!(diag.code, ErrorCode::ErrForeignExportBlocked);
        assert!(
            is_four_part_diagnostic(diag),
            "expected 4-part diag: {diag:?}"
        );
    }

    let ladder_blocked = engine
        .export_view(&doc, ExportScope::Board, &ladder_fe, &ctx, false)
        .unwrap_err();
    assert_eq!(ladder_blocked.len(), 3);
    assert!(
        ladder_blocked
            .iter()
            .all(|d| d.code == ErrorCode::ErrForeignExportBlocked)
    );

    // 3. Cross-language (ST) WITH allow_foreign = true SUCCEEDS, emits "NOT TRANSLATED" blocks,
    //    and emits INFO_COMMENT_RELOCATED without mutating `doc.board.comments`!
    let st_allowed = engine
        .export_view(&doc, ExportScope::Board, &st_fe, &ctx, true)
        .unwrap();
    assert!(st_allowed.value.contains("NOT TRANSLATED"));
    assert!(st_allowed.value.contains("actuator.write(servo_angle);"));
    assert!(
        st_allowed
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentRelocated)
    );
    // Stored board comments were NOT mutated by export!
    let stored_inner = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_inner_startup")
        .unwrap();
    assert_eq!(stored_inner.placement, AstCommentPlacement::Inner);
    assert_eq!(stored_inner.token_ordinal, Some(1));

    // 4. Scoped POU export (`04` §3.3): exporting `OB_MAIN` (no Foreign) to ST SUCCEEDS even
    //    when allow_foreign = false, while exporting `ServoHelper` (has Foreign) is BLOCKED!
    let main_st = engine
        .export_view(&doc, ExportScope::Pou("OB_MAIN"), &st_fe, &ctx, false)
        .unwrap();
    assert!(main_st.value.contains("POU OB_MAIN (st)"));

    let servo_blocked = engine
        .export_view(&doc, ExportScope::Pou("ServoHelper"), &st_fe, &ctx, false)
        .unwrap_err();
    assert_eq!(servo_blocked.len(), 2);
    assert!(
        servo_blocked
            .iter()
            .all(|d| d.code == ErrorCode::ErrForeignExportBlocked)
    );
}

// ---------------------------------------------------------------------------
// 7. I/O Tag Table, Atomic Batch, Validation Guard, Standalone UndoStack,
//    and Multi-Board `.toml` Project Round-Trip
// ---------------------------------------------------------------------------

#[test]
fn io_tag_table_validation_guards_orphan_preservation_and_atomic_batch_work_cleanly() {
    let catalog = MemoryProfileCatalog::builtin_reference();
    let ctx = BoardContext::arduino_uno_r3("mcu_cell");
    let engine = SyncEngine::new();
    let mut doc = BoardDoc::new(make_rich_uno_board());

    // 1. Adding hardware tag on an already-bound pin (`D2`) fails with ERR_HARDWARE_PIN_CONFLICT.
    let conflict = engine
        .add_hardware_tag(
            &mut doc,
            hw_digital_in("n_hw_dup", "DUP_IN", "D2"),
            &ctx,
            &catalog,
        )
        .unwrap_err();
    assert!(
        conflict
            .diagnostics()
            .iter()
            .any(|d| d.code == ErrorCode::ErrHardwarePinConflict)
    );
    assert_eq!(doc.revision, 0);

    // 2. Adding hardware tag with unsupported pull-down on Uno fails with ERR_PIN_CAPABILITY_MISMATCH.
    let mut bad_pull = hw_digital_in("n_hw_pd", "BAD_PD", "D4");
    bad_pull.pull = PullMode::PullDown;
    let pull_err = engine
        .add_hardware_tag(&mut doc, bad_pull, &ctx, &catalog)
        .unwrap_err();
    assert!(
        pull_err
            .diagnostics()
            .iter()
            .any(|d| d.code == ErrorCode::ErrPinCapabilityMismatch)
    );
    assert_eq!(doc.revision, 0);

    // 3. Removing `ESTOP_ISR` (referenced by OB_HW_INT) fails with ERR_UNRESOLVED_ISR!
    let isr_err = engine
        .remove_hardware_tag(&mut doc, NodeId::new("n_hw_estop"), &ctx, &catalog)
        .unwrap_err();
    assert!(
        isr_err
            .diagnostics()
            .iter()
            .any(|d| d.code == ErrorCode::ErrUnresolvedIsr)
    );
    assert_eq!(doc.revision, 0);

    // 4. Atomic Batch: add hardware tag `AUX_IN` on `D6` + comment on `AUX_IN` + POU reading `AUX_IN`
    let batch = SyncCommand::Batch {
        label: "Add AUX_IN tag, note, and helper POU".to_owned(),
        commands: vec![
            SyncCommand::AddHardwareTag(hw_digital_in("n_hw_aux_in", "AUX_IN", "D6")),
            SyncCommand::AddComment(Comment {
                id: "c_aux_in".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_hw_aux_in"),
                },
                placement: AstCommentPlacement::Body,
                token_ordinal: None,
                text: " Auxiliary limit switch".to_owned(),
                origin: CommentOrigin {
                    language: "asl".to_owned(),
                    style: CommentStyle::Visual,
                },
                span: Span::default(),
            }),
            SyncCommand::UpsertPou(Pou {
                id: NodeId::new("n_pou_aux"),
                name: "ReadAux".to_owned(),
                kind: PouKind::Fc {
                    return_type: TypeRef::scalar(ScalarType::Bool),
                },
                preferred_view: Some(AuthoringLanguage::St),
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![Stmt::new(
                    NodeId::new("n_ret_aux"),
                    StmtKind::Return {
                        value: Some(Expr::new(
                            ExprKind::Ident("AUX_IN".to_owned()),
                            Span::default(),
                        )),
                    },
                    Span::default(),
                )]),
                span: Span::default(),
            }),
        ],
    };
    let batch_rep = engine
        .apply_command(&mut doc, batch, &ctx, &catalog)
        .unwrap();
    assert_eq!(batch_rep.domain, EditDomain::Composite);
    assert_eq!(doc.revision, 1);

    // Remove the helper POU first, then remove `AUX_IN` -> comment `c_aux_in` is preserved on board with INFO_COMMENT_ORPHANED!
    engine
        .remove_pou(&mut doc, NodeId::new("n_pou_aux"), &ctx, &catalog)
        .unwrap();
    let del_tag_rep = engine
        .remove_hardware_tag(&mut doc, NodeId::new("n_hw_aux_in"), &ctx, &catalog)
        .unwrap();
    assert!(
        del_tag_rep
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentOrphaned)
    );
    let orphaned_tag_comment = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_aux_in")
        .unwrap();
    assert_eq!(orphaned_tag_comment.text, " Auxiliary limit switch");
    assert_eq!(
        orphaned_tag_comment.anchor,
        CommentAnchor::Board {
            id: "mcu_cell".to_owned()
        }
    );

    // Undo all 3 transactions back to initial board cleanly
    assert!(engine.undo(&mut doc).is_some());
    assert!(engine.undo(&mut doc).is_some());
    assert!(engine.undo(&mut doc).is_some());
    assert_eq!(doc.board, make_rich_uno_board());
}

#[test]
fn bounded_undo_stack_and_multiboard_project_toml_roundtrip_preserve_siblings() {
    let toml_src = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Sync-Roundtrip-Cell"

[[comments]]
id = "c_proj_1"
anchor = { kind = "component", id = "lamp_1" }
placement = "body"
text = " Panel pilot lamp"
origin = { language = "asl", style = "visual" }

[[boards]]
id = "plc_main"
profile = "plc-1200-open-1214"
authoring_language = "ladder"
execution = "plc_scan"
scan_period_us = 10000
[boards.layout]
x = 1.0
y = 2.0
[boards.program]
imports = ["std.iec.timers"]
[[boards.program.hardware]]
id = "n_hw_in"
label = "BTN_START"
pin = "Ia.0"
kind = "digital_in"
[[boards.program.pous]]
id = "n_pou_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
preferred_view = "ladder"
body_kind = "network"
[[boards.program.pous.network.rungs]]
id = "n_r0"
order = 0
[[boards.program.pous.network.rungs.cells]]
id = "n_c0"
row = 0
col = 0
kind = "contact"
contact_kind = "normally_open"
target = "BTN_START"

[[boards]]
id = "mcu_aux"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"
[boards.layout]
x = 40.0
y = 2.0
[boards.program]
imports = ["std.arduino.*"]
[[boards.program.pous]]
id = "n_mcu_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[components]]
id = "lamp_1"
profile = "signal-pilot-lamp"
label = "H1"
[components.layout]
x = 60.0
y = 10.0
[components.params]

[[wires]]
id = "w_1"
from = "plc_main.Qa.0"
to = "lamp_1.X1"
signal = "digital"
waypoints = []
"#;

    let mut project = load_project_str(toml_src, None).unwrap();
    let mut doc = BoardDoc::from_project_toml(toml_src, "plc_main", None)
        .unwrap()
        .unwrap();

    let catalog = MemoryProfileCatalog::builtin_reference();
    let ctx = BoardContext::plc_1200_open_1214("plc_main");
    let engine = SyncEngine::new();

    engine
        .add_hardware_tag(
            &mut doc,
            hw_digital_out("n_hw_out", "LAMP_OUT", "Qa.0"),
            &ctx,
            &catalog,
        )
        .unwrap();
    engine
        .set_origin_file(
            &mut doc,
            NodeId::new("n_pou_main"),
            Some("main.ld".to_owned()),
            &ctx,
            &catalog,
        )
        .unwrap();

    // Write back into multi-board project, serialize to TOML, reload, verify sibling board & wires untouched!
    assert!(doc.write_back_to_project(&mut project));
    let serialized = to_toml_string(&project).unwrap();
    let reloaded = load_project_str(&serialized, None).unwrap();

    assert_eq!(reloaded.boards.len(), 2);
    assert_eq!(reloaded.boards[0].program.hardware.len(), 2);
    assert_eq!(
        reloaded.boards[0]
            .presentation
            .origin_files
            .get(&NodeId::new("n_pou_main")),
        Some(&"main.ld".to_owned())
    );
    // Sibling MCU board, components, wires, and top-level comments remain 100% semantically untouched!
    assert_eq!(reloaded.boards[1].id, project.boards[1].id);
    assert_eq!(reloaded.boards[1].profile, project.boards[1].profile);
    assert_eq!(
        reloaded.boards[1].authoring_language,
        project.boards[1].authoring_language
    );
    assert_eq!(reloaded.boards[1].layout, project.boards[1].layout);
    assert_eq!(
        strip_program_spans(&reloaded.boards[1].program),
        strip_program_spans(&project.boards[1].program)
    );
    assert_eq!(reloaded.components.len(), project.components.len());
    assert_eq!(reloaded.components[0].id, project.components[0].id);
    assert_eq!(reloaded.components[0].layout, project.components[0].layout);
    assert_eq!(reloaded.wires.len(), project.wires.len());
    assert_eq!(reloaded.wires[0].from, project.wires[0].from);
    assert_eq!(reloaded.wires[0].to, project.wires[0].to);
    assert_eq!(reloaded.comments.len(), project.comments.len());
    assert_eq!(reloaded.comments[0].text, project.comments[0].text);

    // Bounded UndoStack drops oldest entry cleanly without panicking when capacity exceeded.
    let mut bounded_doc = BoardDoc::with_max_undo_depth(reloaded.boards[0].clone(), 2);
    engine
        .set_eol_style(&mut bounded_doc, dendri_ast::EolStyle::Crlf, &ctx, &catalog)
        .unwrap();
    engine
        .set_eol_style(&mut bounded_doc, dendri_ast::EolStyle::Lf, &ctx, &catalog)
        .unwrap();
    engine
        .set_eol_style(&mut bounded_doc, dendri_ast::EolStyle::Crlf, &ctx, &catalog)
        .unwrap();
    assert_eq!(bounded_doc.undo_stack.undo_len(), 2);

    // Standalone UndoStack also works independently against a BoardDoc.
    let mut standalone = UndoStack::with_max_depth(2);
    let tx = bounded_doc.undo_stack.peek_undo().unwrap().clone();
    standalone.push(tx);
    assert!(standalone.can_undo());
    let u = standalone.undo(&mut bounded_doc).unwrap();
    assert_eq!(u.revision, 4);
    let r = standalone.redo(&mut bounded_doc).unwrap();
    assert_eq!(r.revision, 5);
}

// ---------------------------------------------------------------------------
// 8. Edge Cases: Stable POU NodeId Remap, Module POU Deletion Orphaning,
//    Nested Ladder ParallelOr Orphaning, Re-Anchoring Dangling Comments,
//    and LadderBoxKind::Foreign Cross-Language Export Guard
// ---------------------------------------------------------------------------

#[test]
fn edge_cases_node_id_remap_parallel_or_orphan_module_pou_delete_and_ladder_foreign_box() {
    let catalog = MemoryProfileCatalog::builtin_reference();
    let ctx = BoardContext::arduino_uno_r3("mcu_cell");
    let engine = SyncEngine::new();
    let mut board = make_rich_uno_board();

    // Add a Ladder ParallelOr cell with a commented nested contact in OB_MAIN
    if let PouBody::Network(net) = &mut board.program.pous[2].body {
        net.rungs[0].cells.push(LadderCell {
            id: NodeId::new("n_par_0"),
            row: 0,
            col: 2,
            kind: LadderCellKind::ParallelOr {
                branches: vec![vec![LadderCell {
                    id: NodeId::new("n_par_branch_cell"),
                    row: 1,
                    col: 2,
                    kind: LadderCellKind::Contact {
                        kind: LadderContactKind::NormallyOpen,
                        target: LValue::Var {
                            name: "motor_run".to_owned(),
                            span: Span::default(),
                        },
                    },
                }]],
            },
        });
    }
    board.comments.push(Comment {
        id: "c_par_cell".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_par_branch_cell"),
        },
        placement: AstCommentPlacement::Body,
        token_ordinal: None,
        text: " Seal-in parallel branch contact".to_owned(),
        origin: CommentOrigin {
            language: "ladder".to_owned(),
            style: CommentStyle::Visual,
        },
        span: Span::default(),
    });
    // Add comment inside module POU `ResetFault`
    board.comments.push(Comment {
        id: "c_mod_pou".to_owned(),
        anchor: CommentAnchor::Node {
            id: NodeId::new("n_reset_stmt"),
        },
        placement: AstCommentPlacement::Leading,
        token_ordinal: None,
        text: " Clears latched emergency fault".to_owned(),
        origin: CommentOrigin {
            language: "st".to_owned(),
            style: CommentStyle::ParenBlock,
        },
        span: Span::default(),
    });

    let mut doc = BoardDoc::new(board);

    // 1. Parser assigns a FRESH POU NodeId `"n_fresh_startup_id"` when re-parsing OB_STARTUP:
    //    SyncEngine preserves stable `"n_pou_startup"` so existing comments and presentation maps stay anchored!
    let fresh_id_cpp = MockFrontend::new(
        LanguageId::Cpp,
        Paradigm::Textual,
        LanguageCapabilities::cpp(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| {
            let startup = Pou {
                id: NodeId::new("n_fresh_startup_id"),
                name: "OB_STARTUP".to_owned(),
                kind: PouKind::Ob {
                    ob: ObKind::Startup,
                },
                preferred_view: None,
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![assign_bool_stmt("n_init_stmt", "motor_run", true)]),
                span: Span::default(),
            };
            let mut bundle = BoardProgramBundle::new(BoardProgram {
                pous: vec![startup],
                ..BoardProgram::default()
            });
            bundle.comments.push(Comment {
                id: "c_fresh_hdr".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_fresh_startup_id"),
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " Initialize board safely on cold boot".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::default(),
            });
            Ok(FrontendOutput::ok(bundle))
        },
    );
    engine
        .apply_view_edit(
            &mut doc,
            "cpp:OB_STARTUP",
            "setup()",
            &fresh_id_cpp,
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(doc.board.program.pous[0].id, NodeId::new("n_pou_startup"));
    assert_eq!(
        doc.board
            .presentation
            .origin_files
            .get(&NodeId::new("n_pou_startup")),
        Some(&"startup.ino".to_owned())
    );
    assert_eq!(
        doc.board
            .comments
            .iter()
            .filter(|c| c.text == " Initialize board safely on cold boot")
            .count(),
        1
    );

    // 2. Deleting nested Ladder ParallelOr branch cell orphans its comment onto surviving `n_par_0`!
    let ladder_rm_branch = MockFrontend::new(
        LanguageId::Ladder,
        Paradigm::Ladder,
        LanguageCapabilities::ladder(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| {
            let main = Pou {
                id: NodeId::new("n_pou_main"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: Some(AuthoringLanguage::Ladder),
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork {
                    rungs: vec![LadderRung {
                        id: NodeId::new("n_rung_0"),
                        order: 0,
                        cells: vec![
                            LadderCell {
                                id: NodeId::new("n_par_0"),
                                row: 0,
                                col: 2,
                                kind: LadderCellKind::ParallelOr { branches: vec![] },
                            },
                            LadderCell {
                                id: NodeId::new("n_foreign_box"),
                                row: 0,
                                col: 3,
                                kind: LadderCellKind::Box(LadderBoxKind::Foreign {
                                    language: "cpp".to_owned(),
                                    fidelity: ForeignFidelity::F0,
                                    text: "PORTB |= (1<<5);".to_owned(),
                                    pack_hint: None,
                                }),
                            },
                        ],
                    }],
                }),
                span: Span::default(),
            };
            Ok(FrontendOutput::ok(BoardProgramBundle::new(BoardProgram {
                pous: vec![main],
                ..BoardProgram::default()
            })))
        },
    );
    engine
        .apply_view_edit(
            &mut doc,
            "ladder:OB_MAIN",
            "rm branch",
            &ladder_rm_branch,
            &ctx,
            &catalog,
        )
        .unwrap();
    let par_comment = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_par_cell")
        .unwrap();
    assert_eq!(par_comment.placement, AstCommentPlacement::Dangling);
    assert_eq!(
        par_comment.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_par_0")
        }
    );

    // 3. Re-anchor the dangling comment cleanly onto `n_rung_0` as Body, and reject invalid Inner without ordinal!
    let bad_reanchor = engine
        .reanchor_comment(
            &mut doc,
            "c_par_cell",
            CommentAnchor::Node {
                id: NodeId::new("n_rung_0"),
            },
            AstCommentPlacement::Inner,
            None,
            &ctx,
            &catalog,
        )
        .unwrap_err();
    assert!(
        bad_reanchor
            .diagnostics()
            .iter()
            .any(|d| d.code == ErrorCode::ErrCommentField)
    );
    engine
        .reanchor_comment(
            &mut doc,
            "c_par_cell",
            CommentAnchor::Node {
                id: NodeId::new("n_rung_0"),
            },
            AstCommentPlacement::Body,
            None,
            &ctx,
            &catalog,
        )
        .unwrap();
    assert_eq!(
        doc.board
            .comments
            .iter()
            .find(|c| c.id == "c_par_cell")
            .unwrap()
            .placement,
        AstCommentPlacement::Body
    );

    // 4. Removing a module-owned POU (`ResetFault`) orphans its inner comment as `Dangling` on `n_mod_util`!
    let rm_mod_pou = engine
        .remove_pou(&mut doc, NodeId::new("n_pou_reset"), &ctx, &catalog)
        .unwrap();
    assert!(
        rm_mod_pou
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentOrphaned)
    );
    let mod_comment = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_mod_pou")
        .unwrap();
    assert_eq!(mod_comment.placement, AstCommentPlacement::Dangling);
    assert_eq!(
        mod_comment.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_mod_util")
        }
    );

    // 5. Recording an assumption targeting an unknown NodeId fails with ERR_COMMENT_ANCHOR!
    let bad_assumption = engine
        .record_assumption(
            &mut doc,
            Assumption {
                node: NodeId::new("n_does_not_exist"),
                expr_ordinal: None,
                policy: AssumptionPolicy::NoEffect,
                value: None,
                note: "Invalid target".to_owned(),
                span: Span::default(),
            },
            &ctx,
            &catalog,
        )
        .unwrap_err();
    assert!(
        bad_assumption
            .diagnostics()
            .iter()
            .any(|d| d.code == ErrorCode::ErrCommentAnchor)
    );

    // 6. `OB_MAIN` now contains a `LadderBoxKind::Foreign` (language = "cpp"):
    //    Exporting `OB_MAIN` to ST fails without allow_foreign, succeeds with allow_foreign = true!
    let st_fe = MockFrontend::new(
        LanguageId::St,
        Paradigm::Textual,
        LanguageCapabilities::st(
            StCommentDialect::Iec61131Ed3,
            CommentEncoding::Utf8Passthrough,
        ),
        |_src, _ctx| Ok(FrontendOutput::ok(BoardProgramBundle::default())),
    );
    let blocked = engine
        .export_view(&doc, ExportScope::Pou("OB_MAIN"), &st_fe, &ctx, false)
        .unwrap_err();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].code, ErrorCode::ErrForeignExportBlocked);
    assert!(is_four_part_diagnostic(&blocked[0]));

    let allowed = engine
        .export_view(&doc, ExportScope::Pou("OB_MAIN"), &st_fe, &ctx, true)
        .unwrap();
    assert!(allowed.value.contains("NOT TRANSLATED"));
    assert!(allowed.value.contains("PORTB |= (1<<5);"));
}

// ---------------------------------------------------------------------------
// 9. Real-Frontend Regenerated Child NodeIds & Comment IDs + Named Child
//    Stability (`locals`, `SfcStep`, `SfcTransition`) + `UpsertDataDecl` ID Change
// ---------------------------------------------------------------------------

#[test]
fn regenerated_child_node_ids_and_comment_ids_rebind_cleanly_preserve_coords_and_orphan_only_deleted()
 {
    let catalog = MemoryProfileCatalog::builtin_reference();
    let mut ctx = BoardContext::arduino_uno_r3("mcu_cell");
    ctx.comment_encoding = CommentEncoding::AsciiFold;
    ctx.st_dialect = StCommentDialect::StrictEd2;
    let engine = SyncEngine::new();

    let mut board = make_rich_uno_board();

    let verbatim_c4 = " Motor ação \u{202E}24V ±5%";
    let verbatim_inner = " gain /* factor */";
    let verbatim_deleted = " genuinely deleted statement note";

    // Add a POU with a named local variable + 3 statements (2 kept with regenerated IDs, 1 deleted).
    board.program.pous.push(Pou {
        id: NodeId::new("n_pou_regen"),
        name: "RegenSt".to_owned(),
        kind: PouKind::Fc {
            return_type: TypeRef::Void,
        },
        preferred_view: Some(AuthoringLanguage::St),
        params: vec![],
        locals: vec![DataDecl {
            id: NodeId::new("n_local_tmp"),
            name: "tmp_flag".to_owned(),
            scope: StorageScope::Temp,
            ty: TypeRef::scalar(ScalarType::Bool),
            initial: Some(Expr::new(
                ExprKind::Literal(Literal::Bool(false)),
                Span::default(),
            )),
            span: Span::default(),
        }],
        body: PouBody::Statements(vec![
            assign_bool_stmt("n_old_s1", "tmp_flag", true),
            assign_bool_stmt("n_old_s2", "motor_run", true),
            assign_bool_stmt("n_old_s3_del", "fault_latched", false),
        ]),
        span: Span::default(),
    });
    board.comments.extend([
        Comment {
            id: "c_local_orig".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_local_tmp"),
            },
            placement: AstCommentPlacement::Leading,
            token_ordinal: None,
            text: " Scratch flag local".to_owned(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: CommentStyle::ParenBlock,
            },
            span: Span::default(),
        },
        Comment {
            id: "c_s1_degraded".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_old_s1"),
            },
            placement: AstCommentPlacement::Leading,
            token_ordinal: None,
            text: verbatim_c4.to_owned(),
            origin: CommentOrigin {
                language: "cpp".to_owned(),
                style: CommentStyle::Line,
            },
            span: Span::default(),
        },
        Comment {
            id: "c_s2_inner".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_old_s2"),
            },
            placement: AstCommentPlacement::Inner,
            token_ordinal: Some(3),
            text: verbatim_inner.to_owned(),
            origin: CommentOrigin {
                language: "cpp".to_owned(),
                style: CommentStyle::Block,
            },
            span: Span::default(),
        },
        Comment {
            id: "c_s3_deleted".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_old_s3_del"),
            },
            placement: AstCommentPlacement::Leading,
            token_ordinal: None,
            text: verbatim_deleted.to_owned(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: CommentStyle::ParenBlock,
            },
            span: Span::default(),
        },
        Comment {
            id: "c_data_run".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_d_run"),
            },
            placement: AstCommentPlacement::Body,
            token_ordinal: None,
            text: " Main motor run coil state".to_owned(),
            origin: CommentOrigin {
                language: "asl".to_owned(),
                style: CommentStyle::Visual,
            },
            span: Span::default(),
        },
    ]);

    let initial_multiset: BTreeMap<String, usize> = comment_multiset(&board.comments)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    let initial_comment_count = board.comments.len();
    let mut doc = BoardDoc::new(board);

    let degraded_s1 = render_comment_for_language(
        LanguageId::St,
        verbatim_c4,
        CommentStyle::Line,
        CommentPlacement::Leading,
        &ctx,
    );
    assert!(degraded_s1.degraded);
    let degraded_s2 = render_comment_for_language(
        LanguageId::St,
        verbatim_inner,
        CommentStyle::Block,
        CommentPlacement::Leading,
        &ctx,
    );

    let degraded_s1_str = degraded_s1.rendered;
    let degraded_s2_str = degraded_s2.rendered;

    // 1. Mock ST frontend synthesizes brand-new POU/local/statement NodeIds AND brand-new comment IDs,
    //    while keeping stmt 1 + stmt 2 and genuinely deleting stmt 3!
    let st_regen = MockFrontend::new(
        LanguageId::St,
        Paradigm::Textual,
        LanguageCapabilities::st(StCommentDialect::StrictEd2, CommentEncoding::AsciiFold),
        move |_src, _ctx| {
            let pou = Pou {
                id: NodeId::new("n_st_synth_pou"),
                name: "RegenSt".to_owned(),
                kind: PouKind::Fc {
                    return_type: TypeRef::Void,
                },
                preferred_view: None,
                params: vec![],
                locals: vec![DataDecl {
                    id: NodeId::new("n_st_synth_local"),
                    name: "tmp_flag".to_owned(),
                    scope: StorageScope::Temp,
                    ty: TypeRef::scalar(ScalarType::Bool),
                    initial: Some(Expr::new(
                        ExprKind::Literal(Literal::Bool(false)),
                        Span::default(),
                    )),
                    span: Span::default(),
                }],
                body: PouBody::Statements(vec![
                    assign_bool_stmt("n_st_synth_s1", "tmp_flag", true),
                    assign_bool_stmt("n_st_synth_s2", "motor_run", true),
                ]),
                span: Span::default(),
            };
            let mut bundle = BoardProgramBundle::new(BoardProgram {
                pous: vec![pou],
                ..BoardProgram::default()
            });
            bundle.comments = vec![
                Comment {
                    id: "c_synth_1".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_st_synth_local"),
                    },
                    placement: AstCommentPlacement::Leading,
                    token_ordinal: None,
                    text: " Scratch flag local".to_owned(),
                    origin: CommentOrigin {
                        language: "st".to_owned(),
                        style: CommentStyle::ParenBlock,
                    },
                    span: Span::default(),
                },
                Comment {
                    id: "c_synth_2".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_st_synth_s1"),
                    },
                    placement: AstCommentPlacement::Leading,
                    token_ordinal: None,
                    text: degraded_s1_str.clone(),
                    origin: CommentOrigin {
                        language: "st".to_owned(),
                        style: CommentStyle::ParenBlock,
                    },
                    span: Span::default(),
                },
                // Relocated Inner comment parsed back as Leading on regenerated statement NodeId!
                Comment {
                    id: "c_synth_3".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_st_synth_s2"),
                    },
                    placement: AstCommentPlacement::Leading,
                    token_ordinal: None,
                    text: degraded_s2_str.clone(),
                    origin: CommentOrigin {
                        language: "st".to_owned(),
                        style: CommentStyle::ParenBlock,
                    },
                    span: Span::default(),
                },
            ];
            Ok(FrontendOutput::ok(bundle))
        },
    );

    let r1 = engine
        .apply_view_edit(
            &mut doc,
            "st:RegenSt",
            "regenerated",
            &st_regen,
            &ctx,
            &catalog,
        )
        .unwrap();

    // Exact 0 duplicate comments and exact C1 multiset conservation!
    assert_eq!(doc.board.comments.len(), initial_comment_count);
    let after_ms: BTreeMap<String, usize> = comment_multiset(&doc.board.comments)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    assert_eq!(after_ms, initial_multiset);

    // Exact 1 INFO_COMMENT_ORPHANED diagnostic (for genuinely deleted stmt 3 ONLY — 0 false orphans!).
    let orphan_diags: Vec<&Diagnostic> = r1
        .diagnostics
        .iter()
        .filter(|d| d.code == ErrorCode::InfoCommentOrphaned)
        .collect();
    assert_eq!(
        orphan_diags.len(),
        1,
        "expected exactly 1 orphan diag, got: {orphan_diags:?}"
    );
    assert!(orphan_diags[0].message.contains("c_s3_deleted"));

    // Named local variable `tmp_flag` preserved its stable NodeId `n_local_tmp`!
    let regen_pou = doc
        .board
        .program
        .pous
        .iter()
        .find(|p| p.name == "RegenSt")
        .unwrap();
    assert_eq!(regen_pou.id, NodeId::new("n_pou_regen"));
    assert_eq!(regen_pou.locals[0].id, NodeId::new("n_local_tmp"));
    let c_local = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_local_orig")
        .unwrap();
    assert_eq!(
        c_local.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_local_tmp")
        }
    );
    assert_eq!(c_local.placement, AstCommentPlacement::Leading);

    // Stmt 1 C4 degraded comment re-bound cleanly onto `n_st_synth_s1` keeping verbatim original!
    let c_s1 = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_s1_degraded")
        .unwrap();
    assert_eq!(
        c_s1.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_st_synth_s1")
        }
    );
    assert_eq!(c_s1.placement, AstCommentPlacement::Leading);
    assert_eq!(c_s1.text, verbatim_c4);
    assert_eq!(c_s1.origin.language, "cpp");
    assert_eq!(c_s1.origin.style, CommentStyle::Line);

    // Stmt 2 relocated Inner comment re-bound onto `n_st_synth_s2` keeping Inner + token_ordinal(3)!
    let c_s2 = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_s2_inner")
        .unwrap();
    assert_eq!(
        c_s2.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_st_synth_s2")
        }
    );
    assert_eq!(c_s2.placement, AstCommentPlacement::Inner);
    assert_eq!(c_s2.token_ordinal, Some(3));
    assert_eq!(c_s2.text, verbatim_inner);

    // Genuinely deleted stmt 3 comment transitioned to Dangling on `n_pou_regen`!
    let c_s3 = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_s3_deleted")
        .unwrap();
    assert_eq!(
        c_s3.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_pou_regen")
        }
    );
    assert_eq!(c_s3.placement, AstCommentPlacement::Dangling);
    assert_eq!(c_s3.text, verbatim_deleted);

    // 2. Mock SFC frontend regenerates fresh SfcStep, SfcTransition, SfcAction NodeIds AND fresh comment IDs:
    //    Named steps (`S0`, `S1`) and transitions (`T0`) MUST retain stable NodeIds so `node_coords` stay attached!
    let sfc_regen = MockFrontend::new(
        LanguageId::Sfc,
        Paradigm::Grafcet,
        LanguageCapabilities::sfc(CommentEncoding::Utf8Passthrough),
        |_src, _ctx| {
            let chart_pou = Pou {
                id: NodeId::new("n_sfc_synth_pou"),
                name: "CycleChart".to_owned(),
                kind: PouKind::Fb,
                preferred_view: None,
                params: vec![],
                locals: vec![],
                body: PouBody::Chart(SfcChart {
                    steps: vec![
                        SfcStep {
                            id: NodeId::new("n_sfc_synth_s0"),
                            name: "S0".to_owned(),
                            initial: true,
                            actions: vec![SfcAction {
                                id: NodeId::new("n_sfc_synth_act0"),
                                qualifier: ActionQualifier::N,
                                duration: None,
                                target: Some(LValue::Var {
                                    name: "motor_run".to_owned(),
                                    span: Span::default(),
                                }),
                                body: vec![],
                            }],
                            macro_chart: None,
                        },
                        SfcStep {
                            id: NodeId::new("n_sfc_synth_s1"),
                            name: "S1".to_owned(),
                            initial: false,
                            actions: vec![SfcAction {
                                id: NodeId::new("n_sfc_synth_act1"),
                                qualifier: ActionQualifier::S,
                                duration: None,
                                target: Some(LValue::Var {
                                    name: "fault_latched".to_owned(),
                                    span: Span::default(),
                                }),
                                body: vec![],
                            }],
                            macro_chart: None,
                        },
                    ],
                    transitions: vec![SfcTransition {
                        id: NodeId::new("n_sfc_synth_t0"),
                        name: "T0".to_owned(),
                        from_steps: vec!["S0".to_owned()],
                        to_steps: vec!["S1".to_owned()],
                        priority: 0,
                        receptivity: Expr::new(
                            ExprKind::Ident("START_BTN".to_owned()),
                            Span::default(),
                        ),
                    }],
                    evolve_to_stability: true,
                }),
                span: Span::default(),
            };
            let mut bundle = BoardProgramBundle::new(BoardProgram {
                pous: vec![chart_pou],
                ..BoardProgram::default()
            });
            bundle.comments = vec![
                Comment {
                    id: "c_sfc_synth_1".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_sfc_synth_s1"),
                    },
                    placement: AstCommentPlacement::Body,
                    token_ordinal: None,
                    text: " Fault state in GRAFCET".to_owned(),
                    origin: CommentOrigin {
                        language: "sfc".to_owned(),
                        style: CommentStyle::Visual,
                    },
                    span: Span::default(),
                },
                Comment {
                    id: "c_sfc_synth_2".to_owned(),
                    anchor: CommentAnchor::Node {
                        id: NodeId::new("n_sfc_synth_act1"),
                    },
                    placement: AstCommentPlacement::Body,
                    token_ordinal: None,
                    text: " Latch fault output".to_owned(),
                    origin: CommentOrigin {
                        language: "sfc".to_owned(),
                        style: CommentStyle::Visual,
                    },
                    span: Span::default(),
                },
            ];
            Ok(FrontendOutput::ok(bundle))
        },
    );

    let r2 = engine
        .apply_view_edit(
            &mut doc,
            "sfc:CycleChart",
            "regen chart",
            &sfc_regen,
            &ctx,
            &catalog,
        )
        .unwrap();

    // Zero new orphans (only the pre-existing `c_s3_deleted` dangling info from validator).
    assert!(
        !r2.diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentOrphaned && d.message.contains("c_step_1"))
    );
    assert!(
        !r2.diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentOrphaned && d.message.contains("c_act_1"))
    );
    // Zero comment duplication!
    assert_eq!(doc.board.comments.len(), initial_comment_count);

    // S0, S1, and T0 retained their exact stable NodeIds (`n_step_0`, `n_step_1`, `n_trans_0`),
    // keeping `[boards.presentation].node_coords` attached!
    let sfc_pou = doc
        .board
        .program
        .pous
        .iter()
        .find(|p| p.name == "CycleChart")
        .unwrap();
    if let PouBody::Chart(chart) = &sfc_pou.body {
        assert_eq!(chart.steps[0].id, NodeId::new("n_step_0"));
        assert_eq!(chart.steps[1].id, NodeId::new("n_step_1"));
        assert_eq!(chart.transitions[0].id, NodeId::new("n_trans_0"));
        assert_eq!(
            doc.board.presentation.node_coords.get(&chart.steps[0].id),
            Some(&[120.0, 48.0])
        );
        assert_eq!(
            doc.board.presentation.node_coords.get(&chart.steps[1].id),
            Some(&[120.0, 160.0])
        );
    } else {
        panic!("expected SfcChart body");
    }
    // Action comment re-bound onto regenerated `n_sfc_synth_act1` cleanly without orphaning!
    let c_act = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_act_1")
        .unwrap();
    assert_eq!(
        c_act.anchor,
        CommentAnchor::Node {
            id: NodeId::new("n_sfc_synth_act1")
        }
    );
    assert_eq!(c_act.placement, AstCommentPlacement::Body);

    // 3. `SyncCommand::UpsertDataDecl` matching by `name` with a changed `id` orphans comments on old ID
    //    cleanly instead of failing with ERR_COMMENT_ANCHOR!
    let r3 = engine
        .upsert_data_decl(
            &mut doc,
            bool_data("n_d_run_v2", "motor_run", true),
            &ctx,
            &catalog,
        )
        .unwrap();
    assert!(
        r3.diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentOrphaned && d.message.contains("c_data_run"))
    );
    let c_data = doc
        .board
        .comments
        .iter()
        .find(|c| c.id == "c_data_run")
        .unwrap();
    assert_eq!(c_data.text, " Main motor run coil state");
    assert_eq!(
        c_data.anchor,
        CommentAnchor::Board {
            id: "mcu_cell".to_owned()
        }
    );
}
