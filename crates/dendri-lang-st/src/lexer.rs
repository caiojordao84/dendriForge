//! Zero-copy UTF-8-safe lexer for IEC 61131-3 Structured Text (`docs/lbc/st.md` §1.1, §4).
//!
//! Uses `dendri-frontend-core`'s [`Cursor`], [`LineIndex`], and [`lex_st_comment`] so
//! all four `DEF-006` [`StCommentDialect`] variants and `ADR-027` verbatim comments
//! (`C1–C5`) are handled consistently with zero panics or unchecked indexing.

use dendri_ast::{EolStyle, ScalarType};
use dendri_frontend_core::{
    Cursor, LineIndex, RawComment, StCommentDialect, detect_eol_style, is_ident_continue,
    is_ident_start, lex_st_comment, merge_contiguous_leading_comments,
};
use dendri_span::{Diagnostic, ErrorCode, Span};

/// Lexical token kind for IEC 61131-3 Structured Text (`asl-core@1` + Ed. 3 `Foreign` hooks).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    // Module / top-level keywords
    Using,
    Namespace,
    EndNamespace,
    Type,
    EndType,
    Struct,
    EndStruct,
    Program,
    EndProgram,
    OrganizationBlock,
    EndOrganizationBlock,
    Function,
    EndFunction,
    FunctionBlock,
    EndFunctionBlock,

    // Variable block & qualifier keywords
    VarInput,
    VarOutput,
    VarInOut,
    Var,
    VarTemp,
    VarGlobal,
    EndVar,
    Constant,
    Retain,
    Persistent,
    At,

    // Type constructor keywords
    Array,
    Of,

    // Control-flow keywords
    If,
    Then,
    Elsif,
    Else,
    EndIf,
    Case,
    EndCase,
    For,
    To,
    By,
    Do,
    EndFor,
    While,
    EndWhile,
    Repeat,
    Until,
    EndRepeat,
    Exit,
    Continue,
    Return,

    // Word operators
    And,
    Or,
    Xor,
    Not,
    Mod,
    Shl,
    Shr,
    BitAnd,
    BitXor,
    BitOr,
    BitNot,

    // IEC Ed. 3 OOP & pointer hooks (preserved as Foreign F0)
    Method,
    EndMethod,
    Interface,
    EndInterface,
    Extends,
    Implements,
    Pointer,
    RefTo,

    // Literals
    Bool(bool),
    Int {
        value: i128,
        typed: Option<ScalarType>,
    },
    Float {
        value: f64,
        typed: Option<ScalarType>,
    },
    TimeUs(i64),
    String(String),

    // Identifiers, direct hardware addresses & vendor pragmas
    Ident(String),
    DirectAddress(String),
    VendorPragma(String),

    // Punctuation & symbolic operators
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Dot,
    DotDot,
    Colon,
    ColonEq,
    FatArrow,
    Semicolon,
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    Ampersand,
    Caret,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    Eof,
}

/// Spanned lexical token with precomputed 1-indexed line bounds and blank-line spacing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub start_line: u32,
    pub end_line: u32,
    pub blank_lines_before: u32,
}

/// Complete lexer output for one ST compilation unit.
#[derive(Debug, Clone)]
pub(crate) struct LexOutput {
    pub tokens: Vec<Token>,
    pub comments: Vec<RawComment>,
    pub eol_style: EolStyle,
}

