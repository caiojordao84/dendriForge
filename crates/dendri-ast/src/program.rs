//! Top-level ASL program document (TOML envelope + statement AST).

use serde::{Deserialize, Serialize};

use crate::stmt::Stmt;
use crate::types::{DataType, HwType, IoMode};

/// File extension for program IR: `.toml` (shape distinguishes program vs project).
pub const PROGRAM_EXTENSION: &str = "toml";

/// First ASL language version tag (assigned after MVP acceptance).
///
/// Sole scheme: `metadata.asl_version` in program `.toml` / embedded `[asl.metadata]`.
/// Not Cargo crate version; not legacy TOON `ASLversion` / `v010`.
pub const ASL_VERSION: &str = "0.0.1";

fn default_asl_version() -> Option<String> {
    Some(ASL_VERSION.to_string())
}

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
    /// ASL language version. Current tag: [`ASL_VERSION`] (`0.0.1`).
    #[serde(default = "default_asl_version")]
    pub asl_version: Option<String>,
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

impl Default for Metadata {
    fn default() -> Self {
        Self {
            asl_version: default_asl_version(),
            project_name: None,
            editor: None,
            author: None,
            notes: None,
            expect_error: None,
        }
    }
}

impl Metadata {
    /// Build metadata stamped with the current [`ASL_VERSION`].
    pub fn with_project(project_name: impl Into<String>) -> Self {
        Self {
            project_name: Some(project_name.into()),
            editor: Some("dendriForge".into()),
            ..Self::default()
        }
    }
}

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
