//! Canonicalize `Expr::ExprText` into typed expr trees for stable round-trips.
//! Also normalizes pin uint literals to hardware label vars in pin-arg slots.

use std::collections::HashMap;

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

fn pin_indices(op: &str) -> Option<&'static [usize]> {
    match op {
        "CFG_DIGITAL_OUTPUT"
        | "CFG_DIGITAL_INPUT"
        | "CFG_DIGITAL_INPUT_PULLUP"
        | "CFG_DIGITAL_INPUT_PULLDOWN"
        | "SET_BOOL"
        | "GET_BOOL"
        | "CFG_PWM_OUTPUT"
        | "SET_PWM_DUTY"
        | "SET_PWM_FREQ"
        | "GET_PWM_DUTY"
        | "CFG_ADC_INPUT"
        | "READ_ADC"
        | "CFG_ATTACH_INTERRUPT"
        | "CFG_DETACH_INTERRUPT"
        | "PLAY_TONE" => Some(&[0]),
        "CFG_I2C" | "CFG_SENSOR_I2C" => Some(&[1, 2]),
        "CFG_SPI" => Some(&[1, 2, 3, 4]),
        "CFG_PLANT_TANK" => Some(&[1, 2]),
        _ => None,
    }
}

fn rewrite_pin_expr(expr: Expr, pin_map: &HashMap<u32, String>) -> Expr {
    match expr {
        Expr::Uint { value } => {
            if let Some(label) = pin_map.get(&(value as u32)) {
                Expr::Var {
                    name: label.clone(),
                }
            } else {
                Expr::Uint { value }
            }
        }
        other => other,
    }
}

/// Resolve every `ExprText` node via [`parse_expr_text`], recursively.
/// When `pin_map` is set, also rewrite pin-slot uints to hardware labels.
pub fn canonicalize_expr(expr: &Expr) -> Expr {
    canonicalize_expr_with(expr, None)
}

fn canonicalize_expr_with(expr: &Expr, pin_map: Option<&HashMap<u32, String>>) -> Expr {
    match expr {
        Expr::ExprText { text } => match parse_expr_text(text) {
            Ok(inner) => canonicalize_expr_with(&inner, pin_map),
            Err(_) => expr.clone(),
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op: *op,
            expr: Box::new(canonicalize_expr_with(expr, pin_map)),
        },
        Expr::Binop { op, left, right } => Expr::Binop {
            op: *op,
            left: Box::new(canonicalize_expr_with(left, pin_map)),
            right: Box::new(canonicalize_expr_with(right, pin_map)),
        },
        Expr::Call { name, args } => {
            let indices = pin_indices(name);
            let args: Vec<Expr> = args
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let mut e = canonicalize_expr_with(a, pin_map);
                    if let (Some(map), Some(idxs)) = (pin_map, indices) {
                        if idxs.contains(&i) {
                            e = rewrite_pin_expr(e, map);
                        }
                    }
                    e
                })
                .collect();
            Expr::Call {
                name: name.clone(),
                args,
            }
        }
        other => other.clone(),
    }
}

fn canonicalize_stmt(stmt: &Stmt, pin_map: Option<&HashMap<u32, String>>) -> Stmt {
    match stmt {
        Stmt::Assign { target, value } => Stmt::Assign {
            target: target.clone(),
            value: canonicalize_expr_with(value, pin_map),
        },
        Stmt::Call { name, args } => {
            let indices = pin_indices(name);
            let args: Vec<Expr> = args
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let mut e = canonicalize_expr_with(a, pin_map);
                    if let (Some(map), Some(idxs)) = (pin_map, indices) {
                        if idxs.contains(&i) {
                            e = rewrite_pin_expr(e, map);
                        }
                    }
                    e
                })
                .collect();
            Stmt::Call {
                name: name.clone(),
                args,
            }
        }
        Stmt::If {
            condition,
            then,
            elif,
            else_body,
        } => Stmt::If {
            condition: canonicalize_expr_with(condition, pin_map),
            then: then
                .iter()
                .map(|s| canonicalize_stmt(s, pin_map))
                .collect(),
            elif: elif
                .iter()
                .map(|a| ElifArm {
                    condition: canonicalize_expr_with(&a.condition, pin_map),
                    then: a
                        .then
                        .iter()
                        .map(|s| canonicalize_stmt(s, pin_map))
                        .collect(),
                })
                .collect(),
            else_body: else_body
                .as_ref()
                .map(|b| b.iter().map(|s| canonicalize_stmt(s, pin_map)).collect()),
        },
        Stmt::While { condition, body } => Stmt::While {
            condition: canonicalize_expr_with(condition, pin_map),
            body: body
                .iter()
                .map(|s| canonicalize_stmt(s, pin_map))
                .collect(),
        },
        Stmt::For { counter, to, body } => Stmt::For {
            counter: counter.clone(),
            to: canonicalize_expr_with(to, pin_map),
            body: body
                .iter()
                .map(|s| canonicalize_stmt(s, pin_map))
                .collect(),
        },
        Stmt::Return { value } => Stmt::Return {
            value: value
                .as_ref()
                .map(|v| canonicalize_expr_with(v, pin_map)),
        },
        other => other.clone(),
    }
}

/// Walk setup / loop / function bodies and replace `ExprText` with typed trees.
///
/// Also normalizes STRING data inits (bare vs quoted) and rewrites pin-number
/// uints in pin-arg slots to hardware labels so logical-name ASL and legacy
/// pin-literal ASL compare equal after frontend emit→parse.
pub fn canonicalize_program(program: &Program) -> Program {
    let pin_map: HashMap<u32, String> = program
        .hardware
        .iter()
        .map(|h| (h.pin, h.label.clone()))
        .collect();
    let pin_ref = if pin_map.is_empty() {
        None
    } else {
        Some(&pin_map)
    };

    let mut out = program.clone();
    for d in &mut out.data {
        if d.data_type == DataType::String {
            d.init = normalize_string_init(&d.init);
        }
    }
    out.setup = out
        .setup
        .iter()
        .map(|s| canonicalize_stmt(s, pin_ref))
        .collect();
    out.loop_body = out
        .loop_body
        .iter()
        .map(|s| canonicalize_stmt(s, pin_ref))
        .collect();
    for f in &mut out.functions {
        f.body = f
            .body
            .iter()
            .map(|s| canonicalize_stmt(s, pin_ref))
            .collect();
    }
    out
}
