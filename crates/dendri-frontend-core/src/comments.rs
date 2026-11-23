//! Complete ADR-027 verbatim human comment engine (`01` §7, `04` §4.3, Gate 8 `C1–C5`).
//!
//! Guarantees:
//! - Verbatim extraction between delimiters: zero trimming, zero NFC/NFD normalization,
//!   LF internal line endings with original EOL restoration on same-language emit.
//! - Contiguous `leading` line-comment merging (blank lines split; `trailing` always single-line).
//! - Nested `(* ... *)` lexing for IEC 61131-3 ST dialects (`DEF-006`).
//! - Projection-only hazard avoidance (C++ trailing `\`, C `*/`/`/*`, ST `(*`/`*)`, Trojan-Source
//!   bidi controls with `WARN_COMMENT_BIDI`, and `CommentEncoding` with `WARN_COMMENT_ENCODING`).
//! - Re-import idempotence (`matches_comment`) so degraded projections never clobber verbatim originals.
//! - Deterministic `inner` relocation (`INFO_COMMENT_RELOCATED`) and orphan preservation (`INFO_COMMENT_ORPHANED`).

use crate::cursor::Cursor;
use crate::traits::{
    BoardContext, CommentPlacement, LanguageCapabilities, LanguageId, RenderedComment,
    StCommentDialect,
};
use dendri_ast::{
    Comment, CommentAnchor, CommentEncoding, CommentPlacement as AstCommentPlacement, CommentStyle,
    EolStyle, NodeId,
};
use dendri_span::{Diagnostic, ErrorCode, Span};
use std::collections::BTreeMap;

/// Raw extracted comment token prior to AST anchoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawComment {
    /// Verbatim comment body between delimiters (LF line endings, zero trimming/normalization).
    pub text: String,
    /// Detected original comment syntax style.
    pub style: CommentStyle,
    /// Full byte span including delimiters.
    pub span: Span,
    /// 1-indexed line where the comment starts.
    pub start_line: u32,
    /// 1-indexed line where the comment ends.
    pub end_line: u32,
    /// Number of blank lines immediately preceding this comment.
    pub blank_lines_before: u32,
    /// `true` iff executable tokens appeared earlier on `start_line` (trailing or inner comment).
    pub preceded_by_code_on_line: bool,
}

/// Normalizes CRLF (`\r\n`) line endings to LF (`\n`) without trimming or Unicode normalization.
#[must_use]
pub fn normalize_eol_to_lf(text: &str) -> String {
    if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text.to_owned()
    }
}

/// Restores the board's preserved [`EolStyle`] on emitted text (avoiding double `\r\r\n`).
#[must_use]
pub fn restore_eol(text: &str, eol: EolStyle) -> String {
    let lf = normalize_eol_to_lf(text);
    match eol {
        EolStyle::Lf => lf,
        EolStyle::Crlf => lf.replace('\n', "\r\n"),
    }
}

/// Returns `true` iff `ch` is a Trojan-Source bidirectional control character (`WARN_COMMENT_BIDI`).
#[must_use]
pub const fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// Returns `true` iff `text` contains any Trojan-Source bidirectional control character.
#[must_use]
pub fn contains_bidi_control(text: &str) -> bool {
    dendri_ast::comment::contains_bidi_control(text)
}

/// Escapes all Trojan-Source bidirectional control characters to `\u{XXXX}` form.
///
/// Returns `(escaped_text, had_bidi_controls)`.
#[must_use]
pub fn escape_bidi_controls(text: &str) -> (String, bool) {
    if !contains_bidi_control(text) {
        return (text.to_owned(), false);
    }
    let mut out = String::with_capacity(text.len().saturating_add(16));
    for ch in text.chars() {
        if is_bidi_control(ch) {
            out.push_str(&format!("\\u{{{:04X}}}", u32::from(ch)));
        } else {
            out.push(ch);
        }
    }
    (out, true)
}

/// Deterministically folds non-ASCII text to pure 7-bit ASCII for `CommentEncoding::AsciiFold`.
///
/// Common Latin diacritics and engineering symbols fold to readable ASCII equivalents;
/// remaining non-ASCII scalar values (emoji, combining marks, CJK, etc.) escape to `\u{XXXX}`.
/// Returns `(folded_text, was_degraded)`.
#[must_use]
pub fn ascii_fold(text: &str) -> (String, bool) {
    if text.is_ascii() {
        return (text.to_owned(), false);
    }
    let mut out = String::with_capacity(text.len().saturating_add(16));
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
            continue;
        }
        match ch {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => out.push('a'),
            'Á' | 'À' | 'Â' | 'Ã' | 'Ä' | 'Å' => out.push('A'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'É' | 'È' | 'Ê' | 'Ë' => out.push('E'),
            'í' | 'ì' | 'î' | 'ï' => out.push('i'),
            'Í' | 'Ì' | 'Î' | 'Ï' => out.push('I'),
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'ø' => out.push('o'),
            'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => out.push('O'),
            'ú' | 'ù' | 'û' | 'ü' => out.push('u'),
            'Ú' | 'Ù' | 'Û' | 'Ü' => out.push('U'),
            'ç' => out.push('c'),
            'Ç' => out.push('C'),
            'ñ' => out.push('n'),
            'Ñ' => out.push('N'),
            'ý' | 'ÿ' => out.push('y'),
            'Ý' => out.push('Y'),
            'ß' => out.push_str("ss"),
            '—' | '–' | '−' => out.push('-'),
            '“' | '”' => out.push('"'),
            '‘' | '’' => out.push('\''),
            '…' => out.push_str("..."),
            '°' => out.push_str("deg"),
            '±' => out.push_str("+/-"),
            '×' => out.push('x'),
            '÷' => out.push('/'),
            'µ' | 'μ' => out.push('u'),
            'Ω' => out.push_str("Ohm"),
            '≤' => out.push_str("<="),
            '≥' => out.push_str(">="),
            '≠' => out.push_str("!="),
            '→' => out.push_str("->"),
            '←' => out.push_str("<-"),
            '⇄' => out.push_str("<->"),
            other => out.push_str(&format!("\\u{{{:04X}}}", u32::from(other))),
        }
    }
    (out, true)
}

