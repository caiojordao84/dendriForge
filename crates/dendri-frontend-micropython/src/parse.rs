//! MicroPython (MVP subset) → ASL AST.

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::{ElifArm, Stmt};
use dendri_ast::{
    DataEntry, DataType, FunctionDef, HardwareEntry, HwType, IoMode, Metadata, Program, Scope,
};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MpyParseError {
    #[error("MicroPython parse error at line {line}: {message}")]
    At { line: usize, message: String },
}

#[derive(Debug, Clone)]
struct Line {
    indent: usize,
    text: String,
    number: usize,
}

#[derive(Debug, Clone)]
struct HwAnno {
    pin: u32,
    io_mode: IoMode,
    label: String,
    target_role: String,
}

#[derive(Debug, Clone)]
struct DataAnno {
    data_type: DataType,
    scope: Scope,
    unit: String,
}

fn preprocess(
    src: &str,
) -> (
    Option<String>,
    Vec<Line>,
    HashMap<String, HwAnno>,
    HashMap<String, DataAnno>,
) {
    let mut project_name = None;
    let mut lines = Vec::new();
    let mut hw_annos = HashMap::new();
    let mut data_annos = HashMap::new();
    for (i, raw) in src.lines().enumerate() {
        let number = i + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            if let Some(rest) = trimmed.strip_prefix("# project:") {
                project_name = Some(rest.trim().to_string());
            }
            if let Some(rest) = trimmed.strip_prefix("# ASL_HW:") {
                if let Some(hw) = parse_asl_hw_anno(rest.trim()) {
                    hw_annos.insert(to_upper_snake(&hw.label), hw.clone());
                    hw_annos.insert(hw.label.clone(), hw);
                }
            }
            if let Some(rest) = trimmed.strip_prefix("# ASL_DATA:") {
                if let Some((name, anno)) = parse_asl_data_anno(rest.trim()) {
                    data_annos.insert(name, anno);
                }
            }
            continue;
        }
        let indent = raw.chars().take_while(|c| *c == ' ').count() / 4;
        // also accept tabs as one indent level
        let indent = if raw.starts_with('\t') {
            raw.chars().take_while(|c| *c == '\t').count()
        } else {
            indent
        };
        lines.push(Line {
            indent,
            text: trimmed.to_string(),
            number,
        });
    }
    (project_name, lines, hw_annos, data_annos)
}

struct Parser {
    lines: Vec<Line>,
    pos: usize,
    project_name: Option<String>,
    pin_consts: HashMap<String, u32>,     // LED_PIN -> 13
    pin_objs: HashMap<String, String>,    // led_pin -> LED_PIN
    obj_to_pin: HashMap<String, u32>,     // led_pin -> 13
    pin_placeholders: HashMap<String, ()>, // start_btn_pin -> ()
    hw_annos: HashMap<String, HwAnno>,
    data_annos: HashMap<String, DataAnno>,
}

impl Parser {
    fn new(src: &str) -> Self {
        let (project_name, lines, hw_annos, data_annos) = preprocess(src);
        Self {
            lines,
            pos: 0,
            project_name,
            pin_consts: HashMap::new(),
            pin_objs: HashMap::new(),
            obj_to_pin: HashMap::new(),
            pin_placeholders: HashMap::new(),
            hw_annos,
            data_annos,
        }
    }

    fn peek(&self) -> Option<&Line> {
        self.lines.get(self.pos)
    }

    fn bump(&mut self) -> Option<Line> {
        if self.pos < self.lines.len() {
            let l = self.lines[self.pos].clone();
            self.pos += 1;
            Some(l)
        } else {
            None
        }
    }

    fn err(&self, line: usize, message: impl Into<String>) -> MpyParseError {
        MpyParseError::At {
            line,
            message: message.into(),
        }
    }

