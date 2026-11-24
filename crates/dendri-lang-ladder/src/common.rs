//! Shared Ladder helpers: FB pin metadata, span shifting, string escaping,
//! and hazard-safe Ladder comment rendering.

use dendri_ast::{
    CallArg, CommentEncoding, CommentStyle, Expr, ExprKind, FbKind, LValue, LadderCompareOp,
    ScalarType, TypeRef,
};
use dendri_frontend_core::{
    BoardContext, ascii_fold, escape_bidi_controls, has_balanced_paren_delimiters,
    normalize_eol_to_lf,
};
use dendri_span::{Diagnostic, ErrorCode, Span};

/// Canonical IEC textual keyword for an [`FbKind`].
#[must_use]
pub(crate) fn fb_kind_name(kind: &FbKind) -> &str {
    match kind {
        FbKind::Ton => "TON",
        FbKind::Tof => "TOF",
        FbKind::Tp => "TP",
        FbKind::Ctu => "CTU",
        FbKind::Ctd => "CTD",
        FbKind::Ctud => "CTUD",
        FbKind::RTrig => "R_TRIG",
        FbKind::FTrig => "F_TRIG",
        FbKind::Sr => "SR",
        FbKind::Rs => "RS",
        FbKind::UartSend => "UART_SEND",
        FbKind::UartRecv => "UART_RECV",
        FbKind::I2cXfer => "I2C_XFER",
        FbKind::SpiXfer => "SPI_XFER",
        FbKind::MbClient => "MB_CLIENT",
        FbKind::MbServer => "MB_SERVER",
        FbKind::User { fb_name } => fb_name.as_str(),
    }
}

/// Parses a built-in [`FbKind`] keyword (case-insensitive); returns `None` for user FB names.
#[must_use]
pub(crate) fn parse_builtin_fb_kind(raw: &str) -> Option<FbKind> {
    if raw.eq_ignore_ascii_case("TON") {
        Some(FbKind::Ton)
    } else if raw.eq_ignore_ascii_case("TOF") {
        Some(FbKind::Tof)
    } else if raw.eq_ignore_ascii_case("TP") {
        Some(FbKind::Tp)
    } else if raw.eq_ignore_ascii_case("CTU") {
        Some(FbKind::Ctu)
    } else if raw.eq_ignore_ascii_case("CTD") {
        Some(FbKind::Ctd)
    } else if raw.eq_ignore_ascii_case("CTUD") {
        Some(FbKind::Ctud)
    } else if raw.eq_ignore_ascii_case("R_TRIG") {
        Some(FbKind::RTrig)
    } else if raw.eq_ignore_ascii_case("F_TRIG") {
        Some(FbKind::FTrig)
    } else if raw.eq_ignore_ascii_case("SR") {
        Some(FbKind::Sr)
    } else if raw.eq_ignore_ascii_case("RS") {
        Some(FbKind::Rs)
    } else if raw.eq_ignore_ascii_case("UART_SEND") {
        Some(FbKind::UartSend)
    } else if raw.eq_ignore_ascii_case("UART_RECV") {
        Some(FbKind::UartRecv)
    } else if raw.eq_ignore_ascii_case("I2C_XFER") {
        Some(FbKind::I2cXfer)
    } else if raw.eq_ignore_ascii_case("SPI_XFER") {
        Some(FbKind::SpiXfer)
    } else if raw.eq_ignore_ascii_case("MB_CLIENT") {
        Some(FbKind::MbClient)
    } else if raw.eq_ignore_ascii_case("MB_SERVER") {
        Some(FbKind::MbServer)
    } else {
        None
    }
}

/// Parses any [`FbKind`] identifier (built-in or `FbKind::User`).
#[must_use]
pub(crate) fn parse_fb_kind(raw: &str) -> FbKind {
    parse_builtin_fb_kind(raw).unwrap_or_else(|| FbKind::User {
        fb_name: raw.to_owned(),
    })
}

/// Primary IEC boolean power-flow trigger input pin for an [`FbKind`] (`docs/lbc/ladder.md` §1.2).
#[must_use]
pub(crate) const fn fb_primary_input_pin(kind: &FbKind) -> &'static str {
    match kind {
        FbKind::Ton | FbKind::Tof | FbKind::Tp => "IN",
        FbKind::Ctu | FbKind::Ctud => "CU",
        FbKind::Ctd => "CD",
        FbKind::RTrig | FbKind::FTrig => "CLK",
        FbKind::Sr => "S1",
        FbKind::Rs => "S",
        FbKind::UartSend | FbKind::I2cXfer | FbKind::SpiXfer | FbKind::MbClient => "REQ",
        FbKind::UartRecv => "EN_R",
        FbKind::MbServer => "ENABLE",
        FbKind::User { .. } => "EN",
    }
}