/// Returns `true` if any `\n`-separated line in `text` ends with `\` (optionally followed by
/// horizontal whitespace), which triggers C++ translation-phase-2 line splicing inside `//`.
#[must_use]
pub fn has_cpp_line_splice_hazard(text: &str) -> bool {
    text.split('\n')
        .any(|line| line.trim_end_matches([' ', '\t']).ends_with('\\'))
}

/// Returns `true` iff all `(*` and `*)` sequences inside `text` are strictly balanced
/// (nesting depth never drops below zero and ends at zero).
#[must_use]
pub fn has_balanced_paren_delimiters(text: &str) -> bool {
    let mut cur = Cursor::new(text);
    let mut depth = 0usize;
    while !cur.is_eof() {
        if cur.eat_str("(*") {
            depth = depth.saturating_add(1);
        } else if cur.eat_str("*)") {
            let Some(next) = depth.checked_sub(1) else {
                return false;
            };
            depth = next;
        } else {
            let _ = cur.bump();
        }
    }
    depth == 0
}

/// Non-destructively escapes C-style block delimiters (`/*`, `*/`, and trailing `/`) so the
/// result can be safely enclosed in `/* ... */` even under `-Wall -Wextra -Werror`.
#[must_use]
pub fn escape_c_block_delimiters(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(8));
    let mut last: Option<char> = None;
    for ch in text.chars() {
        if (last == Some('/') && ch == '*') || (last == Some('*') && ch == '/') {
            out.push('\\');
        }
        out.push(ch);
        last = Some(ch);
    }
    if last == Some('/') {
        out.push(' ');
    }
    out
}

/// Non-destructively escapes IEC ST parenthesis-block delimiters (`(*`, `*)`, and trailing `(`)
/// so the result never contains `(*` or `*)` and closes cleanly in both nested and non-nested ST.
#[must_use]
pub fn escape_st_paren_delimiters(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(8));
    let mut last: Option<char> = None;
    for ch in text.chars() {
        if (last == Some('(') && ch == '*') || (last == Some('*') && ch == ')') {
            out.push('\\');
        }
        out.push(ch);
        last = Some(ch);
    }
    if last == Some('(') {
        out.push(' ');
    }
    out
}

/// Lexes an IEC 61131-3 `(* ... *)` parenthesis block comment from `cursor`.
///
/// When `allow_nesting` is `true` (`Iec61131Ed3`, `RuSTy`), tracks nesting depth and stores
/// nested `(* ... *)` bodies verbatim inside the outer stripped text (`01` §7).
/// Returns `Ok(None)` if `cursor` does not start with `(*`.
pub fn lex_nested_paren_comment(
    cursor: &mut Cursor<'_>,
    allow_nesting: bool,
) -> Result<Option<(String, Span)>, Diagnostic> {
    if !cursor.starts_with("(*") {
        return Ok(None);
    }
    let start = cursor.pos();
    let _ = cursor.eat_str("(*");
    let body_start = cursor.pos();
    let mut depth = 1usize;

    while !cursor.is_eof() {
        if allow_nesting && cursor.starts_with("(*") {
            let _ = cursor.eat_str("(*");
            depth = depth.saturating_add(1);
        } else if cursor.starts_with("*)") {
            let body_end = cursor.pos();
            let _ = cursor.eat_str("*)");
            depth = depth.saturating_sub(1);
            if depth == 0 {
                let raw_body = cursor.source().get(body_start..body_end).unwrap_or("");
                let span = cursor.span_from(start);
                return Ok(Some((normalize_eol_to_lf(raw_body), span)));
            }
        } else {
            let _ = cursor.bump();
        }
    }

    let span = cursor.span_from(start);
    let mut diag = Diagnostic::new(
        ErrorCode::ErrCommentField,
        "unterminated IEC ST `(* ... *)` block comment",
    )
    .with_hint("Close every opened `(*` delimiter with a matching `*)`.");
    diag.span = Some(span);
    Err(diag)
}

/// Lexes a C-style `/* ... */`, `/** ... */`, or `/*! ... */` block comment from `cursor`.
///
/// Returns `Ok(None)` if `cursor` does not start with `/*`.
pub fn lex_c_block_comment(
    cursor: &mut Cursor<'_>,
) -> Result<Option<(String, CommentStyle, Span)>, Diagnostic> {
    if !cursor.starts_with("/*") {
        return Ok(None);
    }
    let start = cursor.pos();
    // Empty `/**/` is a standard empty Block comment, NOT a DocBlock.
    if cursor.eat_str("/**/") {
        return Ok(Some((
            String::new(),
            CommentStyle::Block,
            cursor.span_from(start),
        )));
    }
    let style = if cursor.starts_with("/**") && !cursor.starts_with("/***") {
        let _ = cursor.eat_str("/**");
        CommentStyle::DocBlock
    } else if cursor.eat_str("/*!") {
        CommentStyle::DocBlock
    } else {
        let _ = cursor.eat_str("/*");
        CommentStyle::Block
    };

    let (body, found) = cursor.eat_until_str("*/");
    if !found {
        let span = cursor.span_from(start);
        let mut diag = Diagnostic::new(
            ErrorCode::ErrCommentField,
            "unterminated `/* ... */` block comment",
        )
        .with_hint("Close the block comment with `*/`.");
        diag.span = Some(span);
        return Err(diag);
    }
    let _ = cursor.eat_str("*/");
    Ok(Some((
        normalize_eol_to_lf(body),
        style,
        cursor.span_from(start),
    )))
}

/// Lexes a slash line comment (`//`, `///`, or `//!`) from `cursor` without consuming its newline.
///
/// Returns `None` if `cursor` does not start with `//`.
pub fn lex_slash_line_comment(cursor: &mut Cursor<'_>) -> Option<(String, CommentStyle, Span)> {
    if !cursor.starts_with("//") {
        return None;
    }
    let start = cursor.pos();
    let style = if cursor.starts_with("///") && !cursor.starts_with("////") {
        let _ = cursor.eat_str("///");
        CommentStyle::DocLine
    } else if cursor.eat_str("//!") {
        CommentStyle::DocLine
    } else {
        let _ = cursor.eat_str("//");
        CommentStyle::Line
    };
    let body = cursor.eat_line();
    Some((body.to_owned(), style, cursor.span_from(start)))
}

