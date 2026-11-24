//! Static semantic, type, hardware, import-cycle, and `Foreign`/`F1` pack validator
//! for parsed IEC 61131-3 Structured Text (`docs/lbc/st.md` §1.4, §2–§3, §5–§6).

use crate::lexer::fits_scalar_int_range;
use crate::parser::ParsedSt;
use dendri_ast::{
    ASL_ANALOG_FULL_SCALE, BinaryOp, CallArg, CasePattern, DataDecl, Expr, ExprKind, FbKind,
    ForeignFidelity, HardwareBinding, HardwareKind, LValue, Literal, NodeId, ObKind, ParamDecl,
    ParamDirection, Pou, PouBody, PouKind, ScalarType, Stmt, StmtKind, StorageScope, TypeDecl,
    TypeRef, UnaryOp,
};
use dendri_frontend_core::{
    BoardContext, ForeignLevel, ForeignSite, LanguageId, PackSymbol, PackSymbolKind,
    foreign_untyped_diagnostic,
};
use dendri_span::{Diagnostic, ErrorCode, Severity, Span};
use dendri_stdlib::{StdCatalog, StdDomain, StdSymbol, StdTypeRef};
use std::collections::{BTreeMap, BTreeSet};

/// Validates `parsed` against `ctx`, returning non-fatal diagnostics on success or all
/// diagnostics when any `Severity::Error` occurred.
pub(crate) fn validate_parsed_st(
    parsed: &ParsedSt,
    ctx: &BoardContext,
) -> Result<Vec<Diagnostic>, Vec<Diagnostic>> {
    let std_catalog = ctx.std_catalog();
    let mut checker = StChecker {
        parsed,
        ctx,
        std_catalog,
        diagnostics: Vec::new(),
        structs: BTreeMap::new(),
        enums: BTreeMap::new(),
        enum_variants: BTreeMap::new(),
        foreign_types: BTreeSet::new(),
        user_fbs: BTreeMap::new(),
        user_fcs: BTreeMap::new(),
        pack_fbs: BTreeMap::new(),
        globals: BTreeMap::new(),
        analog_in_codecs: BTreeMap::new(),
    };

    checker.check_all();

    let has_errors = checker
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error);
    if has_errors {
        Err(checker.diagnostics)
    } else {
        Ok(checker.diagnostics)
    }
}

#[derive(Debug, Clone)]
enum SymbolEntry {
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
    EnumConst {
        enum_name: String,
    },
}

struct StmtCtx<'a> {
    in_loop: bool,
    in_isr: bool,
    expected_return: &'a TypeRef,
    pou_name: &'a str,
    pou_id: &'a NodeId,
    imports: &'a [String],
}

struct StChecker<'a> {
    parsed: &'a ParsedSt,
    ctx: &'a BoardContext,
    std_catalog: StdCatalog,
    diagnostics: Vec<Diagnostic>,
    structs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    enums: BTreeMap<String, ScalarType>,
    enum_variants: BTreeMap<String, BTreeSet<String>>,
    foreign_types: BTreeSet<String>,
    user_fbs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    user_fcs: BTreeMap<String, (Vec<ParamDecl>, TypeRef)>,
    pack_fbs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    globals: BTreeMap<String, SymbolEntry>,
    analog_in_codecs: BTreeMap<String, String>,
}

impl<'a> StChecker<'a> {
    fn push_diag(&mut self, code: ErrorCode, span: Span, msg: impl Into<String>) {
        let mut d = Diagnostic::new(code, msg).with_board_id(&self.ctx.board_id);
        d.span = Some(span);
        self.diagnostics.push(d);
    }

