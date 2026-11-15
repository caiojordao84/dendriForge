//! Literal values in ASL expressions / data init fields.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Literal {
    Bool { value: bool },
    Int { value: i64 },
    Uint { value: u64 },
    Real { value: f64 },
    String { value: String },
    /// IEC time literal text, e.g. `T#200ms` (kept textual until Phase 1 normalizes).
    Time { value: String },
    /// Hex literal text (`0x…` or IEC `16#…`).
    Hex { value: String },
}
