//! Unified P1 language frontends ↔ shared ASL AST.
//!
//! Text: C++, MicroPython, Structured Text (DEN-7).
//! Visual: Ladder, SFC/GRAFCET diagram TOML (DEN-8).
//! Rust-as-scripting remains P2.

use dendri_ast::{Program, SourceLanguage};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrontendError {
    #[error("{0}")]
    Message(String),
    #[error("language {0} is not supported (Rust user-language = P2)")]
    Unsupported(&'static str),
}

impl From<dendri_frontend_cpp::CppParseError> for FrontendError {
    fn from(e: dendri_frontend_cpp::CppParseError) -> Self {
        Self::Message(e.to_string())
    }
}

impl From<dendri_frontend_micropython::MpyParseError> for FrontendError {
    fn from(e: dendri_frontend_micropython::MpyParseError) -> Self {
        Self::Message(e.to_string())
    }
}

impl From<dendri_backend_st::StParseError> for FrontendError {
    fn from(e: dendri_backend_st::StParseError) -> Self {
        Self::Message(e.to_string())
    }
}

impl From<dendri_frontend_ladder::LadderError> for FrontendError {
    fn from(e: dendri_frontend_ladder::LadderError) -> Self {
        Self::Message(e.to_string())
    }
}

impl From<dendri_frontend_sfc::SfcError> for FrontendError {
    fn from(e: dendri_frontend_sfc::SfcError) -> Self {
        Self::Message(e.to_string())
    }
}

/// Parse source text of a P1 language into the shared ASL [`Program`] AST.
pub fn parse_source(language: SourceLanguage, text: &str) -> Result<Program, FrontendError> {
    match language {
        SourceLanguage::Cpp => Ok(dendri_frontend_cpp::parse_cpp(text)?),
        SourceLanguage::Micropython => Ok(dendri_frontend_micropython::parse_micropython(text)?),
        SourceLanguage::St => Ok(dendri_backend_st::parse_st(text)?),
        SourceLanguage::Ladder => Ok(dendri_frontend_ladder::parse_ladder(text)?),
        SourceLanguage::Sfc => Ok(dendri_frontend_sfc::parse_sfc(text)?),
        SourceLanguage::Asl => Err(FrontendError::Message(
            "ASL is the IR — use dendri-parse on .asl.toml".into(),
        )),
    }
}

/// Generate source text for a P1 language from the shared ASL [`Program`] AST.
pub fn emit_source(language: SourceLanguage, program: &Program) -> Result<String, FrontendError> {
    match language {
        SourceLanguage::Cpp => Ok(dendri_frontend_cpp::emit_cpp(program)),
        SourceLanguage::Micropython => Ok(dendri_frontend_micropython::emit_micropython(program)),
        SourceLanguage::St => Ok(dendri_backend_st::emit_st(program)),
        SourceLanguage::Ladder => Ok(dendri_frontend_ladder::emit_ladder(program)),
        SourceLanguage::Sfc => Ok(dendri_frontend_sfc::emit_sfc(program)),
        SourceLanguage::Asl => Err(FrontendError::Message(
            "ASL emit is dendri-emit (TOML IR)".into(),
        )),
    }
}

/// Emit → parse → compare. Text langs: setup/loop/data/hardware.
/// Visual langs: ladder/grafcet blocks (+ lowered body for ladder/sfc).
pub fn roundtrip_language(
    language: SourceLanguage,
    program: &Program,
) -> Result<Program, FrontendError> {
    let text = emit_source(language, program)?;
    let again = parse_source(language, &text)?;
    match language {
        SourceLanguage::Ladder => {
            if program.ladder.is_empty() {
                // Synthesized Action view from ASL loop — assert stable block RT.
                if again.ladder.is_empty() {
                    return Err(FrontendError::Message(
                        "ladder synthesize produced empty block".into(),
                    ));
                }
                let text2 = emit_source(language, &again)?;
                let again2 = parse_source(language, &text2)?;
                if again2.ladder != again.ladder {
                    return Err(FrontendError::Message(format!(
                        "ladder synthesized block unstable\n--- emitted ---\n{text2}"
                    )));
                }
            } else if again.ladder != program.ladder {
                return Err(FrontendError::Message(format!(
                    "ladder round-trip block mismatch\n--- emitted ---\n{text}"
                )));
            }
        }
        SourceLanguage::Sfc => {
            if program.grafcet.is_empty() {
                // Synthesized step view — lowering adds tokens; assert stable chart RT.
                if again.grafcet.is_empty() {
                    return Err(FrontendError::Message(
                        "sfc synthesize produced empty chart".into(),
                    ));
                }
                let text2 = emit_source(language, &again)?;
                let again2 = parse_source(language, &text2)?;
                if again2.grafcet != again.grafcet {
                    return Err(FrontendError::Message(format!(
                        "sfc synthesized chart unstable\n--- emitted ---\n{text2}"
                    )));
                }
            } else if again.grafcet != program.grafcet {
                return Err(FrontendError::Message(format!(
                    "sfc round-trip block mismatch\n--- emitted ---\n{text}"
                )));
            }
        }
        _ => {
            if again.setup != program.setup
                || again.loop_body != program.loop_body
                || again.data != program.data
                || again.hardware != program.hardware
            {
                return Err(FrontendError::Message(format!(
                    "{} round-trip AST mismatch\n--- emitted ---\n{text}",
                    language.as_str()
                )));
            }
        }
    }
    Ok(again)
}

pub use dendri_backend_st::{emit_st, parse_st};
pub use dendri_frontend_cpp::{emit_cpp, parse_cpp};
pub use dendri_frontend_ladder::{emit_ladder, parse_ladder};
pub use dendri_frontend_micropython::{emit_micropython, parse_micropython};
pub use dendri_frontend_sfc::{emit_sfc, parse_sfc};