    fn check_all(&mut self) {
        let program = &self.parsed.bundle.program;

        // 0. Index F1/F2 pack FB symbols loaded for ST
        for pack in &self.ctx.loaded_packs {
            if pack.pack.language == LanguageId::St {
                for sym in &pack.symbols {
                    if sym.kind == PackSymbolKind::Fb {
                        let mut fields = BTreeMap::new();
                        for p in &sym.params {
                            if let Some(ty) = p.type_ref() {
                                fields.insert(p.name.to_ascii_uppercase(), ty);
                            }
                        }
                        self.pack_fbs.insert(sym.name.to_ascii_uppercase(), fields);
                    }
                }
            }
        }

        // 1. Validate module names, USING imports & module import cycles
        let mut module_names = BTreeSet::new();
        for m in &program.modules {
            let key = m.name.to_ascii_uppercase();
            if !module_names.insert(key) {
                self.push_diag(
                    ErrorCode::ErrDuplicateDecl,
                    m.span,
                    format!("duplicate namespace/module `{}`", m.name),
                );
            }
        }
        for imp in &program.imports {
            self.check_import_path(imp, Span::default(), &module_names);
        }
        for m in &program.modules {
            for imp in &m.imports {
                self.check_import_path(imp, m.span, &module_names);
            }
        }
        self.check_module_cycles();

        // 2. Validate hardware bindings: pin conflicts & duplicate labels
        let mut used_pins = BTreeSet::new();
        let mut hw_labels = BTreeSet::new();
        for hw in &program.hardware {
            if !used_pins.insert(hw.pin.clone()) {
                self.push_diag(
                    ErrorCode::ErrHardwarePinConflict,
                    hw.span,
                    format!(
                        "hardware port `{}` is bound more than once (duplicate binding `{}`)",
                        hw.pin, hw.label
                    ),
                );
            }
            let key = hw.label.to_ascii_uppercase();
            if !hw_labels.insert(key.clone()) {
                self.push_diag(
                    ErrorCode::ErrDuplicateDecl,
                    hw.span,
                    format!("duplicate hardware tag label `{}`", hw.label),
                );
                continue;
            }
            if hw.kind == HardwareKind::AnalogIn
                && let Some(cid) = &hw.codec
            {
                self.analog_in_codecs.insert(key.clone(), cid.clone());
            }
            let ty = hardware_st_type(hw);
            self.globals.insert(
                key,
                SymbolEntry::Hardware {
                    binding: hw.clone(),
                    ty,
                },
            );
        }

        // 3. Collect types (structs, enums, Decl::Foreign)
        let mut seen_types = BTreeSet::new();
        let all_types = program
            .types
            .iter()
            .chain(program.modules.iter().flat_map(|m| m.types.iter()));
        for td in all_types {
            match td {
                TypeDecl::Struct {
                    name, fields, span, ..
                } => {
                    let key = name.to_ascii_uppercase();
                    if hw_labels.contains(&key) {
                        self.push_diag(
                            ErrorCode::ErrNameCollision,
                            *span,
                            format!("type `{name}` collides with hardware tag `{name}`"),
                        );
                    }
                    if !seen_types.insert(key.clone()) {
                        self.push_diag(
                            ErrorCode::ErrDuplicateDecl,
                            *span,
                            format!("duplicate type declaration `{name}`"),
                        );
                    }
                    let mut field_map = BTreeMap::new();
                    for f in fields {
                        let fkey = f.name.to_ascii_uppercase();
                        if field_map.insert(fkey, f.ty.clone()).is_some() {
                            self.push_diag(
                                ErrorCode::ErrDuplicateDecl,
                                *span,
                                format!("duplicate field `{}` in struct `{name}`", f.name),
                            );
                        }
                    }
                    self.structs.insert(key, field_map);
                }
                TypeDecl::Enum {
                    name,
                    backing,
                    variants,
                    span,
                    ..
                } => {
                    let key = name.to_ascii_uppercase();
                    if hw_labels.contains(&key) {
                        self.push_diag(
                            ErrorCode::ErrNameCollision,
                            *span,
                            format!("enum `{name}` collides with hardware tag `{name}`"),
                        );
                    }
                    if !seen_types.insert(key.clone()) {
                        self.push_diag(
                            ErrorCode::ErrDuplicateDecl,
                            *span,
                            format!("duplicate enum declaration `{name}`"),
                        );
                    }
                    self.enums.insert(key.clone(), backing.clone());
                    let mut vset = BTreeSet::new();
                    for v in variants {
                        let vkey = v.name.to_ascii_uppercase();
                        if !vset.insert(vkey.clone()) {
                            self.push_diag(
                                ErrorCode::ErrDuplicateDecl,
                                *span,
                                format!("duplicate variant `{}` in enum `{name}`", v.name),
                            );
                        }
                        self.globals.entry(vkey).or_insert(SymbolEntry::EnumConst {
                            enum_name: name.clone(),
                        });
                    }
                    self.enum_variants.insert(key, vset);
                }
                TypeDecl::Foreign { name, .. } => {
                    if let Some(n) = name {
                        self.foreign_types.insert(n.to_ascii_uppercase());
                    }
                }
            }
        }

        // 4. Collect global data declarations
        let mut seen_data = BTreeSet::new();
        let all_data: Vec<&DataDecl> = program
            .data
            .iter()
            .chain(program.modules.iter().flat_map(|m| m.data.iter()))
            .collect();
        for d in &all_data {
            let key = d.name.to_ascii_uppercase();
            if hw_labels.contains(&key) {
                self.push_diag(
                    ErrorCode::ErrNameCollision,
                    d.span,
                    format!("global variable `{}` collides with hardware tag", d.name),
                );
            } else if !seen_data.insert(key.clone()) {
                self.push_diag(
                    ErrorCode::ErrDuplicateDecl,
                    d.span,
                    format!("duplicate global variable `{}`", d.name),
                );
            } else {
                self.globals.insert(
                    key,
                    SymbolEntry::Data {
                        ty: d.ty.clone(),
                        scope: d.scope,
                    },
                );
            }
        }

        // 5. Collect POUs (OBs, FCs, FBs)
        let mut seen_pous = BTreeSet::new();
        let all_pous: Vec<(&Pou, Vec<String>)> = program
            .pous
            .iter()
            .map(|p| (p, program.imports.clone()))
            .chain(program.modules.iter().flat_map(|m| {
                let mut imps = program.imports.clone();
                imps.extend(m.imports.iter().cloned());
                m.pous.iter().map(move |p| (p, imps.clone()))
            }))
            .collect();

        for (pou, _) in &all_pous {
            let key = pou.name.to_ascii_uppercase();
            if hw_labels.contains(&key) {
                self.push_diag(
                    ErrorCode::ErrNameCollision,
                    pou.span,
                    format!("POU `{}` collides with hardware tag", pou.name),
                );
            }
            if !seen_pous.insert(key.clone()) {
                self.push_diag(
                    ErrorCode::ErrDuplicateDecl,
                    pou.span,
                    format!("duplicate POU declaration `{}`", pou.name),
                );
            }
            match &pou.kind {
                PouKind::Fc { return_type } => {
                    self.user_fcs
                        .insert(key, (pou.params.clone(), return_type.clone()));
                }
                PouKind::Fb => {
                    let mut fb_fields = BTreeMap::new();
                    for p in &pou.params {
                        fb_fields.insert(p.name.to_ascii_uppercase(), p.ty.clone());
                    }
                    self.user_fbs.insert(key, fb_fields);
                }
                PouKind::Ob { .. } => {}
            }
        }

        // 6. Validate struct field types & global data initializers
        for td in program
            .types
            .iter()
            .chain(program.modules.iter().flat_map(|m| m.types.iter()))
        {
            if let TypeDecl::Struct { fields, span, .. } = td {
                for f in fields {
                    self.check_type_ref(&f.ty, *span);
                }
            }
        }
        let empty_locals = BTreeMap::new();
        for d in &all_data {
            self.validate_data_decl(d, &empty_locals, &program.imports);
        }

        // 7. Validate every POU body
        for (pou, imps) in &all_pous {
            self.validate_pou(pou, imps);
        }
    }

    fn check_import_path(&mut self, imp: &str, span: Span, local_modules: &BTreeSet<String>) {
        if is_known_std_import(imp, &self.std_catalog) {
            return;
        }
        if let Some(local) = imp.strip_prefix("project.")
            && local_modules.contains(&local.to_ascii_uppercase())
        {
            return;
        }
        if local_modules.contains(&imp.to_ascii_uppercase()) {
            return;
        }
        self.push_diag(
            ErrorCode::ErrUnresolvedImport,
            span,
            format!("unresolved USING import `{imp}`"),
        );
    }

    fn check_module_cycles(&mut self) {
        let modules = &self.parsed.bundle.program.modules;
        if modules.is_empty() {
            return;
        }
        let mut graph: BTreeMap<String, (Span, Vec<String>)> = BTreeMap::new();
        for m in modules {
            let key = m.name.to_ascii_uppercase();
            let deps = m
                .imports
                .iter()
                .map(|imp| {
                    imp.strip_prefix("project.")
                        .unwrap_or(imp)
                        .to_ascii_uppercase()
                })
                .collect();
            graph.insert(key, (m.span, deps));
        }

        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let keys: Vec<String> = graph.keys().cloned().collect();
        for k in keys {
            if !visited.contains(&k) {
                self.dfs_module_cycle(&k, &graph, &mut visiting, &mut visited);
            }
        }
    }

    fn dfs_module_cycle(
        &mut self,
        node: &str,
        graph: &BTreeMap<String, (Span, Vec<String>)>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) {
        visiting.insert(node.to_owned());
        if let Some((span, deps)) = graph.get(node) {
            for dep in deps {
                if !graph.contains_key(dep) {
                    continue;
                }
                if visiting.contains(dep) {
                    self.push_diag(
                        ErrorCode::ErrImportCycle,
                        *span,
                        format!("cyclic USING import involving module `{node}` -> `{dep}`"),
                    );
                } else if !visited.contains(dep) {
                    self.dfs_module_cycle(dep, graph, visiting, visited);
                }
            }
        }
        visiting.remove(node);
        visited.insert(node.to_owned());
    }

