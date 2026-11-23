//! Structural IR for IEC 61131-3 Ladder (`Network`) and IEC 60848 GRAFCET (`Chart`)
//! (`01-asl-ir-and-project-schema.md` §2.5, ADR-026).
//!
//! **Zero synthetic temporaries (`__rail_N`, `__sfc_*`) are ever persisted here.**
//! `dendri-vm` lowers both structures strictly in memory at load time.

use crate::expr::{Expr, LValue};
use crate::ident::NodeId;
use crate::project::ForeignFidelity;
use crate::stmt::Stmt;
use crate::types::FbKind;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// IEC 61131-3 Ladder Diagram (`PouBody::Network`)
// ---------------------------------------------------------------------------

/// Contact kinds in IEC 61131-3 Ladder Diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LadderContactKind {
    NormallyOpen,
    NormallyClosed,
    PositiveTransition,
    NegativeTransition,
}

/// Coil kinds in IEC 61131-3 Ladder Diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LadderCoilKind {
    Normal,
    Negated,
    Set,
    Reset,
    PositiveTransition,
    NegativeTransition,
}

/// Strictly typed comparison box operator (never string-matched fallback!).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LadderCompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Box category inside a Ladder cell.
#[derive(Debug, Clone, PartialEq)]
pub enum LadderBoxKind {
    /// Stateful Function Block invocation (`TON`, `CTU`, `R_TRIG`, `SR`, `UART_SEND`, user `FB`).
    Fb {
        kind: FbKind,
        instance: LValue,
        inputs: Vec<(String, Expr)>,
    },
    /// Comparison box (`EQ`, `NE`, `LT`, `LE`, `GT`, `GE`) outputting `EN AND cmp`.
    Compare {
        op: LadderCompareOp,
        left: Expr,
        right: Expr,
    },
    /// `EN`/`ENO`-gated function, math, move, or callable `Structured POU` box.
    Call {
        callee: String,
        args: Vec<(Option<String>, Expr)>,
        output: Option<LValue>,
    },
    /// Locked `Foreign` box rendered non-destructively inside a Ladder rung.
    Foreign {
        language: String,
        fidelity: ForeignFidelity,
        text: String,
        pack_hint: Option<String>,
    },
}

/// Typed cell inside a Ladder rung grid (`(row, col)`).
#[derive(Debug, Clone, PartialEq)]
pub enum LadderCellKind {
    /// Open circuit (`false`) — NEVER conducts!
    Empty,
    /// Horizontal conducting wire.
    Wire,
    Contact {
        kind: LadderContactKind,
        target: LValue,
    },
    Coil {
        kind: LadderCoilKind,
        target: LValue,
    },
    /// Parallel OR sub-branches (`Vec<Vec<LadderCell>>`, each inner vec is a branch series).
    ParallelOr {
        branches: Vec<Vec<LadderCell>>,
    },
    Box(LadderBoxKind),
}

/// Single NodeId-anchored cell inside a [`LadderRung`].
#[derive(Debug, Clone, PartialEq)]
pub struct LadderCell {
    pub id: NodeId,
    pub row: u32,
    pub col: u32,
    pub kind: LadderCellKind,
}

/// Ordered rung inside a [`LadderNetwork`].
#[derive(Debug, Clone, PartialEq)]
pub struct LadderRung {
    pub id: NodeId,
    pub order: u32,
    pub cells: Vec<LadderCell>,
}

/// First-class structural Ladder Diagram POU body (`PouBody::Network`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LadderNetwork {
    pub rungs: Vec<LadderRung>,
}

// ---------------------------------------------------------------------------
// IEC 60848 / IEC 61131-3 GRAFCET / SFC (`PouBody::Chart`)
// ---------------------------------------------------------------------------

/// Action qualifiers per IEC 61131-3 Table 47 (`04-frontends-visual-and-sync.md` §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionQualifier {
    /// Non-stored (automatically resets boolean coil when step deactivates).
    N,
    /// Set / Stored.
    S,
    /// Overriding Reset.
    R,
    /// Time Limited (active for duration while step is active).
    L,
    /// Time Delayed (activates after duration while step remains active).
    D,
    /// Pulse (1 scan on step activation).
    P,
    /// Rising edge (on step activation).
    P1,
    /// Falling edge (on step deactivation).
    P0,
}

/// Action attached to an [`SfcStep`].
#[derive(Debug, Clone, PartialEq)]
pub struct SfcAction {
    pub id: NodeId,
    pub qualifier: ActionQualifier,
    /// Duration required for `L` and `D` qualifiers (`TIME` expression).
    pub duration: Option<Expr>,
    /// Target coil/boolean variable (for `N`/`S`/`R`/`L`/`D` coil actions) or inline body statements.
    pub target: Option<LValue>,
    pub body: Vec<Stmt>,
}

/// Step node inside an [`SfcChart`] (normal, initial, or enclosed macro-step).
#[derive(Debug, Clone, PartialEq)]
pub struct SfcStep {
    pub id: NodeId,
    pub name: String,
    pub initial: bool,
    pub actions: Vec<SfcAction>,
    /// Optional enclosed sub-chart name for macro-steps (`<MacroStep>.DONE`).
    pub macro_chart: Option<String>,
}

/// Transition linking upstream `from` steps to downstream `to` steps.
///
/// - Multiple `from` steps express simultaneous **AND convergence**.
/// - Multiple `to` steps express simultaneous **AND divergence**.
/// - Competing transitions sharing a `from` step express **OR divergence**, resolved
///   deterministically by ascending `priority` (lower number = higher priority).
#[derive(Debug, Clone, PartialEq)]
pub struct SfcTransition {
    pub id: NodeId,
    pub name: String,
    pub from_steps: Vec<String>,
    pub to_steps: Vec<String>,
    pub priority: u32,
    pub receptivity: Expr,
}

/// First-class structural GRAFCET / SFC POU body (`PouBody::Chart`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SfcChart {
    pub steps: Vec<SfcStep>,
    pub transitions: Vec<SfcTransition>,
    /// `false` = single transition step per scan; `true` = evolve until stability (with cycle guard).
    pub evolve_to_stability: bool,
}
