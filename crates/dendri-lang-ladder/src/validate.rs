//! Static semantic validation for IEC 61131-3 Ladder Diagram (`docs/lbc/ladder.md` §1.4).
//!
//! Enforces every frozen `ErrorCode` required by the Ladder LBC:
//! - `ERR_DUPLICATE_ID`
//! - `ERR_TYPE_MISMATCH`
//! - `ERR_CONST_WRITE` / `ERR_HARDWARE_LABEL_WRITE`
//! - `ERR_UNDECLARED_SYMBOL`
//! - `ERR_UNRESOLVED_FUNCTION` / `ERR_ARITY_MISMATCH` / `ERR_RETURN_TYPE_MISMATCH`
//! - `ERR_ISR_FORBIDDEN_OP`
//! - `ERR_UNKNOWN_PORT` / `ERR_PIN_CAPABILITY_MISMATCH`
//! - `ERR_COMMENT_ANCHOR` / `ERR_COMMENT_FIELD`

use crate::common::builtin_fb_field_type;
use dendri_ast::{
    BinaryOp, BoardProgram, CasePattern, CommentAnchor, CommentPlacement as AstCommentPlacement,
    Expr, ExprKind, FbKind, HardwareBinding, HardwareKind, LValue, LadderBoxKind, LadderCell,
    LadderCellKind, LadderNetwork, Literal, NodeId, ObKind, ParamDecl, Pou, PouBody, PouKind,
    ScalarType, Stmt, StmtKind, StorageScope, TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::{BoardContext, BoardProgramBundle, CommentPlacement, LanguageId};
use dendri_span::{Diagnostic, ErrorCode, Span};
use dendri_stdlib::{StdCatalog, StdSymbol, StdTypeRef};
use std::collections::{BTreeMap, BTreeSet};

/// Runs structural + declared-symbol validation suitable during `LanguageFrontend::parse`.
///
/// Standalone single-POU Ladder fragments referencing board-level symbols declared elsewhere
/// are allowed here (`check_undeclared = false`), while duplicate IDs, ISR blocking calls,
/// hardware port/capability errors, and type/immutability violations on declared symbols are
/// caught immediately.
#[must_use]
pub(crate) fn validate_on_parse(
    bundle: &BoardProgramBundle,
    ctx: &BoardContext,
) -> Vec<Diagnostic> {
    let mut validator = LadderValidator::new(&bundle.program, ctx, false);
    validator.validate_all(&bundle.comments);
    validator.diags
}

/// Runs complete closed-world Ladder semantic validation on a [`BoardProgramBundle`]
/// (`docs/lbc/ladder.md` §1.4), including `ERR_UNDECLARED_SYMBOL`, `ERR_UNRESOLVED_FUNCTION`,
/// `ERR_COMMENT_ANCHOR`, and `ERR_COMMENT_FIELD`.
#[must_use]
pub fn validate_ladder_bundle(bundle: &BoardProgramBundle, ctx: &BoardContext) -> Vec<Diagnostic> {
    let mut validator = LadderValidator::new(&bundle.program, ctx, true);
    validator.validate_all(&bundle.comments);
    validator.diags
}

#[derive(Debug, Clone)]
struct VarInfo {
    ty: TypeRef,
    is_const: bool,
    hw_kind: Option<HardwareKind>,
}

#[derive(Debug, Clone)]
struct FnSigInfo {
    arity: usize,
    return_type: Option<TypeRef>,
    /// `true` when the return type mirrors the first argument's numeric/value type (`ADD`, `MOVE`, `ABS`, etc.).
    polymorphic_return: bool,
    /// `true` when any number of arguments is accepted (e.g. variadic `ADD`).
    min_arity: Option<usize>,
}

struct LadderValidator<'a> {
    program: &'a BoardProgram,
    ctx: &'a BoardContext,
    check_undeclared: bool,
    catalog: StdCatalog,
    live_ids: BTreeSet<NodeId>,
    structs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    user_fb_fields: BTreeMap<String, BTreeMap<String, TypeRef>>,
    globals: BTreeMap<String, VarInfo>,
    user_pous: BTreeMap<String, (Vec<ParamDecl>, TypeRef)>,
    diags: Vec<Diagnostic>,
}

impl<'a> LadderValidator<'a> {
    fn new(program: &'a BoardProgram, ctx: &'a BoardContext, check_undeclared: bool) -> Self {
        Self {
            program,
            ctx,
            check_undeclared,
            catalog: ctx.std_catalog(),
            live_ids: BTreeSet::new(),
            structs: BTreeMap::new(),
            user_fb_fields: BTreeMap::new(),
            globals: BTreeMap::new(),
            user_pous: BTreeMap::new(),
            diags: Vec::new(),
        }
    }

    fn push_diag(&mut self, code: ErrorCode, span: Span, msg: impl Into<String>) {
        let mut d = Diagnostic::new(code, msg).with_board_id(&self.ctx.board_id);
        d.span = Some(span);
        self.diags.push(d);
    }

