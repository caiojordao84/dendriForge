//! Shared frontend conformance suite helpers (`04` §2, `05` §4 Gate 8 `C1–C5`).
//!
//! Reusable across every Wave 1–3 language frontend crate (`dendri-lang-cpp`,
//! `dendri-lang-st`, `dendri-lang-ladder`, `dendri-lang-sfc`):
//! - Hostile comment corpus (`C3`) and automated safety/idempotence verifier
//! - Comment conservation checkers (`C1–C5`)
//! - `LOSSLESS_ASL` structural equality checker (span stripping + canonical `NodeId` normalization)
//! - Banned machine magic-comment detector (`ADR-020`)
//! - Auto-generated feature-matrix collector and Markdown table generator (`04` §2)

use crate::comments::{
    contains_bidi_control, has_cpp_block_comment_hazard, has_cpp_line_splice_hazard,
    lex_cpp_comment, lex_st_comment, orphan_comments_to_parent,
};
use crate::cursor::Cursor;
use crate::traits::{
    BoardContext, BoardProgramBundle, CommentPlacement, LanguageFrontend, LanguageId,
};
use dendri_ast::{
    BoardProgram, CasePattern, Comment, CommentEncoding, CommentPlacement as AstCommentPlacement,
    CommentStyle, DataDecl, ElifArm, HardwareBinding, LadderBoxKind, LadderCell, LadderCellKind,
    LadderNetwork, LadderRung, ModuleDecl, NodeId, Pou, PouBody, SfcAction, SfcChart, SfcStep,
    SfcTransition, Stmt, StmtKind, SwitchCase, TypeDecl, comment_multiset,
};
use dendri_span::{ErrorCode, Span};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Hostile Comment Corpus (`C3`) & Verifier
// ---------------------------------------------------------------------------

/// Single adversarial comment test vector in the `C3` hostile corpus (`05` §4 Gate 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostileCommentCase {
    pub id: &'static str,
    pub style: CommentStyle,
    pub placement: CommentPlacement,
    pub text: String,
    pub expects_bidi_warning: bool,
    pub is_non_ascii: bool,
}

