//! 100% pure-Rust IEC 61131-3 Structured Text (`dendri-lang-st`) frontend implementing
//! [`dendri_frontend_core::LanguageFrontend`] (`docs/lbc/st.md`, `04-frontends-visual-and-sync.md`).
//!
//! Features:
//! - Real IEC 61131-3 ST parser & canonical emitter (`PROGRAM` / `ORGANIZATION_BLOCK`,
//!   `FUNCTION`, `FUNCTION_BLOCK`, `TYPE` structs/enums, `NAMESPACE`, `USING`, separate
//!   `VAR_INPUT`/`VAR_OUTPUT`/`VAR_IN_OUT`/`VAR`/`VAR CONSTANT`/`VAR RETAIN`/`VAR PERSISTENT`/
//!   `VAR_TEMP`/`VAR_GLOBAL` blocks, zero invented `SETUP`/`LOOP` keywords).
//! - Bidirectional `iec_address` hardware addressing (`%IX`, `%QX`, `%IW`, `%QW`) via
//!   [`BoardContext`] with zero banned machine magic comments (`ADR-020`).
//! - Complete numeric contract: `ADC_CODE(tag)` ⇄ direct analog input tags and cross-platform
//!   reduced-rational scaling via [`dendri_frontend_core::ReducedRationalPlan::emit_st`]
//!   (never emits `M = 10^9`).
//! - Full `DEF-006` [`StCommentDialect`] support (`Iec61131Ed3`, `RuSTy`, `StrictEd2`,
//!   `ClassicPlc`) and `ADR-027` verbatim human comment conservation (`C1–C5`).
//! - Three-level `Foreign` preservation (`Decl::Foreign`, `Stmt::Foreign`, `Expr::Foreign`),
//!   cross-language export guarding (`ERR_FOREIGN_EXPORT_BLOCKED` / `--allow-foreign`), and
//!   `F1` hash-locked `pack.toml` symbol recognition (`ctx.lookup_pack_symbol`).

mod emitter;
mod lexer;
mod parser;
mod sema;

pub use emitter::emit_scaled_code_expr;

use dendri_ast::{CommentEncoding, CommentStyle};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, CommentPlacement, FeatureMatrixEntry, FeatureSupport,
    FrontendOutput, FrontendResult, LanguageCapabilities, LanguageFrontend, LanguageId, Paradigm,
    RenderedComment, StCommentDialect, matches_comment_with_caps, render_comment_for_language,
};
use std::borrow::Cow;

/// 100% pure-Rust IEC 61131-3 Structured Text frontend (`LanguageId::St`, `Paradigm::Textual`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StFrontend {
    dialect: StCommentDialect,
    comment_encoding: CommentEncoding,
    dialect_override: Option<StCommentDialect>,
    encoding_override: Option<CommentEncoding>,
}

impl Default for StFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl StFrontend {
    /// Creates an ST frontend with the default IEC 61131-3 3rd Edition comment dialect
    /// and UTF-8 passthrough encoding (`docs/lbc/st.md` §8).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            dialect: StCommentDialect::Iec61131Ed3,
            comment_encoding: CommentEncoding::Utf8Passthrough,
            dialect_override: None,
            encoding_override: None,
        }
    }

    /// Creates an ST frontend configured for a specific `DEF-006` [`StCommentDialect`].
    #[must_use]
    pub const fn with_dialect(dialect: StCommentDialect) -> Self {
        Self {
            dialect,
            comment_encoding: CommentEncoding::Utf8Passthrough,
            dialect_override: Some(dialect),
            encoding_override: None,
        }
    }

    /// Creates an ST frontend configured with an explicit `DEF-006` dialect and comment encoding.
    #[must_use]
    pub const fn with_config(dialect: StCommentDialect, comment_encoding: CommentEncoding) -> Self {
        Self {
            dialect,
            comment_encoding,
            dialect_override: Some(dialect),
            encoding_override: Some(comment_encoding),
        }
    }

    /// Returns the configured `DEF-006` [`StCommentDialect`].
    #[must_use]
    pub const fn dialect(&self) -> StCommentDialect {
        self.dialect
    }

    /// Returns the configured [`CommentEncoding`].
    #[must_use]
    pub const fn comment_encoding(&self) -> CommentEncoding {
        self.comment_encoding
    }

    fn effective_ctx<'a>(&self, ctx: &'a BoardContext) -> Cow<'a, BoardContext> {
        let apply_dialect = self
            .dialect_override
            .filter(|&d| ctx.st_dialect == StCommentDialect::Iec61131Ed3 && d != ctx.st_dialect);
        let apply_encoding = self.encoding_override.filter(|&e| {
            ctx.comment_encoding == CommentEncoding::Utf8Passthrough && e != ctx.comment_encoding
        });
        if apply_dialect.is_none() && apply_encoding.is_none() {
            return Cow::Borrowed(ctx);
        }
        let mut owned = ctx.clone();
        if let Some(d) = apply_dialect {
            owned.st_dialect = d;
        }
        if let Some(e) = apply_encoding {
            owned.comment_encoding = e;
        }
        Cow::Owned(owned)
    }

    /// Emits canonical IEC 61131-3 ST while expanding composed `ANALOG_TO_CODE(CODE_TO_ANALOG(..))`
    /// scaling into reduced-rational integer arithmetic via [`dendri_frontend_core::ReducedRationalPlan::emit_st`].
    ///
    /// Never emits `M = 10^9`.
    pub fn emit_with_reduced_rationals(
        &self,
        bundle: &BoardProgramBundle,
        ctx: &BoardContext,
    ) -> FrontendResult<String> {
        let eff = self.effective_ctx(ctx);
        emitter::emit_st(bundle, eff.as_ref(), true)
    }
}

