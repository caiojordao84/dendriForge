//! Frozen 45-variant diagnostic code contract (`01-asl-ir-and-project-schema.md` §8).

use crate::diagnostic::Severity;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Frozen diagnostic code shared across all `dendriForge` v2 crates.
///
/// Every variant MUST have at least one live producer and genuine negative fixture
/// by the completion of the phase that introduces its subsystem (CI Gate 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    // Structural / schema / version
    #[serde(rename = "ERR_TOML_SYNTAX")]
    ErrTomlSyntax,
    #[serde(rename = "ERR_UNKNOWN_FIELD")]
    ErrUnknownField,
    #[serde(rename = "ERR_UNSUPPORTED_VERSION")]
    ErrUnsupportedVersion,

    // Declarations, IDs & pin uniqueness
    #[serde(rename = "ERR_DUPLICATE_ID")]
    ErrDuplicateId,
    #[serde(rename = "ERR_DUPLICATE_DECL")]
    ErrDuplicateDecl,
    #[serde(rename = "ERR_HARDWARE_PIN_CONFLICT")]
    ErrHardwarePinConflict,
    #[serde(rename = "ERR_NAME_COLLISION")]
    ErrNameCollision,

    // Static type checking & name resolution
    #[serde(rename = "ERR_UNKNOWN_TYPE")]
    ErrUnknownType,
    #[serde(rename = "ERR_TYPE_MISMATCH")]
    ErrTypeMismatch,
    #[serde(rename = "ERR_INVALID_CAST")]
    ErrInvalidCast,
    #[serde(rename = "ERR_ARRAY_BOUNDS_CONST")]
    ErrArrayBoundsConst,
    #[serde(rename = "ERR_UNKNOWN_FIELD_ACCESS")]
    ErrUnknownFieldAccess,
    #[serde(rename = "ERR_UNDECLARED_SYMBOL")]
    ErrUndeclaredSymbol,

    // ASL Numeric contract (M = 10^9)
    #[serde(rename = "ERR_CODEC_RESOLUTION_EXCEEDS_SCALE")]
    ErrCodecResolutionExceedsScale,
    #[serde(rename = "ERR_BAND_LIMIT_OVERFLOW")]
    ErrBandLimitOverflow,

    // Immutability
    #[serde(rename = "ERR_CONST_WRITE")]
    ErrConstWrite,
    #[serde(rename = "ERR_HARDWARE_LABEL_WRITE")]
    ErrHardwareLabelWrite,

    // Calls & function signatures
    #[serde(rename = "ERR_UNRESOLVED_FUNCTION")]
    ErrUnresolvedFunction,
    #[serde(rename = "ERR_ARITY_MISMATCH")]
    ErrArityMismatch,
    #[serde(rename = "ERR_RETURN_TYPE_MISMATCH")]
    ErrReturnTypeMismatch,

    // Loop control flow
    #[serde(rename = "ERR_BREAK_OUTSIDE_LOOP")]
    ErrBreakOutsideLoop,
    #[serde(rename = "ERR_CONTINUE_OUTSIDE_LOOP")]
    ErrContinueOutsideLoop,

    // Interrupt safety & recursion
    #[serde(rename = "ERR_ISR_FORBIDDEN_OP")]
    ErrIsrForbiddenOp,
    #[serde(rename = "ERR_UNRESOLVED_ISR")]
    ErrUnresolvedIsr,
    #[serde(rename = "ERR_RECURSION_DEPTH")]
    ErrRecursionDepth,

    // Modules & capabilities
    #[serde(rename = "ERR_IMPORT_CYCLE")]
    ErrImportCycle,
    #[serde(rename = "ERR_UNRESOLVED_IMPORT")]
    ErrUnresolvedImport,
    #[serde(rename = "ERR_UNSUPPORTED_CAPABILITY")]
    ErrUnsupportedCapability,

    // Foreign nodes
    #[serde(rename = "ERR_FOREIGN_UNTYPED")]
    ErrForeignUntyped,
    #[serde(rename = "ERR_FOREIGN_EXPORT_BLOCKED")]
    ErrForeignExportBlocked,

    // Display sink charsets
    #[serde(rename = "ERR_STRING_CHARSET")]
    ErrStringCharset,

    // First-class comments
    #[serde(rename = "ERR_COMMENT_FIELD")]
    ErrCommentField,
    #[serde(rename = "ERR_COMMENT_ANCHOR")]
    ErrCommentAnchor,

    // Hardware, profiles & wiring
    #[serde(rename = "ERR_UNKNOWN_PROFILE")]
    ErrUnknownProfile,
    #[serde(rename = "ERR_UNKNOWN_PORT")]
    ErrUnknownPort,
    #[serde(rename = "ERR_PIN_CAPABILITY_MISMATCH")]
    ErrPinCapabilityMismatch,
    #[serde(rename = "ERR_BAD_ENDPOINT")]
    ErrBadEndpoint,
    #[serde(rename = "ERR_UNSAFE_VOLTAGE_DOMAIN")]
    ErrUnsafeVoltageDomain,

    // Warnings
    #[serde(rename = "WARN_INDETERMINATE_LEVEL")]
    WarnIndeterminateLevel,
    #[serde(rename = "WARN_ASSUMPTION_ACTIVE")]
    WarnAssumptionActive,
    #[serde(rename = "WARN_COMMENT_ENCODING")]
    WarnCommentEncoding,
    #[serde(rename = "WARN_COMMENT_BIDI")]
    WarnCommentBidi,
    #[serde(rename = "WARN_DUTY_CLAMPED")]
    WarnDutyClamped,

    // Info
    #[serde(rename = "INFO_COMMENT_RELOCATED")]
    InfoCommentRelocated,
    #[serde(rename = "INFO_COMMENT_ORPHANED")]
    InfoCommentOrphaned,
}

