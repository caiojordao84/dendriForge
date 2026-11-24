//! Internal lightweight C++ syntax tree produced by [`crate::parser`] and lowered to
//! canonical ASL v2 by [`crate::sema`].

use dendri_ast::{BinaryOp, ForeignFidelity, UnaryOp};
use dendri_span::Span;

/// Parsed C++ translation unit before hardware reconciliation and ASL lowering.
#[derive(Debug, Clone)]
pub(crate) struct CppUnit {
    pub items: Vec<CppTopItem>,
}

/// Parsed C++ type specifier prior to board-context resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CppTypeSpec {
    pub is_const: bool,
    pub is_pointer: bool,
    pub is_ref: bool,
    pub words: Vec<String>,
    pub span: Span,
}

/// Top-level declaration or directive in a C++ translation unit.
#[derive(Debug, Clone)]
pub(crate) enum CppTopItem {
    Include {
        header: String,
        is_angle: bool,
        raw: String,
        span: Span,
    },
    DefineConst {
        name: String,
        expr: CppExpr,
        span: Span,
    },
    Struct {
        name: String,
        fields: Vec<CppFieldDecl>,
        span: Span,
    },
    Enum {
        name: String,
        backing: Option<CppTypeSpec>,
        variants: Vec<CppEnumVariant>,
        span: Span,
    },
    GlobalVar {
        is_const: bool,
        is_constexpr: bool,
        ty: CppTypeSpec,
        name: String,
        array_len: Option<CppExpr>,
        initial: Option<CppExpr>,
        span: Span,
    },
    FunctionProto {
        name: String,
    },
    Function {
        return_ty: CppTypeSpec,
        name: String,
        params: Vec<CppParam>,
        body: Vec<CppStmt>,
        body_span: Span,
        span: Span,
    },
    ForeignDecl {
        name: Option<String>,
        fidelity: ForeignFidelity,
        text: String,
        pack_hint: Option<String>,
        /// If this foreign decl declares a global instance `ClassName varName;`, records `(varName, ClassName)`.
        instance_binding: Option<(String, String)>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct CppFieldDecl {
    pub ty: CppTypeSpec,
    pub name: String,
    pub array_len: Option<CppExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub(crate) struct CppEnumVariant {
    pub name: String,
    pub value: Option<CppExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub(crate) struct CppParam {
    pub ty: CppTypeSpec,
    pub name: String,
    pub array_len: Option<CppExpr>,
    pub span: Span,
}

/// Compound assignment operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompoundOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

impl CompoundOp {
    #[must_use]
    pub(crate) const fn to_binary_op(self) -> BinaryOp {
        match self {
            Self::Add => BinaryOp::Add,
            Self::Sub => BinaryOp::Sub,
            Self::Mul => BinaryOp::Mul,
            Self::Div => BinaryOp::Div,
            Self::Mod => BinaryOp::Mod,
            Self::BitAnd => BinaryOp::BitAnd,
            Self::BitOr => BinaryOp::BitOr,
            Self::BitXor => BinaryOp::BitXor,
            Self::Shl => BinaryOp::Shl,
            Self::Shr => BinaryOp::Shr,
        }
    }
}

/// C++ statement prior to lowering.
#[derive(Debug, Clone)]
pub(crate) struct CppStmt {
    pub kind: CppStmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub(crate) enum CppStmtKind {
    LocalVar {
        is_static: bool,
        is_const: bool,
        is_auto: bool,
        ty: Option<CppTypeSpec>,
        name: String,
        array_len: Option<CppExpr>,
        initial: Option<CppExpr>,
    },
    Block {
        stmts: Vec<CppStmt>,
    },
    Assign {
        target: CppExpr,
        op: Option<CompoundOp>,
        value: CppExpr,
    },
    IncDec {
        target: CppExpr,
        is_increment: bool,
    },
    ExprStmt {
        expr: CppExpr,
    },
    If {
        condition: CppExpr,
        then_body: Vec<CppStmt>,
        elif_arms: Vec<(CppExpr, Vec<CppStmt>)>,
        else_body: Vec<CppStmt>,
    },
    Switch {
        scrutinee: CppExpr,
        cases: Vec<CppSwitchCase>,
        default: Option<Vec<CppStmt>>,
    },
    While {
        condition: CppExpr,
        body: Vec<CppStmt>,
    },
    DoWhile {
        body: Vec<CppStmt>,
        condition: CppExpr,
    },
    For {
        init: Option<Box<CppForInit>>,
        condition: Option<CppExpr>,
        step: Option<Box<CppForStep>>,
        body: Vec<CppStmt>,
    },
    Break,
    Continue,
    Return {
        value: Option<CppExpr>,
    },
    Foreign {
        text: String,
        pack_hint: Option<String>,
        receiver_class: Option<String>,
        method_name: Option<String>,
        call_args: Vec<CppExpr>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum CppForInit {
    Decl {
        ty: CppTypeSpec,
        name: String,
        init: CppExpr,
        span: Span,
    },
    Assign {
        name: String,
        init: CppExpr,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum CppForStep {
    Inc {
        name: String,
    },
    Dec {
        name: String,
    },
    Compound {
        name: String,
        op: CompoundOp,
        rhs: CppExpr,
    },
    Assign {
        name: String,
        rhs: CppExpr,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct CppSwitchCase {
    pub values: Vec<CppExpr>,
    pub body: Vec<CppStmt>,
}

/// C++ expression prior to lowering.
#[derive(Debug, Clone)]
pub(crate) struct CppExpr {
    pub kind: CppExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub(crate) enum CppExprKind {
    BoolLit(bool),
    IntLit {
        value: i128,
        unsigned: bool,
        long_count: u8,
    },
    FloatLit {
        value: f64,
        is_f32: bool,
    },
    CharLit(char),
    StringLit(String),
    Ident(String),
    ScopedIdent {
        scope: String,
        member: String,
    },
    Index {
        base: Box<CppExpr>,
        index: Box<CppExpr>,
    },
    Field {
        base: Box<CppExpr>,
        field: String,
    },
    Call {
        callee: String,
        args: Vec<CppExpr>,
    },
    MethodCall {
        receiver: Box<CppExpr>,
        method: String,
        args: Vec<CppExpr>,
    },
    /// Designated FB invoke: `fb({.IN = a, .PT = b})`
    DesignatedCall {
        callee: String,
        fields: Vec<(String, CppExpr)>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<CppExpr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<CppExpr>,
        right: Box<CppExpr>,
    },
    Ternary {
        condition: Box<CppExpr>,
        then_expr: Box<CppExpr>,
        else_expr: Box<CppExpr>,
    },
    Cast {
        target: CppTypeSpec,
        operand: Box<CppExpr>,
    },
    Foreign {
        text: String,
    },
}
