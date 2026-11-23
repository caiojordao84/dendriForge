//! Static typechecker, scope resolver, and statement/expression validator for `dendri-validate`.

use crate::profile_catalog::{BoardProfileMeta, DisplayCharset};
use dendri_ast::{
    ASL_ANALOG_FULL_SCALE, BinaryOp, CallArg, CasePattern, DataDecl, Expr, ExprKind, FbKind,
    HardwareBinding, HardwareKind, LValue, LadderBoxKind, LadderCell, LadderCellKind,
    LadderNetwork, Literal, ObKind, ParamDecl, Pou, PouBody, PouKind, PullMode, ScalarType,
    SfcChart, Stmt, StmtKind, StorageScope, TypeRef, UnaryOp,
};
use dendri_span::{Diagnostic, DiagnosticBag, ErrorCode, FileId, Span};
use dendri_stdlib::{StdCatalog, StdSymbol, StdTypeRef};
use std::collections::{BTreeMap, BTreeSet};

/// Resolved variable/hardware symbol entry inside a board or POU scope.
#[derive(Debug, Clone)]
pub(crate) enum SymbolEntry {
    Data {
        ty: TypeRef,
        scope: StorageScope,
    },
    Hardware {
        binding: HardwareBinding,
        ty: TypeRef,
    },
    Param {
        ty: TypeRef,
        writable: bool,
    },
    SfcStep,
    SfcMacroStep,
}

/// Board-level typing environment built before validating POU bodies.
pub(crate) struct BoardTypeEnv<'a> {
    pub board_id: &'a str,
    pub file_id: Option<FileId>,
    pub std_catalog: &'a StdCatalog,
    pub imports: Vec<String>,
    pub board_profile: Option<&'a BoardProfileMeta>,
    pub wired_ports: &'a BTreeSet<String>,
    pub display_charsets: &'a BTreeMap<String, DisplayCharset>,
    pub structs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    pub enums: BTreeMap<String, ScalarType>,
    pub foreign_types: BTreeSet<String>,
    pub user_fbs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    pub user_fcs: BTreeMap<String, (Vec<ParamDecl>, TypeRef)>,
    pub globals: BTreeMap<String, SymbolEntry>,
}

impl<'a> BoardTypeEnv<'a> {
    pub(crate) fn push_diag(
        &self,
        bag: &mut DiagnosticBag,
        code: ErrorCode,
        span: Span,
        msg: String,
    ) {
        let mut diag = Diagnostic::new(code, msg).with_board_id(self.board_id);
        if let Some(fid) = self.file_id {
            diag = diag.with_location(fid, span);
        } else {
            diag.span = Some(span);
        }
        bag.push(diag);
    }

