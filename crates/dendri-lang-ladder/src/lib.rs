//! 100% pure-Rust IEC 61131-3 Ladder Diagram (`dendri-lang-ladder`) frontend
//! implementing [`dendri_frontend_core::LanguageFrontend`] (`Paradigm::Ladder`)
//! per `docs/lbc/ladder.md` and `docs/architecture/04-frontends-visual-and-sync.md` (§5.1).
//!
//! Core guarantees:
//! - Structural IR: First-class `PouBody::Network(LadderNetwork)` with **zero** synthetic
//!   `__rail_N` temporaries persisted in ASL (ADR-026).
//! - `LOSSLESS_ASL`: `parse(emit(M, ctx), ctx) == M`, preserving exact `NodeId`s, `rung.order`,
//!   `(row, col)` grid coordinates, nested `ParallelOr` branches, all contact/coil/box kinds,
//!   locked `Foreign` boxes, and verbatim `[[boards.comments]]` (`C1–C5`).
//! - Conservative structural lifting (`lift_statements_to_network`): lifts only declared
//!   canonical Ladder patterns; non-liftable POUs return `None` cleanly and stay intact as
//!   callable `Structured POU` boxes.
//! - Deterministic derived projection (`project_network_to_statements`): projects `LadderNetwork`
//!   into materialized power-flow ASL statements matching `dendri-vm` load-time semantics
//!   (mid-rung mutation hazard safe, `Empty` = open circuit `false`, `Normal` coil unconditional).

mod common;
mod emitter;
mod lexer;
mod lift;
mod matrix;
mod parser;
mod project;
mod validate;

pub use lift::{lift_statements_to_network, lift_statements_to_network_with_fb_kinds};
pub use matrix::feature_matrix_entries;
pub use project::{
    ProjectedNetwork, ProjectedNetworkExport, project_network_for_export,
    project_network_to_statements, project_network_with_decls,
};
pub use validate::validate_ladder_bundle;

use dendri_ast::{CommentEncoding, CommentStyle};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, CommentPlacement, FrontendOutput, FrontendResult,
    LanguageCapabilities, LanguageFrontend, LanguageId, Paradigm, RenderedComment,
    StCommentDialect, matches_comment_with_caps, matches_comment_with_ctx,
    render_comment_for_language,
};
use dendri_span::Severity;

/// Pure-Rust IEC 61131-3 Ladder Diagram frontend (`LanguageId::Ladder`, `Paradigm::Ladder`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderFrontend {
    comment_encoding: CommentEncoding,
}

impl Default for LadderFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl LadderFrontend {
    /// Creates a new Ladder frontend with default `CommentEncoding::Utf8Passthrough`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            comment_encoding: CommentEncoding::Utf8Passthrough,
        }
    }

    /// Creates a Ladder frontend configured with a specific target [`CommentEncoding`].
    #[must_use]
    pub const fn with_comment_encoding(comment_encoding: CommentEncoding) -> Self {
        Self { comment_encoding }
    }

    /// Parses canonical Ladder source and runs complete closed-world static semantic validation
    /// (`validate_ladder_bundle`), returning errors for any `Severity::Error` diagnostic.
    pub fn parse_and_validate(
        &self,
        src: &str,
        ctx: &BoardContext,
    ) -> FrontendResult<BoardProgramBundle> {
        let mut parsed = self.parse(src, ctx)?;
        let diags = validate_ladder_bundle(&parsed.value, ctx);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .cloned()
            .collect();
        if !errors.is_empty() {
            return Err(errors);
        }
        parsed.extend_diagnostics(diags);
        Ok(parsed)
    }
}

impl LanguageFrontend for LadderFrontend {
    fn id(&self) -> LanguageId {
        LanguageId::Ladder
    }

    fn paradigm(&self) -> Paradigm {
        Paradigm::Ladder
    }

    fn parse(&self, src: &str, ctx: &BoardContext) -> FrontendResult<BoardProgramBundle> {
        let bundle = parser::parse_ladder_source(src, ctx)?;
        let diags = validate::validate_on_parse(&bundle, ctx);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .cloned()
            .collect();
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(FrontendOutput::with_diagnostics(bundle, diags))
    }

    fn emit(&self, bundle: &BoardProgramBundle, ctx: &BoardContext) -> FrontendResult<String> {
        emitter::emit_ladder_bundle(bundle, ctx)
    }

    fn render_comment(
        &self,
        text: &str,
        style: CommentStyle,
        placement: CommentPlacement,
        ctx: &BoardContext,
    ) -> RenderedComment {
        render_comment_for_language(LanguageId::Ladder, text, style, placement, ctx)
    }

    fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool {
        let caps = self.capabilities();
        if matches_comment_with_caps(LanguageId::Ladder, &caps, rendered, stored_text) {
            return true;
        }
        // Also recognize Ladder textual `//` and nested `(* ... *)` projections (`docs/lbc/ladder.md` §4).
        let trimmed = rendered.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("(*") {
            let mut text_ctx = BoardContext::new("ladder_matcher", "ladder_matcher", 32);
            text_ctx.comment_encoding = self.comment_encoding;
            text_ctx.st_dialect = StCommentDialect::Iec61131Ed3;
            return matches_comment_with_ctx(LanguageId::St, &text_ctx, rendered, stored_text);
        }
        false
    }

    fn capabilities(&self) -> LanguageCapabilities {
        // Normative capability profile from `docs/lbc/ladder.md` §8
        LanguageCapabilities {
            supported_comment_styles: vec![
                CommentStyle::Visual,
                CommentStyle::Line,
                CommentStyle::ParenBlock,
            ],
            supports_nested_paren_comments: true,
            supports_line_comments: true,
            comment_encoding: self.comment_encoding,
            capabilities: vec!["asl-core@1".to_owned()],
            st_dialect: None,
        }
    }
}