/// Tokenizes IEC 61131-3 ST source text under `dialect`, extracting verbatim comments (`ADR-027`).
pub(crate) fn lex_st(
    src: &str,
    dialect: StCommentDialect,
    board_id: &str,
) -> Result<LexOutput, Vec<Diagnostic>> {
    let eol_style = detect_eol_style(src);
    let line_index = LineIndex::new(src);
    let mut cursor = Cursor::new(src);
    let mut tokens = Vec::new();
    let mut raw_comments = Vec::new();
    let mut diagnostics = Vec::new();

    let mut last_code_line: Option<u32> = None;
    let mut prev_item_end_line: Option<u32> = None;

    while !cursor.is_eof() {
        let _ = cursor.skip_whitespace_and_count_blank_lines();
        if cursor.is_eof() {
            break;
        }

        // 1. Comments according to active `DEF-006` StCommentDialect
        match lex_st_comment(&mut cursor, dialect) {
            Ok(Some((text, style, span))) => {
                let start_line = line_index.line_of(span.start);
                let end_offset = if span.end > span.start {
                    span.end.saturating_sub(1)
                } else {
                    span.start
                };
                let end_line = line_index.line_of(end_offset);
                let blank_lines_before = compute_blank_lines(prev_item_end_line, start_line);
                let preceded_by_code_on_line = last_code_line == Some(start_line);
                raw_comments.push(RawComment {
                    text,
                    style,
                    span,
                    start_line,
                    end_line,
                    blank_lines_before,
                    preceded_by_code_on_line,
                });
                prev_item_end_line = Some(end_line);
                continue;
            }
            Ok(None) => {}
            Err(mut diag) => {
                diag = diag.with_board_id(board_id);
                diagnostics.push(diag);
                break;
            }
        }

        // 2. Executable token
        let start = cursor.pos();
        let start_line = line_index.line_of(cursor.pos_u32());
        let blank_lines_before = compute_blank_lines(prev_item_end_line, start_line);

        match lex_one_token(&mut cursor, start, board_id) {
            Ok(kind) => {
                let span = cursor.span_from(start);
                let end_offset = if span.end > span.start {
                    span.end.saturating_sub(1)
                } else {
                    span.start
                };
                let end_line = line_index.line_of(end_offset);
                tokens.push(Token {
                    kind,
                    span,
                    start_line,
                    end_line,
                    blank_lines_before,
                });
                last_code_line = Some(end_line);
                prev_item_end_line = Some(end_line);
            }
            Err(diag) => {
                diagnostics.push(diag);
                // Ensure forward progress if lex_one_token failed without consuming.
                if cursor.pos() == start {
                    let _ = cursor.bump();
                }
            }
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let eof_pos = cursor.pos_u32();
    let eof_line = line_index.line_of(eof_pos);
    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::point(eof_pos),
        start_line: eof_line,
        end_line: eof_line,
        blank_lines_before: compute_blank_lines(prev_item_end_line, eof_line),
    });

    let comments = merge_contiguous_leading_comments(raw_comments);
    Ok(LexOutput {
        tokens,
        comments,
        eol_style,
    })
}

fn compute_blank_lines(prev_end_line: Option<u32>, start_line: u32) -> u32 {
    match prev_end_line {
        None => start_line.saturating_sub(1),
        Some(prev) => start_line.saturating_sub(prev).saturating_sub(1),
    }
}

fn lex_diag(code: ErrorCode, span: Span, board_id: &str, msg: impl Into<String>) -> Diagnostic {
    let mut d = Diagnostic::new(code, msg).with_board_id(board_id);
    d.span = Some(span);
    d
}

fn lex_one_token(
    cursor: &mut Cursor<'_>,
    start: usize,
    board_id: &str,
) -> Result<TokenKind, Diagnostic> {
    let Some(ch) = cursor.peek() else {
        return Ok(TokenKind::Eof);
    };

    if ch == '\'' {
        return lex_iec_string(cursor, start, board_id);
    }

    if ch == '"' {
        return lex_rejected_wstring(cursor, start, board_id);
    }

    if ch == '%' {
        return lex_direct_address(cursor, start, board_id);
    }

    if ch == '{' {
        return lex_vendor_pragma(cursor, start, board_id);
    }

    if ch.is_ascii_digit() {
        return lex_number(cursor, start, board_id);
    }

    if is_ident_start(ch) {
        return lex_ident_keyword_or_typed_literal(cursor, start, board_id);
    }

    let _ = cursor.bump();
    match ch {
        '(' => Ok(TokenKind::LParen),
        ')' => Ok(TokenKind::RParen),
        '[' => Ok(TokenKind::LBracket),
        ']' => Ok(TokenKind::RBracket),
        ',' => Ok(TokenKind::Comma),
        ';' => Ok(TokenKind::Semicolon),
        '+' => Ok(TokenKind::Plus),
        '-' => Ok(TokenKind::Minus),
        '/' => Ok(TokenKind::Slash),
        '&' => Ok(TokenKind::Ampersand),
        '^' => Ok(TokenKind::Caret),
        '.' => {
            if cursor.eat_char('.') {
                Ok(TokenKind::DotDot)
            } else {
                Ok(TokenKind::Dot)
            }
        }
        ':' => {
            if cursor.eat_char('=') {
                Ok(TokenKind::ColonEq)
            } else {
                Ok(TokenKind::Colon)
            }
        }
        '*' => {
            if cursor.eat_char('*') {
                Ok(TokenKind::StarStar)
            } else {
                Ok(TokenKind::Star)
            }
        }
        '=' => {
            if cursor.eat_char('>') {
                Ok(TokenKind::FatArrow)
            } else {
                Ok(TokenKind::Eq)
            }
        }
        '<' => {
            if cursor.eat_char('>') {
                Ok(TokenKind::Ne)
            } else if cursor.eat_char('=') {
                Ok(TokenKind::Le)
            } else {
                Ok(TokenKind::Lt)
            }
        }
        '>' => {
            if cursor.eat_char('=') {
                Ok(TokenKind::Ge)
            } else {
                Ok(TokenKind::Gt)
            }
        }
        other => Err(lex_diag(
            ErrorCode::ErrTomlSyntax,
            cursor.span_from(start),
            board_id,
            format!("unexpected character `{other}` in IEC 61131-3 ST source"),
        )),
    }
}