/// Returns the normative `C3` hostile comment corpus covering delimiter hazards, C++ trailing `\`,
/// nested/unbalanced ST `(* ... *)`, CRLF/multiline, NFD combining marks, emoji, Trojan-Source
/// bidi controls, very long lines, exact whitespace preservation, and empty comments.
#[must_use]
pub fn hostile_comment_corpus() -> Vec<HostileCommentCase> {
    let long_line = format!(" long-{}", "x".repeat(520));
    vec![
        HostileCommentCase {
            id: "c3_c_block_closer",
            style: CommentStyle::Block,
            placement: CommentPlacement::Leading,
            text: " contains */ closing C block ".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_st_paren_closer",
            style: CommentStyle::ParenBlock,
            placement: CommentPlacement::Leading,
            text: " contains *) closing ST paren block ".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_st_nested_paren",
            style: CommentStyle::ParenBlock,
            placement: CommentPlacement::Leading,
            text: " outer (* inner (* deep *) inner *) outer ".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_st_unbalanced_open",
            style: CommentStyle::ParenBlock,
            placement: CommentPlacement::Leading,
            text: " unbalanced (* opener".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_st_overlapping_paren_star",
            style: CommentStyle::ParenBlock,
            placement: CommentPlacement::Leading,
            text: " overlapping (*) sequence".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_st_trailing_lparen",
            style: CommentStyle::ParenBlock,
            placement: CommentPlacement::Leading,
            text: " ends with (".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_cpp_trailing_backslash",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: " line splice hazard \\".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_cpp_trailing_backslash_spaces",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: " line splice with spaces \\   ".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_combined_backslash_and_block_closer",
            style: CommentStyle::Block,
            placement: CommentPlacement::Leading,
            text: " hazard */ and /* and trailing \\".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_multiline_paragraph",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: " line one\n line two\n line three".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_combining_marks_nfd",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: " cafe\u{0301} vs caf\u{00E9} na\u{0308}ive".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: true,
        },
        HostileCommentCase {
            id: "c3_emoji_and_engineering_utf8",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: " Motor OK \u{2699}\u{FE0F} \u{1F680} 24V \u{00B1}5% 500\u{03A9}".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: true,
        },
        HostileCommentCase {
            id: "c3_bidi_rlo_trojan",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: " access \u{202E}admin\u{2066}user\u{2069}".to_owned(),
            expects_bidi_warning: true,
            is_non_ascii: true,
        },
        HostileCommentCase {
            id: "c3_bidi_lrm_rlm",
            style: CommentStyle::Block,
            placement: CommentPlacement::Leading,
            text: " sensor \u{200E}A\u{200F} value".to_owned(),
            expects_bidi_warning: true,
            is_non_ascii: true,
        },
        HostileCommentCase {
            id: "c3_very_long_line",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: long_line,
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_exact_whitespace",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: "   keep exact spaces   ".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_empty_comment",
            style: CommentStyle::Block,
            placement: CommentPlacement::Leading,
            text: String::new(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_mixed_delimiters_inner",
            style: CommentStyle::Block,
            placement: CommentPlacement::Inner { token_ordinal: 1 },
            text: " /* fake */ (* also *) // slash ".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_cpp_phase2_spliced_block_closer",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: "*\\\n/".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_cpp_doc_line_collision_slash",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: "/ slash".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_cpp_doc_line_collision_bang",
            style: CommentStyle::Line,
            placement: CommentPlacement::Leading,
            text: "! bang".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
        HostileCommentCase {
            id: "c3_cpp_doc_block_collision_star",
            style: CommentStyle::Block,
            placement: CommentPlacement::Leading,
            text: "* star".to_owned(),
            expects_bidi_warning: false,
            is_non_ascii: false,
        },
    ]
}

/// Runs the complete `C3` hostile comment corpus against `frontend` under `ctx`, checking:
/// 1. Zero unescaped Trojan-Source bidi controls in emitted output + mandatory `WARN_COMMENT_BIDI`.
/// 2. Target `CommentEncoding` enforcement (`WARN_COMMENT_ENCODING`, pure ASCII under `AsciiFold`).
/// 3. Complete re-lexability of emitted C++/ST comments with zero premature closure or line-splicing.
/// 4. `C4` re-import idempotence (`frontend.matches_comment` returns `true` for every case).
pub fn verify_hostile_corpus_on_frontend(
    frontend: &dyn LanguageFrontend,
    ctx: &BoardContext,
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    let lang = frontend.id();

    for case in hostile_comment_corpus() {
        let rendered = frontend.render_comment(&case.text, case.style, case.placement, ctx);

        if contains_bidi_control(&rendered.rendered) {
            failures.push(format!(
                "[{}] raw bidi control survived in rendered output: {:?}",
                case.id, rendered.rendered
            ));
        }

        let has_bidi_warn = rendered
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::WarnCommentBidi);
        if case.expects_bidi_warning && !has_bidi_warn {
            failures.push(format!("[{}] missing WARN_COMMENT_BIDI", case.id));
        }

        if ctx.comment_encoding == CommentEncoding::AsciiFold && !rendered.rendered.is_ascii() {
            failures.push(format!(
                "[{}] non-ASCII output under AsciiFold: {:?}",
                case.id, rendered.rendered
            ));
        }

        let has_enc_warn = rendered
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::WarnCommentEncoding);
        if matches!(
            ctx.comment_encoding,
            CommentEncoding::AsciiFold | CommentEncoding::WarnOnly
        ) && case.is_non_ascii
            && !case.expects_bidi_warning
            && !has_enc_warn
        {
            failures.push(format!(
                "[{}] missing WARN_COMMENT_ENCODING for non-ASCII case",
                case.id
            ));
        }

        // Verify emitted textual comment re-lexes completely with zero stray trailing tokens
        if let Err(reason) = verify_rendered_comment_lexes_cleanly(lang, &rendered.rendered, ctx) {
            failures.push(format!("[{}] hazard check failed: {reason}", case.id));
        }

        // Verify C4 re-import idempotence when frontend capabilities align with ctx
        let caps = frontend.capabilities();
        let caps_match_ctx = caps.comment_encoding == ctx.comment_encoding
            && (lang != LanguageId::St || caps.st_dialect == Some(ctx.st_dialect));
        if caps_match_ctx && !frontend.matches_comment(&rendered.rendered, &case.text) {
            failures.push(format!(
                "[{}] C4 idempotence failed: matches_comment({:?}, {:?}) == false",
                case.id, rendered.rendered, case.text
            ));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

fn verify_rendered_comment_lexes_cleanly(
    lang: LanguageId,
    rendered: &str,
    ctx: &BoardContext,
) -> Result<(), String> {
    match lang {
        LanguageId::Ladder | LanguageId::Sfc => Ok(()),
        LanguageId::Cpp => {
            let mut cur = Cursor::new(rendered);
            let mut count = 0usize;
            let mut first_style: Option<CommentStyle> = None;
            while !cur.is_eof() {
                let Some((body, style, _)) = lex_cpp_comment(&mut cur).map_err(|d| d.message)?
                else {
                    return Err(format!(
                        "unexpected non-comment tail in C++ projection: {:?}",
                        cur.remaining()
                    ));
                };
                if let Some(prev_style) = first_style {
                    if prev_style != style {
                        return Err(format!(
                            "C++ multi-line projection split styles: {prev_style:?} vs {style:?}"
                        ));
                    }
                } else {
                    first_style = Some(style);
                }
                if matches!(style, CommentStyle::Line | CommentStyle::DocLine)
                    && has_cpp_line_splice_hazard(&body)
                {
                    return Err(format!(
                        "C++ line-splice backslash hazard survived in line comment: {body:?}"
                    ));
                }
                if matches!(style, CommentStyle::Block | CommentStyle::DocBlock)
                    && has_cpp_block_comment_hazard(&body)
                {
                    return Err(format!(
                        "C++ block/phase-2 delimiter hazard survived in block comment: {body:?}"
                    ));
                }
                count = count.saturating_add(1);
                let _ = cur.eat_newline();
            }
            if count == 0 {
                return Err("empty C++ rendered comment".to_owned());
            }
            Ok(())
        }
        LanguageId::St => {
            let mut cur = Cursor::new(rendered);
            let mut count = 0usize;
            while !cur.is_eof() {
                let Some((_, _, _)) =
                    lex_st_comment(&mut cur, ctx.st_dialect).map_err(|d| d.message)?
                else {
                    return Err(format!(
                        "unexpected non-comment tail in ST {:?} projection: {:?}",
                        ctx.st_dialect,
                        cur.remaining()
                    ));
                };
                count = count.saturating_add(1);
                let _ = cur.eat_newline();
            }
            if count == 0 {
                return Err("empty ST rendered comment".to_owned());
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Comment Conservation Checkers (`C1–C5`) & Magic-Comment Guard
// ---------------------------------------------------------------------------

/// `C1`: Verifies that the multiset of verbatim comment texts is invariant across transformations.
pub fn assert_c1_multiset_invariant(before: &[Comment], after: &[Comment]) -> Result<(), String> {
    let left = comment_multiset(before);
    let right = comment_multiset(after);
    if left == right {
        Ok(())
    } else {
        Err(format!(
            "C1 comment multiset mismatch:\n  before: {left:?}\n  after:  {right:?}"
        ))
    }
}

/// `C2`: Verifies that deleting `deleted_node` transitions anchored comments to `dangling` on
/// `parent_scope` with `INFO_COMMENT_ORPHANED` and zero loss of verbatim comment text.
pub fn assert_c2_orphan_preservation(
    comments: &[Comment],
    deleted_node: &NodeId,
    parent_scope: &NodeId,
    board_id: Option<&str>,
) -> Result<usize, String> {
    let mut cloned = comments.to_vec();
    let expected_orphaned = comments
        .iter()
        .filter(|c| match &c.anchor {
            dendri_ast::CommentAnchor::Node { id } => id == deleted_node,
            dendri_ast::CommentAnchor::Project
            | dendri_ast::CommentAnchor::Board { .. }
            | dendri_ast::CommentAnchor::Component { .. }
            | dendri_ast::CommentAnchor::Wire { .. } => false,
        })
        .count();

    let diags = orphan_comments_to_parent(
        &mut cloned,
        std::slice::from_ref(deleted_node),
        parent_scope,
        board_id,
    );
    assert_c1_multiset_invariant(comments, &cloned)?;

    if diags.len() != expected_orphaned {
        return Err(format!(
            "C2 diagnostic count mismatch: expected {expected_orphaned}, got {}",
            diags.len()
        ));
    }
    if diags
        .iter()
        .any(|d| d.code != ErrorCode::InfoCommentOrphaned)
    {
        return Err("C2 expected strictly INFO_COMMENT_ORPHANED diagnostics".to_owned());
    }
    Ok(expected_orphaned)
}

/// `C4`: Verifies re-import idempotence on every comment in `comments` via `frontend`.
pub fn assert_c4_reimport_idempotence(
    frontend: &dyn LanguageFrontend,
    comments: &[Comment],
    ctx: &BoardContext,
) -> Result<(), String> {
    for comment in comments {
        let placement = CommentPlacement::from_comment(comment)
            .map_err(|code| format!("comment `{}` failed placement check: {code}", comment.id))?;
        let rendered = frontend.render_comment(&comment.text, comment.origin.style, placement, ctx);
        if !frontend.matches_comment(&rendered.rendered, &comment.text) {
            return Err(format!(
                "C4 idempotence failed for comment `{}`: rendered={:?}, stored={:?}",
                comment.id, rendered.rendered, comment.text
            ));
        }
    }
    Ok(())
}

/// `C5`: Validates field invariants (`ERR_COMMENT_FIELD`) across a slice of comments.
pub fn assert_c5_comment_fields(comments: &[Comment]) -> Result<(), Vec<ErrorCode>> {
    let mut errors = Vec::new();
    for comment in comments {
        if CommentPlacement::from_comment(comment).is_err() {
            errors.push(ErrorCode::ErrCommentField);
        }
        if !comment.anchor.is_node()
            && !matches!(
                comment.placement,
                AstCommentPlacement::Header | AstCommentPlacement::Body
            )
        {
            errors.push(ErrorCode::ErrCommentField);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Returns `true` if `emitted_source` contains any permanently banned v1 machine magic comment
/// (`// ASL_HW:`, `// ASL_DATA:`, `(* HW: ... *)`, `04` §4.1, ADR-020).
#[must_use]
pub fn contains_banned_magic_comment(emitted_source: &str) -> bool {
    emitted_source.contains("ASL_HW:")
        || emitted_source.contains("ASL_DATA:")
        || emitted_source.contains("(* HW:")
}

/// End-to-end `LOSSLESS_ASL` + `C1` + ADR-020 verification helper for any [`LanguageFrontend`]:
/// runs `emit -> parse` and verifies structural ASL identity, comment multiset conservation,
/// and absence of banned machine magic comments or `M = 10^9` literals.
pub fn verify_frontend_roundtrip(
    frontend: &dyn LanguageFrontend,
    bundle: &BoardProgramBundle,
    ctx: &BoardContext,
) -> Result<BoardProgramBundle, String> {
    let emitted = frontend
        .emit(bundle, ctx)
        .map_err(|diags| format!("emit failed: {diags:?}"))?;
    if contains_banned_magic_comment(&emitted.value) {
        return Err("ADR-020 violation: emitted output contains banned magic comment".to_owned());
    }
    if crate::numeric_emit::contains_analog_full_scale_literal(&emitted.value) {
        return Err(
            "04 §2 #3 violation: emitted output contains forbidden M = 10^9 literal".to_owned(),
        );
    }
    let reparsed = frontend
        .parse(&emitted.value, ctx)
        .map_err(|diags| format!("re-parse failed: {diags:?}"))?;
    assert_lossless_asl(&bundle.program, &reparsed.value.program)?;
    assert_c1_multiset_invariant(&bundle.comments, &reparsed.value.comments)?;
    Ok(reparsed.value)
}

// ---------------------------------------------------------------------------
// `LOSSLESS_ASL` Structural Equality Checker (`04` §2 #7)
// ---------------------------------------------------------------------------

/// Verifies `LOSSLESS_ASL` (`parse(emit(m)) == m`) ignoring source byte spans and normalizing
/// auto-generated `NodeId`s deterministically in traversal order.
pub fn assert_lossless_asl(expected: &BoardProgram, actual: &BoardProgram) -> Result<(), String> {
    let norm_expected = normalize_program_for_lossless_asl(expected);
    let norm_actual = normalize_program_for_lossless_asl(actual);
    if norm_expected == norm_actual {
        Ok(())
    } else {
        Err(format!(
            "LOSSLESS_ASL mismatch:\n  expected: {norm_expected:#?}\n  actual:   {norm_actual:#?}"
        ))
    }
}

/// Verifies `LOSSLESS_ASL` keeping exact `NodeId`s intact while ignoring source byte spans.
pub fn assert_lossless_asl_exact_ids(
    expected: &BoardProgram,
    actual: &BoardProgram,
) -> Result<(), String> {
    let stripped_expected = strip_program_spans(expected);
    let stripped_actual = strip_program_spans(actual);
    if stripped_expected == stripped_actual {
        Ok(())
    } else {
        Err(format!(
            "LOSSLESS_ASL (exact IDs) mismatch:\n  expected: {stripped_expected:#?}\n  actual:   {stripped_actual:#?}"
        ))
    }
}

/// Returns a deep clone of `program` with every source [`Span`] zeroed to `Span::default()`.
#[must_use]
pub fn strip_program_spans(program: &BoardProgram) -> BoardProgram {
    let mut norm = Normalizer {
        canonicalize_ids: false,
        next_id: 1,
    };
    norm.program(program)
}

/// Returns a deep clone of `program` with all spans zeroed AND all [`NodeId`]s deterministically
/// renumbered (`n_0001`, `n_0002`, ...) in traversal order for pure-text `LOSSLESS_ASL` checks.
#[must_use]
pub fn normalize_program_for_lossless_asl(program: &BoardProgram) -> BoardProgram {
    let mut norm = Normalizer {
        canonicalize_ids: true,
        next_id: 1,
    };
    norm.program(program)
}

struct Normalizer {
    canonicalize_ids: bool,
    next_id: u32,
}

impl Normalizer {
    fn id(&mut self, orig: &NodeId) -> NodeId {
        if self.canonicalize_ids {
            let id = NodeId::new(format!("n_{:04}", self.next_id));
            self.next_id = self.next_id.saturating_add(1);
            id
        } else {
            orig.clone()
        }
    }

    fn program(&mut self, p: &BoardProgram) -> BoardProgram {
        BoardProgram {
            imports: p.imports.clone(),
            hardware: p.hardware.iter().map(|h| self.hardware(h)).collect(),
            types: p.types.iter().map(|t| self.type_decl(t)).collect(),
            data: p.data.iter().map(|d| self.data_decl(d)).collect(),
            modules: p.modules.iter().map(|m| self.module(m)).collect(),
            pous: p.pous.iter().map(|pou| self.pou(pou)).collect(),
        }
    }

    fn hardware(&mut self, h: &HardwareBinding) -> HardwareBinding {
        HardwareBinding {
            id: self.id(&h.id),
            label: h.label.clone(),
            pin: h.pin.clone(),
            kind: h.kind,
            pull: h.pull,
            active_low: h.active_low,
            codec: h.codec.clone(),
            range: h.range.clone(),
            span: Span::default(),
        }
    }

    fn type_decl(&mut self, td: &TypeDecl) -> TypeDecl {
        match td {
            TypeDecl::Struct {
                id,
                name,
                fields,
                span: _,
            } => TypeDecl::Struct {
                id: self.id(id),
                name: name.clone(),
                fields: fields.clone(),
                span: Span::default(),
            },
            TypeDecl::Enum {
                id,
                name,
                backing,
                variants,
                span: _,
            } => TypeDecl::Enum {
                id: self.id(id),
                name: name.clone(),
                backing: backing.clone(),
                variants: variants.clone(),
                span: Span::default(),
            },
            TypeDecl::Foreign {
                id,
                name,
                language,
                fidelity,
                text,
                pack_hint,
                span: _,
            } => TypeDecl::Foreign {
                id: self.id(id),
                name: name.clone(),
                language: language.clone(),
                fidelity: *fidelity,
                text: text.clone(),
                pack_hint: pack_hint.clone(),
                span: Span::default(),
            },
        }
    }

    fn data_decl(&mut self, d: &DataDecl) -> DataDecl {
        DataDecl {
            id: self.id(&d.id),
            name: d.name.clone(),
            scope: d.scope,
            ty: d.ty.clone(),
            initial: d.initial.as_ref().map(dendri_ast::Expr::strip_spans),
            span: Span::default(),
        }
    }

    fn module(&mut self, m: &ModuleDecl) -> ModuleDecl {
        ModuleDecl {
            id: self.id(&m.id),
            name: m.name.clone(),
            imports: m.imports.clone(),
            exports: m.exports.clone(),
            types: m.types.iter().map(|t| self.type_decl(t)).collect(),
            data: m.data.iter().map(|d| self.data_decl(d)).collect(),
            pous: m.pous.iter().map(|p| self.pou(p)).collect(),
            span: Span::default(),
        }
    }

    fn pou(&mut self, p: &Pou) -> Pou {
        Pou {
            id: self.id(&p.id),
            name: p.name.clone(),
            kind: p.kind.clone(),
            preferred_view: p.preferred_view,
            params: p.params.clone(),
            locals: p.locals.iter().map(|d| self.data_decl(d)).collect(),
            body: match &p.body {
                PouBody::Statements(stmts) => {
                    PouBody::Statements(stmts.iter().map(|s| self.stmt(s)).collect())
                }
                PouBody::Network(net) => PouBody::Network(self.network(net)),
                PouBody::Chart(chart) => PouBody::Chart(self.chart(chart)),
            },
            span: Span::default(),
        }
    }

    fn stmt(&mut self, s: &Stmt) -> Stmt {
        let id = self.id(&s.id);
        let kind = match &s.kind {
            StmtKind::Assign { target, value } => StmtKind::Assign {
                target: target.strip_spans(),
                value: value.strip_spans(),
            },
            StmtKind::Call { callee, args } => StmtKind::Call {
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|a| dendri_ast::CallArg {
                        name: a.name.clone(),
                        value: a.value.strip_spans(),
                        span: Span::default(),
                    })
                    .collect(),
            },
            StmtKind::FbInvoke { instance, inputs } => StmtKind::FbInvoke {
                instance: instance.strip_spans(),
                inputs: inputs
                    .iter()
                    .map(|(k, v)| (k.clone(), v.strip_spans()))
                    .collect(),
            },
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => StmtKind::If {
                condition: condition.strip_spans(),
                then_body: then_body.iter().map(|st| self.stmt(st)).collect(),
                elif_arms: elif_arms
                    .iter()
                    .map(|arm| ElifArm {
                        condition: arm.condition.strip_spans(),
                        body: arm.body.iter().map(|st| self.stmt(st)).collect(),
                    })
                    .collect(),
                else_body: else_body.iter().map(|st| self.stmt(st)).collect(),
            },
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => StmtKind::Switch {
                scrutinee: scrutinee.strip_spans(),
                cases: cases
                    .iter()
                    .map(|c| SwitchCase {
                        patterns: c
                            .patterns
                            .iter()
                            .map(|pat| match pat {
                                CasePattern::Value(e) => CasePattern::Value(e.strip_spans()),
                                CasePattern::Range { start, end } => CasePattern::Range {
                                    start: start.strip_spans(),
                                    end: end.strip_spans(),
                                },
                            })
                            .collect(),
                        body: c.body.iter().map(|st| self.stmt(st)).collect(),
                    })
                    .collect(),
                default: default
                    .as_ref()
                    .map(|body| body.iter().map(|st| self.stmt(st)).collect()),
            },
            StmtKind::While { condition, body } => StmtKind::While {
                condition: condition.strip_spans(),
                body: body.iter().map(|st| self.stmt(st)).collect(),
            },
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            } => StmtKind::For {
                counter: counter.clone(),
                from: from.strip_spans(),
                to: to.strip_spans(),
                step: step.strip_spans(),
                inclusive: *inclusive,
                body: body.iter().map(|st| self.stmt(st)).collect(),
            },
            StmtKind::Break => StmtKind::Break,
            StmtKind::Continue => StmtKind::Continue,
            StmtKind::Return { value } => StmtKind::Return {
                value: value.as_ref().map(dendri_ast::Expr::strip_spans),
            },
            StmtKind::Wait { duration } => StmtKind::Wait {
                duration: duration.strip_spans(),
            },
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => StmtKind::Foreign {
                language: language.clone(),
                fidelity: *fidelity,
                text: text.clone(),
                pack_hint: pack_hint.clone(),
            },
        };
        Stmt::new(id, kind, Span::default())
    }

    fn network(&mut self, net: &LadderNetwork) -> LadderNetwork {
        LadderNetwork {
            rungs: net
                .rungs
                .iter()
                .map(|r| LadderRung {
                    id: self.id(&r.id),
                    order: r.order,
                    cells: r.cells.iter().map(|c| self.ladder_cell(c)).collect(),
                })
                .collect(),
        }
    }

    fn ladder_cell(&mut self, c: &LadderCell) -> LadderCell {
        let id = self.id(&c.id);
        let kind = match &c.kind {
            LadderCellKind::Empty => LadderCellKind::Empty,
            LadderCellKind::Wire => LadderCellKind::Wire,
            LadderCellKind::Contact { kind, target } => LadderCellKind::Contact {
                kind: *kind,
                target: target.strip_spans(),
            },
            LadderCellKind::Coil { kind, target } => LadderCellKind::Coil {
                kind: *kind,
                target: target.strip_spans(),
            },
            LadderCellKind::ParallelOr { branches } => LadderCellKind::ParallelOr {
                branches: branches
                    .iter()
                    .map(|br| br.iter().map(|cell| self.ladder_cell(cell)).collect())
                    .collect(),
            },
            LadderCellKind::Box(bk) => LadderCellKind::Box(match bk {
                LadderBoxKind::Fb {
                    kind,
                    instance,
                    inputs,
                } => LadderBoxKind::Fb {
                    kind: kind.clone(),
                    instance: instance.strip_spans(),
                    inputs: inputs
                        .iter()
                        .map(|(k, v)| (k.clone(), v.strip_spans()))
                        .collect(),
                },
                LadderBoxKind::Compare { op, left, right } => LadderBoxKind::Compare {
                    op: *op,
                    left: left.strip_spans(),
                    right: right.strip_spans(),
                },
                LadderBoxKind::Call {
                    callee,
                    args,
                    output,
                } => LadderBoxKind::Call {
                    callee: callee.clone(),
                    args: args
                        .iter()
                        .map(|(name, e)| (name.clone(), e.strip_spans()))
                        .collect(),
                    output: output.as_ref().map(dendri_ast::LValue::strip_spans),
                },
                LadderBoxKind::Foreign {
                    language,
                    fidelity,
                    text,
                    pack_hint,
                } => LadderBoxKind::Foreign {
                    language: language.clone(),
                    fidelity: *fidelity,
                    text: text.clone(),
                    pack_hint: pack_hint.clone(),
                },
            }),
        };
        LadderCell {
            id,
            row: c.row,
            col: c.col,
            kind,
        }
    }

    fn chart(&mut self, ch: &SfcChart) -> SfcChart {
        SfcChart {
            steps: ch
                .steps
                .iter()
                .map(|step| SfcStep {
                    id: self.id(&step.id),
                    name: step.name.clone(),
                    initial: step.initial,
                    actions: step
                        .actions
                        .iter()
                        .map(|a| SfcAction {
                            id: self.id(&a.id),
                            qualifier: a.qualifier,
                            duration: a.duration.as_ref().map(dendri_ast::Expr::strip_spans),
                            target: a.target.as_ref().map(dendri_ast::LValue::strip_spans),
                            body: a.body.iter().map(|st| self.stmt(st)).collect(),
                        })
                        .collect(),
                    macro_chart: step.macro_chart.clone(),
                })
                .collect(),
            transitions: ch
                .transitions
                .iter()
                .map(|t| SfcTransition {
                    id: self.id(&t.id),
                    name: t.name.clone(),
                    from_steps: t.from_steps.clone(),
                    to_steps: t.to_steps.clone(),
                    priority: t.priority,
                    receptivity: t.receptivity.strip_spans(),
                })
                .collect(),
            evolve_to_stability: ch.evolve_to_stability,
        }
    }
}

// ---------------------------------------------------------------------------
// Generated Feature Matrix Collector & Markdown Generator (`04` §2)
// ---------------------------------------------------------------------------

/// Declared conformance support status for a single `(feature, language)` matrix cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureSupport {
    /// Full lossless lift + lower support verified by conformance tests.
    Full,
    /// Partial/canonical pattern support (e.g. conservative Ladder lifting).
    Partial,
    /// Preserved verbatim as `Foreign(F0)` for same-language round-trips.
    ForeignF0,
    /// Type-checked via hash-locked `F1` `pack.toml` signature.
    ForeignF1,
    /// Explicitly rejected with a frozen diagnostic `ErrorCode`.
    Unsupported,
}

impl FeatureSupport {
    /// Compact Markdown cell badge.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::Partial => "Partial",
            Self::ForeignF0 => "Foreign (F0)",
            Self::ForeignF1 => "Foreign (F1)",
            Self::Unsupported => "Unsupported",
        }
    }
}

/// Single test-evidenced feature-matrix observation (`04` §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureMatrixEntry {
    pub category: String,
    pub feature: String,
    pub language: LanguageId,
    pub support: FeatureSupport,
    pub evidence_test: String,
    pub notes: String,
}

/// Deterministic collector that aggregates conformance test outcomes and emits the
/// auto-generated Markdown feature matrix required by `04` §2.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureMatrixCollector {
    entries: Vec<FeatureMatrixEntry>,
}

impl FeatureMatrixCollector {
    /// Creates an empty collector.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Records a single conformance-verified feature entry.
    pub fn record(&mut self, entry: FeatureMatrixEntry) {
        self.entries.push(entry);
    }

    /// Borrows all recorded entries in insertion order.
    #[must_use]
    pub fn entries(&self) -> &[FeatureMatrixEntry] {
        &self.entries
    }

    /// Renders a deterministic, sorted Markdown feature matrix table across all Wave 1 languages.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        type RowKey<'a> = (&'a str, &'a str);
        let mut grouped: BTreeMap<RowKey<'_>, BTreeMap<LanguageId, &FeatureMatrixEntry>> =
            BTreeMap::new();

        for entry in &self.entries {
            grouped
                .entry((entry.category.as_str(), entry.feature.as_str()))
                .or_default()
                .insert(entry.language, entry);
        }

        let mut out = String::new();
        out.push_str(
            "<!-- Auto-generated by dendri-frontend-core FeatureMatrixCollector (04 §2) -->\n",
        );
        out.push_str("| Category | Feature | C++ | ST | Ladder | SFC | Evidence / Notes |\n");
        out.push_str("|---|---|---|---|---|---|---|\n");

        for ((category, feature), langs) in grouped {
            let cell = |id: LanguageId| -> &'static str {
                langs.get(&id).map_or("—", |e| e.support.badge())
            };
            let mut notes_parts: Vec<String> = Vec::new();
            for (&lang, entry) in &langs {
                if !entry.notes.is_empty() {
                    notes_parts.push(format!(
                        "`{}`: {} (`{}`)",
                        lang.as_str(),
                        entry.notes,
                        entry.evidence_test
                    ));
                } else {
                    notes_parts.push(format!("`{}`: `{}`", lang.as_str(), entry.evidence_test));
                }
            }
            out.push_str(&format!(
                "| {category} | {feature} | {} | {} | {} | {} | {} |\n",
                cell(LanguageId::Cpp),
                cell(LanguageId::St),
                cell(LanguageId::Ladder),
                cell(LanguageId::Sfc),
                notes_parts.join("; ")
            ));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::{matches_comment_with_caps, render_comment_for_language};
    use crate::traits::{
        FrontendOutput, FrontendResult, LanguageCapabilities, Paradigm, StCommentDialect,
    };
    use dendri_ast::{
        CommentAnchor, CommentOrigin, Expr, ExprKind, LValue, Literal, ObKind, PouKind,
    };

    struct StubCommentFrontend {
        lang: LanguageId,
        caps: LanguageCapabilities,
    }

    impl LanguageFrontend for StubCommentFrontend {
        fn id(&self) -> LanguageId {
            self.lang
        }

        fn paradigm(&self) -> Paradigm {
            self.lang.default_paradigm()
        }

        fn parse(&self, _src: &str, _ctx: &BoardContext) -> FrontendResult<BoardProgramBundle> {
            Ok(FrontendOutput::ok(BoardProgramBundle::default()))
        }

        fn emit(
            &self,
            _bundle: &BoardProgramBundle,
            _ctx: &BoardContext,
        ) -> FrontendResult<String> {
            Ok(FrontendOutput::ok(String::new()))
        }

        fn render_comment(
            &self,
            text: &str,
            style: CommentStyle,
            placement: CommentPlacement,
            ctx: &BoardContext,
        ) -> crate::traits::RenderedComment {
            render_comment_for_language(self.lang, text, style, placement, ctx)
        }

        fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool {
            matches_comment_with_caps(self.lang, &self.caps, rendered, stored_text)
        }

        fn capabilities(&self) -> LanguageCapabilities {
            self.caps.clone()
        }
    }

    #[test]
    fn hostile_c3_corpus_passes_across_all_languages_dialects_and_encodings() {
        for encoding in [
            CommentEncoding::Utf8Passthrough,
            CommentEncoding::AsciiFold,
            CommentEncoding::WarnOnly,
        ] {
            let mut cpp_ctx = BoardContext::arduino_uno_r3("mcu_1");
            cpp_ctx.comment_encoding = encoding;
            let cpp = StubCommentFrontend {
                lang: LanguageId::Cpp,
                caps: LanguageCapabilities::cpp(encoding),
            };
            assert_eq!(verify_hostile_corpus_on_frontend(&cpp, &cpp_ctx), Ok(()));

            for &dialect in StCommentDialect::ALL {
                let mut st_ctx = BoardContext::plc_1200_open_1214("plc_main");
                st_ctx.comment_encoding = encoding;
                st_ctx.st_dialect = dialect;
                let st = StubCommentFrontend {
                    lang: LanguageId::St,
                    caps: LanguageCapabilities::st(dialect, encoding),
                };
                assert_eq!(
                    verify_hostile_corpus_on_frontend(&st, &st_ctx),
                    Ok(()),
                    "C3 failed for ST {dialect:?} / {encoding:?}"
                );
            }

            let mut lad_ctx = BoardContext::plc_1200_open_1214("plc_main");
            lad_ctx.comment_encoding = encoding;
            let lad = StubCommentFrontend {
                lang: LanguageId::Ladder,
                caps: LanguageCapabilities::ladder(encoding),
            };
            assert_eq!(verify_hostile_corpus_on_frontend(&lad, &lad_ctx), Ok(()));
        }
    }

    #[test]
    fn lossless_asl_checker_normalizes_spans_and_node_ids() {
        let make_prog = |node_prefix: &str, span_offset: u32| BoardProgram {
            imports: vec!["std.iec.timers".to_owned()],
            hardware: vec![],
            types: vec![],
            data: vec![],
            modules: vec![],
            pous: vec![Pou {
                id: NodeId::new(format!("{node_prefix}_pou")),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: None,
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![Stmt::new(
                    NodeId::new(format!("{node_prefix}_stmt")),
                    StmtKind::Assign {
                        target: LValue::Var {
                            name: "x".to_owned(),
                            span: Span::new(span_offset, span_offset + 1),
                        },
                        value: Expr::new(
                            ExprKind::Literal(Literal::Int(42)),
                            Span::new(span_offset + 5, span_offset + 7),
                        ),
                    },
                    Span::new(span_offset, span_offset + 8),
                )]),
                span: Span::new(span_offset, span_offset + 20),
            }],
        };

        let a = make_prog("orig", 10);
        let b = make_prog("reparsed", 200);
        assert!(assert_lossless_asl(&a, &b).is_ok());
        assert!(assert_lossless_asl_exact_ids(&a, &b).is_err());
        assert!(assert_lossless_asl_exact_ids(&a, &make_prog("orig", 99)).is_ok());
    }

    #[test]
    fn c2_c5_magic_comment_and_feature_matrix_markdown_generation() {
        let comments = vec![Comment {
            id: "c_1".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_1"),
            },
            placement: AstCommentPlacement::Leading,
            token_ordinal: None,
            text: " preserve me".to_owned(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: CommentStyle::ParenBlock,
            },
            span: Span::new(0, 16),
        }];
        assert_eq!(assert_c5_comment_fields(&comments), Ok(()));
        assert_eq!(
            assert_c2_orphan_preservation(
                &comments,
                &NodeId::new("n_1"),
                &NodeId::new("n_root"),
                Some("plc_main")
            ),
            Ok(1)
        );
        assert!(contains_banned_magic_comment("// ASL_HW: led=D13"));
        assert!(contains_banned_magic_comment("(* HW: IW64 *)"));
        assert!(!contains_banned_magic_comment("// Regular human comment"));

        let mut collector = FeatureMatrixCollector::new();
        collector.record(FeatureMatrixEntry {
            category: "Comments".to_owned(),
            feature: "C1-C5 Verbatim Conservation".to_owned(),
            language: LanguageId::Cpp,
            support: FeatureSupport::Full,
            evidence_test: "cpp_c1_c5".to_owned(),
            notes: "trailing \\ -> block".to_owned(),
        });
        collector.record(FeatureMatrixEntry {
            category: "Comments".to_owned(),
            feature: "C1-C5 Verbatim Conservation".to_owned(),
            language: LanguageId::St,
            support: FeatureSupport::Full,
            evidence_test: "st_c1_c5".to_owned(),
            notes: "DEF-006 dialects".to_owned(),
        });
        let md = collector.to_markdown();
        assert!(md.contains("| Category | Feature | C++ | ST | Ladder | SFC | Evidence / Notes |"));
        assert!(md.contains("| Comments | C1-C5 Verbatim Conservation | Full | Full | — | — |"));
        assert!(md.contains("`cpp`: trailing \\ -> block (`cpp_c1_c5`)"));
    }

    #[test]
    fn verify_frontend_roundtrip_guards_magic_comments_and_analog_full_scale() {
        struct BadEmitFrontend {
            emitted: &'static str,
        }

        impl LanguageFrontend for BadEmitFrontend {
            fn id(&self) -> LanguageId {
                LanguageId::Cpp
            }
            fn paradigm(&self) -> Paradigm {
                Paradigm::Textual
            }
            fn parse(&self, _src: &str, _ctx: &BoardContext) -> FrontendResult<BoardProgramBundle> {
                Ok(FrontendOutput::ok(BoardProgramBundle::default()))
            }
            fn emit(
                &self,
                _bundle: &BoardProgramBundle,
                _ctx: &BoardContext,
            ) -> FrontendResult<String> {
                Ok(FrontendOutput::ok(self.emitted.to_owned()))
            }
            fn render_comment(
                &self,
                text: &str,
                style: CommentStyle,
                placement: CommentPlacement,
                ctx: &BoardContext,
            ) -> crate::traits::RenderedComment {
                render_comment_for_language(LanguageId::Cpp, text, style, placement, ctx)
            }
            fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool {
                matches_comment_with_caps(
                    LanguageId::Cpp,
                    &LanguageCapabilities::cpp(CommentEncoding::Utf8Passthrough),
                    rendered,
                    stored_text,
                )
            }
            fn capabilities(&self) -> LanguageCapabilities {
                LanguageCapabilities::cpp(CommentEncoding::Utf8Passthrough)
            }
        }

        let ctx = BoardContext::arduino_uno_r3("mcu_1");
        let bundle = BoardProgramBundle::default();

        let magic = BadEmitFrontend {
            emitted: "// ASL_HW: led=D13\nvoid loop() {}",
        };
        assert!(verify_frontend_roundtrip(&magic, &bundle, &ctx).is_err());

        let leaks_m = BadEmitFrontend {
            emitted: "uint64_t a = raw * 1000000000ULL / 1024ULL;",
        };
        assert!(verify_frontend_roundtrip(&leaks_m, &bundle, &ctx).is_err());

        let clean = BadEmitFrontend {
            emitted: "void loop() {}",
        };
        assert!(verify_frontend_roundtrip(&clean, &bundle, &ctx).is_ok());
    }
}
