//! Zero-copy UTF-8-safe lexer for IEC 60848 / IEC 61131-3 GRAFCET / SFC.
//!
//! Built directly on `dendri_frontend_core::cursor::{Cursor, LineIndex}` and
//! `dendri_frontend_core::comments` with exact byte `Span` tracking, nested
//! `(* ... *)` comments, `//` line comments, IEC radix & `T#...` duration
//! literals, and direct hardware addresses (`%IX0.0`, `%QX0.0`, `%IW64`).

use dendri_ast::CommentStyle;
use dendri_frontend_core::comments::{
    RawComment, lex_nested_paren_comment, merge_contiguous_leading_comments,
};
use dendri_frontend_core::cursor::{Cursor, LineIndex, is_ident_continue, is_ident_start};
use dendri_span::{Diagnostic, ErrorCode, Span};

/// Lexical token kinds in the canonical IEC 60848 / IEC 61131-3 SFC grammar.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    // Literals
    Bool(bool),
    Int(i128),
    Float(f64),
    TimeUs(i64),
    String(String),
    /// IEC direct hardware address (e.g. `%IX0.0`, `%QX0.7`, `%IW64`).
    DirectAddr(String),

    // Identifiers (structural SFC keywords are matched contextually so user
    // variable/step/field names like `n`, `t`, `x`, `from`, `to`, `range` never collide)
    Ident(String),

    // Punctuation & operators
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    DotDot,
    Colon,
    ColonEq,
    Semi,
    Plus,
    Minus,
    Star,
    Slash,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    EqEq,
    Ne,
    BangEq,

    Eof,
}

/// A spanned lexical token paired with its 1-indexed start and end lines.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) span: Span,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
}