    fn record_id(&mut self, id: &NodeId, span: Span) {
        if !self.live_ids.insert(id.clone()) {
            self.push_diag(
                ErrorCode::ErrDuplicateId,
                span,
                format!("duplicate Ladder NodeId `{id}`"),
            );
        }
    }

    fn validate_all(&mut self, comments: &[dendri_ast::Comment]) {
        self.collect_ids_and_symbols();
        self.validate_hardware_bindings();

        for pou in &self.program.pous {
            self.validate_pou(pou, &self.program.imports);
        }
        for module in &self.program.modules {
            let mut combined_imports = self.program.imports.clone();
            combined_imports.extend(module.imports.iter().cloned());
            for pou in &module.pous {
                self.validate_pou(pou, &combined_imports);
            }
        }

        self.validate_comments(comments);
    }

    fn collect_ids_and_symbols(&mut self) {
        for hw in &self.program.hardware {
            self.record_id(&hw.id, hw.span);
            self.globals.insert(
                hw.label.clone(),
                VarInfo {
                    ty: hardware_type_ref(hw),
                    is_const: false,
                    hw_kind: Some(hw.kind),
                },
            );
        }

        for td in &self.program.types {
            self.register_type_decl(td);
        }
        for d in &self.program.data {
            self.record_id(&d.id, d.span);
            self.globals.insert(
                d.name.clone(),
                VarInfo {
                    ty: d.ty.clone(),
                    is_const: d.scope == StorageScope::Const,
                    hw_kind: None,
                },
            );
        }

        for pou in &self.program.pous {
            self.register_pou_ids_and_sig(pou);
        }

        for m in &self.program.modules {
            self.record_id(&m.id, m.span);
            for td in &m.types {
                self.register_type_decl(td);
            }
            for d in &m.data {
                self.record_id(&d.id, d.span);
                self.globals.insert(
                    d.name.clone(),
                    VarInfo {
                        ty: d.ty.clone(),
                        is_const: d.scope == StorageScope::Const,
                        hw_kind: None,
                    },
                );
            }
            for pou in &m.pous {
                self.register_pou_ids_and_sig(pou);
            }
        }
    }

    fn register_type_decl(&mut self, td: &TypeDecl) {
        match td {
            TypeDecl::Struct {
                id,
                name,
                fields,
                span,
            } => {
                self.record_id(id, *span);
                let mut map = BTreeMap::new();
                for f in fields {
                    map.insert(f.name.clone(), f.ty.clone());
                }
                self.structs.insert(name.clone(), map);
            }
            TypeDecl::Enum { id, span, .. } | TypeDecl::Foreign { id, span, .. } => {
                self.record_id(id, *span);
            }
        }
    }

    fn register_pou_ids_and_sig(&mut self, pou: &Pou) {
        self.record_id(&pou.id, pou.span);
        for local in &pou.locals {
            self.record_id(&local.id, local.span);
        }
        let ret_ty = match &pou.kind {
            PouKind::Fc { return_type } => return_type.clone(),
            PouKind::Ob { .. } | PouKind::Fb => TypeRef::Void,
        };
        if matches!(pou.kind, PouKind::Fb) {
            let mut fields = BTreeMap::new();
            for p in &pou.params {
                fields.insert(p.name.clone(), p.ty.clone());
            }
            self.user_fb_fields.insert(pou.name.clone(), fields);
        }
        self.user_pous
            .insert(pou.name.clone(), (pou.params.clone(), ret_ty));

        match &pou.body {
            PouBody::Network(net) => {
                for rung in &net.rungs {
                    self.record_id(&rung.id, pou.span);
                    self.collect_cell_ids(&rung.cells, pou.span);
                }
            }
            PouBody::Statements(stmts) => {
                self.collect_stmt_ids(stmts);
            }
            PouBody::Chart(chart) => {
                for step in &chart.steps {
                    self.record_id(&step.id, pou.span);
                    for act in &step.actions {
                        self.record_id(&act.id, pou.span);
                        self.collect_stmt_ids(&act.body);
                    }
                }
                for tr in &chart.transitions {
                    self.record_id(&tr.id, pou.span);
                }
            }
        }
    }

    fn collect_cell_ids(&mut self, cells: &[LadderCell], fallback_span: Span) {
        for cell in cells {
            self.record_id(&cell.id, fallback_span);
            if let LadderCellKind::ParallelOr { branches } = &cell.kind {
                for br in branches {
                    self.collect_cell_ids(br, fallback_span);
                }
            }
        }
    }