    fn parse_program(&mut self) -> Result<Program, MpyParseError> {
        let mut data = Vec::new();
        let mut hardware = Vec::new();
        let mut setup = Vec::new();
        let mut loop_body = Vec::new();
        let mut functions = Vec::new();

        // Placeholders are emitted after pin consts — pre-scan so companion
        // detection works even without ASL_HW annotations.
        for line in &self.lines {
            if line.indent != 0 {
                continue;
            }
            if let Some((name, rhs)) = split_assign(&line.text) {
                if rhs == "None" && name.ends_with("_pin") {
                    self.pin_placeholders.insert(name, ());
                }
            }
        }

        while let Some(line) = self.peek().cloned() {
            if line.indent != 0 {
                return Err(self.err(line.number, "unexpected indented top-level"));
            }
            let t = line.text.as_str();
            if t.starts_with("from ") || t.starts_with("import ") {
                self.bump();
                continue;
            }
            if t == "setup()" || t.starts_with("while True") {
                // entry-point boilerplate — skip rest
                while self.pos < self.lines.len() {
                    self.pos += 1;
                }
                break;
            }
            if t.starts_with("def ") {
                let line = self.bump().unwrap();
                let (name, params) = parse_def_header(&line.text).ok_or_else(|| {
                    self.err(line.number, format!("bad def: {}", line.text))
                })?;
                let body_lines = self.collect_block(1)?;
                let body = self.parse_block(&body_lines)?;
                if name == "setup" {
                    setup = body;
                } else if name == "loop" {
                    loop_body = body;
                } else {
                    functions.push(FunctionDef { name, params, body });
                }
                continue;
            }

            // assignment at top level
            let line = self.bump().unwrap();
            if let Some((name, rhs)) = split_assign(&line.text) {
                if rhs == "None" {
                    // pin object placeholder — remember for companion pin-const detection
                    if name.ends_with("_pin") {
                        self.pin_placeholders.insert(name, ());
                    }
                    continue;
                }
                // Pin constants: ASL_HW anno, *PIN name, or companion `*_pin = None`.
                if let Ok(pin) = rhs.parse::<u32>() {
                    let obj = pin_obj_from_const(&name);
                    let is_pin = self.hw_annos.contains_key(&name)
                        || looks_like_pin_const(&name)
                        || self.pin_placeholders.contains_key(&obj);
                    if is_pin {
                        self.pin_consts.insert(name.clone(), pin);
                        if let Some(anno) = self
                            .hw_annos
                            .get(&name)
                            .or_else(|| self.hw_annos.values().find(|h| h.pin == pin))
                        {
                            hardware.push(HardwareEntry {
                                pin: anno.pin,
                                hw_type: HwType::Digital,
                                io_mode: anno.io_mode,
                                label: anno.label.clone(),
                                target_role: anno.target_role.clone(),
                            });
                        } else {
                            hardware.push(HardwareEntry {
                                pin,
                                hw_type: HwType::Digital,
                                io_mode: IoMode::Do,
                                label: snake_to_camel(&name),
                                target_role: "digital-out".into(),
                            });
                        }
                        continue;
                    }
                }
                if let Some(anno) = self.data_annos.get(&name) {
                    let mut init = rhs.clone();
                    if anno.data_type == DataType::Time {
                        if let Ok(ms) = rhs.parse::<u64>() {
                            init = format!("T#{ms}ms");
                        }
                    } else if anno.data_type == DataType::Bool {
                        init = if rhs == "True" {
                            "TRUE".into()
                        } else {
                            "FALSE".into()
                        };
                    }
                    data.push(DataEntry {
                        name,
                        data_type: anno.data_type,
                        scope: anno.scope,
                        init,
                        unit: anno.unit.clone(),
                    });
                    continue;
                }
                let (data_type, init, scope) = classify_global(&name, &rhs);
                data.push(DataEntry {
                    name,
                    data_type,
                    scope,
                    init,
                    unit: if data_type == DataType::Time {
                        "ms".into()
                    } else {
                        "-".into()
                    },
                });
                continue;
            }
            return Err(self.err(line.number, format!("unsupported top-level: {}", line.text)));
        }

        // Lower Pin(...) / .value() using known maps built during block parse.
        // Re-parse setup/loop with lowering — already done in parse_block via helpers.
        // After setup parse, extract pin object bindings from CFG-like calls.
        refine_hardware(&mut hardware, &setup);

        Ok(Program {
            metadata: Metadata {
                project_name: self
                    .project_name
                    .clone()
                    .or_else(|| Some("micropython-program".into())),
                editor: Some("dendriForge".into()),
                author: None,
                notes: Some("parsed from MicroPython frontend (MVP subset)".into()),
                expect_error: None,
            },
            hardware,
            data,
            setup,
            loop_body,
            functions,
            ladder: vec![],
            grafcet: vec![],
        })
    }

