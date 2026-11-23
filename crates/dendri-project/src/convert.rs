//! Conversion between strict TOML wire DTOs and canonical typed `dendri-ast` IR.

use crate::ProjectError;
use crate::wire::{
    OutBoard, OutBoardProgram, OutComponentInstance, OutDataDecl, OutElifArm, OutLadderCell,
    OutLadderNetwork, OutLadderRung, OutModuleDecl, OutPou, OutProject, OutSfcAction, OutSfcChart,
    OutSfcStep, OutSfcTransition, OutStmt, OutSwitchCase, OutWire, RawBoard, RawBoardProgram,
    RawCallArg, RawCasePattern, RawDataDecl, RawLadderCell, RawLadderNetwork, RawLadderRung,
    RawModuleDecl, RawNamedExpr, RawPou, RawPouBodyKind, RawProject, RawSfcAction, RawSfcChart,
    RawSfcStep, RawSfcTransition, RawStmt,
};
use dendri_ast::{
    Board, BoardProgram, CallArg, CasePattern, ComponentInstance, DataDecl, ElifArm, Expr, LValue,
    LadderBoxKind, LadderCell, LadderCellKind, LadderNetwork, LadderRung, ModuleDecl, Pou, PouBody,
    Project, SfcAction, SfcChart, SfcStep, SfcTransition, Stmt, StmtKind, SwitchCase, TypeDecl,
    Wire,
};
use dendri_expr::{parse_expr, parse_lvalue, print_expr, print_lvalue};
use dendri_span::{Diagnostic, ErrorCode, FileId, Span};
use toml::Spanned;

fn to_span(range: std::ops::Range<usize>) -> Span {
    let start = u32::try_from(range.start).unwrap_or(u32::MAX);
    let end = u32::try_from(range.end).unwrap_or(u32::MAX);
    Span::new(start, end)
}

fn diag_err(
    code: ErrorCode,
    message: impl Into<String>,
    file_id: Option<FileId>,
    span: Span,
) -> ProjectError {
    let mut diag = Diagnostic::new(code, message);
    if let Some(fid) = file_id {
        diag = diag.with_location(fid, span);
    } else {
        diag.span = Some(span);
    }
    ProjectError::Diagnostic(Box::new(diag))
}

fn parse_spanned_expr(
    spanned: &Spanned<String>,
    file_id: Option<FileId>,
) -> Result<Expr, ProjectError> {
    let span = to_span(spanned.span());
    parse_expr(spanned.get_ref())
        .map(|mut e| {
            e.span = span;
            e
        })
        .map_err(|err| {
            diag_err(
                ErrorCode::ErrTomlSyntax,
                format!("invalid ASL expression `{}`: {err}", spanned.get_ref()),
                file_id,
                span,
            )
        })
}

fn parse_plain_expr(src: &str, file_id: Option<FileId>, span: Span) -> Result<Expr, ProjectError> {
    parse_expr(src)
        .map(|mut e| {
            e.span = span;
            e
        })
        .map_err(|err| {
            diag_err(
                ErrorCode::ErrTomlSyntax,
                format!("invalid ASL expression `{src}`: {err}"),
                file_id,
                span,
            )
        })
}

fn parse_plain_lvalue(
    src: &str,
    file_id: Option<FileId>,
    span: Span,
) -> Result<LValue, ProjectError> {
    parse_lvalue(src).map_err(|err| {
        diag_err(
            ErrorCode::ErrTomlSyntax,
            format!("invalid ASL LValue `{src}`: {err}"),
            file_id,
            span,
        )
    })
}

fn parse_spanned_lvalue(
    spanned: &Spanned<String>,
    file_id: Option<FileId>,
) -> Result<LValue, ProjectError> {
    let span = to_span(spanned.span());
    parse_plain_lvalue(spanned.get_ref(), file_id, span)
}

