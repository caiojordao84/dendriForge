//! Minimal surface-text expression parser for `Expr::ExprText` escape hatches.
//!
//! Covers the operators used in Phase 0 goldens (AND / OR / NOT / comparisons /
//! MOD / bare vars). Full expression grammar lands with language frontends.

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};

/// Parse a small ASL expression subset into the shared [`Expr`] AST.
pub fn parse_expr_text(text: &str) -> Result<Expr, String> {
    let tokens = tokenize(text)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_or()?;
    if parser.pos != parser.tokens.len() {
        return Err(format!(
            "trailing tokens in expr_text: {:?}",
            &parser.tokens[parser.pos..]
        ));
    }
    Ok(expr)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Uint(u64),
    Real(f64),
    StringLit(String),
    Not,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
}

fn tokenize(input: &str) -> Result<Vec<Tok>, String> {
    let mut chars = input.chars().peekable();
    let mut out = Vec::new();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        match c {
            '(' => {
                chars.next();
                out.push(Tok::LParen);
            }
            ')' => {
                chars.next();
                out.push(Tok::RParen);
            }
            ',' => {
                chars.next();
                out.push(Tok::Comma);
            }
            '+' => {
                chars.next();
                out.push(Tok::Plus);
            }
            '-' => {
                chars.next();
                out.push(Tok::Minus);
            }
            '*' => {
                chars.next();
                out.push(Tok::Star);
            }
            '/' => {
                chars.next();
                out.push(Tok::Slash);
            }
            '=' => {
                chars.next();
                out.push(Tok::Eq);
            }
            '<' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(Tok::Le);
                } else {
                    out.push(Tok::Lt);
                }
            }
            '>' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(Tok::Ge);
                } else {
                    out.push(Tok::Gt);
                }
            }
            '!' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(Tok::Ne);
                } else {
                    return Err("bare '!' not supported; use NOT".into());
                }
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    chars.next();
                    if d == '"' {
                        break;
                    }
                    if d == '\\' {
                        if let Some(n) = chars.next() {
                            s.push(n);
                        }
                    } else {
                        s.push(d);
                    }
                }
                out.push(Tok::StringLit(s));
            }
            c if c.is_ascii_digit() => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        s.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&'.') {
                    s.push('.');
                    chars.next();
                    while let Some(&d) = chars.peek() {
                        if d.is_ascii_digit() {
                            s.push(d);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    out.push(Tok::Real(s.parse().map_err(|e| format!("real: {e}"))?));
                } else {
                    out.push(Tok::Uint(s.parse().map_err(|e| format!("uint: {e}"))?));
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_alphanumeric() || d == '_' {
                        s.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match s.as_str() {
                    "NOT" | "not" => out.push(Tok::Not),
                    "AND" | "and" => out.push(Tok::And),
                    "OR" | "or" => out.push(Tok::Or),
                    "TRUE" | "true" => out.push(Tok::Ident("TRUE".into())),
                    "FALSE" | "false" => out.push(Tok::Ident("FALSE".into())),
                    _ => out.push(Tok::Ident(s)),
                }
            }
            other => return Err(format!("unexpected char {other:?} in expr_text")),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Or)) {
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

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_cmp()?;
        while matches!(self.peek(), Some(Tok::And)) {
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

    fn parse_cmp(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Eq) => BinaryOp::Eq,
                Some(Tok::Ne) => BinaryOp::Ne,
                Some(Tok::Lt) => BinaryOp::Lt,
                Some(Tok::Le) => BinaryOp::Le,
                Some(Tok::Gt) => BinaryOp::Gt,
                Some(Tok::Ge) => BinaryOp::Ge,
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

    fn parse_add(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinaryOp::Add,
                Some(Tok::Minus) => BinaryOp::Sub,
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

    fn parse_mul(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinaryOp::Mul,
                Some(Tok::Slash) => BinaryOp::Div,
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

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if matches!(self.peek(), Some(Tok::Not)) {
            self.bump();
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            });
        }
        if matches!(self.peek(), Some(Tok::Minus)) {
            self.bump();
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.bump() {
            Some(Tok::LParen) => {
                let e = self.parse_or()?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err("expected ')'".into()),
                }
            }
            Some(Tok::Uint(v)) => Ok(Expr::Uint { value: v }),
            Some(Tok::Real(v)) => Ok(Expr::Real { value: v }),
            Some(Tok::StringLit(v)) => Ok(Expr::String { value: v }),
            Some(Tok::Ident(name)) => {
                if name == "TRUE" {
                    return Ok(Expr::Bool { value: true });
                }
                if name == "FALSE" {
                    return Ok(Expr::Bool { value: false });
                }
                if matches!(self.peek(), Some(Tok::LParen)) {
                    self.bump();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            args.push(self.parse_or()?);
                            match self.peek() {
                                Some(Tok::Comma) => {
                                    self.bump();
                                }
                                Some(Tok::RParen) => break,
                                _ => return Err("expected ',' or ')' in call".into()),
                            }
                        }
                    }
                    match self.bump() {
                        Some(Tok::RParen) => {}
                        _ => return Err("expected ')' after call args".into()),
                    }
                    return Ok(Expr::Call { name, args });
                }
                Ok(Expr::Var { name })
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_not() {
        let e = parse_expr_text("inputState AND NOT biDirState").unwrap();
        match e {
            Expr::Binop { op: BinaryOp::And, .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_mod_call_eq() {
        let e = parse_expr_text("MOD(rebootCount, MILESTONE_CYCLES) = 0").unwrap();
        match e {
            Expr::Binop { op: BinaryOp::Eq, .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}
