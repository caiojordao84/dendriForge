//! Canonical lossless expression printer (`01-asl-ir-and-project-schema.md` §4.2).

use dendri_ast::{BinaryOp, Expr, ExprKind, LValue, Literal, UnaryOp};

/// Prints an [`Expr`] in canonical ASL v2 syntax such that
/// `parse_expr(&print_expr(&e)) == Ok(e)` (modulo byte spans).
#[must_use]
pub fn print_expr(expr: &Expr) -> String {
    let mut out = String::new();
    write_expr(expr, 0, &mut out);
    out
}

/// Prints an [`LValue`] in canonical syntax.
#[must_use]
pub fn print_lvalue(lvalue: &LValue) -> String {
    let mut out = String::new();
    write_lvalue(lvalue, &mut out);
    out
}

fn write_lvalue(lvalue: &LValue, out: &mut String) {
    match lvalue {
        LValue::Var { name, .. } => out.push_str(name),
        LValue::Index { base, index, .. } => {
            write_lvalue(base, out);
            out.push('[');
            write_expr(index, 0, out);
            out.push(']');
        }
        LValue::Field { base, field, .. } => {
            write_lvalue(base, out);
            out.push('.');
            out.push_str(field);
        }
    }
}

fn write_expr(expr: &Expr, parent_prec: u8, out: &mut String) {
    let my_prec = expr_prec(&expr.kind);
    let need_parens = my_prec < parent_prec;
    if need_parens {
        out.push('(');
    }

    match &expr.kind {
        ExprKind::Literal(lit) => write_literal(lit, out),
        ExprKind::Ident(name) => out.push_str(name),
        ExprKind::Index { base, index } => {
            let wrap_base = expr_prec(&base.kind) < 110;
            if wrap_base {
                out.push('(');
            }
            write_expr(base, 0, out);
            if wrap_base {
                out.push(')');
            }
            out.push('[');
            write_expr(index, 0, out);
            out.push(']');
        }
        ExprKind::Field { base, field } => {
            let wrap_base = expr_prec(&base.kind) < 110;
            if wrap_base {
                out.push('(');
            }
            write_expr(base, 0, out);
            if wrap_base {
                out.push(')');
            }
            out.push('.');
            out.push_str(field);
        }
        ExprKind::Call { callee, args } => {
            out.push_str(callee);
            out.push('(');
            for (idx, arg) in args.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                if let Some(name) = &arg.name {
                    out.push_str(name);
                    out.push_str(" := ");
                }
                write_expr(&arg.value, 0, out);
            }
            out.push(')');
        }
        ExprKind::Unary { op, operand } => {
            match op {
                UnaryOp::Neg => out.push('-'),
                UnaryOp::Pos => out.push('+'),
                UnaryOp::Not => out.push_str("NOT "),
                UnaryOp::BitNot => out.push_str("BIT_NOT "),
            }
            // Wrap nested unary `-`/`+` or lower-prec expressions cleanly
            let wrap_operand = expr_prec(&operand.kind) < 100
                || matches!(
                    (op, &operand.kind),
                    (
                        UnaryOp::Neg | UnaryOp::Pos,
                        ExprKind::Unary {
                            op: UnaryOp::Neg | UnaryOp::Pos,
                            ..
                        }
                    )
                );
            if wrap_operand {
                out.push('(');
                write_expr(operand, 0, out);
                out.push(')');
            } else {
                write_expr(operand, 100, out);
            }
        }
        ExprKind::Binary { op, left, right } => {
            let prec = binary_prec(*op);
            // Non-chaining relational/equality MUST wrap same-group children on EITHER side
            let wrap_left = needs_binary_child_parens(*op, &left.kind, true);
            if wrap_left {
                out.push('(');
                write_expr(left, 0, out);
                out.push(')');
            } else {
                write_expr(left, prec, out);
            }

            out.push(' ');
            out.push_str(binary_op_str(*op));
            out.push(' ');

            // Right child requires higher precedence (`prec + 1`) because all binary ops are left-assoc
            let wrap_right = needs_binary_child_parens(*op, &right.kind, false);
            if wrap_right {
                out.push('(');
                write_expr(right, 0, out);
                out.push(')');
            } else {
                write_expr(right, prec.saturating_add(1), out);
            }
        }
        ExprKind::Foreign { language, text } => {
            out.push_str("FOREIGN(");
            write_escaped_string(language, out);
            out.push_str(", ");
            write_escaped_string(text, out);
            out.push(')');
        }
    }

    if need_parens {
        out.push(')');
    }
}

fn needs_binary_child_parens(parent: BinaryOp, child: &ExprKind, is_left: bool) -> bool {
    let ExprKind::Binary { op: child_op, .. } = child else {
        return false;
    };
    if parent.is_relational() && child_op.is_relational() {
        return true;
    }
    if parent.is_equality() && child_op.is_equality() {
        return true;
    }
    if !is_left && binary_prec(*child_op) <= binary_prec(parent) {
        return true;
    }
    false
}

fn write_literal(lit: &Literal, out: &mut String) {
    match lit {
        Literal::Bool(true) => out.push_str("true"),
        Literal::Bool(false) => out.push_str("false"),
        Literal::Int(n) => out.push_str(&n.to_string()),
        Literal::Float(f) => {
            let s = format!("{f}");
            out.push_str(&s);
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                out.push_str(".0");
            }
        }
        Literal::TimeUs(us) => {
            out.push_str("T#");
            if *us == 0 {
                out.push_str("0us");
            } else if *us % 1_000_000 == 0 {
                out.push_str(&(*us / 1_000_000).to_string());
                out.push('s');
            } else if *us % 1_000 == 0 {
                out.push_str(&(*us / 1_000).to_string());
                out.push_str("ms");
            } else {
                out.push_str(&us.to_string());
                out.push_str("us");
            }
        }
        Literal::String(s) => write_escaped_string(s, out),
    }
}

fn write_escaped_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

const fn expr_prec(kind: &ExprKind) -> u8 {
    match kind {
        ExprKind::Literal(_)
        | ExprKind::Ident(_)
        | ExprKind::Call { .. }
        | ExprKind::Foreign { .. } => 120,
        ExprKind::Index { .. } | ExprKind::Field { .. } => 110,
        ExprKind::Unary { .. } => 100,
        ExprKind::Binary { op, .. } => binary_prec(*op),
    }
}

const fn binary_prec(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 10,
        BinaryOp::Xor => 20,
        BinaryOp::And => 30,
        BinaryOp::BitAnd | BinaryOp::BitXor | BinaryOp::BitOr => 40,
        BinaryOp::Eq | BinaryOp::Ne => 50,
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 60,
        BinaryOp::Shl | BinaryOp::Shr => 70,
        BinaryOp::Add | BinaryOp::Sub => 80,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 90,
    }
}

const fn binary_op_str(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "MOD",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Shl => "SHL",
        BinaryOp::Shr => "SHR",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::BitAnd => "BIT_AND",
        BinaryOp::BitXor => "BIT_XOR",
        BinaryOp::BitOr => "BIT_OR",
        BinaryOp::And => "AND",
        BinaryOp::Xor => "XOR",
        BinaryOp::Or => "OR",
    }
}