pub(crate) fn raw_to_project(
    raw: RawProject,
    file_id: Option<FileId>,
) -> Result<Project, ProjectError> {
    let meta_span = to_span(raw.project.span());
    let meta = raw.project.into_inner();
    if meta.schema_version != "2.0" {
        return Err(diag_err(
            ErrorCode::ErrUnsupportedVersion,
            format!(
                "unsupported project schema_version `{}` (expected \"2.0\")",
                meta.schema_version
            ),
            file_id,
            meta_span,
        ));
    }

    let comments = raw
        .comments
        .into_iter()
        .map(|sc| {
            let span = to_span(sc.span());
            let mut c = sc.into_inner();
            c.span = span;
            c
        })
        .collect();

    let mut boards = Vec::with_capacity(raw.boards.len());
    for sb in raw.boards {
        let span = to_span(sb.span());
        boards.push(raw_to_board(sb.into_inner(), span, file_id)?);
    }

    let components = raw
        .components
        .into_iter()
        .map(|sc| {
            let span = to_span(sc.span());
            let c = sc.into_inner();
            ComponentInstance {
                id: c.id,
                profile: c.profile,
                label: c.label,
                layout: c.layout,
                params: c.params,
                span,
            }
        })
        .collect();

    let wires = raw
        .wires
        .into_iter()
        .map(|sw| {
            let span = to_span(sw.span());
            let w = sw.into_inner();
            Wire {
                id: w.id,
                from: w.from,
                to: w.to,
                signal: w.signal,
                waypoints: w.waypoints,
                span,
            }
        })
        .collect();

    Ok(Project {
        meta,
        libraries: raw.libraries,
        packs: raw.packs,
        comments,
        boards,
        components,
        wires,
    })
}

fn raw_to_board(raw: RawBoard, span: Span, file_id: Option<FileId>) -> Result<Board, ProjectError> {
    let program = raw_to_program(raw.program, file_id)?;
    let comments = raw
        .comments
        .into_iter()
        .map(|sc| {
            let sp = to_span(sc.span());
            let mut c = sc.into_inner();
            c.span = sp;
            c
        })
        .collect();
    let assumptions = raw
        .assumptions
        .into_iter()
        .map(|sa| {
            let sp = to_span(sa.span());
            let mut a = sa.into_inner();
            a.span = sp;
            a
        })
        .collect();

    Ok(Board {
        id: raw.id,
        profile: raw.profile,
        authoring_language: raw.authoring_language,
        execution: raw.execution,
        scan_period_us: raw.scan_period_us,
        layout: raw.layout,
        program,
        comments,
        assumptions,
        presentation: raw.presentation,
        span,
    })
}

fn raw_to_program(
    raw: RawBoardProgram,
    file_id: Option<FileId>,
) -> Result<BoardProgram, ProjectError> {
    let hardware = raw
        .hardware
        .into_iter()
        .map(|sh| {
            let sp = to_span(sh.span());
            let mut h = sh.into_inner();
            h.span = sp;
            h
        })
        .collect();
    let types = raw.types.into_iter().map(set_type_decl_span).collect();
    let data = convert_data_decls(raw.data, file_id)?;
    let mut modules = Vec::with_capacity(raw.modules.len());
    for sm in raw.modules {
        let sp = to_span(sm.span());
        modules.push(raw_to_module(sm.into_inner(), sp, file_id)?);
    }
    let mut pous = Vec::with_capacity(raw.pous.len());
    for sp_pou in raw.pous {
        let sp = to_span(sp_pou.span());
        pous.push(raw_to_pou(sp_pou.into_inner(), sp, file_id)?);
    }
    Ok(BoardProgram {
        imports: raw.imports,
        hardware,
        types,
        data,
        modules,
        pous,
    })
}

fn set_type_decl_span(st: Spanned<TypeDecl>) -> TypeDecl {
    let sp = to_span(st.span());
    match st.into_inner() {
        TypeDecl::Struct {
            id, name, fields, ..
        } => TypeDecl::Struct {
            id,
            name,
            fields,
            span: sp,
        },
        TypeDecl::Enum {
            id,
            name,
            backing,
            variants,
            ..
        } => TypeDecl::Enum {
            id,
            name,
            backing,
            variants,
            span: sp,
        },
        TypeDecl::Foreign {
            id,
            name,
            language,
            fidelity,
            text,
            pack_hint,
            ..
        } => TypeDecl::Foreign {
            id,
            name,
            language,
            fidelity,
            text,
            pack_hint,
            span: sp,
        },
    }
}

fn convert_data_decls(
    raw: Vec<Spanned<RawDataDecl>>,
    file_id: Option<FileId>,
) -> Result<Vec<DataDecl>, ProjectError> {
    let mut out = Vec::with_capacity(raw.len());
    for sd in raw {
        let sp = to_span(sd.span());
        let d = sd.into_inner();
        let initial = match d.initial {
            Some(ref expr_s) => Some(parse_spanned_expr(expr_s, file_id)?),
            None => None,
        };
        out.push(DataDecl {
            id: d.id,
            name: d.name,
            scope: d.scope,
            ty: d.ty,
            initial,
            span: sp,
        });
    }
    Ok(out)
}

