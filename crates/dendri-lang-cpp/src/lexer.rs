//! Zero-copy UTF-8-safe Arduino C++ lexer built on `dendri-frontend-core`.
//!
//! Extracts all C++ tokens, preprocessor directives, and verbatim ADR-027 comments
//! (`lex_cpp_comment` + `merge_contiguous_leading_comments`) with zero panics and zero indexing.

use dendri_ast::EolStyle;
use dendri_frontend_core::comments::{
    RawComment, lex_cpp_comment, merge_contiguous_leading_comments,
};
use dendri_frontend_core::cursor::{
    Cursor, LineIndex, detect_eol_style, is_ident_continue, is_ident_start,
};
use dendri_span::{Diagnostic, ErrorCode, Span};

/// Classified Arduino C++ lexical token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Ident(String),
    IntLit {
        value: i128,
        unsigned: bool,
        long_count: u8,
    },
    FloatLit {
        value: f64,
        is_f32: bool,
    },
    CharLit(char),
    StringLit(String),
    /// Preprocessor line (`#include ...`, `#define ...`, `#pragma ...`) or balanced `#if..#endif`.
    Preproc {
        raw: String,
    },
    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semi,
    Comma,
    Colon,
    ColonColon,
    Dot,
    Arrow,
    Question,
    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusPlus,
    MinusMinus,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    AmpAssign,
    PipeAssign,
    CaretAssign,
    ShlAssign,
    ShrAssign,
    EqEq,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,
    AmpAmp,
    PipePipe,
    Bang,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    Eof,
}

/// Spanned C++ token carrying line bounds and leading blank-line count.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub start_line: u32,
    pub end_line: u32,
    pub blank_lines_before: u32,
}

/// Complete lexed representation of a C++ source unit.
#[derive(Debug, Clone)]
pub(crate) struct LexedFile {
    pub tokens: Vec<Token>,
    pub comments: Vec<RawComment>,
    pub eol_style: EolStyle,
}

