//! UTF-8-safe lexer for ASL v2 canonical expressions (`01-asl-ir-and-project-schema.md` §4.2).

use dendri_span::Span;
use thiserror::Error;

/// Lexical token kinds in the ASL v2 expression grammar.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Bool(bool),
    Int(i128),
    Float(f64),
    TimeUs(i64),
    String(String),

    // Identifiers & keywords
    Ident(String),
    Foreign,
    Not,
    BitNot,
    Mod,
    Shl,
    Shr,
    BitAnd,
    BitXor,
    BitOr,
    And,
    Xor,
    Or,

    // Punctuation & operators
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Dot,
    ColonEq,
    Plus,
    Minus,
    Star,
    Slash,
    Lt,
    Le,
    Gt,
    Ge,
    EqEq,
    BangEq,

    Eof,
}

/// Spanned lexical token.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Lexer errors with exact byte spans.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LexError {
    #[error("unexpected character `{ch}` at {span}")]
    UnexpectedChar { ch: char, span: Span },
    #[error("unterminated string literal at {span}")]
    UnterminatedString { span: Span },
    #[error("invalid escape sequence in string literal at {span}")]
    InvalidEscape { span: Span },
    #[error("invalid numeric literal `{text}` at {span}")]
    InvalidNumber { text: String, span: Span },
    #[error("invalid IEC time literal `{text}` at {span}")]
    InvalidTimeLiteral { text: String, span: Span },
    #[error("unexpected `{found}` (expected `{expected}`) at {span}")]
    UnexpectedTokenChar {
        found: String,
        expected: &'static str,
        span: Span,
    },
}

/// Returns `true` if `ident` is a reserved keyword in the canonical ASL expression grammar.
#[must_use]
pub fn is_reserved_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "true"
            | "false"
            | "FOREIGN"
            | "NOT"
            | "BIT_NOT"
            | "MOD"
            | "SHL"
            | "SHR"
            | "BIT_AND"
            | "BIT_XOR"
            | "BIT_OR"
            | "AND"
            | "XOR"
            | "OR"
    )
}

