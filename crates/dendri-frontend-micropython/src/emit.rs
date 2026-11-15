//! ASL AST → MicroPython (MVP subset).

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::Stmt;
use dendri_ast::{DataType, Program};
use std::collections::HashMap;

/// Emit MicroPython from an ASL program AST.
pub fn emit_micropython(program: &Program) -> String {
    let pin_labels: HashMap<u32, String> = program
        .hardware
        .iter()
        .map(|h| (h.pin, h.label.clone()))
        .collect();

    let mut out = String::new();
    out.push_str("# DendriForge MicroPython frontend — MVP subset\n");
    if let Some(name) = &program.metadata.project_name {
        out.push_str(&format!("# project: {name}\n"));
    }
    out.push_str("from machine import Pin\n");
    out.push_str("from time import sleep_ms\n\n");

    for h in &program.hardware {
        out.push_str(&format!("{} = {}\n", to_upper_snake(&h.label), h.pin));
    }
    if !program.hardware.is_empty() {
        out.push('\n');
    }

    for d in &program.data {
        let val = mpy_init(d.data_type, &d.init);
        out.push_str(&format!("{} = {val}\n", d.name));
    }
    if !program.data.is_empty() {
        out.push('\n');
    }

    // Pin objects declared None, assigned in setup.
    for h in &program.hardware {
        out.push_str(&format!("{} = None\n", pin_obj_name(&h.label)));
    }
    if !program.hardware.is_empty() {
        out.push('\n');
    }

    out.push_str("def setup():\n");
    if program.hardware.is_empty() && program.setup.is_empty() {
        out.push_str("    pass\n");
    } else {
        for h in &program.hardware {
            out.push_str(&format!("    global {}\n", pin_obj_name(&h.label)));
        }
        for stmt in &program.setup {
            emit_stmt(stmt, &mut out, 1, &pin_labels, program);
        }
    }
    out.push('\n');

    out.push_str("def loop():\n");
    if program.loop_body.is_empty() {
        out.push_str("    pass\n");
    } else {
        // globals for mutated vars
        let mut mutated = Vec::new();
        collect_assigns(&program.loop_body, &mut mutated);
        for name in &mutated {
            out.push_str(&format!("    global {name}\n"));
        }
        for stmt in &program.loop_body {
            emit_stmt(stmt, &mut out, 1, &pin_labels, program);
        }
    }

    for f in &program.functions {
        let params = f.params.join(", ");
        out.push_str(&format!("\ndef {}({params}):\n", to_lower_snake(&f.name)));
        if f.body.is_empty() {
            out.push_str("    pass\n");
        } else {
            for stmt in &f.body {
                emit_stmt(stmt, &mut out, 1, &pin_labels, program);
            }
        }
    }

    out.push_str("\nsetup()\n");
    out.push_str("while True:\n");
    out.push_str("    loop()\n");
    out
}

fn to_upper_snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    if out.is_empty() {
        "PIN".into()
    } else {
        out
    }
}

fn to_lower_snake(s: &str) -> String {
    to_upper_snake(s).to_ascii_lowercase()
}

fn pin_obj_name(label: &str) -> String {
    format!("{}_pin", to_lower_snake(label).trim_end_matches("_pin"))
}

fn mpy_init(ty: DataType, init: &str) -> String {
    match ty {
        DataType::Bool => {
            if init.eq_ignore_ascii_case("true") {
                "True".into()
            } else {
                "False".into()
            }
        }
        DataType::Time => parse_time_ms(init)
            .map(|ms| ms.to_string())
            .unwrap_or_else(|| "0".into()),
        DataType::String => {
            if init.starts_with('"') {
                init.to_string()
            } else {
                format!("\"{init}\"")
            }
        }
        _ => init.to_string(),
    }
}

fn parse_time_ms(init: &str) -> Option<u64> {
    let s = init.trim();
    let s = s.strip_prefix("T#").or_else(|| s.strip_prefix("t#"))?;
    if let Some(n) = s.strip_suffix("ms") {
        return n.parse().ok();
    }
    if let Some(n) = s.strip_suffix('s') {
        let v: u64 = n.parse().ok()?;
        return Some(v.saturating_mul(1000));
    }
    s.parse().ok()
}

