//! GRAFCET (IEC 60848) AST stubs — types only in Phase 0.
//!
//! Reserved so sim / editors can attach steps & transitions later without
//! breaking the program envelope.

use serde::{Deserialize, Serialize};

use crate::expr::Expr;
use crate::stmt::Stmt;

/// One GRAFCET chart attached to a [`crate::Program`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrafcetBlock {
    pub name: String,
    #[serde(default)]
    pub initial_steps: Vec<String>,
    #[serde(default)]
    pub steps: Vec<GrafcetStep>,
    #[serde(default)]
    pub transitions: Vec<GrafcetTransition>,
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
}