    /// Validates that a [`TypeRef`] refers only to known types, FBs, and codecs (`ERR_UNKNOWN_TYPE`).
    pub(crate) fn check_type_ref(&self, ty: &TypeRef, span: Span, bag: &mut DiagnosticBag) {
        match ty {
            TypeRef::Void => {}
            TypeRef::Scalar { ty: scalar } => {
                if let ScalarType::Code { codec_id } = scalar
                    && let Some(bp) = self.board_profile
                    && !bp.codecs.contains_key(codec_id)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrUnknownType,
                        span,
                        format!("unknown codec `{codec_id}` in CODE<{codec_id}>"),
                    );
                }
            }
            TypeRef::Array { elem, len } => {
                if *len == 0 {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrArrayBoundsConst,
                        span,
                        "array length must be greater than zero".to_owned(),
                    );
                }
                self.check_type_ref(elem, span, bag);
            }
            TypeRef::Named { name } => {
                if !self.structs.contains_key(name)
                    && !self.enums.contains_key(name)
                    && !self.foreign_types.contains(name)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrUnknownType,
                        span,
                        format!("unknown type `{name}`"),
                    );
                }
            }
            TypeRef::Fb { fb_kind } => {
                if let FbKind::User { fb_name } = fb_kind
                    && !self.user_fbs.contains_key(fb_name)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrUnknownType,
                        span,
                        format!("unknown user Function Block `{fb_name}`"),
                    );
                }
            }
        }
    }

    /// Validates all data declarations in `decls` (types & initializers).
    pub(crate) fn validate_data_decls(
        &self,
        decls: &[DataDecl],
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) {
        for d in decls {
            self.check_type_ref(&d.ty, d.span, bag);
            if let Some(init) = &d.initial {
                self.check_duty_overflow(&d.ty, init, d.span, bag);
                if let Some(init_ty) = self.infer_expr(init, Some(&d.ty), locals, bag)
                    && !types_compatible(&d.ty, &init_ty)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        init.span,
                        format!(
                            "initializer for `{}` has type `{}`, expected `{}`",
                            d.name, init_ty, d.ty
                        ),
                    );
                }
            }
        }
    }

    /// Validates a single POU (locals, control flow, ISR safety, expressions, Ladder, SFC).
    pub(crate) fn validate_pou(&self, pou: &Pou, bag: &mut DiagnosticBag) {
        let mut locals = BTreeMap::new();
        for p in &pou.params {
            self.check_type_ref(&p.ty, pou.span, bag);
            locals.insert(
                p.name.clone(),
                SymbolEntry::Param {
                    ty: p.ty.clone(),
                    writable: !matches!(p.direction, dendri_ast::ParamDirection::In),
                },
            );
        }
        if let PouKind::Fc { return_type } = &pou.kind {
            self.check_type_ref(return_type, pou.span, bag);
        }
        for l in &pou.locals {
            if locals.contains_key(&l.name) {
                self.push_diag(
                    bag,
                    ErrorCode::ErrDuplicateDecl,
                    l.span,
                    format!(
                        "duplicate local declaration `{}` in POU `{}`",
                        l.name, pou.name
                    ),
                );
            }
            locals.insert(
                l.name.clone(),
                SymbolEntry::Data {
                    ty: l.ty.clone(),
                    scope: l.scope,
                },
            );
        }
        self.validate_data_decls(&pou.locals, &locals, bag);

        // If this POU is an SFC chart, register step names (`<Step>.X`, `<Step>.T`, `<Macro>.DONE`)
        if let PouBody::Chart(chart) = &pou.body {
            for step in &chart.steps {
                let entry = if step.macro_chart.is_some() {
                    SymbolEntry::SfcMacroStep
                } else {
                    SymbolEntry::SfcStep
                };
                locals.insert(step.name.clone(), entry);
            }
        }

        let in_isr = matches!(
            &pou.kind,
            PouKind::Ob {
                ob: ObKind::HwInt { .. } | ObKind::CyclicInt { .. }
            }
        );
        let expected_return = match &pou.kind {
            PouKind::Fc { return_type } => return_type.clone(),
            PouKind::Ob { .. } | PouKind::Fb => TypeRef::Void,
        };

        let ctx = StmtCheckCtx {
            in_loop: false,
            in_isr,
            expected_return: &expected_return,
            pou_name: &pou.name,
        };

        match &pou.body {
            PouBody::Statements(stmts) => {
                self.validate_stmts(stmts, &ctx, &locals, bag);
            }
            PouBody::Network(net) => {
                self.validate_ladder(net, &locals, bag);
            }
            PouBody::Chart(chart) => {
                self.validate_chart(chart, &ctx, &locals, bag);
            }
        }
    }

    fn validate_stmts(
        &self,
        stmts: &[Stmt],
        ctx: &StmtCheckCtx<'_>,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) {
        for stmt in stmts {
            self.validate_stmt(stmt, ctx, locals, bag);
        }
    }

    fn validate_stmt(
        &self,
        stmt: &Stmt,
        ctx: &StmtCheckCtx<'_>,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) {
        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                if let Some(target_ty) =
                    self.check_lvalue_writable(target, stmt.span, locals, false, bag)
                {
                    self.check_duty_overflow(&target_ty, value, stmt.span, bag);
                    if let Some(val_ty) = self.infer_expr(value, Some(&target_ty), locals, bag)
                        && !types_compatible(&target_ty, &val_ty)
                    {
                        self.push_diag(
                            bag,
                            ErrorCode::ErrTypeMismatch,
                            stmt.span,
                            format!("cannot assign `{val_ty}` to target of type `{target_ty}`"),
                        );
                    }
                } else {
                    // Still check RHS for secondary errors
                    let _ = self.infer_expr(value, None, locals, bag);
                }
            }
            StmtKind::Call { callee, args } => {
                if ctx.in_isr && matches!(callee.as_str(), "WAIT_TIME" | "WAIT_TIME_US") {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrIsrForbiddenOp,
                        stmt.span,
                        format!(
                            "blocking call `{callee}` is forbidden inside interrupt OB `{}`",
                            ctx.pou_name
                        ),
                    );
                }
                let _ = self.check_call(callee, args, stmt.span, true, locals, bag);
            }
            StmtKind::FbInvoke { instance, inputs } => {
                let Some(inst_ty) = self.infer_lvalue(instance, stmt.span, locals, bag) else {
                    return;
                };
                let fb_fields = self.fb_fields_for_type(&inst_ty);
                if fb_fields.is_empty() {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        stmt.span,
                        format!(
                            "`{}` of type `{inst_ty}` is not a Function Block",
                            instance.root_name()
                        ),
                    );
                    return;
                }
                for (name, expr) in inputs {
                    if let Some(field_ty) = fb_fields.get(name) {
                        if let Some(val_ty) = self.infer_expr(expr, Some(field_ty), locals, bag)
                            && !types_compatible(field_ty, &val_ty)
                        {
                            self.push_diag(
                                bag,
                                ErrorCode::ErrTypeMismatch,
                                expr.span,
                                format!("FB input `{name}` expects `{field_ty}`, found `{val_ty}`"),
                            );
                        }
                    } else {
                        self.push_diag(
                            bag,
                            ErrorCode::ErrUnknownFieldAccess,
                            stmt.span,
                            format!("Function Block `{inst_ty}` has no input field `{name}`"),
                        );
                    }
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                if let Some(cty) = self.infer_expr(condition, Some(&bool_ty), locals, bag)
                    && !types_compatible(&bool_ty, &cty)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("`if` condition must be BOOL, found `{cty}`"),
                    );
                }
                self.validate_stmts(then_body, ctx, locals, bag);
                for arm in elif_arms {
                    if let Some(cty) = self.infer_expr(&arm.condition, Some(&bool_ty), locals, bag)
                        && !types_compatible(&bool_ty, &cty)
                    {
                        self.push_diag(
                            bag,
                            ErrorCode::ErrTypeMismatch,
                            arm.condition.span,
                            format!("`elif` condition must be BOOL, found `{cty}`"),
                        );
                    }
                    self.validate_stmts(&arm.body, ctx, locals, bag);
                }
                self.validate_stmts(else_body, ctx, locals, bag);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                let scrut_ty = self.infer_expr(scrutinee, None, locals, bag);
                for c in cases {
                    for pat in &c.patterns {
                        match pat {
                            CasePattern::Value(e) => {
                                if let Some(pty) =
                                    self.infer_expr(e, scrut_ty.as_ref(), locals, bag)
                                    && let Some(sty) = &scrut_ty
                                    && !types_compatible(sty, &pty)
                                {
                                    self.push_diag(
                                        bag,
                                        ErrorCode::ErrTypeMismatch,
                                        e.span,
                                        format!("switch case pattern `{pty}` does not match scrutinee `{sty}`"),
                                    );
                                }
                            }
                            CasePattern::Range { start, end } => {
                                let _ = self.infer_expr(start, scrut_ty.as_ref(), locals, bag);
                                let _ = self.infer_expr(end, scrut_ty.as_ref(), locals, bag);
                            }
                        }
                    }
                    self.validate_stmts(&c.body, ctx, locals, bag);
                }
                if let Some(def) = default {
                    self.validate_stmts(def, ctx, locals, bag);
                }
            }
            StmtKind::While { condition, body } => {
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                if let Some(cty) = self.infer_expr(condition, Some(&bool_ty), locals, bag)
                    && !types_compatible(&bool_ty, &cty)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("`while` condition must be BOOL, found `{cty}`"),
                    );
                }
                let loop_ctx = StmtCheckCtx {
                    in_loop: true,
                    ..*ctx
                };
                self.validate_stmts(body, &loop_ctx, locals, bag);
            }
            StmtKind::For {
                counter,
                from,
                to,
                step,
                body,
                ..
            } => {
                let counter_lv = LValue::Var {
                    name: counter.clone(),
                    span: stmt.span,
                };
                let counter_ty =
                    self.check_lvalue_writable(&counter_lv, stmt.span, locals, false, bag);
                for bound in [from, to, step] {
                    if let Some(bty) = self.infer_expr(bound, counter_ty.as_ref(), locals, bag)
                        && let Some(cty) = &counter_ty
                        && !types_compatible(cty, &bty)
                    {
                        self.push_diag(
                            bag,
                            ErrorCode::ErrTypeMismatch,
                            bound.span,
                            format!("`for` bound `{bty}` does not match counter type `{cty}`"),
                        );
                    }
                }
                let loop_ctx = StmtCheckCtx {
                    in_loop: true,
                    ..*ctx
                };
                self.validate_stmts(body, &loop_ctx, locals, bag);
            }
            StmtKind::Break => {
                if !ctx.in_loop {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrBreakOutsideLoop,
                        stmt.span,
                        "`break` statement is only valid inside a `while` or `for` loop".to_owned(),
                    );
                }
            }
            StmtKind::Continue => {
                if !ctx.in_loop {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrContinueOutsideLoop,
                        stmt.span,
                        "`continue` statement is only valid inside a `while` or `for` loop"
                            .to_owned(),
                    );
                }
            }
            StmtKind::Return { value } => match (ctx.expected_return, value) {
                (TypeRef::Void, None) => {}
                (TypeRef::Void, Some(expr)) => {
                    let _ = self.infer_expr(expr, None, locals, bag);
                    self.push_diag(
                        bag,
                        ErrorCode::ErrReturnTypeMismatch,
                        stmt.span,
                        format!(
                            "POU `{}` returns VOID and cannot return a value",
                            ctx.pou_name
                        ),
                    );
                }
                (expected, None) => {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrReturnTypeMismatch,
                        stmt.span,
                        format!(
                            "POU `{}` must return a value of type `{expected}`",
                            ctx.pou_name
                        ),
                    );
                }
                (expected, Some(expr)) => {
                    if let Some(actual) = self.infer_expr(expr, Some(expected), locals, bag)
                        && !types_compatible(expected, &actual)
                    {
                        self.push_diag(
                            bag,
                            ErrorCode::ErrReturnTypeMismatch,
                            stmt.span,
                            format!(
                                "return expression has type `{actual}`, expected `{expected}` in POU `{}`",
                                ctx.pou_name
                            ),
                        );
                    }
                }
            },
            StmtKind::Wait { duration } => {
                if ctx.in_isr {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrIsrForbiddenOp,
                        stmt.span,
                        format!(
                            "blocking `Wait` is forbidden inside interrupt OB `{}`",
                            ctx.pou_name
                        ),
                    );
                }
                let time_ty = TypeRef::scalar(ScalarType::Time);
                if let Some(dty) = self.infer_expr(duration, Some(&time_ty), locals, bag)
                    && !types_compatible(&time_ty, &dty)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        duration.span,
                        format!("`Wait` duration must be TIME, found `{dty}`"),
                    );
                }
            }
            StmtKind::Foreign { .. } => {
                // Valid at F0/F1/F2; cross-language export checked at board level
            }
        }
    }

    fn validate_ladder(
        &self,
        net: &LadderNetwork,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) {
        for rung in &net.rungs {
            for cell in &rung.cells {
                self.validate_ladder_cell(cell, locals, bag);
            }
        }
    }

    fn validate_ladder_cell(
        &self,
        cell: &LadderCell,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) {
        let bool_ty = TypeRef::scalar(ScalarType::Bool);
        match &cell.kind {
            LadderCellKind::Empty | LadderCellKind::Wire => {}
            LadderCellKind::Contact { target, .. } => {
                if let Some(ty) = self.infer_lvalue(target, target.span(), locals, bag)
                    && !types_compatible(&bool_ty, &ty)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        target.span(),
                        format!("Ladder contact target must be BOOL, found `{ty}`"),
                    );
                }
            }
            LadderCellKind::Coil { target, .. } => {
                // Coils MAY write to writable DigitalOut hardware tags as well as BOOL vars!
                if let Some(ty) =
                    self.check_lvalue_writable(target, target.span(), locals, true, bag)
                    && !types_compatible(&bool_ty, &ty)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        target.span(),
                        format!("Ladder coil target must be BOOL, found `{ty}`"),
                    );
                }
            }
            LadderCellKind::ParallelOr { branches } => {
                for br in branches {
                    for c in br {
                        self.validate_ladder_cell(c, locals, bag);
                    }
                }
            }
            LadderCellKind::Box(box_kind) => match box_kind {
                LadderBoxKind::Fb {
                    instance, inputs, ..
                } => {
                    let _ = self.infer_lvalue(instance, instance.span(), locals, bag);
                    for (_, expr) in inputs {
                        let _ = self.infer_expr(expr, None, locals, bag);
                    }
                }
                LadderBoxKind::Compare { left, right, .. } => {
                    let lt = self.infer_expr(left, None, locals, bag);
                    let rt = self.infer_expr(right, lt.as_ref(), locals, bag);
                    if let (Some(l), Some(r)) = (lt, rt)
                        && !types_compatible(&l, &r)
                    {
                        self.push_diag(
                            bag,
                            ErrorCode::ErrTypeMismatch,
                            left.span,
                            format!("Ladder compare box operand types `{l}` and `{r}` mismatch"),
                        );
                    }
                }
                LadderBoxKind::Call {
                    callee,
                    args,
                    output,
                } => {
                    let call_args: Vec<CallArg> = args
                        .iter()
                        .map(|(name, value)| CallArg {
                            name: name.clone(),
                            value: value.clone(),
                            span: value.span,
                        })
                        .collect();
                    let ret =
                        self.check_call(callee, &call_args, Span::default(), false, locals, bag);
                    if let Some(out_lv) = output
                        && let Some(out_ty) =
                            self.check_lvalue_writable(out_lv, out_lv.span(), locals, true, bag)
                        && let Some(ret_ty) = ret
                        && !types_compatible(&out_ty, &ret_ty)
                    {
                        self.push_diag(
                            bag,
                            ErrorCode::ErrTypeMismatch,
                            out_lv.span(),
                            format!(
                                "Ladder call output `{out_ty}` does not match return `{ret_ty}`"
                            ),
                        );
                    }
                }
                LadderBoxKind::Foreign { .. } => {}
            },
        }
    }

    fn validate_chart(
        &self,
        chart: &SfcChart,
        ctx: &StmtCheckCtx<'_>,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) {
        let bool_ty = TypeRef::scalar(ScalarType::Bool);
        let time_ty = TypeRef::scalar(ScalarType::Time);

        for step in &chart.steps {
            for action in &step.actions {
                if let Some(dur) = &action.duration
                    && let Some(dty) = self.infer_expr(dur, Some(&time_ty), locals, bag)
                    && !types_compatible(&time_ty, &dty)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        dur.span,
                        format!("SFC action duration must be TIME, found `{dty}`"),
                    );
                }
                if let Some(target) = &action.target
                    && let Some(tty) =
                        self.check_lvalue_writable(target, target.span(), locals, true, bag)
                    && !types_compatible(&bool_ty, &tty)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        target.span(),
                        format!("SFC coil action target must be BOOL, found `{tty}`"),
                    );
                }
                self.validate_stmts(&action.body, ctx, locals, bag);
            }
        }

        for tr in &chart.transitions {
            if let Some(rty) = self.infer_expr(&tr.receptivity, Some(&bool_ty), locals, bag)
                && !types_compatible(&bool_ty, &rty)
            {
                self.push_diag(
                    bag,
                    ErrorCode::ErrTypeMismatch,
                    tr.receptivity.span,
                    format!(
                        "SFC transition `{}` receptivity must be BOOL, found `{rty}`",
                        tr.name
                    ),
                );
            }
        }
    }

    fn lookup_symbol<'b>(
        &'b self,
        name: &str,
        locals: &'b BTreeMap<String, SymbolEntry>,
    ) -> Option<&'b SymbolEntry> {
        locals.get(name).or_else(|| self.globals.get(name))
    }

    fn check_lvalue_writable(
        &self,
        lvalue: &LValue,
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        allow_writable_hardware_coil: bool,
        bag: &mut DiagnosticBag,
    ) -> Option<TypeRef> {
        let root = lvalue.root_name();
        match self.lookup_symbol(root, locals) {
            Some(SymbolEntry::Data {
                scope: StorageScope::Const,
                ..
            }) => {
                self.push_diag(
                    bag,
                    ErrorCode::ErrConstWrite,
                    span,
                    format!("cannot write to CONST declaration `{root}`"),
                );
                return None;
            }
            Some(SymbolEntry::Param {
                writable: false, ..
            }) => {
                self.push_diag(
                    bag,
                    ErrorCode::ErrConstWrite,
                    span,
                    format!("cannot write to read-only IN parameter `{root}`"),
                );
                return None;
            }
            Some(SymbolEntry::Hardware { binding, .. }) => {
                let can_coil_write = allow_writable_hardware_coil
                    && matches!(
                        binding.kind,
                        HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional
                    );
                if !can_coil_write {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrHardwareLabelWrite,
                        span,
                        format!(
                            "hardware label `{root}` is an immutable symbolic pin binding; use I/O call or output coil"
                        ),
                    );
                    return None;
                }
            }
            Some(
                SymbolEntry::Data {
                    scope:
                        StorageScope::Var
                        | StorageScope::Retain
                        | StorageScope::Persistent
                        | StorageScope::Temp,
                    ..
                }
                | SymbolEntry::Param { writable: true, .. }
                | SymbolEntry::SfcStep
                | SymbolEntry::SfcMacroStep,
            ) => {}
            None => {}
        }
        self.infer_lvalue(lvalue, span, locals, bag)
    }

    fn infer_lvalue(
        &self,
        lvalue: &LValue,
        fallback_span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) -> Option<TypeRef> {
        match lvalue {
            LValue::Var { name, span } => {
                let sp = if span.is_empty() {
                    fallback_span
                } else {
                    *span
                };
                self.resolve_ident_type(name, sp, locals, bag)
            }
            LValue::Index { base, index, span } => {
                let sp = if span.is_empty() {
                    fallback_span
                } else {
                    *span
                };
                let base_ty = self.infer_lvalue(base, sp, locals, bag)?;
                self.check_index_op(&base_ty, index, sp, locals, bag)
            }
            LValue::Field { base, field, span } => {
                let sp = if span.is_empty() {
                    fallback_span
                } else {
                    *span
                };
                let base_ty = self.infer_lvalue(base, sp, locals, bag)?;
                self.check_field_access(&base_ty, field, sp, bag)
            }
        }
    }

    fn resolve_ident_type(
        &self,
        name: &str,
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) -> Option<TypeRef> {
        match self.lookup_symbol(name, locals) {
            Some(SymbolEntry::Data { ty, .. } | SymbolEntry::Param { ty, .. }) => Some(ty.clone()),
            Some(SymbolEntry::Hardware { binding, ty }) => {
                // Static check for floating DigitalIn without internal pull (`WARN_INDETERMINATE_LEVEL`)
                if binding.kind == HardwareKind::DigitalIn
                    && binding.pull == PullMode::None
                    && !self.wired_ports.contains(&binding.pin)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::WarnIndeterminateLevel,
                        span,
                        format!(
                            "digital input `{}` on pin `{}` has pull=none and no connected wire (electrically indeterminate level)",
                            binding.label, binding.pin
                        ),
                    );
                }
                Some(ty.clone())
            }
            Some(SymbolEntry::SfcStep) => Some(TypeRef::Named {
                name: "__SfcStep".to_owned(),
            }),
            Some(SymbolEntry::SfcMacroStep) => Some(TypeRef::Named {
                name: "__SfcMacroStep".to_owned(),
            }),
            None => {
                self.push_diag(
                    bag,
                    ErrorCode::ErrUndeclaredSymbol,
                    span,
                    format!("undeclared symbol `{name}`"),
                );
                None
            }
        }
    }

    pub(crate) fn infer_expr(
        &self,
        expr: &Expr,
        expected: Option<&TypeRef>,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) -> Option<TypeRef> {
        match &expr.kind {
            ExprKind::Literal(lit) => match lit {
                Literal::Bool(_) => Some(TypeRef::scalar(ScalarType::Bool)),
                Literal::Int(_) => {
                    if let Some(exp) = expected
                        && is_int_like_target(exp)
                    {
                        return Some(exp.clone());
                    }
                    Some(TypeRef::scalar(ScalarType::Dint))
                }
                Literal::Float(_) => {
                    if let Some(
                        exp @ TypeRef::Scalar {
                            ty: ScalarType::Real | ScalarType::Lreal,
                        },
                    ) = expected
                    {
                        return Some(exp.clone());
                    }
                    Some(TypeRef::scalar(ScalarType::Lreal))
                }
                Literal::TimeUs(_) => Some(TypeRef::scalar(ScalarType::Time)),
                Literal::String(_) => Some(TypeRef::scalar(ScalarType::String { max_len: 255 })),
            },
            ExprKind::Ident(name) => self.resolve_ident_type(name, expr.span, locals, bag),
            ExprKind::Index { base, index } => {
                let base_ty = self.infer_expr(base, None, locals, bag)?;
                self.check_index_op(&base_ty, index, expr.span, locals, bag)
            }
            ExprKind::Field { base, field } => {
                let base_ty = self.infer_expr(base, None, locals, bag)?;
                self.check_field_access(&base_ty, field, expr.span, bag)
            }
            ExprKind::Call { callee, args } => {
                self.check_call(callee, args, expr.span, false, locals, bag)
            }
            ExprKind::Unary { op, operand } => {
                let op_ty = self.infer_expr(operand, expected, locals, bag)?;
                match op {
                    UnaryOp::Not => {
                        let bool_ty = TypeRef::scalar(ScalarType::Bool);
                        if !types_compatible(&bool_ty, &op_ty) {
                            self.push_diag(
                                bag,
                                ErrorCode::ErrTypeMismatch,
                                expr.span,
                                format!("`NOT` requires BOOL operand, found `{op_ty}`"),
                            );
                        }
                        Some(bool_ty)
                    }
                    UnaryOp::Neg | UnaryOp::Pos => {
                        if let TypeRef::Scalar { ty } = &op_ty
                            && (ty.is_signed_int()
                                || ty.is_float()
                                || matches!(ty, ScalarType::Analog))
                        {
                            return Some(op_ty);
                        }
                        self.push_diag(
                            bag,
                            ErrorCode::ErrTypeMismatch,
                            expr.span,
                            format!(
                                "unary +/- requires signed numeric or ANALOG type, found `{op_ty}`"
                            ),
                        );
                        None
                    }
                    UnaryOp::BitNot => {
                        if let TypeRef::Scalar { ty } = &op_ty
                            && (ty.is_bitstring() || ty.is_unsigned_int() || ty.is_signed_int())
                        {
                            return Some(op_ty);
                        }
                        self.push_diag(
                            bag,
                            ErrorCode::ErrTypeMismatch,
                            expr.span,
                            format!(
                                "`BIT_NOT` requires integer/bitstring operand, found `{op_ty}`"
                            ),
                        );
                        None
                    }
                }
            }
            ExprKind::Binary { op, left, right } => {
                self.check_binary_op(*op, left, right, expected, expr.span, locals, bag)
            }
            ExprKind::Foreign { text, .. } => {
                if let Some(exp) = expected {
                    Some(exp.clone())
                } else {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrForeignUntyped,
                        expr.span,
                        format!(
                            "foreign expression `{text}` has no expected type from context or F1 pack"
                        ),
                    );
                    None
                }
            }
        }
    }

    fn check_index_op(
        &self,
        base_ty: &TypeRef,
        index: &Expr,
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) -> Option<TypeRef> {
        let TypeRef::Array { elem, len } = base_ty else {
            self.push_diag(
                bag,
                ErrorCode::ErrTypeMismatch,
                span,
                format!("cannot index into non-array type `{base_ty}`"),
            );
            return None;
        };

        let idx_ty = self.infer_expr(index, None, locals, bag);
        if let Some(ty) = &idx_ty {
            let is_int =
                matches!(ty, TypeRef::Scalar { ty: s } if s.is_signed_int() || s.is_unsigned_int());
            if !is_int {
                self.push_diag(
                    bag,
                    ErrorCode::ErrTypeMismatch,
                    index.span,
                    format!("array index must be an integer, found `{ty}`"),
                );
            }
        }

        if let Some(const_idx) = eval_const_int(index)
            && (const_idx < 0 || const_idx >= i128::from(*len))
        {
            self.push_diag(
                bag,
                ErrorCode::ErrArrayBoundsConst,
                index.span,
                format!("constant array index {const_idx} out of bounds for ARRAY[{len}]"),
            );
        }

        Some((**elem).clone())
    }

    fn check_field_access(
        &self,
        base_ty: &TypeRef,
        field: &str,
        span: Span,
        bag: &mut DiagnosticBag,
    ) -> Option<TypeRef> {
        match base_ty {
            TypeRef::Named { name } if name == "__SfcStep" => match field {
                "X" => Some(TypeRef::scalar(ScalarType::Bool)),
                "T" => Some(TypeRef::scalar(ScalarType::Time)),
                _ => {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrUnknownFieldAccess,
                        span,
                        format!("SFC step has no field `.{field}` (expected `.X` or `.T`)"),
                    );
                    None
                }
            },
            TypeRef::Named { name } if name == "__SfcMacroStep" => match field {
                "X" | "DONE" => Some(TypeRef::scalar(ScalarType::Bool)),
                "T" => Some(TypeRef::scalar(ScalarType::Time)),
                _ => {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrUnknownFieldAccess,
                        span,
                        format!(
                            "SFC macro-step has no field `.{field}` (expected `.X`, `.T`, or `.DONE`)"
                        ),
                    );
                    None
                }
            },
            TypeRef::Named { name } => {
                if let Some(fields) = self.structs.get(name)
                    && let Some(ft) = fields.get(field)
                {
                    return Some(ft.clone());
                }
                self.push_diag(
                    bag,
                    ErrorCode::ErrUnknownFieldAccess,
                    span,
                    format!("struct `{name}` has no field `.{field}`"),
                );
                None
            }
            TypeRef::Fb { .. } => {
                let fields = self.fb_fields_for_type(base_ty);
                if let Some(ft) = fields.get(field) {
                    Some(ft.clone())
                } else {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrUnknownFieldAccess,
                        span,
                        format!("Function Block `{base_ty}` has no field `.{field}`"),
                    );
                    None
                }
            }
            TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Array { .. } => {
                self.push_diag(
                    bag,
                    ErrorCode::ErrUnknownFieldAccess,
                    span,
                    format!("type `{base_ty}` does not support field access `.{field}`"),
                );
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_binary_op(
        &self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        expected: Option<&TypeRef>,
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) -> Option<TypeRef> {
        let bool_ty = TypeRef::scalar(ScalarType::Bool);
        match op {
            BinaryOp::And | BinaryOp::Xor | BinaryOp::Or => {
                let lt = self.infer_expr(left, Some(&bool_ty), locals, bag);
                let rt = self.infer_expr(right, Some(&bool_ty), locals, bag);
                if let Some(l) = lt
                    && !types_compatible(&bool_ty, &l)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        left.span,
                        format!("logical operator requires BOOL, found `{l}`"),
                    );
                }
                if let Some(r) = rt
                    && !types_compatible(&bool_ty, &r)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        right.span,
                        format!("logical operator requires BOOL, found `{r}`"),
                    );
                }
                Some(bool_ty)
            }
            BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::Eq
            | BinaryOp::Ne => {
                let lt = self.infer_expr(left, None, locals, bag)?;
                let rt = self.infer_expr(right, Some(&lt), locals, bag)?;
                if !types_compatible(&lt, &rt) {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        span,
                        format!("cannot compare incompatible types `{lt}` and `{rt}`"),
                    );
                }
                Some(bool_ty)
            }
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                let lt = self.infer_expr(left, expected, locals, bag)?;
                let rt = self.infer_expr(right, Some(&lt), locals, bag)?;
                // Strictly forbid mixing ANALOG/DUTY/CODE with plain integers (`01` §3.5)
                let plain_ok = matches!(&lt, TypeRef::Scalar { ty } if ty.is_plain_numeric())
                    || matches!(
                        (&lt, op),
                        (
                            TypeRef::Scalar {
                                ty: ScalarType::Time
                            },
                            BinaryOp::Add | BinaryOp::Sub
                        )
                    );
                if !plain_ok || !types_compatible(&lt, &rt) {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        span,
                        format!("invalid arithmetic operands `{lt}` and `{rt}`"),
                    );
                    return None;
                }
                Some(lt)
            }
            BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr => {
                let lt = self.infer_expr(left, expected, locals, bag)?;
                let rt = self.infer_expr(right, Some(&lt), locals, bag)?;
                let bit_ok = matches!(&lt, TypeRef::Scalar { ty } if ty.is_bitstring() || ty.is_unsigned_int() || ty.is_signed_int());
                if !bit_ok || !types_compatible(&lt, &rt) {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        span,
                        format!("invalid bitwise operands `{lt}` and `{rt}`"),
                    );
                    return None;
                }
                Some(lt)
            }
        }
    }

    fn check_call(
        &self,
        callee: &str,
        args: &[CallArg],
        span: Span,
        _is_stmt: bool,
        locals: &BTreeMap<String, SymbolEntry>,
        bag: &mut DiagnosticBag,
    ) -> Option<TypeRef> {
        // Special display sink built-in (`DISPLAY_PRINT(component_id_str, text_str)`):
        // Enforces `ERR_STRING_CHARSET` ONLY on display components declaring a charset (`01` §7, `DEF-007`).
        if callee == "DISPLAY_PRINT" {
            if args.len() != 2 {
                self.push_diag(
                    bag,
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!(
                        "`DISPLAY_PRINT` expects 2 arguments (display_id, text), got {}",
                        args.len()
                    ),
                );
                return Some(TypeRef::Void);
            }
            if let (Some(target_arg), Some(text_arg)) = (args.first(), args.get(1))
                && let ExprKind::Literal(Literal::String(comp_id)) = &target_arg.value.kind
                && let Some(&charset) = self.display_charsets.get(comp_id)
                && let ExprKind::Literal(Literal::String(text)) = &text_arg.value.kind
                && let Some(bad_ch) = text.chars().find(|&c| !charset.supports_char(c))
            {
                self.push_diag(
                    bag,
                    ErrorCode::ErrStringCharset,
                    text_arg.value.span,
                    format!(
                        "display `{comp_id}` charset `{}` does not support glyph `{bad_ch}` (U+{:04X})",
                        charset.as_str(),
                        u32::from(bad_ch)
                    ),
                );
            }
            return Some(TypeRef::Void);
        }

        // 1. Check user-defined FC POUs
        if let Some((params, ret_ty)) = self.user_fcs.get(callee) {
            if args.len() != params.len() {
                self.push_diag(
                    bag,
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!(
                        "function `{callee}` expects {} argument(s), found {}",
                        params.len(),
                        args.len()
                    ),
                );
                return Some(ret_ty.clone());
            }
            for (arg, param) in args.iter().zip(params.iter()) {
                if let Some(arg_name) = &arg.name
                    && !params.iter().any(|p| &p.name == arg_name)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrArityMismatch,
                        arg.span,
                        format!("unknown named argument `{arg_name}` in call to `{callee}`"),
                    );
                    continue;
                }
                self.check_duty_overflow(&param.ty, &arg.value, arg.span, bag);
                if let Some(actual) = self.infer_expr(&arg.value, Some(&param.ty), locals, bag)
                    && !types_compatible(&param.ty, &actual)
                {
                    self.push_diag(
                        bag,
                        ErrorCode::ErrTypeMismatch,
                        arg.span,
                        format!(
                            "argument `{}` to `{callee}` has type `{actual}`, expected `{}`",
                            param.name, param.ty
                        ),
                    );
                }
            }
            return Some(ret_ty.clone());
        }

        // 2. Check domain-split standard library strictly within imported modules (`01` §6.1)
        let import_refs: Vec<&str> = self.imports.iter().map(String::as_str).collect();
        if let Some(StdSymbol::Function(sig)) =
            self.std_catalog.resolve_symbol(&import_refs, callee)
        {
            if args.len() != sig.params.len() {
                self.push_diag(
                    bag,
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!(
                        "stdlib function `{callee}` expects {} argument(s), found {}",
                        sig.params.len(),
                        args.len()
                    ),
                );
                return Some(std_type_to_typeref(sig.return_type));
            }
            let is_conversion = matches!(
                callee,
                "BOOL_TO_INT"
                    | "INT_TO_DINT"
                    | "DINT_TO_REAL"
                    | "REAL_TO_DINT"
                    | "ANALOG_TO_CODE"
                    | "CODE_TO_ANALOG"
                    | "ANALOG_TO_DUTY"
            );
            for (arg, param) in args.iter().zip(sig.params.iter()) {
                if param.ty == StdTypeRef::PinRef {
                    let _ = self.infer_expr(&arg.value, None, locals, bag);
                    continue;
                }
                if param.ty == StdTypeRef::AnyNum {
                    if let Some(actual) = self.infer_expr(&arg.value, None, locals, bag)
                        && !matches!(actual, TypeRef::Scalar { ref ty } if ty.is_plain_numeric())
                    {
                        self.push_diag(
                            bag,
                            ErrorCode::ErrTypeMismatch,
                            arg.span,
                            format!("`{callee}` requires numeric argument, found `{actual}`"),
                        );
                    }
                    continue;
                }
                let expected_ty = std_type_to_typeref(param.ty);
                self.check_duty_overflow(&expected_ty, &arg.value, arg.span, bag);
                if let Some(actual) = self.infer_expr(&arg.value, Some(&expected_ty), locals, bag)
                    && !types_compatible(&expected_ty, &actual)
                {
                    let code = if is_conversion {
                        ErrorCode::ErrInvalidCast
                    } else {
                        ErrorCode::ErrTypeMismatch
                    };
                    self.push_diag(
                        bag,
                        code,
                        arg.span,
                        format!(
                            "argument `{}` to `{callee}` has type `{actual}`, expected `{expected_ty}`",
                            param.name
                        ),
                    );
                }
            }
            // Validate codec id string in ANALOG_TO_CODE / CODE_TO_ANALOG (`ERR_INVALID_CAST`)
            if matches!(callee, "ANALOG_TO_CODE" | "CODE_TO_ANALOG")
                && let Some(codec_arg) = args.get(1)
                && let ExprKind::Literal(Literal::String(codec_id)) = &codec_arg.value.kind
                && let Some(bp) = self.board_profile
                && !bp.codecs.contains_key(codec_id)
            {
                self.push_diag(
                    bag,
                    ErrorCode::ErrInvalidCast,
                    codec_arg.span,
                    format!("unknown codec `{codec_id}` in `{callee}`"),
                );
            }
            return Some(std_type_to_typeref(sig.return_type));
        }

        self.push_diag(
            bag,
            ErrorCode::ErrUnresolvedFunction,
            span,
            format!("unresolved function `{callee}` (check spelling and module imports)"),
        );
        None
    }

    fn check_duty_overflow(
        &self,
        target_ty: &TypeRef,
        expr: &Expr,
        span: Span,
        bag: &mut DiagnosticBag,
    ) {
        if matches!(
            target_ty,
            TypeRef::Scalar {
                ty: ScalarType::Duty
            }
        ) && let Some(val) = eval_const_int(expr)
            && (val < 0 || val > i128::from(ASL_ANALOG_FULL_SCALE))
        {
            self.push_diag(
                bag,
                ErrorCode::WarnDutyClamped,
                span,
                format!(
                    "DUTY value {val} exceeds nominal range [0, {ASL_ANALOG_FULL_SCALE}] and will clamp"
                ),
            );
        }
    }

    fn fb_fields_for_type(&self, ty: &TypeRef) -> BTreeMap<String, TypeRef> {
        let TypeRef::Fb { fb_kind } = ty else {
            return BTreeMap::new();
        };
        if let FbKind::User { fb_name } = fb_kind {
            return self.user_fbs.get(fb_name).cloned().unwrap_or_default();
        }
        let std_name = match fb_kind {
            FbKind::Ton => "TON",
            FbKind::Tof => "TOF",
            FbKind::Tp => "TP",
            FbKind::Ctu => "CTU",
            FbKind::Ctd => "CTD",
            FbKind::Ctud => "CTUD",
            FbKind::RTrig => "R_TRIG",
            FbKind::FTrig => "F_TRIG",
            FbKind::Sr => "SR",
            FbKind::Rs => "RS",
            FbKind::UartSend => "UART_SEND",
            FbKind::UartRecv => "UART_RECV",
            FbKind::I2cXfer => "I2C_XFER",
            FbKind::SpiXfer => "SPI_XFER",
            FbKind::MbClient => "MB_CLIENT",
            FbKind::MbServer => "MB_SERVER",
            FbKind::User { .. } => return BTreeMap::new(),
        };
        let mut fields = BTreeMap::new();
        if let Some(StdSymbol::FunctionBlock(sig)) =
            self.std_catalog.resolve_symbol(&["std.iec.*"], std_name)
        {
            for f in sig.fields {
                fields.insert(f.name.to_owned(), std_type_to_typeref(f.ty));
            }
        }
        fields
    }
}

