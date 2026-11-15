//! IEC 61131-3-aligned scalar types used by ASL programs and profiles.

use serde::{Deserialize, Serialize};

/// Data types permitted in `[[data]]` and expression typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DataType {
    Bool,
    Int,
    Dint,
    Uint,
    Real,
    Lreal,
    Time,
    String,
    Byte,
    Word,
    Dword,
}

impl DataType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "BOOL",
            Self::Int => "INT",
            Self::Dint => "DINT",
            Self::Uint => "UINT",
            Self::Real => "REAL",
            Self::Lreal => "LREAL",
            Self::Time => "TIME",
            Self::String => "STRING",
            Self::Byte => "BYTE",
            Self::Word => "WORD",
            Self::Dword => "DWORD",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "BOOL" => Some(Self::Bool),
            "INT" => Some(Self::Int),
            "DINT" => Some(Self::Dint),
            "UINT" => Some(Self::Uint),
            "REAL" => Some(Self::Real),
            "LREAL" => Some(Self::Lreal),
            "TIME" => Some(Self::Time),
            "STRING" => Some(Self::String),
            "BYTE" => Some(Self::Byte),
            "WORD" => Some(Self::Word),
            "DWORD" => Some(Self::Dword),
            _ => None,
        }
    }
}

/// Hardware channel type on a pin entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HwType {
    #[serde(alias = "BOOL")]
    Bool,
    Analog,
    Digital,
}

/// Pin direction / mode (canonical DI / DO / BI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IoMode {
    #[serde(rename = "DI", alias = "input", alias = "INPUT")]
    Di,
    #[serde(rename = "DO", alias = "output", alias = "OUTPUT")]
    Do,
    #[serde(rename = "BI", alias = "bi", alias = "BI")]
    Bi,
}