fn collect_assigns(stmts: &[Stmt], out: &mut Vec<String>) {
    for s in stmts {
        match s {
            Stmt::Assign { target, .. } => {
                if !out.contains(target) {
                    out.push(target.clone());
                }
            }
            Stmt::If {
                then,
                elif,
                else_body,
                ..
            } => {
                collect_assigns(then, out);
                for a in elif {
                    collect_assigns(&a.then, out);
                }
                if let Some(b) = else_body {
                    collect_assigns(b, out);
                }
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } => collect_assigns(body, out),
            _ => {}
        }
    }
}

fn resolve_pin_const(expr: &Expr, labels: &HashMap<u32, String>) -> String {
    match expr {
        Expr::Uint { value } => {
            if let Some(l) = labels.get(&(*value as u32)) {
                to_upper_snake(l)
            } else {
                value.to_string()
            }
        }
        other => emit_expr(other, labels),
    }
}

fn resolve_pin_obj(expr: &Expr, labels: &HashMap<u32, String>) -> String {
    match expr {
        Expr::Uint { value } => {
            if let Some(l) = labels.get(&(*value as u32)) {
                pin_obj_name(l)
            } else {
                format!("pin_{value}")
            }
        }
        Expr::Var { name } => pin_obj_name(name),
        other => emit_expr(other, labels),
    }
}

fn emit_stmt(
    stmt: &Stmt,
    out: &mut String,
    indent: usize,
    labels: &HashMap<u32, String>,
    program: &Program,
) {
    let pad = "    ".repeat(indent);
    match stmt {
        Stmt::Assign { target, value } => {
            // TON and similar stay as calls on RHS
            out.push_str(&format!(
                "{pad}{target} = {}\n",
                emit_expr(value, labels)
            ));
        }
        Stmt::Call { name, args } => {
            match name.as_str() {
                "CFG_DIGITAL_OUTPUT" | "CFG_DIGITAL_INPUT" | "CFG_DIGITAL_INPUT_PULLUP" => {
                    let mode = match name.as_str() {
                        "CFG_DIGITAL_INPUT" => "Pin.IN",
                        "CFG_DIGITAL_INPUT_PULLUP" => "Pin.IN, Pin.PULL_UP",
                        _ => "Pin.OUT",
                    };
                    let obj = resolve_pin_obj(&args[0], labels);
                    let pin = resolve_pin_const(&args[0], labels);
                    out.push_str(&format!("{pad}{obj} = Pin({pin}, {mode})\n"));
                }
                "SET_BOOL" => {
                    let obj = resolve_pin_obj(&args[0], labels);
                    let val = match args.get(1) {
                        Some(Expr::Bool { value: true }) => "1".into(),
                        Some(Expr::Bool { value: false }) => "0".into(),
                        Some(e) => emit_expr(e, labels),
                        None => "0".into(),
                    };
                    out.push_str(&format!("{pad}{obj}.value({val})\n"));
                }
                "WAIT_TIME" => {
                    let a = args
                        .first()
                        .map(|e| emit_expr(e, labels))
                        .unwrap_or_else(|| "0".into());
                    out.push_str(&format!("{pad}sleep_ms({a})\n"));
                }
                "WAIT_TIME_US" => {
                    let a = args
                        .first()
                        .map(|e| emit_expr(e, labels))
                        .unwrap_or_else(|| "0".into());
                    out.push_str(&format!("{pad}sleep_us({a})\n"));
                }
                other => {
                    let a = args
                        .iter()
                        .map(|e| emit_expr(e, labels))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!("{pad}{other}({a})\n"));
                }
            }
            let _ = program;
        }
        Stmt::If {
            condition,
            then,
            elif,
            else_body,
        } => {
            out.push_str(&format!("{pad}if {}:\n", emit_expr(condition, labels)));
            if then.is_empty() {
                out.push_str(&format!("{pad}    pass\n"));
            } else {
                for s in then {
                    emit_stmt(s, out, indent + 1, labels, program);
                }
            }
            for arm in elif {
                out.push_str(&format!(
                    "{pad}elif {}:\n",
                    emit_expr(&arm.condition, labels)
                ));
                if arm.then.is_empty() {
                    out.push_str(&format!("{pad}    pass\n"));
                } else {
                    for s in &arm.then {
                        emit_stmt(s, out, indent + 1, labels, program);
                    }
                }
            }
            if let Some(body) = else_body {
                out.push_str(&format!("{pad}else:\n"));
                if body.is_empty() {
                    out.push_str(&format!("{pad}    pass\n"));
                } else {
                    for s in body {
                        emit_stmt(s, out, indent + 1, labels, program);
                    }
                }
            }
        }
        Stmt::While { condition, body } => {
            out.push_str(&format!("{pad}while {}:\n", emit_expr(condition, labels)));
            if body.is_empty() {
                out.push_str(&format!("{pad}    pass\n"));
            } else {
                for s in body {
                    emit_stmt(s, out, indent + 1, labels, program);
                }
            }
        }
        Stmt::For { counter, to, body } => {
            let c = counter.as_deref().unwrap_or("_i");
            out.push_str(&format!(
                "{pad}for {c} in range({}):\n",
                emit_expr(to, labels)
            ));
            if body.is_empty() {
                out.push_str(&format!("{pad}    pass\n"));
            } else {
                for s in body {
                    emit_stmt(s, out, indent + 1, labels, program);
                }
            }
        }
        Stmt::Return { value } => {
            if let Some(v) = value {
                out.push_str(&format!("{pad}return {}\n", emit_expr(v, labels)));
            } else {
                out.push_str(&format!("{pad}return\n"));
            }
        }
        Stmt::Exit => out.push_str(&format!("{pad}break\n")),
        Stmt::Continue => out.push_str(&format!("{pad}continue\n")),
        Stmt::VendorExtension { reason } => {
            out.push_str(&format!("{pad}# VENDOR_EXTENSION: {reason}\n"));
        }
    }
}