struct StmtCheckCtx<'a> {
    in_loop: bool,
    in_isr: bool,
    expected_return: &'a TypeRef,
    pou_name: &'a str,
}

fn is_int_like_target(ty: &TypeRef) -> bool {
    matches!(
        ty,
        TypeRef::Scalar {
            ty: ScalarType::Sint
                | ScalarType::Int
                | ScalarType::Dint
                | ScalarType::Lint
                | ScalarType::Usint
                | ScalarType::Uint
                | ScalarType::Udint
                | ScalarType::Ulint
                | ScalarType::Byte
                | ScalarType::Word
                | ScalarType::Dword
                | ScalarType::Lword
                | ScalarType::Duty
                | ScalarType::Analog
        }
    )
}

pub(crate) fn types_compatible(expected: &TypeRef, actual: &TypeRef) -> bool {
    if expected == actual {
        return true;
    }
    // Allow STRING[N] assignment between string capacities
    matches!(
        (expected, actual),
        (
            TypeRef::Scalar {
                ty: ScalarType::String { .. }
            },
            TypeRef::Scalar {
                ty: ScalarType::String { .. }
            }
        ) | (
            TypeRef::Scalar {
                ty: ScalarType::Code { .. }
            },
            TypeRef::Scalar {
                ty: ScalarType::Code { .. }
            }
        )
    )
}

