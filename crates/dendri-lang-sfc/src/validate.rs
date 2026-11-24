//! Static semantic validation for GRAFCET / SFC programs (`docs/lbc/sfc.md` §1.5).
//!
//! Enforces every frozen `ErrorCode` specified in `docs/lbc/sfc.md` §1.5:
//! - `ERR_DUPLICATE_DECL` / `ERR_DUPLICATE_ID`
//! - `ERR_UNDECLARED_SYMBOL` (transition referencing undeclared step)
//! - `ERR_TYPE_MISMATCH` (non-BOOL receptivity, missing/non-TIME `L`/`D` duration,
//!   duration on non-timed qualifier, non-BOOL coil target)
//! - `ERR_CONST_WRITE` (writes to `CONST` or read-only `<Step>.X` / `<Step>.T` / `<Macro>.DONE`)
//! - `ERR_HARDWARE_LABEL_WRITE` (coil target bound to read-only input hardware tag)
//! - `ERR_UNKNOWN_FIELD_ACCESS` (invalid attribute on step, or `.DONE` on non-macro step)
//! - `ERR_UNRESOLVED_FUNCTION` (macro-step referencing nonexistent sub-chart)
//! - `ERR_RECURSION_DEPTH` (cyclic macro-chart hierarchy)
//! - `ERR_ISR_FORBIDDEN_OP` (blocking `WAIT` inside `OB_HW_INT` or `OB_CYCLIC_INT`)
//! - `ERR_COMMENT_FIELD` / `ERR_COMMENT_ANCHOR` (bundle comment invariants on emit)