    fn check_type_ref(&mut self, ty: &TypeRef, span: Span) {
        match ty {
            TypeRef::Void => {}
            TypeRef::Scalar { ty: s } => {
                if let ScalarType::Code { codec_id } = s
                    && !self.ctx.codecs.is_empty()
                    && !self.ctx.codecs.contains_key(codec_id)
                {
                    self.push_diag(
                        ErrorCode::ErrUnknownType,
                        span,
                        format!("unknown codec `{codec_id}` in `CODE<{codec_id}>`"),
                    );
                }
            }
            TypeRef::Array { elem, len } => {
                if *len == 0 {
                    self.push_diag(
                        ErrorCode::ErrArrayBoundsConst,
                        span,
                        "array length must be >= 1",
                    );
                }
                self.check_type_ref(elem, span);
            }
            TypeRef::Named { name } => {
                let key = name.to_ascii_uppercase();
                if !self.structs.contains_key(&key)
                    && !self.enums.contains_key(&key)
                    && !self.foreign_types.contains(&key)
                    && !self.user_fbs.contains_key(&key)
                    && !self.pack_fbs.contains_key(&key)
                {
                    self.push_diag(
                        ErrorCode::ErrUnknownType,
                        span,
                        format!("unknown type `{name}`"),
                    );
                }
            }
            TypeRef::Fb { fb_kind } => {
                if let FbKind::User { fb_name } = fb_kind {
                    let key = fb_name.to_ascii_uppercase();
                    if !self.user_fbs.contains_key(&key) && !self.pack_fbs.contains_key(&key) {
                        self.push_diag(
                            ErrorCode::ErrUnknownType,
                            span,
                            format!("unknown Function Block type `{fb_name}`"),
                        );
                    }
                }
            }
        }
    }

    fn validate_data_decl(
        &mut self,
        d: &DataDecl,
        locals: &BTreeMap<String, SymbolEntry>,
        imports: &[String],
    ) {
        self.check_type_ref(&d.ty, d.span);
        if let Some(init) = &d.initial {
            self.check_duty_overflow(&d.ty, init, d.span);
            let sctx = StmtCtx {
                in_loop: false,
                in_isr: false,
                expected_return: &TypeRef::Void,
                pou_name: &d.name,
                pou_id: &d.id,
                imports,
            };
            if let Some(init_ty) = self.infer_expr(init, Some(&d.ty), locals, &sctx)
                && !types_compatible(&d.ty, &init_ty)
            {
                self.push_diag(
                    ErrorCode::ErrTypeMismatch,
                    init.span,
                    format!(
                        "initializer for `{}` has type `{init_ty}`, expected `{}`",
                        d.name, d.ty
                    ),
                );
            }
        }
    }

    fn validate_pou(&mut self, pou: &Pou, imports: &[String]) {
        let mut locals = BTreeMap::new();
        for p in &pou.params {
            self.check_type_ref(&p.ty, pou.span);
            let key = p.name.to_ascii_uppercase();
            if locals.contains_key(&key) {
                self.push_diag(
                    ErrorCode::ErrDuplicateDecl,
                    pou.span,
                    format!("duplicate parameter `{}` in POU `{}`", p.name, pou.name),
                );
            }
            locals.insert(
                key,
                SymbolEntry::Param {
                    ty: p.ty.clone(),
                    writable: !matches!(p.direction, ParamDirection::In),
                },
            );
        }
        if let PouKind::Fc { return_type } = &pou.kind {
            self.check_type_ref(return_type, pou.span);
        }
        for l in &pou.locals {
            let key = l.name.to_ascii_uppercase();
            if locals.contains_key(&key) {
                self.push_diag(
                    ErrorCode::ErrDuplicateDecl,
                    l.span,
                    format!(
                        "duplicate local variable `{}` in POU `{}`",
                        l.name, pou.name
                    ),
                );
            }
            locals.insert(
                key,
                SymbolEntry::Data {
                    ty: l.ty.clone(),
                    scope: l.scope,
                },
            );
        }
        for l in &pou.locals {
            self.validate_data_decl(l, &locals, imports);
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
        let sctx = StmtCtx {
            in_loop: false,
            in_isr,
            expected_return: &expected_return,
            pou_name: &pou.name,
            pou_id: &pou.id,
            imports,
        };

        if let PouBody::Statements(stmts) = &pou.body {
            self.validate_stmts(stmts, &sctx, &locals);
        }
    }

    fn validate_stmts(
        &mut self,
        stmts: &[Stmt],
        sctx: &StmtCtx<'_>,
        locals: &BTreeMap<String, SymbolEntry>,
    ) {
        for stmt in stmts {
            self.validate_stmt(stmt, sctx, locals);
        }
    }

    fn validate_stmt(
        &mut self,
        stmt: &Stmt,
        sctx: &StmtCtx<'_>,
        locals: &BTreeMap<String, SymbolEntry>,
    ) {
        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                if let Some(target_ty) = self.check_lvalue_writable(target, stmt.span, locals, sctx)
                {
                    self.check_duty_overflow(&target_ty, value, stmt.span);
                    if let Some(val_ty) = self.infer_expr(value, Some(&target_ty), locals, sctx)
                        && !types_compatible(&target_ty, &val_ty)
                    {
                        self.push_diag(
                            ErrorCode::ErrTypeMismatch,
                            stmt.span,
                            format!("cannot assign `{val_ty}` to target of type `{target_ty}`"),
                        );
                    }
                } else {
                    let _ = self.infer_expr(value, None, locals, sctx);
                }
            }
            StmtKind::Call { callee, args } => {
                if sctx.in_isr
                    && (callee.eq_ignore_ascii_case("WAIT_TIME")
                        || callee.eq_ignore_ascii_case("WAIT_TIME_US"))
                {
                    self.push_diag(
                        ErrorCode::ErrIsrForbiddenOp,
                        stmt.span,
                        format!(
                            "blocking call `{callee}` is forbidden inside interrupt OB `{}`",
                            sctx.pou_name
                        ),
                    );
                }
                let _ = self.check_call(callee, args, stmt.span, locals, sctx);
            }
            StmtKind::FbInvoke { instance, inputs } => {
                let Some(inst_ty) = self.infer_lvalue(instance, stmt.span, locals, sctx) else {
                    return;
                };
                let fb_fields = self.fb_fields_for_type(&inst_ty);
                if fb_fields.is_empty() {
                    self.push_diag(
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
                    let key = name.to_ascii_uppercase();
                    if let Some(field_ty) = fb_fields.get(&key) {
                        if let Some(val_ty) = self.infer_expr(expr, Some(field_ty), locals, sctx)
                            && !types_compatible(field_ty, &val_ty)
                        {
                            self.push_diag(
                                ErrorCode::ErrTypeMismatch,
                                expr.span,
                                format!("FB input `{name}` expects `{field_ty}`, found `{val_ty}`"),
                            );
                        }
                    } else {
                        self.push_diag(
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
                if let Some(cty) = self.infer_expr(condition, Some(&bool_ty), locals, sctx)
                    && !types_compatible(&bool_ty, &cty)
                {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("`IF` condition must be BOOL, found `{cty}`"),
                    );
                }
                self.validate_stmts(then_body, sctx, locals);
                for arm in elif_arms {
                    if let Some(cty) = self.infer_expr(&arm.condition, Some(&bool_ty), locals, sctx)
                        && !types_compatible(&bool_ty, &cty)
                    {
                        self.push_diag(
                            ErrorCode::ErrTypeMismatch,
                            arm.condition.span,
                            format!("`ELSIF` condition must be BOOL, found `{cty}`"),
                        );
                    }
                    self.validate_stmts(&arm.body, sctx, locals);
                }
                self.validate_stmts(else_body, sctx, locals);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                let scrut_ty = self.infer_expr(scrutinee, None, locals, sctx);
                for c in cases {
                    for pat in &c.patterns {
                        match pat {
                            CasePattern::Value(e) => {
                                if let Some(pty) =
                                    self.infer_expr(e, scrut_ty.as_ref(), locals, sctx)
                                    && let Some(sty) = &scrut_ty
                                    && !types_compatible(sty, &pty)
                                {
                                    self.push_diag(
                                        ErrorCode::ErrTypeMismatch,
                                        e.span,
                                        format!(
                                            "`CASE` pattern `{pty}` does not match scrutinee `{sty}`"
                                        ),
                                    );
                                }
                            }
                            CasePattern::Range { start, end } => {
                                if let Some(pty) =
                                    self.infer_expr(start, scrut_ty.as_ref(), locals, sctx)
                                    && let Some(sty) = &scrut_ty
                                    && !types_compatible(sty, &pty)
                                {
                                    self.push_diag(
                                        ErrorCode::ErrTypeMismatch,
                                        start.span,
                                        format!(
                                            "`CASE` range start `{pty}` does not match scrutinee `{sty}`"
                                        ),
                                    );
                                }
                                if let Some(pty) =
                                    self.infer_expr(end, scrut_ty.as_ref(), locals, sctx)
                                    && let Some(sty) = &scrut_ty
                                    && !types_compatible(sty, &pty)
                                {
                                    self.push_diag(
                                        ErrorCode::ErrTypeMismatch,
                                        end.span,
                                        format!(
                                            "`CASE` range end `{pty}` does not match scrutinee `{sty}`"
                                        ),
                                    );
                                }
                            }
                        }
                    }
                    self.validate_stmts(&c.body, sctx, locals);
                }
                if let Some(def) = default {
                    self.validate_stmts(def, sctx, locals);
                }
            }
            StmtKind::While { condition, body } => {
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                if let Some(cty) = self.infer_expr(condition, Some(&bool_ty), locals, sctx)
                    && !types_compatible(&bool_ty, &cty)
                {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        condition.span,
                        format!("`WHILE` condition must be BOOL, found `{cty}`"),
                    );
                }
                let loop_ctx = StmtCtx {
                    in_loop: true,
                    ..*sctx
                };
                self.validate_stmts(body, &loop_ctx, locals);
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
                let counter_ty = self.check_lvalue_writable(&counter_lv, stmt.span, locals, sctx);
                for bound in [from, to, step] {
                    if let Some(bty) = self.infer_expr(bound, counter_ty.as_ref(), locals, sctx)
                        && let Some(cty) = &counter_ty
                        && !types_compatible(cty, &bty)
                    {
                        self.push_diag(
                            ErrorCode::ErrTypeMismatch,
                            bound.span,
                            format!("`FOR` bound `{bty}` does not match counter type `{cty}`"),
                        );
                    }
                }
                let loop_ctx = StmtCtx {
                    in_loop: true,
                    ..*sctx
                };
                self.validate_stmts(body, &loop_ctx, locals);
            }
            StmtKind::Break => {
                if !sctx.in_loop {
                    self.push_diag(
                        ErrorCode::ErrBreakOutsideLoop,
                        stmt.span,
                        "`EXIT` statement is only valid inside a `FOR`, `WHILE`, or `REPEAT` loop",
                    );
                }
            }
            StmtKind::Continue => {
                if !sctx.in_loop {
                    self.push_diag(
                        ErrorCode::ErrContinueOutsideLoop,
                        stmt.span,
                        "`CONTINUE` statement is only valid inside a `FOR`, `WHILE`, or `REPEAT` loop",
                    );
                }
            }
            StmtKind::Return { value } => match (sctx.expected_return, value) {
                (TypeRef::Void, None) => {}
                (TypeRef::Void, Some(expr)) => {
                    let _ = self.infer_expr(expr, None, locals, sctx);
                    self.push_diag(
                        ErrorCode::ErrReturnTypeMismatch,
                        stmt.span,
                        format!(
                            "POU `{}` returns VOID and cannot return a value",
                            sctx.pou_name
                        ),
                    );
                }
                (_, None) => {
                    // Bare `RETURN;` inside a FUNCTION is valid in IEC 61131-3 when a prior return assignment occurred.
                }
                (expected, Some(expr)) => {
                    if let Some(actual) = self.infer_expr(expr, Some(expected), locals, sctx)
                        && !types_compatible(expected, &actual)
                    {
                        self.push_diag(
                            ErrorCode::ErrReturnTypeMismatch,
                            stmt.span,
                            format!(
                                "return expression has type `{actual}`, expected `{expected}` in `FUNCTION {}`",
                                sctx.pou_name
                            ),
                        );
                    }
                }
            },
            StmtKind::Wait { duration } => {
                if sctx.in_isr {
                    self.push_diag(
                        ErrorCode::ErrIsrForbiddenOp,
                        stmt.span,
                        format!(
                            "blocking `WAIT` is forbidden inside interrupt OB `{}`",
                            sctx.pou_name
                        ),
                    );
                }
                let time_ty = TypeRef::scalar(ScalarType::Time);
                if let Some(dty) = self.infer_expr(duration, Some(&time_ty), locals, sctx)
                    && !types_compatible(&time_ty, &dty)
                {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        duration.span,
                        format!("`WAIT` duration must be TIME, found `{dty}`"),
                    );
                }
            }
            StmtKind::Foreign { .. } => {
                // Validate F1 pack arguments if this Foreign stmt came from a pack symbol call
                if let Some((_pack_id, sym, args)) = self.parsed.pack_stmt_calls.get(&stmt.span) {
                    let sym_clone = sym.clone();
                    let args_clone = args.clone();
                    self.check_pack_call_args(&sym_clone, &args_clone, stmt.span, locals, sctx);
                }
            }
        }
    }

