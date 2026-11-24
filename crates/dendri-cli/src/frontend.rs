//! Rosetta Stone Wave 1 CLI helpers (`dendri translate` and `dendri roundtrip`).

use crate::packs::load_verified_packs;
use clap::ValueEnum;
use dendri_frontend_core::{
    BoardContext, LanguageFrontend, LanguageId, StCommentDialect, assert_c1_multiset_invariant,
    assert_c5_comment_fields, contains_analog_full_scale_literal, contains_banned_magic_comment,
    verify_frontend_roundtrip,
};
use dendri_lang_cpp::CppFrontend;
use dendri_lang_ladder::LadderFrontend;
use dendri_lang_sfc::SfcFrontend;
use dendri_lang_st::StFrontend;
use dendri_span::Diagnostic;
use std::path::Path;
use thiserror::Error;

/// Wave 1 language selector for CLI translation and round-trip checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliLanguage {
    Cpp,
    St,
    Ladder,
    Sfc,
}

impl CliLanguage {
    /// Converts to canonical [`LanguageId`].
    #[must_use]
    pub const fn to_language_id(self) -> LanguageId {
        match self {
            Self::Cpp => LanguageId::Cpp,
            Self::St => LanguageId::St,
            Self::Ladder => LanguageId::Ladder,
            Self::Sfc => LanguageId::Sfc,
        }
    }

    /// Default target board profile matching this language's primary domain.
    #[must_use]
    pub const fn default_board_profile(self) -> CliBoardProfile {
        match self {
            Self::Cpp => CliBoardProfile::ArduinoUnoR3,
            Self::St | Self::Ladder | Self::Sfc => CliBoardProfile::Plc1200Open1214,
        }
    }
}

/// Certified Wave 1 board profile selector for CLI frontend operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliBoardProfile {
    #[value(name = "arduino-uno-r3")]
    ArduinoUnoR3,
    #[value(name = "plc-1200-open-1214")]
    Plc1200Open1214,
}

/// IEC 61131-3 ST comment dialect selector (`DEF-006`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliStDialect {
    #[value(name = "iec-ed3")]
    IecEd3,
    #[value(name = "rusty")]
    Rusty,
    #[value(name = "strict-ed2")]
    StrictEd2,
    #[value(name = "classic-plc")]
    ClassicPlc,
}

impl CliStDialect {
    /// Converts to canonical [`StCommentDialect`].
    #[must_use]
    pub const fn to_dialect(self) -> StCommentDialect {
        match self {
            Self::IecEd3 => StCommentDialect::Iec61131Ed3,
            Self::Rusty => StCommentDialect::RuSTy,
            Self::StrictEd2 => StCommentDialect::StrictEd2,
            Self::ClassicPlc => StCommentDialect::ClassicPlc,
        }
    }
}

/// Summary report emitted by a passing `dendri roundtrip`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundtripReport {
    pub language: LanguageId,
    pub board_profile: String,
    pub pou_count: usize,
    pub comment_count: usize,
    pub foreign_count: usize,
    pub emitted_bytes: usize,
}

/// Output produced by a passing `dendri translate`.
#[derive(Debug, Clone)]
pub struct TranslateOutput {
    pub emitted: String,
    pub diagnostics: Vec<Diagnostic>,
    pub pou_count: usize,
    pub comment_count: usize,
}

/// Errors produced by `dendri translate` or `dendri roundtrip`.
#[derive(Debug, Error)]
pub enum FrontendCliError {
    #[error("frontend diagnostics ({} item(s))", .0.len())]
    Diagnostics(Vec<Diagnostic>),
    #[error("conformance check failed: {0}")]
    Conformance(String),
}

/// Constructs a [`BoardContext`] for `board_profile`, attaching verified packs from `assets_dir`
/// when available.
#[must_use]
pub fn build_board_context(
    board_profile: CliBoardProfile,
    st_dialect: CliStDialect,
    allow_foreign_export: bool,
    assets_dir: Option<&Path>,
) -> BoardContext {
    let mut ctx = match board_profile {
        CliBoardProfile::ArduinoUnoR3 => BoardContext::arduino_uno_r3("cli_uno"),
        CliBoardProfile::Plc1200Open1214 => BoardContext::plc_1200_open_1214("cli_plc"),
    };
    ctx.st_dialect = st_dialect.to_dialect();
    ctx.allow_foreign_export = allow_foreign_export;
    if let Some(dir) = assets_dir
        && let Ok((manifests, _)) = load_verified_packs(dir)
    {
        for manifest in manifests {
            ctx.loaded_packs.push(manifest);
        }
    }
    ctx
}

