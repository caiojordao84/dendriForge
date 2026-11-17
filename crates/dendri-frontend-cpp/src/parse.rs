//! Arduino-style C++ (MVP subset) → ASL AST.

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::{ElifArm, Stmt};
use dendri_ast::{
    DataEntry, DataType, FunctionDef, HardwareEntry, HwType, IoMode, Metadata, Program, Scope,
};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CppParseError {
    #[error("C++ parse error at {line}:{col}: {message}")]
    At {
        line: usize,
        col: usize,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Uint(u64),
    /// Preserves ASL `Expr::Hex` spelling (`0x3C`) through C++ round-trip.
    Hex(String),
    #[allow(dead_code)]
    Int(i64),
    Real(f64),
    String(String),
    True,
    False,
    High,
    Low,
    Output,
    Input,
    InputPullup,
    InputPulldown,
    Assign,
    Semi,
    Comma,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Not,
    BitNot,
    PlusPlus,
    Dot,
    Eof,
}

struct Lexer<'a> {
    src: &'a [u8],
    i: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            i: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.i).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.src.get(self.i + 1).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn skip_ws_comments(&mut self) {
        loop {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                self.bump();
            }
            if self.peek() == Some(b'/') && self.peek2() == Some(b'/') {
                while self.peek().is_some() && self.peek() != Some(b'\n') {
                    self.bump();
                }
                continue;
            }
            if self.peek() == Some(b'/') && self.peek2() == Some(b'*') {
                self.bump();
                self.bump();
                while self.i + 1 < self.src.len() {
                    if self.src[self.i] == b'*' && self.src[self.i + 1] == b'/' {
                        self.bump();
                        self.bump();
                        break;
                    }
                    self.bump();
                }
                continue;
            }
            break;
        }
    }

    fn next_tok(&mut self) -> Result<(Tok, usize, usize), CppParseError> {
        self.skip_ws_comments();
        let line = self.line;
        let col = self.col;
        let Some(c) = self.peek() else {
            return Ok((Tok::Eof, line, col));
        };
        match c {
            b'=' => {
                self.bump();
                if self.peek() == Some(b'=') {
                    self.bump();
                    Ok((Tok::Eq, line, col))
                } else {
                    Ok((Tok::Assign, line, col))
                }
            }
            b'!' => {
                self.bump();
                if self.peek() == Some(b'=') {
                    self.bump();
                    Ok((Tok::Ne, line, col))
                } else {
                    Ok((Tok::Not, line, col))
                }
            }
            b'<' => {
                self.bump();
                if self.peek() == Some(b'=') {
                    self.bump();
                    Ok((Tok::Le, line, col))
                } else {
                    Ok((Tok::Lt, line, col))
                }
            }
            b'>' => {
                self.bump();
                if self.peek() == Some(b'=') {
                    self.bump();
                    Ok((Tok::Ge, line, col))
                } else {
                    Ok((Tok::Gt, line, col))
                }
            }
            b'&' => {
                self.bump();
                if self.peek() == Some(b'&') {
                    self.bump();
                    Ok((Tok::AndAnd, line, col))
                } else {
                    Err(CppParseError::At {
                        line,
                        col,
                        message: "bitwise & not in MVP subset".into(),
                    })
                }
            }
            b'|' => {
                self.bump();
                if self.peek() == Some(b'|') {
                    self.bump();
                    Ok((Tok::OrOr, line, col))
                } else {
                    Err(CppParseError::At {
                        line,
                        col,
                        message: "bitwise | not in MVP subset".into(),
                    })
                }
            }
            b'+' => {
                self.bump();
                if self.peek() == Some(b'+') {
                    self.bump();
                    Ok((Tok::PlusPlus, line, col))
                } else {
                    Ok((Tok::Plus, line, col))
                }
            }
            b'.' => {
                self.bump();
                Ok((Tok::Dot, line, col))
            }
            b'-' => {
                self.bump();
                Ok((Tok::Minus, line, col))
            }
            b'*' => {
                self.bump();
                Ok((Tok::Star, line, col))
            }
            b'/' => {
                self.bump();
                Ok((Tok::Slash, line, col))
            }
            b'%' => {
                self.bump();
                Ok((Tok::Percent, line, col))
            }
            b'~' => {
                self.bump();
                Ok((Tok::BitNot, line, col))
            }
            b';' => {
                self.bump();
                Ok((Tok::Semi, line, col))
            }
            b',' => {
                self.bump();
                Ok((Tok::Comma, line, col))
            }
            b'(' => {
                self.bump();
                Ok((Tok::LParen, line, col))
            }
            b')' => {
                self.bump();
                Ok((Tok::RParen, line, col))
            }
            b'{' => {
                self.bump();
                Ok((Tok::LBrace, line, col))
            }
            b'}' => {
                self.bump();
                Ok((Tok::RBrace, line, col))
            }
            b'"' => {
                self.bump();
                let mut s = String::new();
                while let Some(ch) = self.peek() {
                    if ch == b'"' {
                        self.bump();
                        break;
                    }
                    if ch == b'\\' {
                        self.bump();
                        if let Some(n) = self.bump() {
                            s.push(n as char);
                        }
                        continue;
                    }
                    s.push(ch as char);
                    self.bump();
                }
                Ok((Tok::String(s), line, col))
            }
            c if c.is_ascii_digit() => {
                // Hex literals: 0xA5 / 0X3C — keep spelling for Expr::Hex RT.
                if c == b'0' && matches!(self.peek2(), Some(b'x' | b'X')) {
                    self.bump(); // 0
                    let x = self.bump(); // x/X
                    let mut hex = String::from("0");
                    hex.push(x.unwrap() as char);
                    while let Some(ch) = self.peek() {
                        if ch.is_ascii_hexdigit() {
                            hex.push(ch as char);
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    if hex.len() <= 2 {
                        return Err(CppParseError::At {
                            line,
                            col,
                            message: "expected hex digits after 0x".into(),
                        });
                    }
                    // optional UL/ul/L suffix
                    if matches!(self.peek(), Some(b'U' | b'u' | b'L' | b'l')) {
                        self.bump();
                        if matches!(self.peek(), Some(b'L' | b'l' | b'U' | b'u')) {
                            self.bump();
                        }
                    }
                    return Ok((Tok::Hex(hex), line, col));
                }
                let mut num = String::new();
                let mut is_real = false;
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_digit() {
                        num.push(ch as char);
                        self.bump();
                    } else if ch == b'.' && !is_real {
                        is_real = true;
                        num.push('.');
                        self.bump();
                    } else {
                        break;
                    }
                }
                // optional UL/ul/L suffix
                if matches!(self.peek(), Some(b'U' | b'u' | b'L' | b'l')) {
                    self.bump();
                    if matches!(self.peek(), Some(b'L' | b'l' | b'U' | b'u')) {
                        self.bump();
                    }
                }
                if is_real {
                    let v: f64 = num.parse().map_err(|_| CppParseError::At {
                        line,
                        col,
                        message: format!("bad real {num}"),
                    })?;
                    Ok((Tok::Real(v), line, col))
                } else {
                    let v: u64 = num.parse().map_err(|_| CppParseError::At {
                        line,
                        col,
                        message: format!("bad int {num}"),
                    })?;
                    Ok((Tok::Uint(v), line, col))
                }
            }
            b'\'' => {
                // Char literal → single-char string (SERIAL_READLN sugar uses '\n').
                self.bump();
                let ch = match self.bump() {
                    Some(b'\\') => match self.bump() {
                        Some(b'n') => '\n',
                        Some(b't') => '\t',
                        Some(b'r') => '\r',
                        Some(b'0') => '\0',
                        Some(b'\\') => '\\',
                        Some(b'\'') => '\'',
                        Some(other) => other as char,
                        None => {
                            return Err(CppParseError::At {
                                line,
                                col,
                                message: "unterminated char escape".into(),
                            });
                        }
                    },
                    Some(c) => c as char,
                    None => {
                        return Err(CppParseError::At {
                            line,
                            col,
                            message: "unterminated char literal".into(),
                        });
                    }
                };
                if self.peek() != Some(b'\'') {
                    return Err(CppParseError::At {
                        line,
                        col,
                        message: "expected closing ' in char literal".into(),
                    });
                }
                self.bump();
                Ok((Tok::String(ch.to_string()), line, col))
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let mut id = String::new();
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_alphanumeric() || ch == b'_' {
                        id.push(ch as char);
                        self.bump();
                    } else {
                        break;
                    }
                }
                let tok = match id.as_str() {
                    "true" => Tok::True,
                    "false" => Tok::False,
                    "HIGH" => Tok::High,
                    "LOW" => Tok::Low,
                    "OUTPUT" => Tok::Output,
                    "INPUT" => Tok::Input,
                    "INPUT_PULLUP" => Tok::InputPullup,
                    "INPUT_PULLDOWN" => Tok::InputPulldown,
                    _ => Tok::Ident(id),
                };
                Ok((tok, line, col))
            }
            other => Err(CppParseError::At {
                line,
                col,
                message: format!("unexpected {:?}", other as char),
            }),
        }
    }
}

