//! Unified P1 text language frontends ↔ shared ASL AST.
//!
//! Dispatches parse/emit for C++, MicroPython, and Structured Text.
//! Ladder / SFC remain DEN-8. Rust-as-scripting remains P2.

use dendri_ast::{Program, SourceLanguage};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrontendError {
    #[error("{0}")]
    Message(String),
    #[error("language {0} is not a P1 text frontend (Ladder/SFC = DEN-8; Rust = P2)")]
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

/// Parse source text of a P1 language into the shared ASL [`Program`] AST.
pub fn parse_source(language: SourceLanguage, text: &str) -> Result<Program, FrontendError> {
    match language {
        SourceLanguage::Cpp => Ok(dendri_frontend_cpp::parse_cpp(text)?),
        SourceLanguage::Micropython => Ok(dendri_frontend_micropython::parse_micropython(text)?),
        SourceLanguage::St => Ok(dendri_backend_st::parse_st(text)?),
        SourceLanguage::Asl => Err(FrontendError::Message(
            "ASL is the IR — use dendri-parse on .asl.toml".into(),
        )),
        SourceLanguage::Ladder => Err(FrontendError::Unsupported("ladder")),
        SourceLanguage::Sfc => Err(FrontendError::Unsupported("sfc")),
    }
}

/// Generate source text for a P1 language from the shared ASL [`Program`] AST.
pub fn emit_source(language: SourceLanguage, program: &Program) -> Result<String, FrontendError> {
    match language {
        SourceLanguage::Cpp => Ok(dendri_frontend_cpp::emit_cpp(program)),
        SourceLanguage::Micropython => Ok(dendri_frontend_micropython::emit_micropython(program)),
        SourceLanguage::St => Ok(dendri_backend_st::emit_st(program)),
        SourceLanguage::Asl => Err(FrontendError::Message(
            "ASL emit is dendri-emit (TOML IR)".into(),
        )),
        SourceLanguage::Ladder => Err(FrontendError::Unsupported("ladder")),
        SourceLanguage::Sfc => Err(FrontendError::Unsupported("sfc")),
    }
}

/// Emit → parse → compare setup/loop/data/hardware (metadata notes may differ).
pub fn roundtrip_language(
    language: SourceLanguage,
    program: &Program,
) -> Result<Program, FrontendError> {
    let text = emit_source(language, program)?;
    let again = parse_source(language, &text)?;
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
    Ok(again)
}

pub use dendri_backend_st::{emit_st, parse_st};
pub use dendri_frontend_cpp::{emit_cpp, parse_cpp};
pub use dendri_frontend_micropython::{emit_micropython, parse_micropython};