use dendri_ast::{
    ActionQualifier, BinaryOp, BoardProgram, CasePattern, Comment, CommentAnchor,
    CommentPlacement as AstCommentPlacement, Expr, ExprKind, HardwareBinding, HardwareKind, LValue,
    LadderCell, LadderCellKind, Literal, NodeId, ObKind, Pou, PouBody, PouKind, ScalarType,
    SfcChart, Stmt, StmtKind, StorageScope, TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::traits::{BoardContext, CommentPlacement};
use dendri_span::{Diagnostic, ErrorCode, Span};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
enum SymInfo {
    Data { ty: TypeRef, scope: StorageScope },
    Hardware { kind: HardwareKind, ty: TypeRef },
    Step { is_macro: bool },
}

/// Validates a parsed [`BoardProgram`] against `docs/lbc/sfc.md` §1.5 rules.
pub(crate) fn validate_parsed_program(
    program: &BoardProgram,
    ctx: &BoardContext,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mut globals: BTreeMap<String, SymInfo> = BTreeMap::new();

    // 1. Hardware bindings (duplicate label check + symbol table)
    let mut seen_hw = BTreeSet::new();
    for hw in &program.hardware {
        if !seen_hw.insert(hw.label.clone()) {
            push_err(
                &mut diags,
                &ctx.board_id,
                ErrorCode::ErrDuplicateDecl,
                hw.span,
                format!("duplicate hardware tag `{}`", hw.label),
            );
        }
        globals.insert(
            hw.label.clone(),
            SymInfo::Hardware {
                kind: hw.kind,
                ty: hw_type_ref(hw),
            },
        );
    }

    // 2. Global data declarations (duplicate check + symbol table)
    let mut seen_data = BTreeSet::new();
    for d in &program.data {
        if !seen_data.insert(d.name.clone()) {
            push_err(
                &mut diags,
                &ctx.board_id,
                ErrorCode::ErrDuplicateDecl,
                d.span,
                format!("duplicate global variable `{}`", d.name),
            );
        }
        globals.insert(
            d.name.clone(),
            SymInfo::Data {
                ty: d.ty.clone(),
                scope: d.scope,
            },
        );
    }

    // 3. Collect all POUs (top-level + embedded modules) and identify SFC charts
    let mut all_pous: Vec<&Pou> = Vec::new();
    all_pous.extend(program.pous.iter());
    for m in &program.modules {
        all_pous.extend(m.pous.iter());
    }

    let mut seen_pous = BTreeSet::new();
    let mut chart_names = BTreeSet::new();
    let mut macro_edges: BTreeMap<String, Vec<(String, Span)>> = BTreeMap::new();

    for pou in &all_pous {
        if !seen_pous.insert(pou.name.clone()) {
            push_err(
                &mut diags,
                &ctx.board_id,
                ErrorCode::ErrDuplicateDecl,
                pou.span,
                format!("duplicate POU declaration `{}`", pou.name),
            );
        }
        if let PouBody::Chart(chart) = &pou.body {
            chart_names.insert(pou.name.clone());
            let mut edges = Vec::new();
            for step in &chart.steps {
                if let Some(sub) = &step.macro_chart {
                    edges.push((sub.clone(), pou.span));
                }
            }
            macro_edges.insert(pou.name.clone(), edges);
        }
    }

    let has_external_import = program.imports.iter().any(|imp| !imp.starts_with("std."));

    // 4. Validate each POU
    for pou in &all_pous {
        let mut scope = globals.clone();
        let mut seen_locals = BTreeSet::new();
        for p in &pou.params {
            let param_scope = match p.direction {
                dendri_ast::ParamDirection::In => StorageScope::Const,
                dendri_ast::ParamDirection::InOut | dendri_ast::ParamDirection::Out => {
                    StorageScope::Var
                }
            };
            scope.insert(
                p.name.clone(),
                SymInfo::Data {
                    ty: p.ty.clone(),
                    scope: param_scope,
                },
            );
        }
        for l in &pou.locals {
            if !seen_locals.insert(l.name.clone()) {
                push_err(
                    &mut diags,
                    &ctx.board_id,
                    ErrorCode::ErrDuplicateDecl,
                    l.span,
                    format!(
                        "duplicate local variable `{}` in POU `{}`",
                        l.name, pou.name
                    ),
                );
            }
            scope.insert(
                l.name.clone(),
                SymInfo::Data {
                    ty: l.ty.clone(),
                    scope: l.scope,
                },
            );
        }

        let is_isr = matches!(
            pou.kind,
            PouKind::Ob {
                ob: ObKind::HwInt { .. } | ObKind::CyclicInt { .. }
            }
        );

        match &pou.body {
            PouBody::Chart(chart) => {
                validate_chart(
                    pou,
                    chart,
                    is_isr,
                    &mut scope,
                    &chart_names,
                    has_external_import,
                    &ctx.board_id,
                    &mut diags,
                );
            }
            PouBody::Statements(stmts) => {
                validate_stmts(stmts, is_isr, &scope, &ctx.board_id, &mut diags);
            }
            PouBody::Network(_) => {}
        }
    }

    // 5. Macro-chart cycle detection (`ERR_RECURSION_DEPTH`)
    check_macro_cycles(&macro_edges, &ctx.board_id, &mut diags);

    diags
}

/// Validates bundle-level invariants prior to emission:
/// - Unique `NodeId`s across `program` (`ERR_DUPLICATE_ID`)
/// - Valid `Comment` fields (`ERR_COMMENT_FIELD`) and anchors (`ERR_COMMENT_ANCHOR`)
pub(crate) fn validate_bundle_for_emit(
    program: &BoardProgram,
    comments: &[Comment],
    ctx: &BoardContext,
) -> Result<(), Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let mut node_ids = BTreeSet::new();
    collect_program_node_ids(program, &mut node_ids, &ctx.board_id, &mut diags);

    for c in comments {
        if CommentPlacement::from_comment(c).is_err() {
            push_err(
                &mut diags,
                &ctx.board_id,
                ErrorCode::ErrCommentField,
                c.span,
                format!(
                    "comment `{}` violates placement/token_ordinal invariant (`token_ordinal` required iff placement is `inner`)",
                    c.id
                ),
            );
        }
        if !c.anchor.is_node()
            && !matches!(
                c.placement,
                AstCommentPlacement::Header | AstCommentPlacement::Body
            )
        {
            push_err(
                &mut diags,
                &ctx.board_id,
                ErrorCode::ErrCommentField,
                c.span,
                format!(
                    "non-node comment `{}` must use `header` or `body` placement",
                    c.id
                ),
            );
        }
        if let CommentAnchor::Node { id } = &c.anchor
            && !node_ids.contains(id)
        {
            push_err(
                &mut diags,
                &ctx.board_id,
                ErrorCode::ErrCommentAnchor,
                c.span,
                format!(
                    "comment `{}` references unresolved anchor NodeId `{id}`",
                    c.id
                ),
            );
        }
    }

    if diags.is_empty() { Ok(()) } else { Err(diags) }
}