fn lex_ident_keyword_or_typed_literal(
    cursor: &mut Cursor<'_>,
    start: usize,
    board_id: &str,
) -> Result<TokenKind, Diagnostic> {
    let _ = cursor.eat_while(is_ident_continue);
    let raw = cursor.slice_from(start);

    // Typed literal prefix or TIME literal: `<TYPE>#<literal>` / `T#...` / `TIME#...`
    if cursor.peek() == Some('#') {
        return lex_typed_or_time_literal(cursor, start, raw, board_id);
    }

    let upper = raw.to_ascii_uppercase();
    let kind = match upper.as_str() {
        "USING" => TokenKind::Using,
        "NAMESPACE" => TokenKind::Namespace,
        "END_NAMESPACE" => TokenKind::EndNamespace,
        "TYPE" => TokenKind::Type,
        "END_TYPE" => TokenKind::EndType,
        "STRUCT" => TokenKind::Struct,
        "END_STRUCT" => TokenKind::EndStruct,
        "PROGRAM" => TokenKind::Program,
        "END_PROGRAM" => TokenKind::EndProgram,
        "ORGANIZATION_BLOCK" => TokenKind::OrganizationBlock,
        "END_ORGANIZATION_BLOCK" => TokenKind::EndOrganizationBlock,
        "FUNCTION" => TokenKind::Function,
        "END_FUNCTION" => TokenKind::EndFunction,
        "FUNCTION_BLOCK" => TokenKind::FunctionBlock,
        "END_FUNCTION_BLOCK" => TokenKind::EndFunctionBlock,
        "VAR_INPUT" => TokenKind::VarInput,
        "VAR_OUTPUT" => TokenKind::VarOutput,
        "VAR_IN_OUT" => TokenKind::VarInOut,
        "VAR" => TokenKind::Var,
        "VAR_TEMP" => TokenKind::VarTemp,
        "VAR_GLOBAL" => TokenKind::VarGlobal,
        "END_VAR" => TokenKind::EndVar,
        "CONSTANT" => TokenKind::Constant,
        "RETAIN" => TokenKind::Retain,
        "PERSISTENT" => TokenKind::Persistent,
        "AT" => TokenKind::At,
        "ARRAY" => TokenKind::Array,
        "OF" => TokenKind::Of,
        "IF" => TokenKind::If,
        "THEN" => TokenKind::Then,
        "ELSIF" => TokenKind::Elsif,
        "ELSE" => TokenKind::Else,
        "END_IF" => TokenKind::EndIf,
        "CASE" => TokenKind::Case,
        "END_CASE" => TokenKind::EndCase,
        "FOR" => TokenKind::For,
        "TO" => TokenKind::To,
        "BY" => TokenKind::By,
        "DO" => TokenKind::Do,
        "END_FOR" => TokenKind::EndFor,
        "WHILE" => TokenKind::While,
        "END_WHILE" => TokenKind::EndWhile,
        "REPEAT" => TokenKind::Repeat,
        "UNTIL" => TokenKind::Until,
        "END_REPEAT" => TokenKind::EndRepeat,
        "EXIT" => TokenKind::Exit,
        "CONTINUE" => TokenKind::Continue,
        "RETURN" => TokenKind::Return,
        "AND" => TokenKind::And,
        "OR" => TokenKind::Or,
        "XOR" => TokenKind::Xor,
        "NOT" => TokenKind::Not,
        "MOD" => TokenKind::Mod,
        "SHL" => TokenKind::Shl,
        "SHR" => TokenKind::Shr,
        "BIT_AND" => TokenKind::BitAnd,
        "BIT_XOR" => TokenKind::BitXor,
        "BIT_OR" => TokenKind::BitOr,
        "BIT_NOT" => TokenKind::BitNot,
        "METHOD" => TokenKind::Method,
        "END_METHOD" => TokenKind::EndMethod,
        "INTERFACE" => TokenKind::Interface,
        "END_INTERFACE" => TokenKind::EndInterface,
        "EXTENDS" => TokenKind::Extends,
        "IMPLEMENTS" => TokenKind::Implements,
        "POINTER" => TokenKind::Pointer,
        "REF_TO" => TokenKind::RefTo,
        "TRUE" => TokenKind::Bool(true),
        "FALSE" => TokenKind::Bool(false),
        _ => TokenKind::Ident(raw.to_owned()),
    };
    Ok(kind)
}