/// Lexes `src` into tokens and merged ADR-027 verbatim comments.
pub(crate) fn lex_cpp(src: &str, board_id: &str) -> Result<LexedFile, Vec<Diagnostic>> {
    let eol_style = detect_eol_style(src);
    let line_index = LineIndex::new(src);
    let mut cur = Cursor::new(src);
    let mut tokens: Vec<Token> = Vec::new();
    let mut raw_comments: Vec<RawComment> = Vec::new();
    let mut errors: Vec<Diagnostic> = Vec::new();

    let mut last_code_line: Option<u32> = None;

    while !cur.is_eof() {
        let blank_lines = cur.skip_whitespace_and_count_blank_lines();
        if cur.is_eof() {
            break;
        }

        // 1. Comments (`//`, `///`, `//!`, `/* ... */`, `/** ... */`, `/*! ... */`)
        if cur.starts_with("//") || cur.starts_with("/*") {
            let start_pos = cur.pos();
            match lex_cpp_comment(&mut cur) {
                Ok(Some((text, style, span))) => {
                    let start_line = line_index.line_of(span.start);
                    let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
                    let preceded_by_code_on_line = last_code_line == Some(start_line);
                    raw_comments.push(RawComment {
                        text,
                        style,
                        span,
                        start_line,
                        end_line,
                        blank_lines_before: blank_lines,
                        preceded_by_code_on_line,
                    });
                }
                Ok(None) => {}
                Err(mut diag) => {
                    diag = diag.with_board_id(board_id);
                    if diag.span.is_none() {
                        diag.span = Some(cur.span_from(start_pos));
                    }
                    errors.push(diag);
                    break;
                }
            }
            continue;
        }

        // 2. Preprocessor directives (`#...` at start of line or standalone)
        if cur.peek() == Some('#') {
            let start = cur.pos();
            let raw = lex_preproc_directive(&mut cur);
            let span = cur.span_from(start);
            let start_line = line_index.line_of(span.start);
            let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
            last_code_line = Some(end_line);
            tokens.push(Token {
                kind: TokenKind::Preproc { raw },
                span,
                start_line,
                end_line,
                blank_lines_before: blank_lines,
            });
            continue;
        }

        // 3. String literal
        if cur.peek() == Some('"') {
            let start = cur.pos();
            match lex_string_literal(&mut cur, board_id) {
                Ok(s) => {
                    let span = cur.span_from(start);
                    let start_line = line_index.line_of(span.start);
                    let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
                    last_code_line = Some(end_line);
                    tokens.push(Token {
                        kind: TokenKind::StringLit(s),
                        span,
                        start_line,
                        end_line,
                        blank_lines_before: blank_lines,
                    });
                }
                Err(diag) => errors.push(diag),
            }
            continue;
        }

        // 4. Character literal
        if cur.peek() == Some('\'') {
            let start = cur.pos();
            match lex_char_literal(&mut cur, board_id) {
                Ok(ch) => {
                    let span = cur.span_from(start);
                    let start_line = line_index.line_of(span.start);
                    let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
                    last_code_line = Some(end_line);
                    tokens.push(Token {
                        kind: TokenKind::CharLit(ch),
                        span,
                        start_line,
                        end_line,
                        blank_lines_before: blank_lines,
                    });
                }
                Err(diag) => errors.push(diag),
            }
            continue;
        }

        // 5. Numeric literal
        if cur.peek().is_some_and(|c| c.is_ascii_digit()) {
            let start = cur.pos();
            match lex_number_literal(&mut cur, board_id) {
                Ok(kind) => {
                    let span = cur.span_from(start);
                    let start_line = line_index.line_of(span.start);
                    let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
                    last_code_line = Some(end_line);
                    tokens.push(Token {
                        kind,
                        span,
                        start_line,
                        end_line,
                        blank_lines_before: blank_lines,
                    });
                }
                Err(diag) => errors.push(diag),
            }
            continue;
        }

        // 6. Identifier or keyword
        if cur.peek().is_some_and(is_ident_start) {
            if let Some((ident, span)) = cur.eat_ident() {
                let start_line = line_index.line_of(span.start);
                let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
                last_code_line = Some(end_line);
                tokens.push(Token {
                    kind: TokenKind::Ident(ident.to_owned()),
                    span,
                    start_line,
                    end_line,
                    blank_lines_before: blank_lines,
                });
            }
            continue;
        }

        // 7. Punctuators & operators (longest match first)
        let start = cur.pos();
        if let Some(kind) = lex_operator(&mut cur) {
            let span = cur.span_from(start);
            let start_line = line_index.line_of(span.start);
            let end_line = line_index.line_of(span.end.saturating_sub(1).max(span.start));
            last_code_line = Some(end_line);
            tokens.push(Token {
                kind,
                span,
                start_line,
                end_line,
                blank_lines_before: blank_lines,
            });
            continue;
        }

        // Unexpected character
        let bad = cur.bump().unwrap_or('\0');
        let span = cur.span_from(start);
        let mut diag = Diagnostic::new(
            ErrorCode::ErrTomlSyntax,
            format!(
                "unexpected character `{bad}` (U+{:04X}) in C++ source",
                u32::from(bad)
            ),
        )
        .with_board_id(board_id);
        diag.span = Some(span);
        errors.push(diag);
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let eof_pos = cur.pos_u32();
    let eof_line = line_index.line_of(eof_pos);
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::point(eof_pos),
        start_line: eof_line,
        end_line: eof_line,
        blank_lines_before: 0,
    });

    let comments = merge_contiguous_leading_comments(raw_comments);
    Ok(LexedFile {
        tokens,
        comments,
        eol_style,
    })
}

