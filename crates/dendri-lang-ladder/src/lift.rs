//! Conservative structural lifting from canonical ASL `Stmt` patterns into first-class
//! `LadderNetwork` (`docs/lbc/ladder.md` §1.3, `04-frontends-visual-and-sync.md` §5.1).
//!
//! Recognizes ONLY the five declared canonical Ladder patterns:
//! 1. Pure boolean coil assignments (`out := (start OR out) AND NOT stop;`).
//! 2. Guarded latch / unlatch (`IF cond THEN out := true/false; END_IF;` -> `(S)` / `(R)`).
//! 3. Canonical FB call optionally followed by output wire (`t1(IN := rail, PT := T#500ms); q := t1.Q;`).
//! 4. `EN`-gated comparison (`out := rail AND (a >= b);` -> `CMP GE`).
//! 5. `EN`-gated stateless call / math / move (`IF rail THEN dst := callee(args...); END_IF;` -> `CALL`).
//!
//! Any POU containing loops (`WHILE`/`FOR`), `CASE`/`Switch`, `ELSIF`/`ELSE`, multi-statement `IF`,
//! `RETURN`/`BREAK`/`CONTINUE`/`WAIT`, `Foreign`, or arbitrary procedural assignments returns `None`
//! cleanly so it remains an intact callable `Structured POU` box and is NEVER flattened!

use crate::common::{fb_primary_input_pin, fb_primary_output_pin};
use dendri_ast::{
    BinaryOp, Expr, ExprKind, FbKind, LValue, LadderBoxKind, LadderCell, LadderCellKind,
    LadderCoilKind, LadderCompareOp, LadderContactKind, LadderNetwork, LadderRung, Literal, NodeId,
    Stmt, StmtKind, UnaryOp,
};
use dendri_span::Span;
use std::collections::BTreeMap;

/// Conservatively lifts a slice of ASL [`Stmt`]s into a structural [`LadderNetwork`].
///
/// Returns `None` if ANY statement falls outside the declared canonical Ladder patterns,
/// keeping non-liftable POUs intact as callable `Structured POU` boxes (`docs/lbc/ladder.md` §1.3).
#[must_use]
pub fn lift_statements_to_network(stmts: &[Stmt]) -> Option<LadderNetwork> {
    let empty_fb_kinds = BTreeMap::new();
    lift_statements_to_network_with_fb_kinds(stmts, &empty_fb_kinds)
}

/// Conservatively lifts a slice of ASL [`Stmt`]s into a structural [`LadderNetwork`],
/// using caller-provided declared `fb_kinds` (`instance_name -> FbKind`) for exact FB resolution.
#[must_use]
pub fn lift_statements_to_network_with_fb_kinds(
    stmts: &[Stmt],
    fb_kinds: &BTreeMap<String, FbKind>,
) -> Option<LadderNetwork> {
    let mut lifter = Lifter {
        fb_kinds,
        next_cell_seq: 0,
    };
    lifter.lift_all(stmts)
}

struct Lifter<'a> {
    fb_kinds: &'a BTreeMap<String, FbKind>,
    next_cell_seq: u32,
}