fn lex_typed_or_time_literal(
    cursor: &mut Cursor<'_>,
    start: usize,
    prefix: &str,
    board_id: &str,
) -> Result<TokenKind, Diagnostic> {
    let _ = cursor.eat_char('#');
    let upper = prefix.to_ascii_uppercase();

    // IEC duration literal: `T#...` or `TIME#...`
    if upper == "T" || upper == "TIME" {
        let negative = cursor.eat_char('-');
        let body_start = cursor.pos();
        let _ = cursor.eat_while(|c| c.is_ascii_alphanumeric() || c == '_');
        let body = cursor.slice_from(body_start);
        let span = cursor.span_from(start);
        let us = parse_iec_duration_us(body, negative).ok_or_else(|| {
            lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!("invalid IEC TIME literal `{}`", cursor.slice_from(start)),
            )
        })?;
        return Ok(TokenKind::TimeUs(us));
    }

    // Unsupported IEC date/time/wide-string typed prefixes -> ERR_UNKNOWN_TYPE (`docs/lbc/st.md` §1.4)
    if matches!(
        upper.as_str(),
        "WSTRING"
            | "DATE"
            | "D"
            | "TOD"
            | "TIME_OF_DAY"
            | "DT"
            | "DATE_AND_TIME"
            | "LTIME"
            | "LTOD"
            | "LDT"
            | "CHAR"
            | "WCHAR"
    ) {
        let _ = cursor
            .eat_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '\''));
        let span = cursor.span_from(start);
        return Err(lex_diag(
            ErrorCode::ErrUnknownType,
            span,
            board_id,
            format!("unsupported IEC typed literal prefix `{prefix}#...` in asl-core@1"),
        ));
    }

    // Typed BOOL literal: `BOOL#TRUE`, `BOOL#FALSE`, `BOOL#1`, `BOOL#0`
    if upper == "BOOL" {
        let val_start = cursor.pos();
        let _ = cursor.eat_while(|c| c.is_ascii_alphanumeric() || c == '_');
        let val = cursor.slice_from(val_start);
        let span = cursor.span_from(start);
        if val.eq_ignore_ascii_case("TRUE") || val == "1" {
            return Ok(TokenKind::Bool(true));
        }
        if val.eq_ignore_ascii_case("FALSE") || val == "0" {
            return Ok(TokenKind::Bool(false));
        }
        return Err(lex_diag(
            ErrorCode::ErrTomlSyntax,
            span,
            board_id,
            format!("invalid BOOL literal `{}`", cursor.slice_from(start)),
        ));
    }

    // Typed STRING literal: `STRING#'...'`
    if upper == "STRING" {
        if cursor.peek() == Some('\'') {
            return lex_iec_string(cursor, start, board_id);
        }
        return Err(lex_diag(
            ErrorCode::ErrTomlSyntax,
            cursor.span_from(start),
            board_id,
            "expected single-quoted string after `STRING#`",
        ));
    }

    // Typed REAL / LREAL literal: `REAL#3.14`, `LREAL#-1.5e2`
    if upper == "REAL" || upper == "LREAL" {
        let ty = if upper == "REAL" {
            ScalarType::Real
        } else {
            ScalarType::Lreal
        };
        let negative = cursor.eat_char('-');
        let _ = !negative && cursor.eat_char('+');
        if !cursor.peek().is_some_and(|c| c.is_ascii_digit()) {
            return Err(lex_diag(
                ErrorCode::ErrTomlSyntax,
                cursor.span_from(start),
                board_id,
                format!(
                    "expected numeric literal after `{prefix}#` in `{}`",
                    cursor.slice_from(start)
                ),
            ));
        }
        let num_start = cursor.pos();
        let tok = lex_number(cursor, num_start, board_id)?;
        let span = cursor.span_from(start);
        let raw_val = match tok {
            TokenKind::Float { value, .. } => value,
            TokenKind::Int { value, .. } => value as f64,
            TokenKind::Using
            | TokenKind::Namespace
            | TokenKind::EndNamespace
            | TokenKind::Type
            | TokenKind::EndType
            | TokenKind::Struct
            | TokenKind::EndStruct
            | TokenKind::Program
            | TokenKind::EndProgram
            | TokenKind::OrganizationBlock
            | TokenKind::EndOrganizationBlock
            | TokenKind::Function
            | TokenKind::EndFunction
            | TokenKind::FunctionBlock
            | TokenKind::EndFunctionBlock
            | TokenKind::VarInput
            | TokenKind::VarOutput
            | TokenKind::VarInOut
            | TokenKind::Var
            | TokenKind::VarTemp
            | TokenKind::VarGlobal
            | TokenKind::EndVar
            | TokenKind::Constant
            | TokenKind::Retain
            | TokenKind::Persistent
            | TokenKind::At
            | TokenKind::Array
            | TokenKind::Of
            | TokenKind::If
            | TokenKind::Then
            | TokenKind::Elsif
            | TokenKind::Else
            | TokenKind::EndIf
            | TokenKind::Case
            | TokenKind::EndCase
            | TokenKind::For
            | TokenKind::To
            | TokenKind::By
            | TokenKind::Do
            | TokenKind::EndFor
            | TokenKind::While
            | TokenKind::EndWhile
            | TokenKind::Repeat
            | TokenKind::Until
            | TokenKind::EndRepeat
            | TokenKind::Exit
            | TokenKind::Continue
            | TokenKind::Return
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::Xor
            | TokenKind::Not
            | TokenKind::Mod
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::BitAnd
            | TokenKind::BitXor
            | TokenKind::BitOr
            | TokenKind::BitNot
            | TokenKind::Method
            | TokenKind::EndMethod
            | TokenKind::Interface
            | TokenKind::EndInterface
            | TokenKind::Extends
            | TokenKind::Implements
            | TokenKind::Pointer
            | TokenKind::RefTo
            | TokenKind::Bool(_)
            | TokenKind::TimeUs(_)
            | TokenKind::String(_)
            | TokenKind::Ident(_)
            | TokenKind::DirectAddress(_)
            | TokenKind::VendorPragma(_)
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::DotDot
            | TokenKind::Colon
            | TokenKind::ColonEq
            | TokenKind::FatArrow
            | TokenKind::Semicolon
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::StarStar
            | TokenKind::Slash
            | TokenKind::Ampersand
            | TokenKind::Caret
            | TokenKind::Eq
            | TokenKind::Ne
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Eof => {
                return Err(lex_diag(
                    ErrorCode::ErrTomlSyntax,
                    span,
                    board_id,
                    format!(
                        "invalid floating-point literal `{}`",
                        cursor.slice_from(start)
                    ),
                ));
            }
        };
        let signed = if negative { -raw_val } else { raw_val };
        if !signed.is_finite() {
            return Err(lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!("non-finite float literal `{}`", cursor.slice_from(start)),
            ));
        }
        return Ok(TokenKind::Float {
            value: signed,
            typed: Some(ty),
        });
    }

    // Typed integer / bit-string literal: `INT#-10`, `UDINT#27`, `WORD#16#FF00`, `BYTE#2#1010`
    let Some(int_ty) = parse_int_or_bitstring_scalar(&upper) else {
        let _ = cursor.eat_while(|c| c.is_ascii_alphanumeric() || c == '_');
        return Err(lex_diag(
            ErrorCode::ErrTomlSyntax,
            cursor.span_from(start),
            board_id,
            format!(
                "invalid typed literal prefix `{prefix}#` in `{}`",
                cursor.slice_from(start)
            ),
        ));
    };

    let negative = cursor.eat_char('-');
    let _ = !negative && cursor.eat_char('+');
    if !cursor.peek().is_some_and(|c| c.is_ascii_digit()) {
        return Err(lex_diag(
            ErrorCode::ErrTomlSyntax,
            cursor.span_from(start),
            board_id,
            format!(
                "expected integer literal after `{prefix}#` in `{}`",
                cursor.slice_from(start)
            ),
        ));
    }

    let num_start = cursor.pos();
    let tok = lex_number(cursor, num_start, board_id)?;
    let span = cursor.span_from(start);
    let TokenKind::Int { value: raw_int, .. } = tok else {
        return Err(lex_diag(
            ErrorCode::ErrTypeMismatch,
            span,
            board_id,
            format!(
                "typed integer literal `{}` cannot use floating-point syntax",
                cursor.slice_from(start)
            ),
        ));
    };

    let signed_val = if negative {
        raw_int.checked_neg().ok_or_else(|| {
            lex_diag(
                ErrorCode::ErrTypeMismatch,
                span,
                board_id,
                format!("integer overflow in `{}`", cursor.slice_from(start)),
            )
        })?
    } else {
        raw_int
    };

    if !fits_scalar_int_range(&int_ty, signed_val) {
        return Err(lex_diag(
            ErrorCode::ErrTypeMismatch,
            span,
            board_id,
            format!(
                "typed literal `{}` is out of range for `{upper}`",
                cursor.slice_from(start)
            ),
        ));
    }

    Ok(TokenKind::Int {
        value: signed_val,
        typed: Some(int_ty),
    })
}