    fn collect_block(&mut self, min_indent: usize) -> Result<Vec<Line>, MpyParseError> {
        let mut out = Vec::new();
        while let Some(line) = self.peek() {
            if line.indent < min_indent {
                break;
            }
            out.push(self.bump().unwrap());
        }
        Ok(out)
    }

    fn parse_block(&mut self, lines: &[Line]) -> Result<Vec<Stmt>, MpyParseError> {
        let mut stmts = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = &lines[i];
            let t = line.text.trim();
            if t.starts_with("global ") {
                i += 1;
                continue;
            }
            if t.starts_with("if ") && t.ends_with(':') {
                let (stmt, next) = self.parse_if_at(lines, i)?;
                stmts.push(stmt);
                i = next;
                continue;
            }
            if t.starts_with("while ") && t.ends_with(':') {
                let (stmt, next) = self.parse_while_at(lines, i)?;
                stmts.push(stmt);
                i = next;
                continue;
            }
            if t.starts_with("for ") && t.ends_with(':') {
                let (stmt, next) = self.parse_for_at(lines, i)?;
                stmts.push(stmt);
                i = next;
                continue;
            }
            if t == "pass" {
                i += 1;
                continue;
            }
            if t == "break" {
                stmts.push(Stmt::Exit);
                i += 1;
                continue;
            }
            if t == "continue" {
                stmts.push(Stmt::Continue);
                i += 1;
                continue;
            }
            if t.starts_with("return") {
                let rest = t["return".len()..].trim();
                if rest.is_empty() {
                    stmts.push(Stmt::Return { value: None });
                } else {
                    stmts.push(Stmt::Return {
                        value: Some(parse_expr(rest)?),
                    });
                }
                i += 1;
                continue;
            }
            stmts.push(self.lower_simple_stmt(t, line.number)?);
            i += 1;
        }
        Ok(stmts)
    }

    fn parse_if_at(&self, lines: &[Line], start: usize) -> Result<(Stmt, usize), MpyParseError> {
        let base = lines[start].indent;
        let cond_txt = lines[start].text.trim();
        let cond = cond_txt
            .strip_prefix("if ")
            .and_then(|s| s.strip_suffix(':'))
            .ok_or_else(|| self.err(lines[start].number, "bad if"))?;
        let condition = parse_expr(cond.trim())?;
        let mut i = start + 1;
        let mut then_lines = Vec::new();
        while i < lines.len() && lines[i].indent > base {
            then_lines.push(lines[i].clone());
            i += 1;
        }
        // dedent relative
        let then = self.parse_block_relative(&then_lines, base + 1)?;

        let mut elif = Vec::new();
        let mut else_body = None;
        while i < lines.len() && lines[i].indent == base {
            let t = lines[i].text.trim();
            if t.starts_with("elif ") && t.ends_with(':') {
                let c = t
                    .strip_prefix("elif ")
                    .and_then(|s| s.strip_suffix(':'))
                    .unwrap();
                let condition = parse_expr(c.trim())?;
                i += 1;
                let mut body_lines = Vec::new();
                while i < lines.len() && lines[i].indent > base {
                    body_lines.push(lines[i].clone());
                    i += 1;
                }
                let body = self.parse_block_relative(&body_lines, base + 1)?;
                elif.push(ElifArm {
                    condition,
                    then: body,
                });
            } else if t == "else:" {
                i += 1;
                let mut body_lines = Vec::new();
                while i < lines.len() && lines[i].indent > base {
                    body_lines.push(lines[i].clone());
                    i += 1;
                }
                else_body = Some(self.parse_block_relative(&body_lines, base + 1)?);
                break;
            } else {
                break;
            }
        }
        Ok((
            Stmt::If {
                condition,
                then,
                elif,
                else_body,
            },
            i,
        ))
    }

    fn parse_while_at(&self, lines: &[Line], start: usize) -> Result<(Stmt, usize), MpyParseError> {
        let base = lines[start].indent;
        let cond = lines[start]
            .text
            .trim()
            .strip_prefix("while ")
            .and_then(|s| s.strip_suffix(':'))
            .ok_or_else(|| self.err(lines[start].number, "bad while"))?;
        let condition = parse_expr(cond.trim())?;
        let mut i = start + 1;
        let mut body_lines = Vec::new();
        while i < lines.len() && lines[i].indent > base {
            body_lines.push(lines[i].clone());
            i += 1;
        }
        let body = self.parse_block_relative(&body_lines, base + 1)?;
        Ok((Stmt::While { condition, body }, i))
    }

    fn parse_for_at(&self, lines: &[Line], start: usize) -> Result<(Stmt, usize), MpyParseError> {
        let base = lines[start].indent;
        let header = lines[start]
            .text
            .trim()
            .strip_prefix("for ")
            .and_then(|s| s.strip_suffix(':'))
            .ok_or_else(|| self.err(lines[start].number, "bad for"))?;
        // for i in range(N)
        let (counter, to) = if let Some((c, rest)) = header.split_once(" in range(") {
            let n = rest.trim().trim_end_matches(')');
            (Some(c.trim().to_string()), parse_expr(n)?)
        } else {
            return Err(self.err(lines[start].number, "MVP for requires range(...)"));
        };
        let mut i = start + 1;
        let mut body_lines = Vec::new();
        while i < lines.len() && lines[i].indent > base {
            body_lines.push(lines[i].clone());
            i += 1;
        }
        let body = self.parse_block_relative(&body_lines, base + 1)?;
        Ok((Stmt::For { counter, to, body }, i))
    }

    fn parse_block_relative(
        &self,
        lines: &[Line],
        _base: usize,
    ) -> Result<Vec<Stmt>, MpyParseError> {
        // Rebase indents so nested parse_block works — lines keep absolute indent;
        // parse_block only uses relative comparison via if/while helpers on same slice.
        let mut fake = self.clone_for_block();
        fake.parse_block(lines)
    }

    fn clone_for_block(&self) -> Parser {
        Parser {
            lines: vec![],
            pos: 0,
            project_name: self.project_name.clone(),
            pin_consts: self.pin_consts.clone(),
            pin_objs: self.pin_objs.clone(),
            obj_to_pin: self.obj_to_pin.clone(),
            pin_placeholders: self.pin_placeholders.clone(),
            hw_annos: self.hw_annos.clone(),
            data_annos: self.data_annos.clone(),
        }
    }

    fn lower_simple_stmt(&mut self, t: &str, line: usize) -> Result<Stmt, MpyParseError> {
        if let Some((lhs, rhs)) = split_assign(t) {
            // led_pin = Pin(LED_PIN, Pin.OUT)
            if let Some(pin_call) = parse_pin_ctor(&rhs) {
                let (pin_expr, mode) = pin_call;
                let pin = resolve_pin_number(&pin_expr, &self.pin_consts).unwrap_or(0);
                self.obj_to_pin.insert(lhs.clone(), pin);
                if let Some(const_name) = pin_expr_name(&pin_expr) {
                    self.pin_objs.insert(lhs.clone(), const_name);
                }
                let opcode = match mode.as_str() {
                    "Pin.IN" => "CFG_DIGITAL_INPUT",
                    "Pin.IN, Pin.PULL_UP" | "Pin.IN,Pin.PULL_UP" => "CFG_DIGITAL_INPUT_PULLUP",
                    "Pin.IN, Pin.PULL_DOWN" | "Pin.IN,Pin.PULL_DOWN" => {
                        "CFG_DIGITAL_INPUT_PULLDOWN"
                    }
                    _ => "CFG_DIGITAL_OUTPUT",
                };
                return Ok(Stmt::Call {
                    name: opcode.into(),
                    args: vec![Expr::Uint { value: pin as u64 }],
                });
            }
            // q = TON(...)
            let value = parse_expr(&rhs).map_err(|e| self.err(line, e.to_string()))?;
            return Ok(Stmt::Assign {
                target: lhs,
                value: rewrite_expr(value, &self.pin_consts, &self.obj_to_pin),
            });
        }

        // led_pin.value(1)  or sleep_ms(ON_TIME)
        if let Some((obj, method, args)) = parse_obj_method(t) {
            if method == "value" {
                let pin = self
                    .obj_to_pin
                    .get(&obj)
                    .copied()
                    .or_else(|| {
                        // fallback: strip _pin and look up upper
                        let key = to_upper_snake(obj.trim_end_matches("_pin"));
                        self.pin_consts.get(&key).copied()
                    })
                    .unwrap_or(0);
                if args.is_empty() {
                    // read — shouldn't be stmt
                    return Ok(Stmt::Assign {
                        target: "_".into(),
                        value: Expr::Call {
                            name: "GET_BOOL".into(),
                            args: vec![Expr::Uint { value: pin as u64 }],
                        },
                    });
                }
                let val = parse_expr(&args[0])?;
                let val = match val {
                    Expr::Uint { value: 1 } => Expr::Bool { value: true },
                    Expr::Uint { value: 0 } => Expr::Bool { value: false },
                    other => other,
                };
                return Ok(Stmt::Call {
                    name: "SET_BOOL".into(),
                    args: vec![Expr::Uint { value: pin as u64 }, val],
                });
            }
        }

        if let Some((name, args_s)) = parse_call(t) {
            let args = parse_arg_list(&args_s)?;
            let args: Vec<_> = args
                .into_iter()
                .map(|a| rewrite_expr(a, &self.pin_consts, &self.obj_to_pin))
                .collect();
            match name.as_str() {
                "sleep_ms" => {
                    return Ok(Stmt::Call {
                        name: "WAIT_TIME".into(),
                        args,
                    });
                }
                "sleep_us" => {
                    return Ok(Stmt::Call {
                        name: "WAIT_TIME_US".into(),
                        args,
                    });
                }
                other => {
                    return Ok(Stmt::Call {
                        name: other.into(),
                        args,
                    });
                }
            }
        }

        Err(self.err(line, format!("unsupported statement: {t}")))
    }
}