fn raw_to_module(
    raw: RawModuleDecl,
    span: Span,
    file_id: Option<FileId>,
) -> Result<ModuleDecl, ProjectError> {
    let types = raw.types.into_iter().map(set_type_decl_span).collect();
    let data = convert_data_decls(raw.data, file_id)?;
    let mut pous = Vec::with_capacity(raw.pous.len());
    for sp_pou in raw.pous {
        let sp = to_span(sp_pou.span());
        pous.push(raw_to_pou(sp_pou.into_inner(), sp, file_id)?);
    }
    Ok(ModuleDecl {
        id: raw.id,
        name: raw.name,
        imports: raw.imports,
        exports: raw.exports,
        types,
        data,
        pous,
        span,
    })
}

fn raw_to_pou(raw: RawPou, span: Span, file_id: Option<FileId>) -> Result<Pou, ProjectError> {
    let locals = convert_data_decls(raw.locals, file_id)?;
    let body = match raw.body_kind {
        RawPouBodyKind::Statements => {
            if raw.network.is_some() || raw.chart.is_some() {
                return Err(diag_err(
                    ErrorCode::ErrTomlSyntax,
                    "POU with body_kind=\"statements\" must not define `network` or `chart`",
                    file_id,
                    span,
                ));
            }
            PouBody::Statements(convert_stmts(raw.body, file_id)?)
        }
        RawPouBodyKind::Network => {
            if !raw.body.is_empty() || raw.chart.is_some() {
                return Err(diag_err(
                    ErrorCode::ErrTomlSyntax,
                    "POU with body_kind=\"network\" must only define `network`",
                    file_id,
                    span,
                ));
            }
            let net = raw.network.unwrap_or_default();
            PouBody::Network(raw_to_ladder(net, file_id, span)?)
        }
        RawPouBodyKind::Chart => {
            if !raw.body.is_empty() || raw.network.is_some() {
                return Err(diag_err(
                    ErrorCode::ErrTomlSyntax,
                    "POU with body_kind=\"chart\" must only define `chart`",
                    file_id,
                    span,
                ));
            }
            let chart = raw.chart.unwrap_or_default();
            PouBody::Chart(raw_to_chart(chart, file_id)?)
        }
    };
    Ok(Pou {
        id: raw.id,
        name: raw.name,
        kind: raw.kind,
        preferred_view: raw.preferred_view,
        params: raw.params,
        locals,
        body,
        span,
    })
}

fn convert_stmts(
    raw: Vec<Spanned<RawStmt>>,
    file_id: Option<FileId>,
) -> Result<Vec<Stmt>, ProjectError> {
    let mut out = Vec::with_capacity(raw.len());
    for ss in raw {
        let sp = to_span(ss.span());
        out.push(raw_to_stmt(ss.into_inner(), sp, file_id)?);
    }
    Ok(out)
}

fn convert_nested_stmts(
    raw: Vec<RawStmt>,
    parent_span: Span,
    file_id: Option<FileId>,
) -> Result<Vec<Stmt>, ProjectError> {
    let mut out = Vec::with_capacity(raw.len());
    for s in raw {
        out.push(raw_to_stmt(s, parent_span, file_id)?);
    }
    Ok(out)
}

