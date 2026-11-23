//! Canonical typed expression AST (`01-asl-ir-and-project-schema.md` §4.2, ADR-007).
//!
//! Zero `ExprText` escape hatches and zero duplicate literal variants.

use dendri_span::Span;
use serde::{Deserialize, Serialize};

/// Typed literal values in ASL v2 expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Bool(bool),
    /// Arbitrary-precision parsed integer literal (checked against target width during validation).
    Int(i128),
    /// IEEE-754 double-precision floating-point literal (narrowed/validated against `REAL`/`LREAL`).
    Float(f64),
    /// IEC 61131-3 duration literal normalized to microseconds (`T#500us`, `T#1s500ms`, `T#2m`).
    TimeUs(i64),
    /// Verbatim UTF-8 string literal.
    String(String),
}

/// Unary prefix operators (Precedence 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOp {
    Neg,
    Pos,
    Not,
    BitNot,
}

/// Binary operators ordered across precedence levels 3..=11 (`01` §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOp {
    // Precedence 3 — Multiplicative
    Mul,
    Div,
    Mod,
    // Precedence 4 — Additive
    Add,
    Sub,
    // Precedence 5 — Bitwise shift
    Shl,
    Shr,
    // Precedence 6 — Relational (non-chaining!)
    Lt,
    Le,
    Gt,
    Ge,
    // Precedence 7 — Equality (non-chaining, strictly lower than relational!)
    Eq,
    Ne,
    // Precedence 8 — Bitwise
    BitAnd,
    BitXor,
    BitOr,
    // Precedence 9..11 — Logical
    And,
    Xor,
    Or,
}

impl BinaryOp {
    /// Returns `true` for non-chaining relational operators (`<`, `<=`, `>`, `>=`).
    #[must_use]
    pub const fn is_relational(self) -> bool {
        matches!(self, Self::Lt | Self::Le | Self::Gt | Self::Ge)
    }

    /// Returns `true` for non-chaining equality operators (`==`, `!=`).
    #[must_use]
    pub const fn is_equality(self) -> bool {
        matches!(self, Self::Eq | Self::Ne)
    }
}

/// Positional or IEC named argument (`IN := expr`) inside a function call.
#[derive(Debug, Clone, PartialEq)]
pub struct CallArg {
    pub name: Option<String>,
    pub value: Expr,
    pub span: Span,
}

/// Expression kind (one canonical typed representation in memory).
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Literal(Literal),
    Ident(String),
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Field {
        base: Box<Expr>,
        field: String,
    },
    Call {
        callee: String,
        args: Vec<CallArg>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Expression-level foreign construct (`FOREIGN("lang", "text")`, `01` §6.2).
    Foreign {
        language: String,
        text: String,
    },
}

/// Spanned canonical ASL v2 expression node.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    /// Creates a spanned expression node.
    #[must_use]
    pub const fn new(kind: ExprKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// Recursively strips byte spans to `Span::default()` for structural equality checks.
    #[must_use]
    pub fn strip_spans(&self) -> Self {
        let kind = match &self.kind {
            ExprKind::Literal(lit) => ExprKind::Literal(lit.clone()),
            ExprKind::Ident(name) => ExprKind::Ident(name.clone()),
            ExprKind::Index { base, index } => ExprKind::Index {
                base: Box::new(base.strip_spans()),
                index: Box::new(index.strip_spans()),
            },
            ExprKind::Field { base, field } => ExprKind::Field {
                base: Box::new(base.strip_spans()),
                field: field.clone(),
            },
            ExprKind::Call { callee, args } => ExprKind::Call {
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|a| CallArg {
                        name: a.name.clone(),
                        value: a.value.strip_spans(),
                        span: Span::default(),
                    })
                    .collect(),
            },
            ExprKind::Unary { op, operand } => ExprKind::Unary {
                op: *op,
                operand: Box::new(operand.strip_spans()),
            },
            ExprKind::Binary { op, left, right } => ExprKind::Binary {
                op: *op,
                left: Box::new(left.strip_spans()),
                right: Box::new(right.strip_spans()),
            },
            ExprKind::Foreign { language, text } => ExprKind::Foreign {
                language: language.clone(),
                text: text.clone(),
            },
        };
        Self {
            kind,
            span: Span::default(),
        }
    }
}

/// Assignable target location (`Var`, `Index`, or `Field`).
#[derive(Debug, Clone, PartialEq)]
pub enum LValue {
    Var {
        name: String,
        span: Span,
    },
    Index {
        base: Box<LValue>,
        index: Box<Expr>,
        span: Span,
    },
    Field {
        base: Box<LValue>,
        field: String,
        span: Span,
    },
}

impl LValue {
    /// Returns the outermost source span of this `LValue`.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Var { span, .. } | Self::Index { span, .. } | Self::Field { span, .. } => *span,
        }
    }

    /// Returns the root variable/hardware identifier name at the base of this `LValue`.
    #[must_use]
    pub fn root_name(&self) -> &str {
        match self {
            Self::Var { name, .. } => name,
            Self::Index { base, .. } | Self::Field { base, .. } => base.root_name(),
        }
    }

    /// Recursively strips spans for structural equality checks.
    #[must_use]
    pub fn strip_spans(&self) -> Self {
        match self {
            Self::Var { name, .. } => Self::Var {
                name: name.clone(),
                span: Span::default(),
            },
            Self::Index { base, index, .. } => Self::Index {
                base: Box::new(base.strip_spans()),
                index: Box::new(index.strip_spans()),
                span: Span::default(),
            },
            Self::Field { base, field, .. } => Self::Field {
                base: Box::new(base.strip_spans()),
                field: field.clone(),
                span: Span::default(),
            },
        }
    }
}