fn parse_int_or_bitstring_scalar(upper: &str) -> Option<ScalarType> {
    match upper {
        "SINT" => Some(ScalarType::Sint),
        "INT" => Some(ScalarType::Int),
        "DINT" => Some(ScalarType::Dint),
        "LINT" => Some(ScalarType::Lint),
        "USINT" => Some(ScalarType::Usint),
        "UINT" => Some(ScalarType::Uint),
        "UDINT" => Some(ScalarType::Udint),
        "ULINT" => Some(ScalarType::Ulint),
        "BYTE" => Some(ScalarType::Byte),
        "WORD" => Some(ScalarType::Word),
        "DWORD" => Some(ScalarType::Dword),
        "LWORD" => Some(ScalarType::Lword),
        "ANALOG" => Some(ScalarType::Analog),
        "DUTY" => Some(ScalarType::Duty),
        _ => None,
    }
}

/// Checks whether integer literal `val` fits within the fixed-width IEC scalar type range.
pub(crate) fn fits_scalar_int_range(ty: &ScalarType, val: i128) -> bool {
    match ty {
        ScalarType::Sint => (i128::from(i8::MIN)..=i128::from(i8::MAX)).contains(&val),
        ScalarType::Int => (i128::from(i16::MIN)..=i128::from(i16::MAX)).contains(&val),
        ScalarType::Dint => (i128::from(i32::MIN)..=i128::from(i32::MAX)).contains(&val),
        ScalarType::Lint => (i128::from(i64::MIN)..=i128::from(i64::MAX)).contains(&val),
        ScalarType::Usint | ScalarType::Byte => (0..=i128::from(u8::MAX)).contains(&val),
        ScalarType::Uint | ScalarType::Word => (0..=i128::from(u16::MAX)).contains(&val),
        ScalarType::Udint | ScalarType::Dword => (0..=i128::from(u32::MAX)).contains(&val),
        ScalarType::Ulint | ScalarType::Lword => (0..=i128::from(u64::MAX)).contains(&val),
        ScalarType::Analog => (i128::from(i32::MIN)..=i128::from(i32::MAX)).contains(&val),
        ScalarType::Duty => (0..=i128::from(u32::MAX)).contains(&val),
        ScalarType::Bool
        | ScalarType::Real
        | ScalarType::Lreal
        | ScalarType::Time
        | ScalarType::String { .. }
        | ScalarType::Code { .. } => false,
    }
}

