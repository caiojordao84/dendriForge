//! 100% pure-Rust IEC 60848 / IEC 61131-3 GRAFCET / Sequential Function Chart (`sfc`)
//! frontend for `dendriForge` v2 (`docs/lbc/sfc.md`, `04-frontends-visual-and-sync.md` §5.2).
//!
//! Design guarantees:
//! - First-class structural `PouBody::Chart(SfcChart)` parsing & canonical emission
//!   (**ZERO** synthetic `__sfc_*` temporaries stored in ASL, ADR-026).
//! - Satisfies `LOSSLESS_ASL`: `parse(emit(M, ctx), ctx) == M`.
//! - Supports all 8 IEC 61131-3 Table 47 action qualifiers (`N`, `S`, `R`, `L`, `D`, `P`, `P1`, `P0`),
//!   selective OR priority divergence, simultaneous AND divergence/convergence, macro-steps,
//!   `EVOLVE_TO_STABILITY`, and native `<Step>.X`, `<Step>.T`, `<Macro>.DONE` expressions.
//! - Provides [`project_chart_to_statements`] (and [`project_chart_state_decls`]) for derived
//!   IEC 60848 Rule 5 two-phase statement projection ready for Phase 6b ST / RuSTy export.
//! - Complete ADR-027 verbatim human comment conservation (`C1–C5`) and three-level `Foreign`
//!   preservation with zero banned machine magic comments (ADR-020).

mod comments;
mod conformance;
mod emitter;
mod lexer;
mod parser;
mod projection;
mod validate;

pub use conformance::feature_matrix_entries;
pub use projection::{
    DEFAULT_SCAN_DT_US, MAX_STABILITY_ITERS, project_chart_state_decls, project_chart_to_statements,
};

use dendri_ast::{CommentEncoding, CommentStyle};
use dendri_frontend_core::traits::{
    BoardContext, BoardProgramBundle, CommentPlacement, FrontendResult, LanguageCapabilities,
    LanguageFrontend, LanguageId, Paradigm, RenderedComment,
};

/// Pure-Rust IEC 60848 / IEC 61131-3 GRAFCET / SFC language frontend (`Paradigm::Grafcet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SfcFrontend {
    comment_encoding: CommentEncoding,
}

impl SfcFrontend {
    /// Creates an [`SfcFrontend`] using the default `Utf8Passthrough` comment encoding.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            comment_encoding: CommentEncoding::Utf8Passthrough,
        }
    }

    /// Creates an [`SfcFrontend`] configured with an explicit target [`CommentEncoding`].
    #[must_use]
    pub const fn with_comment_encoding(comment_encoding: CommentEncoding) -> Self {
        Self { comment_encoding }
    }
}

impl LanguageFrontend for SfcFrontend {
    fn id(&self) -> LanguageId {
        LanguageId::Sfc
    }

    fn paradigm(&self) -> Paradigm {
        Paradigm::Grafcet
    }

    fn parse(&self, src: &str, ctx: &BoardContext) -> FrontendResult<BoardProgramBundle> {
        parser::parse_sfc(src, ctx)
    }

    fn emit(&self, bundle: &BoardProgramBundle, ctx: &BoardContext) -> FrontendResult<String> {
        emitter::emit_sfc(bundle, ctx)
    }

    fn render_comment(
        &self,
        text: &str,
        style: CommentStyle,
        placement: CommentPlacement,
        ctx: &BoardContext,
    ) -> RenderedComment {
        comments::render_sfc_comment(text, style, placement, ctx)
    }

    fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool {
        comments::matches_sfc_comment(rendered, stored_text, self.comment_encoding)
    }

    fn capabilities(&self) -> LanguageCapabilities {
        comments::sfc_capabilities(self.comment_encoding)
    }
}
