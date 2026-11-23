//! ASL v2 statement AST (`01-asl-ir-and-project-schema.md` §4.1).
//!
//! Every statement carries a stable [`NodeId`] and [`Span`].

use crate::expr::{CallArg, Expr, LValue};
use crate::ident::NodeId;
use crate::project::ForeignFidelity;
use dendri_span::Span;

/// Single `elif` arm inside an `If` statement.
#[derive(Debug, Clone, PartialEq)]
pub struct ElifArm {
    pub condition: Expr,
    pub body: Vec<Stmt>,
}

/// Match pattern inside a `Switch` case (literal value or inclusive range `lo..=hi`).
#[derive(Debug, Clone, PartialEq)]
pub enum CasePattern {
    Value(Expr),
    Range { start: Expr, end: Expr },
}

/// Single branch inside a `Switch` statement (`CASE ... OF` / `switch` / `match`).
#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    pub patterns: Vec<CasePattern>,
    pub body: Vec<Stmt>,
}

/// Statement body kind (`01` §4.1).
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Assign {
        target: LValue,
        value: Expr,
    },
    Call {
        callee: String,
        args: Vec<CallArg>,
    },
    FbInvoke {
        instance: LValue,
        inputs: Vec<(String, Expr)>,
    },
    If {
        condition: Expr,
        then_body: Vec<Stmt>,
        elif_arms: Vec<ElifArm>,
        else_body: Vec<Stmt>,
    },
    Switch {
        scrutinee: Expr,
        cases: Vec<SwitchCase>,
        default: Option<Vec<Stmt>>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    For {
        counter: String,
        from: Expr,
        to: Expr,
        step: Expr,
        inclusive: bool,
        body: Vec<Stmt>,
    },
    Break,
    Continue,
    Return {
        value: Option<Expr>,
    },
    Wait {
        duration: Expr,
    },
    /// Statement-level foreign construct (`Stmt::Foreign`, replacing v1's removed `VendorExtension`).
    Foreign {
        language: String,
        fidelity: ForeignFidelity,
        text: String,
        pack_hint: Option<String>,
    },
}

/// A spanned, NodeId-anchored ASL v2 statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub id: NodeId,
    pub kind: StmtKind,
    pub span: Span,
}

impl Stmt {
    /// Creates a new statement node.
    #[must_use]
    pub const fn new(id: NodeId, kind: StmtKind, span: Span) -> Self {
        Self { id, kind, span }
    }
}
