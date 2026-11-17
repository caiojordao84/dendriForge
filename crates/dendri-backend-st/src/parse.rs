//! Structured Text (IEC 61131-3 subset) → ASL AST.

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::{ElifArm, Stmt};
use dendri_ast::{
    DataEntry, DataType, FunctionDef, HardwareEntry, HwType, IoMode, Metadata, Program, Scope,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StParseError {
    #[error("ST parse error at {line}:{col}: {message}")]
    At {
        line: usize,
        col: usize,
        message: String,
    },
    #[error("ST parse error: {0}")]
    Message(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Uint(u64),
    /// ASL `Expr::Hex` spelling (`16#3C` or `0x3C` surface).
    Hex(String),
    #[allow(dead_code)]
    Int(i64),
    Real(f64),
    String(String),
    Time(String),
    True,
    False,
    Assign, // :=
    Semi,
    Colon,
    Comma,
    LParen,
    RParen,
    Plus,
    Minus,
    Star,
    Slash,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
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
            if self.peek() == Some(b'(') && self.src.get(self.i + 1) == Some(&b'*') {
                self.bump();
                self.bump();
                while self.i + 1 < self.src.len() {
                    if self.src[self.i] == b'*' && self.src[self.i + 1] == b')' {
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

    fn next_tok(&mut self) -> Result<(Tok, usize, usize), StParseError> {
        self.skip_ws_comments();
        let line = self.line;
        let col = self.col;
        let Some(c) = self.peek() else {
            return Ok((Tok::Eof, line, col));
        };
        match c {
            b':' => {
                self.bump();
                if self.peek() == Some(b'=') {
                    self.bump();
                    Ok((Tok::Assign, line, col))
                } else {
                    Ok((Tok::Colon, line, col))
                }
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
            b'+' => {
                self.bump();
                Ok((Tok::Plus, line, col))
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
            b'=' => {
                self.bump();
                Ok((Tok::Eq, line, col))
            }
            b'<' => {
                self.bump();
                if self.peek() == Some(b'>') {
                    self.bump();
                    Ok((Tok::Ne, line, col))
                } else if self.peek() == Some(b'=') {
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
            b'\'' => {
                self.bump();
                let mut s = String::new();
                while let Some(ch) = self.peek() {
                    if ch == b'\'' {
                        self.bump();
                        break;
                    }
                    s.push(ch as char);
                    self.bump();
                }
                Ok((Tok::String(s), line, col))
            }
            b'"' => {
                self.bump();
                let mut s = String::new();
                while let Some(ch) = self.peek() {
                    if ch == b'"' {
                        self.bump();
                        break;
                    }
                    s.push(ch as char);
                    self.bump();
                }
                Ok((Tok::String(s), line, col))
            }
            b'T' | b't' if self.src.get(self.i + 1) == Some(&b'#') => {
                let mut s = String::new();
                s.push(self.bump().unwrap() as char);
                s.push(self.bump().unwrap() as char);
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_alphanumeric() {
                        s.push(ch as char);
                        self.bump();
                    } else {
                        break;
                    }
                }
                Ok((Tok::Time(s), line, col))
            }
            c if c.is_ascii_digit() => {
                // Accept C-style 0xNN so ASL Hex exprs round-trip through ST emit.
                if c == b'0' && matches!(self.peek2(), Some(b'x' | b'X')) {
                    self.bump();
                    let x = self.bump();
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
                        return Err(StParseError::At {
                            line,
                            col,
                            message: "expected hex digits after 0x".into(),
                        });
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
                if is_real {
                    let v: f64 = num.parse().map_err(|_| StParseError::At {
                        line,
                        col,
                        message: format!("bad real {num}"),
                    })?;
                    Ok((Tok::Real(v), line, col))
                } else {
                    let v: u64 = num.parse().map_err(|_| StParseError::At {
                        line,
                        col,
                        message: format!("bad uint {num}"),
                    })?;
                    Ok((Tok::Uint(v), line, col))
                }
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
                let upper = id.to_ascii_uppercase();
                let tok = match upper.as_str() {
                    "TRUE" => Tok::True,
                    "FALSE" => Tok::False,
                    _ => Tok::Ident(id),
                };
                Ok((tok, line, col))
            }
            other => Err(StParseError::At {
                line,
                col,
                message: format!("unexpected character {:?}", other as char),
            }),
        }
    }
}

struct DataAnno {
    data_type: Option<DataType>,
    scope: Option<Scope>,
    unit: Option<String>,
}

struct Parser {
    tokens: Vec<(Tok, usize, usize)>,
    pos: usize,
    project_name: Option<String>,
    hardware: Vec<HardwareEntry>,
    data_annos: std::collections::HashMap<String, DataAnno>,
}

impl Parser {
    fn from_src(src: &str) -> Result<Self, StParseError> {
        let mut project_name = None;
        // Pull project name from `(* project: NAME *)` comments before lexing strips them.
        for line in src.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("(* project:") {
                if let Some(name) = rest.strip_suffix("*)") {
                    project_name = Some(name.trim().to_string());
                }
            }
        }
        let mut hardware = Vec::new();
        let mut data_annos = std::collections::HashMap::new();
        for line in src.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("(* HW:") {
                if let Some(body) = rest.strip_suffix("*)") {
                    if let Some(hw) = parse_hw_comment(body.trim()) {
                        hardware.push(hw);
                    }
                }
            }
            if let Some(rest) = t.strip_prefix("(* ASL_DATA:") {
                if let Some(body) = rest.strip_suffix("*)") {
                    if let Some((name, anno)) = parse_asl_data_comment(body.trim()) {
                        data_annos.insert(name, anno);
                    }
                }
            }
        }

        let mut lex = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let (tok, line, col) = lex.next_tok()?;
            let is_eof = tok == Tok::Eof;
            tokens.push((tok, line, col));
            if is_eof {
                break;
            }
        }
        Ok(Self {
            tokens,
            pos: 0,
            project_name,
            hardware,
            data_annos,
        })
    }

    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].0
    }

    fn loc(&self) -> (usize, usize) {
        let (_, line, col) = self.tokens[self.pos];
        (line, col)
    }

    fn bump(&mut self) -> Tok {
        let tok = self.tokens[self.pos].0.clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn err(&self, message: impl Into<String>) -> StParseError {
        let (line, col) = self.loc();
        StParseError::At {
            line,
            col,
            message: message.into(),
        }
    }

    fn expect_ident_kw(&mut self, kw: &str) -> Result<(), StParseError> {
        match self.bump() {
            Tok::Ident(s) if s.eq_ignore_ascii_case(kw) => Ok(()),
            other => Err(self.err(format!("expected {kw}, got {other:?}"))),
        }
    }

    fn check_ident_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s.eq_ignore_ascii_case(kw))
    }

    fn parse_program(&mut self) -> Result<Program, StParseError> {
        self.expect_ident_kw("PROGRAM")?;
        let _name = match self.bump() {
            Tok::Ident(s) => s,
            _ => return Err(self.err("expected program name")),
        };

        let mut data = Vec::new();
        if self.check_ident_kw("VAR") {
            self.bump();
            while !self.check_ident_kw("END_VAR") && !matches!(self.peek(), Tok::Eof) {
                data.push(self.parse_var_decl()?);
            }
            self.expect_ident_kw("END_VAR")?;
        }

        let mut setup = Vec::new();
        let mut loop_body = Vec::new();
        let mut functions = Vec::new();

        // Prefer explicit SETUP / LOOP section keywords (Dendri emit).
        // Also accept a flat statement list (heuristic split) for hand-written ST.

        if self.check_ident_kw("SETUP") {
            self.bump();
            while !self.check_ident_kw("LOOP")
                && !self.check_ident_kw("FUNCTION")
                && !self.check_ident_kw("END_PROGRAM")
                && !matches!(self.peek(), Tok::Eof)
            {
                setup.push(self.parse_stmt()?);
            }
        }

        if self.check_ident_kw("LOOP") {
            self.bump();
            while !self.check_ident_kw("FUNCTION")
                && !self.check_ident_kw("END_PROGRAM")
                && !matches!(self.peek(), Tok::Eof)
            {
                loop_body.push(self.parse_stmt()?);
            }
        } else if setup.is_empty() {
            // Flat body: classify with MVP heuristic for blink-class programs.
            let mut stmts = Vec::new();
            while !self.check_ident_kw("FUNCTION")
                && !self.check_ident_kw("END_PROGRAM")
                && !matches!(self.peek(), Tok::Eof)
            {
                stmts.push(self.parse_stmt()?);
            }
            let (s, l) = split_setup_loop(stmts);
            setup = s;
            loop_body = l;
        }

        while self.check_ident_kw("FUNCTION") {
            functions.push(self.parse_function()?);
        }

        self.expect_ident_kw("END_PROGRAM")?;
        // optional trailing semicolon ignored
        if matches!(self.peek(), Tok::Semi) {
            self.bump();
        }

        // Infer hardware from CFG_* if none from comments.
        let mut hardware = self.hardware.clone();
        if hardware.is_empty() {
            hardware = infer_hardware(&setup, &loop_body);
        }

        Ok(Program {
            metadata: Metadata {
                project_name: self.project_name.clone().or_else(|| Some("st-program".into())),
                editor: Some("dendriForge".into()),
                author: None,
                notes: Some("parsed from ST frontend (MVP subset)".into()),
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

    fn parse_var_decl(&mut self) -> Result<DataEntry, StParseError> {
        let mut scope = Scope::Var;
        if self.check_ident_kw("CONSTANT") {
            self.bump();
            scope = Scope::Const;
        } else if self.check_ident_kw("TEMP") {
            self.bump();
            scope = Scope::Temp;
        }
        let name = match self.bump() {
            Tok::Ident(s) => s,
            _ => return Err(self.err("expected variable name")),
        };
        if self.check_ident_kw("RETAIN") {
            self.bump();
            scope = Scope::Retain;
        } else if self.check_ident_kw("PERSISTENT") {
            self.bump();
            scope = Scope::Persistent;
        }
        match self.bump() {
            Tok::Colon => {}
            _ => return Err(self.err("expected ':' in VAR decl")),
        }
        let ty_name = match self.bump() {
            Tok::Ident(s) => s,
            _ => return Err(self.err("expected type")),
        };
        let data_type = DataType::parse(&ty_name.to_ascii_uppercase())
            .ok_or_else(|| self.err(format!("unknown type {ty_name}")))?;
        let mut init = default_init(data_type);
        if matches!(self.peek(), Tok::Assign) {
            self.bump();
            init = self.parse_init_literal(data_type)?;
        }
        if matches!(self.peek(), Tok::Semi) {
            self.bump();
        }
        let anno = self.data_annos.get(&name);
        let scope = anno.and_then(|a| a.scope).unwrap_or(scope);
        let data_type = anno.and_then(|a| a.data_type).unwrap_or(data_type);
        let unit = anno
            .and_then(|a| a.unit.clone())
            .unwrap_or_else(|| {
                if data_type == DataType::Time {
                    "ms".into()
                } else {
                    "-".into()
                }
            });
        Ok(DataEntry {
            name,
            data_type,
            scope,
            init,
            unit,
        })
    }

    fn parse_init_literal(&mut self, ty: DataType) -> Result<String, StParseError> {
        match self.bump() {
            Tok::True => Ok("TRUE".into()),
            Tok::False => Ok("FALSE".into()),
            Tok::Time(t) => Ok(t),
            Tok::Uint(u) => Ok(u.to_string()),
            Tok::Int(i) => Ok(i.to_string()),
            Tok::Real(r) => Ok(format_real_init(r)),
            Tok::Minus => match self.bump() {
                Tok::Real(r) => Ok(format_real_init(-r)),
                Tok::Uint(u) => Ok(format!("-{u}")),
                Tok::Int(i) => Ok((-i).to_string()),
                other => Err(self.err(format!("expected numeric after '-', got {other:?}"))),
            },
            Tok::String(s) => Ok(format!("\"{s}\"")),
            Tok::Ident(s) if s.eq_ignore_ascii_case("TRUE") => Ok("TRUE".into()),
            Tok::Ident(s) if s.eq_ignore_ascii_case("FALSE") => Ok("FALSE".into()),
            other => {
                if ty == DataType::Time {
                    Err(self.err(format!("expected TIME literal, got {other:?}")))
                } else {
                    Err(self.err(format!("expected init literal, got {other:?}")))
                }
            }
        }
    }

    fn parse_function(&mut self) -> Result<FunctionDef, StParseError> {
        self.expect_ident_kw("FUNCTION")?;
        let name = match self.bump() {
            Tok::Ident(s) => s,
            _ => return Err(self.err("expected function name")),
        };
        // optional `: BOOL`
        if matches!(self.peek(), Tok::Colon) {
            self.bump();
            let _ = self.bump();
        }
        let mut params = Vec::new();
        if self.check_ident_kw("VAR_INPUT") {
            self.bump();
            while !self.check_ident_kw("END_VAR") {
                let p = match self.bump() {
                    Tok::Ident(s) => s,
                    _ => return Err(self.err("expected param name")),
                };
                if matches!(self.peek(), Tok::Colon) {
                    self.bump();
                    let _ = self.bump(); // type
                }
                if matches!(self.peek(), Tok::Semi) {
                    self.bump();
                }
                params.push(p);
            }
            self.expect_ident_kw("END_VAR")?;
        }
        let mut body = Vec::new();
        while !self.check_ident_kw("END_FUNCTION") && !matches!(self.peek(), Tok::Eof) {
            body.push(self.parse_stmt()?);
        }
        self.expect_ident_kw("END_FUNCTION")?;
        Ok(FunctionDef { name, params, body })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, StParseError> {
        if self.check_ident_kw("IF") {
            return self.parse_if();
        }
        if self.check_ident_kw("WHILE") {
            return self.parse_while();
        }
        if self.check_ident_kw("FOR") {
            return self.parse_for();
        }
        if self.check_ident_kw("RETURN") {
            self.bump();
            if matches!(self.peek(), Tok::Semi) {
                self.bump();
            }
            return Ok(Stmt::Return { value: None });
        }
        if self.check_ident_kw("EXIT") {
            self.bump();
            if matches!(self.peek(), Tok::Semi) {
                self.bump();
            }
            return Ok(Stmt::Exit);
        }

        // assign or call
        let name = match self.bump() {
            Tok::Ident(s) => s,
            other => return Err(self.err(format!("expected statement, got {other:?}"))),
        };

        if matches!(self.peek(), Tok::Assign) {
            self.bump();
            let value = self.parse_expr()?;
            if matches!(self.peek(), Tok::Semi) {
                self.bump();
            }
            return Ok(Stmt::Assign {
                target: name,
                value,
            });
        }

        if matches!(self.peek(), Tok::LParen) {
            self.bump();
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
                Tok::RParen => {}
                _ => return Err(self.err("expected ')'")),
            }
            if matches!(self.peek(), Tok::Semi) {
                self.bump();
            }
            if name.eq_ignore_ascii_case("VENDOR_EXTENSION") {
                let reason = match args.first() {
                    Some(Expr::String { value }) => value.clone(),
                    Some(other) => format!("{other:?}"),
                    None => String::new(),
                };
                return Ok(Stmt::VendorExtension { reason });
            }
            return Ok(Stmt::Call { name, args });
        }

        Err(self.err(format!("unexpected after ident {name}")))
    }

    fn parse_if(&mut self) -> Result<Stmt, StParseError> {
        self.expect_ident_kw("IF")?;
        let condition = self.parse_expr()?;
        self.expect_ident_kw("THEN")?;
        let mut then = Vec::new();
        while !self.check_ident_kw("ELSIF")
            && !self.check_ident_kw("ELSE")
            && !self.check_ident_kw("END_IF")
        {
            then.push(self.parse_stmt()?);
        }
        let mut elif = Vec::new();
        while self.check_ident_kw("ELSIF") {
            self.bump();
            let cond = self.parse_expr()?;
            self.expect_ident_kw("THEN")?;
            let mut body = Vec::new();
            while !self.check_ident_kw("ELSIF")
                && !self.check_ident_kw("ELSE")
                && !self.check_ident_kw("END_IF")
            {
                body.push(self.parse_stmt()?);
            }
            elif.push(ElifArm {
                condition: cond,
                then: body,
            });
        }
        let else_body = if self.check_ident_kw("ELSE") {
            self.bump();
            let mut body = Vec::new();
            while !self.check_ident_kw("END_IF") {
                body.push(self.parse_stmt()?);
            }
            Some(body)
        } else {
            None
        };
        self.expect_ident_kw("END_IF")?;
        if matches!(self.peek(), Tok::Semi) {
            self.bump();
        }
        Ok(Stmt::If {
            condition,
            then,
            elif,
            else_body,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, StParseError> {
        self.expect_ident_kw("WHILE")?;
        let condition = self.parse_expr()?;
        self.expect_ident_kw("DO")?;
        let mut body = Vec::new();
        while !self.check_ident_kw("END_WHILE") {
            body.push(self.parse_stmt()?);
        }
        self.expect_ident_kw("END_WHILE")?;
        if matches!(self.peek(), Tok::Semi) {
            self.bump();
        }
        Ok(Stmt::While { condition, body })
    }

    fn parse_for(&mut self) -> Result<Stmt, StParseError> {
        self.expect_ident_kw("FOR")?;
        let counter = match self.bump() {
            Tok::Ident(s) => s,
            _ => return Err(self.err("expected FOR counter")),
        };
        match self.bump() {
            Tok::Assign => {}
            _ => return Err(self.err("expected := in FOR")),
        }
        let _from = self.parse_expr()?;
        self.expect_ident_kw("TO")?;
        let to = self.parse_expr()?;
        // Emit writes `TO {exclusive} - 1` for IEC-looking inclusive form.
        // Unwrap that pattern so round-trip keeps the ASL exclusive bound.
        let to = match to {
            Expr::Binop {
                op: BinaryOp::Sub,
                left,
                right,
            } => match *right {
                Expr::Uint { value: 1 } | Expr::Int { value: 1 } => *left,
                other => Expr::Binop {
                    op: BinaryOp::Sub,
                    left,
                    right: Box::new(other),
                },
            },
            other => other,
        };
        self.expect_ident_kw("DO")?;
        let mut body = Vec::new();
        while !self.check_ident_kw("END_FOR") {
            body.push(self.parse_stmt()?);
        }
        self.expect_ident_kw("END_FOR")?;
        if matches!(self.peek(), Tok::Semi) {
            self.bump();
        }
        Ok(Stmt::For {
            counter: Some(counter),
            to,
            body,
        })
    }

    fn parse_expr(&mut self) -> Result<Expr, StParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, StParseError> {
        let mut left = self.parse_and()?;
        while self.check_ident_kw("OR") || self.check_ident_kw("XOR") {
            let op = if self.check_ident_kw("XOR") {
                self.bump();
                BinaryOp::Xor
            } else {
                self.bump();
                BinaryOp::Or
            };
            let right = self.parse_and()?;
            left = Expr::Binop {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, StParseError> {
        let mut left = self.parse_cmp()?;
        while self.check_ident_kw("AND") {
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

    fn parse_cmp(&mut self) -> Result<Expr, StParseError> {
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

    fn parse_add(&mut self) -> Result<Expr, StParseError> {
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

    fn parse_mul(&mut self) -> Result<Expr, StParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinaryOp::Mul,
                Tok::Slash => BinaryOp::Div,
                Tok::Ident(s) if s.eq_ignore_ascii_case("MOD") => BinaryOp::Mod,
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

    fn parse_unary(&mut self) -> Result<Expr, StParseError> {
        if self.check_ident_kw("NOT") {
            self.bump();
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            });
        }
        if matches!(self.peek(), Tok::Minus) {
            self.bump();
            let expr = self.parse_unary()?;
            return Ok(match expr {
                Expr::Uint { value } if value <= i64::MAX as u64 => Expr::Int {
                    value: -(value as i64),
                },
                Expr::Int { value } => Expr::Int { value: -value },
                Expr::Real { value } => Expr::Real { value: -value },
                other => Expr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(other),
                },
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, StParseError> {
        match self.bump() {
            Tok::True => Ok(Expr::Bool { value: true }),
            Tok::False => Ok(Expr::Bool { value: false }),
            Tok::Uint(v) => Ok(Expr::Uint { value: v }),
            Tok::Hex(v) => Ok(Expr::Hex { value: v }),
            Tok::Int(v) => Ok(Expr::Int { value: v }),
            Tok::Real(v) => Ok(Expr::Real { value: v }),
            Tok::String(v) => Ok(Expr::String { value: v }),
            Tok::Time(v) => Ok(Expr::Time { value: v }),
            Tok::LParen => {
                let e = self.parse_expr()?;
                match self.bump() {
                    Tok::RParen => Ok(e),
                    _ => Err(self.err("expected ')'")),
                }
            }
            Tok::Ident(name) => {
                if name.eq_ignore_ascii_case("SHL") || name.eq_ignore_ascii_case("SHR") {
                    match self.bump() {
                        Tok::LParen => {}
                        _ => return Err(self.err("expected '(' after SHL/SHR")),
                    }
                    let left = self.parse_expr()?;
                    match self.bump() {
                        Tok::Comma => {}
                        _ => return Err(self.err("expected ','")),
                    }
                    let right = self.parse_expr()?;
                    match self.bump() {
                        Tok::RParen => {}
                        _ => return Err(self.err("expected ')'")),
                    }
                    let op = if name.eq_ignore_ascii_case("SHL") {
                        BinaryOp::Shl
                    } else {
                        BinaryOp::Shr
                    };
                    return Ok(Expr::Binop {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    });
                }
                if matches!(self.peek(), Tok::LParen) {
                    self.bump();
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
                        Tok::RParen => {}
                        _ => return Err(self.err("expected ')'")),
                    }
                    Ok(Expr::Call { name, args })
                } else {
                    Ok(Expr::Var { name })
                }
            }
            other => Err(self.err(format!("expected expression, got {other:?}"))),
        }
    }
}

fn default_init(ty: DataType) -> String {
    match ty {
        DataType::Bool => "FALSE".into(),
        DataType::Time => "T#0ms".into(),
        DataType::String => "\"\"".into(),
        _ => "0".into(),
    }
}

fn format_real_init(value: f64) -> String {
    let s = format!("{value}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{value:.1}")
    }
}

fn parse_asl_data_comment(body: &str) -> Option<(String, DataAnno)> {
    let mut name = None;
    let mut data_type = None;
    let mut scope = None;
    let mut unit = None;
    for part in body.split_whitespace() {
        if let Some(v) = part.strip_prefix("name=") {
            name = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("type=") {
            data_type = DataType::parse(&v.to_ascii_uppercase());
        } else if let Some(v) = part.strip_prefix("scope=") {
            scope = match v.to_ascii_uppercase().as_str() {
                "CONST" => Some(Scope::Const),
                "VAR" => Some(Scope::Var),
                "RETAIN" => Some(Scope::Retain),
                "PERSISTENT" => Some(Scope::Persistent),
                "TEMP" => Some(Scope::Temp),
                "EXTERN" => Some(Scope::Extern),
                _ => None,
            };
        } else if let Some(v) = part.strip_prefix("unit=") {
            unit = Some(v.to_string());
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

fn parse_hw_comment(body: &str) -> Option<HardwareEntry> {
    // pin=13 type=Digital mode=Do label=ledPin role=digital-out
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
    Some(HardwareEntry {
        pin: pin?,
        hw_type,
        io_mode,
        label: if label.is_empty() {
            format!("pin{pin}", pin = pin.unwrap())
        } else {
            label
        },
        target_role: role,
    })
}

fn infer_hardware(setup: &[Stmt], loop_body: &[Stmt]) -> Vec<HardwareEntry> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for stmt in setup.iter().chain(loop_body.iter()) {
        if let Stmt::Call { name, args } = stmt {
            let pin = match args.first() {
                Some(Expr::Uint { value }) => *value as u32,
                _ => continue,
            };
            if !seen.insert(pin) {
                continue;
            }
            let (io_mode, role, hw_type) = match name.as_str() {
                "CFG_DIGITAL_OUTPUT" => (IoMode::Do, "digital-out", HwType::Digital),
                "CFG_DIGITAL_INPUT" | "CFG_DIGITAL_INPUT_PULLUP" | "CFG_DIGITAL_INPUT_PULLDOWN" => {
                    (IoMode::Di, "digital-in", HwType::Digital)
                }
                "CFG_PWM_OUTPUT" | "SET_PWM_DUTY" | "SET_PWM_FREQ" | "GET_PWM_DUTY" => {
                    (IoMode::Do, "pwm-out", HwType::Digital)
                }
                "CFG_ADC_INPUT" | "READ_ADC" => (IoMode::Di, "analog-in", HwType::Analog),
                "SET_BOOL" | "GET_BOOL" => (IoMode::Do, "digital-out", HwType::Digital),
                // CFG_I2C / CFG_SPI / CFG_SERIAL use non-pin-first args — skip here.
                _ => continue,
            };
            if name.starts_with("CFG_")
                || matches!(
                    name.as_str(),
                    "SET_BOOL" | "GET_BOOL" | "SET_PWM_DUTY" | "READ_ADC"
                )
            {
                out.push(HardwareEntry {
                    pin,
                    hw_type,
                    io_mode,
                    label: format!("pin{pin}"),
                    target_role: role.into(),
                });
            }
        }
    }
    out
}

/// Split a flat statement list into setup vs loop for blink-class programs.
/// Leading CFG_* calls (and an optional initial SET_BOOL) → setup; rest → loop.
fn split_setup_loop(stmts: Vec<Stmt>) -> (Vec<Stmt>, Vec<Stmt>) {
    let mut setup = Vec::new();
    let mut i = 0;
    while i < stmts.len() {
        match &stmts[i] {
            Stmt::Call { name, .. }
                if name.starts_with("CFG_")
                    || (name == "SET_BOOL" && setup.iter().any(|s| {
                        matches!(s, Stmt::Call { name, .. } if name.starts_with("CFG_"))
                    }) && !setup.iter().any(|s| {
                        matches!(s, Stmt::Call { name, .. } if name == "SET_BOOL")
                    })) =>
            {
                setup.push(stmts[i].clone());
                i += 1;
            }
            Stmt::Call { name, .. } if name.starts_with("CFG_") => {
                setup.push(stmts[i].clone());
                i += 1;
            }
            _ => break,
        }
    }
    // Also swallow a first SET_BOOL right after CFGs as setup init.
    if i < stmts.len() {
        if let Stmt::Call { name, .. } = &stmts[i] {
            if name == "SET_BOOL"
                && setup
                    .iter()
                    .any(|s| matches!(s, Stmt::Call { name, .. } if name.starts_with("CFG_")))
                && !setup
                    .iter()
                    .any(|s| matches!(s, Stmt::Call { name, .. } if name == "SET_BOOL"))
            {
                setup.push(stmts[i].clone());
                i += 1;
            }
        }
    }
    let loop_body = stmts[i..].to_vec();
    (setup, loop_body)
}

/// Parse Structured Text source into the shared ASL [`Program`] AST.
pub fn parse_st(src: &str) -> Result<Program, StParseError> {
    let mut p = Parser::from_src(src)?;
    let prog = p.parse_program()?;
    // Promote CONSTANT-looking all-caps TIME/BOOL with only read usage — leave as-is.
    // Fix CONST scope: emit uses `(* CONSTANT *)` stripped; re-mark names that are
    // ALL_CAPS and TIME type as Const for blink fixtures.
    let data = prog
        .data
        .into_iter()
        .map(|mut d| {
            if d.scope == Scope::Var
                && d.name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                && matches!(d.data_type, DataType::Time | DataType::Uint | DataType::Dword)
            {
                d.scope = Scope::Const;
            }
            d
        })
        .collect();
    Ok(Program { data, ..prog })
}
