//! AST → IEC 61131-3 Structured Text (MVP subset).

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::Stmt;
use dendri_ast::{DataType, Program, Scope};
use dendri_parse::parse_expr_text;

/// Emit Structured Text text from an ASL program AST.
pub fn emit_st(program: &Program) -> String {
    let mut out = String::new();
    out.push_str("(* DendriForge ST frontend — MVP subset *)\n");
    if let Some(name) = &program.metadata.project_name {
        out.push_str(&format!("(* project: {name} *)\n"));
    }
    out.push_str("PROGRAM MAIN\n");

    if !program.hardware.is_empty() {
        out.push_str("(* hardware *)\n");
        for h in &program.hardware {
            out.push_str(&format!(
                "(* HW: pin={} type={:?} mode={:?} label={} role={} *)\n",
                h.pin, h.hw_type, h.io_mode, h.label, h.target_role
            ));
        }
    }

    out.push_str("VAR\n");
    for d in &program.data {
        let st_ty = d.data_type.as_str();
        let init = st_init(&d.init, d.data_type);
        let retain = match d.scope {
            Scope::Retain | Scope::Persistent => " RETAIN",
            _ => "",
        };
        if d.scope == Scope::Const {
            // CONSTANT keyword is Dendri MVP surface (IEC uses CONSTANT separately).
            out.push_str(&format!(
                "    CONSTANT {name} : {st_ty} := {init};\n",
                name = d.name
            ));
        } else {
            out.push_str(&format!(
                "    {name}{retain} : {st_ty} := {init};\n",
                name = d.name
            ));
        }
    }
    out.push_str("END_VAR\n\n");

    out.push_str("SETUP\n");
    for stmt in &program.setup {
        emit_stmt(stmt, &mut out, 1);
    }
    out.push_str("\nLOOP\n");
    for stmt in &program.loop_body {
        emit_stmt(stmt, &mut out, 1);
    }

    for f in &program.functions {
        out.push_str(&format!("\nFUNCTION {} : BOOL\n", f.name));
        out.push_str("VAR_INPUT\n");
        for p in &f.params {
            out.push_str(&format!("    {p} : BOOL;\n"));
        }
        out.push_str("END_VAR\n");
        for stmt in &f.body {
            emit_stmt(stmt, &mut out, 0);
        }
        out.push_str("END_FUNCTION\n");
    }

    out.push_str("END_PROGRAM\n");
    out
}

fn st_init(init: &str, ty: DataType) -> String {
    match ty {
        DataType::Bool => {
            if init.eq_ignore_ascii_case("true") || init == "TRUE" {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        DataType::Time => {
            if init.starts_with('T') || init.starts_with('t') {
                init.to_string()
            } else {
                format!("T#{init}")
            }
        }
        DataType::String => {
            let inner = init.trim().trim_matches('"').trim_matches('\'');
            format!("'{inner}'")
        }
        _ => init.to_string(),
    }
}

fn emit_stmt(stmt: &Stmt, out: &mut String, indent: usize) {
    let pad = "    ".repeat(indent);
    match stmt {
        Stmt::Assign { target, value } => {
            out.push_str(&format!("{pad}{target} := {};\n", emit_expr(value)));
        }
        Stmt::Call { name, args } => {
            let args_s = args.iter().map(emit_expr).collect::<Vec<_>>().join(", ");
            out.push_str(&format!("{pad}{name}({args_s});\n"));
        }
        Stmt::If {
            condition,
            then,
            elif,
            else_body,
        } => {
            out.push_str(&format!("{pad}IF {} THEN\n", emit_expr(condition)));
            for s in then {
                emit_stmt(s, out, indent + 1);
            }
            for arm in elif {
                out.push_str(&format!("{pad}ELSIF {} THEN\n", emit_expr(&arm.condition)));
                for s in &arm.then {
                    emit_stmt(s, out, indent + 1);
                }
            }
            if let Some(body) = else_body {
                out.push_str(&format!("{pad}ELSE\n"));
                for s in body {
                    emit_stmt(s, out, indent + 1);
                }
            }
            out.push_str(&format!("{pad}END_IF;\n"));
        }
        Stmt::While { condition, body } => {
            out.push_str(&format!("{pad}WHILE {} DO\n", emit_expr(condition)));
            for s in body {
                emit_stmt(s, out, indent + 1);
            }
            out.push_str(&format!("{pad}END_WHILE;\n"));
        }
        Stmt::For { counter, to, body } => {
            let c = counter.as_deref().unwrap_or("_i");
            out.push_str(&format!("{pad}FOR {c} := 0 TO {} - 1 DO\n", emit_expr(to)));
            for s in body {
                emit_stmt(s, out, indent + 1);
            }
            out.push_str(&format!("{pad}END_FOR;\n"));
        }
        Stmt::Return { value } => {
            if let Some(v) = value {
                out.push_str(&format!("{pad}RETURN_VALUE := {};\n", emit_expr(v)));
            }
            out.push_str(&format!("{pad}RETURN;\n"));
        }
        Stmt::Exit => out.push_str(&format!("{pad}EXIT;\n")),
        Stmt::Continue => out.push_str(&format!("{pad}(* CONTINUE *)\n")),
        Stmt::VendorExtension { reason } => {
            out.push_str(&format!("{pad}(* VENDOR_EXTENSION: {reason} *)\n"));
        }
    }
}

fn emit_expr(expr: &Expr) -> String {
    match expr {
        Expr::Bool { value } => {
            if *value {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        Expr::Uint { value } => value.to_string(),
        Expr::Int { value } => value.to_string(),
        Expr::Real { value } => format!("{value}"),
        Expr::String { value } => format!("'{value}'"),
        Expr::Time { value } => value.clone(),
        Expr::Hex { value } => value.clone(),
        Expr::Var { name } => name.clone(),
        Expr::Lit { value } => format!("{value:?}"),
        Expr::Call { name, args } => {
            let a = args.iter().map(emit_expr).collect::<Vec<_>>().join(", ");
            format!("{name}({a})")
        }
        Expr::Unary { op, expr } => {
            let inner = emit_expr(expr);
            match op {
                UnaryOp::Not => format!("NOT ({inner})"),
                UnaryOp::Neg => format!("-({inner})"),
                UnaryOp::BitNot => format!("NOT ({inner})"),
            }
        }
        Expr::Binop { op, left, right } => {
            let l = emit_expr(left);
            let r = emit_expr(right);
            let o = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "MOD",
                BinaryOp::Eq => "=",
                BinaryOp::Ne => "<>",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::And => "AND",
                BinaryOp::Or => "OR",
                BinaryOp::Xor => "XOR",
                BinaryOp::BitAnd => "AND",
                BinaryOp::BitOr => "OR",
                BinaryOp::BitXor => "XOR",
                BinaryOp::Shl => "SHL",
                BinaryOp::Shr => "SHR",
            };
            if matches!(op, BinaryOp::Mod | BinaryOp::Shl | BinaryOp::Shr) {
                format!("{o}({l}, {r})")
            } else {
                format!("({l} {o} {r})")
            }
        }
        Expr::ExprText { text } => match parse_expr_text(text) {
            Ok(e) => emit_expr(&e),
            Err(_) => format!("(* expr_text: {text} *)"),
        },
    }
}