fn std_type_to_typeref(ty: StdTypeRef) -> TypeRef {
    match ty {
        StdTypeRef::Void => TypeRef::Void,
        StdTypeRef::Bool => TypeRef::scalar(ScalarType::Bool),
        StdTypeRef::Sint => TypeRef::scalar(ScalarType::Sint),
        StdTypeRef::Int => TypeRef::scalar(ScalarType::Int),
        StdTypeRef::Dint | StdTypeRef::AnyNum | StdTypeRef::PinRef => {
            TypeRef::scalar(ScalarType::Dint)
        }
        StdTypeRef::Lint => TypeRef::scalar(ScalarType::Lint),
        StdTypeRef::Usint => TypeRef::scalar(ScalarType::Usint),
        StdTypeRef::Uint => TypeRef::scalar(ScalarType::Uint),
        StdTypeRef::Udint => TypeRef::scalar(ScalarType::Udint),
        StdTypeRef::Ulint => TypeRef::scalar(ScalarType::Ulint),
        StdTypeRef::Byte => TypeRef::scalar(ScalarType::Byte),
        StdTypeRef::Word => TypeRef::scalar(ScalarType::Word),
        StdTypeRef::Dword => TypeRef::scalar(ScalarType::Dword),
        StdTypeRef::Lword => TypeRef::scalar(ScalarType::Lword),
        StdTypeRef::Real => TypeRef::scalar(ScalarType::Real),
        StdTypeRef::Lreal => TypeRef::scalar(ScalarType::Lreal),
        StdTypeRef::Time => TypeRef::scalar(ScalarType::Time),
        StdTypeRef::String => TypeRef::scalar(ScalarType::String { max_len: 255 }),
        StdTypeRef::Analog => TypeRef::scalar(ScalarType::Analog),
        StdTypeRef::Duty => TypeRef::scalar(ScalarType::Duty),
        StdTypeRef::Code => TypeRef::scalar(ScalarType::Code {
            codec_id: "*".to_owned(),
        }),
    }
}