#[allow(clippy::too_many_arguments)]
fn validate_chart(
    pou: &Pou,
    chart: &SfcChart,
    is_isr: bool,
    scope: &mut BTreeMap<String, SymInfo>,
    chart_names: &BTreeSet<String>,
    has_external_import: bool,
    board_id: &str,
    diags: &mut Vec<Diagnostic>,
) {
    // 1. Register steps and check duplicate step names + macro targets
    let mut step_names = BTreeSet::new();
    for step in &chart.steps {
        if !step_names.insert(step.name.clone()) {
            push_err(
                diags,
                board_id,
                ErrorCode::ErrDuplicateDecl,
                pou.span,
                format!(
                    "duplicate SFC step name `{}` in chart `{}`",
                    step.name, pou.name
                ),
            );
        }
        scope.insert(
            step.name.clone(),
            SymInfo::Step {
                is_macro: step.macro_chart.is_some(),
            },
        );
        if let Some(sub) = &step.macro_chart
            && !chart_names.contains(sub)
            && !has_external_import
        {
            push_err(
                diags,
                board_id,
                ErrorCode::ErrUnresolvedFunction,
                pou.span,
                format!(
                    "macro-step `{}` in chart `{}` references undeclared sub-chart `{sub}`",
                    step.name, pou.name
                ),
            );
        }
    }

    // 2. Validate step actions (Table 47 duration rules, coil target type & writability, body stmts)
    for step in &chart.steps {
        for action in &step.actions {
            let needs_duration =
                matches!(action.qualifier, ActionQualifier::L | ActionQualifier::D);
            match (needs_duration, &action.duration) {
                (true, None) => {
                    push_err(
                        diags,
                        board_id,
                        ErrorCode::ErrTypeMismatch,
                        pou.span,
                        format!(
                            "SFC qualifier `{:?}` on step `{}` requires a `TIME` duration",
                            action.qualifier, step.name
                        ),
                    );
                }
                (false, Some(dur)) => {
                    push_err(
                        diags,
                        board_id,
                        ErrorCode::ErrTypeMismatch,
                        dur.span,
                        format!(
                            "SFC qualifier `{:?}` on step `{}` forbids a duration operand",
                            action.qualifier, step.name
                        ),
                    );
                }
                (true, Some(dur)) => {
                    if let Some(ty) = infer_expr_type(dur, scope, board_id, diags)
                        && ty != TypeRef::scalar(ScalarType::Time)
                    {
                        push_err(
                            diags,
                            board_id,
                            ErrorCode::ErrTypeMismatch,
                            dur.span,
                            format!(
                                "SFC `{:?}` duration on step `{}` must be `TIME`, found `{ty}`",
                                action.qualifier, step.name
                            ),
                        );
                    }
                }
                (false, None) => {}
            }

            if let Some(target) = &action.target {
                check_lvalue_write(target, true, true, scope, board_id, diags);
            }

            validate_stmts(&action.body, is_isr, scope, board_id, diags);
        }
    }

    // 3. Validate transitions (duplicate names, step existence, BOOL receptivity)
    let mut tr_names = BTreeSet::new();
    for tr in &chart.transitions {
        if !tr_names.insert(tr.name.clone()) {
            push_err(
                diags,
                board_id,
                ErrorCode::ErrDuplicateDecl,
                tr.receptivity.span,
                format!(
                    "duplicate SFC transition name `{}` in chart `{}`",
                    tr.name, pou.name
                ),
            );
        }
        for s in &tr.from_steps {
            if !step_names.contains(s) {
                push_err(
                    diags,
                    board_id,
                    ErrorCode::ErrUndeclaredSymbol,
                    tr.receptivity.span,
                    format!(
                        "transition `{}` references undeclared `FROM` step `{s}` in chart `{}`",
                        tr.name, pou.name
                    ),
                );
            }
        }
        for s in &tr.to_steps {
            if !step_names.contains(s) {
                push_err(
                    diags,
                    board_id,
                    ErrorCode::ErrUndeclaredSymbol,
                    tr.receptivity.span,
                    format!(
                        "transition `{}` references undeclared `TO` step `{s}` in chart `{}`",
                        tr.name, pou.name
                    ),
                );
            }
        }
        if let Some(rty) = infer_expr_type(&tr.receptivity, scope, board_id, diags)
            && rty != TypeRef::scalar(ScalarType::Bool)
        {
            push_err(
                diags,
                board_id,
                ErrorCode::ErrTypeMismatch,
                tr.receptivity.span,
                format!(
                    "SFC transition `{}` receptivity must be `BOOL`, found `{rty}`",
                    tr.name
                ),
            );
        }
    }
}