fn parse_def_header(text: &str) -> Option<(String, Vec<String>)> {
    let rest = text.strip_prefix("def ")?.trim().trim_end_matches(':');
    let (name, params) = rest.split_once('(')?;
    let params = params.trim_end_matches(')');
    let params = if params.trim().is_empty() {
        vec![]
    } else {
        params.split(',').map(|s| s.trim().to_string()).collect()
    };
    Some((name.trim().to_string(), params))
}

fn split_assign(t: &str) -> Option<(String, String)> {
    // ignore == comparison
    let mut depth = 0i32;
    let chars: Vec<char> = t.chars().collect();
    for i in 0..chars.len() {
        match chars[i] {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '=' if depth == 0 => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    return None;
                }
                if i > 0 && matches!(chars[i - 1], '!' | '<' | '>') {
                    return None;
                }
                let lhs = t[..i].trim().to_string();
                let rhs = t[i + 1..].trim().to_string();
                if lhs.is_empty() || rhs.is_empty() {
                    return None;
                }
                return Some((lhs, rhs));
            }
            _ => {}
        }
    }
    None
}

fn classify_global(name: &str, rhs: &str) -> (DataType, String, Scope) {
    let scope = if name
        .chars()
        .all(|c| c.is_ascii_uppercase() || c == '_')
    {
        Scope::Const
    } else {
        Scope::Var
    };
    if rhs == "True" || rhs == "False" {
        return (
            DataType::Bool,
            if rhs == "True" {
                "TRUE".into()
            } else {
                "FALSE".into()
            },
            Scope::Var,
        );
    }
    if let Ok(n) = rhs.parse::<u64>() {
        if scope == Scope::Const {
            return (DataType::Time, format!("T#{n}ms"), Scope::Const);
        }
        return (DataType::Uint, n.to_string(), scope);
    }
    if rhs.starts_with('"') || rhs.starts_with('\'') {
        return (DataType::String, rhs.to_string(), scope);
    }
    if rhs.contains('.') {
        return (DataType::Real, rhs.to_string(), scope);
    }
    (DataType::Int, rhs.to_string(), scope)
}