/// Primary IEC boolean power-flow output pin propagated onto `out_rail` (`docs/lbc/ladder.md` §1.2).
#[must_use]
pub(crate) const fn fb_primary_output_pin(kind: &FbKind) -> &'static str {
    match kind {
        FbKind::Ton
        | FbKind::Tof
        | FbKind::Tp
        | FbKind::Ctu
        | FbKind::Ctd
        | FbKind::RTrig
        | FbKind::FTrig => "Q",
        FbKind::Ctud => "QU",
        FbKind::Sr | FbKind::Rs => "Q1",
        FbKind::UartSend | FbKind::I2cXfer | FbKind::SpiXfer | FbKind::MbClient => "DONE",
        FbKind::UartRecv | FbKind::MbServer => "NDR",
        FbKind::User { .. } => "ENO",
    }
}

/// Canonical IEC field type for built-in Function Blocks (`dendri-stdlib` catalog).
#[must_use]
pub(crate) fn builtin_fb_field_type(kind: &FbKind, field: &str) -> Option<TypeRef> {
    let f = field.to_ascii_uppercase();
    let ty = match (kind, f.as_str()) {
        (FbKind::Ton | FbKind::Tof | FbKind::Tp, "IN" | "Q") => ScalarType::Bool,
        (FbKind::Ton | FbKind::Tof | FbKind::Tp, "PT" | "ET") => ScalarType::Time,
        (FbKind::Ctu, "CU" | "R" | "Q") => ScalarType::Bool,
        (FbKind::Ctu, "PV" | "CV") => ScalarType::Dint,
        (FbKind::Ctd, "CD" | "LD" | "Q") => ScalarType::Bool,
        (FbKind::Ctd, "PV" | "CV") => ScalarType::Dint,
        (FbKind::Ctud, "CU" | "CD" | "R" | "LD" | "QU" | "QD") => ScalarType::Bool,
        (FbKind::Ctud, "PV" | "CV") => ScalarType::Dint,
        (FbKind::RTrig | FbKind::FTrig, "CLK" | "Q") => ScalarType::Bool,
        (FbKind::Sr, "S1" | "R" | "Q1") => ScalarType::Bool,
        (FbKind::Rs, "S" | "R1" | "Q1") => ScalarType::Bool,
        (FbKind::UartSend, "REQ" | "DONE" | "ERR") => ScalarType::Bool,
        (FbKind::UartSend, "DATA") => ScalarType::String { max_len: 255 },
        (FbKind::UartRecv, "EN_R" | "NDR" | "ERR") => ScalarType::Bool,
        (FbKind::UartRecv, "DATA") => ScalarType::String { max_len: 255 },
        (FbKind::I2cXfer, "REQ" | "DONE" | "ERR") => ScalarType::Bool,
        (FbKind::I2cXfer, "ADDR") => ScalarType::Usint,
        (FbKind::SpiXfer, "REQ" | "DONE") => ScalarType::Bool,
        (FbKind::SpiXfer, "TX" | "RX") => ScalarType::Byte,
        (FbKind::MbClient, "REQ" | "DONE" | "ERR") => ScalarType::Bool,
        (FbKind::MbClient, "FC") => ScalarType::Usint,
        (FbKind::MbClient, "ADDR") => ScalarType::Uint,
        (FbKind::MbServer, "ENABLE" | "NDR" | "ERR") => ScalarType::Bool,
        _ => return None,
    };
    Some(TypeRef::scalar(ty))
}

/// Canonical keyword for a [`LadderCompareOp`].
#[must_use]
pub(crate) const fn compare_op_name(op: LadderCompareOp) -> &'static str {
    match op {
        LadderCompareOp::Eq => "EQ",
        LadderCompareOp::Ne => "NE",
        LadderCompareOp::Lt => "LT",
        LadderCompareOp::Le => "LE",
        LadderCompareOp::Gt => "GT",
        LadderCompareOp::Ge => "GE",
    }
}

/// Parses a strictly typed [`LadderCompareOp`] keyword (`EQ`..`GE`).
#[must_use]
pub(crate) fn parse_compare_op(raw: &str) -> Option<LadderCompareOp> {
    if raw.eq_ignore_ascii_case("EQ") {
        Some(LadderCompareOp::Eq)
    } else if raw.eq_ignore_ascii_case("NE") {
        Some(LadderCompareOp::Ne)
    } else if raw.eq_ignore_ascii_case("LT") {
        Some(LadderCompareOp::Lt)
    } else if raw.eq_ignore_ascii_case("LE") {
        Some(LadderCompareOp::Le)
    } else if raw.eq_ignore_ascii_case("GT") {
        Some(LadderCompareOp::Gt)
    } else if raw.eq_ignore_ascii_case("GE") {
        Some(LadderCompareOp::Ge)
    } else {
        None
    }
}