impl Token {
    /// Returns `true` iff this token is an identifier matching `kw` ignoring ASCII case.
    pub(crate) fn is_kw(&self, kw: &str) -> bool {
        match &self.kind {
            TokenKind::Ident(s) => s.eq_ignore_ascii_case(kw),
            TokenKind::Bool(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::TimeUs(_)
            | TokenKind::String(_)
            | TokenKind::DirectAddr(_)
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::DotDot
            | TokenKind::Colon
            | TokenKind::ColonEq
            | TokenKind::Semi
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Eq
            | TokenKind::EqEq
            | TokenKind::Ne
            | TokenKind::BangEq
            | TokenKind::Eof => false,
        }
    }
}

/// Complete lexed token stream plus merged ADR-027 raw comments.
#[derive(Debug, Clone)]
pub(crate) struct LexOutput {
    pub(crate) tokens: Vec<Token>,
    pub(crate) comments: Vec<RawComment>,
}

/// Tokenizes `src`, extracting all `//` and nested `(* ... *)` comments verbatim.
pub(crate) fn tokenize(src: &str, board_id: &str) -> Result<LexOutput, Vec<Diagnostic>> {
    let line_index = LineIndex::new(src);
    let mut cur = Cursor::new(src);
    let mut tokens = Vec::new();
    let mut raw_comments = Vec::new();
    let mut diagnostics = Vec::new();
    let mut last_code_end_line: u32 = 0;

    loop {
        let blank_lines_before = cur.skip_whitespace_and_count_blank_lines();
        if cur.is_eof() {
            let pos = cur.pos_u32();
            let line = line_index.line_of(pos);
            tokens.push(Token {
                kind: TokenKind::Eof,
                span: Span::point(pos),
                start_line: line,
                end_line: line,
            });
            break;
        }

        // 1. Nested IEC `(* ... *)` parenthesis block comments
        if cur.starts_with("(*") {
            match lex_nested_paren_comment(&mut cur, true) {
                Ok(Some((text, span))) => {
                    let start_line = line_index.line_of(span.start);
                    let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
                    raw_comments.push(RawComment {
                        text,
                        style: CommentStyle::ParenBlock,
                        span,
                        start_line,
                        end_line,
                        blank_lines_before,
                        preceded_by_code_on_line: last_code_end_line == start_line,
                    });
                }
                Ok(None) => {}
                Err(mut diag) => {
                    diag = diag.with_board_id(board_id);
                    diagnostics.push(diag);
                    break;
                }
            }
            continue;
        }

        // 2. Single-line `// ...` comments (verbatim after exact `//` prefix; no C++ `\` splicing)
        if cur.starts_with("//") {
            let start = cur.pos();
            let _ = cur.eat_str("//");
            let body = cur.eat_line();
            let span = cur.span_from(start);
            let start_line = line_index.line_of(span.start);
            let end_line = start_line;
            raw_comments.push(RawComment {
                text: body.to_owned(),
                style: CommentStyle::Line,
                span,
                start_line,
                end_line,
                blank_lines_before,
                preceded_by_code_on_line: last_code_end_line == start_line,
            });
            continue;
        }

        // 3. Regular SFC tokens
        let start = cur.pos();
        match lex_one_token(&mut cur, board_id) {
            Ok(kind) => {
                let span = cur.span_from(start);
                let start_line = line_index.line_of(span.start);
                let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
                last_code_end_line = end_line;
                tokens.push(Token {
                    kind,
                    span,
                    start_line,
                    end_line,
                });
            }
            Err(diag) => {
                diagnostics.push(diag);
                // Ensure forward progress if cursor did not move
                if cur.pos() == start {
                    let _ = cur.bump();
                }
            }
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let comments = merge_contiguous_leading_comments(raw_comments);
    Ok(LexOutput { tokens, comments })
}

fn syntax_err(board_id: &str, span: Span, msg: impl Into<String>) -> Diagnostic {
    let mut d = Diagnostic::new(ErrorCode::ErrTomlSyntax, msg).with_board_id(board_id);
    d.span = Some(span);
    d
}

fn lex_one_token(cur: &mut Cursor<'_>, board_id: &str) -> Result<TokenKind, Diagnostic> {
    let start = cur.pos();
    let Some(ch) = cur.peek() else {
        return Ok(TokenKind::Eof);
    };

    // IEC TIME literals: `T#...`, `t#...`, `TIME#...`, `time#...`
    if (ch == 'T' || ch == 't') && cur.peek_second() == Some('#') {
        return lex_time_literal(cur, board_id, 2);
    }
    if cur.starts_with_ascii_ignore_case("TIME#") {
        return lex_time_literal(cur, board_id, 5);
    }

    // IEC direct hardware address: `%IX0.0`, `%QX0.7`, `%IW64`, `%QW64`
    if ch == '%' {
        return lex_direct_addr(cur, board_id);
    }

    // Double-quoted or IEC single-quoted string literal
    if ch == '"' {
        return lex_double_string(cur, board_id);
    }
    if ch == '\'' {
        return lex_single_string(cur, board_id);
    }

    // Numeric literal (decimal, float, or IEC radix `16#FF`, `2#1010`)
    if ch.is_ascii_digit() {
        return lex_number(cur, board_id);
    }

    // Identifier or boolean literal
    if is_ident_start(ch) {
        let (ident, _span) = cur
            .eat_ident()
            .ok_or_else(|| syntax_err(board_id, cur.span_from(start), "expected identifier"))?;
        if ident.eq_ignore_ascii_case("true") {
            return Ok(TokenKind::Bool(true));
        }
        if ident.eq_ignore_ascii_case("false") {
            return Ok(TokenKind::Bool(false));
        }
        return Ok(TokenKind::Ident(ident.to_owned()));
    }

    // Punctuation & multi-char operators
    let _ = cur.bump();
    match ch {
        '(' => Ok(TokenKind::LParen),
        ')' => Ok(TokenKind::RParen),
        '[' => Ok(TokenKind::LBracket),
        ']' => Ok(TokenKind::RBracket),
        '{' => Ok(TokenKind::LBrace),
        '}' => Ok(TokenKind::RBrace),
        ',' => Ok(TokenKind::Comma),
        ';' => Ok(TokenKind::Semi),
        '+' => Ok(TokenKind::Plus),
        '-' => Ok(TokenKind::Minus),
        '*' => Ok(TokenKind::Star),
        '/' => Ok(TokenKind::Slash),
        '.' => {
            if cur.eat_char('.') {
                Ok(TokenKind::DotDot)
            } else {
                Ok(TokenKind::Dot)
            }
        }
        ':' => {
            if cur.eat_char('=') {
                Ok(TokenKind::ColonEq)
            } else {
                Ok(TokenKind::Colon)
            }
        }
        '<' => {
            if cur.eat_char('=') {
                Ok(TokenKind::Le)
            } else if cur.eat_char('>') {
                Ok(TokenKind::Ne)
            } else {
                Ok(TokenKind::Lt)
            }
        }
        '>' => {
            if cur.eat_char('=') {
                Ok(TokenKind::Ge)
            } else {
                Ok(TokenKind::Gt)
            }
        }
        '=' => {
            if cur.eat_char('=') {
                Ok(TokenKind::EqEq)
            } else {
                Ok(TokenKind::Eq)
            }
        }
        '!' => {
            if cur.eat_char('=') {
                Ok(TokenKind::BangEq)
            } else {
                Err(syntax_err(
                    board_id,
                    cur.span_from(start),
                    "unexpected `!` in SFC source (use `NOT` for logical negation or `!=`/`<>` for inequality)",
                ))
            }
        }
        other => Err(syntax_err(
            board_id,
            cur.span_from(start),
            format!("unexpected character `{other}` in SFC source"),
        )),
    }
}

fn lex_direct_addr(cur: &mut Cursor<'_>, board_id: &str) -> Result<TokenKind, Diagnostic> {
    let start = cur.pos();
    let _ = cur.bump(); // '%'
    let _ = cur.eat_while(|c| c.is_ascii_alphanumeric() || c == '.');
    let raw = cur.slice_from(start);
    let span = cur.span_from(start);
    if raw.len() < 3
        || raw.ends_with('.')
        || raw.contains("..")
        || !raw
            .as_bytes()
            .get(1)
            .is_some_and(|b| matches!(b.to_ascii_uppercase(), b'I' | b'Q' | b'M'))
    {
        return Err(syntax_err(
            board_id,
            span,
            format!("malformed IEC direct hardware address `{raw}`"),
        ));
    }
    Ok(TokenKind::DirectAddr(raw.to_ascii_uppercase()))
}

fn lex_number(cur: &mut Cursor<'_>, board_id: &str) -> Result<TokenKind, Diagnostic> {
    let start = cur.pos();
    let _ = cur.eat_while(|c| c.is_ascii_digit() || c == '_');

    // IEC radix literal: `16#FF`, `2#1010_0101`, `8#77`, `10#42`
    if cur.peek() == Some('#') {
        let base_str = cur.slice_from(start);
        let _ = cur.bump(); // '#'
        let digits_start = cur.pos();
        let _ = cur.eat_while(|c| c.is_ascii_hexdigit() || c == '_');
        let raw_full = cur.slice_from(start);
        let span = cur.span_from(start);
        let radix: u32 = base_str.parse().map_err(|_| {
            syntax_err(
                board_id,
                span,
                format!("invalid IEC radix literal `{raw_full}`"),
            )
        })?;
        if !matches!(radix, 2 | 8 | 10 | 16) {
            return Err(syntax_err(
                board_id,
                span,
                format!("unsupported IEC radix `{radix}` in `{raw_full}`"),
            ));
        }
        let cleaned: String = cur
            .source()
            .get(digits_start..cur.pos())
            .unwrap_or("")
            .chars()
            .filter(|&c| c != '_')
            .collect();
        if cleaned.is_empty() {
            return Err(syntax_err(
                board_id,
                span,
                format!("missing digits in IEC radix literal `{raw_full}`"),
            ));
        }
        let val = i128::from_str_radix(&cleaned, radix).map_err(|_| {
            syntax_err(
                board_id,
                span,
                format!("invalid digits in IEC radix literal `{raw_full}`"),
            )
        })?;
        return Ok(TokenKind::Int(val));
    }

    // Float fraction (careful not to consume `..` range operator!)
    let mut is_float = false;
    if cur.peek() == Some('.') && cur.peek_second().is_some_and(|c| c.is_ascii_digit()) {
        is_float = true;
        let _ = cur.bump(); // '.'
        let _ = cur.eat_while(|c| c.is_ascii_digit() || c == '_');
    }

    // Optional exponent
    if matches!(cur.peek(), Some('e' | 'E')) {
        is_float = true;
        let _ = cur.bump();
        if matches!(cur.peek(), Some('+' | '-')) {
            let _ = cur.bump();
        }
        let exp_start = cur.pos();
        let _ = cur.eat_while(|c| c.is_ascii_digit() || c == '_');
        if cur.pos() == exp_start {
            return Err(syntax_err(
                board_id,
                cur.span_from(start),
                format!("malformed float exponent in `{}`", cur.slice_from(start)),
            ));
        }
    }

    let raw = cur.slice_from(start);
    let span = cur.span_from(start);
    let cleaned: String = raw.chars().filter(|&c| c != '_').collect();
    if is_float {
        let val: f64 = cleaned
            .parse()
            .map_err(|_| syntax_err(board_id, span, format!("invalid floating literal `{raw}`")))?;
        if !val.is_finite() {
            return Err(syntax_err(
                board_id,
                span,
                format!("non-finite floating literal `{raw}`"),
            ));
        }
        Ok(TokenKind::Float(val))
    } else {
        let val: i128 = cleaned
            .parse()
            .map_err(|_| syntax_err(board_id, span, format!("invalid integer literal `{raw}`")))?;
        Ok(TokenKind::Int(val))
    }
}

fn lex_time_literal(
    cur: &mut Cursor<'_>,
    board_id: &str,
    prefix_len: usize,
) -> Result<TokenKind, Diagnostic> {
    let start = cur.pos();
    for _ in 0..prefix_len {
        let _ = cur.bump();
    }
    let body_start = cur.pos();
    let _ = cur.eat_while(is_ident_continue);
    let full = cur.slice_from(start);
    let body = cur.source().get(body_start..cur.pos()).unwrap_or("");
    let span = cur.span_from(start);
    let total_us = parse_iec_duration_us(body).ok_or_else(|| {
        syntax_err(
            board_id,
            span,
            format!("invalid IEC TIME literal `{full}` (expected e.g. `T#500ms`, `T#1s500ms`)"),
        )
    })?;
    Ok(TokenKind::TimeUs(total_us))
}

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

fn lex_double_string(cur: &mut Cursor<'_>, board_id: &str) -> Result<TokenKind, Diagnostic> {
    let start = cur.pos();
    let _ = cur.bump(); // opening '"'
    let mut out = String::new();
    while let Some(ch) = cur.bump() {
        match ch {
            '"' => return Ok(TokenKind::String(out)),
            '\\' => {
                let esc_start = cur.pos().saturating_sub(1);
                let Some(esc) = cur.bump() else {
                    return Err(syntax_err(
                        board_id,
                        cur.span_from(start),
                        "unterminated string literal",
                    ));
                };
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '0' => out.push('\0'),
                    'u' => {
                        if cur.bump() != Some('{') {
                            return Err(syntax_err(
                                board_id,
                                cur.span_from(esc_start),
                                "invalid unicode escape in string literal",
                            ));
                        }
                        let hex_start = cur.pos();
                        let _ = cur.eat_while(|c| c != '}' && c != '\n' && c != '\r');
                        let hex_str = cur.source().get(hex_start..cur.pos()).unwrap_or("");
                        if cur.bump() != Some('}') || hex_str.is_empty() || hex_str.len() > 6 {
                            return Err(syntax_err(
                                board_id,
                                cur.span_from(esc_start),
                                "invalid unicode escape in string literal",
                            ));
                        }
                        let code = u32::from_str_radix(hex_str, 16).map_err(|_| {
                            syntax_err(
                                board_id,
                                cur.span_from(esc_start),
                                "invalid hex digits in unicode escape",
                            )
                        })?;
                        let decoded = char::from_u32(code).ok_or_else(|| {
                            syntax_err(
                                board_id,
                                cur.span_from(esc_start),
                                "invalid unicode scalar in escape sequence",
                            )
                        })?;
                        out.push(decoded);
                    }
                    _ => {
                        return Err(syntax_err(
                            board_id,
                            cur.span_from(esc_start),
                            format!("unknown escape sequence `\\{esc}` in string literal"),
                        ));
                    }
                }
            }
            '\n' | '\r' => {
                return Err(syntax_err(
                    board_id,
                    cur.span_from(start),
                    "unterminated string literal before newline",
                ));
            }
            other => out.push(other),
        }
    }
    Err(syntax_err(
        board_id,
        cur.span_from(start),
        "unterminated string literal at EOF",
    ))
}

fn lex_single_string(cur: &mut Cursor<'_>, board_id: &str) -> Result<TokenKind, Diagnostic> {
    let start = cur.pos();
    let _ = cur.bump(); // opening '\''
    let mut out = String::new();
    while let Some(ch) = cur.bump() {
        match ch {
            '\'' => {
                // IEC doubled single quote `''` escapes a literal single quote
                if cur.eat_char('\'') {
                    out.push('\'');
                } else {
                    return Ok(TokenKind::String(out));
                }
            }
            '$' => {
                let esc_start = cur.pos().saturating_sub(1);
                let Some(esc) = cur.bump() else {
                    return Err(syntax_err(
                        board_id,
                        cur.span_from(start),
                        "unterminated IEC single-quoted string literal",
                    ));
                };
                match esc {
                    '$' => out.push('$'),
                    '\'' => out.push('\''),
                    'L' | 'l' | 'N' | 'n' => out.push('\n'),
                    'R' | 'r' => out.push('\r'),
                    'T' | 't' => out.push('\t'),
                    other => {
                        return Err(syntax_err(
                            board_id,
                            cur.span_from(esc_start),
                            format!("unknown IEC string escape `${other}`"),
                        ));
                    }
                }
            }
            '\n' | '\r' => {
                return Err(syntax_err(
                    board_id,
                    cur.span_from(start),
                    "unterminated IEC single-quoted string literal before newline",
                ));
            }
            other => out.push(other),
        }
    }
    Err(syntax_err(
        board_id,
        cur.span_from(start),
        "unterminated IEC single-quoted string literal at EOF",
    ))
}
