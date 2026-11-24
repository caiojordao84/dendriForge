//! Deterministic derived projection from structural `LadderNetwork` to materialized
//! power-flow ASL `Stmt`s for ST / C++ derived export (`docs/lbc/ladder.md` §1.2–§1.3).
//!
//! Semantics match `dendri-vm`'s load-time lowering (`crates/dendri-vm/src/lower.rs` & `vm.rs`)
//! bit-for-bit:
//! - Rungs execute sorted ascending by explicit `rung.order`.
//! - Cells within each rung and branch execute sorted ascending by `(cell.col, cell.row)`.
//! - Every rung starts with left rail initialized to `true`.
//! - `Empty` cell sets `out_rail := false` (open circuit, NEVER conducts).
//! - `Normal` coil writes `target := in_rail` unconditionally on BOTH `true` and `false`.
//! - Coils propagate pre-mutation materialized `out_rail := in_rail` downstream, eliminating
//!   mid-rung mutation hazards (`[a] --(R a)-- (b)`).
//! - Uses IEC 61131-3-safe local identifiers (`ld_rail_N`, `ld_edge_M`) strictly in derived
//!   output — NEVER persisted back into `PouBody::Network`.

use crate::common::{fb_primary_input_pin, fb_primary_output_pin};
use dendri_ast::{
    BinaryOp, CallArg, DataDecl, Expr, ExprKind, FbKind, LValue, LadderBoxKind, LadderCell,
    LadderCellKind, LadderCoilKind, LadderCompareOp, LadderContactKind, LadderNetwork, LadderRung,
    Literal, NodeId, ScalarType, Stmt, StmtKind, StorageScope, TypeRef, UnaryOp,
};
use dendri_frontend_core::{
    BoardContext, ForeignExportDecision, ForeignLevel, ForeignSite, FrontendOutput, FrontendResult,
    LanguageId, RenderedComment, check_foreign_export,
};
use dendri_span::{Diagnostic, Span};

/// Complete derived projection output including ephemeral local rail declarations and stateful
/// edge-memory declarations for `--|P|--`/`--|N|--`/`--(P)--`/`--(N)--`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedNetwork {
    pub stmts: Vec<Stmt>,
    pub rail_locals: Vec<DataDecl>,
    pub edge_decls: Vec<DataDecl>,
}

/// Cross-language export projection result including demarcated `"NOT TRANSLATED"` comments
/// when exporting a network containing `Foreign` boxes under `ctx.allow_foreign_export == true`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedNetworkExport {
    pub stmts: Vec<Stmt>,
    pub rail_locals: Vec<DataDecl>,
    pub edge_decls: Vec<DataDecl>,
    pub foreign_comments: Vec<RenderedComment>,
}

/// Deterministically projects a [`LadderNetwork`] into materialized power-flow ASL [`Stmt`]s
/// matching `dendri-vm` load-time semantics (`docs/lbc/ladder.md` §1.2–§1.3).
#[must_use]
pub fn project_network_to_statements(net: &LadderNetwork) -> Vec<Stmt> {
    project_network_with_decls(net).stmts
}

/// Deterministically projects a [`LadderNetwork`] into materialized ASL [`Stmt`]s together with
/// the derived `BOOL` declarations for ephemeral rails (`ld_rail_N`) and edge slots (`ld_edge_M`).
#[must_use]
pub fn project_network_with_decls(net: &LadderNetwork) -> ProjectedNetwork {
    let mut proj = NetworkProjector::new();
    proj.project_network(net);
    proj.finish()
}