fn raw_to_stmt(raw: RawStmt, span: Span, file_id: Option<FileId>) -> Result<Stmt, ProjectError> {
    match raw {
        RawStmt::Assign { id, target, value } => Ok(Stmt::new(
            id,
            StmtKind::Assign {
                target: parse_plain_lvalue(&target, file_id, span)?,
                value: parse_plain_expr(&value, file_id, span)?,
            },
            span,
        )),
        RawStmt::Call { id, callee, args } => {
            let mut parsed_args = Vec::with_capacity(args.len());
            for arg in args {
                let value = parse_plain_expr(&arg.value, file_id, span)?;
                parsed_args.push(CallArg {
                    name: arg.name,
                    value,
                    span,
                });
            }
            Ok(Stmt::new(
                id,
                StmtKind::Call {
                    callee,
                    args: parsed_args,
                },
                span,
            ))
        }
        RawStmt::FbInvoke {
            id,
            instance,
            inputs,
        } => {
            let mut parsed_inputs = Vec::with_capacity(inputs.len());
            for inp in inputs {
                parsed_inputs.push((inp.name, parse_plain_expr(&inp.value, file_id, span)?));
            }
            Ok(Stmt::new(
                id,
                StmtKind::FbInvoke {
                    instance: parse_plain_lvalue(&instance, file_id, span)?,
                    inputs: parsed_inputs,
                },
                span,
            ))
        }
        RawStmt::If {
            id,
            condition,
            then_body,
            elif_arms,
            else_body,
        } => {
            let mut arms = Vec::with_capacity(elif_arms.len());
            for arm in elif_arms {
                arms.push(ElifArm {
                    condition: parse_plain_expr(&arm.condition, file_id, span)?,
                    body: convert_nested_stmts(arm.body, span, file_id)?,
                });
            }
            Ok(Stmt::new(
                id,
                StmtKind::If {
                    condition: parse_plain_expr(&condition, file_id, span)?,
                    then_body: convert_nested_stmts(then_body, span, file_id)?,
                    elif_arms: arms,
                    else_body: convert_nested_stmts(else_body, span, file_id)?,
                },
                span,
            ))
        }
        RawStmt::Switch {
            id,
            scrutinee,
            cases,
            default,
        } => {
            let mut parsed_cases = Vec::with_capacity(cases.len());
            for c in cases {
                let mut pats = Vec::with_capacity(c.patterns.len());
                for p in c.patterns {
                    match p {
                        RawCasePattern::Value { expr } => {
                            pats.push(CasePattern::Value(parse_plain_expr(&expr, file_id, span)?));
                        }
                        RawCasePattern::Range { start, end } => {
                            pats.push(CasePattern::Range {
                                start: parse_plain_expr(&start, file_id, span)?,
                                end: parse_plain_expr(&end, file_id, span)?,
                            });
                        }
                    }
                }
                parsed_cases.push(SwitchCase {
                    patterns: pats,
                    body: convert_nested_stmts(c.body, span, file_id)?,
                });
            }
            let parsed_default = match default {
                Some(stmts) => Some(convert_nested_stmts(stmts, span, file_id)?),
                None => None,
            };
            Ok(Stmt::new(
                id,
                StmtKind::Switch {
                    scrutinee: parse_plain_expr(&scrutinee, file_id, span)?,
                    cases: parsed_cases,
                    default: parsed_default,
                },
                span,
            ))
        }
        RawStmt::While {
            id,
            condition,
            body,
        } => Ok(Stmt::new(
            id,
            StmtKind::While {
                condition: parse_plain_expr(&condition, file_id, span)?,
                body: convert_nested_stmts(body, span, file_id)?,
            },
            span,
        )),
        RawStmt::For {
            id,
            counter,
            from,
            to,
            step,
            inclusive,
            body,
        } => Ok(Stmt::new(
            id,
            StmtKind::For {
                counter,
                from: parse_plain_expr(&from, file_id, span)?,
                to: parse_plain_expr(&to, file_id, span)?,
                step: parse_plain_expr(&step, file_id, span)?,
                inclusive,
                body: convert_nested_stmts(body, span, file_id)?,
            },
            span,
        )),
        RawStmt::Break { id } => Ok(Stmt::new(id, StmtKind::Break, span)),
        RawStmt::Continue { id } => Ok(Stmt::new(id, StmtKind::Continue, span)),
        RawStmt::Return { id, value } => {
            let expr = match value {
                Some(ref v) => Some(parse_plain_expr(v, file_id, span)?),
                None => None,
            };
            Ok(Stmt::new(id, StmtKind::Return { value: expr }, span))
        }
        RawStmt::Wait { id, duration } => Ok(Stmt::new(
            id,
            StmtKind::Wait {
                duration: parse_plain_expr(&duration, file_id, span)?,
            },
            span,
        )),
        RawStmt::Foreign {
            id,
            language,
            fidelity,
            text,
            pack_hint,
        } => Ok(Stmt::new(
            id,
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            },
            span,
        )),
    }
}

