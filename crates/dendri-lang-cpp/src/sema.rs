//! Hardware pin reconciliation, ASL v2 lowering, static type checking, `F1` pack resolution,
//! and ADR-027 verbatim comment anchoring for `dendri-lang-cpp`.

use crate::board_pins::{
    default_adc_codec_id, default_analog_range, default_pwm_codec_id, is_builtin_pin_ident,
    is_known_port, resolve_pin_from_ident, resolve_pin_from_int, supports_analog_in,
    supports_interrupt, supports_pwm,
};
use crate::cpp_ast::{
    CppExpr, CppExprKind, CppForInit, CppForStep, CppStmt, CppStmtKind, CppTopItem, CppTypeSpec,
    CppUnit,
};
use crate::lexer::{LexedFile, Token};
use crate::types::{
    builtin_fb_kind, c_double_asl_type, cast_callee_name, cast_target_type_from_callee,
    is_int_like_type, is_valid_cpp_cast, resolve_primitive_cpp_type, types_compatible_cpp,
};
use dendri_ast::{
    BinaryOp, BoardPresentation, BoardProgram, CallArg, CasePattern, Comment, CommentAnchor,
    CommentOrigin, CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, ElifArm,
    EnumVariant, Expr, ExprKind, FbKind, ForeignFidelity, HardwareBinding, HardwareKind,
    InterruptEdge, LValue, Literal, NodeId, ObKind, ParamDecl, ParamDirection, Pou, PouBody,
    PouKind, PullMode, ScalarType, Stmt, StmtKind, StorageScope, StructField, SwitchCase, TypeDecl,
    TypeRef, UnaryOp,
};
use dendri_frontend_core::comments::{RawComment, contains_bidi_control};
use dendri_frontend_core::cursor::LineIndex;
use dendri_frontend_core::foreign::{ForeignLevel, ForeignSite, foreign_untyped_diagnostic};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, FrontendOutput, FrontendResult, LanguageId,
};
use dendri_span::{Diagnostic, ErrorCode, Span};
use dendri_stdlib::{ParamDir, StdCatalog, StdSymbol, StdTypeRef};
use std::collections::{BTreeMap, BTreeSet};

/// Lowers a parsed [`CppUnit`] into a validated [`BoardProgramBundle`] with reconciled hardware
/// bindings, static diagnostics, and verbatim ADR-027 comments.
pub(crate) fn lower_and_check(
    src: &str,
    unit: &CppUnit,
    lexed: &LexedFile,
    ctx: &BoardContext,
) -> FrontendResult<BoardProgramBundle> {
    let mut lowerer = Lowerer::new(src, ctx);
    let program = lowerer.run(unit);

    if !lowerer.errors.is_empty() {
        return Err(lowerer.errors);
    }

    let (comments, comment_diags) = lowerer.anchor_comments(lexed, &program);
    lowerer.warnings.extend(comment_diags);

    let mut presentation = BoardPresentation {
        eol_style: lexed.eol_style,
        blank_lines_before: BTreeMap::new(),
        origin_files: BTreeMap::new(),
        node_coords: BTreeMap::new(),
    };
    lowerer.record_blank_lines(&lexed.tokens, &mut presentation.blank_lines_before);

    let bundle = BoardProgramBundle {
        program,
        comments,
        assumptions: Vec::new(),
        presentation,
    };
    Ok(FrontendOutput::with_diagnostics(bundle, lowerer.warnings))
}

#[derive(Debug, Clone)]
struct PinCandidate {
    port: Option<String>,
    raw_int: Option<i128>,
    raw_ident: Option<String>,
    is_analog_ident: bool,
    span: Span,
}

#[derive(Debug, Clone)]
enum SymbolInfo {
    Data { ty: TypeRef, scope: StorageScope },
    Hardware { ty: TypeRef },
    Param { ty: TypeRef, writable: bool },
}

#[derive(Debug, Clone)]
struct AnchorRecord {
    id: NodeId,
    span: Span,
    start_line: u32,
    end_line: u32,
    is_scope_container: bool,
    body_span: Option<Span>,
}

struct Lowerer<'a> {
    src: &'a str,
    line_index: LineIndex,
    ctx: &'a BoardContext,
    std_catalog: StdCatalog,
    next_id: u32,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,

    // Symbol tables
    structs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    enums: BTreeMap<String, ScalarType>,
    enum_variants: BTreeMap<String, (String, i64)>,
    foreign_types: BTreeSet<String>,
    foreign_objects: BTreeMap<String, String>,
    user_fcs: BTreeMap<String, (Vec<ParamDecl>, TypeRef)>,
    pin_candidates: BTreeMap<String, PinCandidate>,
    consumed_pin_consts: BTreeSet<String>,
    hardware_by_label: BTreeMap<String, HardwareBinding>,
    hardware_order: Vec<String>,
    port_to_label: BTreeMap<String, String>,
    isr_bindings: BTreeMap<String, (String, InterruptEdge, u8)>,
    globals: BTreeMap<String, SymbolInfo>,

    // Comment anchoring records
    anchors: Vec<AnchorRecord>,
}

impl<'a> Lowerer<'a> {
    fn new(src: &'a str, ctx: &'a BoardContext) -> Self {
        let mut foreign_objects = BTreeMap::new();
        foreign_objects.insert("Wire".to_owned(), "Wire".to_owned());
        foreign_objects.insert("SPI".to_owned(), "SPI".to_owned());

        Self {
            src,
            line_index: LineIndex::new(src),
            ctx,
            std_catalog: ctx.std_catalog(),
            next_id: 1,
            errors: Vec::new(),
            warnings: Vec::new(),
            structs: BTreeMap::new(),
            enums: BTreeMap::new(),
            enum_variants: BTreeMap::new(),
            foreign_types: BTreeSet::new(),
            foreign_objects,
            user_fcs: BTreeMap::new(),
            pin_candidates: BTreeMap::new(),
            consumed_pin_consts: BTreeSet::new(),
            hardware_by_label: BTreeMap::new(),
            hardware_order: Vec::new(),
            port_to_label: BTreeMap::new(),
            isr_bindings: BTreeMap::new(),
            globals: BTreeMap::new(),
            anchors: Vec::new(),
        }
    }

    fn fresh_id(&mut self) -> NodeId {
        let id = NodeId::new(format!("n_{:04}", self.next_id));
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn record_anchor(
        &mut self,
        id: &NodeId,
        span: Span,
        is_scope_container: bool,
        body_span: Option<Span>,
    ) {
        let start_line = self.line_index.line_of(span.start);
        let end_line = self
            .line_index
            .line_of(span.end.saturating_sub(1).max(span.start));
        self.anchors.push(AnchorRecord {
            id: id.clone(),
            span,
            start_line,
            end_line,
            is_scope_container,
            body_span,
        });
    }

    fn push_err(&mut self, code: ErrorCode, span: Span, msg: impl Into<String>) {
        let mut diag = Diagnostic::new(code, msg).with_board_id(&self.ctx.board_id);
        diag.span = Some(span);
        self.errors.push(diag);
    }

    fn push_warn(&mut self, code: ErrorCode, span: Span, msg: impl Into<String>) {
        let mut diag = Diagnostic::new(code, msg).with_board_id(&self.ctx.board_id);
        diag.span = Some(span);
        self.warnings.push(diag);
    }

    fn slice_span(&self, span: Span) -> &'a str {
        span.slice(self.src).unwrap_or("")
    }