fn eval_const_int(expr: &Expr) -> Option<i128> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(n)) => Some(*n),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => eval_const_int(operand)?.checked_neg(),
        ExprKind::Unary {
            op: UnaryOp::Pos,
            operand,
        } => eval_const_int(operand),
        ExprKind::Binary { op, left, right } => {
            let l = eval_const_int(left)?;
            let r = eval_const_int(right)?;
            match op {
                BinaryOp::Add => l.checked_add(r),
                BinaryOp::Sub => l.checked_sub(r),
                BinaryOp::Mul => l.checked_mul(r),
                BinaryOp::Div => l.checked_div(r),
                BinaryOp::Mod => l.checked_rem(r),
                BinaryOp::Shl
                | BinaryOp::Shr
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::BitAnd
                | BinaryOp::BitXor
                | BinaryOp::BitOr
                | BinaryOp::And
                | BinaryOp::Xor
                | BinaryOp::Or => None,
            }
        }
        ExprKind::Literal(_)
        | ExprKind::Ident(_)
        | ExprKind::Index { .. }
        | ExprKind::Field { .. }
        | ExprKind::Call { .. }
        | ExprKind::Unary {
            op: UnaryOp::Not | UnaryOp::BitNot,
            ..
        }
        | ExprKind::Foreign { .. } => None,
    }
}

/// Maps a [`HardwareBinding`] to its corresponding ASL [`TypeRef`].
pub(crate) fn hardware_binding_type(hw: &HardwareBinding) -> TypeRef {
    match hw.kind {
        HardwareKind::DigitalIn | HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional => {
            TypeRef::scalar(ScalarType::Bool)
        }
        HardwareKind::AnalogIn | HardwareKind::AnalogOut => TypeRef::scalar(ScalarType::Analog),
        HardwareKind::PwmOut => TypeRef::scalar(ScalarType::Duty),
        HardwareKind::BusPin => TypeRef::scalar(ScalarType::Byte),
    }
}