fn raw_to_ladder(
    raw: RawLadderNetwork,
    file_id: Option<FileId>,
    _span: Span,
) -> Result<LadderNetwork, ProjectError> {
    let mut rungs = Vec::with_capacity(raw.rungs.len());
    for rung in raw.rungs {
        rungs.push(raw_to_rung(rung, file_id)?);
    }
    Ok(LadderNetwork { rungs })
}

fn raw_to_rung(raw: RawLadderRung, file_id: Option<FileId>) -> Result<LadderRung, ProjectError> {
    let mut cells = Vec::with_capacity(raw.cells.len());
    for sc in raw.cells {
        let sp = to_span(sc.span());
        cells.push(raw_to_cell(sc.into_inner(), file_id, sp)?);
    }
    Ok(LadderRung {
        id: raw.id,
        order: raw.order,
        cells,
    })
}

fn raw_to_cell(
    raw: RawLadderCell,
    file_id: Option<FileId>,
    span: Span,
) -> Result<LadderCell, ProjectError> {
    match raw {
        RawLadderCell::Empty { id, row, col } => Ok(LadderCell {
            id,
            row,
            col,
            kind: LadderCellKind::Empty,
        }),
        RawLadderCell::Wire { id, row, col } => Ok(LadderCell {
            id,
            row,
            col,
            kind: LadderCellKind::Wire,
        }),
        RawLadderCell::Contact {
            id,
            row,
            col,
            contact_kind,
            target,
        } => Ok(LadderCell {
            id,
            row,
            col,
            kind: LadderCellKind::Contact {
                kind: contact_kind,
                target: parse_plain_lvalue(&target, file_id, span)?,
            },
        }),
        RawLadderCell::Coil {
            id,
            row,
            col,
            coil_kind,
            target,
        } => Ok(LadderCell {
            id,
            row,
            col,
            kind: LadderCellKind::Coil {
                kind: coil_kind,
                target: parse_plain_lvalue(&target, file_id, span)?,
            },
        }),
        RawLadderCell::ParallelOr {
            id,
            row,
            col,
            branches,
        } => {
            let mut out_branches = Vec::with_capacity(branches.len());
            for br in branches {
                let mut cells = Vec::with_capacity(br.len());
                for c in br {
                    cells.push(raw_to_cell(c, file_id, span)?);
                }
                out_branches.push(cells);
            }
            Ok(LadderCell {
                id,
                row,
                col,
                kind: LadderCellKind::ParallelOr {
                    branches: out_branches,
                },
            })
        }
        RawLadderCell::FbBox {
            id,
            row,
            col,
            fb_kind,
            instance,
            inputs,
        } => {
            let mut parsed = Vec::with_capacity(inputs.len());
            for inp in inputs {
                parsed.push((inp.name, parse_plain_expr(&inp.value, file_id, span)?));
            }
            Ok(LadderCell {
                id,
                row,
                col,
                kind: LadderCellKind::Box(LadderBoxKind::Fb {
                    kind: fb_kind,
                    instance: parse_plain_lvalue(&instance, file_id, span)?,
                    inputs: parsed,
                }),
            })
        }
        RawLadderCell::CompareBox {
            id,
            row,
            col,
            op,
            left,
            right,
        } => Ok(LadderCell {
            id,
            row,
            col,
            kind: LadderCellKind::Box(LadderBoxKind::Compare {
                op,
                left: parse_plain_expr(&left, file_id, span)?,
                right: parse_plain_expr(&right, file_id, span)?,
            }),
        }),
        RawLadderCell::CallBox {
            id,
            row,
            col,
            callee,
            args,
            output,
        } => {
            let mut parsed = Vec::with_capacity(args.len());
            for arg in args {
                parsed.push((arg.name, parse_plain_expr(&arg.value, file_id, span)?));
            }
            let out_lv = match output {
                Some(ref lv) => Some(parse_plain_lvalue(lv, file_id, span)?),
                None => None,
            };
            Ok(LadderCell {
                id,
                row,
                col,
                kind: LadderCellKind::Box(LadderBoxKind::Call {
                    callee,
                    args: parsed,
                    output: out_lv,
                }),
            })
        }
        RawLadderCell::ForeignBox {
            id,
            row,
            col,
            language,
            fidelity,
            text,
            pack_hint,
        } => Ok(LadderCell {
            id,
            row,
            col,
            kind: LadderCellKind::Box(LadderBoxKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            }),
        }),
    }
}