fn emit_expr(expr: &Expr, labels: &HashMap<u32, String>) -> String {
    match expr {
        Expr::Bool { value } => {
            if *value {
                "True".into()
            } else {
                "False".into()
            }
        }
        Expr::Uint { value } => value.to_string(),
        Expr::Int { value } => value.to_string(),
        Expr::Real { value } => format!("{value}"),
        Expr::String { value } => format!("\"{value}\""),
        Expr::Time { value } => parse_time_ms(value)
            .map(|ms| ms.to_string())
            .unwrap_or_else(|| value.clone()),
        Expr::Hex { value } => value.clone(),
        Expr::Var { name } => name.clone(),
        Expr::Lit { value } => format!("{value:?}"),
        Expr::Call { name, args } => match name.as_str() {
            "GET_BOOL" => format!("{}.value()", resolve_pin_obj(&args[0], labels)),
            "GET_SYS_TIME" => "ticks_ms()".into(),
            other => {
                let a = args
                    .iter()
                    .map(|e| emit_expr(e, labels))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{other}({a})")
            }
        },
        Expr::Unary { op, expr } => {
            let inner = emit_expr(expr, labels);
            match op {
                UnaryOp::Not => format!("(not {inner})"),
                UnaryOp::Neg => format!("-({inner})"),
                UnaryOp::BitNot => format!("(~{inner})"),
            }
        }
        Expr::Binop { op, left, right } => {
            let l = emit_expr(left, labels);
            let r = emit_expr(right, labels);
            let o = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "%",
                BinaryOp::Eq => "==",
                BinaryOp::Ne => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::And => "and",
                BinaryOp::Or => "or",
                BinaryOp::Xor => "^",
                BinaryOp::BitAnd => "&",
                BinaryOp::BitOr => "|",
                BinaryOp::BitXor => "^",
                BinaryOp::Shl => "<<",
                BinaryOp::Shr => ">>",
            };
            format!("({l} {o} {r})")
        }
        Expr::ExprText { text } => text.clone(),
    }
}