impl Lifter<'_> {
    fn alloc_cell_id(&mut self, rung_order: u32) -> NodeId {
        let id = NodeId::new(format!("n_lift_r{rung_order}_c{}", self.next_cell_seq));
        self.next_cell_seq = self.next_cell_seq.saturating_add(1);
        id
    }

    fn lift_all(&mut self, stmts: &[Stmt]) -> Option<LadderNetwork> {
        let mut rungs = Vec::new();
        let mut idx = 0usize;
        let mut order = 0u32;

        while stmts.get(idx).is_some() {
            self.next_cell_seq = 0;
            let (cells, consumed) = self.lift_statement_at(stmts, idx, order)?;
            let rung_id = NodeId::new(format!("n_lift_r{order}"));
            let laid_out = layout_rung_cells(cells);
            rungs.push(LadderRung {
                id: rung_id,
                order,
                cells: laid_out,
            });
            order = order.saturating_add(1);
            idx = idx.saturating_add(consumed);
        }

        Some(LadderNetwork { rungs })
    }

    fn lift_statement_at(
        &mut self,
        stmts: &[Stmt],
        idx: usize,
        rung_order: u32,
    ) -> Option<(Vec<LadderCell>, usize)> {
        let stmt = stmts.get(idx)?;
        match &stmt.kind {
            // Pattern 1 & Pattern 4: pure boolean / comparison coil assignment
            StmtKind::Assign { target, value } => {
                if lvalue_contains_foreign(target) || expr_contains_foreign(value) {
                    return None;
                }
                let mut cells = self.lift_rail_expr(value, rung_order)?;
                let coil_id = self.alloc_cell_id(rung_order);
                cells.push(LadderCell {
                    id: coil_id,
                    row: 0,
                    col: 0,
                    kind: LadderCellKind::Coil {
                        kind: LadderCoilKind::Normal,
                        target: target.strip_spans(),
                    },
                });
                Some((cells, 1))
            }

            // Pattern 2 (Set/Reset) & Pattern 5 (EN-gated Call / Math / Move)
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                if !elif_arms.is_empty() || !else_body.is_empty() || then_body.len() != 1 {
                    return None;
                }
                if expr_contains_foreign(condition) {
                    return None;
                }
                let mut cells = self.lift_rail_expr(condition, rung_order)?;
                let inner = then_body.first()?;
                match &inner.kind {
                    StmtKind::Assign { target, value } => {
                        if lvalue_contains_foreign(target) || expr_contains_foreign(value) {
                            return None;
                        }
                        match &value.kind {
                            // Pattern 2a: Guarded Set latch (`IF cond THEN out := true; END_IF;`)
                            ExprKind::Literal(Literal::Bool(true)) => {
                                let id = self.alloc_cell_id(rung_order);
                                cells.push(LadderCell {
                                    id,
                                    row: 0,
                                    col: 0,
                                    kind: LadderCellKind::Coil {
                                        kind: LadderCoilKind::Set,
                                        target: target.strip_spans(),
                                    },
                                });
                                Some((cells, 1))
                            }
                            // Pattern 2b: Guarded Reset unlatch (`IF cond THEN out := false; END_IF;`)
                            ExprKind::Literal(Literal::Bool(false)) => {
                                let id = self.alloc_cell_id(rung_order);
                                cells.push(LadderCell {
                                    id,
                                    row: 0,
                                    col: 0,
                                    kind: LadderCellKind::Coil {
                                        kind: LadderCoilKind::Reset,
                                        target: target.strip_spans(),
                                    },
                                });
                                Some((cells, 1))
                            }
                            // Pattern 5a: EN-gated stateless function call
                            ExprKind::Call { callee, args } => {
                                if is_blocking_wait_callee(callee) {
                                    return None;
                                }
                                let mapped_args = args
                                    .iter()
                                    .map(|a| (a.name.clone(), a.value.strip_spans()))
                                    .collect();
                                let id = self.alloc_cell_id(rung_order);
                                cells.push(LadderCell {
                                    id,
                                    row: 0,
                                    col: 0,
                                    kind: LadderCellKind::Box(LadderBoxKind::Call {
                                        callee: callee.clone(),
                                        args: mapped_args,
                                        output: Some(target.strip_spans()),
                                    }),
                                });
                                Some((cells, 1))
                            }
                            // Pattern 5b: EN-gated elemental arithmetic (`ADD`, `SUB`, `MUL`, `DIV`, `MOD`)
                            ExprKind::Binary { op, left, right } => {
                                let math_callee = match op {
                                    BinaryOp::Add => "ADD",
                                    BinaryOp::Sub => "SUB",
                                    BinaryOp::Mul => "MUL",
                                    BinaryOp::Div => "DIV",
                                    BinaryOp::Mod => "MOD",
                                    BinaryOp::Shl
                                    | BinaryOp::Shr
                                    | BinaryOp::Lt
                                    | BinaryOp::Le
                                    | BinaryOp::Gt
                                    | BinaryOp::Ge
                                    | BinaryOp::Eq
                                    | BinaryOp::Ne
                                    | BinaryOp::BitAnd
                                    | BinaryOp::BitXor
                                    | BinaryOp::BitOr
                                    | BinaryOp::And
                                    | BinaryOp::Xor
                                    | BinaryOp::Or => return None,
                                };
                                let id = self.alloc_cell_id(rung_order);
                                cells.push(LadderCell {
                                    id,
                                    row: 0,
                                    col: 0,
                                    kind: LadderCellKind::Box(LadderBoxKind::Call {
                                        callee: math_callee.to_owned(),
                                        args: vec![
                                            (None, left.strip_spans()),
                                            (None, right.strip_spans()),
                                        ],
                                        output: Some(target.strip_spans()),
                                    }),
                                });
                                Some((cells, 1))
                            }
                            // Pattern 5c: EN-gated MOVE (`IF rail THEN dst := src; END_IF;`)
                            ExprKind::Literal(
                                Literal::Int(_)
                                | Literal::Float(_)
                                | Literal::TimeUs(_)
                                | Literal::String(_),
                            )
                            | ExprKind::Ident(_)
                            | ExprKind::Index { .. }
                            | ExprKind::Field { .. } => {
                                let id = self.alloc_cell_id(rung_order);
                                cells.push(LadderCell {
                                    id,
                                    row: 0,
                                    col: 0,
                                    kind: LadderCellKind::Box(LadderBoxKind::Call {
                                        callee: "MOVE".to_owned(),
                                        args: vec![(None, value.strip_spans())],
                                        output: Some(target.strip_spans()),
                                    }),
                                });
                                Some((cells, 1))
                            }
                            ExprKind::Unary { .. } | ExprKind::Foreign { .. } => None,
                        }
                    }
                    // Pattern 5d: EN-gated void call (`IF rail THEN callee(args...); END_IF;`)
                    StmtKind::Call { callee, args } => {
                        if is_blocking_wait_callee(callee)
                            || args.iter().any(|a| expr_contains_foreign(&a.value))
                        {
                            return None;
                        }
                        let mapped_args = args
                            .iter()
                            .map(|a| (a.name.clone(), a.value.strip_spans()))
                            .collect();
                        let id = self.alloc_cell_id(rung_order);
                        cells.push(LadderCell {
                            id,
                            row: 0,
                            col: 0,
                            kind: LadderCellKind::Box(LadderBoxKind::Call {
                                callee: callee.clone(),
                                args: mapped_args,
                                output: None,
                            }),
                        });
                        Some((cells, 1))
                    }
                    // Any other nested statement inside IF is conservatively rejected
                    StmtKind::FbInvoke { .. }
                    | StmtKind::If { .. }
                    | StmtKind::Switch { .. }
                    | StmtKind::While { .. }
                    | StmtKind::For { .. }
                    | StmtKind::Break
                    | StmtKind::Continue
                    | StmtKind::Return { .. }
                    | StmtKind::Wait { .. }
                    | StmtKind::Foreign { .. } => None,
                }
            }

            // Pattern 3: Canonical FB call optionally followed by output wire (`q := fb.Q;`)
            StmtKind::FbInvoke { instance, inputs } => {
                if lvalue_contains_foreign(instance)
                    || inputs.iter().any(|(_, e)| expr_contains_foreign(e))
                {
                    return None;
                }
                let fb_kind = self.resolve_fb_kind(instance, inputs)?;
                let trigger_pin = fb_primary_input_pin(&fb_kind);
                let out_pin = fb_primary_output_pin(&fb_kind);

                let mut trigger_expr: Option<&Expr> = None;
                let mut remaining_inputs = Vec::new();
                for (pin, expr) in inputs {
                    if pin.eq_ignore_ascii_case(trigger_pin) && trigger_expr.is_none() {
                        trigger_expr = Some(expr);
                    } else {
                        remaining_inputs.push((pin.clone(), expr.strip_spans()));
                    }
                }
                let rail_expr = trigger_expr?;
                let mut cells = self.lift_rail_expr(rail_expr, rung_order)?;
                let fb_id = self.alloc_cell_id(rung_order);
                cells.push(LadderCell {
                    id: fb_id,
                    row: 0,
                    col: 0,
                    kind: LadderCellKind::Box(LadderBoxKind::Fb {
                        kind: fb_kind,
                        instance: instance.strip_spans(),
                        inputs: remaining_inputs,
                    }),
                });

                // Check if next statement wires `out_coil := instance.<out_pin>;`
                if let Some(next_stmt) = stmts.get(idx.saturating_add(1))
                    && let StmtKind::Assign { target, value } = &next_stmt.kind
                    && !lvalue_contains_foreign(target)
                    && matches_fb_output_expr(value, instance, out_pin)
                {
                    let coil_id = self.alloc_cell_id(rung_order);
                    cells.push(LadderCell {
                        id: coil_id,
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Coil {
                            kind: LadderCoilKind::Normal,
                            target: target.strip_spans(),
                        },
                    });
                    return Some((cells, 2));
                }

                Some((cells, 1))
            }

            // Explicitly non-liftable structured constructs: stay intact as `Structured POU`!
            StmtKind::Call { .. }
            | StmtKind::Switch { .. }
            | StmtKind::While { .. }
            | StmtKind::For { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Return { .. }
            | StmtKind::Wait { .. }
            | StmtKind::Foreign { .. } => None,
        }
    }

    fn resolve_fb_kind(&self, instance: &LValue, inputs: &[(String, Expr)]) -> Option<FbKind> {
        if let Some(kind) = self.fb_kinds.get(instance.root_name()) {
            return Some(kind.clone());
        }
        let has_pin = |pin: &str| inputs.iter().any(|(k, _)| k.eq_ignore_ascii_case(pin));
        let lower_name = instance.root_name().to_ascii_lowercase();

        if has_pin("PT") {
            if lower_name.contains("tof") {
                return Some(FbKind::Tof);
            }
            if lower_name.starts_with("tp") || lower_name.contains("_tp") {
                return Some(FbKind::Tp);
            }
            return Some(FbKind::Ton);
        }
        if has_pin("CU") && has_pin("CD") {
            return Some(FbKind::Ctud);
        }
        if has_pin("CU") {
            return Some(FbKind::Ctu);
        }
        if has_pin("CD") {
            return Some(FbKind::Ctd);
        }
        if has_pin("S1") {
            return Some(FbKind::Sr);
        }
        if has_pin("R1") {
            return Some(FbKind::Rs);
        }
        if has_pin("CLK") {
            if lower_name.contains("f_trig") || lower_name.contains("ftrig") {
                return Some(FbKind::FTrig);
            }
            return Some(FbKind::RTrig);
        }
        if has_pin("EN_R") {
            return Some(FbKind::UartRecv);
        }
        if has_pin("ENABLE") {
            return Some(FbKind::MbServer);
        }
        if has_pin("REQ") {
            if has_pin("FC") {
                return Some(FbKind::MbClient);
            }
            if has_pin("DATA") {
                return Some(FbKind::UartSend);
            }
            if has_pin("ADDR") {
                return Some(FbKind::I2cXfer);
            }
            if has_pin("TX") {
                return Some(FbKind::SpiXfer);
            }
        }
        if has_pin("EN") {
            return Some(FbKind::User {
                fb_name: instance.root_name().to_owned(),
            });
        }
        None
    }

    fn lift_rail_expr(&mut self, expr: &Expr, rung_order: u32) -> Option<Vec<LadderCell>> {
        match &expr.kind {
            ExprKind::Literal(Literal::Bool(true)) => {
                let id = self.alloc_cell_id(rung_order);
                Some(vec![LadderCell {
                    id,
                    row: 0,
                    col: 0,
                    kind: LadderCellKind::Wire,
                }])
            }
            ExprKind::Literal(Literal::Bool(false)) => {
                let id = self.alloc_cell_id(rung_order);
                Some(vec![LadderCell {
                    id,
                    row: 0,
                    col: 0,
                    kind: LadderCellKind::Empty,
                }])
            }
            ExprKind::Ident(_) | ExprKind::Index { .. } | ExprKind::Field { .. } => {
                let target = expr_to_lvalue(expr)?;
                let id = self.alloc_cell_id(rung_order);
                Some(vec![LadderCell {
                    id,
                    row: 0,
                    col: 0,
                    kind: LadderCellKind::Contact {
                        kind: LadderContactKind::NormallyOpen,
                        target,
                    },
                }])
            }
            ExprKind::Unary { op, operand } => match op {
                UnaryOp::Not => match &operand.kind {
                    ExprKind::Literal(Literal::Bool(true)) => {
                        let id = self.alloc_cell_id(rung_order);
                        Some(vec![LadderCell {
                            id,
                            row: 0,
                            col: 0,
                            kind: LadderCellKind::Empty,
                        }])
                    }
                    ExprKind::Literal(Literal::Bool(false)) => {
                        let id = self.alloc_cell_id(rung_order);
                        Some(vec![LadderCell {
                            id,
                            row: 0,
                            col: 0,
                            kind: LadderCellKind::Wire,
                        }])
                    }
                    ExprKind::Ident(_) | ExprKind::Index { .. } | ExprKind::Field { .. } => {
                        let target = expr_to_lvalue(operand)?;
                        let id = self.alloc_cell_id(rung_order);
                        Some(vec![LadderCell {
                            id,
                            row: 0,
                            col: 0,
                            kind: LadderCellKind::Contact {
                                kind: LadderContactKind::NormallyClosed,
                                target,
                            },
                        }])
                    }
                    ExprKind::Literal(
                        Literal::Int(_)
                        | Literal::Float(_)
                        | Literal::TimeUs(_)
                        | Literal::String(_),
                    )
                    | ExprKind::Call { .. }
                    | ExprKind::Unary { .. }
                    | ExprKind::Binary { .. }
                    | ExprKind::Foreign { .. } => None,
                },
                UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot => None,
            },
            ExprKind::Binary { op, left, right } => match op {
                BinaryOp::And => {
                    let mut cells = self.lift_rail_expr(left, rung_order)?;
                    let right_cells = self.lift_rail_expr(right, rung_order)?;
                    cells.extend(right_cells);
                    Some(cells)
                }
                BinaryOp::Or => {
                    let mut or_terms = Vec::new();
                    collect_or_terms(expr, &mut or_terms);
                    let mut branches = Vec::with_capacity(or_terms.len());
                    for term in or_terms {
                        branches.push(self.lift_rail_expr(term, rung_order)?);
                    }
                    let id = self.alloc_cell_id(rung_order);
                    Some(vec![LadderCell {
                        id,
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::ParallelOr { branches },
                    }])
                }
                BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge => {
                    if expr_contains_foreign(left) || expr_contains_foreign(right) {
                        return None;
                    }
                    let cmp_op = match op {
                        BinaryOp::Eq => LadderCompareOp::Eq,
                        BinaryOp::Ne => LadderCompareOp::Ne,
                        BinaryOp::Lt => LadderCompareOp::Lt,
                        BinaryOp::Le => LadderCompareOp::Le,
                        BinaryOp::Gt => LadderCompareOp::Gt,
                        BinaryOp::Ge => LadderCompareOp::Ge,
                        BinaryOp::Mul
                        | BinaryOp::Div
                        | BinaryOp::Mod
                        | BinaryOp::Add
                        | BinaryOp::Sub
                        | BinaryOp::Shl
                        | BinaryOp::Shr
                        | BinaryOp::BitAnd
                        | BinaryOp::BitXor
                        | BinaryOp::BitOr
                        | BinaryOp::And
                        | BinaryOp::Xor
                        | BinaryOp::Or => return None,
                    };
                    let id = self.alloc_cell_id(rung_order);
                    Some(vec![LadderCell {
                        id,
                        row: 0,
                        col: 0,
                        kind: LadderCellKind::Box(LadderBoxKind::Compare {
                            op: cmp_op,
                            left: left.strip_spans(),
                            right: right.strip_spans(),
                        }),
                    }])
                }
                BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Mod
                | BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Shl
                | BinaryOp::Shr
                | BinaryOp::BitAnd
                | BinaryOp::BitXor
                | BinaryOp::BitOr
                | BinaryOp::Xor => None,
            },
            ExprKind::Literal(
                Literal::Int(_) | Literal::Float(_) | Literal::TimeUs(_) | Literal::String(_),
            )
            | ExprKind::Call { .. }
            | ExprKind::Foreign { .. } => None,
        }
    }
}

