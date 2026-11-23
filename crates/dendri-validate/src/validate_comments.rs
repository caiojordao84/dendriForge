//! Validation of first-class verbatim human comments (`01-asl-ir-and-project-schema.md` §7, ADR-027).

use dendri_ast::{
    AuthoringLanguage, Comment, CommentAnchor, CommentEncoding, CommentPlacement, NodeId,
    comment::contains_bidi_control,
};
use dendri_span::{Diagnostic, DiagnosticBag, ErrorCode, FileId};
use std::collections::BTreeSet;

/// Context for validating comments at project level or board level.
pub(crate) struct CommentValidationCtx<'a> {
    pub file_id: Option<FileId>,
    pub board_ids: &'a BTreeSet<String>,
    pub component_ids: &'a BTreeSet<String>,
    pub wire_ids: &'a BTreeSet<String>,
    pub comment_encoding: CommentEncoding,
    pub export_target_language: Option<AuthoringLanguage>,
}

/// Validates a single comment record against §7 rules (`ERR_COMMENT_FIELD`, `ERR_COMMENT_ANCHOR`,
/// `WARN_COMMENT_BIDI`, `WARN_COMMENT_ENCODING`, `INFO_COMMENT_RELOCATED`, `INFO_COMMENT_ORPHANED`).
pub(crate) fn validate_comment(
    comment: &Comment,
    board_id: Option<&str>,
    board_node_ids: Option<&BTreeSet<NodeId>>,
    ctx: &CommentValidationCtx<'_>,
    seen_comment_ids: &mut BTreeSet<String>,
    bag: &mut DiagnosticBag,
) {
    let push = |bag: &mut DiagnosticBag, code: ErrorCode, msg: String| {
        let mut diag = Diagnostic::new(code, msg);
        if let Some(bid) = board_id {
            diag = diag.with_board_id(bid);
        }
        if let Some(fid) = ctx.file_id {
            diag = diag.with_location(fid, comment.span);
        } else {
            diag.span = Some(comment.span);
        }
        bag.push(diag);
    };

    if !seen_comment_ids.insert(comment.id.clone()) {
        push(
            bag,
            ErrorCode::ErrDuplicateId,
            format!("duplicate comment id `{}`", comment.id),
        );
    }

    // 1. Validate placement <-> token_ordinal & anchor-kind rules (`ERR_COMMENT_FIELD`)
    match comment.placement {
        CommentPlacement::Inner => {
            if comment.token_ordinal.is_none() {
                push(
                    bag,
                    ErrorCode::ErrCommentField,
                    format!(
                        "comment `{}` with placement=\"inner\" MUST specify `token_ordinal`",
                        comment.id
                    ),
                );
            }
        }
        CommentPlacement::Leading
        | CommentPlacement::Trailing
        | CommentPlacement::Dangling
        | CommentPlacement::Header
        | CommentPlacement::Title
        | CommentPlacement::Body => {
            if comment.token_ordinal.is_some() {
                push(
                    bag,
                    ErrorCode::ErrCommentField,
                    format!(
                        "comment `{}` with placement={:?} MUST NOT specify `token_ordinal`",
                        comment.id, comment.placement
                    ),
                );
            }
        }
    }

    if !comment.anchor.is_node()
        && !matches!(
            comment.placement,
            CommentPlacement::Header | CommentPlacement::Body
        )
    {
        push(
            bag,
            ErrorCode::ErrCommentField,
            format!(
                "non-node comment `{}` allows only `header` or `body` placement (found {:?})",
                comment.id, comment.placement
            ),
        );
    }

    // 2. Validate anchor resolution (`ERR_COMMENT_ANCHOR`)
    match &comment.anchor {
        CommentAnchor::Node { id } => match board_node_ids {
            Some(nodes) if nodes.contains(id) => {}
            _ => push(
                bag,
                ErrorCode::ErrCommentAnchor,
                format!(
                    "comment `{}` references unresolved node anchor `{id}`",
                    comment.id
                ),
            ),
        },
        CommentAnchor::Project => {}
        CommentAnchor::Board { id } => {
            if !ctx.board_ids.contains(id) {
                push(
                    bag,
                    ErrorCode::ErrCommentAnchor,
                    format!(
                        "comment `{}` references unresolved board anchor `{id}`",
                        comment.id
                    ),
                );
            }
        }
        CommentAnchor::Component { id } => {
            if !ctx.component_ids.contains(id) {
                push(
                    bag,
                    ErrorCode::ErrCommentAnchor,
                    format!(
                        "comment `{}` references unresolved component anchor `{id}`",
                        comment.id
                    ),
                );
            }
        }
        CommentAnchor::Wire { id } => {
            if !ctx.wire_ids.contains(id) {
                push(
                    bag,
                    ErrorCode::ErrCommentAnchor,
                    format!(
                        "comment `{}` references unresolved wire anchor `{id}`",
                        comment.id
                    ),
                );
            }
        }
    }

    // 3. Trojan-Source bidi control detection (`WARN_COMMENT_BIDI`)
    if contains_bidi_control(&comment.text) {
        push(
            bag,
            ErrorCode::WarnCommentBidi,
            format!(
                "comment `{}` contains Unicode bidirectional control characters (escaped on emission)",
                comment.id
            ),
        );
    }

    // 4. Target comment encoding warning (`WARN_COMMENT_ENCODING`)
    if matches!(
        ctx.comment_encoding,
        CommentEncoding::AsciiFold | CommentEncoding::WarnOnly
    ) && !comment.text.is_ascii()
    {
        push(
            bag,
            ErrorCode::WarnCommentEncoding,
            format!(
                "comment `{}` contains non-ASCII text under {:?} target encoding (projection-only degradation)",
                comment.id, ctx.comment_encoding
            ),
        );
    }

    // 5. Cross-language inner comment relocation (`INFO_COMMENT_RELOCATED`)
    if comment.placement == CommentPlacement::Inner
        && let Some(target_lang) = ctx.export_target_language
        && target_lang.as_str() != comment.origin.language
    {
        push(
            bag,
            ErrorCode::InfoCommentRelocated,
            format!(
                "inner comment `{}` relocates to nearest statement boundary when emitting `{}`",
                comment.id,
                target_lang.as_str()
            ),
        );
    }

    // 6. Orphaned / dangling scope comment (`INFO_COMMENT_ORPHANED`)
    if comment.placement == CommentPlacement::Dangling {
        push(
            bag,
            ErrorCode::InfoCommentOrphaned,
            format!(
                "comment `{}` is orphaned (`dangling`) on parent scope and visible in Comments Panel",
                comment.id
            ),
        );
    }
}
