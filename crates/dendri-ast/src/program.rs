//! Top-level ASL program document (TOML envelope + statement AST).

use serde::{Deserialize, Serialize};

use crate::stmt::Stmt;
use crate::types::{DataType, HwType, IoMode};

/// File extension for program IR: `.asl.toml`.
pub const PROGRAM_EXTENSION: &str = "asl.toml";

/// ASL program — Abstract Syntax Language IR root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub metadata: Metadata,
    #[serde(default)]
    pub hardware: Vec<HardwareEntry>,
    #[serde(default)]
    pub data: Vec<DataEntry>,
    #[serde(default)]
    pub setup: Vec<Stmt>,
    /// Behavioral scan-cycle body. Key name `loop` in TOML.
    #[serde(default, rename = "loop")]
    pub loop_body: Vec<Stmt>,
    #[serde(default)]
    pub functions: Vec<FunctionDef>,
    /// Ladder dialect blocks (IEC 61131-3 LD) — populated by DEN-8 sync.
    #[serde(default)]
    pub ladder: Vec<crate::ladder::LadderBlock>,
    /// GRAFCET / SFC blocks (IEC 60848) — populated by DEN-8 sync.
    #[serde(default)]
    pub grafcet: Vec<crate::grafcet::GrafcetBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    /// Required by validate (may be absent in negative fixtures).
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub editor: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// Expected error id for negative fixtures (`ERR_*`).
    #[serde(default)]
    pub expect_error: Option<String>,
}

// ASL language versioning is intentionally absent until MVP.
// Do not reintroduce `asl_version` / document `version` here before then;
// first released ASL tag will be `0.0.1` once parsers + MVP exist.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardwareEntry {
    pub pin: u32,
    pub hw_type: HwType,
    pub io_mode: IoMode,
    pub label: String,
    /// Role string (e.g. `digital-out`, `pwm-out`, `adc:t`, `i2c-sda`).
    /// Kept as text in Phase 0; Phase 1 may tighten to [`TargetRole`].
    pub target_role: String,
}

/// Known hardware target roles (documentation / future enum tightening).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetRole {
    DigitalOut,
    DigitalIn,
    DigitalInt,
    PwmOut,
    AdcTemp,
    AdcRaw,
    I2cSda,
    I2cScl,
    SpiMosi,
    SpiMiso,
    SpiSck,
    SpiCs,
    UartTx,
    UartRx,
    IntRising,
    IntFalling,
    IntChange,
}

impl TargetRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DigitalOut => "digital-out",
            Self::DigitalIn => "digital-in",
            Self::DigitalInt => "digital-int",
            Self::PwmOut => "pwm-out",
            Self::AdcTemp => "adc:t",
            Self::AdcRaw => "adc:raw",
            Self::I2cSda => "i2c-sda",
            Self::I2cScl => "i2c-scl",
            Self::SpiMosi => "spi-mosi",
            Self::SpiMiso => "spi-miso",
            Self::SpiSck => "spi-sck",
            Self::SpiCs => "spi-cs",
            Self::UartTx => "uart-tx",
            Self::UartRx => "uart-rx",
            Self::IntRising => "int-rising",
            Self::IntFalling => "int-falling",
            Self::IntChange => "int-change",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: DataType,
    pub scope: Scope,
    /// Init literal as surface text (`FALSE`, `0`, `T#200ms`, …) for Phase 0.
    pub init: String,
    #[serde(default = "default_unit")]
    pub unit: String,
}

fn default_unit() -> String {
    "-".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Scope {
    Const,
    Var,
    Retain,
    Persistent,
    Temp,
    Extern,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default)]
    pub params: Vec<String>,
    #[serde(default)]
    pub body: Vec<Stmt>,
}