    fn run(&mut self, unit: &CppUnit) -> BoardProgram {
        let mut imports: Vec<String> = Vec::new();
        let mut seen_imports: BTreeSet<String> = BTreeSet::new();
        let mut types: Vec<TypeDecl> = Vec::new();

        // ------------------------------------------------------------------
        // Pass A: Imports, Types, Foreign decls, Pin candidates, Function headers
        // ------------------------------------------------------------------
        let mut seen_types: BTreeSet<String> = BTreeSet::new();
        let mut defined_fns: BTreeSet<String> = BTreeSet::new();

        for item in &unit.items {
            match item {
                CppTopItem::Include {
                    header,
                    is_angle,
                    raw,
                    span,
                } => {
                    if let Some(asl_import) = map_include_to_asl_import(header, *is_angle) {
                        if seen_imports.insert(asl_import.clone()) {
                            imports.push(asl_import);
                        }
                    } else {
                        let pack_hint = pack_hint_for_header(header);
                        let fidelity = pack_hint
                            .as_deref()
                            .and_then(|pid| {
                                self.ctx
                                    .loaded_packs
                                    .iter()
                                    .find(|p| p.pack.id == pid)
                                    .map(|p| p.fidelity())
                            })
                            .unwrap_or(ForeignFidelity::F0);
                        let id = self.fresh_id();
                        self.record_anchor(&id, *span, false, None);
                        types.push(TypeDecl::Foreign {
                            id,
                            name: Some(header.clone()),
                            language: "cpp".to_owned(),
                            fidelity,
                            text: raw.clone(),
                            pack_hint,
                            span: *span,
                        });
                    }
                }
                CppTopItem::Struct { name, fields, span } => {
                    if !seen_types.insert(name.clone()) {
                        self.push_err(
                            ErrorCode::ErrDuplicateDecl,
                            *span,
                            format!("duplicate struct declaration `{name}`"),
                        );
                        continue;
                    }
                    let mut asl_fields = Vec::new();
                    let mut field_map = BTreeMap::new();
                    let mut seen_fields = BTreeSet::new();
                    for f in fields {
                        if !seen_fields.insert(f.name.clone()) {
                            self.push_err(
                                ErrorCode::ErrDuplicateDecl,
                                f.span,
                                format!("duplicate field `{}` in struct `{name}`", f.name),
                            );
                            continue;
                        }
                        if let Some(ty) =
                            self.resolve_decl_type(&f.ty, f.array_len.as_ref(), false, f.span)
                        {
                            field_map.insert(f.name.clone(), ty.clone());
                            asl_fields.push(StructField {
                                name: f.name.clone(),
                                ty,
                            });
                        }
                    }
                    self.structs.insert(name.clone(), field_map);
                    let id = self.fresh_id();
                    self.record_anchor(&id, *span, false, None);
                    types.push(TypeDecl::Struct {
                        id,
                        name: name.clone(),
                        fields: asl_fields,
                        span: *span,
                    });
                }
                CppTopItem::Enum {
                    name,
                    backing,
                    variants,
                    span,
                } => {
                    if !seen_types.insert(name.clone()) {
                        self.push_err(
                            ErrorCode::ErrDuplicateDecl,
                            *span,
                            format!("duplicate enum declaration `{name}`"),
                        );
                        continue;
                    }
                    let backing_scalar = if let Some(b_spec) = backing {
                        match self.resolve_type_spec(b_spec) {
                            Some(TypeRef::Scalar { ty }) => ty,
                            Some(other) => {
                                self.push_err(
                                    ErrorCode::ErrTypeMismatch,
                                    b_spec.span,
                                    format!("enum `{name}` backing type must be integer, found `{other}`"),
                                );
                                self.ctx.c_int_asl_type()
                            }
                            None => self.ctx.c_int_asl_type(),
                        }
                    } else {
                        self.ctx.c_int_asl_type()
                    };
                    let mut asl_variants = Vec::new();
                    let mut next_val: i64 = 0;
                    for v in variants {
                        let val = if let Some(expr) = &v.value {
                            if let Some(n) = eval_const_cpp_int(expr, &self.enum_variants) {
                                i64::try_from(n).unwrap_or(0)
                            } else {
                                self.push_err(
                                    ErrorCode::ErrTypeMismatch,
                                    v.span,
                                    format!(
                                        "enum variant `{}` requires constant integer value",
                                        v.name
                                    ),
                                );
                                next_val
                            }
                        } else {
                            next_val
                        };
                        next_val = val.saturating_add(1);
                        self.enum_variants
                            .insert(v.name.clone(), (name.clone(), val));
                        self.enum_variants
                            .insert(format!("{name}::{}", v.name), (name.clone(), val));
                        asl_variants.push(EnumVariant {
                            name: v.name.clone(),
                            value: val,
                        });
                    }
                    self.enums.insert(name.clone(), backing_scalar.clone());
                    let id = self.fresh_id();
                    self.record_anchor(&id, *span, false, None);
                    types.push(TypeDecl::Enum {
                        id,
                        name: name.clone(),
                        backing: backing_scalar,
                        variants: asl_variants,
                        span: *span,
                    });
                }
                CppTopItem::ForeignDecl {
                    name,
                    fidelity,
                    text,
                    pack_hint,
                    instance_binding,
                    span,
                } => {
                    let resolved_fidelity = pack_hint
                        .as_deref()
                        .and_then(|pid| {
                            self.ctx
                                .loaded_packs
                                .iter()
                                .find(|p| p.pack.id == pid)
                                .map(|p| p.fidelity())
                        })
                        .unwrap_or(*fidelity);
                    if let Some((var_name, class_name)) = instance_binding {
                        self.foreign_objects
                            .insert(var_name.clone(), class_name.clone());
                    } else if let Some(n) = name {
                        self.foreign_types.insert(n.clone());
                    }
                    let id = self.fresh_id();
                    self.record_anchor(&id, *span, false, None);
                    types.push(TypeDecl::Foreign {
                        id,
                        name: name.clone(),
                        language: "cpp".to_owned(),
                        fidelity: resolved_fidelity,
                        text: text.clone(),
                        pack_hint: pack_hint.clone(),
                        span: *span,
                    });
                }
                CppTopItem::DefineConst { name, expr, span } => {
                    if let Some(cand) = self.extract_pin_candidate(expr, *span) {
                        self.pin_candidates.insert(name.clone(), cand);
                    }
                }
                CppTopItem::GlobalVar {
                    is_const,
                    is_constexpr,
                    array_len,
                    name,
                    initial,
                    span,
                    ..
                } => {
                    if (*is_const || *is_constexpr)
                        && array_len.is_none()
                        && let Some(init_expr) = initial
                        && let Some(cand) = self.extract_pin_candidate(init_expr, *span)
                    {
                        self.pin_candidates.insert(name.clone(), cand);
                    }
                }
                CppTopItem::FunctionProto { .. } => {}
                CppTopItem::Function {
                    return_ty,
                    name,
                    params,
                    span,
                    ..
                } => {
                    if !defined_fns.insert(name.clone()) {
                        self.push_err(
                            ErrorCode::ErrDuplicateDecl,
                            *span,
                            format!("duplicate function definition `{name}`"),
                        );
                        continue;
                    }
                    let ret = self.resolve_type_spec(return_ty).unwrap_or(TypeRef::Void);
                    let mut param_decls = Vec::new();
                    for p in params {
                        let pty = self
                            .resolve_decl_type(&p.ty, p.array_len.as_ref(), true, p.span)
                            .unwrap_or(TypeRef::scalar(self.ctx.c_int_asl_type()));
                        let dir = if p.ty.is_ref {
                            ParamDirection::InOut
                        } else {
                            ParamDirection::In
                        };
                        param_decls.push(ParamDecl {
                            name: p.name.clone(),
                            direction: dir,
                            ty: pty,
                        });
                    }
                    self.user_fcs.insert(name.clone(), (param_decls, ret));
                }
            }
        }

        // ------------------------------------------------------------------
        // Pass B: Reconcile Hardware I/O Tag Table & `attachInterrupt` ISRs
        // ------------------------------------------------------------------
        for item in &unit.items {
            if let CppTopItem::Function { body, .. } = item {
                self.scan_stmts_for_hardware(body, &defined_fns);
            }
        }
        // Any `const uint8_t pot = A0;` initialized to an explicit analog pin reconciles as AnalogIn
        let analog_candidates: Vec<(String, PinCandidate)> = self
            .pin_candidates
            .iter()
            .filter(|(name, c)| c.is_analog_ident && !self.consumed_pin_consts.contains(*name))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (label, cand) in analog_candidates {
            if let Some(port) = cand.port
                && supports_analog_in(&port, self.ctx)
            {
                self.consumed_pin_consts.insert(label.clone());
                self.register_hardware_binding(
                    &label,
                    &port,
                    HardwareKind::AnalogIn,
                    PullMode::None,
                    default_adc_codec_id(self.ctx),
                    Some(default_analog_range(self.ctx)),
                    cand.span,
                );
            }
        }

        // Materialize ordered `hardware` vector in source-declaration order and register in `globals`
        let mut hardware: Vec<HardwareBinding> = Vec::with_capacity(self.hardware_order.len());
        let order_clone = self.hardware_order.clone();
        for label in &order_clone {
            if let Some(hw) = self.hardware_by_label.get(label).cloned() {
                let hw_ty = hardware_binding_asl_type(&hw);
                self.globals
                    .insert(hw.label.clone(), SymbolInfo::Hardware { ty: hw_ty });
                hardware.push(hw);
            }
        }
        hardware.sort_by_key(|h| h.span.start);

        // Check name collisions between hardware labels and types/POUs
        for td in &types {
            let td_name = match td {
                TypeDecl::Struct { name, span, .. } | TypeDecl::Enum { name, span, .. } => {
                    Some((name.as_str(), *span))
                }
                TypeDecl::Foreign { .. } => None,
            };
            if let Some((n, sp)) = td_name
                && self.hardware_by_label.contains_key(n)
            {
                self.push_err(
                    ErrorCode::ErrNameCollision,
                    sp,
                    format!("type `{n}` collides with hardware pin label `{n}`"),
                );
            }
        }

        // ------------------------------------------------------------------
        // Pass C: Lower Global Data Declarations (excluding hardware pin constants)
        // ------------------------------------------------------------------
        let mut data: Vec<DataDecl> = Vec::new();
        let mut seen_globals: BTreeSet<String> = BTreeSet::new();

        for item in &unit.items {
            match item {
                CppTopItem::DefineConst { name, expr, span } => {
                    if self.consumed_pin_consts.contains(name) {
                        continue;
                    }
                    if self.hardware_by_label.contains_key(name) {
                        self.push_err(
                            ErrorCode::ErrNameCollision,
                            *span,
                            format!("`#define {name}` collides with hardware pin label `{name}`"),
                        );
                        continue;
                    }
                    if !seen_globals.insert(name.clone()) {
                        self.push_err(
                            ErrorCode::ErrDuplicateDecl,
                            *span,
                            format!("duplicate global constant `{name}`"),
                        );
                        continue;
                    }
                    let empty_locals = Vec::new();
                    let (asl_expr, inferred_ty) = self.lower_expr(expr, None, &empty_locals);
                    let ty =
                        inferred_ty.unwrap_or_else(|| TypeRef::scalar(self.ctx.c_int_asl_type()));
                    let id = self.fresh_id();
                    self.record_anchor(&id, *span, false, None);
                    self.globals.insert(
                        name.clone(),
                        SymbolInfo::Data {
                            ty: ty.clone(),
                            scope: StorageScope::Const,
                        },
                    );
                    data.push(DataDecl {
                        id,
                        name: name.clone(),
                        scope: StorageScope::Const,
                        ty,
                        initial: Some(asl_expr),
                        span: *span,
                    });
                }
                CppTopItem::GlobalVar {
                    is_const,
                    is_constexpr,
                    ty,
                    name,
                    array_len,
                    initial,
                    span,
                } => {
                    if self.consumed_pin_consts.contains(name) {
                        continue;
                    }
                    if self.hardware_by_label.contains_key(name) {
                        self.push_err(
                            ErrorCode::ErrNameCollision,
                            *span,
                            format!("variable `{name}` collides with hardware pin label `{name}`"),
                        );
                        continue;
                    }
                    if !seen_globals.insert(name.clone()) {
                        self.push_err(
                            ErrorCode::ErrDuplicateDecl,
                            *span,
                            format!("duplicate global variable `{name}`"),
                        );
                        continue;
                    }
                    let Some(resolved_ty) =
                        self.resolve_decl_type(ty, array_len.as_ref(), false, *span)
                    else {
                        continue;
                    };
                    let scope = if *is_const || *is_constexpr {
                        StorageScope::Const
                    } else {
                        StorageScope::Var
                    };
                    let empty_locals = Vec::new();
                    let init_asl = initial.as_ref().map(|e| {
                        let (lowered, init_ty) =
                            self.lower_expr(e, Some(&resolved_ty), &empty_locals);
                        if let Some(actual) = init_ty
                            && !types_compatible_cpp(&resolved_ty, &actual)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                e.span,
                                format!(
                                    "initializer for `{name}` has type `{actual}`, expected `{resolved_ty}`"
                                ),
                            );
                        }
                        lowered
                    });
                    let id = self.fresh_id();
                    self.record_anchor(&id, *span, false, None);
                    self.globals.insert(
                        name.clone(),
                        SymbolInfo::Data {
                            ty: resolved_ty.clone(),
                            scope,
                        },
                    );
                    data.push(DataDecl {
                        id,
                        name: name.clone(),
                        scope,
                        ty: resolved_ty,
                        initial: init_asl,
                        span: *span,
                    });
                }
                CppTopItem::Include { .. }
                | CppTopItem::Struct { .. }
                | CppTopItem::Enum { .. }
                | CppTopItem::FunctionProto { .. }
                | CppTopItem::Function { .. }
                | CppTopItem::ForeignDecl { .. } => {}
            }
        }

        // ------------------------------------------------------------------
        // Pass D: Lower All POUs (setup -> OB_STARTUP, loop -> OB_MAIN, ISR -> OB_HW_INT, helpers -> FC)
        // ------------------------------------------------------------------
        let mut pous: Vec<Pou> = Vec::new();
        for item in &unit.items {
            if let CppTopItem::Function {
                return_ty: _,
                name,
                params: _,
                body,
                body_span,
                span,
            } = item
            {
                if self.hardware_by_label.contains_key(name) {
                    self.push_err(
                        ErrorCode::ErrNameCollision,
                        *span,
                        format!("function `{name}` collides with hardware pin label `{name}`"),
                    );
                }
                let pou = self.lower_function(name, body, *body_span, *span);
                pous.push(pou);
            }
        }

        BoardProgram {
            imports,
            hardware,
            types,
            data,
            modules: Vec::new(),
            pous,
        }
    }

    fn extract_pin_candidate(&self, expr: &CppExpr, span: Span) -> Option<PinCandidate> {
        match &expr.kind {
            CppExprKind::IntLit { value, .. } => Some(PinCandidate {
                port: resolve_pin_from_int(*value, self.ctx),
                raw_int: Some(*value),
                raw_ident: None,
                is_analog_ident: false,
                span,
            }),
            CppExprKind::Ident(ident) => {
                if let Some(port) = resolve_pin_from_ident(ident, self.ctx) {
                    let is_analog_ident = is_builtin_pin_ident(ident) && port.starts_with('A');
                    Some(PinCandidate {
                        port: Some(port),
                        raw_int: None,
                        raw_ident: Some(ident.clone()),
                        is_analog_ident,
                        span,
                    })
                } else {
                    self.pin_candidates.get(ident).cloned()
                }
            }
            CppExprKind::BoolLit(_)
            | CppExprKind::FloatLit { .. }
            | CppExprKind::CharLit(_)
            | CppExprKind::StringLit(_)
            | CppExprKind::ScopedIdent { .. }
            | CppExprKind::Index { .. }
            | CppExprKind::Field { .. }
            | CppExprKind::Call { .. }
            | CppExprKind::MethodCall { .. }
            | CppExprKind::DesignatedCall { .. }
            | CppExprKind::Unary { .. }
            | CppExprKind::Binary { .. }
            | CppExprKind::Ternary { .. }
            | CppExprKind::Cast { .. }
            | CppExprKind::Foreign { .. } => None,
        }
    }

    fn scan_stmts_for_hardware(&mut self, stmts: &[CppStmt], defined_fns: &BTreeSet<String>) {
        for stmt in stmts {
            match &stmt.kind {
                CppStmtKind::LocalVar { initial, .. } => {
                    if let Some(e) = initial {
                        self.scan_expr_for_hardware(e);
                    }
                }
                CppStmtKind::Block { stmts: inner } => {
                    self.scan_stmts_for_hardware(inner, defined_fns);
                }
                CppStmtKind::Assign { target, value, .. } => {
                    self.scan_expr_for_hardware(target);
                    self.scan_expr_for_hardware(value);
                }
                CppStmtKind::IncDec { target, .. } => {
                    self.scan_expr_for_hardware(target);
                }
                CppStmtKind::ExprStmt { expr } => {
                    if let CppExprKind::Call { callee, args } = &expr.kind {
                        if callee == "pinMode" {
                            self.handle_pin_mode_scan(args, stmt.span);
                            continue;
                        }
                        if callee == "attachInterrupt" {
                            self.handle_attach_interrupt_scan(args, defined_fns, stmt.span);
                            continue;
                        }
                    }
                    self.scan_expr_for_hardware(expr);
                }
                CppStmtKind::If {
                    condition,
                    then_body,
                    elif_arms,
                    else_body,
                } => {
                    self.scan_expr_for_hardware(condition);
                    self.scan_stmts_for_hardware(then_body, defined_fns);
                    for (c, b) in elif_arms {
                        self.scan_expr_for_hardware(c);
                        self.scan_stmts_for_hardware(b, defined_fns);
                    }
                    self.scan_stmts_for_hardware(else_body, defined_fns);
                }
                CppStmtKind::Switch {
                    scrutinee,
                    cases,
                    default,
                } => {
                    self.scan_expr_for_hardware(scrutinee);
                    for c in cases {
                        self.scan_stmts_for_hardware(&c.body, defined_fns);
                    }
                    if let Some(def) = default {
                        self.scan_stmts_for_hardware(def, defined_fns);
                    }
                }
                CppStmtKind::While { condition, body }
                | CppStmtKind::DoWhile { body, condition } => {
                    self.scan_expr_for_hardware(condition);
                    self.scan_stmts_for_hardware(body, defined_fns);
                }
                CppStmtKind::For {
                    init,
                    condition,
                    step,
                    body,
                } => {
                    if let Some(i) = init {
                        match i.as_ref() {
                            CppForInit::Decl { init: e, .. }
                            | CppForInit::Assign { init: e, .. } => {
                                self.scan_expr_for_hardware(e);
                            }
                        }
                    }
                    if let Some(c) = condition {
                        self.scan_expr_for_hardware(c);
                    }
                    if let Some(s) = step {
                        match s.as_ref() {
                            CppForStep::Compound { rhs, .. } | CppForStep::Assign { rhs, .. } => {
                                self.scan_expr_for_hardware(rhs);
                            }
                            CppForStep::Inc { .. } | CppForStep::Dec { .. } => {}
                        }
                    }
                    self.scan_stmts_for_hardware(body, defined_fns);
                }
                CppStmtKind::Return { value } => {
                    if let Some(e) = value {
                        self.scan_expr_for_hardware(e);
                    }
                }
                CppStmtKind::Break | CppStmtKind::Continue | CppStmtKind::Foreign { .. } => {}
            }
        }
    }

    fn scan_expr_for_hardware(&mut self, expr: &CppExpr) {
        match &expr.kind {
            CppExprKind::Call { callee, args } => {
                match callee.as_str() {
                    "digitalRead" => {
                        if let Some(pin_arg) = args.first()
                            && let Some((label, port)) = self.resolve_pin_arg(pin_arg)
                            && !self.hardware_by_label.contains_key(&label)
                        {
                            self.register_hardware_binding(
                                &label,
                                &port,
                                HardwareKind::DigitalIn,
                                PullMode::None,
                                None,
                                None,
                                pin_arg.span,
                            );
                        }
                    }
                    "digitalWrite" => {
                        if let Some(pin_arg) = args.first()
                            && let Some((label, port)) = self.resolve_pin_arg(pin_arg)
                            && !self.hardware_by_label.contains_key(&label)
                        {
                            self.register_hardware_binding(
                                &label,
                                &port,
                                HardwareKind::DigitalOut,
                                PullMode::None,
                                None,
                                None,
                                pin_arg.span,
                            );
                        }
                    }
                    "analogRead" => {
                        if let Some(pin_arg) = args.first()
                            && let Some((label, port)) = self.resolve_pin_arg(pin_arg)
                        {
                            if !supports_analog_in(&port, self.ctx) {
                                self.push_err(
                                    ErrorCode::ErrPinCapabilityMismatch,
                                    pin_arg.span,
                                    format!(
                                        "pin `{port}` on `{}` does not support `analogRead` (analog_in capability required)",
                                        self.ctx.profile_id
                                    ),
                                );
                            } else {
                                self.register_hardware_binding(
                                    &label,
                                    &port,
                                    HardwareKind::AnalogIn,
                                    PullMode::None,
                                    default_adc_codec_id(self.ctx),
                                    Some(default_analog_range(self.ctx)),
                                    pin_arg.span,
                                );
                            }
                        }
                    }
                    "analogWrite" => {
                        if let Some(pin_arg) = args.first()
                            && let Some((label, port)) = self.resolve_pin_arg(pin_arg)
                        {
                            if !supports_pwm(&port, self.ctx) {
                                self.push_err(
                                    ErrorCode::ErrPinCapabilityMismatch,
                                    pin_arg.span,
                                    format!(
                                        "pin `{port}` on `{}` does not support `analogWrite` (pwm capability required)",
                                        self.ctx.profile_id
                                    ),
                                );
                            } else {
                                self.register_hardware_binding(
                                    &label,
                                    &port,
                                    HardwareKind::PwmOut,
                                    PullMode::None,
                                    default_pwm_codec_id(self.ctx),
                                    None,
                                    pin_arg.span,
                                );
                            }
                        }
                    }
                    _ => {}
                }
                for a in args {
                    self.scan_expr_for_hardware(a);
                }
            }
            CppExprKind::MethodCall { receiver, args, .. } => {
                self.scan_expr_for_hardware(receiver);
                for a in args {
                    self.scan_expr_for_hardware(a);
                }
            }
            CppExprKind::DesignatedCall { fields, .. } => {
                for (_, e) in fields {
                    self.scan_expr_for_hardware(e);
                }
            }
            CppExprKind::Index { base, index } => {
                self.scan_expr_for_hardware(base);
                self.scan_expr_for_hardware(index);
            }
            CppExprKind::Field { base, .. }
            | CppExprKind::Unary { operand: base, .. }
            | CppExprKind::Cast { operand: base, .. } => {
                self.scan_expr_for_hardware(base);
            }
            CppExprKind::Binary { left, right, .. } => {
                self.scan_expr_for_hardware(left);
                self.scan_expr_for_hardware(right);
            }
            CppExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                self.scan_expr_for_hardware(condition);
                self.scan_expr_for_hardware(then_expr);
                self.scan_expr_for_hardware(else_expr);
            }
            CppExprKind::BoolLit(_)
            | CppExprKind::IntLit { .. }
            | CppExprKind::FloatLit { .. }
            | CppExprKind::CharLit(_)
            | CppExprKind::StringLit(_)
            | CppExprKind::Ident(_)
            | CppExprKind::ScopedIdent { .. }
            | CppExprKind::Foreign { .. } => {}
        }
    }

    fn handle_pin_mode_scan(&mut self, args: &[CppExpr], span: Span) {
        if args.len() != 2 {
            self.push_err(
                ErrorCode::ErrArityMismatch,
                span,
                format!(
                    "`pinMode` expects 2 arguments (pin, mode), found {}",
                    args.len()
                ),
            );
            return;
        }
        let (Some(pin_arg), Some(mode_arg)) = (args.first(), args.get(1)) else {
            return;
        };
        let Some((label, port)) = self.resolve_pin_arg(pin_arg) else {
            return;
        };
        let (kind, pull) = if let CppExprKind::Ident(m) = &mode_arg.kind {
            match m.as_str() {
                "OUTPUT" => (HardwareKind::DigitalOut, PullMode::None),
                "INPUT" => (HardwareKind::DigitalIn, PullMode::None),
                "INPUT_PULLUP" => (HardwareKind::DigitalIn, PullMode::PullUp),
                "INPUT_PULLDOWN" => (HardwareKind::DigitalIn, PullMode::PullDown),
                _ => {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        mode_arg.span,
                        "invalid `pinMode` mode (expected `INPUT`, `OUTPUT`, or `INPUT_PULLUP`)",
                    );
                    return;
                }
            }
        } else {
            self.push_err(
                ErrorCode::ErrTypeMismatch,
                mode_arg.span,
                "invalid `pinMode` mode (expected `INPUT`, `OUTPUT`, or `INPUT_PULLUP`)",
            );
            return;
        };
        self.register_hardware_binding(&label, &port, kind, pull, None, None, pin_arg.span);
    }

    fn handle_attach_interrupt_scan(
        &mut self,
        args: &[CppExpr],
        defined_fns: &BTreeSet<String>,
        span: Span,
    ) {
        if args.len() != 3 {
            self.push_err(
                ErrorCode::ErrArityMismatch,
                span,
                format!(
                    "`attachInterrupt` expects 3 arguments (interrupt, isr, mode), found {}",
                    args.len()
                ),
            );
            return;
        }
        let (Some(int_arg), Some(isr_arg), Some(mode_arg)) =
            (args.first(), args.get(1), args.get(2))
        else {
            return;
        };

        // Unwrap `digitalPinToInterrupt(pin)` or accept bare `pin`
        let pin_expr = if let CppExprKind::Call {
            callee,
            args: iargs,
        } = &int_arg.kind
            && callee == "digitalPinToInterrupt"
        {
            if let Some(inner) = iargs.first() {
                inner
            } else {
                self.push_err(
                    ErrorCode::ErrArityMismatch,
                    int_arg.span,
                    "`digitalPinToInterrupt` expects 1 pin argument",
                );
                return;
            }
        } else {
            int_arg
        };

        let Some((label, port)) = self.resolve_pin_arg(pin_expr) else {
            return;
        };
        if !supports_interrupt(&port, self.ctx) {
            self.push_err(
                ErrorCode::ErrPinCapabilityMismatch,
                pin_expr.span,
                format!(
                    "pin `{port}` on `{}` does not support external hardware interrupts (`attachInterrupt`)",
                    self.ctx.profile_id
                ),
            );
            return;
        }
        if !self.hardware_by_label.contains_key(&label) {
            self.register_hardware_binding(
                &label,
                &port,
                HardwareKind::DigitalIn,
                PullMode::None,
                None,
                None,
                pin_expr.span,
            );
        }

        let isr_name = if let CppExprKind::Ident(n) = &isr_arg.kind {
            n.clone()
        } else {
            self.push_err(
                ErrorCode::ErrUnresolvedIsr,
                isr_arg.span,
                "`attachInterrupt` ISR argument must name a defined function",
            );
            return;
        };
        if !defined_fns.contains(&isr_name) {
            self.push_err(
                ErrorCode::ErrUnresolvedIsr,
                isr_arg.span,
                format!("`attachInterrupt` references undefined ISR function `{isr_name}`"),
            );
            return;
        }

        let edge = if let CppExprKind::Ident(m) = &mode_arg.kind {
            match m.as_str() {
                "RISING" => InterruptEdge::Rising,
                "FALLING" => InterruptEdge::Falling,
                "CHANGE" => InterruptEdge::Change,
                "LOW" => InterruptEdge::Low,
                "HIGH" => InterruptEdge::High,
                _ => {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        mode_arg.span,
                        "invalid `attachInterrupt` trigger mode (expected `RISING`, `FALLING`, `CHANGE`, `LOW`, or `HIGH`)",
                    );
                    return;
                }
            }
        } else if let CppExprKind::BoolLit(b) = &mode_arg.kind {
            if *b {
                InterruptEdge::High
            } else {
                InterruptEdge::Low
            }
        } else {
            self.push_err(
                ErrorCode::ErrTypeMismatch,
                mode_arg.span,
                "invalid `attachInterrupt` trigger mode (expected `RISING`, `FALLING`, `CHANGE`, `LOW`, or `HIGH`)",
            );
            return;
        };

        let priority = u8::try_from(self.isr_bindings.len().saturating_add(1)).unwrap_or(u8::MAX);
        self.isr_bindings.insert(isr_name, (label, edge, priority));
    }

    fn resolve_pin_arg(&mut self, expr: &CppExpr) -> Option<(String, String)> {
        if let CppExprKind::IntLit { value, .. } = &expr.kind {
            if let Some(port) = resolve_pin_from_int(*value, self.ctx) {
                Some((port.clone(), port))
            } else {
                self.push_err(
                    ErrorCode::ErrUnknownPort,
                    expr.span,
                    format!(
                        "unknown pin number `{value}` on board profile `{}`",
                        self.ctx.profile_id
                    ),
                );
                None
            }
        } else if let CppExprKind::Ident(name) = &expr.kind {
            if let Some(cand) = self.pin_candidates.get(name).cloned() {
                self.consumed_pin_consts.insert(name.clone());
                if let Some(port) = cand.port {
                    return Some((name.clone(), port));
                }
                let detail = cand
                    .raw_int
                    .map(|n| n.to_string())
                    .or(cand.raw_ident)
                    .unwrap_or_else(|| name.clone());
                self.push_err(
                    ErrorCode::ErrUnknownPort,
                    expr.span,
                    format!(
                        "pin constant `{name}` refers to unknown port `{detail}` on `{}`",
                        self.ctx.profile_id
                    ),
                );
                return None;
            }
            if let Some(port) = resolve_pin_from_ident(name, self.ctx) {
                return Some((name.clone(), port));
            }
            if let Some(existing) = self.hardware_by_label.get(name) {
                return Some((name.clone(), existing.pin.clone()));
            }
            self.push_err(
                ErrorCode::ErrUnknownPort,
                expr.span,
                format!(
                    "unknown hardware pin `{name}` on board profile `{}`",
                    self.ctx.profile_id
                ),
            );
            None
        } else {
            self.push_err(
                ErrorCode::ErrUnknownPort,
                expr.span,
                "pin argument must be a pin constant, integer literal, or board port identifier",
            );
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn register_hardware_binding(
        &mut self,
        label: &str,
        port: &str,
        kind: HardwareKind,
        pull: PullMode,
        codec: Option<String>,
        range: Option<dendri_ast::ChannelRange>,
        span: Span,
    ) {
        if !is_known_port(port, self.ctx) {
            self.push_err(
                ErrorCode::ErrUnknownPort,
                span,
                format!(
                    "unknown port `{port}` on board profile `{}`",
                    self.ctx.profile_id
                ),
            );
            return;
        }
        if let Some(owner) = self.port_to_label.get(port)
            && owner != label
        {
            self.push_err(
                ErrorCode::ErrHardwarePinConflict,
                span,
                format!(
                    "physical pin `{port}` is bound to conflicting labels `{owner}` and `{label}`"
                ),
            );
            return;
        }
        if let Some(existing) = self.hardware_by_label.get_mut(label) {
            // Upgrade DigitalOut -> PwmOut when `pinMode(pin, OUTPUT)` + `analogWrite(pin, val)` both appear
            if kind == HardwareKind::PwmOut {
                existing.kind = HardwareKind::PwmOut;
                existing.codec = codec;
            } else if kind == HardwareKind::AnalogIn {
                existing.kind = HardwareKind::AnalogIn;
                existing.codec = codec;
                existing.range = range;
            } else if existing.kind == HardwareKind::DigitalIn && pull != PullMode::None {
                existing.pull = pull;
            } else if existing.kind == HardwareKind::DigitalIn && kind == HardwareKind::DigitalOut {
                existing.kind = HardwareKind::DigitalOut;
            }
            return;
        }

        let id = self.fresh_id();
        let binding_span = self.pin_candidates.get(label).map_or(span, |c| c.span);
        self.record_anchor(&id, binding_span, false, None);
        self.port_to_label.insert(port.to_owned(), label.to_owned());
        self.hardware_order.push(label.to_owned());
        self.hardware_by_label.insert(
            label.to_owned(),
            HardwareBinding {
                id,
                label: label.to_owned(),
                pin: port.to_owned(),
                kind,
                pull,
                active_low: false,
                codec,
                range,
                span: binding_span,
            },
        );
    }

    fn resolve_type_spec(&mut self, spec: &CppTypeSpec) -> Option<TypeRef> {
        // `const char*` -> `STRING[255]`
        if spec.is_pointer
            && spec.words.len() == 1
            && spec.words.first().map(String::as_str) == Some("char")
        {
            return Some(TypeRef::scalar(ScalarType::String { max_len: 255 }));
        }

        let word_refs: Vec<&str> = spec.words.iter().map(String::as_str).collect();
        if let Some(prim) = resolve_primitive_cpp_type(&word_refs, self.ctx) {
            return Some(prim);
        }
        if word_refs == ["size_t"] {
            return Some(TypeRef::scalar(self.ctx.c_uint_asl_type()));
        }
        if spec.words.len() == 1
            && let Some(single) = spec.words.first()
        {
            if let Some(fb_kind) = builtin_fb_kind(single) {
                return Some(TypeRef::Fb { fb_kind });
            }
            if self.structs.contains_key(single)
                || self.enums.contains_key(single)
                || self.foreign_types.contains(single)
            {
                return Some(TypeRef::Named {
                    name: single.clone(),
                });
            }
            self.push_err(
                ErrorCode::ErrUnknownType,
                spec.span,
                format!("unknown C++ type `{single}`"),
            );
            return None;
        }
        self.push_err(
            ErrorCode::ErrUnknownType,
            spec.span,
            format!("unknown C++ type `{}`", spec.words.join(" ")),
        );
        None
    }

    fn resolve_decl_type(
        &mut self,
        spec: &CppTypeSpec,
        array_len: Option<&CppExpr>,
        is_param: bool,
        span: Span,
    ) -> Option<TypeRef> {
        let base = self.resolve_type_spec(spec)?;
        let Some(len_expr) = array_len else {
            return Some(base);
        };
        let Some(len_i128) = eval_const_cpp_int(len_expr, &self.enum_variants) else {
            self.push_err(
                ErrorCode::ErrArrayBoundsConst,
                len_expr.span,
                "array length must be a constant positive integer",
            );
            return None;
        };
        if len_i128 <= 0 || len_i128 > i128::from(u32::MAX) {
            self.push_err(
                ErrorCode::ErrArrayBoundsConst,
                len_expr.span,
                format!("invalid array length `{len_i128}` (must be >= 1)"),
            );
            return None;
        }
        let len = u32::try_from(len_i128).unwrap_or(1);
        // `char buf[N]` -> `STRING[N]` (except bare parameter array if desired; keep `STRING[N]` consistent!)
        let is_plain_char = spec.words.len() == 1
            && spec.words.first().map(String::as_str) == Some("char")
            && !is_param;
        if is_plain_char {
            let _ = span;
            return Some(TypeRef::scalar(ScalarType::String { max_len: len }));
        }
        Some(TypeRef::Array {
            elem: Box::new(base),
            len,
        })
    }

    fn lower_function(
        &mut self,
        fn_name: &str,
        body: &[CppStmt],
        body_span: Span,
        span: Span,
    ) -> Pou {
        let (params, return_type) = self
            .user_fcs
            .get(fn_name)
            .cloned()
            .unwrap_or_else(|| (Vec::new(), TypeRef::Void));

        let (pou_name, kind, in_isr) = if fn_name == "setup" {
            (
                "OB_STARTUP".to_owned(),
                PouKind::Ob {
                    ob: ObKind::Startup,
                },
                false,
            )
        } else if fn_name == "loop" {
            (
                "OB_MAIN".to_owned(),
                PouKind::Ob { ob: ObKind::Main },
                false,
            )
        } else if let Some((pin_label, edge, priority)) = self.isr_bindings.get(fn_name).cloned() {
            (
                fn_name.to_owned(),
                PouKind::Ob {
                    ob: ObKind::HwInt {
                        pin_label,
                        edge,
                        priority,
                    },
                },
                true,
            )
        } else {
            (
                fn_name.to_owned(),
                PouKind::Fc {
                    return_type: return_type.clone(),
                },
                false,
            )
        };

        let pou_id = self.fresh_id();
        self.record_anchor(&pou_id, span, true, Some(body_span));

        let mut scope_stack: Vec<BTreeMap<String, SymbolInfo>> = Vec::new();
        let mut param_scope = BTreeMap::new();
        for p in &params {
            param_scope.insert(
                p.name.clone(),
                SymbolInfo::Param {
                    ty: p.ty.clone(),
                    writable: !matches!(p.direction, ParamDirection::In),
                },
            );
        }
        scope_stack.push(param_scope);
        scope_stack.push(BTreeMap::new()); // function body lexical scope

        let mut pou_locals: Vec<DataDecl> = Vec::new();
        let mut fn_ctx = FnLowerCtx {
            pou_name: &pou_name,
            expected_return: &return_type,
            in_loop: false,
            in_isr,
            at_function_top: true,
        };

        let stmts = self.lower_stmts(body, &mut fn_ctx, &mut scope_stack, &mut pou_locals);

        Pou {
            id: pou_id,
            name: pou_name,
            kind,
            preferred_view: None,
            params,
            locals: pou_locals,
            body: PouBody::Statements(stmts),
            span,
        }
    }

    fn lower_stmts(
        &mut self,
        stmts: &[CppStmt],
        fn_ctx: &mut FnLowerCtx<'_>,
        scopes: &mut Vec<BTreeMap<String, SymbolInfo>>,
        pou_locals: &mut Vec<DataDecl>,
    ) -> Vec<Stmt> {
        let mut out = Vec::new();
        for stmt in stmts {
            let lowered = self.lower_stmt(stmt, fn_ctx, scopes, pou_locals);
            out.extend(lowered);
        }
        out
    }

    fn lower_stmt(
        &mut self,
        stmt: &CppStmt,
        fn_ctx: &mut FnLowerCtx<'_>,
        scopes: &mut Vec<BTreeMap<String, SymbolInfo>>,
        pou_locals: &mut Vec<DataDecl>,
    ) -> Vec<Stmt> {
        match &stmt.kind {
            CppStmtKind::LocalVar {
                is_static,
                is_const,
                is_auto,
                ty,
                name,
                array_len,
                initial,
            } => {
                // Check duplicate in current lexical scope (and parameter scope if at top block)
                let dup_in_current = scopes.last().is_some_and(|s| s.contains_key(name));
                let dup_in_params =
                    scopes.len() == 2 && scopes.first().is_some_and(|s| s.contains_key(name));
                if dup_in_current || dup_in_params {
                    self.push_err(
                        ErrorCode::ErrDuplicateDecl,
                        stmt.span,
                        format!("duplicate local variable declaration `{name}`"),
                    );
                    return Vec::new();
                }

                let scope = if *is_const {
                    StorageScope::Const
                } else if *is_static {
                    StorageScope::Var
                } else {
                    StorageScope::Temp
                };

                let (resolved_ty, init_lowered_and_ternary) = if *is_auto {
                    let Some(init_e) = initial else {
                        self.push_err(
                            ErrorCode::ErrUnknownType,
                            stmt.span,
                            format!("`auto` variable `{name}` requires an initializer"),
                        );
                        return Vec::new();
                    };
                    let (asl_init, inferred) = self.lower_expr(init_e, None, scopes);
                    let Some(inferred_ty) = inferred else {
                        return Vec::new();
                    };
                    (inferred_ty, Some((asl_init, false)))
                } else {
                    let Some(spec) = ty else {
                        return Vec::new();
                    };
                    let Some(rty) =
                        self.resolve_decl_type(spec, array_len.as_ref(), false, stmt.span)
                    else {
                        return Vec::new();
                    };
                    if let Some(init_e) = initial {
                        if let CppExprKind::Ternary {
                            condition,
                            then_expr,
                            else_expr,
                        } = &init_e.kind
                        {
                            // Lower `T x = c ? a : b;` to local `x` + `if (c) { x = a; } else { x = b; }`
                            let bool_ty = TypeRef::scalar(ScalarType::Bool);
                            let (c_asl, c_ty) = self.lower_expr(condition, Some(&bool_ty), scopes);
                            if let Some(ct) = c_ty
                                && !types_compatible_cpp(&bool_ty, &ct)
                            {
                                self.push_err(
                                    ErrorCode::ErrTypeMismatch,
                                    condition.span,
                                    format!("ternary condition must be BOOL, found `{ct}`"),
                                );
                            }
                            let (t_asl, t_ty) = self.lower_expr(then_expr, Some(&rty), scopes);
                            let (e_asl, e_ty) = self.lower_expr(else_expr, Some(&rty), scopes);
                            if let Some(tt) = t_ty
                                && !types_compatible_cpp(&rty, &tt)
                            {
                                self.push_err(
                                    ErrorCode::ErrTypeMismatch,
                                    then_expr.span,
                                    format!("ternary branch has type `{tt}`, expected `{rty}`"),
                                );
                            }
                            if let Some(et) = e_ty
                                && !types_compatible_cpp(&rty, &et)
                            {
                                self.push_err(
                                    ErrorCode::ErrTypeMismatch,
                                    else_expr.span,
                                    format!("ternary branch has type `{et}`, expected `{rty}`"),
                                );
                            }
                            let decl_id = self.fresh_id();
                            self.record_anchor(&decl_id, stmt.span, false, None);
                            if let Some(cur) = scopes.last_mut() {
                                cur.insert(
                                    name.clone(),
                                    SymbolInfo::Data {
                                        ty: rty.clone(),
                                        scope,
                                    },
                                );
                            }
                            pou_locals.push(DataDecl {
                                id: decl_id,
                                name: name.clone(),
                                scope,
                                ty: rty,
                                initial: None,
                                span: stmt.span,
                            });
                            let then_id = self.fresh_id();
                            let else_id = self.fresh_id();
                            let if_id = self.fresh_id();
                            self.record_anchor(&if_id, stmt.span, false, None);
                            fn_ctx.at_function_top = false;
                            return vec![Stmt::new(
                                if_id,
                                StmtKind::If {
                                    condition: c_asl,
                                    then_body: vec![Stmt::new(
                                        then_id,
                                        StmtKind::Assign {
                                            target: LValue::Var {
                                                name: name.clone(),
                                                span: stmt.span,
                                            },
                                            value: t_asl,
                                        },
                                        then_expr.span,
                                    )],
                                    elif_arms: Vec::new(),
                                    else_body: vec![Stmt::new(
                                        else_id,
                                        StmtKind::Assign {
                                            target: LValue::Var {
                                                name: name.clone(),
                                                span: stmt.span,
                                            },
                                            value: e_asl,
                                        },
                                        else_expr.span,
                                    )],
                                },
                                stmt.span,
                            )];
                        }
                        let (asl_init, actual_ty) = self.lower_expr(init_e, Some(&rty), scopes);
                        if let Some(actual) = actual_ty
                            && !types_compatible_cpp(&rty, &actual)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                init_e.span,
                                format!(
                                    "initializer for `{name}` has type `{actual}`, expected `{rty}`"
                                ),
                            );
                        }
                        (rty, Some((asl_init, false)))
                    } else {
                        (rty, None)
                    }
                };

                if let Some(cur) = scopes.last_mut() {
                    cur.insert(
                        name.clone(),
                        SymbolInfo::Data {
                            ty: resolved_ty.clone(),
                            scope,
                        },
                    );
                }

                // Keep initializer directly in `pou.locals` at function top or for `const`/`static`;
                // for mid-body/nested mutable locals, emit an assignment at statement site.
                let keep_in_decl = fn_ctx.at_function_top
                    || matches!(scope, StorageScope::Const | StorageScope::Var);
                let decl_id = self.fresh_id();
                self.record_anchor(&decl_id, stmt.span, false, None);

                if keep_in_decl {
                    pou_locals.push(DataDecl {
                        id: decl_id,
                        name: name.clone(),
                        scope,
                        ty: resolved_ty,
                        initial: init_lowered_and_ternary.map(|(e, _)| e),
                        span: stmt.span,
                    });
                    Vec::new()
                } else {
                    pou_locals.push(DataDecl {
                        id: decl_id,
                        name: name.clone(),
                        scope,
                        ty: resolved_ty,
                        initial: None,
                        span: stmt.span,
                    });
                    if let Some((init_val, _)) = init_lowered_and_ternary {
                        let assign_id = self.fresh_id();
                        self.record_anchor(&assign_id, stmt.span, false, None);
                        vec![Stmt::new(
                            assign_id,
                            StmtKind::Assign {
                                target: LValue::Var {
                                    name: name.clone(),
                                    span: stmt.span,
                                },
                                value: init_val,
                            },
                            stmt.span,
                        )]
                    } else {
                        Vec::new()
                    }
                }
            }
            CppStmtKind::Block { stmts: inner } => {
                fn_ctx.at_function_top = false;
                scopes.push(BTreeMap::new());
                let res = self.lower_stmts(inner, fn_ctx, scopes, pou_locals);
                let _ = scopes.pop();
                res
            }
            CppStmtKind::Assign { target, op, value } => {
                fn_ctx.at_function_top = false;
                let Some((lval, target_ty)) = self.lower_writable_lvalue(target, scopes) else {
                    let _ = self.lower_expr(value, None, scopes);
                    return Vec::new();
                };

                // Top-level plain assignment with ternary RHS lowers to `StmtKind::If`
                if op.is_none()
                    && let CppExprKind::Ternary {
                        condition,
                        then_expr,
                        else_expr,
                    } = &value.kind
                {
                    let bool_ty = TypeRef::scalar(ScalarType::Bool);
                    let (c_asl, c_ty) = self.lower_expr(condition, Some(&bool_ty), scopes);
                    if let Some(ct) = c_ty
                        && !types_compatible_cpp(&bool_ty, &ct)
                    {
                        self.push_err(
                            ErrorCode::ErrTypeMismatch,
                            condition.span,
                            format!("ternary condition must be BOOL, found `{ct}`"),
                        );
                    }
                    let (t_asl, t_ty) = self.lower_expr(then_expr, target_ty.as_ref(), scopes);
                    let (e_asl, e_ty) = self.lower_expr(else_expr, target_ty.as_ref(), scopes);
                    if let Some(ref tty) = target_ty {
                        if let Some(tt) = t_ty
                            && !types_compatible_cpp(tty, &tt)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                then_expr.span,
                                format!("cannot assign `{tt}` to target of type `{tty}`"),
                            );
                        }
                        if let Some(et) = e_ty
                            && !types_compatible_cpp(tty, &et)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                else_expr.span,
                                format!("cannot assign `{et}` to target of type `{tty}`"),
                            );
                        }
                    }
                    let then_id = self.fresh_id();
                    let else_id = self.fresh_id();
                    let if_id = self.fresh_id();
                    self.record_anchor(&if_id, stmt.span, false, None);
                    return vec![Stmt::new(
                        if_id,
                        StmtKind::If {
                            condition: c_asl,
                            then_body: vec![Stmt::new(
                                then_id,
                                StmtKind::Assign {
                                    target: lval.clone(),
                                    value: t_asl,
                                },
                                then_expr.span,
                            )],
                            elif_arms: Vec::new(),
                            else_body: vec![Stmt::new(
                                else_id,
                                StmtKind::Assign {
                                    target: lval,
                                    value: e_asl,
                                },
                                else_expr.span,
                            )],
                        },
                        stmt.span,
                    )];
                }

                let (rhs_asl, rhs_ty) = self.lower_expr(value, target_ty.as_ref(), scopes);
                if let (Some(tty), Some(rty)) = (&target_ty, &rhs_ty)
                    && !types_compatible_cpp(tty, rty)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        stmt.span,
                        format!("cannot assign `{rty}` to target of type `{tty}`"),
                    );
                }

                let final_val = if let Some(cop) = op {
                    let lhs_expr = lvalue_to_expr(&lval);
                    Expr::new(
                        ExprKind::Binary {
                            op: cop.to_binary_op(),
                            left: Box::new(lhs_expr),
                            right: Box::new(rhs_asl),
                        },
                        stmt.span,
                    )
                } else {
                    rhs_asl
                };

                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Assign {
                        target: lval,
                        value: final_val,
                    },
                    stmt.span,
                )]
            }
            CppStmtKind::IncDec {
                target,
                is_increment,
            } => {
                fn_ctx.at_function_top = false;
                let Some((lval, target_ty)) = self.lower_writable_lvalue(target, scopes) else {
                    return Vec::new();
                };
                if let Some(tty) = &target_ty
                    && !matches!(tty, TypeRef::Scalar { ty } if ty.is_plain_numeric())
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        stmt.span,
                        format!("increment/decrement requires numeric target, found `{tty}`"),
                    );
                }
                let lhs_expr = lvalue_to_expr(&lval);
                let op = if *is_increment {
                    BinaryOp::Add
                } else {
                    BinaryOp::Sub
                };
                let value = Expr::new(
                    ExprKind::Binary {
                        op,
                        left: Box::new(lhs_expr),
                        right: Box::new(Expr::new(ExprKind::Literal(Literal::Int(1)), stmt.span)),
                    },
                    stmt.span,
                );
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Assign {
                        target: lval,
                        value,
                    },
                    stmt.span,
                )]
            }
            CppStmtKind::ExprStmt { expr } => self.lower_expr_stmt(expr, stmt.span, fn_ctx, scopes),
            CppStmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                fn_ctx.at_function_top = false;
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                let (cond_asl, cond_ty) = self.lower_expr(condition, Some(&bool_ty), scopes);
                if let Some(ct) = cond_ty
                    && !types_compatible_cpp(&bool_ty, &ct)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("`if` condition must be BOOL, found `{ct}`"),
                    );
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, true, Some(stmt.span));

                scopes.push(BTreeMap::new());
                let then_stmts = self.lower_stmts(then_body, fn_ctx, scopes, pou_locals);
                let _ = scopes.pop();

                let mut asl_elifs = Vec::new();
                for (ec, eb) in elif_arms {
                    let (ec_asl, ec_ty) = self.lower_expr(ec, Some(&bool_ty), scopes);
                    if let Some(ct) = ec_ty
                        && !types_compatible_cpp(&bool_ty, &ct)
                    {
                        self.push_err(
                            ErrorCode::ErrTypeMismatch,
                            ec.span,
                            format!("`else if` condition must be BOOL, found `{ct}`"),
                        );
                    }
                    scopes.push(BTreeMap::new());
                    let eb_stmts = self.lower_stmts(eb, fn_ctx, scopes, pou_locals);
                    let _ = scopes.pop();
                    asl_elifs.push(ElifArm {
                        condition: ec_asl,
                        body: eb_stmts,
                    });
                }

                scopes.push(BTreeMap::new());
                let else_stmts = self.lower_stmts(else_body, fn_ctx, scopes, pou_locals);
                let _ = scopes.pop();

                vec![Stmt::new(
                    id,
                    StmtKind::If {
                        condition: cond_asl,
                        then_body: then_stmts,
                        elif_arms: asl_elifs,
                        else_body: else_stmts,
                    },
                    stmt.span,
                )]
            }
            CppStmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                fn_ctx.at_function_top = false;
                let (scrut_asl, scrut_ty) = self.lower_expr(scrutinee, None, scopes);
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, true, Some(stmt.span));

                let mut asl_cases = Vec::new();
                for c in cases {
                    let mut patterns = Vec::new();
                    for val_expr in &c.values {
                        let (pat_asl, pat_ty) =
                            self.lower_expr(val_expr, scrut_ty.as_ref(), scopes);
                        if let (Some(st), Some(pt)) = (&scrut_ty, &pat_ty)
                            && !types_compatible_cpp(st, pt)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                val_expr.span,
                                format!("switch case value `{pt}` does not match scrutinee `{st}`"),
                            );
                        }
                        patterns.push(CasePattern::Value(pat_asl));
                    }
                    scopes.push(BTreeMap::new());
                    let body_stmts = self.lower_stmts(&c.body, fn_ctx, scopes, pou_locals);
                    let _ = scopes.pop();
                    asl_cases.push(SwitchCase {
                        patterns,
                        body: body_stmts,
                    });
                }

                let asl_default = default.as_ref().map(|def_body| {
                    scopes.push(BTreeMap::new());
                    let b = self.lower_stmts(def_body, fn_ctx, scopes, pou_locals);
                    let _ = scopes.pop();
                    b
                });

                vec![Stmt::new(
                    id,
                    StmtKind::Switch {
                        scrutinee: scrut_asl,
                        cases: asl_cases,
                        default: asl_default,
                    },
                    stmt.span,
                )]
            }
            CppStmtKind::While { condition, body } => {
                fn_ctx.at_function_top = false;
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                let (cond_asl, cond_ty) = self.lower_expr(condition, Some(&bool_ty), scopes);
                if let Some(ct) = cond_ty
                    && !types_compatible_cpp(&bool_ty, &ct)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("`while` condition must be BOOL, found `{ct}`"),
                    );
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, true, Some(stmt.span));

                let prev_loop = fn_ctx.in_loop;
                fn_ctx.in_loop = true;
                scopes.push(BTreeMap::new());
                let body_stmts = self.lower_stmts(body, fn_ctx, scopes, pou_locals);
                let _ = scopes.pop();
                fn_ctx.in_loop = prev_loop;

                vec![Stmt::new(
                    id,
                    StmtKind::While {
                        condition: cond_asl,
                        body: body_stmts,
                    },
                    stmt.span,
                )]
            }
            CppStmtKind::DoWhile { body, condition } => {
                fn_ctx.at_function_top = false;
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                let (cond_asl, cond_ty) = self.lower_expr(condition, Some(&bool_ty), scopes);
                if let Some(ct) = cond_ty
                    && !types_compatible_cpp(&bool_ty, &ct)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("`do ... while` condition must be BOOL, found `{ct}`"),
                    );
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, true, Some(stmt.span));

                let prev_loop = fn_ctx.in_loop;
                fn_ctx.in_loop = true;
                scopes.push(BTreeMap::new());
                let body_stmts = self.lower_stmts(body, fn_ctx, scopes, pou_locals);
                let _ = scopes.pop();
                fn_ctx.in_loop = prev_loop;

                // Preserve exact do-while semantics: execute body, evaluate `cond` on every
                // direct `continue;` as well as at end of body, and break if `!cond`.
                let not_cond = Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(cond_asl),
                    },
                    condition.span,
                );
                let mut body_stmts =
                    self.rewrite_do_while_continues(body_stmts, &not_cond, condition.span);
                let break_id = self.fresh_id();
                let guard_id = self.fresh_id();
                body_stmts.push(Stmt::new(
                    guard_id,
                    StmtKind::If {
                        condition: not_cond,
                        then_body: vec![Stmt::new(break_id, StmtKind::Break, condition.span)],
                        elif_arms: Vec::new(),
                        else_body: Vec::new(),
                    },
                    condition.span,
                ));

                vec![Stmt::new(
                    id,
                    StmtKind::While {
                        condition: Expr::new(ExprKind::Literal(Literal::Bool(true)), stmt.span),
                        body: body_stmts,
                    },
                    stmt.span,
                )]
            }
            CppStmtKind::For {
                init,
                condition,
                step,
                body,
            } => {
                fn_ctx.at_function_top = false;
                self.lower_for_stmt(
                    init.as_deref(),
                    condition.as_ref(),
                    step.as_deref(),
                    body,
                    stmt.span,
                    fn_ctx,
                    scopes,
                    pou_locals,
                )
            }
            CppStmtKind::Break => {
                fn_ctx.at_function_top = false;
                if !fn_ctx.in_loop {
                    self.push_err(
                        ErrorCode::ErrBreakOutsideLoop,
                        stmt.span,
                        "`break` statement is only valid inside a loop or switch arm",
                    );
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, false, None);
                vec![Stmt::new(id, StmtKind::Break, stmt.span)]
            }
            CppStmtKind::Continue => {
                fn_ctx.at_function_top = false;
                if !fn_ctx.in_loop {
                    self.push_err(
                        ErrorCode::ErrContinueOutsideLoop,
                        stmt.span,
                        "`continue` statement is only valid inside a loop",
                    );
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, false, None);
                vec![Stmt::new(id, StmtKind::Continue, stmt.span)]
            }
            CppStmtKind::Return { value } => {
                fn_ctx.at_function_top = false;
                let lowered_val = match (fn_ctx.expected_return, value) {
                    (TypeRef::Void, None) => None,
                    (TypeRef::Void, Some(expr)) => {
                        let (e, _) = self.lower_expr(expr, None, scopes);
                        self.push_err(
                            ErrorCode::ErrReturnTypeMismatch,
                            stmt.span,
                            format!("void function `{}` cannot return a value", fn_ctx.pou_name),
                        );
                        Some(e)
                    }
                    (expected, None) => {
                        self.push_err(
                            ErrorCode::ErrReturnTypeMismatch,
                            stmt.span,
                            format!(
                                "function `{}` must return a value of type `{expected}`",
                                fn_ctx.pou_name
                            ),
                        );
                        None
                    }
                    (expected, Some(expr)) => {
                        let (e, actual) = self.lower_expr(expr, Some(expected), scopes);
                        if let Some(act) = actual
                            && !types_compatible_cpp(expected, &act)
                        {
                            self.push_err(
                                ErrorCode::ErrReturnTypeMismatch,
                                expr.span,
                                format!(
                                    "return expression has type `{act}`, expected `{expected}` in `{}`",
                                    fn_ctx.pou_name
                                ),
                            );
                        }
                        Some(e)
                    }
                };
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Return { value: lowered_val },
                    stmt.span,
                )]
            }
            CppStmtKind::Foreign {
                text,
                pack_hint,
                receiver_class,
                method_name,
                call_args,
            } => {
                fn_ctx.at_function_top = false;
                let (fidelity, resolved_pack) = self.check_foreign_method_call(
                    receiver_class.as_deref(),
                    method_name.as_deref(),
                    call_args,
                    stmt.span,
                    scopes,
                );
                let final_hint = resolved_pack.or_else(|| pack_hint.clone());
                let id = self.fresh_id();
                self.record_anchor(&id, stmt.span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Foreign {
                        language: "cpp".to_owned(),
                        fidelity,
                        text: text.clone(),
                        pack_hint: final_hint,
                    },
                    stmt.span,
                )]
            }
        }
    }

    /// Rewrites direct `continue;` statements inside a `do { ... } while (cond);` body
    /// (without descending into nested loops) so `continue` always evaluates `cond`
    /// before starting the next iteration.
    fn rewrite_do_while_continues(
        &mut self,
        stmts: Vec<Stmt>,
        not_cond: &Expr,
        cond_span: Span,
    ) -> Vec<Stmt> {
        let mut out = Vec::with_capacity(stmts.len());
        for mut stmt in stmts {
            match &mut stmt.kind {
                StmtKind::Continue => {
                    let guard_id = self.fresh_id();
                    let break_id = self.fresh_id();
                    out.push(Stmt::new(
                        guard_id,
                        StmtKind::If {
                            condition: not_cond.clone(),
                            then_body: vec![Stmt::new(break_id, StmtKind::Break, cond_span)],
                            elif_arms: Vec::new(),
                            else_body: Vec::new(),
                        },
                        cond_span,
                    ));
                    out.push(stmt);
                }
                StmtKind::If {
                    then_body,
                    elif_arms,
                    else_body,
                    ..
                } => {
                    let tb = std::mem::take(then_body);
                    *then_body = self.rewrite_do_while_continues(tb, not_cond, cond_span);
                    for arm in elif_arms {
                        let ab = std::mem::take(&mut arm.body);
                        arm.body = self.rewrite_do_while_continues(ab, not_cond, cond_span);
                    }
                    let eb = std::mem::take(else_body);
                    *else_body = self.rewrite_do_while_continues(eb, not_cond, cond_span);
                    out.push(stmt);
                }
                StmtKind::Switch { cases, default, .. } => {
                    for case in cases {
                        let cb = std::mem::take(&mut case.body);
                        case.body = self.rewrite_do_while_continues(cb, not_cond, cond_span);
                    }
                    if let Some(def) = default {
                        let db = std::mem::take(def);
                        *def = self.rewrite_do_while_continues(db, not_cond, cond_span);
                    }
                    out.push(stmt);
                }
                // Nested loops own their own `continue;` statements — never descend.
                StmtKind::For { .. }
                | StmtKind::While { .. }
                | StmtKind::Assign { .. }
                | StmtKind::Call { .. }
                | StmtKind::FbInvoke { .. }
                | StmtKind::Break
                | StmtKind::Return { .. }
                | StmtKind::Wait { .. }
                | StmtKind::Foreign { .. } => {
                    out.push(stmt);
                }
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_for_stmt(
        &mut self,
        init: Option<&CppForInit>,
        condition: Option<&CppExpr>,
        step: Option<&CppForStep>,
        body: &[CppStmt],
        span: Span,
        fn_ctx: &mut FnLowerCtx<'_>,
        scopes: &mut Vec<BTreeMap<String, SymbolInfo>>,
        pou_locals: &mut Vec<DataDecl>,
    ) -> Vec<Stmt> {
        scopes.push(BTreeMap::new());

        // Handle loop init declaration / assignment
        let counter_info: Option<(String, Expr, Option<TypeRef>)> = match init {
            Some(CppForInit::Decl {
                ty,
                name,
                init: init_expr,
                span: ispan,
            }) => {
                let rty = self.resolve_type_spec(ty);
                let (from_asl, from_ty) = self.lower_expr(init_expr, rty.as_ref(), scopes);
                if let (Some(expected), Some(actual)) = (&rty, &from_ty)
                    && !types_compatible_cpp(expected, actual)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        init_expr.span,
                        format!("`for` initializer has type `{actual}`, expected `{expected}`"),
                    );
                }
                let final_ty = rty.unwrap_or_else(|| TypeRef::scalar(self.ctx.c_int_asl_type()));
                if let Some(cur) = scopes.last_mut() {
                    cur.insert(
                        name.clone(),
                        SymbolInfo::Data {
                            ty: final_ty.clone(),
                            scope: StorageScope::Temp,
                        },
                    );
                }
                // Register in `pou.locals` if not already present (required by `dendri-validate`!)
                if !pou_locals.iter().any(|d| d.name == *name) {
                    let decl_id = self.fresh_id();
                    pou_locals.push(DataDecl {
                        id: decl_id,
                        name: name.clone(),
                        scope: StorageScope::Temp,
                        ty: final_ty.clone(),
                        initial: None,
                        span: *ispan,
                    });
                }
                Some((name.clone(), from_asl, Some(final_ty)))
            }
            Some(CppForInit::Assign {
                name,
                init: init_expr,
                span: ispan,
            }) => {
                let looked_up = self.lookup_symbol(name, scopes).cloned();
                let counter_ty = match looked_up {
                    Some(SymbolInfo::Data { ty, scope }) => {
                        if scope == StorageScope::Const {
                            self.push_err(
                                ErrorCode::ErrConstWrite,
                                *ispan,
                                format!("cannot use CONST `{name}` as `for` counter"),
                            );
                        }
                        Some(ty)
                    }
                    Some(SymbolInfo::Param { ty, writable }) => {
                        if !writable {
                            self.push_err(
                                ErrorCode::ErrConstWrite,
                                *ispan,
                                format!("cannot mutate read-only parameter `{name}` in `for`"),
                            );
                        }
                        Some(ty)
                    }
                    Some(SymbolInfo::Hardware { .. }) => {
                        self.push_err(
                            ErrorCode::ErrHardwareLabelWrite,
                            *ispan,
                            format!("cannot use hardware label `{name}` as `for` counter"),
                        );
                        None
                    }
                    None => {
                        self.push_err(
                            ErrorCode::ErrUndeclaredSymbol,
                            *ispan,
                            format!("undeclared `for` counter `{name}`"),
                        );
                        None
                    }
                };
                let (from_asl, _) = self.lower_expr(init_expr, counter_ty.as_ref(), scopes);
                Some((name.clone(), from_asl, counter_ty))
            }
            None => None,
        };

        // Try canonical `StmtKind::For` extraction: `for (i = from; i < to; i += step)`
        let canonical =
            if let (Some((counter, from_asl, counter_ty)), Some(cond_expr), Some(step_spec)) =
                (&counter_info, condition, step)
                && let CppExprKind::Binary { op, left, right } = &cond_expr.kind
                && matches!(
                    op,
                    BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
                )
                && matches!(&left.kind, CppExprKind::Ident(n) if n == counter)
            {
                let inclusive = matches!(op, BinaryOp::Le | BinaryOp::Ge);
                let (to_asl, to_ty) = self.lower_expr(right, counter_ty.as_ref(), scopes);
                if let (Some(ct), Some(tt)) = (counter_ty, &to_ty)
                    && !types_compatible_cpp(ct, tt)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        right.span,
                        format!("`for` bound `{tt}` does not match counter type `{ct}`"),
                    );
                }
                let step_asl = match step_spec {
                    CppForStep::Inc { name } if name == counter => {
                        Some(Expr::new(ExprKind::Literal(Literal::Int(1)), span))
                    }
                    CppForStep::Dec { name } if name == counter => {
                        Some(Expr::new(ExprKind::Literal(Literal::Int(-1)), span))
                    }
                    CppForStep::Compound {
                        name,
                        op: crate::cpp_ast::CompoundOp::Add,
                        rhs,
                    } if name == counter => {
                        let (s_asl, _) = self.lower_expr(rhs, counter_ty.as_ref(), scopes);
                        Some(s_asl)
                    }
                    CppForStep::Compound {
                        name,
                        op: crate::cpp_ast::CompoundOp::Sub,
                        rhs,
                    } if name == counter => {
                        let (s_asl, _) = self.lower_expr(rhs, counter_ty.as_ref(), scopes);
                        let neg = if let ExprKind::Literal(Literal::Int(k)) = s_asl.kind {
                            Expr::new(ExprKind::Literal(Literal::Int(-k)), s_asl.span)
                        } else {
                            Expr::new(
                                ExprKind::Unary {
                                    op: UnaryOp::Neg,
                                    operand: Box::new(s_asl),
                                },
                                rhs.span,
                            )
                        };
                        Some(neg)
                    }
                    CppForStep::Inc { .. }
                    | CppForStep::Dec { .. }
                    | CppForStep::Compound { .. }
                    | CppForStep::Assign { .. } => None,
                };
                step_asl.map(|s| (counter.clone(), from_asl.clone(), to_asl, s, inclusive))
            } else {
                None
            };

        let id = self.fresh_id();
        self.record_anchor(&id, span, true, Some(span));

        let prev_loop = fn_ctx.in_loop;
        fn_ctx.in_loop = true;
        let body_stmts = self.lower_stmts(body, fn_ctx, scopes, pou_locals);
        fn_ctx.in_loop = prev_loop;

        if let Some((counter, from, to, step_expr, inclusive)) = canonical {
            let _ = scopes.pop();
            return vec![Stmt::new(
                id,
                StmtKind::For {
                    counter,
                    from,
                    to,
                    step: step_expr,
                    inclusive,
                    body: body_stmts,
                },
                span,
            )];
        }

        // Non-canonical `for` (e.g. `for (;;)`) -> lower cleanly to `While`
        let bool_ty = TypeRef::scalar(ScalarType::Bool);
        let cond_asl = match condition {
            Some(c) => self.lower_expr(c, Some(&bool_ty), scopes).0,
            None => Expr::new(ExprKind::Literal(Literal::Bool(true)), span),
        };
        let mut out = Vec::new();
        if let Some((counter, from_asl, _)) = counter_info {
            let init_id = self.fresh_id();
            out.push(Stmt::new(
                init_id,
                StmtKind::Assign {
                    target: LValue::Var {
                        name: counter,
                        span,
                    },
                    value: from_asl,
                },
                span,
            ));
        }
        let mut while_body = body_stmts;
        if let Some(step_spec) = step {
            let step_stmt = match step_spec {
                CppForStep::Inc { name } => Stmt::new(
                    self.fresh_id(),
                    StmtKind::Assign {
                        target: LValue::Var {
                            name: name.clone(),
                            span,
                        },
                        value: Expr::new(
                            ExprKind::Binary {
                                op: BinaryOp::Add,
                                left: Box::new(Expr::new(ExprKind::Ident(name.clone()), span)),
                                right: Box::new(Expr::new(
                                    ExprKind::Literal(Literal::Int(1)),
                                    span,
                                )),
                            },
                            span,
                        ),
                    },
                    span,
                ),
                CppForStep::Dec { name } => Stmt::new(
                    self.fresh_id(),
                    StmtKind::Assign {
                        target: LValue::Var {
                            name: name.clone(),
                            span,
                        },
                        value: Expr::new(
                            ExprKind::Binary {
                                op: BinaryOp::Sub,
                                left: Box::new(Expr::new(ExprKind::Ident(name.clone()), span)),
                                right: Box::new(Expr::new(
                                    ExprKind::Literal(Literal::Int(1)),
                                    span,
                                )),
                            },
                            span,
                        ),
                    },
                    span,
                ),
                CppForStep::Compound { name, op, rhs } => {
                    let (r_asl, _) = self.lower_expr(rhs, None, scopes);
                    Stmt::new(
                        self.fresh_id(),
                        StmtKind::Assign {
                            target: LValue::Var {
                                name: name.clone(),
                                span,
                            },
                            value: Expr::new(
                                ExprKind::Binary {
                                    op: op.to_binary_op(),
                                    left: Box::new(Expr::new(ExprKind::Ident(name.clone()), span)),
                                    right: Box::new(r_asl),
                                },
                                span,
                            ),
                        },
                        span,
                    )
                }
                CppForStep::Assign { name, rhs } => {
                    let (r_asl, _) = self.lower_expr(rhs, None, scopes);
                    Stmt::new(
                        self.fresh_id(),
                        StmtKind::Assign {
                            target: LValue::Var {
                                name: name.clone(),
                                span,
                            },
                            value: r_asl,
                        },
                        span,
                    )
                }
            };
            while_body.push(step_stmt);
        }
        let _ = scopes.pop();
        out.push(Stmt::new(
            id,
            StmtKind::While {
                condition: cond_asl,
                body: while_body,
            },
            span,
        ));
        out
    }

    fn lower_expr_stmt(
        &mut self,
        expr: &CppExpr,
        stmt_span: Span,
        fn_ctx: &mut FnLowerCtx<'_>,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> Vec<Stmt> {
        match &expr.kind {
            // Reconciled in Pass B (`pinMode` / `attachInterrupt`) — do not emit runtime StmtKind::Call!
            CppExprKind::Call { callee, .. }
                if callee == "pinMode" || callee == "attachInterrupt" =>
            {
                Vec::new()
            }
            // `digitalWrite(pin, val)` -> `DIGITAL_WRITE(label, val)`
            CppExprKind::Call { callee, args } if callee == "digitalWrite" => {
                fn_ctx.at_function_top = false;
                if args.len() != 2 {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        stmt_span,
                        format!("`digitalWrite` expects 2 arguments, found {}", args.len()),
                    );
                    return Vec::new();
                }
                let (Some(pin_arg), Some(val_arg)) = (args.first(), args.get(1)) else {
                    return Vec::new();
                };
                let pin_label = self.resolve_existing_pin_label(pin_arg);
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                let (val_asl, val_ty) = self.lower_expr(val_arg, Some(&bool_ty), scopes);
                if let Some(vt) = val_ty
                    && !types_compatible_cpp(&bool_ty, &vt)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        val_arg.span,
                        format!("`digitalWrite` value must be BOOL/HIGH/LOW, found `{vt}`"),
                    );
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt_span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Call {
                        callee: "DIGITAL_WRITE".to_owned(),
                        args: vec![
                            CallArg {
                                name: None,
                                value: Expr::new(ExprKind::Ident(pin_label), pin_arg.span),
                                span: pin_arg.span,
                            },
                            CallArg {
                                name: None,
                                value: val_asl,
                                span: val_arg.span,
                            },
                        ],
                    },
                    stmt_span,
                )]
            }
            // `analogWrite(pin, val)` -> `PWM_WRITE(label, ANALOG_TO_DUTY(CODE_TO_ANALOG(val, "pwm_8bit")))`
            CppExprKind::Call { callee, args } if callee == "analogWrite" => {
                fn_ctx.at_function_top = false;
                if args.len() != 2 {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        stmt_span,
                        format!("`analogWrite` expects 2 arguments, found {}", args.len()),
                    );
                    return Vec::new();
                }
                let (Some(pin_arg), Some(val_arg)) = (args.first(), args.get(1)) else {
                    return Vec::new();
                };
                let pin_label = self.resolve_existing_pin_label(pin_arg);
                let expected_code = TypeRef::scalar(ScalarType::Code {
                    codec_id: "pwm_8bit".to_owned(),
                });
                let (val_asl, val_ty) = self.lower_expr(val_arg, Some(&expected_code), scopes);
                if let Some(vt) = val_ty
                    && !types_compatible_cpp(&expected_code, &vt)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        val_arg.span,
                        format!("`analogWrite` duty code must be integer, found `{vt}`"),
                    );
                }
                if let Some(cval) = eval_const_cpp_int(val_arg, &self.enum_variants)
                    && !(0..=255).contains(&cval)
                {
                    self.push_warn(
                        ErrorCode::WarnDutyClamped,
                        val_arg.span,
                        format!(
                            "`analogWrite` value {cval} exceeds 8-bit PWM range [0, 255] and will clamp"
                        ),
                    );
                }
                let code_to_analog = Expr::new(
                    ExprKind::Call {
                        callee: "CODE_TO_ANALOG".to_owned(),
                        args: vec![
                            CallArg {
                                name: None,
                                value: val_asl,
                                span: val_arg.span,
                            },
                            CallArg {
                                name: None,
                                value: Expr::new(
                                    ExprKind::Literal(Literal::String("pwm_8bit".to_owned())),
                                    val_arg.span,
                                ),
                                span: val_arg.span,
                            },
                        ],
                    },
                    val_arg.span,
                );
                let analog_to_duty = Expr::new(
                    ExprKind::Call {
                        callee: "ANALOG_TO_DUTY".to_owned(),
                        args: vec![CallArg {
                            name: None,
                            value: code_to_analog,
                            span: val_arg.span,
                        }],
                    },
                    val_arg.span,
                );
                let id = self.fresh_id();
                self.record_anchor(&id, stmt_span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Call {
                        callee: "PWM_WRITE".to_owned(),
                        args: vec![
                            CallArg {
                                name: None,
                                value: Expr::new(ExprKind::Ident(pin_label), pin_arg.span),
                                span: pin_arg.span,
                            },
                            CallArg {
                                name: None,
                                value: analog_to_duty,
                                span: val_arg.span,
                            },
                        ],
                    },
                    stmt_span,
                )]
            }
            // `analogReadResolution(bits)` -> `ANALOG_READ_RESOLUTION(bits)`
            CppExprKind::Call { callee, args } if callee == "analogReadResolution" => {
                fn_ctx.at_function_top = false;
                if args.len() != 1 {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        stmt_span,
                        format!(
                            "`analogReadResolution` expects 1 argument, found {}",
                            args.len()
                        ),
                    );
                    return Vec::new();
                }
                let Some(bits_arg) = args.first() else {
                    return Vec::new();
                };
                if let Some(b) = eval_const_cpp_int(bits_arg, &self.enum_variants)
                    && !(1..=29).contains(&b)
                {
                    self.push_err(
                        ErrorCode::ErrCodecResolutionExceedsScale,
                        bits_arg.span,
                        format!(
                            "`analogReadResolution({b})` exceeds M = 10^9 limit (valid bit widths: 1..=29)"
                        ),
                    );
                }
                let expected = TypeRef::scalar(ScalarType::Usint);
                let (bits_asl, bits_ty) = self.lower_expr(bits_arg, Some(&expected), scopes);
                if let Some(bt) = bits_ty
                    && !is_int_like_type(&bt)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        bits_arg.span,
                        format!("`analogReadResolution` expects integer bit width, found `{bt}`"),
                    );
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt_span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Call {
                        callee: "ANALOG_READ_RESOLUTION".to_owned(),
                        args: vec![CallArg {
                            name: None,
                            value: bits_asl,
                            span: bits_arg.span,
                        }],
                    },
                    stmt_span,
                )]
            }
            // `delay(ms)` -> `WAIT_TIME(ms)` / `delayMicroseconds(us)` -> `WAIT_TIME_US(us)`
            CppExprKind::Call { callee, args }
                if callee == "delay" || callee == "delayMicroseconds" =>
            {
                fn_ctx.at_function_top = false;
                if fn_ctx.in_isr {
                    self.push_err(
                        ErrorCode::ErrIsrForbiddenOp,
                        stmt_span,
                        format!(
                            "blocking `{callee}` is forbidden inside interrupt OB `{}`",
                            fn_ctx.pou_name
                        ),
                    );
                }
                if args.len() != 1 {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        stmt_span,
                        format!("`{callee}` expects 1 argument, found {}", args.len()),
                    );
                    return Vec::new();
                }
                let Some(dur_arg) = args.first() else {
                    return Vec::new();
                };
                let (asl_name, expected) = if callee == "delay" {
                    ("WAIT_TIME", TypeRef::scalar(ScalarType::Udint))
                } else {
                    ("WAIT_TIME_US", TypeRef::scalar(ScalarType::Ulint))
                };
                let (dur_asl, dur_ty) = self.lower_expr(dur_arg, Some(&expected), scopes);
                if let Some(dt) = dur_ty
                    && !is_int_like_type(&dt)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        dur_arg.span,
                        format!("`{callee}` expects integer duration, found `{dt}`"),
                    );
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt_span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Call {
                        callee: asl_name.to_owned(),
                        args: vec![CallArg {
                            name: None,
                            value: dur_asl,
                            span: dur_arg.span,
                        }],
                    },
                    stmt_span,
                )]
            }
            // `Serial.begin` / `Serial.print` / `Serial.println`
            CppExprKind::MethodCall {
                receiver,
                method,
                args,
            } if matches!(&receiver.kind, CppExprKind::Ident(r) if r == "Serial") => {
                fn_ctx.at_function_top = false;
                let (min_args, max_args) = match method.as_str() {
                    "begin" => (1usize, 1usize),
                    "print" => (1, 2),
                    "println" => (0, 2),
                    "write" => (1, 2),
                    "flush" | "end" | "available" | "read" => (0, 0),
                    _ => {
                        self.push_err(
                            ErrorCode::ErrUnresolvedFunction,
                            stmt_span,
                            format!("unknown `Serial.{method}` method"),
                        );
                        return Vec::new();
                    }
                };
                if args.len() < min_args || args.len() > max_args {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        stmt_span,
                        format!(
                            "`Serial.{method}` expects {min_args}..={max_args} argument(s), found {}",
                            args.len()
                        ),
                    );
                }
                let mut call_args = Vec::with_capacity(args.len());
                for a in args {
                    let (v, _) = self.lower_expr(a, None, scopes);
                    call_args.push(CallArg {
                        name: None,
                        value: v,
                        span: a.span,
                    });
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt_span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Call {
                        callee: format!("Serial.{method}"),
                        args: call_args,
                    },
                    stmt_span,
                )]
            }
            // Designated FB invocation: `timer1({.IN = btn, .PT = 1000});`
            CppExprKind::DesignatedCall { callee, fields } => {
                fn_ctx.at_function_top = false;
                let Some(fb_fields) = self.fb_fields_of_var(callee, stmt_span, scopes) else {
                    return Vec::new();
                };
                let mut inputs = Vec::with_capacity(fields.len());
                for (fname, fexpr) in fields {
                    if let Some(fty) = fb_fields.get(fname) {
                        let (asl_v, actual) = self.lower_expr(fexpr, Some(fty), scopes);
                        if let Some(act) = actual
                            && !types_compatible_cpp(fty, &act)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                fexpr.span,
                                format!("FB input `{fname}` expects `{fty}`, found `{act}`"),
                            );
                        }
                        inputs.push((fname.clone(), asl_v));
                    } else {
                        self.push_err(
                            ErrorCode::ErrUnknownFieldAccess,
                            fexpr.span,
                            format!("Function Block `{callee}` has no input field `{fname}`"),
                        );
                    }
                }
                let id = self.fresh_id();
                self.record_anchor(&id, stmt_span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::FbInvoke {
                        instance: LValue::Var {
                            name: callee.clone(),
                            span: expr.span,
                        },
                        inputs,
                    },
                    stmt_span,
                )]
            }
            // Positional FB invocation `timer1(btn, 1000);` OR regular function call statement
            CppExprKind::Call { callee, args } => {
                fn_ctx.at_function_top = false;
                // Check if `callee` is a declared FB instance
                if let Some(SymbolInfo::Data {
                    ty: TypeRef::Fb { fb_kind },
                    ..
                }) = self.lookup_symbol(callee, scopes).cloned()
                {
                    let in_fields = self.fb_input_fields_ordered(&fb_kind);
                    if args.len() > in_fields.len() {
                        self.push_err(
                            ErrorCode::ErrArityMismatch,
                            stmt_span,
                            format!(
                                "Function Block `{callee}` expects at most {} inputs, found {}",
                                in_fields.len(),
                                args.len()
                            ),
                        );
                    }
                    let mut inputs = Vec::new();
                    for (arg, (fname, fty)) in args.iter().zip(in_fields.iter()) {
                        let (asl_v, actual) = self.lower_expr(arg, Some(fty), scopes);
                        if let Some(act) = actual
                            && !types_compatible_cpp(fty, &act)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                arg.span,
                                format!("FB input `{fname}` expects `{fty}`, found `{act}`"),
                            );
                        }
                        inputs.push((fname.clone(), asl_v));
                    }
                    let id = self.fresh_id();
                    self.record_anchor(&id, stmt_span, false, None);
                    return vec![Stmt::new(
                        id,
                        StmtKind::FbInvoke {
                            instance: LValue::Var {
                                name: callee.clone(),
                                span: expr.span,
                            },
                            inputs,
                        },
                        stmt_span,
                    )];
                }

                // Regular function / stdlib / DISPLAY_PRINT call statement
                let (asl_callee, call_args, _) =
                    self.lower_and_check_call(callee, args, stmt_span, scopes);
                let id = self.fresh_id();
                self.record_anchor(&id, stmt_span, false, None);
                vec![Stmt::new(
                    id,
                    StmtKind::Call {
                        callee: asl_callee,
                        args: call_args,
                    },
                    stmt_span,
                )]
            }
            CppExprKind::BoolLit(_)
            | CppExprKind::IntLit { .. }
            | CppExprKind::FloatLit { .. }
            | CppExprKind::CharLit(_)
            | CppExprKind::StringLit(_)
            | CppExprKind::Ident(_)
            | CppExprKind::ScopedIdent { .. }
            | CppExprKind::Index { .. }
            | CppExprKind::Field { .. }
            | CppExprKind::MethodCall { .. }
            | CppExprKind::Unary { .. }
            | CppExprKind::Binary { .. }
            | CppExprKind::Ternary { .. }
            | CppExprKind::Cast { .. }
            | CppExprKind::Foreign { .. } => {
                fn_ctx.at_function_top = false;
                // Other method call in statement position -> check if foreign/pack object
                if let CppExprKind::MethodCall {
                    receiver,
                    method,
                    args,
                } = &expr.kind
                    && let CppExprKind::Ident(obj_name) = &receiver.kind
                {
                    let (fidelity, pack_hint) = self.check_foreign_method_call(
                        Some(obj_name.as_str()),
                        Some(method.as_str()),
                        args,
                        stmt_span,
                        scopes,
                    );
                    let id = self.fresh_id();
                    self.record_anchor(&id, stmt_span, false, None);
                    return vec![Stmt::new(
                        id,
                        StmtKind::Foreign {
                            language: "cpp".to_owned(),
                            fidelity,
                            text: self.slice_span(stmt_span).trim().to_owned(),
                            pack_hint,
                        },
                        stmt_span,
                    )];
                }
                let _ = self.lower_expr(expr, None, scopes);
                Vec::new()
            }
        }
    }

    fn resolve_existing_pin_label(&mut self, pin_arg: &CppExpr) -> String {
        if let CppExprKind::Ident(name) = &pin_arg.kind {
            if self.hardware_by_label.contains_key(name) {
                return name.clone();
            }
            if let Some(port) = resolve_pin_from_ident(name, self.ctx) {
                return self
                    .port_to_label
                    .get(&port)
                    .cloned()
                    .unwrap_or_else(|| name.clone());
            }
            name.clone()
        } else if let CppExprKind::IntLit { value, .. } = &pin_arg.kind {
            if let Some(port) = resolve_pin_from_int(*value, self.ctx) {
                return self.port_to_label.get(&port).cloned().unwrap_or(port);
            }
            format!("D{value}")
        } else {
            "UNKNOWN_PIN".to_owned()
        }
    }

    fn check_foreign_method_call(
        &mut self,
        receiver: Option<&str>,
        method: Option<&str>,
        args: &[CppExpr],
        span: Span,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> (ForeignFidelity, Option<String>) {
        let (Some(recv), Some(meth)) = (receiver, method) else {
            return (ForeignFidelity::F0, None);
        };
        let class_name = self
            .foreign_objects
            .get(recv)
            .map(String::as_str)
            .unwrap_or(recv);
        let qualified = format!("{class_name}.{meth}");

        if let Some((pack, sym)) = self
            .ctx
            .lookup_pack_symbol(LanguageId::Cpp, &qualified)
            .map(|(p, s)| (p.clone(), s.clone()))
        {
            // Validate arity against F1 pack signature (honoring Arduino optional default args)
            let arity_ok = if qualified == "Wire.endTransmission" {
                args.len() <= 1
            } else if qualified == "SPI.beginTransaction" {
                args.len() == 1 || args.len() == sym.params.len()
            } else {
                args.len() == sym.params.len()
            };
            if !arity_ok {
                self.push_err(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!(
                        "pack symbol `{qualified}` expects {} argument(s), found {}",
                        sym.params.len(),
                        args.len()
                    ),
                );
            } else if !(qualified == "SPI.beginTransaction" && args.len() == 1) {
                for (arg, param) in args.iter().zip(sym.params.iter()) {
                    let expected = param.type_ref();
                    let (_, actual) = self.lower_expr(arg, expected.as_ref(), scopes);
                    if let (Some(exp), Some(act)) = (expected, actual)
                        && !types_compatible_cpp(&exp, &act)
                    {
                        self.push_err(
                            ErrorCode::ErrTypeMismatch,
                            arg.span,
                            format!(
                                "argument `{}` to `{qualified}` has type `{act}`, expected `{exp}`",
                                param.name
                            ),
                        );
                    }
                }
            }
            return (pack.fidelity(), Some(pack.pack.id));
        }

        // No loaded pack match -> preserve at F0 with known pack hint if applicable
        let hint = match class_name {
            "Wire" => Some("arduino-wire".to_owned()),
            "SPI" => Some("arduino-spi".to_owned()),
            "Servo" => Some("arduino-servo".to_owned()),
            _ => None,
        };
        (ForeignFidelity::F0, hint)
    }

    fn lower_writable_lvalue(
        &mut self,
        expr: &CppExpr,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> Option<(LValue, Option<TypeRef>)> {
        let lval = self.expr_to_lvalue(expr, scopes)?;
        let root = lval.root_name();
        match self.lookup_symbol(root, scopes).cloned() {
            Some(SymbolInfo::Data {
                scope: StorageScope::Const,
                ..
            }) => {
                self.push_err(
                    ErrorCode::ErrConstWrite,
                    expr.span,
                    format!("cannot write to constant `{root}`"),
                );
                return None;
            }
            Some(SymbolInfo::Param {
                writable: false, ..
            }) => {
                self.push_err(
                    ErrorCode::ErrConstWrite,
                    expr.span,
                    format!("cannot write to read-only parameter `{root}`"),
                );
                return None;
            }
            Some(SymbolInfo::Hardware { .. }) => {
                self.push_err(
                    ErrorCode::ErrHardwareLabelWrite,
                    expr.span,
                    format!(
                        "cannot assign directly to hardware pin label `{root}`; use `digitalWrite`/`analogWrite`"
                    ),
                );
                return None;
            }
            Some(SymbolInfo::Data { .. } | SymbolInfo::Param { writable: true, .. }) => {}
            None => {
                self.push_err(
                    ErrorCode::ErrUndeclaredSymbol,
                    expr.span,
                    format!("undeclared variable `{root}`"),
                );
                return None;
            }
        }
        let ty = self.infer_lvalue_type(&lval, scopes);
        Some((lval, ty))
    }

    fn expr_to_lvalue(
        &mut self,
        expr: &CppExpr,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> Option<LValue> {
        match &expr.kind {
            CppExprKind::Ident(name) => Some(LValue::Var {
                name: name.clone(),
                span: expr.span,
            }),
            CppExprKind::Index { base, index } => {
                let base_lv = self.expr_to_lvalue(base, scopes)?;
                let (idx_asl, _) = self.lower_expr(index, None, scopes);
                Some(LValue::Index {
                    base: Box::new(base_lv),
                    index: Box::new(idx_asl),
                    span: expr.span,
                })
            }
            CppExprKind::Field { base, field } => {
                let base_lv = self.expr_to_lvalue(base, scopes)?;
                Some(LValue::Field {
                    base: Box::new(base_lv),
                    field: field.clone(),
                    span: expr.span,
                })
            }
            CppExprKind::BoolLit(_)
            | CppExprKind::IntLit { .. }
            | CppExprKind::FloatLit { .. }
            | CppExprKind::CharLit(_)
            | CppExprKind::StringLit(_)
            | CppExprKind::ScopedIdent { .. }
            | CppExprKind::Call { .. }
            | CppExprKind::MethodCall { .. }
            | CppExprKind::DesignatedCall { .. }
            | CppExprKind::Unary { .. }
            | CppExprKind::Binary { .. }
            | CppExprKind::Ternary { .. }
            | CppExprKind::Cast { .. }
            | CppExprKind::Foreign { .. } => {
                self.push_err(
                    ErrorCode::ErrTypeMismatch,
                    expr.span,
                    "expression is not assignable",
                );
                None
            }
        }
    }

    fn infer_lvalue_type(
        &mut self,
        lval: &LValue,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> Option<TypeRef> {
        match lval {
            LValue::Var { name, span } => self.resolve_ident_type(name, *span, scopes),
            LValue::Index { base, index, span } => {
                let base_ty = self.infer_lvalue_type(base, scopes)?;
                self.check_array_index(&base_ty, index, *span)
            }
            LValue::Field { base, field, span } => {
                let base_ty = self.infer_lvalue_type(base, scopes)?;
                self.check_field_access(&base_ty, field, *span)
            }
        }
    }

    fn lower_expr(
        &mut self,
        expr: &CppExpr,
        expected: Option<&TypeRef>,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> (Expr, Option<TypeRef>) {
        match &expr.kind {
            CppExprKind::BoolLit(b) => (
                Expr::new(ExprKind::Literal(Literal::Bool(*b)), expr.span),
                Some(TypeRef::scalar(ScalarType::Bool)),
            ),
            CppExprKind::IntLit {
                value,
                unsigned,
                long_count,
            } => {
                if let Some(TypeRef::Scalar {
                    ty: ScalarType::Time,
                }) = expected
                    && let Ok(us) = i64::try_from(*value)
                {
                    return (
                        Expr::new(ExprKind::Literal(Literal::TimeUs(us)), expr.span),
                        Some(TypeRef::scalar(ScalarType::Time)),
                    );
                }
                let ty = if let Some(exp) = expected
                    && is_int_like_type(exp)
                    && !*unsigned
                    && *long_count == 0
                {
                    exp.clone()
                } else {
                    match (*unsigned, *long_count) {
                        (false, 0) => TypeRef::scalar(self.ctx.c_int_asl_type()),
                        (true, 0) => TypeRef::scalar(self.ctx.c_uint_asl_type()),
                        (false, 1) => TypeRef::scalar(ScalarType::Dint),
                        (true, 1) => TypeRef::scalar(ScalarType::Udint),
                        (false, _) => TypeRef::scalar(ScalarType::Lint),
                        (true, _) => TypeRef::scalar(ScalarType::Ulint),
                    }
                };
                (
                    Expr::new(ExprKind::Literal(Literal::Int(*value)), expr.span),
                    Some(ty),
                )
            }
            CppExprKind::FloatLit { value, is_f32 } => {
                let ty = if *is_f32 {
                    TypeRef::scalar(ScalarType::Real)
                } else if let Some(
                    exp @ TypeRef::Scalar {
                        ty: ScalarType::Real | ScalarType::Lreal,
                    },
                ) = expected
                {
                    exp.clone()
                } else {
                    TypeRef::scalar(c_double_asl_type(self.ctx))
                };
                (
                    Expr::new(ExprKind::Literal(Literal::Float(*value)), expr.span),
                    Some(ty),
                )
            }
            CppExprKind::CharLit(ch) => (
                Expr::new(
                    ExprKind::Literal(Literal::Int(i128::from(u32::from(*ch)))),
                    expr.span,
                ),
                Some(TypeRef::scalar(ScalarType::Sint)),
            ),
            CppExprKind::StringLit(s) => (
                Expr::new(ExprKind::Literal(Literal::String(s.clone())), expr.span),
                Some(TypeRef::scalar(ScalarType::String { max_len: 255 })),
            ),
            CppExprKind::Ident(name) => {
                // Arduino SPI / bit constants when used inside pack calls
                if let Some(const_val) = arduino_builtin_int_constant(name) {
                    let ty = expected
                        .filter(|e| is_int_like_type(e))
                        .cloned()
                        .unwrap_or_else(|| TypeRef::scalar(ScalarType::Usint));
                    return (
                        Expr::new(ExprKind::Literal(Literal::Int(const_val)), expr.span),
                        Some(ty),
                    );
                }
                let ty = self.resolve_ident_type(name, expr.span, scopes);
                (Expr::new(ExprKind::Ident(name.clone()), expr.span), ty)
            }
            CppExprKind::ScopedIdent { scope, member } => {
                let key = format!("{scope}::{member}");
                if let Some((enum_name, _)) = self.enum_variants.get(&key) {
                    return (
                        Expr::new(ExprKind::Ident(member.clone()), expr.span),
                        Some(TypeRef::Named {
                            name: enum_name.clone(),
                        }),
                    );
                }
                self.push_err(
                    ErrorCode::ErrUndeclaredSymbol,
                    expr.span,
                    format!("undeclared scoped symbol `{key}`"),
                );
                (Expr::new(ExprKind::Ident(key), expr.span), None)
            }
            CppExprKind::Index { base, index } => {
                let (base_asl, base_ty) = self.lower_expr(base, None, scopes);
                let (idx_asl, idx_ty) = self.lower_expr(index, None, scopes);
                if let Some(it) = idx_ty
                    && !is_int_like_type(&it)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        index.span,
                        format!("array index must be an integer, found `{it}`"),
                    );
                }
                let elem_ty = base_ty
                    .as_ref()
                    .and_then(|bt| self.check_array_index(bt, &idx_asl, expr.span));
                (
                    Expr::new(
                        ExprKind::Index {
                            base: Box::new(base_asl),
                            index: Box::new(idx_asl),
                        },
                        expr.span,
                    ),
                    elem_ty,
                )
            }
            CppExprKind::Field { base, field } => {
                let (base_asl, base_ty) = self.lower_expr(base, None, scopes);
                let field_ty = base_ty
                    .as_ref()
                    .and_then(|bt| self.check_field_access(bt, field, expr.span));
                (
                    Expr::new(
                        ExprKind::Field {
                            base: Box::new(base_asl),
                            field: field.clone(),
                        },
                        expr.span,
                    ),
                    field_ty,
                )
            }
            CppExprKind::Call { callee, args } => {
                let (asl_callee, call_args, ret_ty) =
                    self.lower_and_check_call(callee, args, expr.span, scopes);
                (
                    Expr::new(
                        ExprKind::Call {
                            callee: asl_callee,
                            args: call_args,
                        },
                        expr.span,
                    ),
                    ret_ty,
                )
            }
            CppExprKind::MethodCall {
                receiver,
                method,
                args,
            } => {
                // `Serial.available()` / `Serial.read()`
                if let CppExprKind::Ident(obj) = &receiver.kind
                    && obj == "Serial"
                {
                    let valid = matches!(method.as_str(), "available" | "read" | "peek");
                    if !valid {
                        self.push_err(
                            ErrorCode::ErrUnresolvedFunction,
                            expr.span,
                            format!("unknown `Serial.{method}` expression method"),
                        );
                    }
                    if !args.is_empty() {
                        self.push_err(
                            ErrorCode::ErrArityMismatch,
                            expr.span,
                            format!(
                                "`Serial.{method}` expects 0 arguments, found {}",
                                args.len()
                            ),
                        );
                    }
                    return (
                        Expr::new(
                            ExprKind::Call {
                                callee: format!("Serial.{method}"),
                                args: Vec::new(),
                            },
                            expr.span,
                        ),
                        Some(TypeRef::scalar(self.ctx.c_int_asl_type())),
                    );
                }

                // Pack/Foreign object method call in expression position (`Wire.read()`, `SPI.transfer(0xAA)`, `servo.attach(9)`, `servo.read()`)
                let recv_name = if let CppExprKind::Ident(n) = &receiver.kind {
                    Some(n.as_str())
                } else {
                    None
                };
                let class_name =
                    recv_name.map(|r| self.foreign_objects.get(r).map(String::as_str).unwrap_or(r));
                let text = self.slice_span(expr.span).trim().to_owned();

                if let Some(cls) = class_name {
                    let qualified = format!("{cls}.{method}");
                    if let Some((_pack, sym)) = self
                        .ctx
                        .lookup_pack_symbol(LanguageId::Cpp, &qualified)
                        .map(|(p, s)| (p.clone(), s.clone()))
                    {
                        let arity_ok = if qualified == "Wire.endTransmission" {
                            args.len() <= 1
                        } else {
                            args.len() == sym.params.len()
                        };
                        if !arity_ok {
                            self.push_err(
                                ErrorCode::ErrArityMismatch,
                                expr.span,
                                format!(
                                    "pack symbol `{qualified}` expects {} argument(s), found {}",
                                    sym.params.len(),
                                    args.len()
                                ),
                            );
                        } else {
                            for (arg, param) in args.iter().zip(sym.params.iter()) {
                                let exp = param.type_ref();
                                let (_, act) = self.lower_expr(arg, exp.as_ref(), scopes);
                                if let (Some(e), Some(a)) = (exp, act)
                                    && !types_compatible_cpp(&e, &a)
                                {
                                    self.push_err(
                                        ErrorCode::ErrTypeMismatch,
                                        arg.span,
                                        format!(
                                            "argument `{}` to `{qualified}` has type `{a}`, expected `{e}`",
                                            param.name
                                        ),
                                    );
                                }
                            }
                        }
                        return (
                            Expr::new(
                                ExprKind::Foreign {
                                    language: "cpp".to_owned(),
                                    text,
                                },
                                expr.span,
                            ),
                            sym.return_type_ref(),
                        );
                    }
                }

                // Unmodeled F0 foreign method expression: contextual type or 4-part ERR_FOREIGN_UNTYPED
                if let Some(exp) = expected {
                    (
                        Expr::new(
                            ExprKind::Foreign {
                                language: "cpp".to_owned(),
                                text,
                            },
                            expr.span,
                        ),
                        Some(exp.clone()),
                    )
                } else {
                    self.emit_foreign_untyped(&text, expr.span);
                    (
                        Expr::new(
                            ExprKind::Foreign {
                                language: "cpp".to_owned(),
                                text,
                            },
                            expr.span,
                        ),
                        None,
                    )
                }
            }
            CppExprKind::DesignatedCall { .. } => {
                self.push_err(
                    ErrorCode::ErrTypeMismatch,
                    expr.span,
                    "Function Block invocation is only valid as a statement",
                );
                (
                    Expr::new(ExprKind::Literal(Literal::Bool(false)), expr.span),
                    None,
                )
            }
            CppExprKind::Unary { op, operand } => {
                // Fold unary `-` on non-unsigned numeric literals into canonical negative
                // ASL `Literal::Int(-v)` / `Literal::Float(-v)` so C++ round-trips `LOSSLESS_ASL`.
                if *op == UnaryOp::Neg
                    && let CppExprKind::IntLit {
                        value,
                        unsigned: false,
                        long_count,
                    } = &operand.kind
                    && let Some(neg) = value.checked_neg()
                {
                    let ty = if let Some(exp) = expected
                        && is_int_like_type(exp)
                        && *long_count == 0
                    {
                        exp.clone()
                    } else {
                        match *long_count {
                            0 => TypeRef::scalar(self.ctx.c_int_asl_type()),
                            1 => TypeRef::scalar(ScalarType::Dint),
                            _ => TypeRef::scalar(ScalarType::Lint),
                        }
                    };
                    return (
                        Expr::new(ExprKind::Literal(Literal::Int(neg)), expr.span),
                        Some(ty),
                    );
                }
                if *op == UnaryOp::Neg
                    && let CppExprKind::FloatLit { value, is_f32 } = &operand.kind
                {
                    let ty = if *is_f32 {
                        TypeRef::scalar(ScalarType::Real)
                    } else if let Some(
                        exp @ TypeRef::Scalar {
                            ty: ScalarType::Real | ScalarType::Lreal,
                        },
                    ) = expected
                    {
                        exp.clone()
                    } else {
                        TypeRef::scalar(c_double_asl_type(self.ctx))
                    };
                    return (
                        Expr::new(ExprKind::Literal(Literal::Float(-*value)), expr.span),
                        Some(ty),
                    );
                }
                let expected_inner = if *op == UnaryOp::Not {
                    Some(TypeRef::scalar(ScalarType::Bool))
                } else {
                    expected.cloned()
                };
                let (op_asl, op_ty) = self.lower_expr(operand, expected_inner.as_ref(), scopes);
                let result_ty = match op {
                    UnaryOp::Not => {
                        let bool_ty = TypeRef::scalar(ScalarType::Bool);
                        if let Some(ot) = &op_ty
                            && !types_compatible_cpp(&bool_ty, ot)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                expr.span,
                                format!("logical `!` requires BOOL operand, found `{ot}`"),
                            );
                        }
                        Some(bool_ty)
                    }
                    UnaryOp::Neg | UnaryOp::Pos => {
                        if let Some(ot) = &op_ty {
                            let ok = matches!(
                                ot,
                                TypeRef::Scalar { ty } if ty.is_signed_int() || ty.is_float()
                            );
                            if !ok {
                                self.push_err(
                                    ErrorCode::ErrTypeMismatch,
                                    expr.span,
                                    format!(
                                        "unary +/- requires signed numeric operand, found `{ot}`"
                                    ),
                                );
                            }
                        }
                        op_ty
                    }
                    UnaryOp::BitNot => {
                        if let Some(ot) = &op_ty
                            && !is_int_like_type(ot)
                        {
                            self.push_err(
                                ErrorCode::ErrTypeMismatch,
                                expr.span,
                                format!("bitwise `~` requires integer operand, found `{ot}`"),
                            );
                        }
                        op_ty
                    }
                };
                (
                    Expr::new(
                        ExprKind::Unary {
                            op: *op,
                            operand: Box::new(op_asl),
                        },
                        expr.span,
                    ),
                    result_ty,
                )
            }
            CppExprKind::Binary { op, left, right } => {
                self.lower_binary_expr(*op, left, right, expected, expr.span, scopes)
            }
            CppExprKind::Ternary {
                condition,
                then_expr,
                else_expr,
            } => {
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                let (c_asl, c_ty) = self.lower_expr(condition, Some(&bool_ty), scopes);
                if let Some(ct) = c_ty
                    && !types_compatible_cpp(&bool_ty, &ct)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("ternary condition must be BOOL, found `{ct}`"),
                    );
                }
                let (t_asl, t_ty) = self.lower_expr(then_expr, expected, scopes);
                let (e_asl, e_ty) = self.lower_expr(else_expr, t_ty.as_ref().or(expected), scopes);
                if let (Some(tt), Some(et)) = (&t_ty, &e_ty)
                    && !types_compatible_cpp(tt, et)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        expr.span,
                        format!("ternary branches have incompatible types `{tt}` and `{et}`"),
                    );
                }
                // IEC 61131-3 `SEL(G, IN0, IN1)` -> returns IN1 when G is true, IN0 when false
                (
                    Expr::new(
                        ExprKind::Call {
                            callee: "SEL".to_owned(),
                            args: vec![
                                CallArg {
                                    name: None,
                                    value: c_asl,
                                    span: condition.span,
                                },
                                CallArg {
                                    name: None,
                                    value: e_asl,
                                    span: else_expr.span,
                                },
                                CallArg {
                                    name: None,
                                    value: t_asl,
                                    span: then_expr.span,
                                },
                            ],
                        },
                        expr.span,
                    ),
                    t_ty.or(e_ty),
                )
            }
            CppExprKind::Cast { target, operand } => {
                let target_ty = self.resolve_type_spec(target);
                let (op_asl, from_ty) = self.lower_expr(operand, None, scopes);
                let Some(tty) = target_ty else {
                    return (op_asl, None);
                };
                if let Some(ref fty) = from_ty
                    && !is_valid_cpp_cast(fty, &tty)
                {
                    self.push_err(
                        ErrorCode::ErrInvalidCast,
                        expr.span,
                        format!("invalid cast from `{fty}` to `{tty}`"),
                    );
                }
                let callee = cast_callee_name(from_ty.as_ref(), &tty);
                (
                    Expr::new(
                        ExprKind::Call {
                            callee,
                            args: vec![CallArg {
                                name: None,
                                value: op_asl,
                                span: operand.span,
                            }],
                        },
                        expr.span,
                    ),
                    Some(tty),
                )
            }
            CppExprKind::Foreign { text } => {
                if let Some(exp) = expected {
                    (
                        Expr::new(
                            ExprKind::Foreign {
                                language: "cpp".to_owned(),
                                text: text.clone(),
                            },
                            expr.span,
                        ),
                        Some(exp.clone()),
                    )
                } else {
                    self.emit_foreign_untyped(text, expr.span);
                    (
                        Expr::new(
                            ExprKind::Foreign {
                                language: "cpp".to_owned(),
                                text: text.clone(),
                            },
                            expr.span,
                        ),
                        None,
                    )
                }
            }
        }
    }

    fn lower_binary_expr(
        &mut self,
        op: BinaryOp,
        left: &CppExpr,
        right: &CppExpr,
        expected: Option<&TypeRef>,
        span: Span,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> (Expr, Option<TypeRef>) {
        let bool_ty = TypeRef::scalar(ScalarType::Bool);
        match op {
            BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => {
                let (l_asl, l_ty) = self.lower_expr(left, Some(&bool_ty), scopes);
                let (r_asl, r_ty) = self.lower_expr(right, Some(&bool_ty), scopes);
                if let Some(lt) = l_ty
                    && !types_compatible_cpp(&bool_ty, &lt)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        left.span,
                        format!("logical operator requires BOOL, found `{lt}`"),
                    );
                }
                if let Some(rt) = r_ty
                    && !types_compatible_cpp(&bool_ty, &rt)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        right.span,
                        format!("logical operator requires BOOL, found `{rt}`"),
                    );
                }
                (
                    Expr::new(
                        ExprKind::Binary {
                            op,
                            left: Box::new(l_asl),
                            right: Box::new(r_asl),
                        },
                        span,
                    ),
                    Some(bool_ty),
                )
            }
            BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::Eq
            | BinaryOp::Ne => {
                // If left is a bare unsuffixed literal and right has a concrete variable type, infer right first
                let left_is_lit = matches!(
                    left.kind,
                    CppExprKind::IntLit {
                        unsigned: false,
                        long_count: 0,
                        ..
                    }
                );
                let (l_asl, l_ty, r_asl, r_ty) = if left_is_lit {
                    let (ra, rt) = self.lower_expr(right, None, scopes);
                    let (la, lt) = self.lower_expr(left, rt.as_ref(), scopes);
                    (la, lt, ra, rt)
                } else {
                    let (la, lt) = self.lower_expr(left, None, scopes);
                    let (ra, rt) = self.lower_expr(right, lt.as_ref(), scopes);
                    (la, lt, ra, rt)
                };
                if let (Some(lt), Some(rt)) = (&l_ty, &r_ty)
                    && !types_compatible_cpp(lt, rt)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        span,
                        format!("cannot compare incompatible types `{lt}` and `{rt}`"),
                    );
                }
                (
                    Expr::new(
                        ExprKind::Binary {
                            op,
                            left: Box::new(l_asl),
                            right: Box::new(r_asl),
                        },
                        span,
                    ),
                    Some(bool_ty),
                )
            }
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                let (l_asl, l_ty) = self.lower_expr(left, expected, scopes);
                let (r_asl, r_ty) = self.lower_expr(right, l_ty.as_ref().or(expected), scopes);
                if let (Some(lt), Some(rt)) = (&l_ty, &r_ty) {
                    let numeric_ok = matches!(
                        lt,
                        TypeRef::Scalar { ty } if ty.is_plain_numeric() || matches!(ty, ScalarType::Code { .. })
                    );
                    if !numeric_ok || !types_compatible_cpp(lt, rt) {
                        self.push_err(
                            ErrorCode::ErrTypeMismatch,
                            span,
                            format!("incompatible arithmetic operands `{lt}` and `{rt}`"),
                        );
                    }
                }
                (
                    Expr::new(
                        ExprKind::Binary {
                            op,
                            left: Box::new(l_asl),
                            right: Box::new(r_asl),
                        },
                        span,
                    ),
                    l_ty.or(r_ty),
                )
            }
            BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr => {
                let (l_asl, l_ty) = self.lower_expr(left, expected, scopes);
                let (r_asl, r_ty) = self.lower_expr(right, l_ty.as_ref().or(expected), scopes);
                if let (Some(lt), Some(rt)) = (&l_ty, &r_ty) {
                    let is_shift = matches!(op, BinaryOp::Shl | BinaryOp::Shr);
                    let ok = is_int_like_type(lt)
                        && is_int_like_type(rt)
                        && (is_shift || types_compatible_cpp(lt, rt));
                    if !ok {
                        self.push_err(
                            ErrorCode::ErrTypeMismatch,
                            span,
                            format!("invalid bitwise operands `{lt}` and `{rt}`"),
                        );
                    }
                }
                (
                    Expr::new(
                        ExprKind::Binary {
                            op,
                            left: Box::new(l_asl),
                            right: Box::new(r_asl),
                        },
                        span,
                    ),
                    l_ty.or(r_ty),
                )
            }
        }
    }

    fn lower_and_check_call(
        &mut self,
        callee: &str,
        args: &[CppExpr],
        span: Span,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> (String, Vec<CallArg>, Option<TypeRef>) {
        // 1. Arduino built-in expression intrinsics
        match callee {
            "digitalRead" => {
                if args.len() != 1 {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        span,
                        format!("`digitalRead` expects 1 argument, found {}", args.len()),
                    );
                    return (
                        "DIGITAL_READ".to_owned(),
                        Vec::new(),
                        Some(TypeRef::scalar(ScalarType::Bool)),
                    );
                }
                let Some(pin_arg) = args.first() else {
                    return (
                        "DIGITAL_READ".to_owned(),
                        Vec::new(),
                        Some(TypeRef::scalar(ScalarType::Bool)),
                    );
                };
                let label = self.resolve_existing_pin_label(pin_arg);
                return (
                    "DIGITAL_READ".to_owned(),
                    vec![CallArg {
                        name: None,
                        value: Expr::new(ExprKind::Ident(label), pin_arg.span),
                        span: pin_arg.span,
                    }],
                    Some(TypeRef::scalar(ScalarType::Bool)),
                );
            }
            "analogRead" => {
                let codec_id =
                    default_adc_codec_id(self.ctx).unwrap_or_else(|| "uno_10bit".to_owned());
                let ret_ty = TypeRef::scalar(ScalarType::Code { codec_id });
                if args.len() != 1 {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        span,
                        format!("`analogRead` expects 1 argument, found {}", args.len()),
                    );
                    return ("ADC_CODE".to_owned(), Vec::new(), Some(ret_ty));
                }
                let Some(pin_arg) = args.first() else {
                    return ("ADC_CODE".to_owned(), Vec::new(), Some(ret_ty));
                };
                let label = self.resolve_existing_pin_label(pin_arg);
                return (
                    "ADC_CODE".to_owned(),
                    vec![CallArg {
                        name: None,
                        value: Expr::new(ExprKind::Ident(label), pin_arg.span),
                        span: pin_arg.span,
                    }],
                    Some(ret_ty),
                );
            }
            "millis" => {
                if !args.is_empty() {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        span,
                        format!("`millis` expects 0 arguments, found {}", args.len()),
                    );
                }
                return (
                    "MILLIS".to_owned(),
                    Vec::new(),
                    Some(TypeRef::scalar(ScalarType::Udint)),
                );
            }
            "micros" => {
                if !args.is_empty() {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        span,
                        format!("`micros` expects 0 arguments, found {}", args.len()),
                    );
                }
                return (
                    "MICROS".to_owned(),
                    Vec::new(),
                    Some(TypeRef::scalar(ScalarType::Udint)),
                );
            }
            "DISPLAY_PRINT" => {
                if args.len() != 2 {
                    self.push_err(
                        ErrorCode::ErrArityMismatch,
                        span,
                        format!("`DISPLAY_PRINT` expects 2 arguments, found {}", args.len()),
                    );
                }
                let mut asl_args = Vec::with_capacity(args.len());
                for a in args {
                    let (v, _) = self.lower_expr(a, None, scopes);
                    asl_args.push(CallArg {
                        name: None,
                        value: v,
                        span: a.span,
                    });
                }
                // Enforce `ERR_STRING_CHARSET` strictly on HD44780/LCD display sinks (DEF-007)
                if let (Some(comp_arg), Some(text_arg)) = (args.first(), args.get(1))
                    && let CppExprKind::StringLit(comp_id) = &comp_arg.kind
                    && let CppExprKind::StringLit(text) = &text_arg.kind
                {
                    let lower_id = comp_id.to_ascii_lowercase();
                    if (lower_id.contains("hd44780") || lower_id.contains("lcd"))
                        && let Some(bad_ch) = text.chars().find(|&c| !is_hd44780_printable(c))
                    {
                        self.push_err(
                            ErrorCode::ErrStringCharset,
                            text_arg.span,
                            format!(
                                "display `{comp_id}` charset `hd44780_a00` does not support glyph `{bad_ch}` (U+{:04X})",
                                u32::from(bad_ch)
                            ),
                        );
                    }
                }
                return ("DISPLAY_PRINT".to_owned(), asl_args, Some(TypeRef::Void));
            }
            _ => {}
        }

        // 2. Map idiomatic Arduino/C++ math aliases to canonical `std.iec.math` names
        let mapped_name = match callee {
            "abs" => "ABS",
            "min" => "MIN",
            "max" => "MAX",
            "constrain" => "CLAMP",
            "map" => "MAP_RANGE",
            "sqrt" => "SQRT",
            "sin" => "SIN",
            "cos" => "COS",
            "tan" => "TAN",
            "atan2" => "ATAN2",
            other => other,
        };

        // 3. Explicit cast callee (`TO_UDINT`, `INT_TO_DINT`, etc.)
        if let Some(target_ty) = cast_target_type_from_callee(mapped_name) {
            if args.len() != 1 {
                self.push_err(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!("`{mapped_name}` expects 1 argument, found {}", args.len()),
                );
                return (mapped_name.to_owned(), Vec::new(), Some(target_ty));
            }
            let Some(arg0) = args.first() else {
                return (mapped_name.to_owned(), Vec::new(), Some(target_ty));
            };
            let (v, from_ty) = self.lower_expr(arg0, None, scopes);
            if let Some(ft) = from_ty
                && !is_valid_cpp_cast(&ft, &target_ty)
            {
                self.push_err(
                    ErrorCode::ErrInvalidCast,
                    span,
                    format!("invalid conversion `{mapped_name}` from `{ft}` to `{target_ty}`"),
                );
            }
            return (
                mapped_name.to_owned(),
                vec![CallArg {
                    name: None,
                    value: v,
                    span: arg0.span,
                }],
                Some(target_ty),
            );
        }

        // 4. User-defined FC POU
        if let Some((params, ret_ty)) = self.user_fcs.get(callee).cloned() {
            if args.len() != params.len() {
                self.push_err(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!(
                        "function `{callee}` expects {} argument(s), found {}",
                        params.len(),
                        args.len()
                    ),
                );
            }
            let mut call_args = Vec::with_capacity(args.len());
            for (idx, arg) in args.iter().enumerate() {
                let expected = params.get(idx).map(|p| &p.ty);
                let (v, actual) = self.lower_expr(arg, expected, scopes);
                if let (Some(exp), Some(act)) = (expected, actual)
                    && !types_compatible_cpp(exp, &act)
                {
                    self.push_err(
                        ErrorCode::ErrTypeMismatch,
                        arg.span,
                        format!(
                            "argument {} to `{callee}` has type `{act}`, expected `{exp}`",
                            idx.saturating_add(1)
                        ),
                    );
                }
                call_args.push(CallArg {
                    name: None,
                    value: v,
                    span: arg.span,
                });
            }
            return (callee.to_owned(), call_args, Some(ret_ty));
        }

        // 5. Built-in ASL stdlib (`std.iec.*` and `std.arduino.*` in Arduino C++ scope)
        let scope_imports = &["std.iec.*", "std.arduino.*"];
        if let Some(StdSymbol::Function(sig)) =
            self.std_catalog.resolve_symbol(scope_imports, mapped_name)
        {
            if args.len() != sig.params.len() {
                self.push_err(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!(
                        "stdlib function `{mapped_name}` expects {} argument(s), found {}",
                        sig.params.len(),
                        args.len()
                    ),
                );
            }
            let mut call_args = Vec::with_capacity(args.len());
            let mut any_num_ty: Option<TypeRef> = None;
            for (idx, arg) in args.iter().enumerate() {
                let param = sig.params.get(idx);
                let expected = param.and_then(|p| match p.ty {
                    StdTypeRef::AnyNum => any_num_ty.clone(),
                    StdTypeRef::PinRef => None,
                    StdTypeRef::Void
                    | StdTypeRef::Bool
                    | StdTypeRef::Sint
                    | StdTypeRef::Int
                    | StdTypeRef::Dint
                    | StdTypeRef::Lint
                    | StdTypeRef::Usint
                    | StdTypeRef::Uint
                    | StdTypeRef::Udint
                    | StdTypeRef::Ulint
                    | StdTypeRef::Byte
                    | StdTypeRef::Word
                    | StdTypeRef::Dword
                    | StdTypeRef::Lword
                    | StdTypeRef::Real
                    | StdTypeRef::Lreal
                    | StdTypeRef::Time
                    | StdTypeRef::String
                    | StdTypeRef::Analog
                    | StdTypeRef::Duty
                    | StdTypeRef::Code => Some(std_type_ref_to_asl(p.ty)),
                });
                let (v, actual) = self.lower_expr(arg, expected.as_ref(), scopes);
                if let Some(p) = param {
                    if p.ty == StdTypeRef::AnyNum {
                        if any_num_ty.is_none() {
                            any_num_ty = actual.clone();
                        }
                    } else if p.ty != StdTypeRef::PinRef
                        && let (Some(exp), Some(act)) = (&expected, &actual)
                        && !types_compatible_cpp(exp, act)
                    {
                        self.push_err(
                            ErrorCode::ErrTypeMismatch,
                            arg.span,
                            format!(
                                "argument `{}` to `{mapped_name}` has type `{act}`, expected `{exp}`",
                                p.name
                            ),
                        );
                    }
                }
                call_args.push(CallArg {
                    name: None,
                    value: v,
                    span: arg.span,
                });
            }
            let ret_ty = if sig.return_type == StdTypeRef::AnyNum {
                any_num_ty.unwrap_or_else(|| TypeRef::scalar(self.ctx.c_int_asl_type()))
            } else {
                std_type_ref_to_asl(sig.return_type)
            };
            return (mapped_name.to_owned(), call_args, Some(ret_ty));
        }

        // 6. Unresolved function call!
        self.push_err(
            ErrorCode::ErrUnresolvedFunction,
            span,
            format!("unresolved function `{callee}`"),
        );
        let mut fallback_args = Vec::with_capacity(args.len());
        for a in args {
            let (v, _) = self.lower_expr(a, None, scopes);
            fallback_args.push(CallArg {
                name: None,
                value: v,
                span: a.span,
            });
        }
        (callee.to_owned(), fallback_args, None)
    }

    fn emit_foreign_untyped(&mut self, text: &str, span: Span) {
        let site = ForeignSite {
            level: ForeignLevel::Expr,
            node_id: None,
            language: "cpp",
            fidelity: ForeignFidelity::F0,
            text,
            pack_hint: None,
            span,
        };
        let diag = foreign_untyped_diagnostic(&site, Some(&self.ctx.board_id));
        self.errors.push(diag);
    }

    fn check_array_index(
        &mut self,
        base_ty: &TypeRef,
        index: &Expr,
        span: Span,
    ) -> Option<TypeRef> {
        let TypeRef::Array { elem, len } = base_ty else {
            self.push_err(
                ErrorCode::ErrTypeMismatch,
                span,
                format!("cannot index into non-array type `{base_ty}`"),
            );
            return None;
        };
        if let Some(const_idx) = eval_const_asl_int(index)
            && (const_idx < 0 || const_idx >= i128::from(*len))
        {
            self.push_err(
                ErrorCode::ErrArrayBoundsConst,
                index.span,
                format!("constant array index {const_idx} out of bounds for ARRAY[{len}]"),
            );
        }
        Some((**elem).clone())
    }

    fn check_field_access(
        &mut self,
        base_ty: &TypeRef,
        field: &str,
        span: Span,
    ) -> Option<TypeRef> {
        match base_ty {
            TypeRef::Named { name } => {
                if let Some(fields) = self.structs.get(name)
                    && let Some(ft) = fields.get(field)
                {
                    return Some(ft.clone());
                }
                self.push_err(
                    ErrorCode::ErrUnknownFieldAccess,
                    span,
                    format!("struct `{name}` has no field `.{field}`"),
                );
                None
            }
            TypeRef::Fb { fb_kind } => {
                let fields = self.fb_all_fields(fb_kind);
                if let Some(ft) = fields.get(field) {
                    Some(ft.clone())
                } else {
                    self.push_err(
                        ErrorCode::ErrUnknownFieldAccess,
                        span,
                        format!("Function Block `{base_ty}` has no field `.{field}`"),
                    );
                    None
                }
            }
            TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Array { .. } => {
                self.push_err(
                    ErrorCode::ErrUnknownFieldAccess,
                    span,
                    format!("type `{base_ty}` does not support field access `.{field}`"),
                );
                None
            }
        }
    }

    fn fb_fields_of_var(
        &mut self,
        var_name: &str,
        span: Span,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> Option<BTreeMap<String, TypeRef>> {
        match self.lookup_symbol(var_name, scopes).cloned() {
            Some(SymbolInfo::Data {
                ty: TypeRef::Fb { fb_kind },
                ..
            }) => Some(self.fb_all_fields(&fb_kind)),
            Some(_) => {
                self.push_err(
                    ErrorCode::ErrTypeMismatch,
                    span,
                    format!("`{var_name}` is not a Function Block instance"),
                );
                None
            }
            None => {
                self.push_err(
                    ErrorCode::ErrUndeclaredSymbol,
                    span,
                    format!("undeclared Function Block instance `{var_name}`"),
                );
                None
            }
        }
    }

    fn fb_all_fields(&self, fb_kind: &FbKind) -> BTreeMap<String, TypeRef> {
        let Some(std_name) = fb_std_name(fb_kind) else {
            return BTreeMap::new();
        };
        let mut map = BTreeMap::new();
        if let Some(StdSymbol::FunctionBlock(sig)) =
            self.std_catalog.resolve_symbol(&["std.iec.*"], std_name)
        {
            for f in sig.fields {
                map.insert(f.name.to_owned(), std_type_ref_to_asl(f.ty));
            }
        }
        map
    }

    fn fb_input_fields_ordered(&self, fb_kind: &FbKind) -> Vec<(String, TypeRef)> {
        let Some(std_name) = fb_std_name(fb_kind) else {
            return Vec::new();
        };
        if let Some(StdSymbol::FunctionBlock(sig)) =
            self.std_catalog.resolve_symbol(&["std.iec.*"], std_name)
        {
            return sig
                .fields
                .iter()
                .filter(|f| f.direction == ParamDir::In)
                .map(|f| (f.name.to_owned(), std_type_ref_to_asl(f.ty)))
                .collect();
        }
        Vec::new()
    }

    fn lookup_symbol<'b>(
        &'b self,
        name: &str,
        scopes: &'b [BTreeMap<String, SymbolInfo>],
    ) -> Option<&'b SymbolInfo> {
        for scope in scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        self.globals.get(name)
    }

    fn resolve_ident_type(
        &mut self,
        name: &str,
        span: Span,
        scopes: &[BTreeMap<String, SymbolInfo>],
    ) -> Option<TypeRef> {
        match self.lookup_symbol(name, scopes).cloned() {
            Some(
                SymbolInfo::Data { ty, .. }
                | SymbolInfo::Param { ty, .. }
                | SymbolInfo::Hardware { ty },
            ) => Some(ty),
            None => {
                if let Some((enum_name, _)) = self.enum_variants.get(name) {
                    return Some(TypeRef::Named {
                        name: enum_name.clone(),
                    });
                }
                self.push_err(
                    ErrorCode::ErrUndeclaredSymbol,
                    span,
                    format!("undeclared symbol `{name}`"),
                );
                None
            }
        }
    }

    /// Anchors every merged verbatim [`RawComment`] to an AST node or board header/body,
    /// strictly honoring `C1` (zero dropped comments) and `C5` (`ERR_COMMENT_FIELD` invariants).
    fn anchor_comments(
        &self,
        lexed: &LexedFile,
        _program: &BoardProgram,
    ) -> (Vec<Comment>, Vec<Diagnostic>) {
        let mut comments = Vec::with_capacity(lexed.comments.len());
        let mut diags = Vec::new();

        // Sort anchors by span start (tie-break: wider containers first)
        let mut sorted_anchors = self.anchors.clone();
        sorted_anchors.sort_by_key(|a| (a.span.start, std::cmp::Reverse(a.span.end)));

        let first_anchor_start = sorted_anchors.first().map(|a| a.span.start);

        for (idx, raw) in lexed.comments.iter().enumerate() {
            if contains_bidi_control(&raw.text) {
                let mut d = Diagnostic::new(
                    ErrorCode::WarnCommentBidi,
                    "source comment contains Unicode bidirectional control characters",
                )
                .with_board_id(&self.ctx.board_id);
                d.span = Some(raw.span);
                diags.push(d);
            }

            let (anchor, placement, token_ordinal) =
                self.classify_comment(raw, &sorted_anchors, &lexed.tokens, first_anchor_start);

            comments.push(Comment {
                id: format!("c_{:04}", idx.saturating_add(1)),
                anchor,
                placement,
                token_ordinal,
                text: raw.text.clone(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: raw.style,
                },
                span: raw.span,
            });
        }

        (comments, diags)
    }

    fn classify_comment(
        &self,
        raw: &RawComment,
        anchors: &[AnchorRecord],
        tokens: &[Token],
        first_anchor_start: Option<u32>,
    ) -> (CommentAnchor, AstCommentPlacement, Option<u32>) {
        // 0. Empty program (no nodes): all comments anchor to Board Header
        if anchors.is_empty() {
            return (
                CommentAnchor::Board {
                    id: self.ctx.board_id.clone(),
                },
                AstCommentPlacement::Header,
                None,
            );
        }

        // 1. Inline / trailing comment (code appeared earlier on the same line)
        if raw.preceded_by_code_on_line {
            // Check if strictly inside a non-container statement/declaration -> `Inner` with `token_ordinal`
            if matches!(raw.style, CommentStyle::Block | CommentStyle::DocBlock)
                && let Some(inner_node) = anchors
                    .iter()
                    .filter(|a| {
                        !a.is_scope_container
                            && a.span.start < raw.span.start
                            && raw.span.end < a.span.end
                    })
                    .min_by_key(|a| a.span.len())
            {
                let ord = tokens
                    .iter()
                    .filter(|t| {
                        t.span.start >= inner_node.span.start && t.span.end <= raw.span.start
                    })
                    .count();
                return (
                    CommentAnchor::Node {
                        id: inner_node.id.clone(),
                    },
                    AstCommentPlacement::Inner,
                    Some(u32::try_from(ord).unwrap_or(0)),
                );
            }

            // Trailing comment on the same line as the preceding node
            if let Some(prev_node) = anchors
                .iter()
                .filter(|a| a.end_line == raw.start_line && a.span.start <= raw.span.start)
                .max_by_key(|a| (a.span.end, std::cmp::Reverse(a.span.len())))
            {
                return (
                    CommentAnchor::Node {
                        id: prev_node.id.clone(),
                    },
                    AstCommentPlacement::Trailing,
                    None,
                );
            }
        }

        // 2. File header doc comments (`//!`, `/*!`) before first declaration
        if let Some(first_start) = first_anchor_start
            && raw.span.end <= first_start
            && let Some(raw_slice) = raw.span.slice(self.src)
            && (raw_slice.starts_with("//!") || raw_slice.starts_with("/*!"))
        {
            return (
                CommentAnchor::Board {
                    id: self.ctx.board_id.clone(),
                },
                AstCommentPlacement::Header,
                None,
            );
        }

        // 3. Find innermost enclosing scope container (`Pou`, `If`, `While`, `For`, `Switch`)
        let enclosing_scope = anchors
            .iter()
            .filter(|a| {
                a.is_scope_container
                    && a.body_span
                        .is_some_and(|bs| bs.start <= raw.span.start && raw.span.end <= bs.end)
            })
            .min_by_key(|a| a.span.len());

        // 4. Leading comment on next sibling node inside the same scope
        let next_node = if let Some(scope) = enclosing_scope {
            let scope_end = scope.body_span.map_or(scope.span.end, |bs| bs.end);
            anchors
                .iter()
                .filter(|a| {
                    a.id != scope.id && a.span.start >= raw.span.end && a.span.end <= scope_end
                })
                .min_by_key(|a| (a.span.start, std::cmp::Reverse(a.span.len())))
        } else {
            anchors
                .iter()
                .filter(|a| a.span.start >= raw.span.end)
                .min_by_key(|a| (a.span.start, std::cmp::Reverse(a.span.len())))
        };

        if let Some(next) = next_node {
            // Check if this is a standalone top-of-file header separated by blank line(s) from the first node
            if enclosing_scope.is_none()
                && first_anchor_start == Some(next.span.start)
                && next.start_line > raw.end_line.saturating_add(1)
            {
                return (
                    CommentAnchor::Board {
                        id: self.ctx.board_id.clone(),
                    },
                    AstCommentPlacement::Header,
                    None,
                );
            }
            return (
                CommentAnchor::Node {
                    id: next.id.clone(),
                },
                AstCommentPlacement::Leading,
                None,
            );
        }

        // 5. Dangling comment inside a POU or block (before closing `}`)
        if let Some(scope) = enclosing_scope {
            return (
                CommentAnchor::Node {
                    id: scope.id.clone(),
                },
                AstCommentPlacement::Dangling,
                None,
            );
        }

        // 6. Trailing footer comment after all top-level declarations (satisfies C5: Board + Body)
        (
            CommentAnchor::Board {
                id: self.ctx.board_id.clone(),
            },
            AstCommentPlacement::Body,
            None,
        )
    }

    fn record_blank_lines(&self, tokens: &[Token], out: &mut BTreeMap<NodeId, u8>) {
        for anchor in &self.anchors {
            if let Some(tok) = tokens.iter().find(|t| t.span.start == anchor.span.start)
                && tok.blank_lines_before > 0
            {
                let clamped = u8::try_from(tok.blank_lines_before.min(2)).unwrap_or(2);
                out.insert(anchor.id.clone(), clamped);
            }
        }
    }
}