/// Tokenizes `src` into a complete vector ending with [`TokenKind::Eof`].
pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    let mut lexer = Lexer::new(src);
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token()?;
        let is_eof = tok.kind == TokenKind::Eof;
        tokens.push(tok);
        if is_eof {
            break;
        }
    }
    Ok(tokens)
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    const fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn remaining(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or("")
    }

    fn peek_char(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn peek_second_char(&self) -> Option<char> {
        let mut iter = self.remaining().chars();
        iter.next()?;
        iter.next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.pos = self.pos.saturating_add(ch.len_utf8());
        Some(ch)
    }

    fn span_from(&self, start: usize) -> Span {
        let s = u32::try_from(start).unwrap_or(u32::MAX);
        let e = u32::try_from(self.pos).unwrap_or(u32::MAX);
        Span::new(s, e)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek_char() {
            if ch.is_whitespace() {
                self.bump_char();
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace();
        let start = self.pos;
        let Some(ch) = self.peek_char() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                span: self.span_from(start),
            });
        };

        // IEC TIME literal: `T#...` or `t#...`
        if (ch == 'T' || ch == 't') && self.peek_second_char() == Some('#') {
            return self.lex_time_literal();
        }

        if ch == '"' {
            return self.lex_string();
        }

        if ch.is_ascii_digit() {
            return self.lex_number();
        }

        if ch.is_ascii_alphabetic() || ch == '_' {
            return Ok(self.lex_ident_or_keyword());
        }

        self.bump_char();
        let kind = match ch {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            ':' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::ColonEq
                } else {
                    return Err(LexError::UnexpectedTokenChar {
                        found: ":".to_owned(),
                        expected: ":=",
                        span: self.span_from(start),
                    });
                }
            }
            '<' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            '=' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::EqEq
                } else {
                    return Err(LexError::UnexpectedTokenChar {
                        found: "=".to_owned(),
                        expected: "==",
                        span: self.span_from(start),
                    });
                }
            }
            '!' => {
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::BangEq
                } else {
                    return Err(LexError::UnexpectedTokenChar {
                        found: "!".to_owned(),
                        expected: "!=",
                        span: self.span_from(start),
                    });
                }
            }
            other => {
                return Err(LexError::UnexpectedChar {
                    ch: other,
                    span: self.span_from(start),
                });
            }
        };

        Ok(Token {
            kind,
            span: self.span_from(start),
        })
    }

    fn lex_ident_or_keyword(&mut self) -> Token {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                self.bump_char();
            } else {
                break;
            }
        }
        let text = self.src.get(start..self.pos).unwrap_or("");
        let kind = match text {
            "true" => TokenKind::Bool(true),
            "false" => TokenKind::Bool(false),
            "FOREIGN" => TokenKind::Foreign,
            "NOT" => TokenKind::Not,
            "BIT_NOT" => TokenKind::BitNot,
            "MOD" => TokenKind::Mod,
            "SHL" => TokenKind::Shl,
            "SHR" => TokenKind::Shr,
            "BIT_AND" => TokenKind::BitAnd,
            "BIT_XOR" => TokenKind::BitXor,
            "BIT_OR" => TokenKind::BitOr,
            "AND" => TokenKind::And,
            "XOR" => TokenKind::Xor,
            "OR" => TokenKind::Or,
            _ => TokenKind::Ident(text.to_owned()),
        };
        Token {
            kind,
            span: self.span_from(start),
        }
    }

    fn lex_number(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_digit() || ch == '_' {
                self.bump_char();
            } else {
                break;
            }
        }

        // IEC radix literal: `16#FF`, `2#1010`, `8#77`
        if self.peek_char() == Some('#') {
            let base_str = self.src.get(start..self.pos).unwrap_or("");
            self.bump_char(); // '#'
            let digits_start = self.pos;
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_hexdigit() || ch == '_' {
                    self.bump_char();
                } else {
                    break;
                }
            }
            let raw_full = self.src.get(start..self.pos).unwrap_or("");
            let span = self.span_from(start);
            let radix: u32 = base_str.parse().map_err(|_| LexError::InvalidNumber {
                text: raw_full.to_owned(),
                span,
            })?;
            if !matches!(radix, 2 | 8 | 10 | 16) {
                return Err(LexError::InvalidNumber {
                    text: raw_full.to_owned(),
                    span,
                });
            }
            let cleaned: String = self
                .src
                .get(digits_start..self.pos)
                .unwrap_or("")
                .chars()
                .filter(|&c| c != '_')
                .collect();
            if cleaned.is_empty() {
                return Err(LexError::InvalidNumber {
                    text: raw_full.to_owned(),
                    span,
                });
            }
            let val =
                i128::from_str_radix(&cleaned, radix).map_err(|_| LexError::InvalidNumber {
                    text: raw_full.to_owned(),
                    span,
                })?;
            return Ok(Token {
                kind: TokenKind::Int(val),
                span,
            });
        }

        // Float fractional and/or exponent part
        let mut is_float = false;
        if self.peek_char() == Some('.')
            && self.peek_second_char().is_some_and(|c| c.is_ascii_digit())
        {
            is_float = true;
            self.bump_char(); // '.'
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_digit() || ch == '_' {
                    self.bump_char();
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek_char(), Some('e' | 'E')) {
            is_float = true;
            self.bump_char();
            if matches!(self.peek_char(), Some('+' | '-')) {
                self.bump_char();
            }
            let exp_start = self.pos;
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_digit() || ch == '_' {
                    self.bump_char();
                } else {
                    break;
                }
            }
            if self.pos == exp_start {
                return Err(LexError::InvalidNumber {
                    text: self.src.get(start..self.pos).unwrap_or("").to_owned(),
                    span: self.span_from(start),
                });
            }
        }

        let raw = self.src.get(start..self.pos).unwrap_or("");
        let span = self.span_from(start);
        let cleaned: String = raw.chars().filter(|&c| c != '_').collect();
        if is_float {
            let val: f64 = cleaned.parse().map_err(|_| LexError::InvalidNumber {
                text: raw.to_owned(),
                span,
            })?;
            if !val.is_finite() {
                return Err(LexError::InvalidNumber {
                    text: raw.to_owned(),
                    span,
                });
            }
            Ok(Token {
                kind: TokenKind::Float(val),
                span,
            })
        } else {
            let val: i128 = cleaned.parse().map_err(|_| LexError::InvalidNumber {
                text: raw.to_owned(),
                span,
            })?;
            Ok(Token {
                kind: TokenKind::Int(val),
                span,
            })
        }
    }

    fn lex_time_literal(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        self.bump_char(); // 'T' / 't'
        self.bump_char(); // '#'
        let body_start = self.pos;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                self.bump_char();
            } else {
                break;
            }
        }
        let full = self.src.get(start..self.pos).unwrap_or("");
        let body = self.src.get(body_start..self.pos).unwrap_or("");
        let span = self.span_from(start);
        let total_us = parse_iec_duration_us(body).ok_or_else(|| LexError::InvalidTimeLiteral {
            text: full.to_owned(),
            span,
        })?;
        Ok(Token {
            kind: TokenKind::TimeUs(total_us),
            span,
        })
    }

    fn lex_string(&mut self) -> Result<Token, LexError> {
        let start = self.pos;
        self.bump_char(); // opening '"'
        let mut out = String::new();
        while let Some(ch) = self.bump_char() {
            match ch {
                '"' => {
                    return Ok(Token {
                        kind: TokenKind::String(out),
                        span: self.span_from(start),
                    });
                }
                '\\' => {
                    let esc_start = self.pos.saturating_sub(1);
                    let Some(esc) = self.bump_char() else {
                        return Err(LexError::UnterminatedString {
                            span: self.span_from(start),
                        });
                    };
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        '0' => out.push('\0'),
                        'u' => {
                            if self.bump_char() != Some('{') {
                                return Err(LexError::InvalidEscape {
                                    span: self.span_from(esc_start),
                                });
                            }
                            let hex_start = self.pos;
                            while let Some(hc) = self.peek_char() {
                                if hc == '}' {
                                    break;
                                }
                                self.bump_char();
                            }
                            let hex_str = self.src.get(hex_start..self.pos).unwrap_or("");
                            if self.bump_char() != Some('}')
                                || hex_str.is_empty()
                                || hex_str.len() > 6
                            {
                                return Err(LexError::InvalidEscape {
                                    span: self.span_from(esc_start),
                                });
                            }
                            let code = u32::from_str_radix(hex_str, 16).map_err(|_| {
                                LexError::InvalidEscape {
                                    span: self.span_from(esc_start),
                                }
                            })?;
                            let decoded =
                                char::from_u32(code).ok_or_else(|| LexError::InvalidEscape {
                                    span: self.span_from(esc_start),
                                })?;
                            out.push(decoded);
                        }
                        _ => {
                            return Err(LexError::InvalidEscape {
                                span: self.span_from(esc_start),
                            });
                        }
                    }
                }
                '\n' | '\r' => {
                    return Err(LexError::UnterminatedString {
                        span: self.span_from(start),
                    });
                }
                other => out.push(other),
            }
        }
        Err(LexError::UnterminatedString {
            span: self.span_from(start),
        })
    }
}

/// Parses compound IEC durations like `500us`, `1s500ms`, `2m`, `1h30m` into microseconds.
fn parse_iec_duration_us(body: &str) -> Option<i64> {
    if body.is_empty() {
        return None;
    }
    let bytes = body.as_bytes();
    let mut i = 0usize;
    let mut total: i64 = 0;
    let mut parsed_any = false;

    while i < bytes.len() {
        let num_start = i;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
        {
            i = i.saturating_add(1);
        }
        if i == num_start {
            return None;
        }
        let cleaned: String = body
            .get(num_start..i)?
            .chars()
            .filter(|&c| c != '_')
            .collect();
        let amount: i64 = cleaned.parse().ok()?;

        let unit_start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
            i = i.saturating_add(1);
        }
        let unit = body.get(unit_start..i)?;
        let mult: i64 = match unit {
            "us" | "US" => 1,
            "ms" | "MS" => 1_000,
            "s" | "S" => 1_000_000,
            "m" | "M" => 60_000_000,
            "h" | "H" => 3_600_000_000,
            "d" | "D" => 86_400_000_000,
            _ => return None,
        };
        let term = amount.checked_mul(mult)?;
        total = total.checked_add(term)?;
        parsed_any = true;
    }

    parsed_any.then_some(total)
}
