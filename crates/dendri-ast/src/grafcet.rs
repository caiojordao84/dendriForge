//! GRAFCET / SFC (IEC 60848 + IEC 61131-3 SFC) AST — diagram IR + TOML.
//!
//! Steps hold ASL actions; transitions carry receptivity expressions.
//! Layout positions are editor hints and round-trip through TOML.
//! MVP: linear charts + simple AND divergence (multi from/to); nested charts deferred.

use serde::{Deserialize, Serialize};

use crate::expr::Expr;
use crate::stmt::Stmt;

/// One GRAFCET / SFC chart attached to a [`crate::Program`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrafcetBlock {
    pub name: String,
    #[serde(default)]
    pub initial_steps: Vec<String>,
    #[serde(default)]
    pub steps: Vec<GrafcetStep>,
    #[serde(default)]
    pub transitions: Vec<GrafcetTransition>,
    /// Optional I/O assignment table.
    #[serde(default)]
    pub io_map: Vec<GrafcetIoBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrafcetIoBinding {
    pub signal: String,
    pub target: String,
    #[serde(default)]
    pub direction: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrafcetStep {
    pub id: String,
    #[serde(default)]
    pub initial: bool,
    /// Actions executed while the step is active (stored as ASL stmts).
    #[serde(default)]
    pub actions: Vec<Stmt>,
    #[serde(default)]
    pub comment: Option<String>,
    /// Editor canvas position (logical units).
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrafcetTransition {
    pub id: String,
    /// Upstream step ids.
    pub from: Vec<String>,
    /// Downstream step ids.
    pub to: Vec<String>,
    /// Receptivity condition.
    pub condition: Expr,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

/// Document envelope stored in `[[sources]].text` for language = sfc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrafcetDocument {
    pub name: String,
    #[serde(default)]
    pub initial_steps: Vec<String>,
    #[serde(default)]
    pub steps: Vec<GrafcetStep>,
    #[serde(default)]
    pub transitions: Vec<GrafcetTransition>,
    #[serde(default)]
    pub io_map: Vec<GrafcetIoBinding>,
    #[serde(default)]
    pub hardware: Vec<crate::program::HardwareEntry>,
    #[serde(default)]
    pub data: Vec<crate::program::DataEntry>,
    #[serde(default)]
    pub setup: Vec<Stmt>,
}

impl GrafcetDocument {
    pub fn into_block(self) -> GrafcetBlock {
        GrafcetBlock {
            name: self.name,
            initial_steps: self.initial_steps,
            steps: self.steps,
            transitions: self.transitions,
            io_map: self.io_map,
        }
    }

    pub fn from_block(block: &GrafcetBlock) -> Self {
        Self {
            name: block.name.clone(),
            initial_steps: block.initial_steps.clone(),
            steps: block.steps.clone(),
            transitions: block.transitions.clone(),
            io_map: block.io_map.clone(),
            hardware: vec![],
            data: vec![],
            setup: vec![],
        }
    }
}