/// Lexes a single C++ comment (`//`, `///`, `//!`, `/* ... */`, `/** ... */`, `/*! ... */`).
pub fn lex_cpp_comment(
    cursor: &mut Cursor<'_>,
) -> Result<Option<(String, CommentStyle, Span)>, Diagnostic> {
    if let Some(line) = lex_slash_line_comment(cursor) {
        return Ok(Some(line));
    }
    lex_c_block_comment(cursor)
}

/// Lexes a single IEC 61131-3 ST comment according to `dialect` (`DEF-006`).
pub fn lex_st_comment(
    cursor: &mut Cursor<'_>,
    dialect: StCommentDialect,
) -> Result<Option<(String, CommentStyle, Span)>, Diagnostic> {
    if cursor.starts_with("(*") {
        let parsed = lex_nested_paren_comment(cursor, dialect.supports_nested_paren_comments())?;
        return Ok(parsed.map(|(text, span)| (text, CommentStyle::ParenBlock, span)));
    }
    if dialect.supports_line_comments()
        && cursor.starts_with("//")
        && let Some((text, _style, span)) = lex_slash_line_comment(cursor)
    {
        // In IEC ST, `//` is always `CommentStyle::Line`; restore any third slash into body verbatim.
        let full_body = if span
            .slice(cursor.source())
            .is_some_and(|s| s.starts_with("///") && !s.starts_with("////"))
        {
            format!("/{text}")
        } else if span
            .slice(cursor.source())
            .is_some_and(|s| s.starts_with("//!"))
        {
            format!("!{text}")
        } else {
            text
        };
        return Ok(Some((full_body, CommentStyle::Line, span)));
    }
    if dialect.supports_c_block_comments() && cursor.starts_with("/*") {
        let start = cursor.pos();
        let _ = cursor.eat_str("/*");
        let (body, found) = cursor.eat_until_str("*/");
        if !found {
            let span = cursor.span_from(start);
            let mut diag = Diagnostic::new(
                ErrorCode::ErrCommentField,
                "unterminated IEC ST `/* ... */` block comment",
            )
            .with_hint("Close the block comment with `*/`.");
            diag.span = Some(span);
            return Err(diag);
        }
        let _ = cursor.eat_str("*/");
        return Ok(Some((
            normalize_eol_to_lf(body),
            CommentStyle::Block,
            cursor.span_from(start),
        )));
    }
    Ok(None)
}

/// Merges contiguous `leading` line comments of matching style with no blank line between them
/// into single `\n`-joined records (`01` §7, `04` §4.3).
///
/// Blank lines split records; `trailing` and block comments are never merged.
#[must_use]
pub fn merge_contiguous_leading_comments(raw: Vec<RawComment>) -> Vec<RawComment> {
    let mut merged: Vec<RawComment> = Vec::with_capacity(raw.len());
    for item in raw {
        let can_merge = if let Some(prev) = merged.last() {
            matches!(prev.style, CommentStyle::Line | CommentStyle::DocLine)
                && prev.style == item.style
                && !prev.preceded_by_code_on_line
                && !item.preceded_by_code_on_line
                && item.blank_lines_before == 0
                && item.start_line == prev.end_line.saturating_add(1)
        } else {
            false
        };

        if can_merge && let Some(prev) = merged.last_mut() {
            prev.text.push('\n');
            prev.text.push_str(&item.text);
            prev.span = prev.span.merge(item.span);
            prev.end_line = item.end_line;
        } else {
            merged.push(item);
        }
    }
    merged
}

/// Renders a verbatim comment for `language` without mutating stored text (`ADR-027`).
///
/// Applies:
/// 1. Trojan-Source bidi escaping (`WARN_COMMENT_BIDI`)
/// 2. Target `CommentEncoding` (`WARN_COMMENT_ENCODING`)
/// 3. Language- and dialect-specific hazard avoidance (C++ trailing `\`, `*/`/`/*`, ST `(*`/`*)`).
#[must_use]
pub fn render_comment_for_language(
    language: LanguageId,
    text: &str,
    style: CommentStyle,
    placement: CommentPlacement,
    ctx: &BoardContext,
) -> RenderedComment {
    let mut diagnostics = Vec::new();
    let mut degraded = false;

    let lf_text = normalize_eol_to_lf(text);

    // 1. Trojan-Source bidi control detection and escaping (projection-only)
    let (after_bidi, had_bidi) = escape_bidi_controls(&lf_text);
    if had_bidi {
        degraded = true;
        diagnostics.push(
            Diagnostic::new(
                ErrorCode::WarnCommentBidi,
                "comment contains Unicode bidirectional control characters; rendered escaped in projection",
            )
            .with_board_id(&ctx.board_id)
            .with_hint("Stored verbatim comment text is unchanged; remove bidi controls to silence this warning."),
        );
    }

    // 2. Target encoding policy (projection-only)
    let projected = match ctx.comment_encoding {
        CommentEncoding::Utf8Passthrough => after_bidi,
        CommentEncoding::WarnOnly => {
            if !after_bidi.is_ascii() {
                diagnostics.push(
                    Diagnostic::new(
                        ErrorCode::WarnCommentEncoding,
                        "comment contains non-ASCII text under `warn_only` target encoding",
                    )
                    .with_board_id(&ctx.board_id),
                );
            }
            after_bidi
        }
        CommentEncoding::AsciiFold => {
            let (folded, did_fold) = ascii_fold(&after_bidi);
            if did_fold {
                degraded = true;
                diagnostics.push(
                    Diagnostic::new(
                        ErrorCode::WarnCommentEncoding,
                        "comment contains non-ASCII text; folded to ASCII in emitted projection only",
                    )
                    .with_board_id(&ctx.board_id)
                    .with_hint("Stored verbatim UTF-8 comment text in ASL is preserved intact."),
                );
            }
            folded
        }
    };

    // 3. Target-language syntax & hazard avoidance
    match language {
        LanguageId::Cpp => render_cpp(&projected, style, placement, degraded, diagnostics),
        LanguageId::St => render_st(
            &projected,
            style,
            placement,
            ctx.st_dialect,
            degraded,
            diagnostics,
        ),
        LanguageId::Ladder | LanguageId::Sfc => RenderedComment {
            rendered: projected,
            emitted_style: CommentStyle::Visual,
            degraded,
            diagnostics,
        },
    }
}