/// Lexes a preprocessor directive, stopping before any trailing `//` or `/*` comment on single-line
/// directives (`#include`, `#define`, `#pragma`, `#undef`), and capturing balanced `#if..#endif` blocks.
fn lex_preproc_directive(cur: &mut Cursor<'_>) -> String {
    let start = cur.pos();
    let cp = cur.checkpoint();
    let _ = cur.eat_char('#');
    let _ = cur.skip_horizontal_whitespace();
    let directive_word = cur.eat_while(is_ident_continue);
    let is_conditional = matches!(directive_word, "if" | "ifdef" | "ifndef");
    cur.restore(cp);

    if is_conditional {
        return lex_conditional_preproc_block(cur);
    }

    // Consume until newline (following backslash line continuations) or trailing comment start.
    let mut in_string = false;
    let mut in_angle_include = directive_word == "include";
    let mut angle_open = false;
    while let Some(ch) = cur.peek() {
        if ch == '\n' || (ch == '\r' && cur.peek_second() == Some('\n')) {
            break;
        }
        if !in_string && !angle_open && (cur.starts_with("//") || cur.starts_with("/*")) {
            break;
        }
        if ch == '\\' {
            let line_cp = cur.checkpoint();
            let _ = cur.bump();
            let _ = cur.skip_horizontal_whitespace();
            if cur.eat_newline().is_some() {
                continue;
            }
            cur.restore(line_cp);
            let _ = cur.bump();
            if in_string {
                let _ = cur.bump();
            }
            continue;
        }
        if ch == '"' && !angle_open {
            in_string = !in_string;
            let _ = cur.bump();
            continue;
        }
        if in_angle_include && !in_string {
            if ch == '<' {
                angle_open = true;
            } else if ch == '>' && angle_open {
                angle_open = false;
                in_angle_include = false;
            }
        }
        let _ = cur.bump();
    }
    cur.slice_from(start)
        .trim_end_matches([' ', '\t'])
        .to_owned()
}

/// Captures a balanced `#if`/`#ifdef`/`#ifndef` ... `#endif` conditional compilation block verbatim.
fn lex_conditional_preproc_block(cur: &mut Cursor<'_>) -> String {
    let start = cur.pos();
    let mut depth = 0usize;
    while !cur.is_eof() {
        let line_start = cur.checkpoint();
        let _ = cur.skip_horizontal_whitespace();
        if cur.eat_char('#') {
            let _ = cur.skip_horizontal_whitespace();
            let word = cur.eat_while(is_ident_continue);
            if matches!(word, "if" | "ifdef" | "ifndef") {
                depth = depth.saturating_add(1);
            } else if word == "endif" {
                depth = depth.saturating_sub(1);
            }
        }
        cur.restore(line_start);
        let _ = cur.eat_line();
        if depth == 0 {
            break;
        }
        if cur.eat_newline().is_none() {
            break;
        }
    }
    cur.slice_from(start).to_owned()
}

fn lex_string_literal(cur: &mut Cursor<'_>, board_id: &str) -> Result<String, Diagnostic> {
    let start = cur.pos();
    let _ = cur.eat_char('"');
    let mut out = String::new();
    while let Some(ch) = cur.bump() {
        match ch {
            '"' => return Ok(out),
            '\n' => {
                let mut diag = Diagnostic::new(
                    ErrorCode::ErrTomlSyntax,
                    "unterminated C++ string literal before end of line",
                )
                .with_board_id(board_id);
                diag.span = Some(cur.span_from(start));
                return Err(diag);
            }
            '\\' => {
                let escaped = lex_escape_char(cur, start, board_id)?;
                out.push(escaped);
            }
            other => out.push(other),
        }
    }
    let mut diag = Diagnostic::new(
        ErrorCode::ErrTomlSyntax,
        "unterminated C++ string literal at end of file",
    )
    .with_board_id(board_id);
    diag.span = Some(cur.span_from(start));
    Err(diag)
}