fn collect_or_terms<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let ExprKind::Binary {
        op: BinaryOp::Or,
        left,
        right,
    } = &expr.kind
    {
        collect_or_terms(left, out);
        collect_or_terms(right, out);
    } else {
        out.push(expr);
    }
}

fn expr_to_lvalue(expr: &Expr) -> Option<LValue> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(LValue::Var {
            name: name.clone(),
            span: Span::default(),
        }),
        ExprKind::Index { base, index } => {
            if expr_contains_foreign(index) {
                return None;
            }
            Some(LValue::Index {
                base: Box::new(expr_to_lvalue(base)?),
                index: Box::new(index.strip_spans()),
                span: Span::default(),
            })
        }
        ExprKind::Field { base, field } => Some(LValue::Field {
            base: Box::new(expr_to_lvalue(base)?),
            field: field.clone(),
            span: Span::default(),
        }),
        ExprKind::Literal(_)
        | ExprKind::Call { .. }
        | ExprKind::Unary { .. }
        | ExprKind::Binary { .. }
        | ExprKind::Foreign { .. } => None,
    }
}

fn matches_fb_output_expr(expr: &Expr, instance: &LValue, expected_pin: &str) -> bool {
    let ExprKind::Field { base, field } = &expr.kind else {
        return false;
    };
    if !field.eq_ignore_ascii_case(expected_pin) {
        return false;
    }
    let Some(base_lv) = expr_to_lvalue(base) else {
        return false;
    };
    base_lv.strip_spans() == instance.strip_spans()
}