fn validate_stmts(
    stmts: &[Stmt],
    is_isr: bool,
    scope: &BTreeMap<String, SymInfo>,
    board_id: &str,
    diags: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                check_lvalue_write(target, false, false, scope, board_id, diags);
                let _ = infer_expr_type(value, scope, board_id, diags);
            }
            StmtKind::Call { callee: _, args } => {
                for a in args {
                    let _ = infer_expr_type(&a.value, scope, board_id, diags);
                }
            }
            StmtKind::FbInvoke {
                instance: _,
                inputs,
            } => {
                for (_, e) in inputs {
                    let _ = infer_expr_type(e, scope, board_id, diags);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                if let Some(cty) = infer_expr_type(condition, scope, board_id, diags)
                    && cty != TypeRef::scalar(ScalarType::Bool)
                {
                    push_err(
                        diags,
                        board_id,
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("IF condition must be `BOOL`, found `{cty}`"),
                    );
                }
                validate_stmts(then_body, is_isr, scope, board_id, diags);
                for arm in elif_arms {
                    let _ = infer_expr_type(&arm.condition, scope, board_id, diags);
                    validate_stmts(&arm.body, is_isr, scope, board_id, diags);
                }
                validate_stmts(else_body, is_isr, scope, board_id, diags);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                let _ = infer_expr_type(scrutinee, scope, board_id, diags);
                for c in cases {
                    for pat in &c.patterns {
                        match pat {
                            CasePattern::Value(e) => {
                                let _ = infer_expr_type(e, scope, board_id, diags);
                            }
                            CasePattern::Range { start, end } => {
                                let _ = infer_expr_type(start, scope, board_id, diags);
                                let _ = infer_expr_type(end, scope, board_id, diags);
                            }
                        }
                    }
                    validate_stmts(&c.body, is_isr, scope, board_id, diags);
                }
                if let Some(d) = default {
                    validate_stmts(d, is_isr, scope, board_id, diags);
                }
            }
            StmtKind::While { condition, body } => {
                let _ = infer_expr_type(condition, scope, board_id, diags);
                validate_stmts(body, is_isr, scope, board_id, diags);
            }
            StmtKind::For {
                counter: _,
                from,
                to,
                step,
                inclusive: _,
                body,
            } => {
                let _ = infer_expr_type(from, scope, board_id, diags);
                let _ = infer_expr_type(to, scope, board_id, diags);
                let _ = infer_expr_type(step, scope, board_id, diags);
                validate_stmts(body, is_isr, scope, board_id, diags);
            }
            StmtKind::Wait { duration } => {
                if is_isr {
                    push_err(
                        diags,
                        board_id,
                        ErrorCode::ErrIsrForbiddenOp,
                        stmt.span,
                        "blocking `WAIT` is forbidden inside `OB_HW_INT` / `OB_CYCLIC_INT` interrupt handlers",
                    );
                }
                if let Some(dty) = infer_expr_type(duration, scope, board_id, diags)
                    && dty != TypeRef::scalar(ScalarType::Time)
                {
                    push_err(
                        diags,
                        board_id,
                        ErrorCode::ErrTypeMismatch,
                        duration.span,
                        format!("`WAIT` duration must be `TIME`, found `{dty}`"),
                    );
                }
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    let _ = infer_expr_type(v, scope, board_id, diags);
                }
            }
            StmtKind::Break | StmtKind::Continue | StmtKind::Foreign { .. } => {}
        }
    }
}