/// Projects a [`LadderNetwork`] for cross-language export to `target_language` (e.g. ST or C++),
/// enforcing the `ERR_FOREIGN_EXPORT_BLOCKED` four-part diagnostic guard (`docs/lbc/ladder.md` §5).
///
/// - Rejects any `LadderBoxKind::Foreign` from another language unless `ctx.allow_foreign_export`
///   is enabled.
/// - When `ctx.allow_foreign_export == true`, succeeds and collects demarcated `"NOT TRANSLATED"`
///   comment blocks rendered safely for `target_language`.
pub fn project_network_for_export(
    net: &LadderNetwork,
    target_language: LanguageId,
    ctx: &BoardContext,
) -> FrontendResult<ProjectedNetworkExport> {
    let mut blocked: Vec<Diagnostic> = Vec::new();
    let mut foreign_comments: Vec<RenderedComment> = Vec::new();
    let mut diags: Vec<Diagnostic> = Vec::new();

    // Check rungs in deterministic execution order
    let mut sorted_rungs: Vec<&LadderRung> = net.rungs.iter().collect();
    sorted_rungs.sort_by_key(|r| r.order);
    for rung in sorted_rungs {
        check_cells_foreign_export(
            &rung.cells,
            target_language,
            ctx,
            &mut blocked,
            &mut foreign_comments,
            &mut diags,
        );
    }

    if !blocked.is_empty() {
        return Err(blocked);
    }

    let projected = project_network_with_decls(net);
    Ok(FrontendOutput::with_diagnostics(
        ProjectedNetworkExport {
            stmts: projected.stmts,
            rail_locals: projected.rail_locals,
            edge_decls: projected.edge_decls,
            foreign_comments,
        },
        diags,
    ))
}

fn check_cells_foreign_export(
    cells: &[LadderCell],
    target_language: LanguageId,
    ctx: &BoardContext,
    blocked: &mut Vec<Diagnostic>,
    foreign_comments: &mut Vec<RenderedComment>,
    diags: &mut Vec<Diagnostic>,
) {
    let mut sorted: Vec<&LadderCell> = cells.iter().collect();
    sorted.sort_by_key(|c| (c.col, c.row));

    for cell in sorted {
        match &cell.kind {
            LadderCellKind::Empty
            | LadderCellKind::Wire
            | LadderCellKind::Contact { .. }
            | LadderCellKind::Coil { .. } => {}
            LadderCellKind::ParallelOr { branches } => {
                for br in branches {
                    check_cells_foreign_export(
                        br,
                        target_language,
                        ctx,
                        blocked,
                        foreign_comments,
                        diags,
                    );
                }
            }
            LadderCellKind::Box(box_kind) => match box_kind {
                LadderBoxKind::Fb { .. }
                | LadderBoxKind::Compare { .. }
                | LadderBoxKind::Call { .. } => {}
                LadderBoxKind::Foreign {
                    language,
                    fidelity,
                    text,
                    pack_hint,
                } => {
                    let site = ForeignSite {
                        level: ForeignLevel::Stmt,
                        node_id: Some(&cell.id),
                        language,
                        fidelity: *fidelity,
                        text,
                        pack_hint: pack_hint.as_deref(),
                        span: Span::default(),
                    };
                    match check_foreign_export(&site, target_language, ctx) {
                        Ok(ForeignExportDecision::VerbatimSameLanguage(_)) => {}
                        Ok(ForeignExportDecision::DemarcatedNotTranslated(rendered)) => {
                            diags.extend(rendered.diagnostics.iter().cloned());
                            foreign_comments.push(rendered);
                        }
                        Err(diag) => blocked.push(diag),
                    }
                }
            },
        }
    }
}

struct NetworkProjector {
    stmts: Vec<Stmt>,
    next_rail: u32,
    next_edge: u32,
    next_stmt_id: u32,
}

impl NetworkProjector {
    const fn new() -> Self {
        Self {
            stmts: Vec::new(),
            next_rail: 0,
            next_edge: 0,
            next_stmt_id: 0,
        }
    }

    fn alloc_rail(&mut self) -> String {
        let name = format!("ld_rail_{}", self.next_rail);
        self.next_rail = self.next_rail.saturating_add(1);
        name
    }

