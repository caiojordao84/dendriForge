//! Canonicalize `Expr::ExprText` into typed expr trees for stable round-trips.

use dendri_ast::expr::Expr;
use dendri_ast::stmt::{ElifArm, Stmt};
use dendri_ast::{DataType, Program};

use crate::parse_expr_text;

/// Normalize STRING data inits so bare `PING` and quoted `"PING"` compare equal.
fn normalize_string_init(init: &str) -> String {
    let t = init.trim();
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        t.to_string()
    } else {
        format!("\"{t}\"")
    }
}

/// Resolve every `ExprText` node via [`parse_expr_text`], recursively.
pub fn canonicalize_expr(expr: &Expr) -> Expr {
    match expr {
        Expr::ExprText { text } => match parse_expr_text(text) {
            Ok(inner) => canonicalize_expr(&inner),
            Err(_) => expr.clone(),
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op: *op,
            expr: Box::new(canonicalize_expr(expr)),
        },
        Expr::Binop { op, left, right } => Expr::Binop {
            op: *op,
            left: Box::new(canonicalize_expr(left)),
            right: Box::new(canonicalize_expr(right)),
        },
        Expr::Call { name, args } => Expr::Call {
            name: name.clone(),
            args: args.iter().map(canonicalize_expr).collect(),
        },
        other => other.clone(),
    }
}

fn canonicalize_stmt(stmt: &Stmt) -> Stmt {
    match stmt {
        Stmt::Assign { target, value } => Stmt::Assign {
            target: target.clone(),
            value: canonicalize_expr(value),
        },
        Stmt::Call { name, args } => Stmt::Call {
            name: name.clone(),
            args: args.iter().map(canonicalize_expr).collect(),
        },
        Stmt::If {
            condition,
            then,
            elif,
            else_body,
        } => Stmt::If {
            condition: canonicalize_expr(condition),
            then: then.iter().map(canonicalize_stmt).collect(),
            elif: elif
                .iter()
                .map(|a| ElifArm {
                    condition: canonicalize_expr(&a.condition),
                    then: a.then.iter().map(canonicalize_stmt).collect(),
                })
                .collect(),
            else_body: else_body
                .as_ref()
                .map(|b| b.iter().map(canonicalize_stmt).collect()),
        },
        Stmt::While { condition, body } => Stmt::While {
            condition: canonicalize_expr(condition),
            body: body.iter().map(canonicalize_stmt).collect(),
        },
        Stmt::For { counter, to, body } => Stmt::For {
            counter: counter.clone(),
            to: canonicalize_expr(to),
            body: body.iter().map(canonicalize_stmt).collect(),
        },
        Stmt::Return { value } => Stmt::Return {
            value: value.as_ref().map(canonicalize_expr),
        },
        other => other.clone(),
    }
}

/// Walk setup / loop / function bodies and replace `ExprText` with typed trees.
///
/// Also normalizes STRING data inits (bare vs quoted) so migrated goldens and
/// emit→parse results compare equal.
///
/// Used by frontend round-trip so fixtures authored with `expr_text` compare
/// equal to the structured form emit→parse regenerates.
pub fn canonicalize_program(program: &Program) -> Program {
    let mut out = program.clone();
    for d in &mut out.data {
        if d.data_type == DataType::String {
            d.init = normalize_string_init(&d.init);
        }
    }
    out.setup = out.setup.iter().map(canonicalize_stmt).collect();
    out.loop_body = out.loop_body.iter().map(canonicalize_stmt).collect();
    for f in &mut out.functions {
        f.body = f.body.iter().map(canonicalize_stmt).collect();
    }
    out
}