fn check_lvalue_write(
    target: &LValue,
    allow_output_coil_hw: bool,
    require_bool: bool,
    scope: &BTreeMap<String, SymInfo>,
    board_id: &str,
    diags: &mut Vec<Diagnostic>,
) {
    let root = target.root_name();
    let span = target.span();
    match scope.get(root) {
        Some(SymInfo::Step { .. }) => {
            push_err(
                diags,
                board_id,
                ErrorCode::ErrConstWrite,
                span,
                format!(
                    "cannot write to read-only SFC step attribute on `{root}` (`<Step>.X`, `<Step>.T`, `<Macro>.DONE` are read-only)"
                ),
            );
        }
        Some(SymInfo::Data {
            scope: StorageScope::Const,
            ..
        }) => {
            push_err(
                diags,
                board_id,
                ErrorCode::ErrConstWrite,
                span,
                format!("cannot write to read-only `CONSTANT` symbol `{root}`"),
            );
        }
        Some(SymInfo::Data { ty, .. }) => {
            if require_bool
                && matches!(target, LValue::Var { .. })
                && *ty != TypeRef::scalar(ScalarType::Bool)
            {
                push_err(
                    diags,
                    board_id,
                    ErrorCode::ErrTypeMismatch,
                    span,
                    format!("SFC action coil target `{root}` must be `BOOL`, found `{ty}`"),
                );
            }
        }
        Some(SymInfo::Hardware { kind, ty }) => {
            let can_write = allow_output_coil_hw
                && matches!(
                    kind,
                    HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional
                );
            if !can_write {
                push_err(
                    diags,
                    board_id,
                    ErrorCode::ErrHardwareLabelWrite,
                    span,
                    format!(
                        "cannot write directly to hardware tag `{root}` ({kind:?}); read-only input tags cannot be driven by coils or assignments"
                    ),
                );
            } else if require_bool && *ty != TypeRef::scalar(ScalarType::Bool) {
                push_err(
                    diags,
                    board_id,
                    ErrorCode::ErrTypeMismatch,
                    span,
                    format!("SFC action coil target `{root}` must be `BOOL`, found `{ty}`"),
                );
            }
        }
        None => {}
    }

    // Also inspect any index expressions inside the LValue
    if let LValue::Index { base, index, .. } = target {
        let _ = infer_expr_type(index, scope, board_id, diags);
        check_lvalue_index_exprs(base, scope, board_id, diags);
    } else if let LValue::Field { base, .. } = target {
        check_lvalue_index_exprs(base, scope, board_id, diags);
    }
}

fn check_lvalue_index_exprs(
    lv: &LValue,
    scope: &BTreeMap<String, SymInfo>,
    board_id: &str,
    diags: &mut Vec<Diagnostic>,
) {
    match lv {
        LValue::Var { .. } => {}
        LValue::Index { base, index, .. } => {
            let _ = infer_expr_type(index, scope, board_id, diags);
            check_lvalue_index_exprs(base, scope, board_id, diags);
        }
        LValue::Field { base, .. } => {
            check_lvalue_index_exprs(base, scope, board_id, diags);
        }
    }
}

