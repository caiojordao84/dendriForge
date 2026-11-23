//! 100% pure-Rust shared polyglot frontend infrastructure for `dendriForge` v2
//! (`04-frontends-visual-and-sync.md` §1.2, §2, §3.3, §4.1–4.3;
//! `01-asl-ir-and-project-schema.md` §3, §6–§7).
//!
//! Provides zero-FFI building blocks shared across all Wave 1–3 language crates:
//! - [`cursor`]: Zero-copy UTF-8-safe `Cursor<'a>`, `LineIndex`, and `Spanned<T>`.
//! - [`pratt`]: Table-driven, recursion-bounded Pratt expression parser engine.
//! - [`traits`]: Normative [`LanguageFrontend`] trait, [`BoardContext`], [`StCommentDialect`] (`DEF-006`),
//!   and [`LanguageCapabilities`].
//! - [`comments`]: Complete ADR-027 verbatim human comment engine (`C1–C5`).
//! - [`numeric_emit`]: Cross-platform reduced-rational ADC/code emitter (never emits `M = 10^9`).
//! - [`foreign`]: Three-level `Foreign` helpers, strict four-part diagnostics, cross-language export guard,
//!   and hash-locked `pack.toml` loader (`DEF-008`).
//! - [`conformance`]: Reusable `C1–C5` / `C3` hostile corpus, `LOSSLESS_ASL`, and feature-matrix suite.

pub mod comments;
pub mod conformance;
pub mod cursor;
pub mod foreign;
pub mod numeric_emit;
pub mod pratt;
pub mod traits;

pub use comments::{
    RawComment, ascii_fold, contains_bidi_control, cpp_phase2_spliced, escape_bidi_controls,
    escape_c_block_delimiters, escape_st_paren_delimiters, has_balanced_paren_delimiters,
    has_cpp_block_comment_hazard, has_cpp_line_splice_hazard, is_bidi_control, lex_c_block_comment,
    lex_cpp_comment, lex_nested_paren_comment, lex_slash_line_comment, lex_st_comment,
    matches_comment_with_caps, matches_comment_with_ctx, merge_contiguous_leading_comments,
    normalize_eol_to_lf, orphan_comments_for_deleted_nodes, orphan_comments_to_parent,
    relocate_inner_comments, render_comment_for_language, restore_eol, unwrap_comment_delimiters,
};
pub use conformance::{
    FeatureMatrixCollector, FeatureMatrixEntry, FeatureSupport, HostileCommentCase,
    assert_c1_multiset_invariant, assert_c2_orphan_preservation, assert_c4_reimport_idempotence,
    assert_c5_comment_fields, assert_lossless_asl, assert_lossless_asl_exact_ids,
    contains_banned_magic_comment, hostile_comment_corpus, normalize_program_for_lossless_asl,
    strip_program_spans, verify_frontend_roundtrip, verify_hostile_corpus_on_frontend,
};
pub use cursor::{
    Cursor, CursorCheckpoint, LineIndex, Spanned, detect_eol_style, is_ident_continue,
    is_ident_start,
};
pub use foreign::{
    ForeignDiagnosticSpec, ForeignExportDecision, ForeignLevel, ForeignSite, PACK_SCHEMA_VERSION,
    PackChecksums, PackError, PackFidelity, PackManifest, PackMetadata, PackParam, PackSymbol,
    PackSymbolKind, assumption_active_diagnostic, build_four_part_diagnostic, check_foreign_export,
    check_program_foreign_export, collect_foreign_sites, compute_sha256_hex,
    find_active_assumption, foreign_export_blocked_diagnostic, foreign_untyped_diagnostic,
    is_four_part_diagnostic, parse_pack_type_ref, resolve_foreign_fidelity,
};
pub use numeric_emit::{EmitIntWidth, ReducedRationalPlan, contains_analog_full_scale_literal};
pub use pratt::{
    Associativity, DEFAULT_PRATT_MAX_DEPTH, InfixSpec, PrattDriver, PrefixSpec, parse_pratt,
    parse_pratt_with_limit,
};
pub use traits::{
    AslModule, BoardContext, BoardProgramBundle, CommentPlacement, FrontendOutput, FrontendResult,
    LanguageCapabilities, LanguageFrontend, LanguageId, Paradigm, ParseLanguageIdError,
    RenderedComment, StCommentDialect, StDialectCapabilityRow,
};
