//! Ladder (IEC 61131-3 LD) AST — diagram IR + TOML serialization.
//!
//! Cells form left-to-right series power flow within a rung. Parallel branches
//! are MVP-deferred (document as gap). Layout coords are editor hints and
//! round-trip through TOML.

use serde::{Deserialize, Serialize};

use crate::expr::Expr;
use crate::stmt::Stmt;

/// One Ladder network / program unit attached to a [`crate::Program`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderBlock {
    pub name: String,
    #[serde(default)]
    pub rungs: Vec<LadderRung>,
    /// Optional I/O assignment table (mandatory for full industrial use; MVP may be empty).
    #[serde(default)]
    pub io_map: Vec<LadderIoBinding>,
}

/// Binding from a ladder operand name to a hardware pin / ASL var.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderIoBinding {
    pub operand: String,
    /// e.g. `board.D2`, `led1`, or ASL var name.
    pub target: String,
    #[serde(default)]
    pub direction: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderRung {
    pub id: String,
    #[serde(default)]
    pub cells: Vec<LadderCell>,
    #[serde(default)]
    pub comment: Option<String>,
    /// Vertical order in the editor (0 = top).
    #[serde(default)]
    pub row: u32,
}

/// Cell kinds mirror IEC LD contacts/coils/FBs plus DendriForge action hooks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LadderCell {
    ContactNo {
        operand: String,
        #[serde(default)]
        col: u32,
    },
    ContactNc {
        operand: String,
        #[serde(default)]
        col: u32,
    },
    Coil {
        operand: String,
        #[serde(default)]
        col: u32,
    },
    CoilSet {
        operand: String,
        #[serde(default)]
        col: u32,
    },
    CoilReset {
        operand: String,
        #[serde(default)]
        col: u32,
    },
    /// IEC timer FB placeholder (TON / TOF / TP).
    FbTimer {
        fb: TimerFbKind,
        instance: String,
        #[serde(default)]
        args: Vec<Expr>,
        #[serde(default)]
        col: u32,
    },
    /// IEC counter FB placeholder (CTU / CTD / CTUD).
    FbCounter {
        fb: CounterFbKind,
        instance: String,
        #[serde(default)]
        args: Vec<Expr>,
        #[serde(default)]
        col: u32,
    },
    FbCompare {
        op: String,
        left: Expr,
        right: Expr,
        #[serde(default)]
        col: u32,
    },
    /// Execute ASL statement(s) when power is true (MVP sim bridge).
    Action {
        #[serde(default)]
        stmts: Vec<Stmt>,
        #[serde(default)]
        col: u32,
    },
    /// Horizontal / vertical wire placeholder for editors.
    Wire {
        #[serde(default)]
        col: u32,
    },
    Empty {
        #[serde(default)]
        col: u32,
    },
}

impl LadderCell {
    pub fn col(&self) -> u32 {
        match self {
            Self::ContactNo { col, .. }
            | Self::ContactNc { col, .. }
            | Self::Coil { col, .. }
            | Self::CoilSet { col, .. }
            | Self::CoilReset { col, .. }
            | Self::FbTimer { col, .. }
            | Self::FbCounter { col, .. }
            | Self::FbCompare { col, .. }
            | Self::Action { col, .. }
            | Self::Wire { col }
            | Self::Empty { col } => *col,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TimerFbKind {
    Ton,
    Tof,
    Tp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CounterFbKind {
    Ctu,
    Ctd,
    Ctud,
}

/// Document envelope stored in `[[sources]].text` for language = ladder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderDocument {
    pub name: String,
    #[serde(default)]
    pub rungs: Vec<LadderRung>,
    #[serde(default)]
    pub io_map: Vec<LadderIoBinding>,
    /// Optional hardware/data/setup mirrored for standalone ladder sources.
    #[serde(default)]
    pub hardware: Vec<crate::program::HardwareEntry>,
    #[serde(default)]
    pub data: Vec<crate::program::DataEntry>,
    #[serde(default)]
    pub setup: Vec<Stmt>,
}

impl LadderDocument {
    pub fn into_block(self) -> LadderBlock {
        LadderBlock {
            name: self.name,
            rungs: self.rungs,
            io_map: self.io_map,
        }
    }

    pub fn from_block(block: &LadderBlock) -> Self {
        Self {
            name: block.name.clone(),
            rungs: block.rungs.clone(),
            io_map: block.io_map.clone(),
            hardware: vec![],
            data: vec![],
            setup: vec![],
        }
    }
}
