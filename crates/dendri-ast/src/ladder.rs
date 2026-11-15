//! Ladder (IEC 61131-3 LD) AST stubs — types only in Phase 0.
//!
//! Reserved so the IR envelope can grow Ladder blocks without shatter.

use serde::{Deserialize, Serialize};

use crate::expr::Expr;

/// One Ladder network / program unit attached to a [`crate::Program`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderBlock {
    pub name: String,
    #[serde(default)]
    pub rungs: Vec<LadderRung>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderRung {
    pub id: String,
    #[serde(default)]
    pub cells: Vec<LadderCell>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Cell kinds mirror the Implementation Plan Ladder token sketch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LadderCell {
    ContactNo { operand: String },
    ContactNc { operand: String },
    Coil { operand: String },
    CoilSet { operand: String },
    CoilReset { operand: String },
    /// IEC timer FB placeholder (TON / TOF / TP).
    FbTimer {
        fb: TimerFbKind,
        instance: String,
        #[serde(default)]
        args: Vec<Expr>,
    },
    /// IEC counter FB placeholder (CTU / CTD / CTUD).
    FbCounter {
        fb: CounterFbKind,
        instance: String,
        #[serde(default)]
        args: Vec<Expr>,
    },
    FbCompare {
        op: String,
        left: Expr,
        right: Expr,
    },
    /// Horizontal / vertical wire placeholder for editors.
    Wire,
    Empty,
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