fn looks_like_pin_const(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper == "PIN" || upper.ends_with("_PIN") || upper.starts_with("PIN_")
}

fn pin_obj_from_const(const_name: &str) -> String {
    let lower = const_name.to_ascii_lowercase();
    if lower.ends_with("_pin") {
        lower
    } else {
        format!("{lower}_pin")
    }
}

fn parse_asl_hw_anno(body: &str) -> Option<HwAnno> {
    let mut pin = None;
    let mut label = String::new();
    let mut role = String::from("digital-out");
    let mut io_mode = IoMode::Do;
    for part in body.split_whitespace() {
        if let Some(v) = part.strip_prefix("pin=") {
            pin = v.parse().ok();
        } else if let Some(v) = part.strip_prefix("label=") {
            label = v.to_string();
        } else if let Some(v) = part.strip_prefix("role=") {
            role = v.to_string();
        } else if let Some(v) = part.strip_prefix("mode=") {
            io_mode = match v {
                "Di" | "DI" => IoMode::Di,
                "Bi" | "BI" => IoMode::Bi,
                _ => IoMode::Do,
            };
        }
    }
    let pin = pin?;
    if label.is_empty() {
        label = format!("pin{pin}");
    }
    Some(HwAnno {
        pin,
        io_mode,
        label,
        target_role: role,
    })
}