    fn collect_stmt_ids(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.record_id(&s.id, s.span);
            match &s.kind {
                StmtKind::If {
                    then_body,
                    elif_arms,
                    else_body,
                    ..
                } => {
                    self.collect_stmt_ids(then_body);
                    for arm in elif_arms {
                        self.collect_stmt_ids(&arm.body);
                    }
                    self.collect_stmt_ids(else_body);
                }
                StmtKind::Switch { cases, default, .. } => {
                    for c in cases {
                        self.collect_stmt_ids(&c.body);
                    }
                    if let Some(def) = default {
                        self.collect_stmt_ids(def);
                    }
                }
                StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                    self.collect_stmt_ids(body);
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

    fn validate_hardware_bindings(&mut self) {
        for hw in &self.program.hardware {
            self.validate_single_hardware_binding(hw);
        }
    }

    fn validate_single_hardware_binding(&mut self, hw: &HardwareBinding) {
        // 1. Unresolved `%...` IEC direct address
        if hw.pin.starts_with('%') {
            self.push_diag(
                ErrorCode::ErrUnknownPort,
                hw.span,
                format!(
                    "unresolved IEC direct address `{}` on board profile `{}`",
                    hw.pin, self.ctx.profile_id
                ),
            );
            return;
        }

        // 2. Profile with explicit IEC port map (e.g. `plc-1200-open-1214`)
        if !self.ctx.port_to_iec.is_empty() {
            let Some(iec) = self.ctx.iec_for_port(&hw.pin) else {
                self.push_diag(
                    ErrorCode::ErrUnknownPort,
                    hw.span,
                    format!(
                        "unknown hardware port `{}` on board profile `{}`",
                        hw.pin, self.ctx.profile_id
                    ),
                );
                return;
            };
            let upper = iec.to_ascii_uppercase();
            let cap_ok = if upper.starts_with("%IX") {
                hw.kind == HardwareKind::DigitalIn
            } else if upper.starts_with("%QX") {
                hw.kind == HardwareKind::DigitalOut
            } else if upper.starts_with("%IW") {
                hw.kind == HardwareKind::AnalogIn
            } else if upper.starts_with("%QW") {
                hw.kind == HardwareKind::AnalogOut
            } else {
                true
            };
            if !cap_ok {
                self.push_diag(
                    ErrorCode::ErrPinCapabilityMismatch,
                    hw.span,
                    format!(
                        "port `{}` (`{iec}`) does not support hardware kind `{:?}`",
                        hw.pin, hw.kind
                    ),
                );
            }
            return;
        }

        // 3. Arduino Uno R3 certified profile pin/capability rules
        if self.ctx.profile_id == "arduino-uno-r3" {
            let is_digital = hw
                .pin
                .strip_prefix('D')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| n <= 13);
            let is_analog = hw
                .pin
                .strip_prefix('A')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| n <= 5);
            if !is_digital && !is_analog {
                self.push_diag(
                    ErrorCode::ErrUnknownPort,
                    hw.span,
                    format!(
                        "unknown hardware port `{}` on board profile `arduino-uno-r3`",
                        hw.pin
                    ),
                );
                return;
            }
            let is_pwm = matches!(hw.pin.as_str(), "D3" | "D5" | "D6" | "D9" | "D10" | "D11");
            let cap_ok = match hw.kind {
                HardwareKind::DigitalIn
                | HardwareKind::DigitalOut
                | HardwareKind::DigitalBidirectional
                | HardwareKind::BusPin => true,
                HardwareKind::AnalogIn => is_analog,
                HardwareKind::PwmOut => is_pwm,
                HardwareKind::AnalogOut => false,
            };
            if !cap_ok {
                self.push_diag(
                    ErrorCode::ErrPinCapabilityMismatch,
                    hw.span,
                    format!(
                        "port `{}` on `arduino-uno-r3` does not support `{:?}`",
                        hw.pin, hw.kind
                    ),
                );
            }
        }
    }

    fn validate_pou(&mut self, pou: &Pou, imports: &[String]) {
        let mut locals = BTreeMap::new();
        for param in &pou.params {
            locals.insert(
                param.name.clone(),
                VarInfo {
                    ty: param.ty.clone(),
                    is_const: false,
                    hw_kind: None,
                },
            );
        }
        for local in &pou.locals {
            locals.insert(
                local.name.clone(),
                VarInfo {
                    ty: local.ty.clone(),
                    is_const: local.scope == StorageScope::Const,
                    hw_kind: None,
                },
            );
        }

        let in_isr = matches!(
            pou.kind,
            PouKind::Ob {
                ob: ObKind::HwInt { .. } | ObKind::CyclicInt { .. }
            }
        );

        match &pou.body {
            PouBody::Network(net) => {
                self.validate_network(net, &locals, imports, in_isr, pou.span);
            }
            PouBody::Statements(stmts) => {
                self.validate_stmts(stmts, &locals, imports, in_isr);
            }
            PouBody::Chart(_) => {}
        }
    }

    fn validate_network(
        &mut self,
        net: &LadderNetwork,
        locals: &BTreeMap<String, VarInfo>,
        imports: &[String],
        in_isr: bool,
        pou_span: Span,
    ) {
        for rung in &net.rungs {
            for cell in &rung.cells {
                self.validate_cell(cell, locals, imports, in_isr, pou_span);
            }
        }
    }