fn lex_char_literal(cur: &mut Cursor<'_>, board_id: &str) -> Result<char, Diagnostic> {
    let start = cur.pos();
    let _ = cur.eat_char('\'');
    let Some(first) = cur.bump() else {
        let mut diag = Diagnostic::new(
            ErrorCode::ErrTomlSyntax,
            "unterminated C++ character literal",
        )
        .with_board_id(board_id);
        diag.span = Some(cur.span_from(start));
        return Err(diag);
    };
    if first == '\'' || first == '\n' {
        let mut diag = Diagnostic::new(
            ErrorCode::ErrTomlSyntax,
            "empty or unterminated C++ character literal",
        )
        .with_board_id(board_id);
        diag.span = Some(cur.span_from(start));
        return Err(diag);
    }
    let ch = if first == '\\' {
        lex_escape_char(cur, start, board_id)?
    } else {
        first
    };
    if !cur.eat_char('\'') {
        let _ = cur.eat_while(|c| c != '\'' && c != '\n');
        let _ = cur.eat_char('\'');
        let mut diag = Diagnostic::new(
            ErrorCode::ErrTomlSyntax,
            "expected closing `'` in C++ character literal",
        )
        .with_board_id(board_id);
        diag.span = Some(cur.span_from(start));
        return Err(diag);
    }
    Ok(ch)
}

fn lex_escape_char(
    cur: &mut Cursor<'_>,
    lit_start: usize,
    board_id: &str,
) -> Result<char, Diagnostic> {
    let Some(esc) = cur.bump() else {
        let mut diag = Diagnostic::new(
            ErrorCode::ErrTomlSyntax,
            "unterminated escape sequence in literal",
        )
        .with_board_id(board_id);
        diag.span = Some(cur.span_from(lit_start));
        return Err(diag);
    };
    match esc {
        'n' => Ok('\n'),
        'r' => Ok('\r'),
        't' => Ok('\t'),
        '0' => Ok('\0'),
        '\\' => Ok('\\'),
        '"' => Ok('"'),
        '\'' => Ok('\''),
        'a' => Ok('\x07'),
        'b' => Ok('\x08'),
        'f' => Ok('\x0C'),
        'v' => Ok('\x0B'),
        'x' => {
            let hex_digits = cur.eat_while(|c| c.is_ascii_hexdigit());
            let parsed = u32::from_str_radix(hex_digits, 16)
                .ok()
                .and_then(char::from_u32);
            if let Some(c) = parsed {
                Ok(c)
            } else {
                let mut diag = Diagnostic::new(
                    ErrorCode::ErrTomlSyntax,
                    format!("invalid hex escape `\\x{hex_digits}`"),
                )
                .with_board_id(board_id);
                diag.span = Some(cur.span_from(lit_start));
                Err(diag)
            }
        }
        other => {
            let mut diag = Diagnostic::new(
                ErrorCode::ErrTomlSyntax,
                format!("unknown escape sequence `\\{other}`"),
            )
            .with_board_id(board_id);
            diag.span = Some(cur.span_from(lit_start));
            Err(diag)
        }
    }
}