fn parse_asl_data_anno(body: &str) -> Option<(String, DataAnno)> {
    let mut name = None;
    let mut data_type = DataType::Uint;
    let mut scope = Scope::Var;
    let mut unit = String::from("-");
    for part in body.split_whitespace() {
        if let Some(v) = part.strip_prefix("name=") {
            name = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("type=") {
            data_type = DataType::parse(v)?;
        } else if let Some(v) = part.strip_prefix("scope=") {
            scope = match v {
                "CONST" => Scope::Const,
                "RETAIN" => Scope::Retain,
                "PERSISTENT" => Scope::Persistent,
                "TEMP" => Scope::Temp,
                "EXTERN" => Scope::Extern,
                _ => Scope::Var,
            };
        } else if let Some(v) = part.strip_prefix("unit=") {
            unit = v.to_string();
        }
    }
    Some((
        name?,
        DataAnno {
            data_type,
            scope,
            unit,
        },
    ))
}

fn snake_to_camel(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut out = String::new();
    let mut up = false;
    for (i, c) in lower.chars().enumerate() {
        if c == '_' {
            up = true;
            continue;
        }
        if i == 0 {
            out.push(c);
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
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

fn parse_pin_ctor(rhs: &str) -> Option<(String, String)> {
    let rest = rhs.strip_prefix("Pin(")?.strip_suffix(')')?;
    let (pin, mode) = rest.split_once(',')?;
    Some((pin.trim().to_string(), mode.trim().to_string()))
}

fn pin_expr_name(pin_expr: &str) -> Option<String> {
    if pin_expr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(pin_expr.to_string())
    } else {
        None
    }
}

fn resolve_pin_number(pin_expr: &str, consts: &HashMap<String, u32>) -> Option<u32> {
    if let Ok(n) = pin_expr.parse::<u32>() {
        return Some(n);
    }
    consts.get(pin_expr).copied()
}

fn parse_obj_method(t: &str) -> Option<(String, String, Vec<String>)> {
    // obj.method(args)
    let (left, args) = t.split_once('(')?;
    let args = args.strip_suffix(')')?;
    let (obj, method) = left.rsplit_once('.')?;
    let arg_list = if args.trim().is_empty() {
        vec![]
    } else {
        split_args(args)
    };
    Some((obj.to_string(), method.to_string(), arg_list))
}

fn parse_call(t: &str) -> Option<(String, String)> {
    let (name, rest) = t.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    if name.contains('.') {
        return None;
    }
    Some((name.trim().to_string(), args.to_string()))
}

fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn parse_arg_list(s: &str) -> Result<Vec<Expr>, MpyParseError> {
    if s.trim().is_empty() {
        return Ok(vec![]);
    }
    split_args(s)
        .into_iter()
        .map(|a| parse_expr(&a))
        .collect()
}

fn parse_expr(s: &str) -> Result<Expr, MpyParseError> {
    let s = s.trim();
    ExprParser { s, i: 0 }.parse_or()
}

struct ExprParser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> ExprParser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }

    fn skip_ws(&mut self) {
        while self.rest().starts_with(char::is_whitespace) {
            self.i += 1;
        }
    }

    fn parse_or(&mut self) -> Result<Expr, MpyParseError> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.rest().starts_with("or ") || self.rest() == "or" {
                self.i += 2;
                self.skip_ws();
                let right = self.parse_and()?;
                left = Expr::Binop {
                    op: BinaryOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, MpyParseError> {
        let mut left = self.parse_not()?;
        loop {
            self.skip_ws();
            if self.rest().starts_with("and ") || self.rest().starts_with("and(") {
                self.i += 3;
                self.skip_ws();
                let right = self.parse_not()?;
                left = Expr::Binop {
                    op: BinaryOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, MpyParseError> {
        self.skip_ws();
        if self.rest().starts_with("not ") || self.rest().starts_with("not(") {
            self.i += 3;
            self.skip_ws();
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(self.parse_not()?),
            });
        }
        self.parse_cmp()
    }

    fn parse_cmp(&mut self) -> Result<Expr, MpyParseError> {
        let mut left = self.parse_add()?;
        loop {
            self.skip_ws();
            let op = if self.rest().starts_with("==") {
                self.i += 2;
                BinaryOp::Eq
            } else if self.rest().starts_with("!=") {
                self.i += 2;
                BinaryOp::Ne
            } else if self.rest().starts_with("<=") {
                self.i += 2;
                BinaryOp::Le
            } else if self.rest().starts_with(">=") {
                self.i += 2;
                BinaryOp::Ge
            } else if self.rest().starts_with('<') {
                self.i += 1;
                BinaryOp::Lt
            } else if self.rest().starts_with('>') {
                self.i += 1;
                BinaryOp::Gt
            } else {
                break;
            };
            let right = self.parse_add()?;
            left = Expr::Binop {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, MpyParseError> {
        let mut left = self.parse_mul()?;
        loop {
            self.skip_ws();
            let op = if self.rest().starts_with('+') {
                self.i += 1;
                BinaryOp::Add
            } else if self.rest().starts_with('-') {
                self.i += 1;
                BinaryOp::Sub
            } else {
                break;
            };
            let right = self.parse_mul()?;
            left = Expr::Binop {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, MpyParseError> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            let op = if self.rest().starts_with('*') {
                self.i += 1;
                BinaryOp::Mul
            } else if self.rest().starts_with('/') {
                self.i += 1;
                BinaryOp::Div
            } else if self.rest().starts_with('%') {
                self.i += 1;
                BinaryOp::Mod
            } else {
                break;
            };
            let right = self.parse_unary()?;
            left = Expr::Binop {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, MpyParseError> {
        self.skip_ws();
        if self.rest().starts_with('-') {
            self.i += 1;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.rest().starts_with('~') {
            self.i += 1;
            return Ok(Expr::Unary {
                op: UnaryOp::BitNot,
                expr: Box::new(self.parse_unary()?),
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, MpyParseError> {
        self.skip_ws();
        let r = self.rest();
        if r.starts_with("True") {
            self.i += 4;
            return Ok(Expr::Bool { value: true });
        }
        if r.starts_with("False") {
            self.i += 5;
            return Ok(Expr::Bool { value: false });
        }
        if r.starts_with('(') {
            self.i += 1;
            // ternary: (1 if x else 0)
            let inner = self.parse_or()?;
            self.skip_ws();
            if self.rest().starts_with(')') {
                self.i += 1;
                return Ok(inner);
            }
            return Err(MpyParseError::At {
                line: 0,
                message: "expected ')'".into(),
            });
        }
        if r.starts_with('"') || r.starts_with('\'') {
            let quote = r.chars().next().unwrap();
            self.i += 1;
            let start = self.i;
            while self.i < self.s.len() && self.s.as_bytes()[self.i] as char != quote {
                self.i += 1;
            }
            let val = self.s[start..self.i].to_string();
            if self.i < self.s.len() {
                self.i += 1;
            }
            return Ok(Expr::String { value: val });
        }
        if r.starts_with(|c: char| c.is_ascii_digit()) {
            let start = self.i;
            while self.i < self.s.len()
                && (self.s.as_bytes()[self.i].is_ascii_digit() || self.s.as_bytes()[self.i] == b'.')
            {
                self.i += 1;
            }
            let num = &self.s[start..self.i];
            if num.contains('.') {
                return Ok(Expr::Real {
                    value: num.parse().unwrap_or(0.0),
                });
            }
            return Ok(Expr::Uint {
                value: num.parse().unwrap_or(0),
            });
        }
        if r.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            let start = self.i;
            while self.i < self.s.len() {
                let c = self.s.as_bytes()[self.i] as char;
                if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                    self.i += 1;
                } else {
                    break;
                }
            }
            let name = self.s[start..self.i].to_string();
            self.skip_ws();
            if self.rest().starts_with('(') {
                self.i += 1;
                let args_start = self.i;
                let mut depth = 1i32;
                while self.i < self.s.len() && depth > 0 {
                    match self.s.as_bytes()[self.i] as char {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    if depth > 0 {
                        self.i += 1;
                    }
                }
                let args_s = self.s[args_start..self.i].to_string();
                if self.i < self.s.len() {
                    self.i += 1; // )
                }
                // method call obj.value()
                if let Some((obj, method)) = name.rsplit_once('.') {
                    if method == "value" {
                        return Ok(Expr::Call {
                            name: "GET_BOOL".into(),
                            args: vec![Expr::Var {
                                name: obj.to_string(),
                            }],
                        });
                    }
                }
                let args = if args_s.trim().is_empty() {
                    vec![]
                } else {
                    parse_arg_list(&args_s)?
                };
                return Ok(Expr::Call { name, args });
            }
            // ternary sugar already handled elsewhere
            if name.contains('.') {
                // Pin.OUT etc as var
                return Ok(Expr::Var { name });
            }
            return Ok(Expr::Var { name });
        }
        // ternary: 1 if cond else 0
        if let Some(expr) = try_parse_ternary(self.s) {
            self.i = self.s.len();
            return Ok(expr);
        }
        Err(MpyParseError::At {
            line: 0,
            message: format!("bad expr near '{}'", self.rest()),
        })
    }
}

fn try_parse_ternary(s: &str) -> Option<Expr> {
    // 1 if X else 0
    let (then_s, rest) = s.split_once(" if ")?;
    let (cond_s, else_s) = rest.split_once(" else ")?;
    let then_e = parse_expr(then_s.trim()).ok()?;
    let cond = parse_expr(cond_s.trim()).ok()?;
    let else_e = parse_expr(else_s.trim()).ok()?;
    Some(Expr::Call {
        name: "SEL".into(),
        args: vec![cond, else_e, then_e],
    })
}

fn rewrite_expr(
    expr: Expr,
    pin_consts: &HashMap<String, u32>,
    obj_to_pin: &HashMap<String, u32>,
) -> Expr {
    match expr {
        Expr::Var { name } => {
            if let Some(&pin) = pin_consts.get(&name) {
                Expr::Uint { value: pin as u64 }
            } else if let Some(&pin) = obj_to_pin.get(&name) {
                Expr::Uint { value: pin as u64 }
            } else {
                Expr::Var { name }
            }
        }
        Expr::Call { name, args } => {
            let args: Vec<_> = args
                .into_iter()
                .map(|a| rewrite_expr(a, pin_consts, obj_to_pin))
                .collect();
            if name == "GET_BOOL" {
                let pin = match args.first() {
                    Some(Expr::Uint { value }) => *value,
                    Some(Expr::Var { name }) => obj_to_pin
                        .get(name)
                        .copied()
                        .or_else(|| pin_consts.get(name).copied())
                        .unwrap_or(0) as u64,
                    _ => 0,
                };
                return Expr::Call {
                    name: "GET_BOOL".into(),
                    args: vec![Expr::Uint { value: pin }],
                };
            }
            Expr::Call { name, args }
        }
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(rewrite_expr(*expr, pin_consts, obj_to_pin)),
        },
        Expr::Binop { op, left, right } => Expr::Binop {
            op,
            left: Box::new(rewrite_expr(*left, pin_consts, obj_to_pin)),
            right: Box::new(rewrite_expr(*right, pin_consts, obj_to_pin)),
        },
        other => other,
    }
}

fn refine_hardware(hardware: &mut [HardwareEntry], setup: &[Stmt]) {
    for stmt in setup {
        if let Stmt::Call { name, args } = stmt {
            let pin = match args.first() {
                Some(Expr::Uint { value }) => *value as u32,
                _ => continue,
            };
            if let Some(h) = hardware.iter_mut().find(|h| h.pin == pin) {
                match name.as_str() {
                    "CFG_DIGITAL_INPUT"
                    | "CFG_DIGITAL_INPUT_PULLUP"
                    | "CFG_DIGITAL_INPUT_PULLDOWN" => {
                        h.io_mode = IoMode::Di;
                        h.target_role = "digital-in".into();
                    }
                    "CFG_DIGITAL_OUTPUT" => {
                        h.io_mode = IoMode::Do;
                        h.target_role = "digital-out".into();
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Parse MicroPython source into the shared ASL [`Program`] AST.
pub fn parse_micropython(src: &str) -> Result<Program, MpyParseError> {
    let mut p = Parser::new(src);
    p.parse_program()
}
