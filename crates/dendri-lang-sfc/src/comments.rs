//! ADR-027 verbatim human comment rendering & `C4` re-import idempotence matching
//! for `dendri-lang-sfc` (`docs/lbc/sfc.md` §4 & §8).
//!
//! Supported comment styles:
//! - `CommentStyle::Visual` (raw visual canvas note with bidi escaping & encoding policy)
//! - `CommentStyle::Line` (`// ...` single/multi-line with zero delimiter hazards)
//! - `CommentStyle::ParenBlock` (nested IEC `(* ... *)` block comments)

use dendri_ast::{CommentEncoding, CommentStyle};
use dendri_frontend_core::comments::{
    ascii_fold, escape_bidi_controls, escape_st_paren_delimiters, has_balanced_paren_delimiters,
    lex_nested_paren_comment, normalize_eol_to_lf,
};
use dendri_frontend_core::cursor::Cursor;
use dendri_frontend_core::traits::{
    BoardContext, CommentPlacement, LanguageCapabilities, RenderedComment,
};
use dendri_span::{Diagnostic, ErrorCode};

/// Builds the normative [`LanguageCapabilities`] for `dendri-lang-sfc` (`docs/lbc/sfc.md` §8).
#[must_use]
pub(crate) fn sfc_capabilities(comment_encoding: CommentEncoding) -> LanguageCapabilities {
    LanguageCapabilities {
        supported_comment_styles: vec![
            CommentStyle::Visual,
            CommentStyle::Line,
            CommentStyle::ParenBlock,
        ],
        supports_nested_paren_comments: true,
        supports_line_comments: true,
        comment_encoding,
        capabilities: vec!["asl-core@1".to_owned()],
        st_dialect: None,
    }
}