fn lex_number_literal(cur: &mut Cursor<'_>, board_id: &str) -> Result<TokenKind, Diagnostic> {
    let start = cur.pos();

    // Hex: 0x... / 0X...
    if cur.eat_str("0x") || cur.eat_str("0X") {
        let digits = cur.eat_while(|c| c.is_ascii_hexdigit() || c == '\'');
        let cleaned = digits.replace('\'', "");
        let (unsigned, long_count) = lex_int_suffix(cur, start, board_id)?;
        if cleaned.is_empty() {
            return Err(syntax_err(
                "invalid hexadecimal literal: expected hex digits after `0x`",
                cur.span_from(start),
                board_id,
            ));
        }
        let value = i128::from_str_radix(&cleaned, 16).map_err(|_| {
            syntax_err(
                "hexadecimal literal overflows 128-bit range",
                cur.span_from(start),
                board_id,
            )
        })?;
        return Ok(TokenKind::IntLit {
            value,
            unsigned,
            long_count,
        });
    }

    // Binary: 0b... / 0B...
    if cur.eat_str("0b") || cur.eat_str("0B") {
        let digits = cur.eat_while(|c| c.is_ascii_alphanumeric() || c == '\'');
        let cleaned = digits.replace('\'', "");
        if cleaned.is_empty() {
            return Err(syntax_err(
                "invalid binary literal: expected `0` or `1` after `0b`",
                cur.span_from(start),
                board_id,
            ));
        }
        // Separate trailing u/U/l/L suffix from binary digits
        let split_idx = cleaned
            .find(|c: char| !matches!(c, '0' | '1'))
            .unwrap_or(cleaned.len());
        let (bin_part, suf_part) = cleaned.split_at(split_idx);
        if bin_part.is_empty() {
            return Err(syntax_err(
                format!("invalid binary literal `0b{cleaned}`"),
                cur.span_from(start),
                board_id,
            ));
        }
        let (unsigned, long_count) =
            parse_int_suffix_str(suf_part, cur.span_from(start), board_id)?;
        let value = i128::from_str_radix(bin_part, 2).map_err(|_| {
            syntax_err(
                "binary literal overflows 128-bit range",
                cur.span_from(start),
                board_id,
            )
        })?;
        return Ok(TokenKind::IntLit {
            value,
            unsigned,
            long_count,
        });
    }

    // Decimal, octal, or floating-point
    let int_part = cur.eat_while(|c| c.is_ascii_digit() || c == '\'');
    let mut is_float = false;

    if cur.peek() == Some('.') && cur.peek_second().is_some_and(|c| c.is_ascii_digit()) {
        is_float = true;
        let _ = cur.bump();
        let _ = cur.eat_while(|c| c.is_ascii_digit() || c == '\'');
    }

    if matches!(cur.peek(), Some('e' | 'E')) {
        is_float = true;
        let _ = cur.bump();
        if matches!(cur.peek(), Some('+' | '-')) {
            let _ = cur.bump();
        }
        let exp_digits = cur.eat_while(|c| c.is_ascii_digit());
        if exp_digits.is_empty() {
            return Err(syntax_err(
                "invalid floating-point exponent",
                cur.span_from(start),
                board_id,
            ));
        }
    }

    if is_float {
        let num_slice = cur.slice_from(start).replace('\'', "");
        let is_f32 = if matches!(cur.peek(), Some('f' | 'F')) {
            let _ = cur.bump();
            true
        } else {
            if matches!(cur.peek(), Some('l' | 'L')) {
                let _ = cur.bump();
            }
            false
        };
        if cur.peek().is_some_and(is_ident_continue) {
            let _ = cur.eat_while(is_ident_continue);
            return Err(syntax_err(
                "invalid floating-point literal suffix",
                cur.span_from(start),
                board_id,
            ));
        }
        let value = num_slice.parse::<f64>().map_err(|_| {
            syntax_err(
                format!("invalid floating-point literal `{num_slice}`"),
                cur.span_from(start),
                board_id,
            )
        })?;
        return Ok(TokenKind::FloatLit { value, is_f32 });
    }

    let cleaned_int = int_part.replace('\'', "");
    let (unsigned, long_count) = lex_int_suffix(cur, start, board_id)?;

    // Octal literal (`077`, `00`, etc.) when starting with `0` and length > 1
    let value = if cleaned_int.len() > 1 && cleaned_int.starts_with('0') {
        i128::from_str_radix(&cleaned_int, 8).map_err(|_| {
            syntax_err(
                format!("invalid octal integer literal `{cleaned_int}`"),
                cur.span_from(start),
                board_id,
            )
        })?
    } else {
        cleaned_int.parse::<i128>().map_err(|_| {
            syntax_err(
                format!("invalid decimal integer literal `{cleaned_int}`"),
                cur.span_from(start),
                board_id,
            )
        })?
    };

    Ok(TokenKind::IntLit {
        value,
        unsigned,
        long_count,
    })
}

fn lex_int_suffix(
    cur: &mut Cursor<'_>,
    lit_start: usize,
    board_id: &str,
) -> Result<(bool, u8), Diagnostic> {
    let suf = cur.eat_while(is_ident_continue);
    parse_int_suffix_str(suf, cur.span_from(lit_start), board_id)
}