struct FnLowerCtx<'a> {
    pou_name: &'a str,
    expected_return: &'a TypeRef,
    in_loop: bool,
    in_isr: bool,
    at_function_top: bool,
}

fn map_include_to_asl_import(header: &str, is_angle: bool) -> Option<String> {
    if !is_angle {
        let stem = header
            .strip_suffix(".hpp")
            .or_else(|| header.strip_suffix(".h"))
            .unwrap_or(header);
        if stem.eq_ignore_ascii_case("Arduino") {
            return Some("std.arduino.*".to_owned());
        }
        return Some(format!("project.{stem}"));
    }
    match header {
        "Arduino.h" => Some("std.arduino.*".to_owned()),
        "stdint.h" | "cstdint" => Some("std.iec.*".to_owned()),
        "math.h" | "cmath" => Some("std.iec.math".to_owned()),
        other => other
            .strip_prefix("std/")
            .and_then(|s| s.strip_suffix(".h"))
            .map(|rest| format!("std.{}", rest.replace('/', "."))),
    }
}

fn pack_hint_for_header(header: &str) -> Option<String> {
    match header {
        "Wire.h" => Some("arduino-wire".to_owned()),
        "SPI.h" => Some("arduino-spi".to_owned()),
        "Servo.h" => Some("arduino-servo".to_owned()),
        _ => None,
    }
}