    fn alloc_edge(&mut self) -> String {
        let name = format!("ld_edge_{}", self.next_edge);
        self.next_edge = self.next_edge.saturating_add(1);
        name
    }

    fn alloc_stmt_id(&mut self) -> NodeId {
        let id = NodeId::new(format!("n_proj_{}", self.next_stmt_id));
        self.next_stmt_id = self.next_stmt_id.saturating_add(1);
        id
    }

    fn push_stmt(&mut self, kind: StmtKind) {
        let id = self.alloc_stmt_id();
        self.stmts.push(Stmt::new(id, kind, Span::default()));
    }

    fn assign_var(&mut self, name: &str, value: Expr) {
        self.push_stmt(StmtKind::Assign {
            target: var_lvalue(name),
            value,
        });
    }

    fn project_network(&mut self, net: &LadderNetwork) {
        // Sort rungs strictly ascending by explicit `rung.order` (`docs/lbc/ladder.md` §1.2)
        let mut sorted_rungs: Vec<&LadderRung> = net.rungs.iter().collect();
        sorted_rungs.sort_by_key(|r| r.order);

        for rung in sorted_rungs {
            let left_rail = self.alloc_rail();
            self.assign_var(&left_rail, bool_expr(true));

            let mut sorted_cells: Vec<&LadderCell> = rung.cells.iter().collect();
            sorted_cells.sort_by_key(|c| (c.col, c.row));

            let mut cur_rail = left_rail;
            for cell in sorted_cells {
                cur_rail = self.project_cell(cell, &cur_rail);
            }
        }
    }