fn parse_int_suffix_str(suf: &str, span: Span, board_id: &str) -> Result<(bool, u8), Diagnostic> {
    match suf {
        "" => Ok((false, 0)),
        "u" | "U" => Ok((true, 0)),
        "l" | "L" => Ok((false, 1)),
        "ll" | "LL" => Ok((false, 2)),
        "ul" | "uL" | "Ul" | "UL" | "lu" | "lU" | "Lu" | "LU" => Ok((true, 1)),
        "ull" | "uLL" | "Ull" | "ULL" | "llu" | "llU" | "LLu" | "LLU" => Ok((true, 2)),
        other => Err(syntax_err(
            format!("invalid integer literal suffix `{other}`"),
            span,
            board_id,
        )),
    }
}

fn lex_operator(cur: &mut Cursor<'_>) -> Option<TokenKind> {
    // 3-char operators
    if cur.eat_str("<<=") {
        return Some(TokenKind::ShlAssign);
    }
    if cur.eat_str(">>=") {
        return Some(TokenKind::ShrAssign);
    }
    // 2-char operators
    if cur.eat_str("::") {
        return Some(TokenKind::ColonColon);
    }
    if cur.eat_str("->") {
        return Some(TokenKind::Arrow);
    }
    if cur.eat_str("++") {
        return Some(TokenKind::PlusPlus);
    }
    if cur.eat_str("--") {
        return Some(TokenKind::MinusMinus);
    }
    if cur.eat_str("+=") {
        return Some(TokenKind::PlusAssign);
    }
    if cur.eat_str("-=") {
        return Some(TokenKind::MinusAssign);
    }
    if cur.eat_str("*=") {
        return Some(TokenKind::StarAssign);
    }
    if cur.eat_str("/=") {
        return Some(TokenKind::SlashAssign);
    }
    if cur.eat_str("%=") {
        return Some(TokenKind::PercentAssign);
    }
    if cur.eat_str("&=") {
        return Some(TokenKind::AmpAssign);
    }
    if cur.eat_str("|=") {
        return Some(TokenKind::PipeAssign);
    }
    if cur.eat_str("^=") {
        return Some(TokenKind::CaretAssign);
    }
    if cur.eat_str("==") {
        return Some(TokenKind::EqEq);
    }
    if cur.eat_str("!=") {
        return Some(TokenKind::BangEq);
    }
    if cur.eat_str("<=") {
        return Some(TokenKind::Le);
    }
    if cur.eat_str(">=") {
        return Some(TokenKind::Ge);
    }
    if cur.eat_str("&&") {
        return Some(TokenKind::AmpAmp);
    }
    if cur.eat_str("||") {
        return Some(TokenKind::PipePipe);
    }
    if cur.eat_str("<<") {
        return Some(TokenKind::Shl);
    }
    if cur.eat_str(">>") {
        return Some(TokenKind::Shr);
    }
    // 1-char operators/punctuators
    let ch = cur.peek()?;
    let kind = match ch {
        '(' => TokenKind::LParen,
        ')' => TokenKind::RParen,
        '{' => TokenKind::LBrace,
        '}' => TokenKind::RBrace,
        '[' => TokenKind::LBracket,
        ']' => TokenKind::RBracket,
        ';' => TokenKind::Semi,
        ',' => TokenKind::Comma,
        ':' => TokenKind::Colon,
        '.' => TokenKind::Dot,
        '?' => TokenKind::Question,
        '+' => TokenKind::Plus,
        '-' => TokenKind::Minus,
        '*' => TokenKind::Star,
        '/' => TokenKind::Slash,
        '%' => TokenKind::Percent,
        '=' => TokenKind::Assign,
        '<' => TokenKind::Lt,
        '>' => TokenKind::Gt,
        '!' => TokenKind::Bang,
        '&' => TokenKind::Amp,
        '|' => TokenKind::Pipe,
        '^' => TokenKind::Caret,
        '~' => TokenKind::Tilde,
        _ => return None,
    };
    let _ = cur.bump();
    Some(kind)
}

fn syntax_err(msg: impl Into<String>, span: Span, board_id: &str) -> Diagnostic {
    let mut diag = Diagnostic::new(ErrorCode::ErrTomlSyntax, msg).with_board_id(board_id);
    diag.span = Some(span);
    diag
}