fn hardware_binding_asl_type(hw: &HardwareBinding) -> TypeRef {
    match hw.kind {
        HardwareKind::DigitalIn | HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional => {
            TypeRef::scalar(ScalarType::Bool)
        }
        HardwareKind::AnalogIn | HardwareKind::AnalogOut => TypeRef::scalar(ScalarType::Analog),
        HardwareKind::PwmOut => TypeRef::scalar(ScalarType::Duty),
        HardwareKind::BusPin => TypeRef::scalar(ScalarType::Byte),
    }
}

fn lvalue_to_expr(lval: &LValue) -> Expr {
    match lval {
        LValue::Var { name, span } => Expr::new(ExprKind::Ident(name.clone()), *span),
        LValue::Index { base, index, span } => Expr::new(
            ExprKind::Index {
                base: Box::new(lvalue_to_expr(base)),
                index: index.clone(),
            },
            *span,
        ),
        LValue::Field { base, field, span } => Expr::new(
            ExprKind::Field {
                base: Box::new(lvalue_to_expr(base)),
                field: field.clone(),
            },
            *span,
        ),
    }
}

fn eval_const_cpp_int(
    expr: &CppExpr,
    enum_variants: &BTreeMap<String, (String, i64)>,
) -> Option<i128> {
    match &expr.kind {
        CppExprKind::IntLit { value, .. } => Some(*value),
        CppExprKind::CharLit(c) => Some(i128::from(u32::from(*c))),
        CppExprKind::Ident(name) => enum_variants.get(name).map(|(_, v)| i128::from(*v)),
        CppExprKind::ScopedIdent { scope, member } => {
            let key = format!("{scope}::{member}");
            enum_variants.get(&key).map(|(_, v)| i128::from(*v))
        }
        CppExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => eval_const_cpp_int(operand, enum_variants)?.checked_neg(),
        CppExprKind::Unary {
            op: UnaryOp::Pos,
            operand,
        } => eval_const_cpp_int(operand, enum_variants),
        CppExprKind::Binary { op, left, right } => {
            let l = eval_const_cpp_int(left, enum_variants)?;
            let r = eval_const_cpp_int(right, enum_variants)?;
            match op {
                BinaryOp::Add => l.checked_add(r),
                BinaryOp::Sub => l.checked_sub(r),
                BinaryOp::Mul => l.checked_mul(r),
                BinaryOp::Div => l.checked_div(r),
                BinaryOp::Mod => l.checked_rem(r),
                BinaryOp::Shl => u32::try_from(r).ok().and_then(|s| l.checked_shl(s)),
                BinaryOp::Shr => u32::try_from(r).ok().and_then(|s| l.checked_shr(s)),
                BinaryOp::BitAnd => Some(l & r),
                BinaryOp::BitOr => Some(l | r),
                BinaryOp::BitXor => Some(l ^ r),
                BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::And
                | BinaryOp::Xor
                | BinaryOp::Or => None,
            }
        }
        CppExprKind::BoolLit(_)
        | CppExprKind::FloatLit { .. }
        | CppExprKind::StringLit(_)
        | CppExprKind::Index { .. }
        | CppExprKind::Field { .. }
        | CppExprKind::Call { .. }
        | CppExprKind::MethodCall { .. }
        | CppExprKind::DesignatedCall { .. }
        | CppExprKind::Unary {
            op: UnaryOp::Not | UnaryOp::BitNot,
            ..
        }
        | CppExprKind::Ternary { .. }
        | CppExprKind::Cast { .. }
        | CppExprKind::Foreign { .. } => None,
    }
}

