//! Unified ASL error ID scheme.
//!
//! Phase 0 freezes on the `ERR_*` vocabulary (matching the error corpus).
//! Legacy `PARSE_ERROR_00x` IDs from `docs/grammar.md` map 1:1 onto these
//! variants via [`ErrorCode::from_legacy_parse_error`].

use serde::{Deserialize, Serialize};

/// Diagnostic severity for validators / parsers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSeverity {
    Error,
    Warning,
    Info,
}

/// Single frozen error-code enum for ASL parse + validate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    // --- Metadata / document structure ---
    /// Required `[metadata]` key missing.
    ErrMetadataMissingField,
    /// `[[data]]` / `[[hardware]]` / setup / loop / function step count mismatch.
    ErrBlockCountMismatch,
    /// Duplicate pin number in `[[hardware]]`.
    ErrHardwarePinConflict,

    // --- Types / scopes / names ---
    /// Data entry uses a type not in the ASL type set.
    ErrDataUnknownType,
    /// Identifier not declared in `[[data]]` or `[[hardware]]`.
    ErrUndeclaredVariable,
    /// Type mismatch at an expression / call site.
    ErrTypeMismatch,
    /// Assignment to a `CONST`-scoped variable.
    ErrConstWriteViolation,
    /// Call arity does not match declaration / stdlib signature.
    ErrFunctionArityMismatch,

    // --- Time / pins / resolution ---
    /// Raw subtraction of time-sourced `DWORD` values; use `TIME_DIFF`.
    ErrRawTimeSubtraction,
    /// Pin argument must resolve via hardware label (raw int emit forbidden).
    ErrPinLabelRequired,
    /// ISR reference does not resolve to a `[[functions]]` entry.
    ErrUnresolvedIsr,
    /// Call name not in stdlib or user functions.
    ErrUnresolvedFunction,
    /// Unresolved mapping requires `VENDOR_EXTENSION`.
    ErrVendorExtensionRequired,

    // --- Control / memory / ISR ---
    /// Nested `FOR` / repeat depth exceeds maximum (4).
    ErrNestedLoopDepth,
    /// `RETAIN` / `PERSISTENT` written without `SYS_WRITE_RETAIN`.
    ErrRetainWriteWithoutSys,
    /// `isr_*` body contains a heap-allocating call.
    ErrIsrHeapAllocation,

    // --- Schema / IR ---
    /// Unknown statement or expression `kind` tag in TOML IR.
    ErrUnknownIrKind,
    /// TOML envelope failed structural deserialize.
    ErrIrDeserialize,
}

impl ErrorCode {
    /// Stable wire / fixture id string (e.g. `ERR_UNDECLARED_VARIABLE`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ErrMetadataMissingField => "ERR_METADATA_MISSING_FIELD",
            Self::ErrBlockCountMismatch => "ERR_BLOCK_COUNT_MISMATCH",
            Self::ErrHardwarePinConflict => "ERR_HARDWARE_PIN_CONFLICT",
            Self::ErrDataUnknownType => "ERR_DATA_UNKNOWN_TYPE",
            Self::ErrUndeclaredVariable => "ERR_UNDECLARED_VARIABLE",
            Self::ErrTypeMismatch => "ERR_TYPE_MISMATCH",
            Self::ErrConstWriteViolation => "ERR_CONST_WRITE_VIOLATION",
            Self::ErrFunctionArityMismatch => "ERR_FUNCTION_ARITY_MISMATCH",
            Self::ErrRawTimeSubtraction => "ERR_RAW_TIME_SUBTRACTION",
            Self::ErrPinLabelRequired => "ERR_PIN_LABEL_REQUIRED",
            Self::ErrUnresolvedIsr => "ERR_UNRESOLVED_ISR",
            Self::ErrUnresolvedFunction => "ERR_UNRESOLVED_FUNCTION",
            Self::ErrVendorExtensionRequired => "ERR_VENDOR_EXTENSION_REQUIRED",
            Self::ErrNestedLoopDepth => "ERR_NESTED_LOOP_DEPTH",
            Self::ErrRetainWriteWithoutSys => "ERR_RETAIN_WRITE_WITHOUT_SYS",
            Self::ErrIsrHeapAllocation => "ERR_ISR_HEAP_ALLOCATION",
            Self::ErrUnknownIrKind => "ERR_UNKNOWN_IR_KIND",
            Self::ErrIrDeserialize => "ERR_IR_DESERIALIZE",
        }
    }

    /// Map legacy `PARSE_ERROR_00x` catalogue entries onto the frozen enum.
    pub fn from_legacy_parse_error(id: &str) -> Option<Self> {
        match id {
            "PARSE_ERROR_001" => Some(Self::ErrUndeclaredVariable),
            "PARSE_ERROR_002" => Some(Self::ErrTypeMismatch),
            "PARSE_ERROR_003" => Some(Self::ErrRawTimeSubtraction),
            "PARSE_ERROR_004" => Some(Self::ErrPinLabelRequired),
            "PARSE_ERROR_005" => Some(Self::ErrUnresolvedIsr),
            "PARSE_ERROR_006" => Some(Self::ErrUnresolvedFunction),
            "PARSE_ERROR_007" => Some(Self::ErrVendorExtensionRequired),
            "PARSE_ERROR_008" | "PARSE_ERROR_009" | "PARSE_ERROR_010" | "PARSE_ERROR_011"
            | "PARSE_ERROR_012" => Some(Self::ErrBlockCountMismatch),
            "PARSE_ERROR_013" => Some(Self::ErrNestedLoopDepth),
            "PARSE_ERROR_014" => Some(Self::ErrRetainWriteWithoutSys),
            "PARSE_ERROR_015" => Some(Self::ErrIsrHeapAllocation),
            _ => None,
        }
    }

    pub const fn severity(self) -> ErrorSeverity {
        ErrorSeverity::Error
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_parse_errors_map() {
        assert_eq!(
            ErrorCode::from_legacy_parse_error("PARSE_ERROR_001"),
            Some(ErrorCode::ErrUndeclaredVariable)
        );
        assert_eq!(
            ErrorCode::from_legacy_parse_error("PARSE_ERROR_003"),
            Some(ErrorCode::ErrRawTimeSubtraction)
        );
        assert_eq!(ErrorCode::ErrUndeclaredVariable.as_str(), "ERR_UNDECLARED_VARIABLE");
    }
}