fn is_blocking_wait_callee(callee: &str) -> bool {
    callee.eq_ignore_ascii_case("WAIT_TIME") || callee.eq_ignore_ascii_case("WAIT_TIME_US")
}

fn lvalue_contains_foreign(lv: &LValue) -> bool {
    match lv {
        LValue::Var { .. } => false,
        LValue::Index { base, index, .. } => {
            lvalue_contains_foreign(base) || expr_contains_foreign(index)
        }
        LValue::Field { base, .. } => lvalue_contains_foreign(base),
    }
}

fn expr_contains_foreign(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) => false,
        ExprKind::Index { base, index } => {
            expr_contains_foreign(base) || expr_contains_foreign(index)
        }
        ExprKind::Field { base, .. } => expr_contains_foreign(base),
        ExprKind::Call { args, .. } => args.iter().any(|a| expr_contains_foreign(&a.value)),
        ExprKind::Unary { operand, .. } => expr_contains_foreign(operand),
        ExprKind::Binary { left, right, .. } => {
            expr_contains_foreign(left) || expr_contains_foreign(right)
        }
        ExprKind::Foreign { .. } => true,
    }
}

/// Assigns deterministic non-overlapping `(row, col)` grid coordinates left-to-right across a rung
/// and top-to-bottom across nested `ParallelOr` branches.
fn layout_rung_cells(cells: Vec<LadderCell>) -> Vec<LadderCell> {
    let (laid_out, _, _) = layout_series(cells, 0, 0);
    laid_out
}

