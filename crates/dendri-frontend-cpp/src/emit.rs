//! ASL AST → Arduino-style C++ (MVP subset).

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::Stmt;
use dendri_ast::{DataType, Program, Scope};
use std::collections::HashMap;

/// Emit Arduino-style C++ from an ASL program AST.
pub fn emit_cpp(program: &Program) -> String {
    let pin_labels: HashMap<u32, String> = program
        .hardware
        .iter()
        .map(|h| (h.pin, h.label.clone()))
        .collect();

    let mut out = String::new();
    out.push_str("// DendriForge C++ frontend — MVP Arduino subset\n");
    if let Some(name) = &program.metadata.project_name {
        out.push_str(&format!("// project: {name}\n"));
    }
    out.push('\n');

    for h in &program.hardware {
        out.push_str(&format!(
            "// ASL_HW: pin={} type={:?} mode={:?} label={} role={}\n",
            h.pin, h.hw_type, h.io_mode, h.label, h.target_role
        ));
        out.push_str(&format!("const int {} = {};\n", h.label, h.pin));
    }
    if !program.hardware.is_empty() {
        out.push('\n');
    }

    for d in &program.data {
        out.push_str(&format!(
            "// ASL_DATA: name={} type={} scope={} unit={}\n",
            d.name,
            d.data_type.as_str(),
            scope_tag(d.scope),
            d.unit
        ));
        match d.scope {
            Scope::Const => {
                let (ty, val) = cpp_const(d.data_type, &d.init);
                out.push_str(&format!("const {ty} {} = {val};\n", d.name));
            }
            _ => {
                let (ty, val) = cpp_var(d.data_type, &d.init);
                out.push_str(&format!("{ty} {} = {val};\n", d.name));
            }
        }
    }
    if !program.data.is_empty() {
        out.push('\n');
    }

    out.push_str("void setup() {\n");
    for stmt in &program.setup {
        emit_stmt(stmt, &mut out, 1, &pin_labels);
    }
    out.push_str("}\n\n");

    out.push_str("void loop() {\n");
    for stmt in &program.loop_body {
        emit_stmt(stmt, &mut out, 1, &pin_labels);
    }
    out.push_str("}\n");

    for f in &program.functions {
        let params = f
            .params
            .iter()
            .map(|p| format!("bool {p}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("\nbool {}({params}) {{\n", f.name));
        for stmt in &f.body {
            emit_stmt(stmt, &mut out, 1, &pin_labels);
        }
        out.push_str("}\n");
    }

    out
}

fn cpp_const(ty: DataType, init: &str) -> (&'static str, String) {
    match ty {
        DataType::Time => {
            let ms = parse_time_ms(init).unwrap_or(0);
            ("unsigned long", format!("{ms}UL"))
        }
        DataType::Bool => (
            "bool",
            if init.eq_ignore_ascii_case("true") {
                "true".into()
            } else {
                "false".into()
            },
        ),
        DataType::Real | DataType::Lreal => ("float", init.to_string()),
        DataType::String => {
            let inner = init.trim().trim_matches('"');
            ("String", format!("\"{inner}\""))
        }
        DataType::Int | DataType::Dint => ("int", init.to_string()),
        DataType::Uint | DataType::Byte | DataType::Word | DataType::Dword => {
            ("unsigned int", init.to_string())
        }
    }
}

fn cpp_var(ty: DataType, init: &str) -> (&'static str, String) {
    match ty {
        DataType::Bool => (
            "bool",
            if init.eq_ignore_ascii_case("true") {
                "true".into()
            } else {
                "false".into()
            },
        ),
        DataType::Time => {
            let ms = parse_time_ms(init).unwrap_or(0);
            ("unsigned long", format!("{ms}UL"))
        }
        DataType::Real | DataType::Lreal => ("float", init.to_string()),
        DataType::String => ("String", format!("String(\"{}\")", init.trim_matches('"'))),
        DataType::Int | DataType::Dint => ("int", init.to_string()),
        _ => ("unsigned int", init.to_string()),
    }
}

fn scope_tag(scope: Scope) -> &'static str {
    match scope {
        Scope::Const => "CONST",
        Scope::Var => "VAR",
        Scope::Retain => "RETAIN",
        Scope::Persistent => "PERSISTENT",
        Scope::Temp => "TEMP",
        Scope::Extern => "EXTERN",
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

fn resolve_pin(expr: &Expr, labels: &HashMap<u32, String>) -> String {
    match expr {
        Expr::Uint { value } => {
            if let Some(l) = labels.get(&(*value as u32)) {
                l.clone()
            } else {
                value.to_string()
            }
        }
        other => emit_expr(other, labels),
    }
}

fn emit_stmt(stmt: &Stmt, out: &mut String, indent: usize, labels: &HashMap<u32, String>) {
    let pad = "  ".repeat(indent);
    match stmt {
        Stmt::Assign { target, value } => {
            out.push_str(&format!("{pad}{target} = {};\n", emit_expr(value, labels)));
        }
        Stmt::Call { name, args } => {
            out.push_str(&format!("{pad}{};\n", emit_call(name, args, labels)));
        }
        Stmt::If {
            condition,
            then,
            elif,
            else_body,
        } => {
            out.push_str(&format!("{pad}if ({}) {{\n", emit_expr(condition, labels)));
            for s in then {
                emit_stmt(s, out, indent + 1, labels);
            }
            out.push_str(&format!("{pad}}}"));
            for arm in elif {
                out.push_str(&format!(
                    " else if ({}) {{\n",
                    emit_expr(&arm.condition, labels)
                ));
                for s in &arm.then {
                    emit_stmt(s, out, indent + 1, labels);
                }
                out.push_str(&format!("{pad}}}"));
            }
            if let Some(body) = else_body {
                out.push_str(" else {\n");
                for s in body {
                    emit_stmt(s, out, indent + 1, labels);
                }
                out.push_str(&format!("{pad}}}"));
            }
            out.push('\n');
        }
        Stmt::While { condition, body } => {
            out.push_str(&format!("{pad}while ({}) {{\n", emit_expr(condition, labels)));
            for s in body {
                emit_stmt(s, out, indent + 1, labels);
            }
            out.push_str(&format!("{pad}}}\n"));
        }
        Stmt::For { counter, to, body } => {
            let c = counter.as_deref().unwrap_or("_i");
            out.push_str(&format!(
                "{pad}for (int {c} = 0; {c} < {}; {c}++) {{\n",
                emit_expr(to, labels)
            ));
            for s in body {
                emit_stmt(s, out, indent + 1, labels);
            }
            out.push_str(&format!("{pad}}}\n"));
        }
        Stmt::Return { value } => {
            if let Some(v) = value {
                out.push_str(&format!("{pad}return {};\n", emit_expr(v, labels)));
            } else {
                out.push_str(&format!("{pad}return;\n"));
            }
        }
        Stmt::Exit => out.push_str(&format!("{pad}break;\n")),
        Stmt::Continue => out.push_str(&format!("{pad}continue;\n")),
        Stmt::VendorExtension { reason } => {
            out.push_str(&format!("{pad}// VENDOR_EXTENSION: {reason}\n"));
        }
    }
}

fn emit_call(name: &str, args: &[Expr], labels: &HashMap<u32, String>) -> String {
    match name {
        "CFG_DIGITAL_OUTPUT" => {
            format!("pinMode({}, OUTPUT)", resolve_pin(&args[0], labels))
        }
        "CFG_DIGITAL_INPUT" => {
            format!("pinMode({}, INPUT)", resolve_pin(&args[0], labels))
        }
        "CFG_DIGITAL_INPUT_PULLUP" => {
            format!("pinMode({}, INPUT_PULLUP)", resolve_pin(&args[0], labels))
        }
        "CFG_DIGITAL_INPUT_PULLDOWN" => {
            format!("pinMode({}, INPUT_PULLDOWN)", resolve_pin(&args[0], labels))
        }
        "SET_BOOL" => {
            let pin = resolve_pin(&args[0], labels);
            let val = match args.get(1) {
                Some(Expr::Bool { value: true }) => "HIGH".into(),
                Some(Expr::Bool { value: false }) => "LOW".into(),
                Some(e) => emit_expr(e, labels),
                None => "LOW".into(),
            };
            format!("digitalWrite({pin}, {val})")
        }
        "GET_BOOL" => format!("digitalRead({})", resolve_pin(&args[0], labels)),
        "CFG_PWM_OUTPUT" => {
            // Arduino AVR subset: pinMode OUTPUT; freq kept as ASL call arg for round-trip.
            let pin = resolve_pin(&args[0], labels);
            let freq = args
                .get(1)
                .map(|e| emit_expr(e, labels))
                .unwrap_or_else(|| "1000".into());
            format!("CFG_PWM_OUTPUT({pin}, {freq})")
        }
        "SET_PWM_DUTY" => {
            let pin = resolve_pin(&args[0], labels);
            let duty = args
                .get(1)
                .map(|e| emit_expr(e, labels))
                .unwrap_or_else(|| "0".into());
            format!("analogWrite({pin}, {duty})")
        }
        "SET_PWM_FREQ" => {
            let a = args
                .iter()
                .map(|e| emit_expr(e, labels))
                .collect::<Vec<_>>()
                .join(", ");
            format!("SET_PWM_FREQ({a})")
        }
        "GET_PWM_DUTY" => format!("GET_PWM_DUTY({})", resolve_pin(&args[0], labels)),
        "CFG_ADC_INPUT" => {
            format!("CFG_ADC_INPUT({})", resolve_pin(&args[0], labels))
        }
        "READ_ADC" => format!("analogRead({})", resolve_pin(&args[0], labels)),
        "CFG_SERIAL" => {
            let baud = args
                .first()
                .map(|e| emit_expr(e, labels))
                .unwrap_or_else(|| "9600".into());
            format!("Serial.begin({baud})")
        }
        "SERIAL_PRINT" => {
            let msg = args
                .first()
                .map(|e| emit_expr(e, labels))
                .unwrap_or_else(|| "\"\"".into());
            format!("Serial.print({msg})")
        }
        "SERIAL_PRINTLN" => {
            let msg = args
                .first()
                .map(|e| emit_expr(e, labels))
                .unwrap_or_else(|| "\"\"".into());
            format!("Serial.println({msg})")
        }
        "SERIAL_FLUSH" => "Serial.flush()".into(),
        "SERIAL_AVAILABLE" => "Serial.available()".into(),
        "SERIAL_READ" => "Serial.read()".into(),
        "SERIAL_READLN" => "Serial.readStringUntil('\\n')".into(),
        "CFG_I2C" => {
            // Emit Wire.begin(sda, scl) + keep bus/freq via CFG_I2C for full round-trip.
            let a = args
                .iter()
                .map(|e| emit_expr(e, labels))
                .collect::<Vec<_>>()
                .join(", ");
            format!("CFG_I2C({a})")
        }
        "CFG_SPI" => {
            let a = args
                .iter()
                .map(|e| emit_expr(e, labels))
                .collect::<Vec<_>>()
                .join(", ");
            format!("CFG_SPI({a})")
        }
        "WAIT_TIME" => {
            let a = args.first().map(|e| emit_expr(e, labels)).unwrap_or_else(|| "0".into());
            format!("delay({a})")
        }
        "WAIT_TIME_US" => {
            let a = args.first().map(|e| emit_expr(e, labels)).unwrap_or_else(|| "0".into());
            format!("delayMicroseconds({a})")
        }
        "GET_SYS_TIME" => "millis()".into(),
        "GET_SYS_TIME_US" => "micros()".into(),
        other => {
            let a = args
                .iter()
                .map(|e| emit_expr(e, labels))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{other}({a})")
        }
    }
}

fn emit_expr(expr: &Expr, labels: &HashMap<u32, String>) -> String {
    match expr {
        Expr::Bool { value } => {
            if *value {
                "true".into()
            } else {
                "false".into()
            }
        }
        Expr::Uint { value } => value.to_string(),
        Expr::Int { value } => value.to_string(),
        Expr::Real { value } => format!("{value}"),
        Expr::String { value } => format!("\"{value}\""),
        Expr::Time { value } => parse_time_ms(value)
            .map(|ms| format!("{ms}UL"))
            .unwrap_or_else(|| value.clone()),
        Expr::Hex { value } => value.clone(),
        Expr::Var { name } => name.clone(),
        Expr::Lit { value } => format!("{value:?}"),
        Expr::Call { name, args } => emit_call(name, args, labels),
        Expr::Unary { op, expr } => {
            let inner = emit_expr(expr, labels);
            match op {
                UnaryOp::Not => format!("!({inner})"),
                UnaryOp::Neg => format!("-({inner})"),
                UnaryOp::BitNot => format!("~({inner})"),
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
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
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
