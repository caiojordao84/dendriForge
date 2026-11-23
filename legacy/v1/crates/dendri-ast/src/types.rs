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

/// Internal resistor bias for a digital input (`CFG_DIGITAL_INPUT` consults this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullMode {
    Pullup,
    Pulldown,
    #[default]
    None,
}

impl PullMode {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pullup => "pullup",
            Self::Pulldown => "pulldown",
            Self::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pullup" | "pull-up" | "up" => Some(Self::Pullup),
            "pulldown" | "pull-down" | "down" => Some(Self::Pulldown),
            "none" | "" => Some(Self::None),
            _ => None,
        }
    }
}

#[cfg(test)]
mod pull_mode_tests {
    use super::*;
    use crate::program::HardwareEntry;

    #[test]
    fn hardware_entry_deserializes_pullup() {
        let toml = r#"
pin = 2
hw_type = "digital"
io_mode = "DI"
label = "btnPin"
target_role = "digital-in"
pull = "pullup"
"#;
        let hw: HardwareEntry = toml::from_str(toml).expect("parse hardware");
        assert_eq!(hw.pull, PullMode::Pullup);
        assert_eq!(hw.label, "btnPin");
    }

    #[test]
    fn hardware_entry_omits_none_pull_on_serialize() {
        let hw = HardwareEntry {
            pin: 13,
            hw_type: HwType::Digital,
            io_mode: IoMode::Do,
            label: "ledPin".into(),
            target_role: "digital-out".into(),
            pull: PullMode::None,
        };
        let text = toml::to_string(&hw).expect("serialize");
        assert!(
            !text.contains("pull"),
            "none pull must skip serialize: {text}"
        );
    }

    #[test]
    fn pull_mode_parses_aliases() {
        assert_eq!(PullMode::parse("pullup"), Some(PullMode::Pullup));
        assert_eq!(PullMode::parse("pulldown"), Some(PullMode::Pulldown));
        assert_eq!(PullMode::parse("none"), Some(PullMode::None));
    }
}