fn render_cpp(
    projected: &str,
    style: CommentStyle,
    placement: CommentPlacement,
    mut degraded: bool,
    diagnostics: Vec<Diagnostic>,
) -> RenderedComment {
    let is_inner = matches!(placement, CommentPlacement::Inner { .. });
    let is_header = matches!(placement, CommentPlacement::Header);
    let has_backslash = has_cpp_line_splice_hazard(projected);
    let has_c_block_hazard =
        projected.contains("*/") || projected.contains("/*") || projected.ends_with('/');

    // Inner expression comments in C++ MUST stay inline block comments.
    if is_inner {
        let body = if has_c_block_hazard {
            degraded = true;
            escape_c_block_delimiters(projected)
        } else {
            projected.to_owned()
        };
        return RenderedComment {
            rendered: format!("/*{body}*/"),
            emitted_style: CommentStyle::Block,
            degraded,
            diagnostics,
        };
    }

    match style {
        CommentStyle::Line | CommentStyle::DocLine | CommentStyle::Visual => {
            if has_backslash {
                // C++ hazard avoidance: `//` ending in `\` splices next line -> emit as block comment.
                degraded = true;
                let body = if has_c_block_hazard {
                    escape_c_block_delimiters(projected)
                } else {
                    projected.to_owned()
                };
                RenderedComment {
                    rendered: format!("/*{body}*/"),
                    emitted_style: CommentStyle::Block,
                    degraded,
                    diagnostics,
                }
            } else {
                let (prefix, out_style) = match style {
                    CommentStyle::DocLine => {
                        (if is_header { "//!" } else { "///" }, CommentStyle::DocLine)
                    }
                    CommentStyle::Line | CommentStyle::Visual => ("//", CommentStyle::Line),
                    CommentStyle::Block | CommentStyle::ParenBlock | CommentStyle::DocBlock => {
                        ("//", CommentStyle::Line)
                    }
                };
                RenderedComment {
                    rendered: render_prefixed_lines(prefix, projected),
                    emitted_style: out_style,
                    degraded,
                    diagnostics,
                }
            }
        }
        CommentStyle::Block | CommentStyle::DocBlock | CommentStyle::ParenBlock => {
            let paren_nested_to_cpp = matches!(style, CommentStyle::ParenBlock)
                && (projected.contains("(*") || projected.contains("*)"));
            let must_avoid_block = has_c_block_hazard || paren_nested_to_cpp;

            if must_avoid_block {
                if !has_backslash {
                    // Project hazardous block / nested ST paren_block cleanly as C++ `//` line comments.
                    RenderedComment {
                        rendered: render_prefixed_lines("//", projected),
                        emitted_style: CommentStyle::Line,
                        degraded: true,
                        diagnostics,
                    }
                } else {
                    // Both block-delimiter hazard AND trailing `\`: non-destructive delimiter escape.
                    degraded = true;
                    let body = escape_c_block_delimiters(projected);
                    RenderedComment {
                        rendered: format!("/*{body}*/"),
                        emitted_style: CommentStyle::Block,
                        degraded,
                        diagnostics,
                    }
                }
            } else {
                let (open, out_style) = match style {
                    CommentStyle::DocBlock => (
                        if is_header { "/*!" } else { "/**" },
                        CommentStyle::DocBlock,
                    ),
                    CommentStyle::Block | CommentStyle::ParenBlock => ("/*", CommentStyle::Block),
                    CommentStyle::Line | CommentStyle::DocLine | CommentStyle::Visual => {
                        ("/*", CommentStyle::Block)
                    }
                };
                // Guard empty `/**/` vs `/**` opening if body is empty on DocBlock
                let rendered = if open == "/**" && projected.is_empty() {
                    "/** */".to_owned()
                } else {
                    format!("{open}{projected}*/")
                };
                RenderedComment {
                    rendered,
                    emitted_style: out_style,
                    degraded,
                    diagnostics,
                }
            }
        }
    }
}

fn render_st(
    projected: &str,
    style: CommentStyle,
    placement: CommentPlacement,
    dialect: StCommentDialect,
    mut degraded: bool,
    diagnostics: Vec<Diagnostic>,
) -> RenderedComment {
    let is_inner = matches!(placement, CommentPlacement::Inner { .. });
    let can_line = dialect.supports_line_comments() && !is_inner;
    let can_c_block = dialect.supports_c_block_comments();
    let can_nest_paren = dialect.supports_nested_paren_comments();

    let paren_safe = if can_nest_paren {
        has_balanced_paren_delimiters(projected) && !projected.ends_with('(')
    } else {
        !projected.contains("*)") && !projected.contains("(*") && !projected.ends_with('(')
    };
    let c_block_safe = can_c_block
        && !projected.contains("*/")
        && !projected.contains("/*")
        && !projected.ends_with('/');

    match style {
        CommentStyle::Line | CommentStyle::DocLine | CommentStyle::Visual => {
            if can_line {
                RenderedComment {
                    rendered: render_prefixed_lines("//", projected),
                    emitted_style: CommentStyle::Line,
                    degraded,
                    diagnostics,
                }
            } else if paren_safe {
                RenderedComment {
                    rendered: format!("(*{projected}*)"),
                    emitted_style: CommentStyle::ParenBlock,
                    degraded: degraded || !matches!(style, CommentStyle::Visual),
                    diagnostics,
                }
            } else {
                degraded = true;
                let escaped = escape_st_paren_delimiters(projected);
                RenderedComment {
                    rendered: format!("(*{escaped}*)"),
                    emitted_style: CommentStyle::ParenBlock,
                    degraded,
                    diagnostics,
                }
            }
        }
        CommentStyle::Block | CommentStyle::DocBlock => {
            if c_block_safe {
                RenderedComment {
                    rendered: format!("/*{projected}*/"),
                    emitted_style: CommentStyle::Block,
                    degraded,
                    diagnostics,
                }
            } else if can_line {
                RenderedComment {
                    rendered: render_prefixed_lines("//", projected),
                    emitted_style: CommentStyle::Line,
                    degraded: true,
                    diagnostics,
                }
            } else if paren_safe {
                RenderedComment {
                    rendered: format!("(*{projected}*)"),
                    emitted_style: CommentStyle::ParenBlock,
                    degraded: true,
                    diagnostics,
                }
            } else {
                degraded = true;
                let escaped = escape_st_paren_delimiters(projected);
                RenderedComment {
                    rendered: format!("(*{escaped}*)"),
                    emitted_style: CommentStyle::ParenBlock,
                    degraded,
                    diagnostics,
                }
            }
        }
        CommentStyle::ParenBlock => {
            if paren_safe {
                RenderedComment {
                    rendered: format!("(*{projected}*)"),
                    emitted_style: CommentStyle::ParenBlock,
                    degraded,
                    diagnostics,
                }
            } else if can_line {
                // Dialect allows `//`: project hazardous ST block cleanly as line comments.
                RenderedComment {
                    rendered: render_prefixed_lines("//", projected),
                    emitted_style: CommentStyle::Line,
                    degraded: true,
                    diagnostics,
                }
            } else {
                // Block-only dialect (e.g. StrictEd2) or Inner placement: documented non-destructive escape.
                degraded = true;
                let escaped = escape_st_paren_delimiters(projected);
                RenderedComment {
                    rendered: format!("(*{escaped}*)"),
                    emitted_style: CommentStyle::ParenBlock,
                    degraded,
                    diagnostics,
                }
            }
        }
    }
}

