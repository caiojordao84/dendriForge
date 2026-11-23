//! Parse ASL program IR and single-file projects (both use `.toml`).
//!
//! Document kind is detected by content shape: a `[board]` section plus an
//! `[asl]` / `[asl.*]` program marks a project; otherwise the file is a bare
//! ASL program. Structural deserialize uses [`dendri_ast`] types as the source
//! of truth. Negative fixtures may carry `[fixture_inject]` tables consumed by
//! validate.

use std::fs;
use std::path::Path;

use dendri_ast::{ErrorCode, Program, SimulationProject};
use serde::Deserialize;
use thiserror::Error;

mod canonicalize;
mod expr_text;

pub use canonicalize::{canonicalize_expr, canonicalize_program};
pub use expr_text::parse_expr_text;

/// Optional harness metadata on error-corpus fixtures (not part of ASL IR).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FixtureInject {
    #[serde(default)]
    pub unknown_type_token: Option<String>,
    #[serde(default)]
    pub unknown_type_var: Option<String>,
    #[serde(default)]
    pub declared_setup_count: Option<usize>,
    #[serde(default)]
    pub declared_loop_count: Option<usize>,
    #[serde(default)]
    pub declared_hardware_count: Option<usize>,
    #[serde(default)]
    pub declared_data_count: Option<usize>,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("{code}: {detail}")]
    Ir { code: ErrorCode, detail: String },
}

impl ParseError {
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Ir { code, .. } => Some(*code),
            Self::Io(_) => None,
        }
    }
}

/// Parsed document ready for validate / emit / sim.
#[derive(Debug, Clone)]
pub struct ParsedProgram {
    pub program: Program,
    pub inject: FixtureInject,
    /// Original TOML text (for diagnostics).
    pub source: String,
}

/// Parsed single-file simulation project.
#[derive(Debug, Clone)]
pub struct ParsedProject {
    pub project: SimulationProject,
    pub source: String,
}

pub fn parse_program_str(text: &str) -> Result<ParsedProgram, ParseError> {
    let value: toml::Value = toml::from_str(text).map_err(|e| ParseError::Ir {
        code: ErrorCode::ErrIrDeserialize,
        detail: e.to_string(),
    })?;

    let inject = value
        .get("fixture_inject")
        .cloned()
        .map(FixtureInject::deserialize)
        .transpose()
        .map_err(|e| ParseError::Ir {
            code: ErrorCode::ErrIrDeserialize,
            detail: format!("fixture_inject: {e}"),
        })?
        .unwrap_or_default();

    let mut clean = value;
    if let toml::Value::Table(ref mut t) = clean {
        t.remove("fixture_inject");
    }

    let program: Program = clean.try_into().map_err(|e: toml::de::Error| ParseError::Ir {
        code: ErrorCode::ErrIrDeserialize,
        detail: e.to_string(),
    })?;

    Ok(ParsedProgram {
        program,
        inject,
        source: text.to_string(),
    })
}

pub fn parse_program_file(path: &Path) -> Result<ParsedProgram, ParseError> {
    let text = fs::read_to_string(path)?;
    parse_program_str(&text)
}

pub fn parse_project_str(text: &str) -> Result<ParsedProject, ParseError> {
    let project: SimulationProject = toml::from_str(text).map_err(|e| ParseError::Ir {
        code: ErrorCode::ErrIrDeserialize,
        detail: e.to_string(),
    })?;
    Ok(ParsedProject {
        project,
        source: text.to_string(),
    })
}

pub fn parse_project_file(path: &Path) -> Result<ParsedProject, ParseError> {
    let text = fs::read_to_string(path)?;
    parse_project_str(&text)
}

/// Auto-detect project vs program by TOML content shape (extension is always `.toml`).
pub fn parse_path(path: &Path) -> Result<ParsedDocument, ParseError> {
    let text = fs::read_to_string(path)?;
    if looks_like_project(&text) {
        return Ok(ParsedDocument::Project(parse_project_str(&text)?));
    }
    Ok(ParsedDocument::Program(parse_program_str(&text)?))
}

/// Project files carry `[board]` plus an embedded `[asl]` program envelope.
pub fn looks_like_project(text: &str) -> bool {
    let has_board = text.contains("[board]") || text.contains("[board.");
    let has_asl = text.contains("[asl.")
        || text.contains("[asl]\n")
        || text.contains("\n[asl]")
        || text.starts_with("[asl]");
    has_board && has_asl
}

#[derive(Debug, Clone)]
pub enum ParsedDocument {
    Program(ParsedProgram),
    Project(ParsedProject),
}

impl ParsedDocument {
    pub fn program(&self) -> &Program {
        match self {
            Self::Program(p) => &p.program,
            Self::Project(p) => &p.project.asl,
        }
    }

    pub fn inject(&self) -> FixtureInject {
        match self {
            Self::Program(p) => p.inject.clone(),
            Self::Project(_) => FixtureInject::default(),
        }
    }
}

/// Apply error-corpus `fixture_inject` mutations onto a TOML value, then re-parse.
/// Used so E02 can swap in an unknown type token without breaking Phase 0 load tests.
pub fn apply_inject_and_reparse(
    text: &str,
    inject: &FixtureInject,
) -> Result<Result<Program, ErrorCode>, ParseError> {
    let mut value: toml::Value = toml::from_str(text).map_err(|e| ParseError::Ir {
        code: ErrorCode::ErrIrDeserialize,
        detail: e.to_string(),
    })?;

    if let (Some(token), Some(var)) = (&inject.unknown_type_token, &inject.unknown_type_var) {
        if let Some(data) = value.get_mut("data").and_then(|d| d.as_array_mut()) {
            for entry in data {
                let name = entry.get("name").and_then(|v| v.as_str());
                if name == Some(var.as_str()) {
                    if let Some(table) = entry.as_table_mut() {
                        table.insert("type".into(), toml::Value::String(token.clone()));
                    }
                }
            }
        }
    }

    if let toml::Value::Table(ref mut t) = value {
        t.remove("fixture_inject");
    }

    if let Some(data) = value.get("data").and_then(|d| d.as_array()) {
        for entry in data {
            if let Some(ty) = entry.get("type").and_then(|v| v.as_str()) {
                if dendri_ast::DataType::parse(ty).is_none() {
                    return Ok(Err(ErrorCode::ErrDataUnknownType));
                }
            }
        }
    }

    match value.try_into::<Program>() {
        Ok(p) => Ok(Ok(p)),
        Err(_) => Ok(Err(ErrorCode::ErrIrDeserialize)),
    }
}