fn layout_series(
    cells: Vec<LadderCell>,
    base_row: u32,
    start_col: u32,
) -> (Vec<LadderCell>, u32, u32) {
    let mut cur_col = start_col;
    let mut max_height = 1u32;
    let mut out = Vec::with_capacity(cells.len());

    for mut cell in cells {
        cell.row = base_row;
        cell.col = cur_col;
        match cell.kind {
            LadderCellKind::ParallelOr { branches } => {
                let mut br_row = base_row;
                let mut max_br_end_col = cur_col.saturating_add(1);
                let mut laid_branches = Vec::with_capacity(branches.len());
                for br in branches {
                    let (laid_br, end_col, br_height) = layout_series(br, br_row, cur_col);
                    max_br_end_col = max_br_end_col.max(end_col);
                    br_row = br_row.saturating_add(br_height);
                    laid_branches.push(laid_br);
                }
                let total_h = br_row.saturating_sub(base_row).max(1);
                max_height = max_height.max(total_h);
                cur_col = max_br_end_col;
                cell.kind = LadderCellKind::ParallelOr {
                    branches: laid_branches,
                };
            }
            LadderCellKind::Empty
            | LadderCellKind::Wire
            | LadderCellKind::Contact { .. }
            | LadderCellKind::Coil { .. }
            | LadderCellKind::Box(_) => {
                cur_col = cur_col.saturating_add(1);
            }
        }
        out.push(cell);
    }

    (out, cur_col, max_height)
}
