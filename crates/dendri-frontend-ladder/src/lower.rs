//! Lower Ladder series rungs to ASL setup/loop statements.

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::Stmt;
use dendri_ast::{
    DataEntry, HardwareEntry, LadderBlock, LadderCell, Metadata, Program, TimerFbKind,
};

/// Lower a ladder block into an executable ASL program body.
pub fn lower_ladder_to_program(
    block: &LadderBlock,
    hardware: Vec<HardwareEntry>,
    data: Vec<DataEntry>,
    setup: Vec<Stmt>,
) -> Program {
    let mut loop_body = Vec::new();
    for rung in &block.rungs {
        loop_body.extend(lower_rung(&rung.cells));
    }
    Program {
        metadata: Metadata {
            project_name: Some(format!("{}-asl", block.name)),
            editor: Some("dendriForge".into()),
            author: None,
            notes: Some("lowered from ladder".into()),
            expect_error: None,
        },
        hardware,
        data,
        setup,
        loop_body,
        functions: vec![],
        ladder: vec![block.clone()],
        grafcet: vec![],
    }
}

fn lower_rung(cells: &[LadderCell]) -> Vec<Stmt> {
    if cells.is_empty() {
        return vec![];
    }
    // Pure action rung with no contacts/coils: execute unconditionally.
    let only_actions = cells.iter().all(|c| matches!(c, LadderCell::Action { .. } | LadderCell::Wire { .. } | LadderCell::Empty { .. }));
    if only_actions {
        let mut out = Vec::new();
        for c in cells {
            if let LadderCell::Action { stmts, .. } = c {
                out.extend(stmts.clone());
            }
        }
        return out;
    }

    let power = power_expr(cells);
    let mut out = Vec::new();
    for cell in cells {
        match cell {
            LadderCell::Coil { operand, .. } => {
                out.push(Stmt::Assign {
                    target: operand.clone(),
                    value: power.clone(),
                });
            }
            LadderCell::CoilSet { operand, .. } => {
                out.push(Stmt::If {
                    condition: power.clone(),
                    then: vec![Stmt::Assign {
                        target: operand.clone(),
                        value: Expr::Bool { value: true },
                    }],
                    elif: vec![],
                    else_body: None,
                });
            }
            LadderCell::CoilReset { operand, .. } => {
                out.push(Stmt::If {
                    condition: power.clone(),
                    then: vec![Stmt::Assign {
                        target: operand.clone(),
                        value: Expr::Bool { value: false },
                    }],
                    elif: vec![],
                    else_body: None,
                });
            }
            LadderCell::Action { stmts, .. } => {
                out.push(Stmt::If {
                    condition: power.clone(),
                    then: stmts.clone(),
                    elif: vec![],
                    else_body: None,
                });
            }
            LadderCell::FbTimer {
                fb: TimerFbKind::Ton,
                instance,
                args,
                ..
            } => {
                // TON(instance, IN=power, PT=args[0]?)
                let mut ton_args = vec![
                    Expr::String {
                        value: instance.clone(),
                    },
                    power.clone(),
                ];
                if let Some(pt) = args.first() {
                    ton_args.push(pt.clone());
                }
                out.push(Stmt::Call {
                    name: "TON".into(),
                    args: ton_args,
                });
            }
            LadderCell::ContactNo { .. }
            | LadderCell::ContactNc { .. }
            | LadderCell::Wire { .. }
            | LadderCell::Empty { .. }
            | LadderCell::FbTimer { .. }
            | LadderCell::FbCounter { .. }
            | LadderCell::FbCompare { .. } => {}
        }
    }
    out
}

/// Series AND of contact conditions (power rail true when empty of contacts).
fn power_expr(cells: &[LadderCell]) -> Expr {
    let mut parts: Vec<Expr> = Vec::new();
    for cell in cells {
        match cell {
            LadderCell::ContactNo { operand, .. } => {
                parts.push(Expr::Var {
                    name: operand.clone(),
                });
            }
            LadderCell::ContactNc { operand, .. } => {
                parts.push(Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(Expr::Var {
                        name: operand.clone(),
                    }),
                });
            }
            LadderCell::FbCompare {
                op, left, right, ..
            } => {
                let bin = match op.as_str() {
                    "eq" | "==" | "=" => BinaryOp::Eq,
                    "ne" | "!=" | "<>" => BinaryOp::Ne,
                    "lt" | "<" => BinaryOp::Lt,
                    "le" | "<=" => BinaryOp::Le,
                    "gt" | ">" => BinaryOp::Gt,
                    "ge" | ">=" => BinaryOp::Ge,
                    _ => BinaryOp::Eq,
                };
                parts.push(Expr::Binop {
                    op: bin,
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                });
            }
            _ => {}
        }
    }
    match parts.len() {
        0 => Expr::Bool { value: true },
        1 => parts.pop().unwrap(),
        _ => {
            let mut acc = parts.remove(0);
            for p in parts {
                acc = Expr::Binop {
                    op: BinaryOp::And,
                    left: Box::new(acc),
                    right: Box::new(p),
                };
            }
            acc
        }
    }
}