fn lex_number(
    cursor: &mut Cursor<'_>,
    start: usize,
    board_id: &str,
) -> Result<TokenKind, Diagnostic> {
    let _ = cursor.eat_while(|c| c.is_ascii_digit() || c == '_');

    // Based integer literal: `2#1010`, `8#77`, `10#42`, `16#FF`
    if cursor.peek() == Some('#') {
        let base_str = cursor.slice_from(start);
        let _ = cursor.eat_char('#');
        let digits_start = cursor.pos();
        let _ = cursor.eat_while(|c| c.is_ascii_alphanumeric() || c == '_');
        let full = cursor.slice_from(start);
        let span = cursor.span_from(start);

        let radix: u32 = base_str.parse().map_err(|_| {
            lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!("invalid integer radix `{full}`"),
            )
        })?;
        if !matches!(radix, 2 | 8 | 10 | 16) {
            return Err(lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!(
                    "unsupported IEC integer base `{radix}` in `{full}` (expected 2, 8, 10, or 16)"
                ),
            ));
        }

        let digits_raw = cursor.slice_from(digits_start);
        let cleaned: String = digits_raw.chars().filter(|&c| c != '_').collect();
        if cleaned.is_empty() {
            return Err(lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!("missing digits in based integer literal `{full}`"),
            ));
        }
        let value = i128::from_str_radix(&cleaned, radix).map_err(|_| {
            lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!("invalid base-{radix} integer literal `{full}`"),
            )
        })?;
        return Ok(TokenKind::Int { value, typed: None });
    }

    // Float fractional part (only when `.` is followed by a digit, never `..`!)
    let mut is_float = false;
    if cursor.peek() == Some('.') && cursor.peek_second().is_some_and(|c| c.is_ascii_digit()) {
        is_float = true;
        let _ = cursor.eat_char('.');
        let _ = cursor.eat_while(|c| c.is_ascii_digit() || c == '_');
    }

    // Float exponent part
    if matches!(cursor.peek(), Some('e' | 'E')) {
        is_float = true;
        let _ = cursor.bump();
        if matches!(cursor.peek(), Some('+' | '-')) {
            let _ = cursor.bump();
        }
        let exp_start = cursor.pos();
        let _ = cursor.eat_while(|c| c.is_ascii_digit() || c == '_');
        if cursor.pos() == exp_start {
            return Err(lex_diag(
                ErrorCode::ErrTomlSyntax,
                cursor.span_from(start),
                board_id,
                format!(
                    "missing exponent digits in float literal `{}`",
                    cursor.slice_from(start)
                ),
            ));
        }
    }

    // Reject stray identifier suffix glued onto a number (e.g. `123abc`)
    if cursor.peek().is_some_and(is_ident_start) {
        let _ = cursor.eat_while(is_ident_continue);
        return Err(lex_diag(
            ErrorCode::ErrTomlSyntax,
            cursor.span_from(start),
            board_id,
            format!("malformed numeric literal `{}`", cursor.slice_from(start)),
        ));
    }

    let raw = cursor.slice_from(start);
    let span = cursor.span_from(start);
    if raw.ends_with('_') {
        return Err(lex_diag(
            ErrorCode::ErrTomlSyntax,
            span,
            board_id,
            format!("trailing underscore in numeric literal `{raw}`"),
        ));
    }
    let cleaned: String = raw.chars().filter(|&c| c != '_').collect();
    if is_float {
        let value: f64 = cleaned.parse().map_err(|_| {
            lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!("invalid floating-point literal `{raw}`"),
            )
        })?;
        if !value.is_finite() {
            return Err(lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!("non-finite floating-point literal `{raw}`"),
            ));
        }
        Ok(TokenKind::Float { value, typed: None })
    } else {
        let value: i128 = cleaned.parse().map_err(|_| {
            lex_diag(
                ErrorCode::ErrTomlSyntax,
                span,
                board_id,
                format!("invalid integer literal `{raw}`"),
            )
        })?;
        Ok(TokenKind::Int { value, typed: None })
    }
}