fn render_prefixed_lines(prefix: &str, text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(16));
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(prefix);
        out.push_str(line);
    }
    out
}

/// Strips target-language comment delimiters from a single rendered comment (or contiguous `//` block),
/// returning its LF-normalized inner body if `rendered` is a syntactically complete comment.
#[must_use]
pub fn unwrap_comment_delimiters(
    language: LanguageId,
    rendered: &str,
    st_dialect: StCommentDialect,
) -> Option<String> {
    let lf = normalize_eol_to_lf(rendered);
    match language {
        LanguageId::Ladder | LanguageId::Sfc => Some(lf),
        LanguageId::Cpp => {
            if lf.starts_with("/*") {
                let mut cur = Cursor::new(&lf);
                let (body, _, _) = lex_c_block_comment(&mut cur).ok()??;
                if cur.is_eof() { Some(body) } else { None }
            } else if lf.starts_with("//") {
                unwrap_slash_lines(&lf, true)
            } else {
                None
            }
        }
        LanguageId::St => {
            if lf.starts_with("(*") {
                let mut cur = Cursor::new(&lf);
                let (body, _) =
                    lex_nested_paren_comment(&mut cur, st_dialect.supports_nested_paren_comments())
                        .ok()??;
                if cur.is_eof() { Some(body) } else { None }
            } else if lf.starts_with("/*") && st_dialect.supports_c_block_comments() {
                let mut cur = Cursor::new(&lf);
                let (body, _, _) = lex_st_comment(&mut cur, st_dialect).ok()??;
                if cur.is_eof() { Some(body) } else { None }
            } else if lf.starts_with("//") && st_dialect.supports_line_comments() {
                unwrap_slash_lines(&lf, false)
            } else {
                None
            }
        }
    }
}

fn unwrap_slash_lines(lf: &str, strip_doc_prefix: bool) -> Option<String> {
    let mut lines = Vec::new();
    for line in lf.split('\n') {
        let stripped = if strip_doc_prefix {
            if let Some(rest) = line.strip_prefix("///") {
                if rest.starts_with('/') {
                    line.strip_prefix("//")?
                } else {
                    rest
                }
            } else if let Some(rest) = line.strip_prefix("//!") {
                rest
            } else {
                line.strip_prefix("//")?
            }
        } else {
            line.strip_prefix("//")?
        };
        lines.push(stripped);
    }
    Some(lines.join("\n"))
}

/// Re-import idempotence matcher (`C4`): returns `true` iff `rendered` (either raw with delimiters
/// OR already stripped to parsed comment body) matches a valid projection of `stored_text`
/// under `language` and `caps`.
#[must_use]
pub fn matches_comment_with_caps(
    language: LanguageId,
    caps: &LanguageCapabilities,
    rendered: &str,
    stored_text: &str,
) -> bool {
    let mut ctx = BoardContext::new("matcher", "matcher", 32);
    ctx.comment_encoding = caps.comment_encoding;
    ctx.st_dialect = caps.st_dialect.unwrap_or_default();
    matches_comment_with_ctx(language, &ctx, rendered, stored_text)
}

/// Re-import idempotence matcher (`C4`) parameterized by a full [`BoardContext`].
#[must_use]
pub fn matches_comment_with_ctx(
    language: LanguageId,
    ctx: &BoardContext,
    rendered: &str,
    stored_text: &str,
) -> bool {
    let rendered_lf = normalize_eol_to_lf(rendered);
    let stored_lf = normalize_eol_to_lf(stored_text);

    // 1. Direct verbatim body match
    if rendered_lf == stored_lf {
        return true;
    }

    let unwrapped_input = unwrap_comment_delimiters(language, &rendered_lf, ctx.st_dialect);
    if unwrapped_input.as_deref() == Some(stored_lf.as_str()) {
        return true;
    }

    // 2. Compare against every canonical projection that `render_comment_for_language` can produce
    //    for `stored_text` under the active language, dialect, and comment_encoding.
    const STYLES: &[CommentStyle] = &[
        CommentStyle::Line,
        CommentStyle::Block,
        CommentStyle::ParenBlock,
        CommentStyle::DocLine,
        CommentStyle::DocBlock,
        CommentStyle::Visual,
    ];
    const PLACEMENTS: &[CommentPlacement] = &[
        CommentPlacement::Leading,
        CommentPlacement::Trailing,
        CommentPlacement::Header,
        CommentPlacement::Inner { token_ordinal: 0 },
    ];

    for &style in STYLES {
        for &placement in PLACEMENTS {
            let candidate =
                render_comment_for_language(language, &stored_lf, style, placement, ctx);
            if candidate.rendered == rendered_lf {
                return true;
            }
            if let Some(candidate_body) =
                unwrap_comment_delimiters(language, &candidate.rendered, ctx.st_dialect)
            {
                if candidate_body == rendered_lf {
                    return true;
                }
                if unwrapped_input.as_deref() == Some(candidate_body.as_str()) {
                    return true;
                }
            }
        }
    }

    false
}