    fn project_cell(&mut self, cell: &LadderCell, in_rail: &str) -> String {
        let out_rail = self.alloc_rail();
        match &cell.kind {
            // Empty cell = OPEN CIRCUIT (`false`), NEVER conducts!
            LadderCellKind::Empty => {
                self.assign_var(&out_rail, bool_expr(false));
            }
            LadderCellKind::Wire => {
                self.assign_var(&out_rail, ident_expr(in_rail));
            }
            LadderCellKind::Contact { kind, target } => {
                let target_expr = lvalue_to_expr(target);
                match kind {
                    LadderContactKind::NormallyOpen => {
                        self.assign_var(
                            &out_rail,
                            binary_expr(BinaryOp::And, ident_expr(in_rail), target_expr),
                        );
                    }
                    LadderContactKind::NormallyClosed => {
                        self.assign_var(
                            &out_rail,
                            binary_expr(BinaryOp::And, ident_expr(in_rail), not_expr(target_expr)),
                        );
                    }
                    LadderContactKind::PositiveTransition => {
                        // Read target ONCE into materialized bit rail, matching `dendri-vm`!
                        let bit_rail = self.alloc_rail();
                        let edge_var = self.alloc_edge();
                        self.assign_var(&bit_rail, target_expr);
                        let edge_hit = binary_expr(
                            BinaryOp::And,
                            ident_expr(&bit_rail),
                            not_expr(ident_expr(&edge_var)),
                        );
                        self.assign_var(
                            &out_rail,
                            binary_expr(BinaryOp::And, ident_expr(in_rail), edge_hit),
                        );
                        self.assign_var(&edge_var, ident_expr(&bit_rail));
                    }
                    LadderContactKind::NegativeTransition => {
                        let bit_rail = self.alloc_rail();
                        let edge_var = self.alloc_edge();
                        self.assign_var(&bit_rail, target_expr);
                        let edge_hit = binary_expr(
                            BinaryOp::And,
                            not_expr(ident_expr(&bit_rail)),
                            ident_expr(&edge_var),
                        );
                        self.assign_var(
                            &out_rail,
                            binary_expr(BinaryOp::And, ident_expr(in_rail), edge_hit),
                        );
                        self.assign_var(&edge_var, ident_expr(&bit_rail));
                    }
                }
            }
            LadderCellKind::Coil { kind, target } => {
                let target_clean = target.strip_spans();
                match kind {
                    LadderCoilKind::Normal => {
                        // Unconditionally writes BOTH `true` and `false`!
                        self.push_stmt(StmtKind::Assign {
                            target: target_clean,
                            value: ident_expr(in_rail),
                        });
                    }
                    LadderCoilKind::Negated => {
                        // Unconditionally writes `!in_rail`!
                        self.push_stmt(StmtKind::Assign {
                            target: target_clean,
                            value: not_expr(ident_expr(in_rail)),
                        });
                    }
                    LadderCoilKind::Set => {
                        let inner_id = self.alloc_stmt_id();
                        let set_stmt = Stmt::new(
                            inner_id,
                            StmtKind::Assign {
                                target: target_clean,
                                value: bool_expr(true),
                            },
                            Span::default(),
                        );
                        self.push_stmt(StmtKind::If {
                            condition: ident_expr(in_rail),
                            then_body: vec![set_stmt],
                            elif_arms: Vec::new(),
                            else_body: Vec::new(),
                        });
                    }
                    LadderCoilKind::Reset => {
                        let inner_id = self.alloc_stmt_id();
                        let reset_stmt = Stmt::new(
                            inner_id,
                            StmtKind::Assign {
                                target: target_clean,
                                value: bool_expr(false),
                            },
                            Span::default(),
                        );
                        self.push_stmt(StmtKind::If {
                            condition: ident_expr(in_rail),
                            then_body: vec![reset_stmt],
                            elif_arms: Vec::new(),
                            else_body: Vec::new(),
                        });
                    }
                    LadderCoilKind::PositiveTransition => {
                        let edge_var = self.alloc_edge();
                        let pulse = binary_expr(
                            BinaryOp::And,
                            ident_expr(in_rail),
                            not_expr(ident_expr(&edge_var)),
                        );
                        self.push_stmt(StmtKind::Assign {
                            target: target_clean,
                            value: pulse,
                        });
                        self.assign_var(&edge_var, ident_expr(in_rail));
                    }
                    LadderCoilKind::NegativeTransition => {
                        let edge_var = self.alloc_edge();
                        let pulse = binary_expr(
                            BinaryOp::And,
                            not_expr(ident_expr(in_rail)),
                            ident_expr(&edge_var),
                        );
                        self.push_stmt(StmtKind::Assign {
                            target: target_clean,
                            value: pulse,
                        });
                        self.assign_var(&edge_var, ident_expr(in_rail));
                    }
                }
                // Always propagate PRE-MUTATION materialized rail downstream (`[a] --(R a)-- (b)` safe!)
                self.assign_var(&out_rail, ident_expr(in_rail));
            }
            LadderCellKind::ParallelOr { branches } => {
                self.assign_var(&out_rail, bool_expr(false));
                for branch in branches {
                    let mut sorted: Vec<&LadderCell> = branch.iter().collect();
                    sorted.sort_by_key(|c| (c.col, c.row));
                    let mut br_rail = in_rail.to_owned();
                    for c in sorted {
                        br_rail = self.project_cell(c, &br_rail);
                    }
                    self.assign_var(
                        &out_rail,
                        binary_expr(BinaryOp::Or, ident_expr(&out_rail), ident_expr(&br_rail)),
                    );
                }
            }
            LadderCellKind::Box(box_kind) => match box_kind {
                LadderBoxKind::Compare { op, left, right } => {
                    let bin_op = match op {
                        LadderCompareOp::Eq => BinaryOp::Eq,
                        LadderCompareOp::Ne => BinaryOp::Ne,
                        LadderCompareOp::Lt => BinaryOp::Lt,
                        LadderCompareOp::Le => BinaryOp::Le,
                        LadderCompareOp::Gt => BinaryOp::Gt,
                        LadderCompareOp::Ge => BinaryOp::Ge,
                    };
                    let then_id = self.alloc_stmt_id();
                    let else_id = self.alloc_stmt_id();
                    let then_stmt = Stmt::new(
                        then_id,
                        StmtKind::Assign {
                            target: var_lvalue(&out_rail),
                            value: binary_expr(bin_op, left.strip_spans(), right.strip_spans()),
                        },
                        Span::default(),
                    );
                    let else_stmt = Stmt::new(
                        else_id,
                        StmtKind::Assign {
                            target: var_lvalue(&out_rail),
                            value: bool_expr(false),
                        },
                        Span::default(),
                    );
                    self.push_stmt(StmtKind::If {
                        condition: ident_expr(in_rail),
                        then_body: vec![then_stmt],
                        elif_arms: Vec::new(),
                        else_body: vec![else_stmt],
                    });
                }
                LadderBoxKind::Fb {
                    kind,
                    instance,
                    inputs,
                } => {
                    let rail_pin = fb_primary_input_pin(kind);
                    let mut fb_inputs: Vec<(String, Expr)> = inputs
                        .iter()
                        .map(|(k, v)| (k.clone(), v.strip_spans()))
                        .collect();
                    let already_bound = fb_inputs
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case(rail_pin));
                    if !already_bound {
                        fb_inputs.push((rail_pin.to_owned(), ident_expr(in_rail)));
                    }
                    let inst_clean = instance.strip_spans();
                    self.push_stmt(StmtKind::FbInvoke {
                        instance: inst_clean.clone(),
                        inputs: fb_inputs,
                    });
                    let out_expr = match kind {
                        FbKind::User { .. } => ident_expr(in_rail),
                        FbKind::Ton
                        | FbKind::Tof
                        | FbKind::Tp
                        | FbKind::Ctu
                        | FbKind::Ctd
                        | FbKind::Ctud
                        | FbKind::RTrig
                        | FbKind::FTrig
                        | FbKind::Sr
                        | FbKind::Rs
                        | FbKind::UartSend
                        | FbKind::UartRecv
                        | FbKind::I2cXfer
                        | FbKind::SpiXfer
                        | FbKind::MbClient
                        | FbKind::MbServer => Expr::new(
                            ExprKind::Field {
                                base: Box::new(lvalue_to_expr(&inst_clean)),
                                field: fb_primary_output_pin(kind).to_owned(),
                            },
                            Span::default(),
                        ),
                    };
                    self.assign_var(&out_rail, out_expr);
                }
                LadderBoxKind::Call {
                    callee,
                    args,
                    output,
                } => {
                    let inner_id = self.alloc_stmt_id();
                    let call_stmt = build_projected_call_stmt(inner_id, callee, args, output);
                    self.push_stmt(StmtKind::If {
                        condition: ident_expr(in_rail),
                        then_body: vec![call_stmt],
                        elif_arms: Vec::new(),
                        else_body: Vec::new(),
                    });
                    // IEC EN/ENO: pass EN downstream
                    self.assign_var(&out_rail, ident_expr(in_rail));
                }
                LadderBoxKind::Foreign {
                    language,
                    fidelity,
                    text,
                    pack_hint,
                } => {
                    self.push_stmt(StmtKind::Foreign {
                        language: language.clone(),
                        fidelity: *fidelity,
                        text: text.clone(),
                        pack_hint: pack_hint.clone(),
                    });
                    self.assign_var(&out_rail, ident_expr(in_rail));
                }
            },
        }
        out_rail
    }

    fn finish(self) -> ProjectedNetwork {
        let rail_locals = (0..self.next_rail)
            .map(|i| DataDecl {
                id: NodeId::new(format!("n_proj_rail_decl_{i}")),
                name: format!("ld_rail_{i}"),
                scope: StorageScope::Temp,
                ty: TypeRef::scalar(ScalarType::Bool),
                initial: Some(bool_expr(false)),
                span: Span::default(),
            })
            .collect();
        let edge_decls = (0..self.next_edge)
            .map(|i| DataDecl {
                id: NodeId::new(format!("n_proj_edge_decl_{i}")),
                name: format!("ld_edge_{i}"),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Bool),
                initial: Some(bool_expr(false)),
                span: Span::default(),
            })
            .collect();
        ProjectedNetwork {
            stmts: self.stmts,
            rail_locals,
            edge_decls,
        }
    }
}