fn lex_iec_string(
    cursor: &mut Cursor<'_>,
    start: usize,
    board_id: &str,
) -> Result<TokenKind, Diagnostic> {
    let _ = cursor.eat_char('\'');
    let mut out = String::new();
    let mut pending_hex_bytes: Vec<u8> = Vec::new();

    let flush_hex = |bytes: &mut Vec<u8>, target: &mut String| {
        if bytes.is_empty() {
            return;
        }
        match std::str::from_utf8(bytes) {
            Ok(valid) => target.push_str(valid),
            Err(_) => {
                for &b in bytes.iter() {
                    target.push(char::from(b));
                }
            }
        }
        bytes.clear();
    };

    while let Some(ch) = cursor.bump() {
        match ch {
            '\'' => {
                flush_hex(&mut pending_hex_bytes, &mut out);
                return Ok(TokenKind::String(out));
            }
            '\n' | '\r' => {
                return Err(lex_diag(
                    ErrorCode::ErrTomlSyntax,
                    cursor.span_from(start),
                    board_id,
                    "unterminated IEC single-quoted string literal",
                ));
            }
            '$' => {
                let esc_start = cursor.pos().saturating_sub(1);
                let Some(next) = cursor.bump() else {
                    return Err(lex_diag(
                        ErrorCode::ErrTomlSyntax,
                        cursor.span_from(start),
                        board_id,
                        "unterminated IEC string escape sequence",
                    ));
                };
                match next {
                    '$' => {
                        flush_hex(&mut pending_hex_bytes, &mut out);
                        out.push('$');
                    }
                    '\'' => {
                        flush_hex(&mut pending_hex_bytes, &mut out);
                        out.push('\'');
                    }
                    'L' | 'l' | 'N' | 'n' => {
                        flush_hex(&mut pending_hex_bytes, &mut out);
                        out.push('\n');
                    }
                    'R' | 'r' => {
                        flush_hex(&mut pending_hex_bytes, &mut out);
                        out.push('\r');
                    }
                    'T' | 't' => {
                        flush_hex(&mut pending_hex_bytes, &mut out);
                        out.push('\t');
                    }
                    'P' | 'p' => {
                        flush_hex(&mut pending_hex_bytes, &mut out);
                        out.push('\x0C');
                    }
                    hi if hi.is_ascii_hexdigit() => {
                        let Some(lo) = cursor.bump() else {
                            return Err(lex_diag(
                                ErrorCode::ErrTomlSyntax,
                                cursor.span_from(esc_start),
                                board_id,
                                "truncated `$XX` hex escape in IEC string literal",
                            ));
                        };
                        if !lo.is_ascii_hexdigit() {
                            return Err(lex_diag(
                                ErrorCode::ErrTomlSyntax,
                                cursor.span_from(esc_start),
                                board_id,
                                format!(
                                    "invalid `$XX` hex escape `${hi}{lo}` in IEC string literal"
                                ),
                            ));
                        }
                        let hex_pair = format!("{hi}{lo}");
                        let byte = u8::from_str_radix(&hex_pair, 16).map_err(|_| {
                            lex_diag(
                                ErrorCode::ErrTomlSyntax,
                                cursor.span_from(esc_start),
                                board_id,
                                format!("invalid hex byte `${hex_pair}` in IEC string literal"),
                            )
                        })?;
                        pending_hex_bytes.push(byte);
                    }
                    other => {
                        return Err(lex_diag(
                            ErrorCode::ErrTomlSyntax,
                            cursor.span_from(esc_start),
                            board_id,
                            format!("invalid IEC string escape `${other}`"),
                        ));
                    }
                }
            }
            other => {
                flush_hex(&mut pending_hex_bytes, &mut out);
                out.push(other);
            }
        }
    }

    Err(lex_diag(
        ErrorCode::ErrTomlSyntax,
        cursor.span_from(start),
        board_id,
        "unterminated IEC single-quoted string literal at end of input",
    ))
}