fn frontend_for(lang: CliLanguage) -> Box<dyn LanguageFrontend> {
    match lang {
        CliLanguage::Cpp => Box::new(CppFrontend::new()),
        CliLanguage::St => Box::new(StFrontend::new()),
        CliLanguage::Ladder => Box::new(LadderFrontend::new()),
        CliLanguage::Sfc => Box::new(SfcFrontend::new()),
    }
}

/// Translates `source` from `from` language into `to` language via canonical ASL IR.
pub fn run_translate(
    source: &str,
    from: CliLanguage,
    to: CliLanguage,
    ctx: &BoardContext,
) -> Result<TranslateOutput, FrontendCliError> {
    let src_fe = frontend_for(from);
    let dst_fe = frontend_for(to);

    let parsed = src_fe
        .parse(source, ctx)
        .map_err(FrontendCliError::Diagnostics)?;
    let emitted = dst_fe
        .emit(&parsed.value, ctx)
        .map_err(FrontendCliError::Diagnostics)?;

    if contains_banned_magic_comment(&emitted.value) {
        return Err(FrontendCliError::Conformance(
            "emitted output contained a banned machine magic comment (ADR-020)".to_owned(),
        ));
    }
    if contains_analog_full_scale_literal(&emitted.value) {
        return Err(FrontendCliError::Conformance(
            "emitted output leaked forbidden M = 10^9 scale literal (ADR-025)".to_owned(),
        ));
    }

    let mut diagnostics = parsed.diagnostics;
    diagnostics.extend(emitted.diagnostics);

    Ok(TranslateOutput {
        emitted: emitted.value,
        diagnostics,
        pou_count: parsed.value.program.pous.len(),
        comment_count: parsed.value.comments.len(),
    })
}

/// Verifies `LOSSLESS_ASL`, `C1` comment multiset invariance, `C5` comment validity,
/// zero banned magic comments, and zero `M = 10^9` leakage for a single source file.
pub fn run_roundtrip(
    source: &str,
    lang: CliLanguage,
    ctx: &BoardContext,
) -> Result<RoundtripReport, FrontendCliError> {
    let fe = frontend_for(lang);
    let parsed = fe
        .parse(source, ctx)
        .map_err(FrontendCliError::Diagnostics)?;

    assert_c5_comment_fields(&parsed.value.comments).map_err(|codes| {
        FrontendCliError::Conformance(format!("C5 comment validation failed: {codes:?}"))
    })?;

    let emitted = fe
        .emit(&parsed.value, ctx)
        .map_err(FrontendCliError::Diagnostics)?;

    if contains_banned_magic_comment(&emitted.value) {
        return Err(FrontendCliError::Conformance(
            "emitted output contained a banned machine magic comment (ADR-020)".to_owned(),
        ));
    }
    if contains_analog_full_scale_literal(&emitted.value) {
        return Err(FrontendCliError::Conformance(
            "emitted output leaked forbidden M = 10^9 scale literal (ADR-025)".to_owned(),
        ));
    }

    let reparsed = fe
        .parse(&emitted.value, ctx)
        .map_err(FrontendCliError::Diagnostics)?;

    verify_frontend_roundtrip(fe.as_ref(), &parsed.value, ctx)
        .map_err(FrontendCliError::Conformance)?;

    assert_c1_multiset_invariant(&parsed.value.comments, &reparsed.value.comments)
        .map_err(FrontendCliError::Conformance)?;

    let foreign_count = dendri_frontend_core::collect_foreign_sites(&parsed.value.program).len();

    Ok(RoundtripReport {
        language: lang.to_language_id(),
        board_profile: ctx.profile_id.clone(),
        pou_count: parsed.value.program.pous.len(),
        comment_count: parsed.value.comments.len(),
        foreign_count,
        emitted_bytes: emitted.value.len(),
    })
}
