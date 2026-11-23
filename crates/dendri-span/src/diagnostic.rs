//! Structured compiler, validator, and runtime diagnostics.

use crate::Span;
use crate::error_code::ErrorCode;
use crate::source_map::{FileId, SourceMap};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Severity level of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Fatal validation, parse, or execution error.
    Error,
    /// Non-fatal engineering warning.
    Warning,
    /// Informational notice (e.g., comment relocated/orphaned).
    Info,
}

impl Severity {
    /// Canonical lowercase label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured diagnostic matching `01-asl-ir-and-project-schema.md` §8.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Frozen diagnostic code.
    pub code: ErrorCode,
    /// Severity level (`Error`, `Warning`, or `Info`).
    pub severity: Severity,
    /// Optional owning board instance ID in multi-board projects.
    pub board_id: Option<String>,
    /// Optional registered file ID in a [`SourceMap`].
    pub file_id: Option<FileId>,
    /// Optional byte span inside the source file.
    pub span: Option<Span>,
    /// Clear, actionable primary message.
    pub message: String,
    /// Optional remediation hint ("what now").
    pub hint: Option<String>,
}

impl Diagnostic {
    /// Constructs a diagnostic using the code's default severity.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: code.default_severity(),
            board_id: None,
            file_id: None,
            span: None,
            message: message.into(),
            hint: None,
        }
    }

    /// Overrides the severity (e.g., warning-level `ERR_UNSAFE_VOLTAGE_DOMAIN`).
    #[must_use]
    pub const fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Attaches a board instance ID.
    #[must_use]
    pub fn with_board_id(mut self, board_id: impl Into<String>) -> Self {
        self.board_id = Some(board_id.into());
        self
    }

    /// Attaches a source location (`FileId` + [`Span`]).
    #[must_use]
    pub const fn with_location(mut self, file_id: FileId, span: Span) -> Self {
        self.file_id = Some(file_id);
        self.span = Some(span);
        self
    }

    /// Attaches an actionable remediation hint.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Formats the diagnostic cleanly, enriching with file/line/snippet when present in `sources`.
    #[must_use]
    pub fn render(&self, sources: Option<&SourceMap>) -> String {
        let mut out = format!("{}[{}]", self.severity, self.code);
        if let Some(board) = &self.board_id {
            out.push_str(&format!(" (board `{board}`)"));
        }
        out.push_str(&format!(": {}", self.message));

        if let (Some(sm), Some(fid), Some(span)) = (sources, self.file_id, self.span)
            && let Some(file) = sm.get(fid)
        {
            let lc = file.line_col(span.start);
            out.push_str(&format!("\n  --> {}:{lc}", file.name()));
            if let Some(snippet) = file.render_snippet(span) {
                out.push('\n');
                out.push_str(&snippet);
            }
        }

        if let Some(hint) = &self.hint {
            out.push_str(&format!("\n  hint: {hint}"));
        }
        out
    }
}

/// Ordered accumulator of diagnostics produced during a validation or compilation pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticBag {
    items: Vec<Diagnostic>,
}

impl DiagnosticBag {
    /// Creates an empty bag.
    #[must_use]
    pub const fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Appends a diagnostic.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.items.push(diagnostic);
    }

    /// Returns all collected diagnostics in deterministic insertion order.
    #[must_use]
    pub fn all(&self) -> &[Diagnostic] {
        &self.items
    }

    /// Returns `true` if any collected diagnostic has [`Severity::Error`].
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }

    /// Counts diagnostics matching `severity`.
    #[must_use]
    pub fn count_by_severity(&self, severity: Severity) -> usize {
        self.items.iter().filter(|d| d.severity == severity).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_rich_diagnostic_with_board_location_snippet_and_hint() {
        let mut sm = SourceMap::new();
        let fid = sm.add_file("project.toml", "from = \"badendpoint\"");
        let diag = Diagnostic::new(ErrorCode::ErrBadEndpoint, "endpoint must contain '.'")
            .with_board_id("plc_main")
            .with_location(fid, Span::new(8, 19))
            .with_hint("Use `<instance_id>.<port_id>`, e.g. `plc_main.Qa.0`.");

        let rendered = diag.render(Some(&sm));
        assert!(rendered.contains("error[ERR_BAD_ENDPOINT] (board `plc_main`)"));
        assert!(rendered.contains("--> project.toml:1:9"));
        assert!(rendered.contains("^^^^^^^^^^^"));
        assert!(rendered.contains("hint: Use `<instance_id>.<port_id>`"));
    }

    #[test]
    fn diagnostic_bag_tracks_errors_warnings_and_info() {
        let mut bag = DiagnosticBag::new();
        assert!(!bag.has_errors());
        bag.push(Diagnostic::new(
            ErrorCode::WarnDutyClamped,
            "duty exceeded M",
        ));
        bag.push(Diagnostic::new(
            ErrorCode::InfoCommentRelocated,
            "inner comment moved",
        ));
        assert!(!bag.has_errors());
        assert_eq!(bag.count_by_severity(Severity::Warning), 1);
        assert_eq!(bag.count_by_severity(Severity::Info), 1);

        bag.push(Diagnostic::new(
            ErrorCode::ErrBandLimitOverflow,
            "band exceeds i32",
        ));
        assert!(bag.has_errors());
        assert_eq!(bag.count_by_severity(Severity::Error), 1);
        assert_eq!(bag.all().len(), 3);
    }
}