    fn validate_cell(
        &mut self,
        cell: &LadderCell,
        locals: &BTreeMap<String, VarInfo>,
        imports: &[String],
        in_isr: bool,
        fallback_span: Span,
    ) {
        let bool_ty = TypeRef::scalar(ScalarType::Bool);
        match &cell.kind {
            LadderCellKind::Empty | LadderCellKind::Wire => {}
            LadderCellKind::Contact { kind: _, target } => {
                if let Some(ty) = self.resolve_lvalue_type(target, locals)
                    && !types_compatible(&bool_ty, &ty)
                {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        target.span(),
                        format!(
                            "Ladder contact `{}` must reference BOOL, found `{ty}`",
                            dendri_expr::print_lvalue(target)
                        ),
                    );
                }
            }
            LadderCellKind::Coil { kind: _, target } => {
                self.check_lvalue_writable(target, locals, true);
                if let Some(ty) = self.resolve_lvalue_type(target, locals)
                    && !types_compatible(&bool_ty, &ty)
                {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        target.span(),
                        format!(
                            "Ladder coil `{}` must reference BOOL, found `{ty}`",
                            dendri_expr::print_lvalue(target)
                        ),
                    );
                }
            }
            LadderCellKind::ParallelOr { branches } => {
                for br in branches {
                    for c in br {
                        self.validate_cell(c, locals, imports, in_isr, fallback_span);
                    }
                }
            }
            LadderCellKind::Box(box_kind) => match box_kind {
                LadderBoxKind::Fb {
                    kind,
                    instance,
                    inputs,
                } => {
                    if let Some(inst_ty) = self.resolve_lvalue_type(instance, locals) {
                        match inst_ty {
                            TypeRef::Fb { fb_kind } if fb_kind == *kind => {}
                            other @ (TypeRef::Void
                            | TypeRef::Scalar { .. }
                            | TypeRef::Array { .. }
                            | TypeRef::Named { .. }
                            | TypeRef::Fb { .. }) => {
                                self.push_diag(
                                    ErrorCode::ErrTypeMismatch,
                                    instance.span(),
                                    format!(
                                        "Ladder FB box expects `{}` instance, found `{other}`",
                                        crate::common::fb_kind_name(kind)
                                    ),
                                );
                            }
                        }
                    }
                    for (pin_name, expr) in inputs {
                        let expr_ty = self.infer_expr_type(expr, locals, imports);
                        if let Some(expected_ty) = self.fb_field_type(kind, pin_name)
                            && let Some(actual_ty) = expr_ty
                            && !types_compatible(&expected_ty, &actual_ty)
                        {
                            self.push_diag(
                                ErrorCode::ErrTypeMismatch,
                                expr.span,
                                format!(
                                    "FB `{}` input `{pin_name}` expects `{expected_ty}`, found `{actual_ty}`",
                                    crate::common::fb_kind_name(kind)
                                ),
                            );
                        }
                    }
                }
                LadderBoxKind::Compare { op: _, left, right } => {
                    let lt = self.infer_expr_type(left, locals, imports);
                    let rt = self.infer_expr_type(right, locals, imports);
                    let mismatch = match (&lt, &rt) {
                        (Some(l), Some(r)) => !compare_types_compatible(l, r),
                        (Some(l), None) if is_int_literal(right) => !accepts_int_literal(l),
                        (None, Some(r)) if is_int_literal(left) => !accepts_int_literal(r),
                        _ => false,
                    };
                    if mismatch {
                        let l_desc = lt
                            .as_ref()
                            .map_or_else(|| "INT_LITERAL".to_owned(), ToString::to_string);
                        let r_desc = rt
                            .as_ref()
                            .map_or_else(|| "INT_LITERAL".to_owned(), ToString::to_string);
                        self.push_diag(
                            ErrorCode::ErrTypeMismatch,
                            left.span.merge(right.span),
                            format!(
                                "Ladder comparison box operand types `{l_desc}` and `{r_desc}` are incompatible"
                            ),
                        );
                    }
                }
                LadderBoxKind::Call {
                    callee,
                    args,
                    output,
                } => {
                    if in_isr
                        && (callee.eq_ignore_ascii_case("WAIT_TIME")
                            || callee.eq_ignore_ascii_case("WAIT_TIME_US"))
                    {
                        self.push_diag(
                            ErrorCode::ErrIsrForbiddenOp,
                            fallback_span,
                            format!(
                                "blocking call `{callee}` is forbidden inside interrupt OB Ladder network"
                            ),
                        );
                    }

                    let mut arg_types = Vec::with_capacity(args.len());
                    for (_, expr) in args {
                        arg_types.push(self.infer_expr_type(expr, locals, imports));
                    }

                    let sig = self.resolve_callee_sig(callee, imports);
                    match sig {
                        Some(sig) => {
                            let arity_ok = if let Some(min_a) = sig.min_arity {
                                args.len() >= min_a
                            } else {
                                args.len() == sig.arity
                            };
                            if !arity_ok {
                                self.push_diag(
                                    ErrorCode::ErrArityMismatch,
                                    fallback_span,
                                    format!(
                                        "Ladder CALL `{callee}` expects {} argument(s), found {}",
                                        sig.arity,
                                        args.len()
                                    ),
                                );
                            }
                            if let Some(out_lv) = output {
                                self.check_lvalue_writable(out_lv, locals, true);
                                let ret_ty = if sig.polymorphic_return {
                                    arg_types.first().and_then(Clone::clone)
                                } else {
                                    sig.return_type.clone()
                                };
                                if let Some(ret) = ret_ty {
                                    if ret == TypeRef::Void {
                                        self.push_diag(
                                            ErrorCode::ErrReturnTypeMismatch,
                                            out_lv.span(),
                                            format!(
                                                "Ladder CALL `{callee}` returns VOID and cannot bind output `{}`",
                                                dendri_expr::print_lvalue(out_lv)
                                            ),
                                        );
                                    } else if let Some(out_ty) =
                                        self.resolve_lvalue_type(out_lv, locals)
                                        && !types_compatible(&out_ty, &ret)
                                    {
                                        self.push_diag(
                                            ErrorCode::ErrReturnTypeMismatch,
                                            out_lv.span(),
                                            format!(
                                                "Ladder CALL `{callee}` return type `{ret}` does not match output `{out_ty}`"
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                        None => {
                            if self.check_undeclared {
                                self.push_diag(
                                    ErrorCode::ErrUnresolvedFunction,
                                    fallback_span,
                                    format!("unresolved Ladder CALL target `{callee}`"),
                                );
                            }
                            if let Some(out_lv) = output {
                                self.check_lvalue_writable(out_lv, locals, true);
                                let _ = self.resolve_lvalue_type(out_lv, locals);
                            }
                        }
                    }
                }
                LadderBoxKind::Foreign { .. } => {}
            },
        }
    }

    fn validate_stmts(
        &mut self,
        stmts: &[Stmt],
        locals: &BTreeMap<String, VarInfo>,
        imports: &[String],
        in_isr: bool,
    ) {
        for s in stmts {
            match &s.kind {
                StmtKind::Wait { .. } => {
                    if in_isr {
                        self.push_diag(
                            ErrorCode::ErrIsrForbiddenOp,
                            s.span,
                            "blocking `WAIT` is forbidden inside interrupt OB",
                        );
                    }
                }
                StmtKind::Call { callee, args } => {
                    if in_isr
                        && (callee.eq_ignore_ascii_case("WAIT_TIME")
                            || callee.eq_ignore_ascii_case("WAIT_TIME_US"))
                    {
                        self.push_diag(
                            ErrorCode::ErrIsrForbiddenOp,
                            s.span,
                            format!("blocking call `{callee}` is forbidden inside interrupt OB"),
                        );
                    }
                    for a in args {
                        let _ = self.infer_expr_type(&a.value, locals, imports);
                    }
                }
                StmtKind::Assign { target, value } => {
                    self.check_lvalue_writable(target, locals, false);
                    let _ = self.resolve_lvalue_type(target, locals);
                    let _ = self.infer_expr_type(value, locals, imports);
                }
                StmtKind::FbInvoke { instance, inputs } => {
                    let _ = self.resolve_lvalue_type(instance, locals);
                    for (_, expr) in inputs {
                        let _ = self.infer_expr_type(expr, locals, imports);
                    }
                }
                StmtKind::If {
                    condition,
                    then_body,
                    elif_arms,
                    else_body,
                } => {
                    let _ = self.infer_expr_type(condition, locals, imports);
                    self.validate_stmts(then_body, locals, imports, in_isr);
                    for arm in elif_arms {
                        let _ = self.infer_expr_type(&arm.condition, locals, imports);
                        self.validate_stmts(&arm.body, locals, imports, in_isr);
                    }
                    self.validate_stmts(else_body, locals, imports, in_isr);
                }
                StmtKind::Switch {
                    scrutinee,
                    cases,
                    default,
                } => {
                    let _ = self.infer_expr_type(scrutinee, locals, imports);
                    for c in cases {
                        for p in &c.patterns {
                            match p {
                                CasePattern::Value(e) => {
                                    let _ = self.infer_expr_type(e, locals, imports);
                                }
                                CasePattern::Range { start, end } => {
                                    let _ = self.infer_expr_type(start, locals, imports);
                                    let _ = self.infer_expr_type(end, locals, imports);
                                }
                            }
                        }
                        self.validate_stmts(&c.body, locals, imports, in_isr);
                    }
                    if let Some(def) = default {
                        self.validate_stmts(def, locals, imports, in_isr);
                    }
                }
                StmtKind::While { condition, body } => {
                    let _ = self.infer_expr_type(condition, locals, imports);
                    self.validate_stmts(body, locals, imports, in_isr);
                }
                StmtKind::For {
                    counter: _,
                    from,
                    to,
                    step,
                    inclusive: _,
                    body,
                } => {
                    let _ = self.infer_expr_type(from, locals, imports);
                    let _ = self.infer_expr_type(to, locals, imports);
                    let _ = self.infer_expr_type(step, locals, imports);
                    self.validate_stmts(body, locals, imports, in_isr);
                }
                StmtKind::Return { value } => {
                    if let Some(e) = value {
                        let _ = self.infer_expr_type(e, locals, imports);
                    }
                }
                StmtKind::Break | StmtKind::Continue | StmtKind::Foreign { .. } => {}
            }
        }
    }

    fn lookup_var<'b>(
        &'b self,
        name: &str,
        locals: &'b BTreeMap<String, VarInfo>,
    ) -> Option<&'b VarInfo> {
        locals.get(name).or_else(|| self.globals.get(name))
    }

    fn check_lvalue_writable(
        &mut self,
        lvalue: &LValue,
        locals: &BTreeMap<String, VarInfo>,
        allow_digital_out_coil: bool,
    ) {
        let root = lvalue.root_name();
        let Some(info) = self.lookup_var(root, locals).cloned() else {
            return;
        };
        if info.is_const {
            self.push_diag(
                ErrorCode::ErrConstWrite,
                lvalue.span(),
                format!("cannot write to CONSTANT `{root}`"),
            );
            return;
        }
        if let Some(hw_kind) = info.hw_kind {
            let writable = match hw_kind {
                HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional => {
                    allow_digital_out_coil
                }
                HardwareKind::DigitalIn
                | HardwareKind::AnalogIn
                | HardwareKind::AnalogOut
                | HardwareKind::PwmOut
                | HardwareKind::BusPin => false,
            };
            if !writable {
                self.push_diag(
                    ErrorCode::ErrHardwareLabelWrite,
                    lvalue.span(),
                    format!("cannot write to read-only hardware tag `{root}` (`{hw_kind:?}`)"),
                );
            }
        }
    }

    fn resolve_lvalue_type(
        &mut self,
        lvalue: &LValue,
        locals: &BTreeMap<String, VarInfo>,
    ) -> Option<TypeRef> {
        match lvalue {
            LValue::Var { name, span } => {
                if let Some(info) = self.lookup_var(name, locals) {
                    Some(info.ty.clone())
                } else {
                    if self.check_undeclared {
                        self.push_diag(
                            ErrorCode::ErrUndeclaredSymbol,
                            *span,
                            format!("undeclared Ladder variable or hardware tag `{name}`"),
                        );
                    }
                    None
                }
            }
            LValue::Index { base, index, .. } => {
                let _ = self.infer_expr_type(index, locals, &self.program.imports);
                let base_ty = self.resolve_lvalue_type(base, locals)?;
                match base_ty {
                    TypeRef::Array { elem, .. } => Some(*elem),
                    other @ (TypeRef::Void
                    | TypeRef::Scalar { .. }
                    | TypeRef::Named { .. }
                    | TypeRef::Fb { .. }) => {
                        self.push_diag(
                            ErrorCode::ErrTypeMismatch,
                            lvalue.span(),
                            format!("cannot index into non-array type `{other}`"),
                        );
                        None
                    }
                }
            }
            LValue::Field { base, field, span } => {
                let base_ty = self.resolve_lvalue_type(base, locals)?;
                self.resolve_member_type(&base_ty, field, *span)
            }
        }
    }

    fn resolve_member_type(
        &mut self,
        base_ty: &TypeRef,
        field: &str,
        span: Span,
    ) -> Option<TypeRef> {
        match base_ty {
            TypeRef::Fb { fb_kind } => {
                let found = self.fb_field_type(fb_kind, field);
                if found.is_none() && self.check_undeclared {
                    self.push_diag(
                        ErrorCode::ErrUnknownFieldAccess,
                        span,
                        format!("unknown FB field `{field}` on `{base_ty}`"),
                    );
                }
                found
            }
            TypeRef::Named { name } => {
                let found = self.structs.get(name).and_then(|m| m.get(field)).cloned();
                if found.is_none() && self.check_undeclared {
                    self.push_diag(
                        ErrorCode::ErrUnknownFieldAccess,
                        span,
                        format!("unknown struct field `{field}` on `{name}`"),
                    );
                }
                found
            }
            TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Array { .. } => {
                self.push_diag(
                    ErrorCode::ErrUnknownFieldAccess,
                    span,
                    format!("type `{base_ty}` has no field `{field}`"),
                );
                None
            }
        }
    }

    fn fb_field_type(&self, kind: &FbKind, field: &str) -> Option<TypeRef> {
        if let Some(ty) = builtin_fb_field_type(kind, field) {
            return Some(ty);
        }
        if let FbKind::User { fb_name } = kind {
            if field.eq_ignore_ascii_case("EN") || field.eq_ignore_ascii_case("ENO") {
                return Some(TypeRef::scalar(ScalarType::Bool));
            }
            return self
                .user_fb_fields
                .get(fb_name)
                .and_then(|fields| fields.get(field))
                .cloned();
        }
        None
    }

    fn infer_expr_type(
        &mut self,
        expr: &Expr,
        locals: &BTreeMap<String, VarInfo>,
        imports: &[String],
    ) -> Option<TypeRef> {
        match &expr.kind {
            ExprKind::Literal(lit) => match lit {
                Literal::Bool(_) => Some(TypeRef::scalar(ScalarType::Bool)),
                Literal::Int(_) => None, // polymorphic integer literal
                Literal::Float(_) => Some(TypeRef::scalar(ScalarType::Real)),
                Literal::TimeUs(_) => Some(TypeRef::scalar(ScalarType::Time)),
                Literal::String(_) => Some(TypeRef::scalar(ScalarType::String { max_len: 255 })),
            },
            ExprKind::Ident(name) => {
                if let Some(info) = self.lookup_var(name, locals) {
                    Some(info.ty.clone())
                } else {
                    if self.check_undeclared {
                        self.push_diag(
                            ErrorCode::ErrUndeclaredSymbol,
                            expr.span,
                            format!("undeclared symbol `{name}` in Ladder expression"),
                        );
                    }
                    None
                }
            }
            ExprKind::Index { base, index } => {
                let _ = self.infer_expr_type(index, locals, imports);
                let base_ty = self.infer_expr_type(base, locals, imports)?;
                match base_ty {
                    TypeRef::Array { elem, .. } => Some(*elem),
                    TypeRef::Void
                    | TypeRef::Scalar { .. }
                    | TypeRef::Named { .. }
                    | TypeRef::Fb { .. } => None,
                }
            }
            ExprKind::Field { base, field } => {
                let base_ty = self.infer_expr_type(base, locals, imports)?;
                self.resolve_member_type(&base_ty, field, expr.span)
            }
            ExprKind::Unary { op, operand } => {
                let t = self.infer_expr_type(operand, locals, imports);
                match op {
                    UnaryOp::Not => Some(TypeRef::scalar(ScalarType::Bool)),
                    UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot => t,
                }
            }
            ExprKind::Binary { op, left, right } => {
                let lt = self.infer_expr_type(left, locals, imports);
                let rt = self.infer_expr_type(right, locals, imports);
                match op {
                    BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or
                    | BinaryOp::Xor => Some(TypeRef::scalar(ScalarType::Bool)),
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
            ExprKind::Call { callee, args } => {
                let mut first_arg_ty = None;
                for (idx, a) in args.iter().enumerate() {
                    let t = self.infer_expr_type(&a.value, locals, imports);
                    if idx == 0 {
                        first_arg_ty = t;
                    }
                }
                let sig = self.resolve_callee_sig(callee, imports)?;
                if sig.polymorphic_return {
                    first_arg_ty
                } else {
                    sig.return_type
                }
            }
            ExprKind::Foreign { .. } => None,
        }
    }

    fn resolve_callee_sig(&self, callee: &str, imports: &[String]) -> Option<FnSigInfo> {
        // 1. User-defined POUs (`FC`, `FB`, `OB`)
        if let Some((params, ret_ty)) = self.user_pous.get(callee) {
            return Some(FnSigInfo {
                arity: params.len(),
                return_type: Some(ret_ty.clone()),
                polymorphic_return: false,
                min_arity: None,
            });
        }

        // 2. Canonical IEC Ladder elemental math/move boxes (`docs/lbc/ladder.md` §2)
        if callee.eq_ignore_ascii_case("MOVE") {
            return Some(FnSigInfo {
                arity: 1,
                return_type: None,
                polymorphic_return: true,
                min_arity: None,
            });
        }
        if callee.eq_ignore_ascii_case("ADD")
            || callee.eq_ignore_ascii_case("SUB")
            || callee.eq_ignore_ascii_case("MUL")
            || callee.eq_ignore_ascii_case("DIV")
            || callee.eq_ignore_ascii_case("MOD")
        {
            return Some(FnSigInfo {
                arity: 2,
                return_type: None,
                polymorphic_return: true,
                min_arity: None,
            });
        }

        // 3. Domain-split stdlib catalog (`std.iec.*`, `std.plc.*`, `std.arduino.*`)
        let effective_imports: Vec<&str> = if imports.is_empty() {
            vec!["std.iec.*"]
        } else {
            imports.iter().map(String::as_str).collect()
        };
        if let Some(StdSymbol::Function(sig)) =
            self.catalog.resolve_symbol(&effective_imports, callee)
        {
            let polymorphic = sig.return_type == StdTypeRef::AnyNum;
            return Some(FnSigInfo {
                arity: sig.params.len(),
                return_type: std_type_to_ast(sig.return_type),
                polymorphic_return: polymorphic,
                min_arity: None,
            });
        }

        // 4. Loaded hash-locked `pack.toml` manifests
        for lang in [LanguageId::Ladder, LanguageId::St] {
            if let Some((_, sym)) = self.ctx.lookup_pack_symbol(lang, callee) {
                return Some(FnSigInfo {
                    arity: sym.params.len(),
                    return_type: sym.return_type_ref(),
                    polymorphic_return: false,
                    min_arity: None,
                });
            }
        }

        None
    }

    fn validate_comments(&mut self, comments: &[dendri_ast::Comment]) {
        for c in comments {
            if CommentPlacement::from_comment(c).is_err() {
                self.push_diag(
                    ErrorCode::ErrCommentField,
                    c.span,
                    format!(
                        "invalid comment placement/token_ordinal combination on comment `{}`",
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
                self.push_diag(
                    ErrorCode::ErrCommentField,
                    c.span,
                    format!(
                        "non-node comment `{}` only allows `header` or `body` placement",
                        c.id
                    ),
                );
            }
            if let CommentAnchor::Node { id } = &c.anchor
                && !self.live_ids.contains(id)
            {
                self.push_diag(
                    ErrorCode::ErrCommentAnchor,
                    c.span,
                    format!(
                        "comment `{}` references nonexistent Ladder NodeId `{id}`",
                        c.id
                    ),
                );
            }
        }
    }
}

fn hardware_type_ref(hw: &HardwareBinding) -> TypeRef {
    match hw.kind {
        HardwareKind::DigitalIn | HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional => {
            TypeRef::scalar(ScalarType::Bool)
        }
        HardwareKind::AnalogIn | HardwareKind::AnalogOut => match &hw.codec {
            Some(codec_id) => TypeRef::scalar(ScalarType::Code {
                codec_id: codec_id.clone(),
            }),
            None => TypeRef::scalar(ScalarType::Analog),
        },
        HardwareKind::PwmOut => TypeRef::scalar(ScalarType::Duty),
        HardwareKind::BusPin => TypeRef::scalar(ScalarType::Byte),
    }
}

fn std_type_to_ast(st: StdTypeRef) -> Option<TypeRef> {
    let ty = match st {
        StdTypeRef::Void => return Some(TypeRef::Void),
        StdTypeRef::Bool => ScalarType::Bool,
        StdTypeRef::Sint => ScalarType::Sint,
        StdTypeRef::Int => ScalarType::Int,
        StdTypeRef::Dint => ScalarType::Dint,
        StdTypeRef::Lint => ScalarType::Lint,
        StdTypeRef::Usint => ScalarType::Usint,
        StdTypeRef::Uint => ScalarType::Uint,
        StdTypeRef::Udint => ScalarType::Udint,
        StdTypeRef::Ulint => ScalarType::Ulint,
        StdTypeRef::Byte => ScalarType::Byte,
        StdTypeRef::Word => ScalarType::Word,
        StdTypeRef::Dword => ScalarType::Dword,
        StdTypeRef::Lword => ScalarType::Lword,
        StdTypeRef::Real => ScalarType::Real,
        StdTypeRef::Lreal => ScalarType::Lreal,
        StdTypeRef::Time => ScalarType::Time,
        StdTypeRef::String => ScalarType::String { max_len: 255 },
        StdTypeRef::Analog => ScalarType::Analog,
        StdTypeRef::Duty => ScalarType::Duty,
        StdTypeRef::Code | StdTypeRef::AnyNum | StdTypeRef::PinRef => return None,
    };
    Some(TypeRef::scalar(ty))
}

fn types_compatible(expected: &TypeRef, actual: &TypeRef) -> bool {
    if expected == actual {
        return true;
    }
    matches!(
        (expected, actual),
        (
            TypeRef::Scalar {
                ty: ScalarType::String { .. },
            },
            TypeRef::Scalar {
                ty: ScalarType::String { .. },
            },
        )
    )
}

fn is_int_literal(expr: &Expr) -> bool {
    matches!(expr.kind, ExprKind::Literal(Literal::Int(_)))
}

fn accepts_int_literal(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Scalar { ty: s } => {
            s.is_signed_int()
                || s.is_unsigned_int()
                || s.is_bitstring()
                || matches!(s, ScalarType::Code { .. })
        }
        TypeRef::Void | TypeRef::Array { .. } | TypeRef::Named { .. } | TypeRef::Fb { .. } => false,
    }
}

fn compare_types_compatible(left: &TypeRef, right: &TypeRef) -> bool {
    if left == right {
        return true;
    }
    if let (TypeRef::Scalar { ty: l }, TypeRef::Scalar { ty: r }) = (left, right) {
        (l.is_signed_int() && r.is_signed_int())
            || (l.is_unsigned_int() && r.is_unsigned_int())
            || (l.is_float() && r.is_float())
            || (l.is_bitstring() && r.is_bitstring())
            || matches!(
                (l, r),
                (ScalarType::String { .. }, ScalarType::String { .. })
            )
    } else {
        false
    }
}