/// Error returned when parsing an unknown diagnostic code string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseErrorCodeError {
    /// The unrecognized input string.
    pub input: String,
}

impl fmt::Display for ParseErrorCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown diagnostic code `{}`", self.input)
    }
}

impl std::error::Error for ParseErrorCodeError {}

impl ErrorCode {
    /// Complete ordered list of all 45 frozen diagnostic codes.
    pub const ALL: &'static [Self] = &[
        Self::ErrTomlSyntax,
        Self::ErrUnknownField,
        Self::ErrUnsupportedVersion,
        Self::ErrDuplicateId,
        Self::ErrDuplicateDecl,
        Self::ErrHardwarePinConflict,
        Self::ErrNameCollision,
        Self::ErrUnknownType,
        Self::ErrTypeMismatch,
        Self::ErrInvalidCast,
        Self::ErrArrayBoundsConst,
        Self::ErrUnknownFieldAccess,
        Self::ErrUndeclaredSymbol,
        Self::ErrCodecResolutionExceedsScale,
        Self::ErrBandLimitOverflow,
        Self::ErrConstWrite,
        Self::ErrHardwareLabelWrite,
        Self::ErrUnresolvedFunction,
        Self::ErrArityMismatch,
        Self::ErrReturnTypeMismatch,
        Self::ErrBreakOutsideLoop,
        Self::ErrContinueOutsideLoop,
        Self::ErrIsrForbiddenOp,
        Self::ErrUnresolvedIsr,
        Self::ErrRecursionDepth,
        Self::ErrImportCycle,
        Self::ErrUnresolvedImport,
        Self::ErrUnsupportedCapability,
        Self::ErrForeignUntyped,
        Self::ErrForeignExportBlocked,
        Self::ErrStringCharset,
        Self::ErrCommentField,
        Self::ErrCommentAnchor,
        Self::ErrUnknownProfile,
        Self::ErrUnknownPort,
        Self::ErrPinCapabilityMismatch,
        Self::ErrBadEndpoint,
        Self::ErrUnsafeVoltageDomain,
        Self::WarnIndeterminateLevel,
        Self::WarnAssumptionActive,
        Self::WarnCommentEncoding,
        Self::WarnCommentBidi,
        Self::WarnDutyClamped,
        Self::InfoCommentRelocated,
        Self::InfoCommentOrphaned,
    ];

    /// Canonical wire/display string identifier (e.g., `"ERR_TOML_SYNTAX"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ErrTomlSyntax => "ERR_TOML_SYNTAX",
            Self::ErrUnknownField => "ERR_UNKNOWN_FIELD",
            Self::ErrUnsupportedVersion => "ERR_UNSUPPORTED_VERSION",
            Self::ErrDuplicateId => "ERR_DUPLICATE_ID",
            Self::ErrDuplicateDecl => "ERR_DUPLICATE_DECL",
            Self::ErrHardwarePinConflict => "ERR_HARDWARE_PIN_CONFLICT",
            Self::ErrNameCollision => "ERR_NAME_COLLISION",
            Self::ErrUnknownType => "ERR_UNKNOWN_TYPE",
            Self::ErrTypeMismatch => "ERR_TYPE_MISMATCH",
            Self::ErrInvalidCast => "ERR_INVALID_CAST",
            Self::ErrArrayBoundsConst => "ERR_ARRAY_BOUNDS_CONST",
            Self::ErrUnknownFieldAccess => "ERR_UNKNOWN_FIELD_ACCESS",
            Self::ErrUndeclaredSymbol => "ERR_UNDECLARED_SYMBOL",
            Self::ErrCodecResolutionExceedsScale => "ERR_CODEC_RESOLUTION_EXCEEDS_SCALE",
            Self::ErrBandLimitOverflow => "ERR_BAND_LIMIT_OVERFLOW",
            Self::ErrConstWrite => "ERR_CONST_WRITE",
            Self::ErrHardwareLabelWrite => "ERR_HARDWARE_LABEL_WRITE",
            Self::ErrUnresolvedFunction => "ERR_UNRESOLVED_FUNCTION",
            Self::ErrArityMismatch => "ERR_ARITY_MISMATCH",
            Self::ErrReturnTypeMismatch => "ERR_RETURN_TYPE_MISMATCH",
            Self::ErrBreakOutsideLoop => "ERR_BREAK_OUTSIDE_LOOP",
            Self::ErrContinueOutsideLoop => "ERR_CONTINUE_OUTSIDE_LOOP",
            Self::ErrIsrForbiddenOp => "ERR_ISR_FORBIDDEN_OP",
            Self::ErrUnresolvedIsr => "ERR_UNRESOLVED_ISR",
            Self::ErrRecursionDepth => "ERR_RECURSION_DEPTH",
            Self::ErrImportCycle => "ERR_IMPORT_CYCLE",
            Self::ErrUnresolvedImport => "ERR_UNRESOLVED_IMPORT",
            Self::ErrUnsupportedCapability => "ERR_UNSUPPORTED_CAPABILITY",
            Self::ErrForeignUntyped => "ERR_FOREIGN_UNTYPED",
            Self::ErrForeignExportBlocked => "ERR_FOREIGN_EXPORT_BLOCKED",
            Self::ErrStringCharset => "ERR_STRING_CHARSET",
            Self::ErrCommentField => "ERR_COMMENT_FIELD",
            Self::ErrCommentAnchor => "ERR_COMMENT_ANCHOR",
            Self::ErrUnknownProfile => "ERR_UNKNOWN_PROFILE",
            Self::ErrUnknownPort => "ERR_UNKNOWN_PORT",
            Self::ErrPinCapabilityMismatch => "ERR_PIN_CAPABILITY_MISMATCH",
            Self::ErrBadEndpoint => "ERR_BAD_ENDPOINT",
            Self::ErrUnsafeVoltageDomain => "ERR_UNSAFE_VOLTAGE_DOMAIN",
            Self::WarnIndeterminateLevel => "WARN_INDETERMINATE_LEVEL",
            Self::WarnAssumptionActive => "WARN_ASSUMPTION_ACTIVE",
            Self::WarnCommentEncoding => "WARN_COMMENT_ENCODING",
            Self::WarnCommentBidi => "WARN_COMMENT_BIDI",
            Self::WarnDutyClamped => "WARN_DUTY_CLAMPED",
            Self::InfoCommentRelocated => "INFO_COMMENT_RELOCATED",
            Self::InfoCommentOrphaned => "INFO_COMMENT_ORPHANED",
        }
    }

    /// Default severity defined by the specification for this code.
    #[must_use]
    pub const fn default_severity(self) -> Severity {
        match self {
            Self::ErrTomlSyntax
            | Self::ErrUnknownField
            | Self::ErrUnsupportedVersion
            | Self::ErrDuplicateId
            | Self::ErrDuplicateDecl
            | Self::ErrHardwarePinConflict
            | Self::ErrNameCollision
            | Self::ErrUnknownType
            | Self::ErrTypeMismatch
            | Self::ErrInvalidCast
            | Self::ErrArrayBoundsConst
            | Self::ErrUnknownFieldAccess
            | Self::ErrUndeclaredSymbol
            | Self::ErrCodecResolutionExceedsScale
            | Self::ErrBandLimitOverflow
            | Self::ErrConstWrite
            | Self::ErrHardwareLabelWrite
            | Self::ErrUnresolvedFunction
            | Self::ErrArityMismatch
            | Self::ErrReturnTypeMismatch
            | Self::ErrBreakOutsideLoop
            | Self::ErrContinueOutsideLoop
            | Self::ErrIsrForbiddenOp
            | Self::ErrUnresolvedIsr
            | Self::ErrRecursionDepth
            | Self::ErrImportCycle
            | Self::ErrUnresolvedImport
            | Self::ErrUnsupportedCapability
            | Self::ErrForeignUntyped
            | Self::ErrForeignExportBlocked
            | Self::ErrStringCharset
            | Self::ErrCommentField
            | Self::ErrCommentAnchor
            | Self::ErrUnknownProfile
            | Self::ErrUnknownPort
            | Self::ErrPinCapabilityMismatch
            | Self::ErrBadEndpoint
            | Self::ErrUnsafeVoltageDomain => Severity::Error,
            Self::WarnIndeterminateLevel
            | Self::WarnAssumptionActive
            | Self::WarnCommentEncoding
            | Self::WarnCommentBidi
            | Self::WarnDutyClamped => Severity::Warning,
            Self::InfoCommentRelocated | Self::InfoCommentOrphaned => Severity::Info,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ErrorCode {
    type Err = ParseErrorCodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for &code in Self::ALL {
            if code.as_str() == s {
                return Ok(code);
            }
        }
        Err(ParseErrorCodeError {
            input: s.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn all_45_codes_are_unique_and_round_trip() {
        assert_eq!(ErrorCode::ALL.len(), 45);
        let mut seen = BTreeSet::new();
        for &code in ErrorCode::ALL {
            let text = code.as_str();
            assert!(seen.insert(text), "duplicate code string: {text}");
            let parsed = ErrorCode::from_str(text).unwrap();
            assert_eq!(parsed, code);
            assert_eq!(code.to_string(), text);
        }
    }

    #[test]
    fn default_severities_match_code_prefixes() {
        for &code in ErrorCode::ALL {
            let s = code.as_str();
            match code.default_severity() {
                Severity::Error => assert!(s.starts_with("ERR_"), "{s}"),
                Severity::Warning => assert!(s.starts_with("WARN_"), "{s}"),
                Severity::Info => assert!(s.starts_with("INFO_"), "{s}"),
            }
        }
    }

    #[test]
    fn unknown_code_returns_parse_error() {
        let err = ErrorCode::from_str("ERR_DOES_NOT_EXIST").unwrap_err();
        assert_eq!(err.input, "ERR_DOES_NOT_EXIST");
        assert!(err.to_string().contains("ERR_DOES_NOT_EXIST"));
    }
}