fn infer_expr_type(
    expr: &Expr,
    scope: &BTreeMap<String, SymInfo>,
    board_id: &str,
    diags: &mut Vec<Diagnostic>,
) -> Option<TypeRef> {
    match &expr.kind {
        ExprKind::Literal(lit) => match lit {
            Literal::Bool(_) => Some(TypeRef::scalar(ScalarType::Bool)),
            Literal::Int(_) => Some(TypeRef::scalar(ScalarType::Dint)),
            Literal::Float(_) => Some(TypeRef::scalar(ScalarType::Lreal)),
            Literal::TimeUs(_) => Some(TypeRef::scalar(ScalarType::Time)),
            Literal::String(_) => Some(TypeRef::scalar(ScalarType::String { max_len: 255 })),
        },
        ExprKind::Ident(name) => match scope.get(name) {
            Some(SymInfo::Data { ty, .. } | SymInfo::Hardware { ty, .. }) => Some(ty.clone()),
            Some(SymInfo::Step { is_macro }) => Some(TypeRef::Named {
                name: if *is_macro {
                    "__SfcMacroStep".to_owned()
                } else {
                    "__SfcStep".to_owned()
                },
            }),
            None => None,
        },
        ExprKind::Field { base, field } => {
            if let ExprKind::Ident(step_name) = &base.kind
                && let Some(SymInfo::Step { is_macro }) = scope.get(step_name)
            {
                return match field.as_str() {
                    "X" => Some(TypeRef::scalar(ScalarType::Bool)),
                    "T" => Some(TypeRef::scalar(ScalarType::Time)),
                    "DONE" if *is_macro => Some(TypeRef::scalar(ScalarType::Bool)),
                    "DONE" => {
                        push_err(
                            diags,
                            board_id,
                            ErrorCode::ErrUnknownFieldAccess,
                            expr.span,
                            format!(
                                "step `{step_name}` is not a macro-step and does not expose `.DONE` (only `.X` and `.T`)"
                            ),
                        );
                        None
                    }
                    other => {
                        push_err(
                            diags,
                            board_id,
                            ErrorCode::ErrUnknownFieldAccess,
                            expr.span,
                            format!(
                                "unknown SFC step attribute `.{other}` on `{step_name}` (valid attributes: `.X`, `.T`{})",
                                if *is_macro { ", `.DONE`" } else { "" }
                            ),
                        );
                        None
                    }
                };
            }
            let _ = infer_expr_type(base, scope, board_id, diags);
            None
        }
        ExprKind::Index { base, index } => {
            let base_ty = infer_expr_type(base, scope, board_id, diags);
            let _ = infer_expr_type(index, scope, board_id, diags);
            if let Some(TypeRef::Array { elem, .. }) = base_ty {
                Some(*elem)
            } else {
                None
            }
        }
        ExprKind::Call { callee: _, args } => {
            for a in args {
                let _ = infer_expr_type(&a.value, scope, board_id, diags);
            }
            None
        }
        ExprKind::Unary { op, operand } => {
            let inner = infer_expr_type(operand, scope, board_id, diags);
            match op {
                UnaryOp::Not => Some(TypeRef::scalar(ScalarType::Bool)),
                UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot => inner,
            }
        }
        ExprKind::Binary { op, left, right } => {
            let lt = infer_expr_type(left, scope, board_id, diags);
            let rt = infer_expr_type(right, scope, board_id, diags);
            match op {
                BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::And
                | BinaryOp::Xor
                | BinaryOp::Or => Some(TypeRef::scalar(ScalarType::Bool)),
                BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Mod
                | BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Shl
                | BinaryOp::Shr
                | BinaryOp::BitAnd
                | BinaryOp::BitXor
                | BinaryOp::BitOr => lt.or(rt),
            }
        }
        ExprKind::Foreign { .. } => None,
    }
}

fn check_macro_cycles(
    edges: &BTreeMap<String, Vec<(String, Span)>>,
    board_id: &str,
    diags: &mut Vec<Diagnostic>,
) {
    let mut state: BTreeMap<&str, u8> = BTreeMap::new(); // 0=unvisited, 1=visiting, 2=done
    let mut reported = BTreeSet::new();

    fn dfs<'a>(
        node: &'a str,
        edges: &'a BTreeMap<String, Vec<(String, Span)>>,
        state: &mut BTreeMap<&'a str, u8>,
    ) -> Option<Span> {
        state.insert(node, 1);
        if let Some(neighbors) = edges.get(node) {
            for (next, span) in neighbors {
                match state.get(next.as_str()).copied().unwrap_or(0) {
                    1 => return Some(*span),
                    0 => {
                        if let Some(s) = dfs(next.as_str(), edges, state) {
                            return Some(s);
                        }
                    }
                    _ => {}
                }
            }
        }
        state.insert(node, 2);
        None
    }

    for chart_name in edges.keys() {
        if state.get(chart_name.as_str()).copied().unwrap_or(0) == 0
            && let Some(span) = dfs(chart_name.as_str(), edges, &mut state)
            && reported.insert(chart_name.as_str())
        {
            push_err(
                diags,
                board_id,
                ErrorCode::ErrRecursionDepth,
                span,
                format!("recursive macro-step cycle detected involving SFC chart `{chart_name}`"),
            );
        }
    }
}

fn hw_type_ref(hw: &HardwareBinding) -> TypeRef {
    match hw.kind {
        HardwareKind::DigitalIn
        | HardwareKind::DigitalOut
        | HardwareKind::DigitalBidirectional
        | HardwareKind::BusPin => TypeRef::scalar(ScalarType::Bool),
        HardwareKind::AnalogIn | HardwareKind::AnalogOut => match &hw.codec {
            Some(c) => TypeRef::scalar(ScalarType::Code {
                codec_id: c.clone(),
            }),
            None => TypeRef::scalar(ScalarType::Analog),
        },
        HardwareKind::PwmOut => TypeRef::scalar(ScalarType::Duty),
    }
}