/// Shifts all byte spans inside `expr` by `offset`.
pub(crate) fn shift_expr_spans(expr: &mut Expr, offset: u32) {
    expr.span = shift_span(expr.span, offset);
    match &mut expr.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::Foreign { .. } => {}
        ExprKind::Index { base, index } => {
            shift_expr_spans(base, offset);
            shift_expr_spans(index, offset);
        }
        ExprKind::Field { base, field: _ } => {
            shift_expr_spans(base, offset);
        }
        ExprKind::Call { callee: _, args } => {
            for arg in args {
                shift_call_arg_spans(arg, offset);
            }
        }
        ExprKind::Unary { op: _, operand } => {
            shift_expr_spans(operand, offset);
        }
        ExprKind::Binary { op: _, left, right } => {
            shift_expr_spans(left, offset);
            shift_expr_spans(right, offset);
        }
    }
}

fn shift_call_arg_spans(arg: &mut CallArg, offset: u32) {
    arg.span = shift_span(arg.span, offset);
    shift_expr_spans(&mut arg.value, offset);
}

/// Shifts all byte spans inside `lvalue` by `offset`.
pub(crate) fn shift_lvalue_spans(lvalue: &mut LValue, offset: u32) {
    match lvalue {
        LValue::Var { name: _, span } => {
            *span = shift_span(*span, offset);
        }
        LValue::Index { base, index, span } => {
            *span = shift_span(*span, offset);
            shift_lvalue_spans(base, offset);
            shift_expr_spans(index, offset);
        }
        LValue::Field {
            base,
            field: _,
            span,
        } => {
            *span = shift_span(*span, offset);
            shift_lvalue_spans(base, offset);
        }
    }
}

#[must_use]
pub(crate) const fn shift_span(span: Span, offset: u32) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}

/// Formats a UTF-8 string as a double-quoted Ladder literal with canonical escapes.
#[must_use]
pub(crate) fn format_string_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Projects a verbatim comment string for Ladder textual emission (`//` and nested `(* ... *)`),
/// applying Trojan-Source bidi escaping (`WARN_COMMENT_BIDI`) and `ctx.comment_encoding`
/// (`WARN_COMMENT_ENCODING`) without mutating stored text.
#[must_use]
pub(crate) fn project_comment_body(
    text: &str,
    ctx: &BoardContext,
    diags: &mut Vec<Diagnostic>,
) -> String {
    let lf = normalize_eol_to_lf(text);
    let (after_bidi, had_bidi) = escape_bidi_controls(&lf);
    if had_bidi {
        diags.push(
            Diagnostic::new(
                ErrorCode::WarnCommentBidi,
                "comment contains Unicode bidirectional control characters; rendered escaped in Ladder projection",
            )
            .with_board_id(&ctx.board_id)
            .with_hint(
                "Stored verbatim comment text is unchanged; remove bidi controls to silence this warning.",
            ),
        );
    }
    match ctx.comment_encoding {
        CommentEncoding::Utf8Passthrough => after_bidi,
        CommentEncoding::WarnOnly => {
            if !after_bidi.is_ascii() {
                diags.push(
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
                diags.push(
                    Diagnostic::new(
                        ErrorCode::WarnCommentEncoding,
                        "comment contains non-ASCII text; folded to ASCII in emitted Ladder projection only",
                    )
                    .with_board_id(&ctx.board_id)
                    .with_hint("Stored verbatim UTF-8 comment text in ASL is preserved intact."),
                );
            }
            folded
        }
    }
}

/// Renders a comment for Ladder textual output (`//` lines or hazard-free nested `(* ... *)`).
///
/// Guarantees zero delimiter hazards and exact re-lexability for every string in `C3`.
#[must_use]
pub(crate) fn render_ladder_text_comment(
    text: &str,
    preferred_style: CommentStyle,
    indent: &str,
    inline_only: bool,
    ctx: &BoardContext,
    diags: &mut Vec<Diagnostic>,
) -> String {
    let body = project_comment_body(text, ctx, diags);
    let paren_safe = has_balanced_paren_delimiters(&body) && !body.ends_with('(');

    if inline_only {
        if !body.contains('\n') {
            return format!("//{body}");
        }
        if paren_safe {
            return format!("(*{body}*)");
        }
        let escaped = dendri_frontend_core::escape_st_paren_delimiters(&body);
        return format!("(*{escaped}*)");
    }

    let use_paren = matches!(preferred_style, CommentStyle::ParenBlock) && paren_safe;
    if use_paren {
        format!("{indent}(*{body}*)")
    } else {
        let mut out = String::with_capacity(body.len().saturating_add(16));
        for (idx, line) in body.split('\n').enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            out.push_str(indent);
            out.push_str("//");
            out.push_str(line);
        }
        out
    }
}
