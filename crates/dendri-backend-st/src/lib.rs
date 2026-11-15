//! AST → IEC 61131-3 Structured Text emit stub.
//!
//! Emits a readable ST sketch from the same [`Program`] AST that `dendri-sim` runs.
//! This is intentionally incomplete (P1 frontend/backends continue in DEN-7) but
//! proves the co-design rule: one AST → validate / sim / emit.

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::Stmt;
use dendri_ast::{DataType, Program, Scope};
use dendri_parse::parse_expr_text;

/// Emit Structured Text text from an ASL program AST.
pub fn emit_st(program: &Program) -> String {
    let mut out = String::new();
    out.push_str("(* DendriForge ST emit stub — not a complete IEC toolchain *)\n");
    if let Some(name) = &program.metadata.project_name {
        out.push_str(&format!("(* project: {name} *)\n"));
    }
    out.push_str("PROGRAM MAIN\n");
    out.push_str("VAR\n");
    for d in &program.data {
        let st_ty = data_type_st(d.data_type);
        let init = st_init(&d.init, d.data_type);
        let retain = match d.scope {
            Scope::Retain | Scope::Persistent => " RETAIN",
            Scope::Const => "", // CONST → CONSTANT below
            _ => "",
        };
        if d.scope == Scope::Const {
            out.push_str(&format!("    {name} : {st_ty} := {init}; (* CONSTANT *)\n", name = d.name));
        } else {
            out.push_str(&format!(
                "    {name}{retain} : {st_ty} := {init};\n",
                name = d.name
            ));
        }
    }
    out.push_str("END_VAR\n\n");

    out.push_str("(* --- setup (cold start) --- *)\n");
    for stmt in &program.setup {
        emit_stmt(stmt, &mut out, 0);
    }
    out.push_str("\n(* --- scan cycle / loop --- *)\n");
    for stmt in &program.loop_body {
        emit_stmt(stmt, &mut out, 0);
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

fn data_type_st(t: DataType) -> &'static str {
    t.as_str()
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
                out.push_str(&format!("{pad}{} := {};\n", "RETURN_VALUE", emit_expr(v)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{Metadata, Stmt};
    use dendri_ast::expr::Expr;

    #[test]
    fn emits_set_bool() {
        let p = Program {
            metadata: Metadata {
                project_name: Some("stub".into()),
                editor: None,
                author: None,
                notes: None,
                expect_error: None,
            },
            hardware: vec![],
            data: vec![],
            setup: vec![Stmt::Call {
                name: "SET_BOOL".into(),
                args: vec![Expr::Uint { value: 13 }, Expr::Bool { value: true }],
            }],
            loop_body: vec![],
            functions: vec![],
            ladder: vec![],
            grafcet: vec![],
        };
        let st = emit_st(&p);
        assert!(st.contains("SET_BOOL(13, TRUE)"));
        assert!(st.contains("PROGRAM MAIN"));
    }
}