    fn lookup_symbol<'b>(
        &'b self,
        name: &str,
        locals: &'b BTreeMap<String, SymbolEntry>,
    ) -> Option<&'b SymbolEntry> {
        let key = name.to_ascii_uppercase();
        locals.get(&key).or_else(|| self.globals.get(&key))
    }

    fn check_lvalue_writable(
        &mut self,
        lvalue: &LValue,
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) -> Option<TypeRef> {
        // Direct write to read-only SFC step attributes (`<Step>.X`, `<Step>.T`, `<Macro>.DONE`)
        if let LValue::Field { base, field, .. } = lvalue
            && let LValue::Var { name, .. } = base.as_ref()
            && self.lookup_symbol(name, locals).is_none()
            && is_sfc_step_attr(field)
        {
            self.push_diag(
                ErrorCode::ErrConstWrite,
                span,
                format!("SFC step attribute `{name}.{field}` is read-only"),
            );
            return None;
        }

        let root = lvalue.root_name();
        match self.lookup_symbol(root, locals).cloned() {
            Some(SymbolEntry::Data {
                scope: StorageScope::Const,
                ..
            }) => {
                self.push_diag(
                    ErrorCode::ErrConstWrite,
                    span,
                    format!("cannot assign to `VAR CONSTANT` symbol `{root}`"),
                );
                return None;
            }
            Some(SymbolEntry::EnumConst { .. }) => {
                self.push_diag(
                    ErrorCode::ErrConstWrite,
                    span,
                    format!("cannot assign to enum constant `{root}`"),
                );
                return None;
            }
            Some(SymbolEntry::Param {
                writable: false, ..
            }) => {
                self.push_diag(
                    ErrorCode::ErrConstWrite,
                    span,
                    format!("cannot assign to read-only `VAR_INPUT` parameter `{root}`"),
                );
                return None;
            }
            Some(SymbolEntry::Hardware { binding, .. }) => {
                let writable_hw = matches!(
                    binding.kind,
                    HardwareKind::DigitalOut
                        | HardwareKind::DigitalBidirectional
                        | HardwareKind::AnalogOut
                        | HardwareKind::PwmOut
                );
                if !writable_hw {
                    self.push_diag(
                        ErrorCode::ErrHardwareLabelWrite,
                        span,
                        format!(
                            "cannot assign to input hardware tag `{root}` (port `{}`)",
                            binding.pin
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
                | SymbolEntry::Param { writable: true, .. },
            )
            | None => {}
        }
        self.infer_lvalue(lvalue, span, locals, sctx)
    }

    fn infer_lvalue(
        &mut self,
        lvalue: &LValue,
        fallback_span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) -> Option<TypeRef> {
        match lvalue {
            LValue::Var { name, span } => {
                let sp = if span.is_empty() {
                    fallback_span
                } else {
                    *span
                };
                self.resolve_ident_type(name, sp, locals)
            }
            LValue::Index { base, index, span } => {
                let sp = if span.is_empty() {
                    fallback_span
                } else {
                    *span
                };
                let base_ty = self.infer_lvalue(base, sp, locals, sctx)?;
                self.check_index_op(&base_ty, index, sp, locals, sctx)
            }
            LValue::Field { base, field, span } => {
                let sp = if span.is_empty() {
                    fallback_span
                } else {
                    *span
                };
                let base_ty = self.infer_lvalue(base, sp, locals, sctx)?;
                self.check_field_access(&base_ty, field, sp)
            }
        }
    }

    fn resolve_ident_type(
        &mut self,
        name: &str,
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
    ) -> Option<TypeRef> {
        match self.lookup_symbol(name, locals) {
            Some(
                SymbolEntry::Data { ty, .. }
                | SymbolEntry::Param { ty, .. }
                | SymbolEntry::Hardware { ty, .. },
            ) => Some(ty.clone()),
            Some(SymbolEntry::EnumConst { enum_name }) => Some(TypeRef::Named {
                name: enum_name.clone(),
            }),
            None => {
                self.push_diag(
                    ErrorCode::ErrUndeclaredSymbol,
                    span,
                    format!("undeclared variable or hardware tag `{name}`"),
                );
                None
            }
        }
    }

    fn infer_expr(
        &mut self,
        expr: &Expr,
        expected: Option<&TypeRef>,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) -> Option<TypeRef> {
        // Explicit typed literal (`INT#-10`, `UINT#5`, `REAL#3.14`) keeps its exact declared type!
        if let Some(explicit_ty) = self.parsed.typed_literals.get(&expr.span) {
            return Some(TypeRef::scalar(explicit_ty.clone()));
        }

        match &expr.kind {
            ExprKind::Literal(lit) => match lit {
                Literal::Bool(_) => Some(TypeRef::scalar(ScalarType::Bool)),
                Literal::Int(val) => {
                    if let Some(exp) = expected
                        && is_int_like_target(exp)
                    {
                        if let TypeRef::Scalar { ty: s } = exp
                            && !fits_scalar_int_range(s, *val)
                        {
                            self.push_diag(
                                ErrorCode::ErrTypeMismatch,
                                expr.span,
                                format!("integer literal `{val}` is out of range for `{exp}`"),
                            );
                        }
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
            ExprKind::Ident(name) => self.resolve_ident_type(name, expr.span, locals),
            ExprKind::Index { base, index } => {
                let base_ty = self.infer_expr(base, None, locals, sctx)?;
                self.check_index_op(&base_ty, index, expr.span, locals, sctx)
            }
            ExprKind::Field { base, field } => {
                // Qualified enum access (`EnumType.Variant`)
                if let ExprKind::Ident(base_name) = &base.kind {
                    let bkey = base_name.to_ascii_uppercase();
                    if let Some(variants) = self.enum_variants.get(&bkey) {
                        if variants.contains(&field.to_ascii_uppercase()) {
                            return Some(TypeRef::Named {
                                name: base_name.clone(),
                            });
                        }
                        self.push_diag(
                            ErrorCode::ErrUnknownFieldAccess,
                            expr.span,
                            format!("enum `{base_name}` has no variant `{field}`"),
                        );
                        return None;
                    }
                    // First-class SFC step attributes (`<Step>.X`, `<Step>.T`, `<Macro>.DONE`)
                    if self.lookup_symbol(base_name, locals).is_none()
                        && let Some(step_attr_ty) = sfc_step_attr_type(field)
                    {
                        return Some(step_attr_ty);
                    }
                }
                let base_ty = self.infer_expr(base, None, locals, sctx)?;
                self.check_field_access(&base_ty, field, expr.span)
            }
            ExprKind::Call { callee, args } => {
                self.check_call(callee, args, expr.span, locals, sctx)
            }
            ExprKind::Unary { op, operand } => {
                let op_ty = self.infer_expr(operand, expected, locals, sctx)?;
                match op {
                    UnaryOp::Not => {
                        let bool_ty = TypeRef::scalar(ScalarType::Bool);
                        if !types_compatible(&bool_ty, &op_ty) {
                            self.push_diag(
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
                            ErrorCode::ErrTypeMismatch,
                            expr.span,
                            format!(
                                "unary +/- requires signed numeric or ANALOG operand, found `{op_ty}`"
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
                self.check_binary_op(*op, left, right, expected, expr.span, locals, sctx)
            }
            ExprKind::Foreign { language, text } => {
                if let Some((_pack_id, sym)) = self.parsed.pack_expr_types.get(&expr.span) {
                    return sym.return_type_ref();
                }
                if let Some(exp) = expected {
                    Some(exp.clone())
                } else {
                    let site = ForeignSite {
                        level: ForeignLevel::Expr,
                        node_id: Some(sctx.pou_id),
                        language,
                        fidelity: ForeignFidelity::F0,
                        text,
                        pack_hint: None,
                        span: expr.span,
                    };
                    self.diagnostics
                        .push(foreign_untyped_diagnostic(&site, Some(&self.ctx.board_id)));
                    None
                }
            }
        }
    }

    fn check_index_op(
        &mut self,
        base_ty: &TypeRef,
        index: &Expr,
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) -> Option<TypeRef> {
        let TypeRef::Array { elem, len } = base_ty else {
            self.push_diag(
                ErrorCode::ErrTypeMismatch,
                span,
                format!("cannot index into non-array type `{base_ty}`"),
            );
            return None;
        };

        let idx_ty = self.infer_expr(index, None, locals, sctx);
        if let Some(ty) = &idx_ty {
            let is_int =
                matches!(ty, TypeRef::Scalar { ty: s } if s.is_signed_int() || s.is_unsigned_int());
            if !is_int {
                self.push_diag(
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
                ErrorCode::ErrArrayBoundsConst,
                index.span,
                format!(
                    "constant array index `{const_idx}` out of bounds for `ARRAY[0..{}]`",
                    len.saturating_sub(1)
                ),
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
        let fkey = field.to_ascii_uppercase();
        match base_ty {
            TypeRef::Named { name } => {
                let key = name.to_ascii_uppercase();
                if let Some(fields) = self.structs.get(&key)
                    && let Some(ft) = fields.get(&fkey)
                {
                    return Some(ft.clone());
                }
                self.push_diag(
                    ErrorCode::ErrUnknownFieldAccess,
                    span,
                    format!("struct `{name}` has no field `.{field}`"),
                );
                None
            }
            TypeRef::Fb { .. } => {
                let fields = self.fb_fields_for_type(base_ty);
                if let Some(ft) = fields.get(&fkey) {
                    Some(ft.clone())
                } else {
                    self.push_diag(
                        ErrorCode::ErrUnknownFieldAccess,
                        span,
                        format!("Function Block `{base_ty}` has no field `.{field}`"),
                    );
                    None
                }
            }
            TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Array { .. } => {
                self.push_diag(
                    ErrorCode::ErrUnknownFieldAccess,
                    span,
                    format!("type `{base_ty}` does not support field access `.{field}`"),
                );
                None
            }
        }
    }

    fn is_untyped_numeric_literal(&self, expr: &Expr) -> bool {
        matches!(
            expr.kind,
            ExprKind::Literal(Literal::Int(_) | Literal::Float(_))
        ) && !self.parsed.typed_literals.contains_key(&expr.span)
    }

    fn infer_symmetric_pair(
        &mut self,
        left: &Expr,
        right: &Expr,
        expected: Option<&TypeRef>,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) -> Option<(TypeRef, TypeRef)> {
        if expected.is_none()
            && self.is_untyped_numeric_literal(left)
            && !self.is_untyped_numeric_literal(right)
        {
            let rt = self.infer_expr(right, None, locals, sctx)?;
            let lt = self.infer_expr(left, Some(&rt), locals, sctx)?;
            Some((lt, rt))
        } else {
            let lt = self.infer_expr(left, expected, locals, sctx)?;
            let rt = self.infer_expr(right, Some(&lt), locals, sctx)?;
            Some((lt, rt))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_binary_op(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        expected: Option<&TypeRef>,
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) -> Option<TypeRef> {
        let bool_ty = TypeRef::scalar(ScalarType::Bool);
        match op {
            BinaryOp::And | BinaryOp::Xor | BinaryOp::Or => {
                let lt = self.infer_expr(left, Some(&bool_ty), locals, sctx);
                let rt = self.infer_expr(right, Some(&bool_ty), locals, sctx);
                if let Some(l) = lt
                    && !types_compatible(&bool_ty, &l)
                {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        left.span,
                        format!("logical operator requires BOOL operand, found `{l}`"),
                    );
                }
                if let Some(r) = rt
                    && !types_compatible(&bool_ty, &r)
                {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        right.span,
                        format!("logical operator requires BOOL operand, found `{r}`"),
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
                let (lt, rt) = self.infer_symmetric_pair(left, right, None, locals, sctx)?;
                if !types_compatible(&lt, &rt) {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        span,
                        format!("cannot compare incompatible types `{lt}` and `{rt}`"),
                    );
                }
                Some(bool_ty)
            }
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                let (lt, rt) = self.infer_symmetric_pair(left, right, expected, locals, sctx)?;
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
                        ErrorCode::ErrTypeMismatch,
                        span,
                        format!("invalid arithmetic operands `{lt}` and `{rt}`"),
                    );
                    return None;
                }
                Some(lt)
            }
            BinaryOp::Shl | BinaryOp::Shr => {
                let lt = self.infer_expr(left, expected, locals, sctx)?;
                let rt = self.infer_expr(right, None, locals, sctx)?;
                let left_ok = matches!(&lt, TypeRef::Scalar { ty } if ty.is_bitstring() || ty.is_unsigned_int() || ty.is_signed_int());
                let right_ok = matches!(&rt, TypeRef::Scalar { ty } if ty.is_unsigned_int() || ty.is_signed_int());
                if !left_ok || !right_ok {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        span,
                        format!("invalid shift operands `{lt}` and `{rt}`"),
                    );
                    return None;
                }
                Some(lt)
            }
            BinaryOp::BitAnd | BinaryOp::BitXor | BinaryOp::BitOr => {
                let (lt, rt) = self.infer_symmetric_pair(left, right, expected, locals, sctx)?;
                let bit_ok = matches!(&lt, TypeRef::Scalar { ty } if ty.is_bitstring() || ty.is_unsigned_int() || ty.is_signed_int());
                if !bit_ok || !types_compatible(&lt, &rt) {
                    self.push_diag(
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
        &mut self,
        callee: &str,
        args: &[CallArg],
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) -> Option<TypeRef> {
        let upper = callee.to_ascii_uppercase();

        // 1. Canonical ASL analog read intrinsic: `ADC_CODE(tag)`
        if upper == "ADC_CODE" {
            if args.len() != 1 {
                self.push_diag(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!("`ADC_CODE` expects 1 argument, got {}", args.len()),
                );
                return Some(TypeRef::scalar(ScalarType::Code {
                    codec_id: "*".to_owned(),
                }));
            }
            if let Some(arg) = args.first()
                && let ExprKind::Ident(tag) = &arg.value.kind
            {
                let tkey = tag.to_ascii_uppercase();
                if let Some(cid) = self.analog_in_codecs.get(&tkey) {
                    return Some(TypeRef::scalar(ScalarType::Code {
                        codec_id: cid.clone(),
                    }));
                }
                // Allow direct board port name (e.g. `ADC_CODE(IW64)`) if declared on BoardContext
                if self.ctx.iec_for_port(tag).is_some() {
                    let cid = if tag == "IW66" {
                        "plc_bipolar_27648".to_owned()
                    } else {
                        self.ctx
                            .default_adc_codec
                            .as_ref()
                            .map_or_else(|| "*".to_owned(), |c| c.id.clone())
                    };
                    return Some(TypeRef::scalar(ScalarType::Code { codec_id: cid }));
                }
                self.push_diag(
                    ErrorCode::ErrUndeclaredSymbol,
                    arg.span,
                    format!("unknown analog input tag `{tag}` in `ADC_CODE`"),
                );
            }
            return Some(TypeRef::scalar(ScalarType::Code {
                codec_id: "*".to_owned(),
            }));
        }

        // 2. User-defined FUNCTION POUs
        if let Some((params, ret_ty)) = self.user_fcs.get(&upper).cloned() {
            if args.len() != params.len() {
                self.push_diag(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!(
                        "function `{callee}` expects {} argument(s), found {}",
                        params.len(),
                        args.len()
                    ),
                );
                return Some(ret_ty);
            }
            for (idx, arg) in args.iter().enumerate() {
                let param = if let Some(aname) = &arg.name {
                    let found = params.iter().find(|p| p.name.eq_ignore_ascii_case(aname));
                    let Some(p) = found else {
                        self.push_diag(
                            ErrorCode::ErrArityMismatch,
                            arg.span,
                            format!("unknown named argument `{aname}` in call to `{callee}`"),
                        );
                        continue;
                    };
                    p
                } else if let Some(p) = params.get(idx) {
                    p
                } else {
                    continue;
                };
                self.check_duty_overflow(&param.ty, &arg.value, arg.span);
                if let Some(actual) = self.infer_expr(&arg.value, Some(&param.ty), locals, sctx)
                    && !types_compatible(&param.ty, &actual)
                {
                    self.push_diag(
                        ErrorCode::ErrTypeMismatch,
                        arg.span,
                        format!(
                            "argument `{}` to `{callee}` has type `{actual}`, expected `{}`",
                            param.name, param.ty
                        ),
                    );
                }
            }
            return Some(ret_ty);
        }

        // 3. Domain-split stdlib resolution (`std.iec.*` default + explicit `USING` imports)
        let mut effective_imports: Vec<&str> =
            Vec::with_capacity(sctx.imports.len().saturating_add(1));
        effective_imports.push("std.iec.*");
        for imp in sctx.imports {
            effective_imports.push(imp.as_str());
        }

        if let Some(StdSymbol::Function(sig)) =
            self.std_catalog.resolve_symbol(&effective_imports, &upper)
        {
            if args.len() != sig.params.len() {
                self.push_diag(
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
                upper.as_str(),
                "BOOL_TO_INT"
                    | "INT_TO_DINT"
                    | "DINT_TO_REAL"
                    | "REAL_TO_DINT"
                    | "ANALOG_TO_CODE"
                    | "CODE_TO_ANALOG"
                    | "ANALOG_TO_DUTY"
            );

            let mut any_num_inferred: Option<TypeRef> = None;
            for (arg, param) in args.iter().zip(sig.params.iter()) {
                if param.ty == StdTypeRef::PinRef {
                    let _ = self.infer_expr(&arg.value, None, locals, sctx);
                    continue;
                }
                if param.ty == StdTypeRef::AnyNum {
                    if let Some(actual) =
                        self.infer_expr(&arg.value, any_num_inferred.as_ref(), locals, sctx)
                    {
                        if !matches!(actual, TypeRef::Scalar { ref ty } if ty.is_plain_numeric()) {
                            self.push_diag(
                                ErrorCode::ErrTypeMismatch,
                                arg.span,
                                format!("`{callee}` requires numeric argument, found `{actual}`"),
                            );
                        } else if let Some(prev) = &any_num_inferred {
                            if !types_compatible(prev, &actual) {
                                self.push_diag(
                                    ErrorCode::ErrTypeMismatch,
                                    arg.span,
                                    format!(
                                        "`{callee}` numeric arguments mismatch: `{prev}` vs `{actual}`"
                                    ),
                                );
                            }
                        } else {
                            any_num_inferred = Some(actual);
                        }
                    }
                    continue;
                }
                let expected_ty = std_type_to_typeref(param.ty);
                self.check_duty_overflow(&expected_ty, &arg.value, arg.span);
                if let Some(actual) = self.infer_expr(&arg.value, Some(&expected_ty), locals, sctx)
                    && !types_compatible(&expected_ty, &actual)
                {
                    let code = if is_conversion {
                        ErrorCode::ErrInvalidCast
                    } else {
                        ErrorCode::ErrTypeMismatch
                    };
                    self.push_diag(
                        code,
                        arg.span,
                        format!(
                            "argument `{}` to `{callee}` has type `{actual}`, expected `{expected_ty}`",
                            param.name
                        ),
                    );
                }
            }

            if matches!(upper.as_str(), "ANALOG_TO_CODE" | "CODE_TO_ANALOG")
                && let Some(codec_arg) = args.get(1)
                && let ExprKind::Literal(Literal::String(codec_id)) = &codec_arg.value.kind
                && !self.ctx.codecs.is_empty()
                && !self.ctx.codecs.contains_key(codec_id)
            {
                self.push_diag(
                    ErrorCode::ErrInvalidCast,
                    codec_arg.span,
                    format!(
                        "unknown codec `{codec_id}` in `{callee}` for board `{}`",
                        self.ctx.profile_id
                    ),
                );
            }

            if sig.return_type == StdTypeRef::AnyNum
                && let Some(inferred) = any_num_inferred
            {
                return Some(inferred);
            }
            return Some(std_type_to_typeref(sig.return_type));
        }

        // 4. IEC 61131-3 explicit scalar conversions (`<SRC>_TO_<DST>` and Ed. 3 `TO_<DST>`)
        if let Some(cast_result) = self.try_check_iec_cast(&upper, args, span, locals, sctx) {
            return cast_result;
        }

        // 5. Domain-split isolation check: detect unimported `std.arduino.*` or `std.plc.*` symbols!
        if let Some(owner_module) = self.find_unimported_platform_symbol(&upper) {
            self.push_diag(
                ErrorCode::ErrUnresolvedImport,
                span,
                format!(
                    "symbol `{callee}` belongs to platform module `{owner_module}` which is not imported via `USING`"
                ),
            );
            return None;
        }

        self.push_diag(
            ErrorCode::ErrUnresolvedFunction,
            span,
            format!("unresolved function `{callee}`"),
        );
        None
    }

    fn try_check_iec_cast(
        &mut self,
        upper_name: &str,
        args: &[CallArg],
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) -> Option<Option<TypeRef>> {
        // Ed. 3 `TO_<DST>(expr)` (used by `ReducedRationalPlan::emit_st`: `TO_UDINT`, `TO_DINT`, `TO_ULINT`, `TO_LINT`)
        if let Some(dst_str) = upper_name.strip_prefix("TO_")
            && let Some(dst_scalar) = parse_cast_scalar(dst_str)
        {
            if args.len() != 1 {
                self.push_diag(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!("`{upper_name}` expects 1 argument, got {}", args.len()),
                );
                return Some(Some(TypeRef::scalar(dst_scalar)));
            }
            if let Some(arg) = args.first()
                && let Some(src_ty) = self.infer_expr(&arg.value, None, locals, sctx)
            {
                let valid = match &src_ty {
                    TypeRef::Scalar { ty: src_s } => is_valid_scalar_cast(src_s, &dst_scalar),
                    TypeRef::Void
                    | TypeRef::Array { .. }
                    | TypeRef::Named { .. }
                    | TypeRef::Fb { .. } => false,
                };
                if !valid {
                    self.push_diag(
                        ErrorCode::ErrInvalidCast,
                        span,
                        format!("invalid conversion `{upper_name}` from `{src_ty}` to `{dst_str}`"),
                    );
                }
            }
            return Some(Some(TypeRef::scalar(dst_scalar)));
        }

        // Classic IEC `<SRC>_TO_<DST>(expr)`
        if let Some((src_str, dst_str)) = upper_name.split_once("_TO_")
            && let (Some(src_scalar), Some(dst_scalar)) =
                (parse_cast_scalar(src_str), parse_cast_scalar(dst_str))
        {
            if !is_valid_scalar_cast(&src_scalar, &dst_scalar) {
                self.push_diag(
                    ErrorCode::ErrInvalidCast,
                    span,
                    format!("forbidden IEC conversion `{upper_name}` (`{src_str}` -> `{dst_str}`)"),
                );
                return Some(None);
            }
            if args.len() != 1 {
                self.push_diag(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!("`{upper_name}` expects 1 argument, got {}", args.len()),
                );
                return Some(Some(TypeRef::scalar(dst_scalar)));
            }
            if let Some(arg) = args.first() {
                let expected_src = TypeRef::scalar(src_scalar.clone());
                if let Some(actual) = self.infer_expr(&arg.value, Some(&expected_src), locals, sctx)
                {
                    let code_as_int = matches!(
                        (&src_scalar, &actual),
                        (
                            ScalarType::Int | ScalarType::Dint | ScalarType::Udint,
                            TypeRef::Scalar {
                                ty: ScalarType::Code { .. }
                            }
                        )
                    );
                    if !types_compatible(&expected_src, &actual) && !code_as_int {
                        self.push_diag(
                            ErrorCode::ErrInvalidCast,
                            arg.span,
                            format!(
                                "`{upper_name}` expects `{expected_src}` argument, found `{actual}`"
                            ),
                        );
                    }
                }
            }
            return Some(Some(TypeRef::scalar(dst_scalar)));
        }

        None
    }

    fn find_unimported_platform_symbol(&self, upper_name: &str) -> Option<&'static str> {
        for module in self.std_catalog.modules() {
            if matches!(module.domain, StdDomain::Arduino | StdDomain::Plc)
                && module.get_symbol(upper_name).is_some()
            {
                return Some(module.path);
            }
        }
        None
    }

    fn check_pack_call_args(
        &mut self,
        sym: &PackSymbol,
        args: &[CallArg],
        span: Span,
        locals: &BTreeMap<String, SymbolEntry>,
        sctx: &StmtCtx<'_>,
    ) {
        if args.len() != sym.params.len() {
            self.push_diag(
                ErrorCode::ErrArityMismatch,
                span,
                format!(
                    "F1 pack symbol `{}` expects {} argument(s), found {}",
                    sym.name,
                    sym.params.len(),
                    args.len()
                ),
            );
            return;
        }
        for (arg, param) in args.iter().zip(sym.params.iter()) {
            if let Some(expected_ty) = param.type_ref()
                && let Some(actual) = self.infer_expr(&arg.value, Some(&expected_ty), locals, sctx)
                && !types_compatible(&expected_ty, &actual)
            {
                self.push_diag(
                    ErrorCode::ErrTypeMismatch,
                    arg.span,
                    format!(
                        "argument `{}` to F1 pack symbol `{}` has type `{actual}`, expected `{expected_ty}`",
                        param.name, sym.name
                    ),
                );
            }
        }
    }

    fn check_duty_overflow(&mut self, target_ty: &TypeRef, expr: &Expr, span: Span) {
        if matches!(
            target_ty,
            TypeRef::Scalar {
                ty: ScalarType::Duty
            }
        ) && let Some(val) = eval_const_int(expr)
            && (val < 0 || val > i128::from(ASL_ANALOG_FULL_SCALE))
        {
            self.push_diag(
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
            let key = fb_name.to_ascii_uppercase();
            if let Some(fields) = self.user_fbs.get(&key) {
                return fields.clone();
            }
            return self.pack_fbs.get(&key).cloned().unwrap_or_default();
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
                fields.insert(f.name.to_ascii_uppercase(), std_type_to_typeref(f.ty));
            }
        }
        // Merge F1 pack-declared fields (e.g. `plc-modbus-tcp.pack.toml` extends `MB_CLIENT`/`MB_SERVER`)
        if let Some(pack_fields) = self.pack_fbs.get(std_name) {
            for (k, v) in pack_fields {
                fields.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        fields
    }
}

fn is_known_std_import(imp: &str, catalog: &StdCatalog) -> bool {
    if catalog.get_module(imp).is_some() {
        return true;
    }
    if let Some(prefix) = imp.strip_suffix(".*") {
        return catalog.modules().any(|m| {
            m.path.starts_with(prefix) && m.path.as_bytes().get(prefix.len()) == Some(&b'.')
        });
    }
    false
}

fn hardware_st_type(hw: &HardwareBinding) -> TypeRef {
    match hw.kind {
        HardwareKind::DigitalIn | HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional => {
            TypeRef::scalar(ScalarType::Bool)
        }
        HardwareKind::AnalogIn => {
            if let Some(cid) = &hw.codec {
                TypeRef::scalar(ScalarType::Code {
                    codec_id: cid.clone(),
                })
            } else {
                TypeRef::scalar(ScalarType::Analog)
            }
        }
        HardwareKind::AnalogOut => TypeRef::scalar(ScalarType::Analog),
        HardwareKind::PwmOut => TypeRef::scalar(ScalarType::Duty),
        HardwareKind::BusPin => TypeRef::scalar(ScalarType::Byte),
    }
}

fn is_sfc_step_attr(field: &str) -> bool {
    field.eq_ignore_ascii_case("X")
        || field.eq_ignore_ascii_case("T")
        || field.eq_ignore_ascii_case("DONE")
}

fn sfc_step_attr_type(field: &str) -> Option<TypeRef> {
    if field.eq_ignore_ascii_case("X") || field.eq_ignore_ascii_case("DONE") {
        Some(TypeRef::scalar(ScalarType::Bool))
    } else if field.eq_ignore_ascii_case("T") {
        Some(TypeRef::scalar(ScalarType::Time))
    } else {
        None
    }
}

fn parse_cast_scalar(upper: &str) -> Option<ScalarType> {
    match upper {
        "BOOL" => Some(ScalarType::Bool),
        "SINT" => Some(ScalarType::Sint),
        "INT" => Some(ScalarType::Int),
        "DINT" => Some(ScalarType::Dint),
        "LINT" => Some(ScalarType::Lint),
        "USINT" => Some(ScalarType::Usint),
        "UINT" => Some(ScalarType::Uint),
        "UDINT" => Some(ScalarType::Udint),
        "ULINT" => Some(ScalarType::Ulint),
        "BYTE" => Some(ScalarType::Byte),
        "WORD" => Some(ScalarType::Word),
        "DWORD" => Some(ScalarType::Dword),
        "LWORD" => Some(ScalarType::Lword),
        "REAL" => Some(ScalarType::Real),
        "LREAL" => Some(ScalarType::Lreal),
        "TIME" => Some(ScalarType::Time),
        "STRING" => Some(ScalarType::String { max_len: 255 }),
        _ => None,
    }
}

fn is_valid_scalar_cast(src: &ScalarType, dst: &ScalarType) -> bool {
    if src == dst {
        return true;
    }
    match (src, dst) {
        // Explicit `BOOL_TO_REAL` / `REAL_TO_BOOL` / `BOOL_TO_LREAL` are forbidden (`docs/lbc/st.md` §1.4)
        (ScalarType::Bool, dst) => {
            dst.is_signed_int() || dst.is_unsigned_int() || dst.is_bitstring()
        }
        (src, ScalarType::Bool) => {
            src.is_signed_int() || src.is_unsigned_int() || src.is_bitstring()
        }
        // `CODE<...>` converts explicitly to integer/bitstring types (`ReducedRationalPlan::emit_st`)
        (ScalarType::Code { .. }, dst) => {
            dst.is_signed_int() || dst.is_unsigned_int() || dst.is_bitstring()
        }
        (src, dst)
            if (src.is_plain_numeric() || src.is_bitstring())
                && (dst.is_plain_numeric() || dst.is_bitstring()) =>
        {
            true
        }
        _ => false,
    }
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

fn types_compatible(expected: &TypeRef, actual: &TypeRef) -> bool {
    if expected == actual {
        return true;
    }
    match (expected, actual) {
        (
            TypeRef::Scalar {
                ty: ScalarType::String { .. },
            },
            TypeRef::Scalar {
                ty: ScalarType::String { .. },
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Code { .. },
            },
            TypeRef::Scalar {
                ty: ScalarType::Code { .. },
            },
        ) => true,
        (TypeRef::Named { name: a }, TypeRef::Named { name: b }) => a.eq_ignore_ascii_case(b),
        (
            TypeRef::Fb {
                fb_kind: FbKind::User { fb_name: a },
            },
            TypeRef::Fb {
                fb_kind: FbKind::User { fb_name: b },
            },
        ) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
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