fn lex_rejected_wstring(
    cursor: &mut Cursor<'_>,
    start: usize,
    board_id: &str,
) -> Result<TokenKind, Diagnostic> {
    let _ = cursor.eat_char('"');
    while let Some(ch) = cursor.bump() {
        if ch == '$' {
            let _ = cursor.bump();
        } else if ch == '"' || ch == '\n' || ch == '\r' {
            break;
        }
    }
    Err(lex_diag(
        ErrorCode::ErrUnknownType,
        cursor.span_from(start),
        board_id,
        "double-quoted WSTRING literals are not supported in asl-core@1; use single-quoted STRING `'...'`",
    ))
}

fn lex_direct_address(
    cursor: &mut Cursor<'_>,
    start: usize,
    board_id: &str,
) -> Result<TokenKind, Diagnostic> {
    let _ = cursor.eat_char('%');
    let area = cursor.bump();
    let size = cursor.bump();
    let valid_area = matches!(area, Some('I' | 'i' | 'Q' | 'q' | 'M' | 'm'));
    let valid_size = matches!(
        size,
        Some('X' | 'x' | 'B' | 'b' | 'W' | 'w' | 'D' | 'd' | 'L' | 'l')
    );

    let digits_start = cursor.pos();
    let _ = cursor.eat_while(|c| c.is_ascii_digit());
    let has_primary_digits = cursor.pos() > digits_start;

    let mut valid_dot = true;
    if cursor.peek() == Some('.') {
        let _ = cursor.eat_char('.');
        let sub_start = cursor.pos();
        let _ = cursor.eat_while(|c| c.is_ascii_digit());
        if cursor.pos() == sub_start {
            valid_dot = false;
        }
    }

    if !valid_area || !valid_size || !has_primary_digits || !valid_dot {
        let _ = cursor.eat_while(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
        return Err(lex_diag(
            ErrorCode::ErrTomlSyntax,
            cursor.span_from(start),
            board_id,
            format!(
                "malformed IEC direct hardware address `{}` (expected `%<I|Q|M><X|B|W|D|L><digits>[.<bit>]`)",
                cursor.slice_from(start)
            ),
        ));
    }

    let addr = cursor.slice_from(start).to_ascii_uppercase();
    Ok(TokenKind::DirectAddress(addr))
}

fn lex_vendor_pragma(
    cursor: &mut Cursor<'_>,
    start: usize,
    board_id: &str,
) -> Result<TokenKind, Diagnostic> {
    let _ = cursor.eat_char('{');
    let mut depth = 1usize;
    let mut in_single_quote = false;
    while let Some(ch) = cursor.bump() {
        if in_single_quote {
            if ch == '$' {
                let _ = cursor.bump();
            } else if ch == '\'' {
                in_single_quote = false;
            }
            continue;
        }
        match ch {
            '\'' => in_single_quote = true,
            '{' => depth = depth.saturating_add(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(TokenKind::VendorPragma(cursor.slice_from(start).to_owned()));
                }
            }
            _ => {}
        }
    }
    Err(lex_diag(
        ErrorCode::ErrTomlSyntax,
        cursor.span_from(start),
        board_id,
        "unterminated `{...}` vendor pragma",
    ))
}

/// Parses IEC 61131-3 compound duration bodies (`500us`, `1s500ms`, `2m`, `1h30m`, etc.)
/// into signed microseconds (`i64`).
fn parse_iec_duration_us(body: &str, negative: bool) -> Option<i64> {
    if body.is_empty() {
        return None;
    }
    let bytes = body.as_bytes();
    let mut idx = 0usize;
    let mut total: i128 = 0;
    let mut parsed_any = false;

    while idx < bytes.len() {
        let num_start = idx;
        while bytes
            .get(idx)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
        {
            idx = idx.saturating_add(1);
        }
        if idx == num_start {
            return None;
        }
        let cleaned: String = body
            .get(num_start..idx)?
            .chars()
            .filter(|&c| c != '_')
            .collect();
        if cleaned.is_empty() {
            return None;
        }
        let amount: i128 = cleaned.parse().ok()?;

        let unit_start = idx;
        while bytes.get(idx).is_some_and(u8::is_ascii_alphabetic) {
            idx = idx.saturating_add(1);
        }
        let unit = body.get(unit_start..idx)?.to_ascii_lowercase();
        let mult: i128 = match unit.as_str() {
            "us" => 1,
            "ms" => 1_000,
            "s" => 1_000_000,
            "m" => 60_000_000,
            "h" => 3_600_000_000,
            "d" => 86_400_000_000,
            _ => return None,
        };
        let term = amount.checked_mul(mult)?;
        total = total.checked_add(term)?;
        parsed_any = true;
    }

    if !parsed_any {
        return None;
    }
    let signed = if negative {
        total.checked_neg()?
    } else {
        total
    };
    i64::try_from(signed).ok()
}