/// Relocates `CommentPlacement::Inner` comments to `CommentPlacement::Leading` on their enclosing
/// node boundary for cross-language or visual emission (`01` §7, `04` §4.3).
///
/// Emits one `INFO_COMMENT_RELOCATED` diagnostic per relocated comment and preserves the exact
/// verbatim comment text multiset (`C1`).
#[must_use]
pub fn relocate_inner_comments(
    comments: &[Comment],
    target_language: LanguageId,
    board_id: Option<&str>,
) -> (Vec<Comment>, Vec<Diagnostic>) {
    let mut projected = Vec::with_capacity(comments.len());
    let mut diagnostics = Vec::new();
    let is_visual_target = matches!(target_language, LanguageId::Ladder | LanguageId::Sfc);

    for comment in comments {
        let mut copy = comment.clone();
        if copy.placement == AstCommentPlacement::Inner
            && (is_visual_target || copy.origin.language != target_language.as_str())
        {
            copy.placement = AstCommentPlacement::Leading;
            copy.token_ordinal = None;
            let mut diag = Diagnostic::new(
                ErrorCode::InfoCommentRelocated,
                format!(
                    "inner comment `{}` relocated to enclosing statement boundary for `{}` emission",
                    copy.id,
                    target_language.as_str()
                ),
            );
            if let Some(bid) = board_id {
                diag = diag.with_board_id(bid);
            }
            diag.span = Some(copy.span);
            diagnostics.push(diag);
        }
        projected.push(copy);
    }

    (projected, diagnostics)
}

/// Non-destructively transitions comments anchored on deleted nodes into `dangling` comments on
/// each node's parent scope (`C2` orphan preservation, `INFO_COMMENT_ORPHANED`).
///
/// Never deletes any user commentary.
#[must_use]
pub fn orphan_comments_for_deleted_nodes(
    comments: &mut [Comment],
    deleted_to_parent: &BTreeMap<NodeId, NodeId>,
    board_id: Option<&str>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for comment in comments {
        if let CommentAnchor::Node { id } = &comment.anchor
            && let Some(parent_scope) = deleted_to_parent.get(id)
        {
            let deleted_label = id.to_string();
            comment.anchor = CommentAnchor::Node {
                id: parent_scope.clone(),
            };
            comment.placement = AstCommentPlacement::Dangling;
            comment.token_ordinal = None;

            let mut diag = Diagnostic::new(
                ErrorCode::InfoCommentOrphaned,
                format!(
                    "comment `{}` orphaned after deleting node `{deleted_label}`; preserved as `dangling` on `{parent_scope}`",
                    comment.id
                ),
            );
            if let Some(bid) = board_id {
                diag = diag.with_board_id(bid);
            }
            diag.span = Some(comment.span);
            diagnostics.push(diag);
        }
    }
    diagnostics
}