/// Renders a verbatim comment for `dendri-lang-sfc` without mutating stored text (`ADR-027`).
///
/// - Applies Trojan-Source bidi escaping (`WARN_COMMENT_BIDI`)
/// - Applies target `ctx.comment_encoding` (`WARN_COMMENT_ENCODING`)
/// - Preserves `CommentStyle::Visual` for visual canvas views, `//` for `Line`, and
///   nested `(* ... *)` for `ParenBlock` with automatic hazard avoidance (`*)`, unbalanced `(*`,
///   trailing `(`).
#[must_use]
pub(crate) fn render_sfc_comment(
    text: &str,
    style: CommentStyle,
    placement: CommentPlacement,
    ctx: &BoardContext,
) -> RenderedComment {
    let (projected, mut degraded, diagnostics) = apply_bidi_and_encoding(text, ctx);
    let is_inner = matches!(placement, CommentPlacement::Inner { .. });
    let paren_safe = has_balanced_paren_delimiters(&projected) && !projected.ends_with('(');

    match style {
        CommentStyle::Visual => RenderedComment {
            rendered: projected,
            emitted_style: CommentStyle::Visual,
            degraded,
            diagnostics,
        },
        CommentStyle::Line | CommentStyle::DocLine => {
            if is_inner {
                let (body, did_escape) = if paren_safe {
                    (projected, false)
                } else {
                    (escape_st_paren_delimiters(&projected), true)
                };
                RenderedComment {
                    rendered: format!("(*{body}*)"),
                    emitted_style: CommentStyle::ParenBlock,
                    degraded: degraded || did_escape || style != CommentStyle::ParenBlock,
                    diagnostics,
                }
            } else {
                RenderedComment {
                    rendered: render_slash_lines(&projected),
                    emitted_style: CommentStyle::Line,
                    degraded: degraded || style == CommentStyle::DocLine,
                    diagnostics,
                }
            }
        }
        CommentStyle::ParenBlock | CommentStyle::Block | CommentStyle::DocBlock => {
            if paren_safe {
                RenderedComment {
                    rendered: format!("(*{projected}*)"),
                    emitted_style: CommentStyle::ParenBlock,
                    degraded: degraded || style != CommentStyle::ParenBlock,
                    diagnostics,
                }
            } else if !is_inner {
                degraded = true;
                RenderedComment {
                    rendered: render_slash_lines(&projected),
                    emitted_style: CommentStyle::Line,
                    degraded,
                    diagnostics,
                }
            } else {
                degraded = true;
                let escaped = escape_st_paren_delimiters(&projected);
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

/// Renders a comment specifically for textual SFC emission (`emit()`), guaranteeing that even
/// `CommentStyle::Visual` notes emit with valid textual delimiters (`(* ... *)` or `// ...`)
/// and round-trip verbatim through the SFC lexer (`C1`).
#[must_use]
pub(crate) fn render_sfc_textual_comment(
    text: &str,
    style: CommentStyle,
    placement: CommentPlacement,
    ctx: &BoardContext,
) -> RenderedComment {
    let effective_style = match style {
        CommentStyle::Visual => CommentStyle::ParenBlock,
        CommentStyle::Line
        | CommentStyle::Block
        | CommentStyle::ParenBlock
        | CommentStyle::DocLine
        | CommentStyle::DocBlock => style,
    };
    render_sfc_comment(text, effective_style, placement, ctx)
}

/// Re-import idempotence matcher (`C4`) for `dendri-lang-sfc`.
///
/// Returns `true` iff `rendered` (raw text or delimited `//` / nested `(* ... *)`) represents a
/// valid SFC projection of `stored_text` under `encoding`.
#[must_use]
pub(crate) fn matches_sfc_comment(
    rendered: &str,
    stored_text: &str,
    encoding: CommentEncoding,
) -> bool {
    let rendered_lf = normalize_eol_to_lf(rendered);
    let stored_lf = normalize_eol_to_lf(stored_text);

    if rendered_lf == stored_lf {
        return true;
    }

    let unwrapped_input = unwrap_sfc_comment_delimiters(&rendered_lf);
    if unwrapped_input.as_deref() == Some(stored_lf.as_str()) {
        return true;
    }

    let mut ctx = BoardContext::new("sfc_matcher", "sfc_matcher", 32);
    ctx.comment_encoding = encoding;

    const STYLES: &[CommentStyle] = &[
        CommentStyle::Visual,
        CommentStyle::Line,
        CommentStyle::ParenBlock,
        CommentStyle::Block,
        CommentStyle::DocLine,
        CommentStyle::DocBlock,
    ];
    const PLACEMENTS: &[CommentPlacement] = &[
        CommentPlacement::Leading,
        CommentPlacement::Trailing,
        CommentPlacement::Header,
        CommentPlacement::Title,
        CommentPlacement::Body,
        CommentPlacement::Dangling,
        CommentPlacement::Inner { token_ordinal: 0 },
    ];

    for &style in STYLES {
        for &placement in PLACEMENTS {
            let candidate = render_sfc_comment(&stored_lf, style, placement, &ctx);
            if candidate.rendered == rendered_lf {
                return true;
            }
            if let Some(candidate_body) = unwrap_sfc_comment_delimiters(&candidate.rendered) {
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

/// Strips `//` or nested `(* ... *)` delimiters from a complete rendered SFC comment.
#[must_use]
pub(crate) fn unwrap_sfc_comment_delimiters(lf: &str) -> Option<String> {
    if lf.starts_with("(*") {
        let mut cur = Cursor::new(lf);
        let (body, _) = lex_nested_paren_comment(&mut cur, true).ok()??;
        if cur.is_eof() { Some(body) } else { None }
    } else if lf.starts_with("//") {
        let mut lines = Vec::new();
        for line in lf.split('\n') {
            let stripped = line.strip_prefix("//")?;
            lines.push(stripped);
        }
        Some(lines.join("\n"))
    } else {
        Some(lf.to_owned())
    }
}

fn apply_bidi_and_encoding(text: &str, ctx: &BoardContext) -> (String, bool, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let mut degraded = false;
    let lf = normalize_eol_to_lf(text);

    let (after_bidi, had_bidi) = escape_bidi_controls(&lf);
    if had_bidi {
        degraded = true;
        diagnostics.push(
            Diagnostic::new(
                ErrorCode::WarnCommentBidi,
                "comment contains Unicode bidirectional control characters; rendered escaped in SFC projection",
            )
            .with_board_id(&ctx.board_id)
            .with_hint(
                "Stored verbatim comment text is unchanged; remove bidi controls to silence this warning.",
            ),
        );
    }

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
                        "comment contains non-ASCII text; folded to ASCII in emitted SFC projection only",
                    )
                    .with_board_id(&ctx.board_id)
                    .with_hint("Stored verbatim UTF-8 comment text in ASL is preserved intact."),
                );
            }
            folded
        }
    };

    (projected, degraded, diagnostics)
}

fn render_slash_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(16));
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str("//");
        out.push_str(line);
    }
    out
}