fn eval_const_asl_int(expr: &Expr) -> Option<i128> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(n)) => Some(*n),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => eval_const_asl_int(operand)?.checked_neg(),
        ExprKind::Unary {
            op: UnaryOp::Pos,
            operand,
        } => eval_const_asl_int(operand),
        ExprKind::Binary { op, left, right } => {
            let l = eval_const_asl_int(left)?;
            let r = eval_const_asl_int(right)?;
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

fn fb_std_name(fb_kind: &FbKind) -> Option<&'static str> {
    match fb_kind {
        FbKind::Ton => Some("TON"),
        FbKind::Tof => Some("TOF"),
        FbKind::Tp => Some("TP"),
        FbKind::Ctu => Some("CTU"),
        FbKind::Ctd => Some("CTD"),
        FbKind::Ctud => Some("CTUD"),
        FbKind::RTrig => Some("R_TRIG"),
        FbKind::FTrig => Some("F_TRIG"),
        FbKind::Sr => Some("SR"),
        FbKind::Rs => Some("RS"),
        FbKind::UartSend => Some("UART_SEND"),
        FbKind::UartRecv => Some("UART_RECV"),
        FbKind::I2cXfer => Some("I2C_XFER"),
        FbKind::SpiXfer => Some("SPI_XFER"),
        FbKind::MbClient => Some("MB_CLIENT"),
        FbKind::MbServer => Some("MB_SERVER"),
        FbKind::User { .. } => None,
    }
}

fn std_type_ref_to_asl(ty: StdTypeRef) -> TypeRef {
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

/// Well-known Arduino bit/SPI/serial constants.
fn arduino_builtin_int_constant(name: &str) -> Option<i128> {
    match name {
        "LSBFIRST" => Some(0),
        "MSBFIRST" => Some(1),
        "SPI_MODE0" => Some(0),
        "SPI_MODE1" => Some(1),
        "SPI_MODE2" => Some(2),
        "SPI_MODE3" => Some(3),
        "DEC" => Some(10),
        "HEX" => Some(16),
        "OCT" => Some(8),
        "BIN" => Some(2),
        _ => None,
    }
}

/// HD44780 A00 ROM printable character set (ASCII `0x20..=0x7E` plus `°`, `µ`).
fn is_hd44780_printable(ch: char) -> bool {
    matches!(ch, ' '..='~' | '°' | 'µ' | 'μ')
}
