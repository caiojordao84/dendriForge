//! Statement AST for setup / loop / function bodies.

use serde::{Deserialize, Serialize};

use crate::expr::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Stmt {
    Assign {
        target: String,
        value: Expr,
    },
    Call {
        name: String,
        #[serde(default)]
        args: Vec<Expr>,
    },
    If {
        condition: Expr,
        #[serde(default)]
        then: Vec<Stmt>,
        #[serde(default)]
        elif: Vec<ElifArm>,
        #[serde(default, rename = "else")]
        else_body: Option<Vec<Stmt>>,
    },
    While {
        condition: Expr,
        #[serde(default)]
        body: Vec<Stmt>,
    },
    For {
        /// Depth-indexed generator name (`_i0`) or explicit VAR counter.
        #[serde(default)]
        counter: Option<String>,
        /// Exclusive upper bound expression (`TO n` → `0..n`).
        to: Expr,
        #[serde(default)]
        body: Vec<Stmt>,
    },
    Return {
        #[serde(default)]
        value: Option<Expr>,
    },
    Exit,
    Continue,
    VendorExtension {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElifArm {
    pub condition: Expr,
    #[serde(default)]
    pub then: Vec<Stmt>,
}