fn raw_to_chart(raw: RawSfcChart, file_id: Option<FileId>) -> Result<SfcChart, ProjectError> {
    let mut steps = Vec::with_capacity(raw.steps.len());
    for s in raw.steps {
        steps.push(raw_to_step(s, file_id)?);
    }
    let mut transitions = Vec::with_capacity(raw.transitions.len());
    for t in raw.transitions {
        transitions.push(raw_to_transition(t, file_id)?);
    }
    Ok(SfcChart {
        steps,
        transitions,
        evolve_to_stability: raw.evolve_to_stability,
    })
}

fn raw_to_step(raw: RawSfcStep, file_id: Option<FileId>) -> Result<SfcStep, ProjectError> {
    let mut actions = Vec::with_capacity(raw.actions.len());
    for a in raw.actions {
        actions.push(raw_to_action(a, file_id)?);
    }
    Ok(SfcStep {
        id: raw.id,
        name: raw.name,
        initial: raw.initial,
        actions,
        macro_chart: raw.macro_chart,
    })
}

fn raw_to_action(raw: RawSfcAction, file_id: Option<FileId>) -> Result<SfcAction, ProjectError> {
    let duration = match raw.duration {
        Some(ref d) => Some(parse_spanned_expr(d, file_id)?),
        None => None,
    };
    let target = match raw.target {
        Some(ref t) => Some(parse_spanned_lvalue(t, file_id)?),
        None => None,
    };
    Ok(SfcAction {
        id: raw.id,
        qualifier: raw.qualifier,
        duration,
        target,
        body: convert_stmts(raw.body, file_id)?,
    })
}

fn raw_to_transition(
    raw: RawSfcTransition,
    file_id: Option<FileId>,
) -> Result<SfcTransition, ProjectError> {
    Ok(SfcTransition {
        id: raw.id,
        name: raw.name,
        from_steps: raw.from_steps,
        to_steps: raw.to_steps,
        priority: raw.priority,
        receptivity: parse_spanned_expr(&raw.receptivity, file_id)?,
    })
}

// ===========================================================================
// Canonical Project -> OutProject serializer
// ===========================================================================

pub(crate) fn project_to_raw(project: &Project) -> OutProject {
    OutProject {
        project: project.meta.clone(),
        libraries: project.libraries.clone(),
        packs: project.packs.clone(),
        comments: project.comments.clone(),
        boards: project.boards.iter().map(board_to_out).collect(),
        components: project
            .components
            .iter()
            .map(|c| OutComponentInstance {
                id: c.id.clone(),
                profile: c.profile.clone(),
                label: c.label.clone(),
                layout: c.layout.clone(),
                params: c.params.clone(),
            })
            .collect(),
        wires: project
            .wires
            .iter()
            .map(|w| OutWire {
                id: w.id.clone(),
                from: w.from.clone(),
                to: w.to.clone(),
                signal: w.signal,
                waypoints: w.waypoints.clone(),
            })
            .collect(),
    }
}

fn board_to_out(board: &Board) -> OutBoard {
    OutBoard {
        id: board.id.clone(),
        profile: board.profile.clone(),
        authoring_language: board.authoring_language,
        execution: board.execution,
        scan_period_us: board.scan_period_us,
        layout: board.layout.clone(),
        program: program_to_out(&board.program),
        comments: board.comments.clone(),
        assumptions: board.assumptions.clone(),
        presentation: board.presentation.clone(),
    }
}

fn program_to_out(prog: &BoardProgram) -> OutBoardProgram {
    OutBoardProgram {
        imports: prog.imports.clone(),
        hardware: prog.hardware.clone(),
        types: prog.types.clone(),
        data: prog.data.iter().map(data_to_out).collect(),
        modules: prog.modules.iter().map(module_to_out).collect(),
        pous: prog.pous.iter().map(pou_to_out).collect(),
    }
}

fn module_to_out(m: &ModuleDecl) -> OutModuleDecl {
    OutModuleDecl {
        id: m.id.clone(),
        name: m.name.clone(),
        imports: m.imports.clone(),
        exports: m.exports.clone(),
        types: m.types.clone(),
        data: m.data.iter().map(data_to_out).collect(),
        pous: m.pous.iter().map(pou_to_out).collect(),
    }
}

fn data_to_out(d: &DataDecl) -> OutDataDecl {
    OutDataDecl {
        id: d.id.clone(),
        name: d.name.clone(),
        scope: d.scope,
        ty: d.ty.clone(),
        initial: d.initial.as_ref().map(print_expr),
    }
}

