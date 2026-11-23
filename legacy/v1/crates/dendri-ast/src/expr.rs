//! Expression AST shared by statements, conditions, and call arguments.

use serde::{Deserialize, Serialize};

use crate::lit::Literal;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expr {
    Lit {
        value: Literal,
    },
    /// Convenience: bare bool without nested `Lit` wrapper (fixture sugar).
    Bool {
        value: bool,
    },
    Uint {
        value: u64,
    },
    Int {
        value: i64,
    },
    Real {
        value: f64,
    },
    String {
        value: String,
    },
    Time {
        value: String,
    },
    Hex {
        value: String,
    },
    Var {
        name: String,
    },
    Call {
        name: String,
        #[serde(default)]
        args: Vec<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binop {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Escape hatch: surface text to be parsed in Phase 1 when typed tables
    /// would be unreasonably deep. Prefer typed forms for MVP goldens.
    ExprText {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOp {
    Not,
    Neg,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Xor,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}