impl LanguageFrontend for StFrontend {
    fn id(&self) -> LanguageId {
        LanguageId::St
    }

    fn paradigm(&self) -> Paradigm {
        Paradigm::Textual
    }

    fn parse(&self, src: &str, ctx: &BoardContext) -> FrontendResult<BoardProgramBundle> {
        let eff = self.effective_ctx(ctx);
        let parsed = parser::parse_st(src, eff.as_ref())?;
        let diagnostics = sema::validate_parsed_st(&parsed, eff.as_ref())?;
        Ok(FrontendOutput::with_diagnostics(parsed.bundle, diagnostics))
    }

    fn emit(&self, bundle: &BoardProgramBundle, ctx: &BoardContext) -> FrontendResult<String> {
        let eff = self.effective_ctx(ctx);
        emitter::emit_st(bundle, eff.as_ref(), false)
    }

    fn render_comment(
        &self,
        text: &str,
        style: CommentStyle,
        placement: CommentPlacement,
        ctx: &BoardContext,
    ) -> RenderedComment {
        let eff = self.effective_ctx(ctx);
        render_comment_for_language(LanguageId::St, text, style, placement, eff.as_ref())
    }

    fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool {
        matches_comment_with_caps(LanguageId::St, &self.capabilities(), rendered, stored_text)
    }

    fn capabilities(&self) -> LanguageCapabilities {
        LanguageCapabilities::st(self.dialect, self.comment_encoding)
    }
}

