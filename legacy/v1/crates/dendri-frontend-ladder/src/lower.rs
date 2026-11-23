//! Lower Ladder rungs to ASL setup/loop statements.
//!
//! Series power flows left-to-right. [`LadderCell::Parallel`] is an OR of nested
//! series branches. IEC timer/counter/edge FBs lower to the same call shapes
//! `dendri-sim` already runs (DEN-11).

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::Stmt;
use dendri_ast::{
    CounterFbKind, DataEntry, HardwareEntry, LadderBlock, LadderCell, Metadata, Program,
    TimerFbKind,
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
            asl_version: Some(dendri_ast::ASL_VERSION.into()),
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
    // Pure action rung with no contacts/coils/FBs: execute unconditionally.
    let only_actions = cells.iter().all(|c| {
        matches!(
            c,
            LadderCell::Action { .. } | LadderCell::Wire { .. } | LadderCell::Empty { .. }
        )
    });
    if only_actions {
        let mut out = Vec::new();
        for c in cells {
            if let LadderCell::Action { stmts, .. } = c {
                out.extend(stmts.clone());
            }
        }
        return out;
    }

    let mut out = Vec::new();
    let _ = lower_series(cells, Expr::Bool { value: true }, &mut out);
    out
}

/// Left-to-right series scan. Returns the power leaving the right end of `cells`.
fn lower_series(cells: &[LadderCell], mut power: Expr, out: &mut Vec<Stmt>) -> Expr {
    for cell in cells {
        match cell {
            LadderCell::ContactNo { operand, .. } => {
                power = and_expr(
                    power,
                    Expr::Var {
                        name: operand.clone(),
                    },
                );
            }
            LadderCell::ContactNc { operand, .. } => {
                power = and_expr(
                    power,
                    Expr::Unary {
                        op: UnaryOp::Not,
                        expr: Box::new(Expr::Var {
                            name: operand.clone(),
                        }),
                    },
                );
            }
            LadderCell::FbCompare {
                op, left, right, ..
            } => {
                power = and_expr(power, compare_expr(op, left, right));
            }
            LadderCell::Parallel { branches, .. } => {
                if branches.is_empty() {
                    power = Expr::Bool { value: false };
                } else {
                    let mut branch_outs = Vec::with_capacity(branches.len());
                    for branch in branches {
                        let bp = lower_series(&branch.cells, power.clone(), out);
                        branch_outs.push(bp);
                    }
                    power = or_all(branch_outs);
                }
            }
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
                fb,
                instance,
                args,
                ..
            } => {
                out.push(lower_timer(*fb, instance, &power, args));
            }
            LadderCell::FbCounter {
                fb,
                instance,
                args,
                ..
            } => {
                out.push(lower_counter(*fb, instance, &power, args));
            }
            LadderCell::FbEdge {
                rising,
                instance,
                operand,
                ..
            } => {
                let clk = match operand {
                    Some(name) => Expr::Var { name: name.clone() },
                    None => power.clone(),
                };
                let name = if *rising { "R_TRIG" } else { "F_TRIG" };
                out.push(Stmt::Call {
                    name: name.into(),
                    args: vec![
                        Expr::String {
                            value: instance.clone(),
                        },
                        clk,
                    ],
                });
            }
            LadderCell::Wire { .. } | LadderCell::Empty { .. } => {}
        }
    }
    power
}

fn lower_timer(fb: TimerFbKind, instance: &str, power: &Expr, args: &[Expr]) -> Stmt {
    let name = match fb {
        TimerFbKind::Ton => "TON",
        TimerFbKind::Tof => "TOF",
        TimerFbKind::Tp => "TP",
    };
    let mut ton_args = vec![
        Expr::String {
            value: instance.into(),
        },
        power.clone(),
    ];
    if let Some(pt) = args.first() {
        ton_args.push(pt.clone());
    } else {
        ton_args.push(Expr::Uint { value: 0 });
    }
    Stmt::Call {
        name: name.into(),
        args: ton_args,
    }
}

fn lower_counter(fb: CounterFbKind, instance: &str, power: &Expr, args: &[Expr]) -> Stmt {
    let bool_false = Expr::Bool { value: false };
    let zero = Expr::Int { value: 0 };
    match fb {
        CounterFbKind::Ctu => {
            let r = args.first().cloned().unwrap_or_else(|| bool_false.clone());
            let pv = args.get(1).cloned().unwrap_or_else(|| zero.clone());
            Stmt::Call {
                name: "CTU".into(),
                args: vec![
                    Expr::String {
                        value: instance.into(),
                    },
                    power.clone(),
                    r,
                    pv,
                ],
            }
        }
        CounterFbKind::Ctd => {
            let ld = args.first().cloned().unwrap_or_else(|| bool_false.clone());
            let pv = args.get(1).cloned().unwrap_or_else(|| zero.clone());
            Stmt::Call {
                name: "CTD".into(),
                args: vec![
                    Expr::String {
                        value: instance.into(),
                    },
                    power.clone(),
                    ld,
                    pv,
                ],
            }
        }
        CounterFbKind::Ctud => {
            let cd = args.first().cloned().unwrap_or_else(|| bool_false.clone());
            let r = args.get(1).cloned().unwrap_or_else(|| bool_false.clone());
            let ld = args.get(2).cloned().unwrap_or_else(|| bool_false.clone());
            let pv = args.get(3).cloned().unwrap_or_else(|| zero.clone());
            Stmt::Call {
                name: "CTUD".into(),
                args: vec![
                    Expr::String {
                        value: instance.into(),
                    },
                    power.clone(),
                    cd,
                    r,
                    ld,
                    pv,
                ],
            }
        }
    }
}

fn compare_expr(op: &str, left: &Expr, right: &Expr) -> Expr {
    let bin = match op {
        "eq" | "==" | "=" => BinaryOp::Eq,
        "ne" | "!=" | "<>" => BinaryOp::Ne,
        "lt" | "<" => BinaryOp::Lt,
        "le" | "<=" => BinaryOp::Le,
        "gt" | ">" => BinaryOp::Gt,
        "ge" | ">=" => BinaryOp::Ge,
        _ => BinaryOp::Eq,
    };
    Expr::Binop {
        op: bin,
        left: Box::new(left.clone()),
        right: Box::new(right.clone()),
    }
}

fn and_expr(left: Expr, right: Expr) -> Expr {
    match left {
        Expr::Bool { value: true } => right,
        other => Expr::Binop {
            op: BinaryOp::And,
            left: Box::new(other),
            right: Box::new(right),
        },
    }
}

fn or_all(parts: Vec<Expr>) -> Expr {
    match parts.len() {
        0 => Expr::Bool { value: false },
        1 => parts.into_iter().next().unwrap(),
        _ => {
            let mut iter = parts.into_iter();
            let mut acc = iter.next().unwrap();
            for p in iter {
                acc = Expr::Binop {
                    op: BinaryOp::Or,
                    left: Box::new(acc),
                    right: Box::new(p),
                };
            }
            acc
        }
    }
}