#[derive(Debug, Clone)]
struct HwAnno {
    pin: u32,
    hw_type: HwType,
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

struct Parser {
    tokens: Vec<(Tok, usize, usize)>,
    pos: usize,
    project_name: Option<String>,
    pin_consts: HashMap<String, u32>,
    hw_annos: HashMap<String, HwAnno>,
    data_annos: HashMap<String, DataAnno>,
}

impl Parser {
    fn from_src(src: &str) -> Result<Self, CppParseError> {
        let mut project_name = None;
        let mut hw_annos = HashMap::new();
        let mut data_annos = HashMap::new();
        for line in src.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("// project:") {
                project_name = Some(rest.trim().to_string());
            }
            if let Some(rest) = t.strip_prefix("// ASL_HW:") {
                if let Some(hw) = parse_asl_hw_anno(rest.trim()) {
                    hw_annos.insert(hw.label.clone(), hw);
                }
            }
            if let Some(rest) = t.strip_prefix("// ASL_DATA:") {
                if let Some((name, anno)) = parse_asl_data_anno(rest.trim()) {
                    data_annos.insert(name, anno);
                }
            }
        }
        let mut lex = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lex.next_tok()?;
            let done = tok == Tok::Eof;
            tokens.push((tok, line, col));
            if done {
                break;
            }
        }
        Ok(Self {
            tokens,
            pos: 0,
            project_name,
            pin_consts: HashMap::new(),
            hw_annos,
            data_annos,
        })
    }

    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].0
    }

    fn loc(&self) -> (usize, usize) {
        (self.tokens[self.pos].1, self.tokens[self.pos].2)
    }

    fn bump(&mut self) -> Tok {
        let t = self.tokens[self.pos].0.clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn err(&self, message: impl Into<String>) -> CppParseError {
        let (line, col) = self.loc();
        CppParseError::At {
            line,
            col,
            message: message.into(),
        }
    }

    fn check_ident(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Ident(id) if id == s)
    }

    fn parse_translation_unit(&mut self) -> Result<Program, CppParseError> {
        let mut data = Vec::new();
        let mut hardware = Vec::new();
        let mut setup = Vec::new();
        let mut loop_body = Vec::new();
        let mut functions = Vec::new();

        while !matches!(self.peek(), Tok::Eof) {
            // const int ledPin = 13;
            // const unsigned long ON_TIME = 100UL;
            // bool ledState = false;
            // void setup() { ... }
            // void loop() { ... }
            // bool foo(...) { ... }

            if self.check_ident("void") || self.check_ident("bool") || self.check_ident("int") {
                // Could be function or var. Lookahead: type name ( name )
                let saved = self.pos;
                let ret_ty = match self.bump() {
                    Tok::Ident(s) => s,
                    _ => unreachable!(),
                };
                let name = match self.bump() {
                    Tok::Ident(s) => s,
                    other => {
                        self.pos = saved;
                        return Err(self.err(format!("expected name after type, got {other:?}")));
                    }
                };
                if matches!(self.peek(), Tok::LParen) {
                    let params = self.parse_param_list()?;
                    self.expect_lbrace()?;
                    let body = self.parse_block_body()?;
                    if name == "setup" {
                        setup = body;
                    } else if name == "loop" {
                        loop_body = body;
                    } else {
                        functions.push(FunctionDef {
                            name,
                            params,
                            body,
                        });
                    }
                    let _ = ret_ty;
                    continue;
                }
                // variable: restore and parse as decl starting at type
                self.pos = saved;
            }

            if self.check_ident("const")
                || self.check_ident("bool")
                || self.check_ident("int")
                || self.check_ident("unsigned")
                || self.check_ident("float")
                || self.check_ident("String")
            {
                let (entry, pin_opt) = self.parse_decl()?;
                if let Some((label, pin)) = pin_opt {
                    self.pin_consts.insert(label.clone(), pin);
                    if let Some(anno) = self.hw_annos.get(&label) {
                        hardware.push(HardwareEntry {
                            pin: anno.pin,
                            hw_type: anno.hw_type,
                            io_mode: anno.io_mode,
                            label: anno.label.clone(),
                            target_role: anno.target_role.clone(),
                        });
                    } else {
                        hardware.push(HardwareEntry {
                            pin,
                            hw_type: HwType::Digital,
                            io_mode: IoMode::Do,
                            label,
                            target_role: "digital-out".into(),
                        });
                    }
                } else {
                    data.push(entry);
                }
                continue;
            }

            return Err(self.err(format!("unexpected top-level token {:?}", self.peek())));
        }

        let setup = rewrite_stmts(setup, &self.pin_consts);
        let loop_body = rewrite_stmts(loop_body, &self.pin_consts);
        let functions = functions
            .into_iter()
            .map(|mut f| {
                f.body = rewrite_stmts(f.body, &self.pin_consts);
                f
            })
            .collect::<Vec<_>>();

        refine_hardware_from_setup(&mut hardware, &setup);

        // If no pin consts, infer hardware from CFG_* calls.
        if hardware.is_empty() {
            hardware = infer_hardware_from_calls(&setup);
        }

        Ok(Program {
            metadata: Metadata {
                asl_version: Some(dendri_ast::ASL_VERSION.into()),
                project_name: self
                    .project_name
                    .clone()
                    .or_else(|| Some("cpp-program".into())),
                editor: Some("dendriForge".into()),
                author: None,
                notes: Some("parsed from C++ frontend (MVP Arduino subset)".into()),
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

    fn parse_param_list(&mut self) -> Result<Vec<String>, CppParseError> {
        match self.bump() {
            Tok::LParen => {}
            _ => return Err(self.err("expected '('")),
        }
        let mut params = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                // type
                let _ = self.bump();
                let name = match self.bump() {
                    Tok::Ident(s) => s,
                    _ => return Err(self.err("expected param name")),
                };
                params.push(name);
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        match self.bump() {
            Tok::RParen => Ok(params),
            _ => Err(self.err("expected ')'")),
        }
    }

    fn expect_lbrace(&mut self) -> Result<(), CppParseError> {
        match self.bump() {
            Tok::LBrace => Ok(()),
            _ => Err(self.err("expected '{'")),
        }
    }

    fn parse_block_body(&mut self) -> Result<Vec<Stmt>, CppParseError> {
        let mut body = Vec::new();
        while !matches!(self.peek(), Tok::RBrace | Tok::Eof) {
            body.push(self.parse_stmt()?);
        }
        match self.bump() {
            Tok::RBrace => Ok(body),
            _ => Err(self.err("expected '}'")),
        }
    }

    fn parse_decl(&mut self) -> Result<(DataEntry, Option<(String, u32)>), CppParseError> {
        let mut is_const = false;
        if self.check_ident("const") {
            self.bump();
            is_const = true;
        }
        // type: unsigned long | unsigned int | bool | int | float | String
        let ty_tok = self.bump();
        let mut type_name = match ty_tok {
            Tok::Ident(s) => s,
            _ => return Err(self.err("expected type")),
        };
        if type_name == "unsigned" {
            match self.bump() {
                Tok::Ident(s) => {
                    type_name = format!("unsigned {s}");
                }
                _ => return Err(self.err("expected long/int after unsigned")),
            }
        }
        let name = match self.bump() {
            Tok::Ident(s) => s,
            _ => return Err(self.err("expected declarator name")),
        };
        match self.bump() {
            Tok::Assign => {}
            _ => return Err(self.err("expected '=' in declaration")),
        }
        let init_expr = self.parse_expr()?;
        match self.bump() {
            Tok::Semi => {}
            _ => return Err(self.err("expected ';' after declaration")),
        }

        // Pin constant: ASL_HW annotation, or legacy `*Pin` const int label.
        if is_const && (type_name == "int" || type_name == "unsigned int") {
            if let Expr::Uint { value } = init_expr {
                let is_pin = self.hw_annos.contains_key(&name) || name.ends_with("Pin");
                if is_pin {
                    return Ok((
                        DataEntry {
                            name: name.clone(),
                            data_type: DataType::Uint,
                            scope: Scope::Const,
                            init: value.to_string(),
                            unit: "-".into(),
                        },
                        Some((name, value as u32)),
                    ));
                }
            }
        }

        let mut data_type;
        let mut init;
        let mut unit;
        match type_name.as_str() {
            "bool" => {
                let v = match init_expr {
                    Expr::Bool { value: true } => "TRUE",
                    _ => "FALSE",
                };
                data_type = DataType::Bool;
                init = v.to_string();
                unit = "-".into();
            }
            "unsigned long" => {
                let ms = match init_expr {
                    Expr::Uint { value } => value,
                    _ => 0,
                };
                if is_const {
                    data_type = DataType::Time;
                    init = format!("T#{ms}ms");
                    unit = "ms".into();
                } else {
                    data_type = DataType::Uint;
                    init = ms.to_string();
                    unit = "-".into();
                }
            }
            "unsigned int" => {
                let n = match init_expr {
                    Expr::Uint { value } => value,
                    _ => 0,
                };
                data_type = DataType::Uint;
                init = n.to_string();
                unit = "-".into();
            }
            "float" => {
                data_type = DataType::Real;
                init = match init_expr {
                    Expr::Real { value } => format_real_init(value),
                    Expr::Uint { value } => format_real_init(value as f64),
                    Expr::Int { value } => format_real_init(value as f64),
                    Expr::Unary {
                        op: UnaryOp::Neg,
                        expr,
                    } => match *expr {
                        Expr::Real { value } => format_real_init(-value),
                        Expr::Uint { value } => format_real_init(-(value as f64)),
                        Expr::Int { value } => format_real_init(-(value as f64)),
                        _ => "0.0".into(),
                    },
                    _ => "0.0".into(),
                };
                unit = "-".into();
            }
            "int" => {
                data_type = DataType::Int;
                init = match init_expr {
                    Expr::Uint { value } => value.to_string(),
                    Expr::Int { value } => value.to_string(),
                    _ => "0".into(),
                };
                unit = "-".into();
            }
            "String" => {
                data_type = DataType::String;
                init = match init_expr {
                    Expr::String { value } => format!("\"{value}\""),
                    _ => "\"\"".into(),
                };
                unit = "-".into();
            }
            _ => {
                data_type = DataType::Uint;
                init = "0".into();
                unit = "-".into();
            }
        };

        let mut scope = if is_const { Scope::Const } else { Scope::Var };
        if let Some(anno) = self.data_annos.get(&name) {
            data_type = anno.data_type;
            scope = anno.scope;
            unit = anno.unit.clone();
            if data_type == DataType::Time {
                if let Ok(ms) = init.parse::<u64>() {
                    init = format!("T#{ms}ms");
                }
            }
        }

        Ok((
            DataEntry {
                name,
                data_type,
                scope,
                init,
                unit,
            },
            None,
        ))
    }

    fn parse_stmt(&mut self) -> Result<Stmt, CppParseError> {
        if self.check_ident("if") {
            return self.parse_if();
        }
        if self.check_ident("while") {
            return self.parse_while();
        }
        if self.check_ident("for") {
            return self.parse_for();
        }
        if self.check_ident("return") {
            self.bump();
            if matches!(self.peek(), Tok::Semi) {
                self.bump();
                return Ok(Stmt::Return { value: None });
            }
            let value = self.parse_expr()?;
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';'")),
            }
            return Ok(Stmt::Return {
                value: Some(value),
            });
        }
        if self.check_ident("break") {
            self.bump();
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';'")),
            }
            return Ok(Stmt::Exit);
        }
        if self.check_ident("continue") {
            self.bump();
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';'")),
            }
            return Ok(Stmt::Continue);
        }

        // int i = ...; local decl → ignore as assign after introducing TEMP — MVP: skip type
        if self.check_ident("int")
            || self.check_ident("bool")
            || self.check_ident("unsigned")
            || self.check_ident("float")
        {
            // local var: treat as assign if form `type name = expr;`
            let _ = self.bump();
            if self.check_ident("long") || self.check_ident("int") {
                self.bump();
            }
            let name = match self.bump() {
                Tok::Ident(s) => s,
                _ => return Err(self.err("expected local name")),
            };
            match self.bump() {
                Tok::Assign => {}
                _ => return Err(self.err("expected '='")),
            }
            let value = self.parse_expr()?;
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';'")),
            }
            return Ok(Stmt::Assign {
                target: name,
                value,
            });
        }

        // assign or call (including Serial.println / Wire.begin member calls)
        let name = match self.bump() {
            Tok::Ident(s) => s,
            other => return Err(self.err(format!("expected statement, got {other:?}"))),
        };

        if matches!(self.peek(), Tok::Assign) {
            self.bump();
            let value = self.parse_expr()?;
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';'")),
            }
            return Ok(Stmt::Assign {
                target: name,
                value,
            });
        }

        if matches!(self.peek(), Tok::Dot) {
            self.bump();
            let method = match self.bump() {
                Tok::Ident(m) => m,
                _ => return Err(self.err("expected method name after '.'")),
            };
            let args = self.parse_arg_list()?;
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';' after member call")),
            }
            return Ok(lower_member_call_stmt(&name, &method, args));
        }

        if matches!(self.peek(), Tok::LParen) {
            let args = self.parse_arg_list()?;
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';' after call")),
            }
            return Ok(lower_call_stmt(&name, args));
        }

        Err(self.err(format!("unexpected after {name}")))
    }

    fn parse_if(&mut self) -> Result<Stmt, CppParseError> {
        self.bump(); // if
        match self.bump() {
            Tok::LParen => {}
            _ => return Err(self.err("expected '(' after if")),
        }
        let condition = self.parse_expr()?;
        match self.bump() {
            Tok::RParen => {}
            _ => return Err(self.err("expected ')'")),
        }
        let then = self.parse_braced_or_single()?;
        let mut elif = Vec::new();
        let mut else_body = None;
        while self.check_ident("else") {
            self.bump();
            if self.check_ident("if") {
                self.bump();
                match self.bump() {
                    Tok::LParen => {}
                    _ => return Err(self.err("expected '('")),
                }
                let cond = self.parse_expr()?;
                match self.bump() {
                    Tok::RParen => {}
                    _ => return Err(self.err("expected ')'")),
                }
                let body = self.parse_braced_or_single()?;
                elif.push(ElifArm {
                    condition: cond,
                    then: body,
                });
            } else {
                else_body = Some(self.parse_braced_or_single()?);
                break;
            }
        }
        Ok(Stmt::If {
            condition,
            then,
            elif,
            else_body,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, CppParseError> {
        self.bump();
        match self.bump() {
            Tok::LParen => {}
            _ => return Err(self.err("expected '('")),
        }
        let condition = self.parse_expr()?;
        match self.bump() {
            Tok::RParen => {}
            _ => return Err(self.err("expected ')'")),
        }
        let body = self.parse_braced_or_single()?;
        Ok(Stmt::While { condition, body })
    }

    fn parse_for(&mut self) -> Result<Stmt, CppParseError> {
        // for (int i = 0; i < N; i++) { body }
        self.bump();
        match self.bump() {
            Tok::LParen => {}
            _ => return Err(self.err("expected '('")),
        }
        // init
        let counter = if self.check_ident("int") {
            self.bump();
            let name = match self.bump() {
                Tok::Ident(s) => s,
                _ => return Err(self.err("expected counter")),
            };
            match self.bump() {
                Tok::Assign => {}
                _ => return Err(self.err("expected '='")),
            }
            let _ = self.parse_expr()?;
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';'")),
            }
            Some(name)
        } else {
            match self.bump() {
                Tok::Semi => None,
                _ => return Err(self.err("MVP for requires int init or empty")),
            }
        };
        // condition: i < expr  (parse add-expr, not full expr — `<` is the for-bound op)
        let to = if matches!(self.peek(), Tok::Semi) {
            self.bump();
            Expr::Uint { value: 0 }
        } else {
            let _lhs = self.parse_add()?;
            let op = self.bump();
            let bound = self.parse_expr()?;
            match self.bump() {
                Tok::Semi => {}
                _ => return Err(self.err("expected ';'")),
            }
            match op {
                Tok::Lt => bound,
                Tok::Le => Expr::Binop {
                    op: BinaryOp::Add,
                    left: Box::new(bound),
                    right: Box::new(Expr::Uint { value: 1 }),
                },
                _ => return Err(self.err("MVP for condition must use < or <=")),
            }
        };
        // incr
        while !matches!(self.peek(), Tok::RParen | Tok::Eof) {
            self.bump();
        }
        match self.bump() {
            Tok::RParen => {}
            _ => return Err(self.err("expected ')'")),
        }
        let body = self.parse_braced_or_single()?;
        Ok(Stmt::For { counter, to, body })
    }

    fn parse_braced_or_single(&mut self) -> Result<Vec<Stmt>, CppParseError> {
        if matches!(self.peek(), Tok::LBrace) {
            self.bump();
            self.parse_block_body()
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, CppParseError> {
        match self.bump() {
            Tok::LParen => {}
            _ => return Err(self.err("expected '('")),
        }
        let mut args = Vec::new();
        if !matches!(self.peek(), Tok::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if matches!(self.peek(), Tok::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        match self.bump() {
            Tok::RParen => Ok(args),
            _ => Err(self.err("expected ')'")),
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, CppParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, CppParseError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Tok::OrOr) {
            self.bump();
            let right = self.parse_and()?;
            left = Expr::Binop {
                op: BinaryOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, CppParseError> {
        let mut left = self.parse_cmp()?;
        while matches!(self.peek(), Tok::AndAnd) {
            self.bump();
            let right = self.parse_cmp()?;
            left = Expr::Binop {
                op: BinaryOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_cmp(&mut self) -> Result<Expr, CppParseError> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek() {
                Tok::Eq => BinaryOp::Eq,
                Tok::Ne => BinaryOp::Ne,
                Tok::Lt => BinaryOp::Lt,
                Tok::Le => BinaryOp::Le,
                Tok::Gt => BinaryOp::Gt,
                Tok::Ge => BinaryOp::Ge,
                _ => break,
            };
            self.bump();
            let right = self.parse_add()?;
            left = Expr::Binop {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, CppParseError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinaryOp::Add,
                Tok::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.bump();
            let right = self.parse_mul()?;
            left = Expr::Binop {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, CppParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinaryOp::Mul,
                Tok::Slash => BinaryOp::Div,
                Tok::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.bump();
            let right = self.parse_unary()?;
            left = Expr::Binop {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, CppParseError> {
        match self.peek() {
            Tok::Not => {
                self.bump();
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(self.parse_unary()?),
                })
            }
            Tok::Minus => {
                self.bump();
                let inner = self.parse_unary()?;
                // Fold numeric literals so `CFG_TFT(..., -1, ...)` and
                // `const float TEMP_OFFSET = -40.0` round-trip cleanly.
                match inner {
                    Expr::Uint { value } if value <= i64::MAX as u64 => Ok(Expr::Int {
                        value: -(value as i64),
                    }),
                    Expr::Int { value } => Ok(Expr::Int { value: -value }),
                    Expr::Real { value } => Ok(Expr::Real { value: -value }),
                    other => Ok(Expr::Unary {
                        op: UnaryOp::Neg,
                        expr: Box::new(other),
                    }),
                }
            }
            Tok::BitNot => {
                self.bump();
                Ok(Expr::Unary {
                    op: UnaryOp::BitNot,
                    expr: Box::new(self.parse_unary()?),
                })
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, CppParseError> {
        match self.bump() {
            Tok::True | Tok::High => Ok(Expr::Bool { value: true }),
            Tok::False | Tok::Low => Ok(Expr::Bool { value: false }),
            Tok::Output => Ok(Expr::Var {
                name: "OUTPUT".into(),
            }),
            Tok::Input => Ok(Expr::Var {
                name: "INPUT".into(),
            }),
            Tok::InputPullup => Ok(Expr::Var {
                name: "INPUT_PULLUP".into(),
            }),
            Tok::InputPulldown => Ok(Expr::Var {
                name: "INPUT_PULLDOWN".into(),
            }),
            Tok::Uint(v) => Ok(Expr::Uint { value: v }),
            Tok::Hex(v) => Ok(Expr::Hex { value: v }),
            Tok::Int(v) => Ok(Expr::Int { value: v }),
            Tok::Real(v) => Ok(Expr::Real { value: v }),
            Tok::String(v) => Ok(Expr::String { value: v }),
            Tok::LParen => {
                // ternary: (cond) ? HIGH : LOW  — or grouping
                let e = self.parse_expr()?;
                match self.bump() {
                    Tok::RParen => {
                        if self.check_ident("") {
                            // no
                        }
                        // ternary after (expr) ?
                        // Actually `(` expr `)` already done. Check for `?`
                        // Our lexer doesn't have `?` — handle in rewrite of digitalWrite args.
                        Ok(e)
                    }
                    _ => Err(self.err("expected ')'")),
                }
            }
            Tok::Ident(name) => {
                if matches!(self.peek(), Tok::Dot) {
                    self.bump();
                    let method = match self.bump() {
                        Tok::Ident(m) => m,
                        _ => return Err(self.err("expected method after '.'")),
                    };
                    let args = self.parse_arg_list()?;
                    Ok(lower_member_call_expr(&name, &method, args))
                } else if matches!(self.peek(), Tok::LParen) {
                    let args = self.parse_arg_list()?;
                    Ok(lower_call_expr(&name, args))
                } else {
                    Ok(Expr::Var { name })
                }
            }
            other => Err(self.err(format!("expected expression, got {other:?}"))),
        }
    }
}

fn lower_member_call_stmt(object: &str, method: &str, args: Vec<Expr>) -> Stmt {
    match (object, method) {
        ("Serial", "begin") => Stmt::Call {
            name: "CFG_SERIAL".into(),
            args,
        },
        ("Serial", "print") => Stmt::Call {
            name: "SERIAL_PRINT".into(),
            args,
        },
        ("Serial", "println") => Stmt::Call {
            name: "SERIAL_PRINTLN".into(),
            args,
        },
        ("Serial", "flush") => Stmt::Call {
            name: "SERIAL_FLUSH".into(),
            args,
        },
        ("Wire", "begin") => {
            // Wire.begin(sda, scl) — bus=0, freq default 100000 when omitted.
            let mut a = vec![Expr::Uint { value: 0 }];
            if args.len() >= 2 {
                a.push(args[0].clone());
                a.push(args[1].clone());
                a.push(args.get(2).cloned().unwrap_or(Expr::Int { value: 100_000 }));
            } else {
                a.extend([
                    Expr::Uint { value: 21 },
                    Expr::Uint { value: 22 },
                    Expr::Int { value: 100_000 },
                ]);
            }
            Stmt::Call {
                name: "CFG_I2C".into(),
                args: a,
            }
        }
        ("SPI", "begin") => {
            // SPI.begin(sck, miso, mosi, cs) ESP32 order → ASL CFG_SPI(bus,mosi,miso,sck,cs)
            if args.len() >= 4 {
                Stmt::Call {
                    name: "CFG_SPI".into(),
                    args: vec![
                        Expr::Uint { value: 1 },
                        args[2].clone(), // mosi
                        args[1].clone(), // miso
                        args[0].clone(), // sck
                        args[3].clone(), // cs
                    ],
                }
            } else {
                Stmt::Call {
                    name: "CFG_SPI".into(),
                    args: vec![
                        Expr::Uint { value: 1 },
                        Expr::Uint { value: 23 },
                        Expr::Int { value: 19 },
                        Expr::Uint { value: 18 },
                        Expr::Uint { value: 5 },
                    ],
                }
            }
        }
        _ => Stmt::Call {
            name: format!("{object}_{method}"),
            args,
        },
    }
}

fn lower_member_call_expr(object: &str, method: &str, args: Vec<Expr>) -> Expr {
    match (object, method) {
        ("Serial", "available") => Expr::Call {
            name: "SERIAL_AVAILABLE".into(),
            args,
        },
        ("Serial", "read") => Expr::Call {
            name: "SERIAL_READ".into(),
            args,
        },
        ("Serial", "readStringUntil") => Expr::Call {
            name: "SERIAL_READLN".into(),
            args: vec![],
        },
        _ => Expr::Call {
            name: format!("{object}_{method}"),
            args,
        },
    }
}

fn lower_call_stmt(name: &str, args: Vec<Expr>) -> Stmt {
    match name {
        "pinMode" => {
            let pin = args.first().cloned().unwrap_or(Expr::Uint { value: 0 });
            let mode = args.get(1);
            let opcode = match mode {
                Some(Expr::Var { name }) if name == "OUTPUT" => "CFG_DIGITAL_OUTPUT",
                Some(Expr::Var { name }) if name == "INPUT_PULLUP" => "CFG_DIGITAL_INPUT_PULLUP",
                Some(Expr::Var { name }) if name == "INPUT_PULLDOWN" => {
                    "CFG_DIGITAL_INPUT_PULLDOWN"
                }
                Some(Expr::Var { name }) if name == "INPUT" => "CFG_DIGITAL_INPUT",
                _ => "CFG_DIGITAL_OUTPUT",
            };
            let opcode = match mode {
                Some(Expr::Var { name: m }) => match m.as_str() {
                    "OUTPUT" => "CFG_DIGITAL_OUTPUT",
                    "INPUT_PULLUP" => "CFG_DIGITAL_INPUT_PULLUP",
                    "INPUT_PULLDOWN" => "CFG_DIGITAL_INPUT_PULLDOWN",
                    "INPUT" => "CFG_DIGITAL_INPUT",
                    _ => opcode,
                },
                _ => opcode,
            };
            Stmt::Call {
                name: opcode.into(),
                args: vec![pin],
            }
        }
        "digitalWrite" => Stmt::Call {
            name: "SET_BOOL".into(),
            args: vec![
                args.first().cloned().unwrap_or(Expr::Uint { value: 0 }),
                args.get(1)
                    .cloned()
                    .unwrap_or(Expr::Bool { value: false }),
            ],
        },
        "delay" => Stmt::Call {
            name: "WAIT_TIME".into(),
            args: vec![args.first().cloned().unwrap_or(Expr::Uint { value: 0 })],
        },
        "delayMicroseconds" => Stmt::Call {
            name: "WAIT_TIME_US".into(),
            args,
        },
        "analogWrite" => Stmt::Call {
            name: "SET_PWM_DUTY".into(),
            args,
        },
        "VENDOR_EXTENSION" => {
            let reason = match args.first() {
                Some(Expr::String { value }) => value.clone(),
                Some(other) => format!("{other:?}"),
                None => String::new(),
            };
            Stmt::VendorExtension { reason }
        }
        other => Stmt::Call {
            name: other.into(),
            args,
        },
    }
}

fn lower_call_expr(name: &str, args: Vec<Expr>) -> Expr {
    match name {
        "digitalRead" => Expr::Call {
            name: "GET_BOOL".into(),
            args,
        },
        "analogRead" => Expr::Call {
            name: "READ_ADC".into(),
            args,
        },
        "millis" => Expr::Call {
            name: "GET_SYS_TIME".into(),
            args: vec![],
        },
        "micros" => Expr::Call {
            name: "GET_SYS_TIME_US".into(),
            args: vec![],
        },
        other => Expr::Call {
            name: other.into(),
            args,
        },
    }
}

fn rewrite_stmts(stmts: Vec<Stmt>, pins: &HashMap<String, u32>) -> Vec<Stmt> {
    stmts
        .into_iter()
        .map(|s| rewrite_stmt(s, pins))
        .collect()
}

fn rewrite_stmt(stmt: Stmt, pins: &HashMap<String, u32>) -> Stmt {
    match stmt {
        Stmt::Assign { target, value } => Stmt::Assign {
            target,
            value: rewrite_expr(value, pins),
        },
        Stmt::Call { name, args } => Stmt::Call {
            name,
            args: args.into_iter().map(|a| rewrite_expr(a, pins)).collect(),
        },
        Stmt::If {
            condition,
            then,
            elif,
            else_body,
        } => Stmt::If {
            condition: rewrite_expr(condition, pins),
            then: rewrite_stmts(then, pins),
            elif: elif
                .into_iter()
                .map(|a| ElifArm {
                    condition: rewrite_expr(a.condition, pins),
                    then: rewrite_stmts(a.then, pins),
                })
                .collect(),
            else_body: else_body.map(|b| rewrite_stmts(b, pins)),
        },
        Stmt::While { condition, body } => Stmt::While {
            condition: rewrite_expr(condition, pins),
            body: rewrite_stmts(body, pins),
        },
        Stmt::For { counter, to, body } => Stmt::For {
            counter,
            to: rewrite_expr(to, pins),
            body: rewrite_stmts(body, pins),
        },
        Stmt::Return { value } => Stmt::Return {
            value: value.map(|v| rewrite_expr(v, pins)),
        },
        other => other,
    }
}

fn rewrite_expr(expr: Expr, pins: &HashMap<String, u32>) -> Expr {
    match expr {
        Expr::Var { name } => {
            if let Some(&pin) = pins.get(&name) {
                Expr::Uint { value: pin as u64 }
            } else {
                Expr::Var { name }
            }
        }
        Expr::Call { name, args } => Expr::Call {
            name,
            args: args.into_iter().map(|a| rewrite_expr(a, pins)).collect(),
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(rewrite_expr(*expr, pins)),
        },
        Expr::Binop { op, left, right } => Expr::Binop {
            op,
            left: Box::new(rewrite_expr(*left, pins)),
            right: Box::new(rewrite_expr(*right, pins)),
        },
        other => other,
    }
}

fn refine_hardware_from_setup(hardware: &mut [HardwareEntry], setup: &[Stmt]) {
    for stmt in setup {
        if let Stmt::Call { name, args } = stmt {
            let pin = match args.first() {
                Some(Expr::Uint { value }) => *value as u32,
                _ => continue,
            };
            if let Some(h) = hardware.iter_mut().find(|h| h.pin == pin) {
                // Preserve ASL_HW `role=` when present; only refine io_mode from CFG_*.
                match name.as_str() {
                    "CFG_DIGITAL_INPUT"
                    | "CFG_DIGITAL_INPUT_PULLUP"
                    | "CFG_DIGITAL_INPUT_PULLDOWN" => {
                        h.io_mode = IoMode::Di;
                    }
                    "CFG_DIGITAL_OUTPUT" => {
                        h.io_mode = IoMode::Do;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn infer_hardware_from_calls(setup: &[Stmt]) -> Vec<HardwareEntry> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for stmt in setup {
        if let Stmt::Call { name, args } = stmt {
            let pin = match args.first() {
                Some(Expr::Uint { value }) => *value as u32,
                _ => continue,
            };
            if !seen.insert(pin) {
                continue;
            }
            let (io_mode, role) = match name.as_str() {
                "CFG_DIGITAL_OUTPUT" => (IoMode::Do, "digital-out"),
                "CFG_DIGITAL_INPUT"
                | "CFG_DIGITAL_INPUT_PULLUP"
                | "CFG_DIGITAL_INPUT_PULLDOWN" => (IoMode::Di, "digital-in"),
                _ => continue,
            };
            out.push(HardwareEntry {
                pin,
                hw_type: HwType::Digital,
                io_mode,
                label: format!("pin{pin}"),
                target_role: role.into(),
            });
        }
    }
    out
}

fn parse_asl_hw_anno(body: &str) -> Option<HwAnno> {
    let mut pin = None;
    let mut label = String::new();
    let mut role = String::from("digital-out");
    let mut io_mode = IoMode::Do;
    let mut hw_type = HwType::Digital;
    for part in body.split_whitespace() {
        if let Some(v) = part.strip_prefix("pin=") {
            pin = v.parse().ok();
        } else if let Some(v) = part.strip_prefix("label=") {
            label = v.to_string();
        } else if let Some(v) = part.strip_prefix("role=") {
            role = v.to_string();
        } else if let Some(v) = part.strip_prefix("type=") {
            hw_type = match v {
                "Analog" | "analog" => HwType::Analog,
                "Bool" | "bool" | "BOOL" => HwType::Bool,
                _ => HwType::Digital,
            };
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
        hw_type,
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

fn format_real_init(value: f64) -> String {
    let s = format!("{value}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{value:.1}")
    }
}

/// Parse Arduino-style C++ source into the shared ASL [`Program`] AST.
pub fn parse_cpp(src: &str) -> Result<Program, CppParseError> {
    let mut p = Parser::from_src(src)?;
    p.parse_translation_unit()
}