/// Convenience helper for orphaning all comments anchored on any node in `deleted_nodes`
/// onto a single `parent_scope` (`INFO_COMMENT_ORPHANED`).
#[must_use]
pub fn orphan_comments_to_parent(
    comments: &mut [Comment],
    deleted_nodes: &[NodeId],
    parent_scope: &NodeId,
    board_id: Option<&str>,
) -> Vec<Diagnostic> {
    let map: BTreeMap<NodeId, NodeId> = deleted_nodes
        .iter()
        .map(|node| (node.clone(), parent_scope.clone()))
        .collect();
    orphan_comments_for_deleted_nodes(comments, &map, board_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{CommentOrigin, comment_multiset};
    use proptest::prelude::*;

    fn test_ctx(encoding: CommentEncoding, dialect: StCommentDialect) -> BoardContext {
        let mut ctx = BoardContext::plc_1200_open_1214("plc_main");
        ctx.comment_encoding = encoding;
        ctx.st_dialect = dialect;
        ctx
    }

    #[test]
    fn nested_st_paren_comment_lexing_respects_dialect_def_006() {
        let src = "(* outer (* inner (* deep *) inner *) outer *) rest";
        let mut cur_ed3 = Cursor::new(src);
        let (body, style, span) = lex_st_comment(&mut cur_ed3, StCommentDialect::Iec61131Ed3)
            .unwrap()
            .unwrap();
        assert_eq!(style, CommentStyle::ParenBlock);
        assert_eq!(body, " outer (* inner (* deep *) inner *) outer ");
        assert_eq!(cur_ed3.remaining(), " rest");
        assert_eq!(
            span.slice(src),
            Some("(* outer (* inner (* deep *) inner *) outer *)")
        );

        // StrictEd2 is non-nested: closes at the FIRST `*)`
        let mut cur_ed2 = Cursor::new("(* simple *) next");
        let (body2, style2, _) = lex_st_comment(&mut cur_ed2, StCommentDialect::StrictEd2)
            .unwrap()
            .unwrap();
        assert_eq!(style2, CommentStyle::ParenBlock);
        assert_eq!(body2, " simple ");
        assert_eq!(cur_ed2.remaining(), " next");

        // StrictEd2 does NOT accept `//` or `/* */`
        let mut cur_line = Cursor::new("// not ed2");
        assert_eq!(
            lex_st_comment(&mut cur_line, StCommentDialect::StrictEd2).unwrap(),
            None
        );
        let mut cur_cblock = Cursor::new("/* not ed2 */");
        assert_eq!(
            lex_st_comment(&mut cur_cblock, StCommentDialect::StrictEd2).unwrap(),
            None
        );

        // ClassicPlc accepts `//` and `(* *)` (non-nested), rejects `/* */`
        let mut cur_classic_line = Cursor::new("// ok in classic");
        assert!(
            lex_st_comment(&mut cur_classic_line, StCommentDialect::ClassicPlc)
                .unwrap()
                .is_some()
        );
        let mut cur_classic_cblock = Cursor::new("/* nope */");
        assert_eq!(
            lex_st_comment(&mut cur_classic_cblock, StCommentDialect::ClassicPlc).unwrap(),
            None
        );

        // Unterminated nested comment produces structured diagnostic with exact span
        let mut cur_unterm = Cursor::new("(* outer (* inner *)");
        let err = lex_st_comment(&mut cur_unterm, StCommentDialect::RuSTy).unwrap_err();
        assert_eq!(err.code, ErrorCode::ErrCommentField);
        assert_eq!(err.span, Some(Span::new(0, 20)));
    }

    #[test]
    fn contiguous_leading_line_comment_merging_splits_on_blank_line_and_trailing() {
        let raw = vec![
            RawComment {
                text: " line 1".to_owned(),
                style: CommentStyle::Line,
                span: Span::new(0, 9),
                start_line: 1,
                end_line: 1,
                blank_lines_before: 0,
                preceded_by_code_on_line: false,
            },
            RawComment {
                text: " line 2".to_owned(),
                style: CommentStyle::Line,
                span: Span::new(10, 19),
                start_line: 2,
                end_line: 2,
                blank_lines_before: 0,
                preceded_by_code_on_line: false,
            },
            // Blank line splits the next run
            RawComment {
                text: " paragraph 2".to_owned(),
                style: CommentStyle::Line,
                span: Span::new(21, 35),
                start_line: 4,
                end_line: 4,
                blank_lines_before: 1,
                preceded_by_code_on_line: false,
            },
            // Trailing comment on next line MUST NOT merge
            RawComment {
                text: " trailing".to_owned(),
                style: CommentStyle::Line,
                span: Span::new(45, 56),
                start_line: 5,
                end_line: 5,
                blank_lines_before: 0,
                preceded_by_code_on_line: true,
            },
        ];

        let merged = merge_contiguous_leading_comments(raw);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, " line 1\n line 2");
        assert_eq!(merged[0].span, Span::new(0, 19));
        assert_eq!(merged[1].text, " paragraph 2");
        assert_eq!(merged[2].text, " trailing");
    }

    #[test]
    fn cpp_and_st_hazard_avoidance_and_c4_idempotence() {
        let ctx_ed3 = test_ctx(CommentEncoding::Utf8Passthrough, StCommentDialect::RuSTy);
        let ctx_ed2 = test_ctx(
            CommentEncoding::Utf8Passthrough,
            StCommentDialect::StrictEd2,
        );

        // 1. C++ `//` ending with `\` -> projected as block comment `/* ... */`
        let r1 = render_comment_for_language(
            LanguageId::Cpp,
            " path C:\\temp\\",
            CommentStyle::Line,
            CommentPlacement::Leading,
            &ctx_ed3,
        );
        assert_eq!(r1.emitted_style, CommentStyle::Block);
        assert_eq!(r1.rendered, "/* path C:\\temp\\*/");
        assert!(r1.degraded);
        assert!(matches_comment_with_ctx(
            LanguageId::Cpp,
            &ctx_ed3,
            &r1.rendered,
            " path C:\\temp\\"
        ));

        // 2. C++ block containing `*/` -> projected as `//` line comment
        let r2 = render_comment_for_language(
            LanguageId::Cpp,
            " end */ danger",
            CommentStyle::Block,
            CommentPlacement::Leading,
            &ctx_ed3,
        );
        assert_eq!(r2.emitted_style, CommentStyle::Line);
        assert_eq!(r2.rendered, "// end */ danger");
        assert!(matches_comment_with_ctx(
            LanguageId::Cpp,
            &ctx_ed3,
            &r2.rendered,
            " end */ danger"
        ));

        // 3. C++ BOTH `*/` AND trailing `\` -> non-destructive block escape
        let r3 = render_comment_for_language(
            LanguageId::Cpp,
            " both */ and \\",
            CommentStyle::Block,
            CommentPlacement::Leading,
            &ctx_ed3,
        );
        assert_eq!(r3.emitted_style, CommentStyle::Block);
        let inner3 = r3
            .rendered
            .strip_prefix("/*")
            .unwrap()
            .strip_suffix("*/")
            .unwrap();
        assert!(!inner3.contains("*/"));
        assert!(!inner3.contains("/*"));
        assert!(matches_comment_with_ctx(
            LanguageId::Cpp,
            &ctx_ed3,
            &r3.rendered,
            " both */ and \\"
        ));

        // 4. ST StrictEd2 with `*)` and `(*)` -> escaped `(* ... *)`, zero unescaped delimiters
        let r4 = render_comment_for_language(
            LanguageId::St,
            " hostile *) and (*) and (",
            CommentStyle::ParenBlock,
            CommentPlacement::Leading,
            &ctx_ed2,
        );
        assert_eq!(r4.emitted_style, CommentStyle::ParenBlock);
        assert!(r4.degraded);
        // Must re-lex as a single complete StrictEd2 comment
        let mut cur = Cursor::new(&r4.rendered);
        let (reparsed_body, _, _) = lex_st_comment(&mut cur, StCommentDialect::StrictEd2)
            .unwrap()
            .unwrap();
        assert!(cur.is_eof());
        // C4 idempotence: matches BOTH full rendered form AND re-parsed inner body!
        assert!(matches_comment_with_ctx(
            LanguageId::St,
            &ctx_ed2,
            &r4.rendered,
            " hostile *) and (*) and ("
        ));
        assert!(matches_comment_with_ctx(
            LanguageId::St,
            &ctx_ed2,
            &reparsed_body,
            " hostile *) and (*) and ("
        ));
        // Genuine human edit MUST return false!
        assert!(!matches_comment_with_ctx(
            LanguageId::St,
            &ctx_ed2,
            "edited by human",
            " hostile *) and (*) and ("
        ));

        // 5. ST RuSTy keeps balanced nested `(* ... *)` verbatim!
        let r5 = render_comment_for_language(
            LanguageId::St,
            " outer (* inner *) outer ",
            CommentStyle::ParenBlock,
            CommentPlacement::Leading,
            &ctx_ed3,
        );
        assert_eq!(r5.rendered, "(* outer (* inner *) outer *)");
        assert!(!r5.degraded);
    }

    #[test]
    fn bidi_and_encoding_degradation_emit_warnings_and_preserve_c4() {
        let ctx_fold = test_ctx(CommentEncoding::AsciiFold, StCommentDialect::RuSTy);
        let original = " Motor ação \u{202E}24V ±5% 🚀";
        let rendered = render_comment_for_language(
            LanguageId::Cpp,
            original,
            CommentStyle::Line,
            CommentPlacement::Leading,
            &ctx_fold,
        );
        assert!(rendered.degraded);
        assert!(rendered.rendered.is_ascii());
        assert!(!contains_bidi_control(&rendered.rendered));
        assert!(rendered.rendered.contains("\\u{202E}"));
        assert!(rendered.rendered.contains("acao"));
        assert!(rendered.rendered.contains("+/-5%"));
        assert!(
            rendered
                .diagnostics
                .iter()
                .any(|d| d.code == ErrorCode::WarnCommentBidi)
        );
        assert!(
            rendered
                .diagnostics
                .iter()
                .any(|d| d.code == ErrorCode::WarnCommentEncoding)
        );
        assert!(matches_comment_with_ctx(
            LanguageId::Cpp,
            &ctx_fold,
            &rendered.rendered,
            original
        ));

        // Under Utf8Passthrough, replacing `ação` with `acao` is NOT an accidental match
        let ctx_utf8 = test_ctx(CommentEncoding::Utf8Passthrough, StCommentDialect::RuSTy);
        assert!(!matches_comment_with_ctx(
            LanguageId::Cpp,
            &ctx_utf8,
            "//acao",
            "ação"
        ));
    }

    #[test]
    fn relocate_inner_and_orphan_preserve_c1_multiset_and_emit_info_diagnostics() {
        let mut comments = vec![
            Comment {
                id: "c_1".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_stmt_1"),
                },
                placement: AstCommentPlacement::Inner,
                token_ordinal: Some(2),
                text: " gain factor".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Block,
                },
                span: Span::new(10, 26),
            },
            Comment {
                id: "c_2".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_stmt_1"),
                },
                placement: AstCommentPlacement::Leading,
                token_ordinal: None,
                text: " drive motor".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Line,
                },
                span: Span::new(0, 14),
            },
        ];

        // Relocate for cross-language ST emission
        let (relocated, reloc_diags) =
            relocate_inner_comments(&comments, LanguageId::St, Some("plc_main"));
        assert_eq!(reloc_diags.len(), 1);
        assert_eq!(reloc_diags[0].code, ErrorCode::InfoCommentRelocated);
        assert_eq!(relocated[0].placement, AstCommentPlacement::Leading);
        assert_eq!(relocated[0].token_ordinal, None);
        assert_eq!(comment_multiset(&comments), comment_multiset(&relocated));

        // Same-language C++ keeps Inner comment in place
        let (same_lang, same_diags) =
            relocate_inner_comments(&comments, LanguageId::Cpp, Some("mcu_1"));
        assert!(same_diags.is_empty());
        assert_eq!(same_lang[0].placement, AstCommentPlacement::Inner);
        assert_eq!(same_lang[0].token_ordinal, Some(2));

        // Orphan comments when n_stmt_1 is deleted
        let before: BTreeMap<String, usize> = comment_multiset(&comments)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        let orphan_diags = orphan_comments_to_parent(
            &mut comments,
            &[NodeId::new("n_stmt_1")],
            &NodeId::new("n_ob_main"),
            Some("plc_main"),
        );
        assert_eq!(orphan_diags.len(), 2);
        assert!(
            orphan_diags
                .iter()
                .all(|d| d.code == ErrorCode::InfoCommentOrphaned)
        );
        assert!(
            comments
                .iter()
                .all(|c| c.placement == AstCommentPlacement::Dangling && c.token_ordinal.is_none())
        );
        let after: BTreeMap<String, usize> = comment_multiset(&comments)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn eol_normalization_and_restoration_round_trip() {
        let crlf = "a\r\nb\r\nc";
        let lf = normalize_eol_to_lf(crlf);
        assert_eq!(lf, "a\nb\nc");
        assert_eq!(restore_eol(&lf, EolStyle::Crlf), crlf);
        assert_eq!(restore_eol(crlf, EolStyle::Crlf), crlf);
        assert_eq!(restore_eol(crlf, EolStyle::Lf), lf);
    }

    #[test]
    fn explicit_overlapping_delimiter_escapes_are_hazard_free() {
        for tricky in ["*/*", "/*/", "/*/*", "*/**/", "///", "/"] {
            let escaped = escape_c_block_delimiters(tricky);
            assert!(!escaped.contains("/*"), "{tricky:?} -> {escaped:?}");
            assert!(!escaped.contains("*/"), "{tricky:?} -> {escaped:?}");
            assert!(!escaped.ends_with('/'), "{tricky:?} -> {escaped:?}");
        }
        for tricky in ["(*)", "(*(*)", "*)*)", "((*))", "(((", "("] {
            let escaped = escape_st_paren_delimiters(tricky);
            assert!(!escaped.contains("(*"), "{tricky:?} -> {escaped:?}");
            assert!(!escaped.contains("*)"), "{tricky:?} -> {escaped:?}");
            assert!(!escaped.ends_with('('), "{tricky:?} -> {escaped:?}");
        }
    }

    proptest! {
        #[test]
        fn st_and_c_delimiter_escapers_always_relex_cleanly_as_single_comment(
            raw in "[a-zA-Z0-9 ()*/\\\\\n\t]{0,64}"
        ) {
            let st_body = escape_st_paren_delimiters(&raw);
            prop_assert!(!st_body.contains("(*"));
            prop_assert!(!st_body.contains("*)"));
            prop_assert!(!st_body.ends_with('('));
            let st_wrapped = format!("(*{st_body}*)");
            for dialect in StCommentDialect::ALL {
                let mut cur = Cursor::new(&st_wrapped);
                let parsed = lex_st_comment(&mut cur, *dialect);
                prop_assert!(parsed.is_ok(), "failed to relex {st_wrapped:?} in {dialect:?}");
                prop_assert!(parsed.unwrap().is_some());
                prop_assert!(cur.is_eof());
            }

            let c_body = escape_c_block_delimiters(&raw);
            prop_assert!(!c_body.contains("/*"));
            prop_assert!(!c_body.contains("*/"));
            prop_assert!(!c_body.ends_with('/'));
            let c_wrapped = format!("/*{c_body}*/");
            let mut cur_c = Cursor::new(&c_wrapped);
            let parsed_c = lex_c_block_comment(&mut cur_c);
            prop_assert!(parsed_c.is_ok(), "failed to relex {c_wrapped:?}");
            prop_assert!(parsed_c.unwrap().is_some());
            prop_assert!(cur_c.is_eof());
        }
    }
}