fn build_projected_call_stmt(
    id: NodeId,
    callee: &str,
    args: &[(Option<String>, Expr)],
    output: &Option<LValue>,
) -> Stmt {
    if let Some(out_lv) = output {
        let out_clean = out_lv.strip_spans();
        if callee.eq_ignore_ascii_case("MOVE")
            && args.len() == 1
            && let Some((_, src)) = args.first()
        {
            return Stmt::new(
                id,
                StmtKind::Assign {
                    target: out_clean,
                    value: src.strip_spans(),
                },
                Span::default(),
            );
        }
        if args.len() == 2
            && let (Some((_, a)), Some((_, b))) = (args.first(), args.get(1))
        {
            let math_op = if callee.eq_ignore_ascii_case("ADD") {
                Some(BinaryOp::Add)
            } else if callee.eq_ignore_ascii_case("SUB") {
                Some(BinaryOp::Sub)
            } else if callee.eq_ignore_ascii_case("MUL") {
                Some(BinaryOp::Mul)
            } else if callee.eq_ignore_ascii_case("DIV") {
                Some(BinaryOp::Div)
            } else if callee.eq_ignore_ascii_case("MOD") {
                Some(BinaryOp::Mod)
            } else {
                None
            };
            if let Some(op) = math_op {
                return Stmt::new(
                    id,
                    StmtKind::Assign {
                        target: out_clean,
                        value: binary_expr(op, a.strip_spans(), b.strip_spans()),
                    },
                    Span::default(),
                );
            }
        }
        let call_args = to_call_args(args);
        return Stmt::new(
            id,
            StmtKind::Assign {
                target: out_clean,
                value: Expr::new(
                    ExprKind::Call {
                        callee: callee.to_owned(),
                        args: call_args,
                    },
                    Span::default(),
                ),
            },
            Span::default(),
        );
    }

    Stmt::new(
        id,
        StmtKind::Call {
            callee: callee.to_owned(),
            args: to_call_args(args),
        },
        Span::default(),
    )
}