/// Returns the conformance-test-evidenced feature-support matrix rows for `dendri-lang-st`
/// (`04-frontends-visual-and-sync.md` §2, `docs/lbc/st.md` §8).
#[must_use]
pub fn feature_matrix_entries() -> Vec<FeatureMatrixEntry> {
    let row = |category: &str,
               feature: &str,
               support: FeatureSupport,
               evidence_test: &str,
               notes: &str| FeatureMatrixEntry {
        category: category.to_owned(),
        feature: feature.to_owned(),
        language: LanguageId::St,
        support,
        evidence_test: evidence_test.to_owned(),
        notes: notes.to_owned(),
    };

    vec![
        row(
            "POUs & Organization Blocks",
            "OB_STARTUP / OB_MAIN / OB_HW_INT & typed FC / FB POUs",
            FeatureSupport::Full,
            "st_pous_and_var_blocks_roundtrip",
            "PROGRAM / ORGANIZATION_BLOCK / FUNCTION / FUNCTION_BLOCK; zero invented SETUP/LOOP",
        ),
        row(
            "Declarations",
            "Separate VAR_INPUT / VAR_OUTPUT / VAR_IN_OUT / VAR / CONSTANT / RETAIN / PERSISTENT / VAR_TEMP / VAR_GLOBAL",
            FeatureSupport::Full,
            "st_pous_and_var_blocks_roundtrip",
            "Exact declaration order and StorageScope conservation",
        ),
        row(
            "Hardware & I/O",
            "Board I/O Tag Table binding (zero banned magic comments, ADR-020)",
            FeatureSupport::Full,
            "st_iec_address_hardware_roundtrip_zero_magic_comments",
            "Direct AT %IX / %QX / %IW / %QW via BoardContext iec_address",
        ),
        row(
            "Structured Statements & Types",
            "Control flow (if/for/while/switch/case), expressions, fixed-width types, arrays, structs & enums",
            FeatureSupport::Full,
            "st_literals_types_and_operators_roundtrip",
            "IEC radix (2#/8#/16#), TIME (T#1s500ms), CASE..OF ranges, FOR..BY, REPEAT..UNTIL, STRUCT/ENUM/0-based ARRAY",
        ),
        row(
            "Function Blocks",
            "Standard IEC 61131-3 FBs (TON/TOF/TP, CTU/CTD/CTUD, R_TRIG/F_TRIG, SR/RS, Comm)",
            FeatureSupport::Full,
            "st_control_flow_fb_and_sfc_step_attrs",
            "Named FB invocations with .Q/.ET/.CV member access & function return assignment",
        ),
        row(
            "GRAFCET / SFC",
            "Native Step Attributes (<Step>.X, <Step>.T, <Macro>.DONE)",
            FeatureSupport::Full,
            "st_control_flow_fb_and_sfc_step_attrs",
            "First-class <Step>.X and <Step>.T typed access in ST expressions",
        ),
        row(
            "Ladder (IEC 61131-3 LD)",
            "Derived projection (Network -> materialized power-flow ASL / ST / C++)",
            FeatureSupport::Full,
            "st_regression_6_derived_ladder_sorting_midrung_reset_fb_pins_and_parallel_or",
            "Materialized VAR_TEMP _rail, all 17 FbKind pins, recursive ParallelOr",
        ),
        row(
            "Numeric Contract",
            "Cross-platform reduced-rational ADC scaling (never emits M=10^9)",
            FeatureSupport::Full,
            "st_numeric_contract_adc_code_and_reduced_rational_never_emits_m",
            "ADC_CODE(IW64) <-> IW64 & ReducedRationalPlan::emit_st with span->binned clamp",
        ),
        row(
            "Comments (ADR-027)",
            "C1-C5 verbatim conservation & hostile corpus (C3)",
            FeatureSupport::Full,
            "st_def_006_all_four_dialects_and_hostile_c1_c5_corpus",
            "All 4 DEF-006 dialects (Iec61131Ed3, RuSTy, StrictEd2, ClassicPlc), nested (* *), bidi, C4",
        ),
        row(
            "Foreign & Packs",
            "Three-level Foreign (F0) & cross-language export guard",
            FeatureSupport::ForeignF0,
            "st_three_level_foreign_and_cross_language_export_guard",
            "INTERFACE/METHOD/EXTENDS/POINTER TO/ADR/^/**/pragma preserved verbatim; cross-lang gated",
        ),
        row(
            "Foreign & Packs",
            "F1 hash-locked pack.toml symbols (DEF-008)",
            FeatureSupport::ForeignF1,
            "st_f1_pack_symbol_recognition_modbus_and_vendor",
            "MB_CLIENT / MB_SERVER & vendor F1 signatures via ctx.lookup_pack_symbol",
        ),
        row(
            "Conformance",
            "LOSSLESS_ASL round-trip (proptest)",
            FeatureSupport::Full,
            "st_lossless_asl_proptest",
            "parse(emit(m)) == m modulo byte spans across generated ST programs",
        ),
        row(
            "Conformance",
            "Negative fixtures for every frozen ErrorCode",
            FeatureSupport::Full,
            "st_negative_fixtures_cover_all_error_codes",
            "Genuine negative fixture for every ST ErrorCode with exact byte span",
        ),
        row(
            "Stdlib & Dialects",
            "Domain-split std.iec.* default vs explicit USING std.plc.* / isolated std.arduino.*",
            FeatureSupport::Full,
            "st_negative_fixtures_cover_all_error_codes",
            "Unimported platform symbols fail with ERR_UNRESOLVED_IMPORT",
        ),
        row(
            "Rejected Constructs",
            "WSTRING / DATE / TOD / DT / chained comparisons / non-zero ARRAY lower bounds",
            FeatureSupport::Unsupported,
            "st_negative_fixtures_cover_all_error_codes",
            "Every rejected construct produces an exact frozen ErrorCode and byte Span",
        ),
    ]
}