fn pou_to_out(pou: &Pou) -> OutPou {
    let (body_kind, body, network, chart) = match &pou.body {
        PouBody::Statements(stmts) => (
            RawPouBodyKind::Statements,
            stmts.iter().map(stmt_to_out).collect(),
            None,
            None,
        ),
        PouBody::Network(net) => (
            RawPouBodyKind::Network,
            Vec::new(),
            Some(ladder_to_out(net)),
            None,
        ),
        PouBody::Chart(c) => (
            RawPouBodyKind::Chart,
            Vec::new(),
            None,
            Some(chart_to_out(c)),
        ),
    };
    OutPou {
        id: pou.id.clone(),
        name: pou.name.clone(),
        kind: pou.kind.clone(),
        preferred_view: pou.preferred_view,
        params: pou.params.clone(),
        locals: pou.locals.iter().map(data_to_out).collect(),
        body_kind,
        body,
        network,
        chart,
    }
}

fn stmt_to_out(stmt: &Stmt) -> OutStmt {
    match &stmt.kind {
        StmtKind::Assign { target, value } => OutStmt::Assign {
            id: stmt.id.clone(),
            target: print_lvalue(target),
            value: print_expr(value),
        },
        StmtKind::Call { callee, args } => OutStmt::Call {
            id: stmt.id.clone(),
            callee: callee.clone(),
            args: args
                .iter()
                .map(|a| RawCallArg {
                    name: a.name.clone(),
                    value: print_expr(&a.value),
                })
                .collect(),
        },
        StmtKind::FbInvoke { instance, inputs } => OutStmt::FbInvoke {
            id: stmt.id.clone(),
            instance: print_lvalue(instance),
            inputs: inputs
                .iter()
                .map(|(n, e)| RawNamedExpr {
                    name: n.clone(),
                    value: print_expr(e),
                })
                .collect(),
        },
        StmtKind::If {
            condition,
            then_body,
            elif_arms,
            else_body,
        } => OutStmt::If {
            id: stmt.id.clone(),
            condition: print_expr(condition),
            then_body: then_body.iter().map(stmt_to_out).collect(),
            elif_arms: elif_arms
                .iter()
                .map(|arm| OutElifArm {
                    condition: print_expr(&arm.condition),
                    body: arm.body.iter().map(stmt_to_out).collect(),
                })
                .collect(),
            else_body: else_body.iter().map(stmt_to_out).collect(),
        },
        StmtKind::Switch {
            scrutinee,
            cases,
            default,
        } => OutStmt::Switch {
            id: stmt.id.clone(),
            scrutinee: print_expr(scrutinee),
            cases: cases
                .iter()
                .map(|c| OutSwitchCase {
                    patterns: c
                        .patterns
                        .iter()
                        .map(|p| match p {
                            CasePattern::Value(e) => RawCasePattern::Value {
                                expr: print_expr(e),
                            },
                            CasePattern::Range { start, end } => RawCasePattern::Range {
                                start: print_expr(start),
                                end: print_expr(end),
                            },
                        })
                        .collect(),
                    body: c.body.iter().map(stmt_to_out).collect(),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|stmts| stmts.iter().map(stmt_to_out).collect()),
        },
        StmtKind::While { condition, body } => OutStmt::While {
            id: stmt.id.clone(),
            condition: print_expr(condition),
            body: body.iter().map(stmt_to_out).collect(),
        },
        StmtKind::For {
            counter,
            from,
            to,
            step,
            inclusive,
            body,
        } => OutStmt::For {
            id: stmt.id.clone(),
            counter: counter.clone(),
            from: print_expr(from),
            to: print_expr(to),
            step: print_expr(step),
            inclusive: *inclusive,
            body: body.iter().map(stmt_to_out).collect(),
        },
        StmtKind::Break => OutStmt::Break {
            id: stmt.id.clone(),
        },
        StmtKind::Continue => OutStmt::Continue {
            id: stmt.id.clone(),
        },
        StmtKind::Return { value } => OutStmt::Return {
            id: stmt.id.clone(),
            value: value.as_ref().map(print_expr),
        },
        StmtKind::Wait { duration } => OutStmt::Wait {
            id: stmt.id.clone(),
            duration: print_expr(duration),
        },
        StmtKind::Foreign {
            language,
            fidelity,
            text,
            pack_hint,
        } => OutStmt::Foreign {
            id: stmt.id.clone(),
            language: language.clone(),
            fidelity: *fidelity,
            text: text.clone(),
            pack_hint: pack_hint.clone(),
        },
    }
}

fn ladder_to_out(net: &LadderNetwork) -> OutLadderNetwork {
    OutLadderNetwork {
        rungs: net
            .rungs
            .iter()
            .map(|r| OutLadderRung {
                id: r.id.clone(),
                order: r.order,
                cells: r.cells.iter().map(cell_to_out).collect(),
            })
            .collect(),
    }
}

fn cell_to_out(cell: &LadderCell) -> OutLadderCell {
    match &cell.kind {
        LadderCellKind::Empty => OutLadderCell::Empty {
            id: cell.id.clone(),
            row: cell.row,
            col: cell.col,
        },
        LadderCellKind::Wire => OutLadderCell::Wire {
            id: cell.id.clone(),
            row: cell.row,
            col: cell.col,
        },
        LadderCellKind::Contact { kind, target } => OutLadderCell::Contact {
            id: cell.id.clone(),
            row: cell.row,
            col: cell.col,
            contact_kind: *kind,
            target: print_lvalue(target),
        },
        LadderCellKind::Coil { kind, target } => OutLadderCell::Coil {
            id: cell.id.clone(),
            row: cell.row,
            col: cell.col,
            coil_kind: *kind,
            target: print_lvalue(target),
        },
        LadderCellKind::ParallelOr { branches } => OutLadderCell::ParallelOr {
            id: cell.id.clone(),
            row: cell.row,
            col: cell.col,
            branches: branches
                .iter()
                .map(|br| br.iter().map(cell_to_out).collect())
                .collect(),
        },
        LadderCellKind::Box(box_kind) => match box_kind {
            LadderBoxKind::Fb {
                kind,
                instance,
                inputs,
            } => OutLadderCell::FbBox {
                id: cell.id.clone(),
                row: cell.row,
                col: cell.col,
                fb_kind: kind.clone(),
                instance: print_lvalue(instance),
                inputs: inputs
                    .iter()
                    .map(|(n, e)| RawNamedExpr {
                        name: n.clone(),
                        value: print_expr(e),
                    })
                    .collect(),
            },
            LadderBoxKind::Compare { op, left, right } => OutLadderCell::CompareBox {
                id: cell.id.clone(),
                row: cell.row,
                col: cell.col,
                op: *op,
                left: print_expr(left),
                right: print_expr(right),
            },
            LadderBoxKind::Call {
                callee,
                args,
                output,
            } => OutLadderCell::CallBox {
                id: cell.id.clone(),
                row: cell.row,
                col: cell.col,
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|(n, e)| RawCallArg {
                        name: n.clone(),
                        value: print_expr(e),
                    })
                    .collect(),
                output: output.as_ref().map(print_lvalue),
            },
            LadderBoxKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => OutLadderCell::ForeignBox {
                id: cell.id.clone(),
                row: cell.row,
                col: cell.col,
                language: language.clone(),
                fidelity: *fidelity,
                text: text.clone(),
                pack_hint: pack_hint.clone(),
            },
        },
    }
}

fn chart_to_out(chart: &SfcChart) -> OutSfcChart {
    OutSfcChart {
        steps: chart
            .steps
            .iter()
            .map(|s| OutSfcStep {
                id: s.id.clone(),
                name: s.name.clone(),
                initial: s.initial,
                actions: s
                    .actions
                    .iter()
                    .map(|a| OutSfcAction {
                        id: a.id.clone(),
                        qualifier: a.qualifier,
                        duration: a.duration.as_ref().map(print_expr),
                        target: a.target.as_ref().map(print_lvalue),
                        body: a.body.iter().map(stmt_to_out).collect(),
                    })
                    .collect(),
                macro_chart: s.macro_chart.clone(),
            })
            .collect(),
        transitions: chart
            .transitions
            .iter()
            .map(|t| OutSfcTransition {
                id: t.id.clone(),
                name: t.name.clone(),
                from_steps: t.from_steps.clone(),
                to_steps: t.to_steps.clone(),
                priority: t.priority,
                receptivity: print_expr(&t.receptivity),
            })
            .collect(),
        evolve_to_stability: chart.evolve_to_stability,
    }
}