fn collect_program_node_ids(
    program: &BoardProgram,
    seen: &mut BTreeSet<NodeId>,
    board_id: &str,
    diags: &mut Vec<Diagnostic>,
) {
    let mut record = |id: &NodeId, span: Span| {
        if !seen.insert(id.clone()) {
            push_err(
                diags,
                board_id,
                ErrorCode::ErrDuplicateId,
                span,
                format!("duplicate AST NodeId `{id}` in board program"),
            );
        }
    };

    for h in &program.hardware {
        record(&h.id, h.span);
    }
    for t in &program.types {
        match t {
            TypeDecl::Struct { id, span, .. }
            | TypeDecl::Enum { id, span, .. }
            | TypeDecl::Foreign { id, span, .. } => record(id, *span),
        }
    }
    for d in &program.data {
        record(&d.id, d.span);
    }
    for m in &program.modules {
        record(&m.id, m.span);
        for t in &m.types {
            match t {
                TypeDecl::Struct { id, span, .. }
                | TypeDecl::Enum { id, span, .. }
                | TypeDecl::Foreign { id, span, .. } => record(id, *span),
            }
        }
        for d in &m.data {
            record(&d.id, d.span);
        }
        for pou in &m.pous {
            collect_pou_node_ids(pou, &mut record);
        }
    }
    for pou in &program.pous {
        collect_pou_node_ids(pou, &mut record);
    }
}

fn collect_pou_node_ids(pou: &Pou, record: &mut impl FnMut(&NodeId, Span)) {
    record(&pou.id, pou.span);
    for l in &pou.locals {
        record(&l.id, l.span);
    }
    match &pou.body {
        PouBody::Statements(stmts) => collect_stmt_node_ids(stmts, record),
        PouBody::Network(net) => {
            for r in &net.rungs {
                record(&r.id, pou.span);
                collect_ladder_cell_ids(&r.cells, pou.span, record);
            }
        }
        PouBody::Chart(chart) => {
            for s in &chart.steps {
                record(&s.id, pou.span);
                for a in &s.actions {
                    record(&a.id, pou.span);
                    collect_stmt_node_ids(&a.body, record);
                }
            }
            for tr in &chart.transitions {
                record(&tr.id, tr.receptivity.span);
            }
        }
    }
}

fn collect_ladder_cell_ids(
    cells: &[LadderCell],
    span: Span,
    record: &mut impl FnMut(&NodeId, Span),
) {
    for c in cells {
        record(&c.id, span);
        if let LadderCellKind::ParallelOr { branches } = &c.kind {
            for br in branches {
                collect_ladder_cell_ids(br, span, record);
            }
        }
    }
}

fn collect_stmt_node_ids(stmts: &[Stmt], record: &mut impl FnMut(&NodeId, Span)) {
    for s in stmts {
        record(&s.id, s.span);
        match &s.kind {
            StmtKind::If {
                then_body,
                elif_arms,
                else_body,
                ..
            } => {
                collect_stmt_node_ids(then_body, record);
                for arm in elif_arms {
                    collect_stmt_node_ids(&arm.body, record);
                }
                collect_stmt_node_ids(else_body, record);
            }
            StmtKind::Switch { cases, default, .. } => {
                for c in cases {
                    collect_stmt_node_ids(&c.body, record);
                }
                if let Some(d) = default {
                    collect_stmt_node_ids(d, record);
                }
            }
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                collect_stmt_node_ids(body, record);
            }
            StmtKind::Assign { .. }
            | StmtKind::Call { .. }
            | StmtKind::FbInvoke { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Return { .. }
            | StmtKind::Wait { .. }
            | StmtKind::Foreign { .. } => {}
        }
    }
}

fn push_err(
    diags: &mut Vec<Diagnostic>,
    board_id: &str,
    code: ErrorCode,
    span: Span,
    msg: impl Into<String>,
) {
    let mut d = Diagnostic::new(code, msg).with_board_id(board_id);
    d.span = Some(span);
    diags.push(d);
}