fn to_call_args(args: &[(Option<String>, Expr)]) -> Vec<CallArg> {
    args.iter()
        .map(|(name, value)| CallArg {
            name: name.clone(),
            value: value.strip_spans(),
            span: Span::default(),
        })
        .collect()
}

fn var_lvalue(name: &str) -> LValue {
    LValue::Var {
        name: name.to_owned(),
        span: Span::default(),
    }
}

fn ident_expr(name: &str) -> Expr {
    Expr::new(ExprKind::Ident(name.to_owned()), Span::default())
}

fn bool_expr(val: bool) -> Expr {
    Expr::new(ExprKind::Literal(Literal::Bool(val)), Span::default())
}

fn not_expr(inner: Expr) -> Expr {
    Expr::new(
        ExprKind::Unary {
            op: UnaryOp::Not,
            operand: Box::new(inner),
        },
        Span::default(),
    )
}

fn binary_expr(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::new(
        ExprKind::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        },
        Span::default(),
    )
}

fn lvalue_to_expr(lv: &LValue) -> Expr {
    match lv {
        LValue::Var { name, .. } => Expr::new(ExprKind::Ident(name.clone()), Span::default()),
        LValue::Index { base, index, .. } => Expr::new(
            ExprKind::Index {
                base: Box::new(lvalue_to_expr(base)),
                index: Box::new(index.strip_spans()),
            },
            Span::default(),
        ),
        LValue::Field { base, field, .. } => Expr::new(
            ExprKind::Field {
                base: Box::new(lvalue_to_expr(base)),
                field: field.clone(),
            },
            Span::default(),
        ),
    }
}
