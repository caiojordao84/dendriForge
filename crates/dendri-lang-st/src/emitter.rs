//! Canonical IEC 61131-3 Structured Text emitter (`docs/lbc/st.md` §1–§7).
//!
//! Guarantees:
//! - Canonical 4-space indentation, uppercase IEC keywords, explicit semicolons.
//! - Zero invented `SETUP`/`LOOP` keywords; zero banned machine magic comments (`ADR-020`).
//! - Never emits `M = 10^9`; cross-platform scaling uses [`ReducedRationalPlan::emit_st`].
//! - Full `ADR-027` verbatim comment conservation (`C1–C5`) across all four `DEF-006` dialects,
//!   including EOL restoration, `inner` comment relocation/splicing, and anti-merge blank-line guards.
//! - Enforces cross-language `Foreign` export gating via [`check_program_foreign_export`].

use crate::lexer::{fits_scalar_int_range, lex_st};
use dendri_ast::{
    BinaryOp, BoardProgram, CallArg, CasePattern, Codec, CodecError, CodecKind, Comment,
    CommentAnchor, CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, Expr, ExprKind,
    FbKind, ForeignFidelity, HardwareBinding, HardwareKind, LValue, LadderBoxKind, LadderCell,
    LadderCellKind, LadderCoilKind, LadderCompareOp, LadderContactKind, LadderNetwork, Literal,
    ModuleDecl, NodeId, ParamDecl, ParamDirection, Pou, PouBody, PouKind, ScalarType, SfcChart,
    Stmt, StmtKind, StorageScope, TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, CommentPlacement, ForeignExportDecision, ForeignLevel,
    ForeignSite, FrontendOutput, FrontendResult, LanguageId, PackSymbolKind, ReducedRationalPlan,
    check_foreign_export, check_program_foreign_export, relocate_inner_comments,
    render_comment_for_language, restore_eol,
};
use dendri_span::{Diagnostic, ErrorCode, Span};
use dendri_stdlib::{StdCatalog, StdSymbol, StdTypeRef};
use std::collections::{BTreeMap, BTreeSet};

/// Emits a cross-platform reduced-rational IEC ST scaling expression using [`ReducedRationalPlan::emit_st`].
///
/// Never emits `M = 10^9`.
pub fn emit_scaled_code_expr(
    expr: &str,
    src_codec: &Codec,
    dst_codec: &Codec,
) -> Result<String, CodecError> {
    let plan = ReducedRationalPlan::compute(src_codec, dst_codec)?;
    Ok(plan.emit_st(expr))
}

/// Emits canonical IEC 61131-3 ST source text from `bundle` under `ctx`.
pub(crate) fn emit_st(
    bundle: &BoardProgramBundle,
    ctx: &BoardContext,
    force_reduced_rationals: bool,
) -> FrontendResult<String> {
    // 1. Cross-language Foreign export guard (same-language ST is NEVER blocked)
    check_program_foreign_export(&bundle.program, LanguageId::St, ctx)?;

    // 2. Validate comment field invariants (`C5`) before projection
    let mut field_errors = Vec::new();
    for c in &bundle.comments {
        if let Err(code) = CommentPlacement::from_comment(c) {
            let mut d = Diagnostic::new(
                code,
                format!(
                    "comment `{}` violates placement/token_ordinal invariant",
                    c.id
                ),
            )
            .with_board_id(&ctx.board_id);
            d.span = Some(c.span);
            field_errors.push(d);
        }
    }
    if !field_errors.is_empty() {
        return Err(field_errors);
    }

    // 3. Relocate cross-language `Inner` comments (`INFO_COMMENT_RELOCATED`)
    let (comments, mut diagnostics) =
        relocate_inner_comments(&bundle.comments, LanguageId::St, Some(&ctx.board_id));

    let comment_index = CommentIndex::build(&comments);

    // Direct-addressed AnalogIn hardware labels lift `PressRaw` <-> `ADC_CODE(PressRaw)`
    let analog_in_direct_tags: BTreeSet<String> = bundle
        .program
        .hardware
        .iter()
        .filter(|h| h.kind == HardwareKind::AnalogIn && ctx.iec_for_port(&h.pin).is_some())
        .map(|h| h.label.to_ascii_uppercase())
        .collect();

    let type_env = EmitTypeEnv::build(&bundle.program, ctx);

    let mut emitter = Emitter {
        ctx,
        comments: &comments,
        comment_index,
        emitted_comments: BTreeSet::new(),
        blank_lines_before: &bundle.presentation.blank_lines_before,
        analog_in_direct_tags,
        type_env,
        local_types: BTreeMap::new(),
        current_return_type: None,
        force_reduced_rationals,
        out: String::with_capacity(512),
        diagnostics: Vec::new(),
        last_line_was_line_comment: false,
    };

    emitter.emit_program(&bundle.program);
    diagnostics.append(&mut emitter.diagnostics);

    let final_text = restore_eol(&emitter.out, bundle.presentation.eol_style);
    Ok(FrontendOutput::with_diagnostics(final_text, diagnostics))
}

struct CommentIndex {
    header: Vec<usize>,
    body: Vec<usize>,
    leading: BTreeMap<NodeId, Vec<usize>>,
    trailing: BTreeMap<NodeId, Vec<usize>>,
    inner: BTreeMap<NodeId, Vec<(u32, usize)>>,
    dangling: BTreeMap<NodeId, Vec<usize>>,
}

impl CommentIndex {
    fn build(comments: &[Comment]) -> Self {
        let mut header = Vec::new();
        let mut body = Vec::new();
        let mut leading: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        let mut trailing: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        let mut inner: BTreeMap<NodeId, Vec<(u32, usize)>> = BTreeMap::new();
        let mut dangling: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();

        for (idx, c) in comments.iter().enumerate() {
            match &c.anchor {
                CommentAnchor::Node { id } => match c.placement {
                    AstCommentPlacement::Leading
                    | AstCommentPlacement::Header
                    | AstCommentPlacement::Title
                    | AstCommentPlacement::Body => {
                        leading.entry(id.clone()).or_default().push(idx);
                    }
                    AstCommentPlacement::Trailing => {
                        trailing.entry(id.clone()).or_default().push(idx);
                    }
                    AstCommentPlacement::Inner => {
                        let ord = c.token_ordinal.unwrap_or(0);
                        inner.entry(id.clone()).or_default().push((ord, idx));
                    }
                    AstCommentPlacement::Dangling => {
                        dangling.entry(id.clone()).or_default().push(idx);
                    }
                },
                CommentAnchor::Board { .. }
                | CommentAnchor::Project
                | CommentAnchor::Component { .. }
                | CommentAnchor::Wire { .. } => match c.placement {
                    AstCommentPlacement::Header
                    | AstCommentPlacement::Leading
                    | AstCommentPlacement::Title => header.push(idx),
                    AstCommentPlacement::Body
                    | AstCommentPlacement::Trailing
                    | AstCommentPlacement::Dangling
                    | AstCommentPlacement::Inner => body.push(idx),
                },
            }
        }

        for list in inner.values_mut() {
            list.sort_by_key(|&(ord, idx)| (ord, idx));
        }

        Self {
            header,
            body,
            leading,
            trailing,
            inner,
            dangling,
        }
    }
}

type PackFnSig = (Vec<(String, Option<TypeRef>)>, Option<TypeRef>);

struct EmitTypeEnv {
    globals: BTreeMap<String, TypeRef>,
    structs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    user_fcs: BTreeMap<String, (Vec<ParamDecl>, TypeRef)>,
    user_fbs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    pack_fbs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    pack_fns: BTreeMap<String, PackFnSig>,
    std_catalog: StdCatalog,
    imports: Vec<String>,
    used_global_names: BTreeSet<String>,
}

impl EmitTypeEnv {
    fn build(program: &BoardProgram, ctx: &BoardContext) -> Self {
        let mut globals = BTreeMap::new();
        let mut structs = BTreeMap::new();
        let mut user_fcs = BTreeMap::new();
        let mut user_fbs = BTreeMap::new();
        let mut pack_fbs = BTreeMap::new();
        let mut pack_fns = BTreeMap::new();
        let mut used_global_names = BTreeSet::new();

        for pack in &ctx.loaded_packs {
            if pack.pack.language == LanguageId::St {
                for sym in &pack.symbols {
                    let key = sym.name.to_ascii_uppercase();
                    match sym.kind {
                        PackSymbolKind::Fb => {
                            let mut fields = BTreeMap::new();
                            for p in &sym.params {
                                if let Some(ty) = p.type_ref() {
                                    fields.insert(p.name.to_ascii_uppercase(), ty);
                                }
                            }
                            pack_fbs.insert(key, fields);
                        }
                        PackSymbolKind::Function | PackSymbolKind::Method => {
                            let params = sym
                                .params
                                .iter()
                                .map(|p| (p.name.clone(), p.type_ref()))
                                .collect();
                            pack_fns.insert(key, (params, sym.return_type_ref()));
                        }
                    }
                }
            }
        }

        for hw in &program.hardware {
            let key = hw.label.to_ascii_uppercase();
            used_global_names.insert(key.clone());
            globals.insert(key, emit_hardware_type(hw));
        }

        for td in program
            .types
            .iter()
            .chain(program.modules.iter().flat_map(|m| m.types.iter()))
        {
            match td {
                TypeDecl::Struct { name, fields, .. } => {
                    let key = name.to_ascii_uppercase();
                    used_global_names.insert(key.clone());
                    let mut fmap = BTreeMap::new();
                    for f in fields {
                        fmap.insert(f.name.to_ascii_uppercase(), f.ty.clone());
                    }
                    structs.insert(key, fmap);
                }
                TypeDecl::Enum { name, variants, .. } => {
                    let key = name.to_ascii_uppercase();
                    used_global_names.insert(key);
                    for v in variants {
                        let vkey = v.name.to_ascii_uppercase();
                        used_global_names.insert(vkey.clone());
                        globals.insert(vkey, TypeRef::Named { name: name.clone() });
                    }
                }
                TypeDecl::Foreign { name, .. } => {
                    if let Some(n) = name {
                        used_global_names.insert(n.to_ascii_uppercase());
                    }
                }
            }
        }

        for d in program
            .data
            .iter()
            .chain(program.modules.iter().flat_map(|m| m.data.iter()))
        {
            let key = d.name.to_ascii_uppercase();
            used_global_names.insert(key.clone());
            globals.insert(key, d.ty.clone());
        }

        for pou in program
            .pous
            .iter()
            .chain(program.modules.iter().flat_map(|m| m.pous.iter()))
        {
            let key = pou.name.to_ascii_uppercase();
            used_global_names.insert(key.clone());
            match &pou.kind {
                PouKind::Fc { return_type } => {
                    user_fcs.insert(key, (pou.params.clone(), return_type.clone()));
                }
                PouKind::Fb => {
                    let mut fields = BTreeMap::new();
                    for p in &pou.params {
                        fields.insert(p.name.to_ascii_uppercase(), p.ty.clone());
                    }
                    user_fbs.insert(key, fields);
                }
                PouKind::Ob { .. } => {}
            }
        }

        let mut imports = vec!["std.iec.*".to_owned()];
        imports.extend(program.imports.iter().cloned());
        for m in &program.modules {
            imports.extend(m.imports.iter().cloned());
        }

        Self {
            globals,
            structs,
            user_fcs,
            user_fbs,
            pack_fbs,
            pack_fns,
            std_catalog: ctx.std_catalog(),
            imports,
            used_global_names,
        }
    }

    fn fb_field_type(&self, fb_ty: &TypeRef, field: &str) -> Option<TypeRef> {
        let TypeRef::Fb { fb_kind } = fb_ty else {
            return None;
        };
        let fkey = field.to_ascii_uppercase();
        if let FbKind::User { fb_name } = fb_kind {
            let key = fb_name.to_ascii_uppercase();
            if let Some(fields) = self.user_fbs.get(&key) {
                return fields.get(&fkey).cloned();
            }
            if let Some(fields) = self.pack_fbs.get(&key) {
                return fields.get(&fkey).cloned();
            }
            return None;
        }
        let fb_name = fb_ty.to_string();
        if let Some(fields) = self.pack_fbs.get(&fb_name.to_ascii_uppercase())
            && let Some(t) = fields.get(&fkey)
        {
            return Some(t.clone());
        }
        for module in self.std_catalog.modules() {
            for fb in module.function_blocks {
                if fb.name.eq_ignore_ascii_case(&fb_name) {
                    for f in fb.fields {
                        if f.name.eq_ignore_ascii_case(field) {
                            return std_type_to_opt_typeref(f.ty);
                        }
                    }
                }
            }
        }
        None
    }
}

fn emit_hardware_type(hw: &HardwareBinding) -> TypeRef {
    match hw.kind {
        HardwareKind::DigitalIn | HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional => {
            TypeRef::scalar(ScalarType::Bool)
        }
        HardwareKind::AnalogIn | HardwareKind::AnalogOut => TypeRef::scalar(ScalarType::Int),
        HardwareKind::PwmOut => TypeRef::scalar(ScalarType::Duty),
        HardwareKind::BusPin => TypeRef::scalar(ScalarType::Byte),
    }
}

fn std_type_to_opt_typeref(ty: StdTypeRef) -> Option<TypeRef> {
    match ty {
        StdTypeRef::Void => Some(TypeRef::Void),
        StdTypeRef::Bool => Some(TypeRef::scalar(ScalarType::Bool)),
        StdTypeRef::Sint => Some(TypeRef::scalar(ScalarType::Sint)),
        StdTypeRef::Int => Some(TypeRef::scalar(ScalarType::Int)),
        StdTypeRef::Dint => Some(TypeRef::scalar(ScalarType::Dint)),
        StdTypeRef::Lint => Some(TypeRef::scalar(ScalarType::Lint)),
        StdTypeRef::Usint => Some(TypeRef::scalar(ScalarType::Usint)),
        StdTypeRef::Uint => Some(TypeRef::scalar(ScalarType::Uint)),
        StdTypeRef::Udint => Some(TypeRef::scalar(ScalarType::Udint)),
        StdTypeRef::Ulint => Some(TypeRef::scalar(ScalarType::Ulint)),
        StdTypeRef::Byte => Some(TypeRef::scalar(ScalarType::Byte)),
        StdTypeRef::Word => Some(TypeRef::scalar(ScalarType::Word)),
        StdTypeRef::Dword => Some(TypeRef::scalar(ScalarType::Dword)),
        StdTypeRef::Lword => Some(TypeRef::scalar(ScalarType::Lword)),
        StdTypeRef::Real => Some(TypeRef::scalar(ScalarType::Real)),
        StdTypeRef::Lreal => Some(TypeRef::scalar(ScalarType::Lreal)),
        StdTypeRef::Time => Some(TypeRef::scalar(ScalarType::Time)),
        StdTypeRef::String => Some(TypeRef::scalar(ScalarType::String { max_len: 255 })),
        StdTypeRef::Analog => Some(TypeRef::scalar(ScalarType::Analog)),
        StdTypeRef::Duty => Some(TypeRef::scalar(ScalarType::Duty)),
        StdTypeRef::Code => Some(TypeRef::scalar(ScalarType::Code {
            codec_id: "*".to_owned(),
        })),
        StdTypeRef::AnyNum | StdTypeRef::PinRef => None,
    }
}

struct Emitter<'a> {
    ctx: &'a BoardContext,
    comments: &'a [Comment],
    comment_index: CommentIndex,
    emitted_comments: BTreeSet<usize>,
    blank_lines_before: &'a BTreeMap<NodeId, u8>,
    analog_in_direct_tags: BTreeSet<String>,
    type_env: EmitTypeEnv,
    local_types: BTreeMap<String, TypeRef>,
    current_return_type: Option<TypeRef>,
    force_reduced_rationals: bool,
    out: String,
    diagnostics: Vec<Diagnostic>,
    last_line_was_line_comment: bool,
}

impl<'a> Emitter<'a> {
    fn push_indent(&mut self, level: usize) {
        for _ in 0..level {
            self.out.push_str("    ");
        }
    }

    fn push_blank_line_if_needed(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            if !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            self.out.push('\n');
            self.last_line_was_line_comment = false;
        }
    }

    fn emit_presented_blank_lines(&mut self, id: &NodeId) {
        if let Some(&count) = self.blank_lines_before.get(id)
            && count > 0
            && !self.out.is_empty()
            && !self.out.ends_with("\n\n")
        {
            if !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            self.out.push('\n');
            self.last_line_was_line_comment = false;
        }
    }

    fn push_rendered_comment_lines(
        &mut self,
        rendered: &dendri_frontend_core::RenderedComment,
        indent: usize,
    ) {
        // Guard C1: never emit two distinct Line-style comments adjacent with 0 blank lines!
        let is_line_style = matches!(
            rendered.emitted_style,
            CommentStyle::Line | CommentStyle::DocLine
        );
        if is_line_style && self.last_line_was_line_comment {
            self.out.push('\n');
        }

        // Only prefix subsequent lines with indentation when every line carries its own `//`
        // delimiter; block comments (`(* ... *)`, `/* ... */`) own every byte after `\n` verbatim.
        for (line_idx, line) in rendered.rendered.split('\n').enumerate() {
            if line_idx > 0 {
                self.out.push('\n');
            }
            if line_idx == 0 || is_line_style {
                self.push_indent(indent);
            }
            self.out.push_str(line);
        }
    }

    fn emit_block_comment_by_idx(
        &mut self,
        idx: usize,
        placement: CommentPlacement,
        indent: usize,
    ) {
        if !self.emitted_comments.insert(idx) {
            return;
        }
        let Some(comment) = self.comments.get(idx) else {
            return;
        };
        let rendered = render_comment_for_language(
            LanguageId::St,
            &comment.text,
            comment.origin.style,
            placement,
            self.ctx,
        );
        self.diagnostics.extend(rendered.diagnostics.clone());
        self.push_rendered_comment_lines(&rendered, indent);
        self.out.push('\n');
        self.last_line_was_line_comment = matches!(
            rendered.emitted_style,
            CommentStyle::Line | CommentStyle::DocLine
        );
    }

    fn emit_leading_comments(&mut self, id: &NodeId, indent: usize) {
        let Some(indices) = self.comment_index.leading.get(id).cloned() else {
            return;
        };
        for idx in indices {
            self.emit_block_comment_by_idx(idx, CommentPlacement::Leading, indent);
        }
    }

    fn emit_dangling_comments(&mut self, id: &NodeId, indent: usize) {
        let Some(indices) = self.comment_index.dangling.get(id).cloned() else {
            return;
        };
        for idx in indices {
            self.emit_block_comment_by_idx(idx, CommentPlacement::Dangling, indent);
        }
    }

    /// Pre-renders any trailing comments for `id` that cannot stay single-line (e.g. multi-line
    /// `//` fallback) so they are emitted safely before the node without splitting across lines.
    fn emit_multiline_trailing_as_leading(&mut self, id: &NodeId, indent: usize) -> Vec<String> {
        let Some(indices) = self.comment_index.trailing.get(id).cloned() else {
            return Vec::new();
        };
        let mut inline_suffixes = Vec::new();
        for idx in indices {
            if self.emitted_comments.contains(&idx) {
                continue;
            }
            let Some(comment) = self.comments.get(idx) else {
                continue;
            };
            let rendered = render_comment_for_language(
                LanguageId::St,
                &comment.text,
                comment.origin.style,
                CommentPlacement::Trailing,
                self.ctx,
            );
            if rendered.rendered.contains('\n') && rendered.emitted_style == CommentStyle::Line {
                self.emit_block_comment_by_idx(idx, CommentPlacement::Leading, indent);
            } else if self.emitted_comments.insert(idx) {
                self.diagnostics.extend(rendered.diagnostics);
                inline_suffixes.push(rendered.rendered);
            }
        }
        inline_suffixes
    }

    fn emit_inline_trailing_suffixes(&mut self, suffixes: &[String]) {
        for s in suffixes {
            self.out.push(' ');
            self.out.push_str(s);
        }
    }

    fn emit_program(&mut self, program: &BoardProgram) {
        // 1. Header comments
        let headers = self.comment_index.header.clone();
        if !headers.is_empty() {
            for idx in headers {
                self.emit_block_comment_by_idx(idx, CommentPlacement::Header, 0);
            }
            self.push_blank_line_if_needed();
        }

        // 2. USING imports
        for imp in &program.imports {
            self.out.push_str("USING ");
            self.out.push_str(imp);
            self.out.push_str(";\n");
            self.last_line_was_line_comment = false;
        }
        if !program.imports.is_empty() {
            self.push_blank_line_if_needed();
        }

        // 3. Types (`TYPE ... END_TYPE` for structs/enums; verbatim or demarcated for `Decl::Foreign`)
        self.emit_type_decls(&program.types, 0);

        // 4. Global Hardware bindings (`AT %...` when `iec_address` exists, else plain `VAR_GLOBAL`)
        if !program.hardware.is_empty() {
            self.emit_hardware_block(&program.hardware, 0);
            self.push_blank_line_if_needed();
        }

        // 5. Global Data declarations in contiguous scope blocks
        if !program.data.is_empty() {
            self.emit_data_blocks(&program.data, 0, true);
            self.push_blank_line_if_needed();
        }

        // 6. Embedded modules (`NAMESPACE ... END_NAMESPACE`)
        for module in &program.modules {
            self.emit_module(module);
            self.push_blank_line_if_needed();
        }

        // 7. POUs (`PROGRAM`, `FUNCTION`, `FUNCTION_BLOCK`)
        for (idx, pou) in program.pous.iter().enumerate() {
            if idx > 0 {
                self.push_blank_line_if_needed();
            }
            self.emit_pou(pou, 0);
        }

        // 8. Board-level Body comments & any unvisited comments (guarantees C1 100%)
        let body_indices = self.comment_index.body.clone();
        let remaining: Vec<usize> = (0..self.comments.len())
            .filter(|idx| !self.emitted_comments.contains(idx) && !body_indices.contains(idx))
            .collect();

        if !body_indices.is_empty() || !remaining.is_empty() {
            self.push_blank_line_if_needed();
            for idx in body_indices {
                self.emit_block_comment_by_idx(idx, CommentPlacement::Body, 0);
            }
            for idx in remaining {
                self.emit_block_comment_by_idx(idx, CommentPlacement::Body, 0);
            }
        }
    }

    fn emit_type_decls(&mut self, types: &[TypeDecl], indent: usize) {
        let mut idx = 0usize;
        while idx < types.len() {
            let Some(td) = types.get(idx) else {
                break;
            };
            match td {
                TypeDecl::Foreign {
                    id,
                    language,
                    fidelity,
                    text,
                    pack_hint,
                    span,
                    ..
                } => {
                    self.emit_presented_blank_lines(id);
                    self.emit_leading_comments(id, indent);
                    let trailing = self.emit_multiline_trailing_as_leading(id, indent);
                    let site = ForeignSite {
                        level: ForeignLevel::Decl,
                        node_id: Some(id),
                        language,
                        fidelity: *fidelity,
                        text,
                        pack_hint: pack_hint.as_deref(),
                        span: *span,
                    };
                    if let Ok(decision) = check_foreign_export(&site, LanguageId::St, self.ctx) {
                        match decision {
                            ForeignExportDecision::VerbatimSameLanguage(raw) => {
                                self.push_indent(indent);
                                self.out.push_str(&raw);
                                self.emit_inline_trailing_suffixes(&trailing);
                                self.out.push('\n');
                                self.last_line_was_line_comment = false;
                            }
                            ForeignExportDecision::DemarcatedNotTranslated(rendered) => {
                                self.diagnostics.extend(rendered.diagnostics.clone());
                                self.push_rendered_comment_lines(&rendered, indent);
                                self.emit_inline_trailing_suffixes(&trailing);
                                self.out.push('\n');
                                self.last_line_was_line_comment = matches!(
                                    rendered.emitted_style,
                                    CommentStyle::Line | CommentStyle::DocLine
                                );
                            }
                        }
                    }
                    self.push_blank_line_if_needed();
                    idx = idx.saturating_add(1);
                }
                TypeDecl::Struct { .. } | TypeDecl::Enum { .. } => {
                    self.push_indent(indent);
                    self.out.push_str("TYPE\n");
                    self.last_line_was_line_comment = false;
                    while let Some(cur) = types.get(idx) {
                        match cur {
                            TypeDecl::Struct {
                                id, name, fields, ..
                            } => {
                                self.emit_leading_comments(id, indent.saturating_add(1));
                                let trailing = self.emit_multiline_trailing_as_leading(
                                    id,
                                    indent.saturating_add(1),
                                );
                                self.push_indent(indent.saturating_add(1));
                                self.out.push_str(name);
                                self.out.push_str(" : STRUCT\n");
                                self.last_line_was_line_comment = false;
                                for f in fields {
                                    self.push_indent(indent.saturating_add(2));
                                    self.out.push_str(&f.name);
                                    self.out.push_str(" : ");
                                    write_type_ref(&f.ty, &mut self.out);
                                    self.out.push_str(";\n");
                                }
                                self.push_indent(indent.saturating_add(1));
                                self.out.push_str("END_STRUCT;");
                                self.emit_inline_trailing_suffixes(&trailing);
                                self.out.push('\n');
                                self.last_line_was_line_comment = false;
                                idx = idx.saturating_add(1);
                            }
                            TypeDecl::Enum {
                                id,
                                name,
                                backing,
                                variants,
                                ..
                            } => {
                                self.emit_leading_comments(id, indent.saturating_add(1));
                                let trailing = self.emit_multiline_trailing_as_leading(
                                    id,
                                    indent.saturating_add(1),
                                );
                                self.push_indent(indent.saturating_add(1));
                                self.out.push_str(name);
                                self.out.push_str(" : ");
                                write_scalar_type(backing, &mut self.out);
                                self.out.push_str(" (\n");
                                self.last_line_was_line_comment = false;
                                for (vidx, v) in variants.iter().enumerate() {
                                    self.push_indent(indent.saturating_add(2));
                                    self.out.push_str(&v.name);
                                    self.out.push_str(" := ");
                                    self.out.push_str(&v.value.to_string());
                                    if vidx.saturating_add(1) < variants.len() {
                                        self.out.push(',');
                                    }
                                    self.out.push('\n');
                                }
                                self.push_indent(indent.saturating_add(1));
                                self.out.push_str(");");
                                self.emit_inline_trailing_suffixes(&trailing);
                                self.out.push('\n');
                                self.last_line_was_line_comment = false;
                                idx = idx.saturating_add(1);
                            }
                            TypeDecl::Foreign { .. } => break,
                        }
                    }
                    self.push_indent(indent);
                    self.out.push_str("END_TYPE\n");
                    self.last_line_was_line_comment = false;
                    self.push_blank_line_if_needed();
                }
            }
        }
    }

    fn emit_hardware_block(&mut self, hardware: &[HardwareBinding], indent: usize) {
        self.push_indent(indent);
        self.out.push_str("VAR_GLOBAL\n");
        self.last_line_was_line_comment = false;
        for hw in hardware {
            self.emit_leading_comments(&hw.id, indent.saturating_add(1));
            let trailing =
                self.emit_multiline_trailing_as_leading(&hw.id, indent.saturating_add(1));
            self.push_indent(indent.saturating_add(1));
            self.out.push_str(&hw.label);
            if let Some(iec_addr) = self.ctx.iec_for_port(&hw.pin) {
                self.out.push_str(" AT ");
                self.out.push_str(iec_addr);
            }
            self.out.push_str(" : ");
            self.out.push_str(hardware_emit_st_type(hw.kind));
            self.out.push(';');
            self.emit_inline_trailing_suffixes(&trailing);
            self.out.push('\n');
            self.last_line_was_line_comment = false;
        }
        self.push_indent(indent);
        self.out.push_str("END_VAR\n");
        self.last_line_was_line_comment = false;
    }

    fn emit_data_blocks(&mut self, decls: &[DataDecl], indent: usize, is_global: bool) {
        let mut idx = 0usize;
        while idx < decls.len() {
            let Some(first) = decls.get(idx) else {
                break;
            };
            let scope = first.scope;
            self.push_indent(indent);
            self.out.push_str(var_block_header(scope, is_global));
            self.out.push('\n');
            self.last_line_was_line_comment = false;

            while let Some(d) = decls.get(idx) {
                if d.scope != scope {
                    break;
                }
                self.emit_leading_comments(&d.id, indent.saturating_add(1));
                let trailing =
                    self.emit_multiline_trailing_as_leading(&d.id, indent.saturating_add(1));
                self.push_indent(indent.saturating_add(1));
                self.out.push_str(&d.name);
                self.out.push_str(" : ");
                write_type_ref(&d.ty, &mut self.out);
                if let Some(init) = &d.initial {
                    self.out.push_str(" := ");
                    let expr_str = self.format_init_expr(init, &d.ty);
                    self.out.push_str(&expr_str);
                }
                self.out.push(';');
                self.emit_inline_trailing_suffixes(&trailing);
                self.out.push('\n');
                self.last_line_was_line_comment = false;
                idx = idx.saturating_add(1);
            }

            self.push_indent(indent);
            self.out.push_str("END_VAR\n");
            self.last_line_was_line_comment = false;
        }
    }

    fn emit_param_blocks(&mut self, params: &[ParamDecl], indent: usize) {
        let mut idx = 0usize;
        while idx < params.len() {
            let Some(first) = params.get(idx) else {
                break;
            };
            let dir = first.direction;
            let header = match dir {
                ParamDirection::In => "VAR_INPUT",
                ParamDirection::Out => "VAR_OUTPUT",
                ParamDirection::InOut => "VAR_IN_OUT",
            };
            self.push_indent(indent);
            self.out.push_str(header);
            self.out.push('\n');
            self.last_line_was_line_comment = false;

            while let Some(p) = params.get(idx) {
                if p.direction != dir {
                    break;
                }
                self.push_indent(indent.saturating_add(1));
                self.out.push_str(&p.name);
                self.out.push_str(" : ");
                write_type_ref(&p.ty, &mut self.out);
                self.out.push_str(";\n");
                idx = idx.saturating_add(1);
            }

            self.push_indent(indent);
            self.out.push_str("END_VAR\n");
            self.last_line_was_line_comment = false;
        }
    }

    fn emit_module(&mut self, module: &ModuleDecl) {
        self.emit_presented_blank_lines(&module.id);
        self.emit_leading_comments(&module.id, 0);
        let trailing = self.emit_multiline_trailing_as_leading(&module.id, 0);
        self.out.push_str("NAMESPACE ");
        self.out.push_str(&module.name);
        self.emit_inline_trailing_suffixes(&trailing);
        self.out.push('\n');
        self.last_line_was_line_comment = false;

        for imp in &module.imports {
            self.push_indent(1);
            self.out.push_str("USING ");
            self.out.push_str(imp);
            self.out.push_str(";\n");
        }
        self.emit_type_decls(&module.types, 1);
        if !module.data.is_empty() {
            self.emit_data_blocks(&module.data, 1, true);
        }
        for pou in &module.pous {
            self.emit_pou(pou, 1);
        }
        self.emit_dangling_comments(&module.id, 1);
        self.out.push_str("END_NAMESPACE\n");
        self.last_line_was_line_comment = false;
    }

    fn emit_pou(&mut self, pou: &Pou, indent: usize) {
        self.emit_presented_blank_lines(&pou.id);
        self.emit_leading_comments(&pou.id, indent);
        let trailing = self.emit_multiline_trailing_as_leading(&pou.id, indent);

        let prev_locals = std::mem::take(&mut self.local_types);
        for p in &pou.params {
            self.local_types
                .insert(p.name.to_ascii_uppercase(), p.ty.clone());
        }
        for l in &pou.locals {
            self.local_types
                .insert(l.name.to_ascii_uppercase(), l.ty.clone());
        }
        let prev_ret = self.current_return_type.take();

        self.push_indent(indent);
        let (end_kw, fn_name) = match &pou.kind {
            PouKind::Ob { .. } => {
                self.out.push_str("PROGRAM ");
                self.out.push_str(&pou.name);
                ("END_PROGRAM", None)
            }
            PouKind::Fc { return_type } => {
                self.current_return_type = Some(return_type.clone());
                self.out.push_str("FUNCTION ");
                self.out.push_str(&pou.name);
                self.out.push_str(" : ");
                write_type_ref(return_type, &mut self.out);
                ("END_FUNCTION", Some(pou.name.as_str()))
            }
            PouKind::Fb => {
                self.out.push_str("FUNCTION_BLOCK ");
                self.out.push_str(&pou.name);
                ("END_FUNCTION_BLOCK", None)
            }
        };
        self.emit_inline_trailing_suffixes(&trailing);
        self.out.push('\n');
        self.last_line_was_line_comment = false;

        let inner_indent = indent.saturating_add(1);
        self.emit_param_blocks(&pou.params, inner_indent);
        self.emit_data_blocks(&pou.locals, inner_indent, false);

        match &pou.body {
            PouBody::Statements(stmts) => {
                self.emit_stmts(stmts, inner_indent, fn_name, true);
            }
            PouBody::Network(net) => {
                // Materialize collision-guarded local VAR_TEMP booleans strictly in emitted ST
                // (never persisted in ASL) so mid-rung resets & ParallelOr branches preserve exact power flow.
                let mut used_names = self.type_env.used_global_names.clone();
                used_names.insert(pou.name.to_ascii_uppercase());
                for p in &pou.params {
                    used_names.insert(p.name.to_ascii_uppercase());
                }
                for l in &pou.locals {
                    used_names.insert(l.name.to_ascii_uppercase());
                }
                let saved_out = std::mem::take(&mut self.out);
                let saved_last = self.last_line_was_line_comment;
                self.last_line_was_line_comment = false;
                let rail_temps = self.emit_derived_ladder(net, inner_indent, &mut used_names);
                let body_last = self.last_line_was_line_comment;
                let ladder_body = std::mem::replace(&mut self.out, saved_out);
                self.last_line_was_line_comment = saved_last;

                if !rail_temps.is_empty() {
                    self.push_indent(inner_indent);
                    self.out.push_str("VAR_TEMP\n");
                    for tname in &rail_temps {
                        self.push_indent(inner_indent.saturating_add(1));
                        self.out.push_str(tname);
                        self.out.push_str(" : BOOL;\n");
                    }
                    self.push_indent(inner_indent);
                    self.out.push_str("END_VAR\n");
                }
                self.out.push_str(&ladder_body);
                self.last_line_was_line_comment = body_last;
            }
            PouBody::Chart(chart) => {
                self.emit_derived_chart(chart, inner_indent);
            }
        }

        self.emit_dangling_comments(&pou.id, inner_indent);
        self.push_indent(indent);
        self.out.push_str(end_kw);
        self.out.push('\n');
        self.last_line_was_line_comment = false;

        self.local_types = prev_locals;
        self.current_return_type = prev_ret;
    }

    fn emit_stmts(
        &mut self,
        stmts: &[Stmt],
        indent: usize,
        fn_name: Option<&str>,
        is_top_level_of_fn: bool,
    ) {
        let len = stmts.len();
        for (idx, stmt) in stmts.iter().enumerate() {
            let is_last_top_stmt = is_top_level_of_fn && idx.saturating_add(1) == len;
            self.emit_stmt(stmt, indent, fn_name, is_last_top_stmt);
        }
    }

    fn emit_stmt(
        &mut self,
        stmt: &Stmt,
        indent: usize,
        fn_name: Option<&str>,
        is_last_top_stmt: bool,
    ) {
        self.emit_presented_blank_lines(&stmt.id);
        self.emit_leading_comments(&stmt.id, indent);
        let trailing = self.emit_multiline_trailing_as_leading(&stmt.id, indent);

        // Check for inner comments anchored on this statement
        let inner_comments = self
            .comment_index
            .inner
            .get(&stmt.id)
            .cloned()
            .unwrap_or_default();

        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                let target_ty = self.resolve_lvalue_type(target);
                let mut line = String::new();
                self.write_lvalue_to(target, &mut line);
                line.push_str(" := ");
                line.push_str(&self.format_expr_typed(value, 0, target_ty.as_ref()));
                line.push(';');
                self.emit_single_line_stmt_with_inner(
                    &stmt.id,
                    line,
                    &inner_comments,
                    &trailing,
                    indent,
                );
            }
            StmtKind::Call { callee, args } => {
                let arg_tys = self.call_arg_expected_types(callee, args, None);
                let mut line = String::new();
                line.push_str(callee);
                line.push('(');
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        line.push_str(", ");
                    }
                    if let Some(name) = &arg.name {
                        line.push_str(name);
                        line.push_str(" := ");
                    }
                    let exp = arg_tys.get(i).and_then(|t| t.as_ref());
                    line.push_str(&self.format_expr_typed(&arg.value, 0, exp));
                }
                line.push_str(");");
                self.emit_single_line_stmt_with_inner(
                    &stmt.id,
                    line,
                    &inner_comments,
                    &trailing,
                    indent,
                );
            }
            StmtKind::FbInvoke { instance, inputs } => {
                let inst_ty = self.resolve_lvalue_type(instance);
                let mut line = String::new();
                self.write_lvalue_to(instance, &mut line);
                line.push('(');
                for (i, (pname, expr)) in inputs.iter().enumerate() {
                    if i > 0 {
                        line.push_str(", ");
                    }
                    line.push_str(pname);
                    line.push_str(" := ");
                    let exp = inst_ty
                        .as_ref()
                        .and_then(|ty| self.type_env.fb_field_type(ty, pname));
                    line.push_str(&self.format_expr_typed(expr, 0, exp.as_ref()));
                }
                line.push_str(");");
                self.emit_single_line_stmt_with_inner(
                    &stmt.id,
                    line,
                    &inner_comments,
                    &trailing,
                    indent,
                );
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                self.relocate_remaining_inner(&inner_comments, indent);
                self.push_indent(indent);
                self.out.push_str("IF ");
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                let cond_s = self.format_expr_typed(condition, 0, Some(&bool_ty));
                self.out.push_str(&cond_s);
                self.out.push_str(" THEN\n");
                self.last_line_was_line_comment = false;
                self.emit_stmts(then_body, indent.saturating_add(1), fn_name, false);
                for arm in elif_arms {
                    self.push_indent(indent);
                    self.out.push_str("ELSIF ");
                    let arm_s = self.format_expr_typed(&arm.condition, 0, Some(&bool_ty));
                    self.out.push_str(&arm_s);
                    self.out.push_str(" THEN\n");
                    self.last_line_was_line_comment = false;
                    self.emit_stmts(&arm.body, indent.saturating_add(1), fn_name, false);
                }
                if !else_body.is_empty() {
                    self.push_indent(indent);
                    self.out.push_str("ELSE\n");
                    self.last_line_was_line_comment = false;
                    self.emit_stmts(else_body, indent.saturating_add(1), fn_name, false);
                }
                self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                self.push_indent(indent);
                self.out.push_str("END_IF;");
                self.emit_inline_trailing_suffixes(&trailing);
                self.out.push('\n');
                self.last_line_was_line_comment = false;
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                self.relocate_remaining_inner(&inner_comments, indent);
                let scrut_ty = self.infer_emit_expr_type(scrutinee);
                self.push_indent(indent);
                self.out.push_str("CASE ");
                let scr = self.format_expr_typed(scrutinee, 0, None);
                self.out.push_str(&scr);
                self.out.push_str(" OF\n");
                self.last_line_was_line_comment = false;
                for c in cases {
                    self.push_indent(indent.saturating_add(1));
                    for (pidx, pat) in c.patterns.iter().enumerate() {
                        if pidx > 0 {
                            self.out.push_str(", ");
                        }
                        match pat {
                            CasePattern::Value(e) => {
                                let s = self.format_expr_typed(e, 0, scrut_ty.as_ref());
                                self.out.push_str(&s);
                            }
                            CasePattern::Range { start, end } => {
                                let lo = self.format_expr_typed(start, 0, scrut_ty.as_ref());
                                let hi = self.format_expr_typed(end, 0, scrut_ty.as_ref());
                                self.out.push_str(&lo);
                                self.out.push_str("..");
                                self.out.push_str(&hi);
                            }
                        }
                    }
                    self.out.push_str(":\n");
                    self.last_line_was_line_comment = false;
                    self.emit_stmts(&c.body, indent.saturating_add(2), fn_name, false);
                }
                if let Some(def) = default {
                    self.push_indent(indent);
                    self.out.push_str("ELSE\n");
                    self.last_line_was_line_comment = false;
                    self.emit_stmts(def, indent.saturating_add(1), fn_name, false);
                }
                self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                self.push_indent(indent);
                self.out.push_str("END_CASE;");
                self.emit_inline_trailing_suffixes(&trailing);
                self.out.push('\n');
                self.last_line_was_line_comment = false;
            }
            StmtKind::While { condition, body } => {
                self.relocate_remaining_inner(&inner_comments, indent);
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                self.push_indent(indent);
                self.out.push_str("WHILE ");
                let c = self.format_expr_typed(condition, 0, Some(&bool_ty));
                self.out.push_str(&c);
                self.out.push_str(" DO\n");
                self.last_line_was_line_comment = false;
                self.emit_stmts(body, indent.saturating_add(1), fn_name, false);
                self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                self.push_indent(indent);
                self.out.push_str("END_WHILE;");
                self.emit_inline_trailing_suffixes(&trailing);
                self.out.push('\n');
                self.last_line_was_line_comment = false;
            }
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            } => {
                self.relocate_remaining_inner(&inner_comments, indent);
                let counter_ty = self.lookup_var_type(counter);
                let from_s = self.format_expr_typed(from, 0, counter_ty.as_ref());
                let to_s = self.format_expr_typed(to, 0, counter_ty.as_ref());
                let step_s = self.format_expr_typed(step, 0, counter_ty.as_ref());

                if !*inclusive {
                    match eval_const_int_expr(step) {
                        Some(s) if s > 0 => {
                            self.push_indent(indent);
                            self.out.push_str("FOR ");
                            self.out.push_str(counter);
                            self.out.push_str(" := ");
                            self.out.push_str(&from_s);
                            self.out.push_str(" TO (");
                            self.out.push_str(&to_s);
                            self.out.push_str(" - 1)");
                            if s != 1 {
                                self.out.push_str(" BY ");
                                self.out.push_str(&step_s);
                            }
                            self.out.push_str(" DO\n");
                            self.last_line_was_line_comment = false;
                            self.emit_stmts(body, indent.saturating_add(1), fn_name, false);
                            self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                            self.push_indent(indent);
                            self.out.push_str("END_FOR;");
                            self.emit_inline_trailing_suffixes(&trailing);
                            self.out.push('\n');
                            self.last_line_was_line_comment = false;
                        }
                        Some(s) if s < 0 => {
                            self.push_indent(indent);
                            self.out.push_str("FOR ");
                            self.out.push_str(counter);
                            self.out.push_str(" := ");
                            self.out.push_str(&from_s);
                            self.out.push_str(" TO (");
                            self.out.push_str(&to_s);
                            self.out.push_str(" + 1) BY ");
                            self.out.push_str(&step_s);
                            self.out.push_str(" DO\n");
                            self.last_line_was_line_comment = false;
                            self.emit_stmts(body, indent.saturating_add(1), fn_name, false);
                            self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                            self.push_indent(indent);
                            self.out.push_str("END_FOR;");
                            self.emit_inline_trailing_suffixes(&trailing);
                            self.out.push('\n');
                            self.last_line_was_line_comment = false;
                        }
                        Some(_) | None => {
                            // Unknown-sign non-const step (or zero step): lower to a WHILE loop
                            // preserving signed direction and updating the counter before CONTINUE.
                            let is_unsigned = matches!(
                                &counter_ty,
                                Some(TypeRef::Scalar { ty }) if ty.is_unsigned_int()
                            );
                            self.push_indent(indent);
                            self.out.push_str(&format!("{counter} := {from_s};\n"));
                            self.push_indent(indent);
                            if is_unsigned {
                                self.out.push_str(&format!(
                                    "WHILE ({step_s} > 0) AND ({counter} < {to_s}) DO\n"
                                ));
                            } else {
                                self.out.push_str(&format!(
                                    "WHILE (({step_s} > 0) AND ({counter} < {to_s})) OR (({step_s} < 0) AND ({counter} > {to_s})) DO\n"
                                ));
                            }
                            self.last_line_was_line_comment = false;
                            let step_stmt = format!("{counter} := {counter} + {step_s};");
                            self.emit_stmts_with_continue_prefix(
                                body,
                                &step_stmt,
                                indent.saturating_add(1),
                                fn_name,
                            );
                            self.push_indent(indent.saturating_add(1));
                            self.out.push_str(&step_stmt);
                            self.out.push('\n');
                            self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                            self.push_indent(indent);
                            self.out.push_str("END_WHILE;");
                            self.emit_inline_trailing_suffixes(&trailing);
                            self.out.push('\n');
                            self.last_line_was_line_comment = false;
                        }
                    }
                } else {
                    self.push_indent(indent);
                    self.out.push_str("FOR ");
                    self.out.push_str(counter);
                    self.out.push_str(" := ");
                    self.out.push_str(&from_s);
                    self.out.push_str(" TO ");
                    self.out.push_str(&to_s);
                    let is_default_step = matches!(step.kind, ExprKind::Literal(Literal::Int(1)));
                    if !is_default_step {
                        self.out.push_str(" BY ");
                        self.out.push_str(&step_s);
                    }
                    self.out.push_str(" DO\n");
                    self.last_line_was_line_comment = false;
                    self.emit_stmts(body, indent.saturating_add(1), fn_name, false);
                    self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                    self.push_indent(indent);
                    self.out.push_str("END_FOR;");
                    self.emit_inline_trailing_suffixes(&trailing);
                    self.out.push('\n');
                    self.last_line_was_line_comment = false;
                }
            }
            StmtKind::Break => {
                self.emit_single_line_stmt_with_inner(
                    &stmt.id,
                    "EXIT;".to_owned(),
                    &inner_comments,
                    &trailing,
                    indent,
                );
            }
            StmtKind::Continue => {
                self.emit_single_line_stmt_with_inner(
                    &stmt.id,
                    "CONTINUE;".to_owned(),
                    &inner_comments,
                    &trailing,
                    indent,
                );
            }
            StmtKind::Return { value } => match (fn_name, value) {
                (Some(fname), Some(expr)) => {
                    let ret_ty = self.current_return_type.clone();
                    let mut line = String::new();
                    line.push_str(fname);
                    line.push_str(" := ");
                    line.push_str(&self.format_expr_typed(expr, 0, ret_ty.as_ref()));
                    line.push(';');
                    self.emit_single_line_stmt_with_inner(
                        &stmt.id,
                        line,
                        &inner_comments,
                        &trailing,
                        indent,
                    );
                    if !is_last_top_stmt {
                        self.push_indent(indent);
                        self.out.push_str("RETURN;\n");
                        self.last_line_was_line_comment = false;
                    }
                }
                (None, Some(expr)) => {
                    // Fallback if Return(Some) appears outside a FUNCTION
                    let mut line = String::new();
                    line.push_str("RETURN ");
                    line.push_str(&self.format_expr(expr, 0));
                    line.push(';');
                    self.emit_single_line_stmt_with_inner(
                        &stmt.id,
                        line,
                        &inner_comments,
                        &trailing,
                        indent,
                    );
                }
                (_, None) => {
                    self.emit_single_line_stmt_with_inner(
                        &stmt.id,
                        "RETURN;".to_owned(),
                        &inner_comments,
                        &trailing,
                        indent,
                    );
                }
            },
            StmtKind::Wait { duration } => {
                let mut line = String::new();
                line.push_str("WAIT(");
                line.push_str(&self.format_expr(duration, 0));
                line.push_str(");");
                self.emit_single_line_stmt_with_inner(
                    &stmt.id,
                    line,
                    &inner_comments,
                    &trailing,
                    indent,
                );
            }
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => {
                self.relocate_remaining_inner(&inner_comments, indent);
                let site = ForeignSite {
                    level: ForeignLevel::Stmt,
                    node_id: Some(&stmt.id),
                    language,
                    fidelity: *fidelity,
                    text,
                    pack_hint: pack_hint.as_deref(),
                    span: stmt.span,
                };
                if let Ok(decision) = check_foreign_export(&site, LanguageId::St, self.ctx) {
                    match decision {
                        ForeignExportDecision::VerbatimSameLanguage(raw) => {
                            self.push_indent(indent);
                            self.out.push_str(&raw);
                            self.emit_inline_trailing_suffixes(&trailing);
                            self.out.push('\n');
                            self.last_line_was_line_comment = false;
                        }
                        ForeignExportDecision::DemarcatedNotTranslated(rendered) => {
                            self.diagnostics.extend(rendered.diagnostics.clone());
                            self.push_rendered_comment_lines(&rendered, indent);
                            self.emit_inline_trailing_suffixes(&trailing);
                            self.out.push('\n');
                            self.last_line_was_line_comment = matches!(
                                rendered.emitted_style,
                                CommentStyle::Line | CommentStyle::DocLine
                            );
                        }
                    }
                }
            }
        }
    }

    fn emit_single_line_stmt_with_inner(
        &mut self,
        _stmt_id: &NodeId,
        stmt_text: String,
        inner_comments: &[(u32, usize)],
        trailing: &[String],
        indent: usize,
    ) {
        if inner_comments.is_empty() {
            self.push_indent(indent);
            self.out.push_str(&stmt_text);
            self.emit_inline_trailing_suffixes(trailing);
            self.out.push('\n');
            self.last_line_was_line_comment = false;
            return;
        }

        // Tokenize the canonical statement line so Inner comments splice at exact `token_ordinal`
        let lex_tokens = lex_st(&stmt_text, self.ctx.st_dialect, &self.ctx.board_id)
            .ok()
            .map(|l| l.tokens);

        let mut insertions: Vec<(usize, String)> = Vec::new();
        let mut fallback_relocate: Vec<(u32, usize)> = Vec::new();

        for &(ord, cidx) in inner_comments {
            let ord_usize = usize::try_from(ord).unwrap_or(usize::MAX);
            let can_splice = lex_tokens.as_ref().and_then(|toks| {
                // Non-EOF tokens live at 0..toks.len()-1; splice strictly inside (1..tok_count)
                let tok_count = toks.len().saturating_sub(1);
                if ord_usize > 0 && ord_usize < tok_count {
                    toks.get(ord_usize).map(|t| t.span.start as usize)
                } else {
                    None
                }
            });

            if let Some(byte_pos) = can_splice
                && let Some(comment) = self.comments.get(cidx)
                && self.emitted_comments.insert(cidx)
            {
                let rendered = render_comment_for_language(
                    LanguageId::St,
                    &comment.text,
                    comment.origin.style,
                    CommentPlacement::Inner { token_ordinal: ord },
                    self.ctx,
                );
                self.diagnostics.extend(rendered.diagnostics);
                insertions.push((byte_pos, rendered.rendered));
            } else {
                fallback_relocate.push((ord, cidx));
            }
        }

        if !fallback_relocate.is_empty() {
            self.relocate_remaining_inner(&fallback_relocate, indent);
        }

        let mut final_line = stmt_text;
        // Splice from highest byte offset downward so earlier offsets remain exact
        insertions.sort_by_key(|&(pos, _)| std::cmp::Reverse(pos));
        for (pos, snippet) in insertions {
            if pos <= final_line.len() && final_line.is_char_boundary(pos) {
                final_line.insert_str(pos, &format!("{snippet} "));
            }
        }

        self.push_indent(indent);
        self.out.push_str(&final_line);
        self.emit_inline_trailing_suffixes(trailing);
        self.out.push('\n');
        self.last_line_was_line_comment = false;
    }

    fn relocate_remaining_inner(&mut self, inner_comments: &[(u32, usize)], indent: usize) {
        for &(_ord, cidx) in inner_comments {
            if self.emitted_comments.contains(&cidx) {
                continue;
            }
            if let Some(comment) = self.comments.get(cidx) {
                let mut diag = Diagnostic::new(
                    ErrorCode::InfoCommentRelocated,
                    format!(
                        "inner comment `{}` relocated to enclosing ST statement boundary",
                        comment.id
                    ),
                )
                .with_board_id(&self.ctx.board_id);
                diag.span = Some(comment.span);
                self.diagnostics.push(diag);
            }
            self.emit_block_comment_by_idx(cidx, CommentPlacement::Leading, indent);
        }
    }

    fn write_lvalue_to(&mut self, lv: &LValue, buf: &mut String) {
        match lv {
            LValue::Var { name, .. } => buf.push_str(name),
            LValue::Index { base, index, .. } => {
                self.write_lvalue_to(base, buf);
                buf.push('[');
                buf.push_str(&self.format_expr(index, 0));
                buf.push(']');
            }
            LValue::Field { base, field, .. } => {
                self.write_lvalue_to(base, buf);
                buf.push('.');
                buf.push_str(field);
            }
        }
    }

    fn format_init_expr(&mut self, expr: &Expr, target_ty: &TypeRef) -> String {
        self.format_expr_typed(expr, 0, Some(target_ty))
    }

    fn format_expr(&mut self, expr: &Expr, parent_prec: u8) -> String {
        self.format_expr_typed(expr, parent_prec, None)
    }

    fn format_expr_typed(
        &mut self,
        expr: &Expr,
        parent_prec: u8,
        expected: Option<&TypeRef>,
    ) -> String {
        let mut buf = String::new();
        self.write_expr_to(expr, parent_prec, expected, &mut buf);
        buf
    }

    fn lookup_var_type(&self, name: &str) -> Option<TypeRef> {
        let key = name.to_ascii_uppercase();
        self.local_types
            .get(&key)
            .or_else(|| self.type_env.globals.get(&key))
            .cloned()
    }

    fn resolve_lvalue_type(&self, lv: &LValue) -> Option<TypeRef> {
        match lv {
            LValue::Var { name, .. } => self.lookup_var_type(name),
            LValue::Index { base, .. } => match self.resolve_lvalue_type(base)? {
                TypeRef::Array { elem, .. } => Some(*elem),
                TypeRef::Void
                | TypeRef::Scalar { .. }
                | TypeRef::Named { .. }
                | TypeRef::Fb { .. } => None,
            },
            LValue::Field { base, field, .. } => {
                let base_ty = self.resolve_lvalue_type(base)?;
                self.resolve_field_type(&base_ty, field)
            }
        }
    }

    fn resolve_field_type(&self, base_ty: &TypeRef, field: &str) -> Option<TypeRef> {
        let fkey = field.to_ascii_uppercase();
        match base_ty {
            TypeRef::Named { name } => self
                .type_env
                .structs
                .get(&name.to_ascii_uppercase())
                .and_then(|fields| fields.get(&fkey))
                .cloned(),
            TypeRef::Fb { .. } => self.type_env.fb_field_type(base_ty, field),
            TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Array { .. } => None,
        }
    }

    /// Lightweight type inference for emitter negative-literal disambiguation.
    /// Returns `None` for untyped numeric literals so sibling inference prefers typed operands.
    fn infer_emit_expr_type(&self, expr: &Expr) -> Option<TypeRef> {
        match &expr.kind {
            ExprKind::Literal(lit) => match lit {
                Literal::Bool(_) => Some(TypeRef::scalar(ScalarType::Bool)),
                Literal::TimeUs(_) => Some(TypeRef::scalar(ScalarType::Time)),
                Literal::String(_) => Some(TypeRef::scalar(ScalarType::String { max_len: 255 })),
                Literal::Int(_) | Literal::Float(_) => None,
            },
            ExprKind::Ident(name) => self.lookup_var_type(name),
            ExprKind::Index { base, .. } => match self.infer_emit_expr_type(base)? {
                TypeRef::Array { elem, .. } => Some(*elem),
                TypeRef::Void
                | TypeRef::Scalar { .. }
                | TypeRef::Named { .. }
                | TypeRef::Fb { .. } => None,
            },
            ExprKind::Field { base, field } => {
                if let ExprKind::Ident(bname) = &base.kind
                    && self.lookup_var_type(bname).is_none()
                {
                    if field.eq_ignore_ascii_case("X") || field.eq_ignore_ascii_case("DONE") {
                        return Some(TypeRef::scalar(ScalarType::Bool));
                    }
                    if field.eq_ignore_ascii_case("T") {
                        return Some(TypeRef::scalar(ScalarType::Time));
                    }
                }
                let base_ty = self.infer_emit_expr_type(base)?;
                self.resolve_field_type(&base_ty, field)
            }
            ExprKind::Call { callee, args } => {
                let upper = callee.to_ascii_uppercase();
                if let Some((_, ret)) = self.type_env.user_fcs.get(&upper) {
                    return Some(ret.clone());
                }
                if let Some((_, ret)) = self.type_env.pack_fns.get(&upper) {
                    return ret.clone();
                }
                let import_refs: Vec<&str> =
                    self.type_env.imports.iter().map(String::as_str).collect();
                if let Some(StdSymbol::Function(sig)) = self
                    .type_env
                    .std_catalog
                    .resolve_symbol(&import_refs, &upper)
                {
                    if sig.return_type == StdTypeRef::AnyNum {
                        for arg in args {
                            if let Some(t) = self.infer_emit_expr_type(&arg.value) {
                                return Some(t);
                            }
                        }
                        return None;
                    }
                    return std_type_to_opt_typeref(sig.return_type);
                }
                None
            }
            ExprKind::Unary { op, operand } => match op {
                UnaryOp::Not => Some(TypeRef::scalar(ScalarType::Bool)),
                UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot => self.infer_emit_expr_type(operand),
            },
            ExprKind::Binary { op, left, right } => match op {
                BinaryOp::And
                | BinaryOp::Xor
                | BinaryOp::Or
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::Eq
                | BinaryOp::Ne => Some(TypeRef::scalar(ScalarType::Bool)),
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Mod
                | BinaryOp::BitAnd
                | BinaryOp::BitXor
                | BinaryOp::BitOr => self
                    .infer_emit_expr_type(left)
                    .or_else(|| self.infer_emit_expr_type(right)),
                BinaryOp::Shl | BinaryOp::Shr => self.infer_emit_expr_type(left),
            },
            ExprKind::Foreign { .. } => None,
        }
    }

    fn call_arg_expected_types(
        &self,
        callee: &str,
        args: &[CallArg],
        outer_expected: Option<&TypeRef>,
    ) -> Vec<Option<TypeRef>> {
        let upper = callee.to_ascii_uppercase();
        if let Some((params, _)) = self.type_env.user_fcs.get(&upper) {
            return args
                .iter()
                .enumerate()
                .map(|(i, arg)| {
                    if let Some(aname) = &arg.name {
                        params
                            .iter()
                            .find(|p| p.name.eq_ignore_ascii_case(aname))
                            .map(|p| p.ty.clone())
                    } else {
                        params.get(i).map(|p| p.ty.clone())
                    }
                })
                .collect();
        }
        if let Some((params, _)) = self.type_env.pack_fns.get(&upper) {
            return args
                .iter()
                .enumerate()
                .map(|(i, arg)| {
                    if let Some(aname) = &arg.name {
                        params
                            .iter()
                            .find(|(pname, _)| pname.eq_ignore_ascii_case(aname))
                            .and_then(|(_, ty)| ty.clone())
                    } else {
                        params.get(i).and_then(|(_, ty)| ty.clone())
                    }
                })
                .collect();
        }
        let import_refs: Vec<&str> = self.type_env.imports.iter().map(String::as_str).collect();
        if let Some(StdSymbol::Function(sig)) = self
            .type_env
            .std_catalog
            .resolve_symbol(&import_refs, &upper)
        {
            // Infer common AnyNum type from outer_expected or any typed argument
            let any_num_ty = outer_expected.cloned().or_else(|| {
                args.iter().enumerate().find_map(|(i, arg)| {
                    let is_any = sig
                        .params
                        .get(i)
                        .is_some_and(|p| p.ty == StdTypeRef::AnyNum)
                        || (sig.params.len() == 1
                            && sig
                                .params
                                .first()
                                .is_some_and(|p| p.ty == StdTypeRef::AnyNum));
                    if is_any {
                        self.infer_emit_expr_type(&arg.value)
                    } else {
                        None
                    }
                })
            });
            return args
                .iter()
                .enumerate()
                .map(|(i, arg)| {
                    let p = if let Some(aname) = &arg.name {
                        sig.params
                            .iter()
                            .find(|p| p.name.eq_ignore_ascii_case(aname))
                    } else {
                        sig.params.get(i)
                    };
                    match p.map(|p| p.ty) {
                        Some(StdTypeRef::AnyNum) => any_num_ty.clone(),
                        Some(other) => std_type_to_opt_typeref(other),
                        None => None,
                    }
                })
                .collect();
        }
        vec![None; args.len()]
    }

    fn emit_stmts_with_continue_prefix(
        &mut self,
        stmts: &[Stmt],
        step_stmt: &str,
        indent: usize,
        fn_name: Option<&str>,
    ) {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Continue => {
                    self.emit_presented_blank_lines(&stmt.id);
                    self.emit_leading_comments(&stmt.id, indent);
                    let trailing = self.emit_multiline_trailing_as_leading(&stmt.id, indent);
                    self.push_indent(indent);
                    self.out.push_str(step_stmt);
                    self.out.push('\n');
                    self.push_indent(indent);
                    self.out.push_str("CONTINUE;");
                    self.emit_inline_trailing_suffixes(&trailing);
                    self.out.push('\n');
                    self.last_line_was_line_comment = false;
                }
                StmtKind::If {
                    condition,
                    then_body,
                    elif_arms,
                    else_body,
                } => {
                    self.emit_presented_blank_lines(&stmt.id);
                    self.emit_leading_comments(&stmt.id, indent);
                    let trailing = self.emit_multiline_trailing_as_leading(&stmt.id, indent);
                    let inner_comments = self
                        .comment_index
                        .inner
                        .get(&stmt.id)
                        .cloned()
                        .unwrap_or_default();
                    self.relocate_remaining_inner(&inner_comments, indent);
                    let bool_ty = TypeRef::scalar(ScalarType::Bool);
                    self.push_indent(indent);
                    self.out.push_str("IF ");
                    let cond_s = self.format_expr_typed(condition, 0, Some(&bool_ty));
                    self.out.push_str(&cond_s);
                    self.out.push_str(" THEN\n");
                    self.last_line_was_line_comment = false;
                    self.emit_stmts_with_continue_prefix(
                        then_body,
                        step_stmt,
                        indent.saturating_add(1),
                        fn_name,
                    );
                    for arm in elif_arms {
                        self.push_indent(indent);
                        self.out.push_str("ELSIF ");
                        let arm_s = self.format_expr_typed(&arm.condition, 0, Some(&bool_ty));
                        self.out.push_str(&arm_s);
                        self.out.push_str(" THEN\n");
                        self.last_line_was_line_comment = false;
                        self.emit_stmts_with_continue_prefix(
                            &arm.body,
                            step_stmt,
                            indent.saturating_add(1),
                            fn_name,
                        );
                    }
                    if !else_body.is_empty() {
                        self.push_indent(indent);
                        self.out.push_str("ELSE\n");
                        self.last_line_was_line_comment = false;
                        self.emit_stmts_with_continue_prefix(
                            else_body,
                            step_stmt,
                            indent.saturating_add(1),
                            fn_name,
                        );
                    }
                    self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                    self.push_indent(indent);
                    self.out.push_str("END_IF;");
                    self.emit_inline_trailing_suffixes(&trailing);
                    self.out.push('\n');
                    self.last_line_was_line_comment = false;
                }
                StmtKind::Switch {
                    scrutinee,
                    cases,
                    default,
                } => {
                    self.emit_presented_blank_lines(&stmt.id);
                    self.emit_leading_comments(&stmt.id, indent);
                    let trailing = self.emit_multiline_trailing_as_leading(&stmt.id, indent);
                    let inner_comments = self
                        .comment_index
                        .inner
                        .get(&stmt.id)
                        .cloned()
                        .unwrap_or_default();
                    self.relocate_remaining_inner(&inner_comments, indent);
                    let scrut_ty = self.infer_emit_expr_type(scrutinee);
                    self.push_indent(indent);
                    self.out.push_str("CASE ");
                    let scr = self.format_expr_typed(scrutinee, 0, None);
                    self.out.push_str(&scr);
                    self.out.push_str(" OF\n");
                    self.last_line_was_line_comment = false;
                    for c in cases {
                        self.push_indent(indent.saturating_add(1));
                        for (pidx, pat) in c.patterns.iter().enumerate() {
                            if pidx > 0 {
                                self.out.push_str(", ");
                            }
                            match pat {
                                CasePattern::Value(e) => {
                                    let s = self.format_expr_typed(e, 0, scrut_ty.as_ref());
                                    self.out.push_str(&s);
                                }
                                CasePattern::Range { start, end } => {
                                    let lo = self.format_expr_typed(start, 0, scrut_ty.as_ref());
                                    let hi = self.format_expr_typed(end, 0, scrut_ty.as_ref());
                                    self.out.push_str(&lo);
                                    self.out.push_str("..");
                                    self.out.push_str(&hi);
                                }
                            }
                        }
                        self.out.push_str(":\n");
                        self.last_line_was_line_comment = false;
                        self.emit_stmts_with_continue_prefix(
                            &c.body,
                            step_stmt,
                            indent.saturating_add(2),
                            fn_name,
                        );
                    }
                    if let Some(def) = default {
                        self.push_indent(indent);
                        self.out.push_str("ELSE\n");
                        self.last_line_was_line_comment = false;
                        self.emit_stmts_with_continue_prefix(
                            def,
                            step_stmt,
                            indent.saturating_add(1),
                            fn_name,
                        );
                    }
                    self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                    self.push_indent(indent);
                    self.out.push_str("END_CASE;");
                    self.emit_inline_trailing_suffixes(&trailing);
                    self.out.push('\n');
                    self.last_line_was_line_comment = false;
                }
                // Nested loops & all other statements keep normal emission
                StmtKind::For { .. }
                | StmtKind::While { .. }
                | StmtKind::Assign { .. }
                | StmtKind::Call { .. }
                | StmtKind::FbInvoke { .. }
                | StmtKind::Break
                | StmtKind::Return { .. }
                | StmtKind::Wait { .. }
                | StmtKind::Foreign { .. } => {
                    self.emit_stmt(stmt, indent, fn_name, false);
                }
            }
        }
    }

    fn write_expr_to(
        &mut self,
        expr: &Expr,
        parent_prec: u8,
        expected: Option<&TypeRef>,
        buf: &mut String,
    ) {
        // Cross-platform reduced-rational expansion for `ANALOG_TO_CODE(CODE_TO_ANALOG(e, src), dst)`
        if let Some(scaled) = self.try_emit_reduced_rational_call(expr) {
            buf.push_str(&scaled);
            return;
        }

        // Canonical ST analog idiom: `ADC_CODE(PressRaw)` -> `PressRaw` for direct-addressed `AnalogIn` tags
        if let ExprKind::Call { callee, args } = &expr.kind
            && callee.eq_ignore_ascii_case("ADC_CODE")
            && args.len() == 1
            && let Some(first) = args.first()
            && first.name.is_none()
            && let ExprKind::Ident(tag) = &first.value.kind
            && self
                .analog_in_direct_tags
                .contains(&tag.to_ascii_uppercase())
        {
            buf.push_str(tag);
            return;
        }

        let my_prec = expr_prec(&expr.kind);
        let need_parens = my_prec < parent_prec;
        if need_parens {
            buf.push('(');
        }

        match &expr.kind {
            ExprKind::Literal(lit) => write_literal(lit, expected, buf),
            ExprKind::Ident(name) => buf.push_str(name),
            ExprKind::Index { base, index } => {
                let wrap_base = expr_prec(&base.kind) < 110;
                if wrap_base {
                    buf.push('(');
                }
                self.write_expr_to(base, 0, None, buf);
                if wrap_base {
                    buf.push(')');
                }
                buf.push('[');
                self.write_expr_to(index, 0, None, buf);
                buf.push(']');
            }
            ExprKind::Field { base, field } => {
                let wrap_base = expr_prec(&base.kind) < 110;
                if wrap_base {
                    buf.push('(');
                }
                self.write_expr_to(base, 0, None, buf);
                if wrap_base {
                    buf.push(')');
                }
                buf.push('.');
                buf.push_str(field);
            }
            ExprKind::Call { callee, args } => {
                let arg_tys = self.call_arg_expected_types(callee, args, expected);
                buf.push_str(callee);
                buf.push('(');
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        buf.push_str(", ");
                    }
                    if let Some(name) = &arg.name {
                        buf.push_str(name);
                        buf.push_str(" := ");
                    }
                    let exp = arg_tys.get(i).and_then(|t| t.as_ref());
                    self.write_expr_to(&arg.value, 0, exp, buf);
                }
                buf.push(')');
            }
            ExprKind::Unary { op, operand } => {
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                let op_expected = match op {
                    UnaryOp::Not => Some(&bool_ty),
                    UnaryOp::Neg | UnaryOp::Pos | UnaryOp::BitNot => expected,
                };
                match op {
                    UnaryOp::Neg => buf.push('-'),
                    UnaryOp::Pos => buf.push('+'),
                    UnaryOp::Not => buf.push_str("NOT "),
                    UnaryOp::BitNot => buf.push_str("BIT_NOT "),
                }
                let wrap_operand = expr_prec(&operand.kind) < 100
                    || matches!(
                        (op, &operand.kind),
                        (
                            UnaryOp::Neg | UnaryOp::Pos,
                            ExprKind::Unary {
                                op: UnaryOp::Neg | UnaryOp::Pos,
                                ..
                            }
                        )
                    );
                if wrap_operand {
                    buf.push('(');
                    self.write_expr_to(operand, 0, op_expected, buf);
                    buf.push(')');
                } else {
                    self.write_expr_to(operand, 100, op_expected, buf);
                }
            }
            ExprKind::Binary { op, left, right } => {
                let prec = binary_prec(*op);
                let bool_ty = TypeRef::scalar(ScalarType::Bool);
                let (left_exp_owned, right_exp_owned): (Option<TypeRef>, Option<TypeRef>) = match op
                {
                    BinaryOp::And | BinaryOp::Xor | BinaryOp::Or => {
                        (Some(bool_ty.clone()), Some(bool_ty))
                    }
                    BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::Eq
                    | BinaryOp::Ne => {
                        let cmp_ty = self
                            .infer_emit_expr_type(left)
                            .or_else(|| self.infer_emit_expr_type(right));
                        (cmp_ty.clone(), cmp_ty)
                    }
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod
                    | BinaryOp::BitAnd
                    | BinaryOp::BitXor
                    | BinaryOp::BitOr => {
                        let common = expected
                            .cloned()
                            .or_else(|| self.infer_emit_expr_type(left))
                            .or_else(|| self.infer_emit_expr_type(right));
                        (common.clone(), common)
                    }
                    BinaryOp::Shl | BinaryOp::Shr => {
                        let l = expected
                            .cloned()
                            .or_else(|| self.infer_emit_expr_type(left));
                        (l, None)
                    }
                };

                let wrap_left = needs_binary_child_parens(*op, &left.kind, true);
                if wrap_left {
                    buf.push('(');
                    self.write_expr_to(left, 0, left_exp_owned.as_ref(), buf);
                    buf.push(')');
                } else {
                    self.write_expr_to(left, prec, left_exp_owned.as_ref(), buf);
                }

                buf.push(' ');
                buf.push_str(binary_op_st(*op));
                buf.push(' ');

                let wrap_right = needs_binary_child_parens(*op, &right.kind, false);
                if wrap_right {
                    buf.push('(');
                    self.write_expr_to(right, 0, right_exp_owned.as_ref(), buf);
                    buf.push(')');
                } else {
                    self.write_expr_to(
                        right,
                        prec.saturating_add(1),
                        right_exp_owned.as_ref(),
                        buf,
                    );
                }
            }
            ExprKind::Foreign { language, text } => {
                let site = ForeignSite {
                    level: ForeignLevel::Expr,
                    node_id: None,
                    language,
                    fidelity: ForeignFidelity::F0,
                    text,
                    pack_hint: None,
                    span: expr.span,
                };
                if let Ok(decision) = check_foreign_export(&site, LanguageId::St, self.ctx) {
                    match decision {
                        ForeignExportDecision::VerbatimSameLanguage(raw) => {
                            let needs_group = parent_prec > 0 && raw.contains("**");
                            if needs_group {
                                buf.push('(');
                            }
                            buf.push_str(&raw);
                            if needs_group {
                                buf.push(')');
                            }
                        }
                        ForeignExportDecision::DemarcatedNotTranslated(rendered) => {
                            self.diagnostics.extend(rendered.diagnostics);
                            buf.push_str(&rendered.rendered);
                            buf.push_str(" 0");
                        }
                    }
                }
            }
        }

        if need_parens {
            buf.push(')');
        }
    }

    /// Lowers `ANALOG_TO_CODE(CODE_TO_ANALOG(expr, src_codec), dst_codec)` via
    /// [`ReducedRationalPlan::emit_st`] when `force_reduced_rationals` is active OR when
    /// `src_codec` belongs to a foreign board profile (cross-platform projection!).
    fn try_emit_reduced_rational_call(&mut self, expr: &Expr) -> Option<String> {
        let ExprKind::Call { callee, args } = &expr.kind else {
            return None;
        };
        if !callee.eq_ignore_ascii_case("ANALOG_TO_CODE") || args.len() != 2 {
            return None;
        }
        let inner = &args.first()?.value;
        let ExprKind::Literal(Literal::String(dst_id)) = &args.get(1)?.value.kind else {
            return None;
        };
        let dst_id = dst_id.as_str();
        let ExprKind::Call {
            callee: inner_callee,
            args: inner_args,
        } = &inner.kind
        else {
            return None;
        };
        if !inner_callee.eq_ignore_ascii_case("CODE_TO_ANALOG") || inner_args.len() != 2 {
            return None;
        }
        let src_expr = &inner_args.first()?.value;
        let ExprKind::Literal(Literal::String(src_id)) = &inner_args.get(1)?.value.kind else {
            return None;
        };
        let src_id = src_id.as_str();

        // Expand when explicitly requested OR when translating from a non-local board codec
        let is_cross_board = !self.ctx.codecs.contains_key(src_id);
        if !self.force_reduced_rationals && !is_cross_board {
            return None;
        }

        let src_codec = self.resolve_codec(src_id)?;
        let dst_codec = self.resolve_codec(dst_id)?;
        let plan = ReducedRationalPlan::compute(&src_codec, &dst_codec).ok()?;
        let inner_st = self.format_expr(src_expr, 0);
        Some(plan.emit_st(&inner_st))
    }

    fn resolve_codec(&self, id: &str) -> Option<Codec> {
        if let Some(c) = self.ctx.codec(id) {
            return Some(c.clone());
        }
        match id {
            "pwm_8bit" => Codec::new("pwm_8bit", CodecKind::Binned, 256, 255).ok(),
            "uno_10bit" => Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).ok(),
            "adc_12bit" => Codec::new("adc_12bit", CodecKind::Binned, 4096, 4095).ok(),
            "adc_16bit" => Codec::new("adc_16bit", CodecKind::Binned, 65_536, 65_535).ok(),
            "plc_span_27648" => Codec::new("plc_span_27648", CodecKind::Span, 27_648, 27_648).ok(),
            "plc_bipolar_27648" => {
                Codec::new("plc_bipolar_27648", CodecKind::SpanBipolar, 27_648, 27_648).ok()
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Derived ST Emission for Visual POUs (`PouBody::Network`, `PouBody::Chart`)
    // -----------------------------------------------------------------------

    fn emit_derived_ladder(
        &mut self,
        net: &LadderNetwork,
        indent: usize,
        used_names: &mut BTreeSet<String>,
    ) -> Vec<String> {
        let mut rungs: Vec<&dendri_ast::LadderRung> = net.rungs.iter().collect();
        rungs.sort_by_key(|r| r.order);
        if rungs.is_empty() {
            return Vec::new();
        }
        let mut rail_temps = Vec::new();
        let rung_rail = alloc_ladder_temp("_rail", used_names, &mut rail_temps);

        for rung in rungs {
            self.emit_leading_comments(&rung.id, indent);
            self.push_indent(indent);
            self.out.push_str(&rung_rail);
            self.out.push_str(" := TRUE;\n");
            self.last_line_was_line_comment = false;

            let mut sorted_cells: Vec<&LadderCell> = rung.cells.iter().collect();
            sorted_cells.sort_by_key(|c| (c.col, c.row));
            self.emit_ladder_series(
                &sorted_cells,
                &rung_rail,
                indent,
                used_names,
                &mut rail_temps,
            );
        }
        rail_temps
    }

    fn emit_ladder_series(
        &mut self,
        cells: &[&LadderCell],
        rail_var: &str,
        indent: usize,
        used_names: &mut BTreeSet<String>,
        rail_temps: &mut Vec<String>,
    ) {
        for cell in cells {
            self.emit_leading_comments(&cell.id, indent);
            match &cell.kind {
                LadderCellKind::Empty => {
                    self.push_indent(indent);
                    self.out.push_str(&format!("{rail_var} := FALSE;\n"));
                    self.last_line_was_line_comment = false;
                }
                LadderCellKind::Wire => {}
                LadderCellKind::Contact { kind, target } => {
                    let mut lv = String::new();
                    self.write_lvalue_to(target, &mut lv);
                    let cond = match kind {
                        LadderContactKind::NormallyOpen
                        | LadderContactKind::PositiveTransition
                        | LadderContactKind::NegativeTransition => lv,
                        LadderContactKind::NormallyClosed => format!("(NOT {lv})"),
                    };
                    self.push_indent(indent);
                    self.out
                        .push_str(&format!("{rail_var} := {rail_var} AND {cond};\n"));
                    self.last_line_was_line_comment = false;
                }
                LadderCellKind::ParallelOr { branches } => {
                    if branches.is_empty() {
                        self.push_indent(indent);
                        self.out.push_str(&format!("{rail_var} := FALSE;\n"));
                        self.last_line_was_line_comment = false;
                        continue;
                    }
                    let saved_in = alloc_ladder_temp("_rail_in", used_names, rail_temps);
                    let or_acc = alloc_ladder_temp("_rail_or", used_names, rail_temps);
                    let br_rail = alloc_ladder_temp("_rail_br", used_names, rail_temps);

                    self.push_indent(indent);
                    self.out.push_str(&format!("{saved_in} := {rail_var};\n"));
                    self.push_indent(indent);
                    self.out.push_str(&format!("{or_acc} := FALSE;\n"));
                    self.last_line_was_line_comment = false;

                    for br in branches {
                        let mut sorted_br: Vec<&LadderCell> = br.iter().collect();
                        sorted_br.sort_by_key(|c| (c.col, c.row));
                        self.push_indent(indent);
                        self.out.push_str(&format!("{br_rail} := {saved_in};\n"));
                        self.last_line_was_line_comment = false;

                        self.emit_ladder_series(
                            &sorted_br, &br_rail, indent, used_names, rail_temps,
                        );

                        self.push_indent(indent);
                        self.out
                            .push_str(&format!("{or_acc} := {or_acc} OR {br_rail};\n"));
                        self.last_line_was_line_comment = false;
                    }

                    self.push_indent(indent);
                    self.out.push_str(&format!("{rail_var} := {or_acc};\n"));
                    self.last_line_was_line_comment = false;
                }
                LadderCellKind::Coil { kind, target } => {
                    let mut lv = String::new();
                    self.write_lvalue_to(target, &mut lv);
                    self.push_indent(indent);
                    match kind {
                        LadderCoilKind::Normal
                        | LadderCoilKind::PositiveTransition
                        | LadderCoilKind::NegativeTransition => {
                            self.out.push_str(&format!("{lv} := {rail_var};\n"));
                        }
                        LadderCoilKind::Negated => {
                            self.out.push_str(&format!("{lv} := NOT ({rail_var});\n"));
                        }
                        LadderCoilKind::Set => {
                            self.out
                                .push_str(&format!("IF {rail_var} THEN {lv} := TRUE; END_IF;\n"));
                        }
                        LadderCoilKind::Reset => {
                            self.out
                                .push_str(&format!("IF {rail_var} THEN {lv} := FALSE; END_IF;\n"));
                        }
                    }
                    self.last_line_was_line_comment = false;
                }
                LadderCellKind::Box(box_kind) => match box_kind {
                    LadderBoxKind::Fb {
                        kind,
                        instance,
                        inputs,
                    } => {
                        let (rail_in, rail_out) = ladder_fb_rail_pins(kind);
                        let has_rail_in =
                            inputs.iter().any(|(k, _)| k.eq_ignore_ascii_case(rail_in));
                        let inst_ty =
                            self.resolve_lvalue_type(instance)
                                .unwrap_or_else(|| TypeRef::Fb {
                                    fb_kind: kind.clone(),
                                });
                        let mut inst = String::new();
                        self.write_lvalue_to(instance, &mut inst);
                        self.push_indent(indent);
                        self.out.push_str(&inst);
                        self.out.push('(');
                        let mut wrote_any = false;
                        if !has_rail_in {
                            self.out.push_str(rail_in);
                            self.out.push_str(" := ");
                            self.out.push_str(rail_var);
                            wrote_any = true;
                        }
                        for (k, v) in inputs {
                            if wrote_any {
                                self.out.push_str(", ");
                            }
                            self.out.push_str(k);
                            self.out.push_str(" := ");
                            let exp = self.type_env.fb_field_type(&inst_ty, k);
                            let vs = self.format_expr_typed(v, 0, exp.as_ref());
                            self.out.push_str(&vs);
                            wrote_any = true;
                        }
                        self.out.push_str(");\n");
                        self.push_indent(indent);
                        self.out
                            .push_str(&format!("{rail_var} := {inst}.{rail_out};\n"));
                        self.last_line_was_line_comment = false;
                    }
                    LadderBoxKind::Compare { op, left, right } => {
                        let cmp_ty = self
                            .infer_emit_expr_type(left)
                            .or_else(|| self.infer_emit_expr_type(right));
                        let ls = self.format_expr_typed(left, 0, cmp_ty.as_ref());
                        let rs = self.format_expr_typed(right, 0, cmp_ty.as_ref());
                        let sym = match op {
                            LadderCompareOp::Eq => "=",
                            LadderCompareOp::Ne => "<>",
                            LadderCompareOp::Lt => "<",
                            LadderCompareOp::Le => "<=",
                            LadderCompareOp::Gt => ">",
                            LadderCompareOp::Ge => ">=",
                        };
                        self.push_indent(indent);
                        self.out.push_str(&format!(
                            "{rail_var} := {rail_var} AND ({ls} {sym} {rs});\n"
                        ));
                        self.last_line_was_line_comment = false;
                    }
                    LadderBoxKind::Call {
                        callee,
                        args,
                        output,
                    } => {
                        let call_args: Vec<CallArg> = args
                            .iter()
                            .map(|(name, e)| CallArg {
                                name: name.clone(),
                                value: e.clone(),
                                span: e.span,
                            })
                            .collect();
                        let out_ty = output.as_ref().and_then(|lv| self.resolve_lvalue_type(lv));
                        let arg_tys =
                            self.call_arg_expected_types(callee, &call_args, out_ty.as_ref());
                        let mut call_s = format!("{callee}(");
                        for (i, (name, e)) in args.iter().enumerate() {
                            if i > 0 {
                                call_s.push_str(", ");
                            }
                            if let Some(n) = name {
                                call_s.push_str(n);
                                call_s.push_str(" := ");
                            }
                            let exp = arg_tys.get(i).and_then(|t| t.as_ref());
                            call_s.push_str(&self.format_expr_typed(e, 0, exp));
                        }
                        call_s.push(')');
                        self.push_indent(indent);
                        if let Some(out_lv) = output {
                            let mut olv = String::new();
                            self.write_lvalue_to(out_lv, &mut olv);
                            self.out.push_str(&format!(
                                "IF {rail_var} THEN {olv} := {call_s}; END_IF;\n"
                            ));
                        } else {
                            self.out
                                .push_str(&format!("IF {rail_var} THEN {call_s}; END_IF;\n"));
                        }
                        self.last_line_was_line_comment = false;
                    }
                    LadderBoxKind::Foreign {
                        language,
                        fidelity,
                        text,
                        pack_hint,
                    } => {
                        let site = ForeignSite {
                            level: ForeignLevel::Stmt,
                            node_id: Some(&cell.id),
                            language,
                            fidelity: *fidelity,
                            text,
                            pack_hint: pack_hint.as_deref(),
                            span: Span::default(),
                        };
                        if let Ok(decision) = check_foreign_export(&site, LanguageId::St, self.ctx)
                        {
                            match decision {
                                ForeignExportDecision::VerbatimSameLanguage(raw) => {
                                    self.push_indent(indent);
                                    self.out.push_str(&raw);
                                    self.out.push('\n');
                                    self.last_line_was_line_comment = false;
                                }
                                ForeignExportDecision::DemarcatedNotTranslated(rendered) => {
                                    self.diagnostics.extend(rendered.diagnostics.clone());
                                    self.push_rendered_comment_lines(&rendered, indent);
                                    self.out.push('\n');
                                    self.last_line_was_line_comment = matches!(
                                        rendered.emitted_style,
                                        CommentStyle::Line | CommentStyle::DocLine
                                    );
                                }
                            }
                        }
                    }
                },
            }
        }
    }

    fn emit_derived_chart(&mut self, chart: &SfcChart, indent: usize) {
        for step in &chart.steps {
            self.emit_leading_comments(&step.id, indent);
            for action in &step.actions {
                self.emit_leading_comments(&action.id, indent);
                self.push_indent(indent);
                self.out.push_str(&format!("IF {}.X THEN\n", step.name));
                self.last_line_was_line_comment = false;
                if let Some(target) = &action.target {
                    let mut lv = String::new();
                    self.write_lvalue_to(target, &mut lv);
                    self.push_indent(indent.saturating_add(1));
                    let val = match action.qualifier {
                        dendri_ast::ActionQualifier::R => "FALSE",
                        dendri_ast::ActionQualifier::N
                        | dendri_ast::ActionQualifier::S
                        | dendri_ast::ActionQualifier::L
                        | dendri_ast::ActionQualifier::D
                        | dendri_ast::ActionQualifier::P
                        | dendri_ast::ActionQualifier::P1
                        | dendri_ast::ActionQualifier::P0 => "TRUE",
                    };
                    self.out.push_str(&format!("{lv} := {val};\n"));
                }
                self.emit_stmts(&action.body, indent.saturating_add(1), None, false);
                self.push_indent(indent);
                self.out.push_str("END_IF;\n");
                self.last_line_was_line_comment = false;
            }
        }
    }
}

fn alloc_ladder_temp(
    base: &str,
    used_names: &mut BTreeSet<String>,
    rail_temps: &mut Vec<String>,
) -> String {
    let base_upper = base.to_ascii_uppercase();
    let name = if used_names.insert(base_upper) {
        base.to_owned()
    } else {
        let mut idx = 1u32;
        loop {
            let candidate = format!("{base}_{idx}");
            if used_names.insert(candidate.to_ascii_uppercase()) {
                break candidate;
            }
            idx = idx.saturating_add(1);
        }
    };
    rail_temps.push(name.clone());
    name
}

const fn ladder_fb_rail_pins(kind: &FbKind) -> (&'static str, &'static str) {
    match kind {
        FbKind::Ton | FbKind::Tof | FbKind::Tp => ("IN", "Q"),
        FbKind::Ctu => ("CU", "Q"),
        FbKind::Ctd => ("CD", "Q"),
        FbKind::Ctud => ("CU", "QU"),
        FbKind::RTrig | FbKind::FTrig => ("CLK", "Q"),
        FbKind::Sr => ("S1", "Q1"),
        FbKind::Rs => ("S", "Q1"),
        FbKind::UartSend | FbKind::I2cXfer | FbKind::SpiXfer | FbKind::MbClient => ("REQ", "DONE"),
        FbKind::UartRecv => ("EN_R", "NDR"),
        FbKind::MbServer => ("ENABLE", "NDR"),
        FbKind::User { .. } => ("EN", "ENO"),
    }
}

fn eval_const_int_expr(expr: &Expr) -> Option<i128> {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(n)) => Some(*n),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => eval_const_int_expr(operand).and_then(i128::checked_neg),
        ExprKind::Unary {
            op: UnaryOp::Pos,
            operand,
        } => eval_const_int_expr(operand),
        ExprKind::Literal(_)
        | ExprKind::Ident(_)
        | ExprKind::Index { .. }
        | ExprKind::Field { .. }
        | ExprKind::Call { .. }
        | ExprKind::Unary { .. }
        | ExprKind::Binary { .. }
        | ExprKind::Foreign { .. } => None,
    }
}

const fn var_block_header(scope: StorageScope, is_global: bool) -> &'static str {
    match (is_global, scope) {
        (true, StorageScope::Var) => "VAR_GLOBAL",
        (true, StorageScope::Const) => "VAR_GLOBAL CONSTANT",
        (true, StorageScope::Retain) => "VAR_GLOBAL RETAIN",
        (true, StorageScope::Persistent) => "VAR_GLOBAL PERSISTENT",
        (true, StorageScope::Temp) => "VAR_TEMP",
        (false, StorageScope::Var) => "VAR",
        (false, StorageScope::Const) => "VAR CONSTANT",
        (false, StorageScope::Retain) => "VAR RETAIN",
        (false, StorageScope::Persistent) => "VAR PERSISTENT",
        (false, StorageScope::Temp) => "VAR_TEMP",
    }
}

const fn hardware_emit_st_type(kind: HardwareKind) -> &'static str {
    match kind {
        HardwareKind::DigitalIn | HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional => {
            "BOOL"
        }
        HardwareKind::AnalogIn | HardwareKind::AnalogOut => "INT",
        HardwareKind::PwmOut => "DUTY",
        HardwareKind::BusPin => "BYTE",
    }
}

fn write_type_ref(ty: &TypeRef, buf: &mut String) {
    match ty {
        TypeRef::Void => buf.push_str("VOID"),
        TypeRef::Scalar { ty } => write_scalar_type(ty, buf),
        TypeRef::Array { elem, len } => {
            buf.push_str("ARRAY[0..");
            buf.push_str(&len.saturating_sub(1).to_string());
            buf.push_str("] OF ");
            write_type_ref(elem, buf);
        }
        TypeRef::Named { name } => buf.push_str(name),
        TypeRef::Fb { fb_kind } => {
            buf.push_str(
                &TypeRef::Fb {
                    fb_kind: fb_kind.clone(),
                }
                .to_string(),
            );
        }
    }
}

fn write_scalar_type(ty: &ScalarType, buf: &mut String) {
    match ty {
        ScalarType::Bool => buf.push_str("BOOL"),
        ScalarType::Sint => buf.push_str("SINT"),
        ScalarType::Int => buf.push_str("INT"),
        ScalarType::Dint => buf.push_str("DINT"),
        ScalarType::Lint => buf.push_str("LINT"),
        ScalarType::Usint => buf.push_str("USINT"),
        ScalarType::Uint => buf.push_str("UINT"),
        ScalarType::Udint => buf.push_str("UDINT"),
        ScalarType::Ulint => buf.push_str("ULINT"),
        ScalarType::Byte => buf.push_str("BYTE"),
        ScalarType::Word => buf.push_str("WORD"),
        ScalarType::Dword => buf.push_str("DWORD"),
        ScalarType::Lword => buf.push_str("LWORD"),
        ScalarType::Real => buf.push_str("REAL"),
        ScalarType::Lreal => buf.push_str("LREAL"),
        ScalarType::Time => buf.push_str("TIME"),
        ScalarType::String { max_len } => {
            if *max_len == 255 {
                buf.push_str("STRING");
            } else {
                buf.push_str("STRING[");
                buf.push_str(&max_len.to_string());
                buf.push(']');
            }
        }
        ScalarType::Analog => buf.push_str("ANALOG"),
        ScalarType::Duty => buf.push_str("DUTY"),
        ScalarType::Code { codec_id } => {
            buf.push_str("CODE<");
            buf.push_str(codec_id);
            buf.push('>');
        }
    }
}

fn write_literal(lit: &Literal, expected: Option<&TypeRef>, buf: &mut String) {
    match lit {
        Literal::Bool(true) => buf.push_str("TRUE"),
        Literal::Bool(false) => buf.push_str("FALSE"),
        Literal::Int(n) => {
            if *n < 0 {
                // Typed negative literal preserves exact `Literal::Int(neg)` across parse(emit(m))
                // while matching the constrained target scalar type (SINT/INT/DINT/LINT/ANALOG).
                if let Some(TypeRef::Scalar { ty: scalar }) = expected
                    && (scalar.is_signed_int() || matches!(scalar, ScalarType::Analog))
                    && fits_scalar_int_range(scalar, *n)
                {
                    write_scalar_type(scalar, buf);
                    buf.push('#');
                } else if (i128::from(i32::MIN)..=i128::from(i32::MAX)).contains(n) {
                    buf.push_str("DINT#");
                } else {
                    buf.push_str("LINT#");
                }
            }
            buf.push_str(&n.to_string());
        }
        Literal::Float(f) => {
            let neg = f.is_sign_negative() && *f != 0.0;
            if neg {
                if let Some(TypeRef::Scalar {
                    ty: scalar @ (ScalarType::Real | ScalarType::Lreal),
                }) = expected
                {
                    write_scalar_type(scalar, buf);
                    buf.push('#');
                } else {
                    buf.push_str("LREAL#");
                }
            }
            let s = format!("{f}");
            buf.push_str(&s);
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                buf.push_str(".0");
            }
        }
        Literal::TimeUs(us) => write_iec_time_us(*us, buf),
        Literal::String(s) => write_iec_string(s, buf),
    }
}

fn write_iec_time_us(us: i64, buf: &mut String) {
    buf.push_str("T#");
    if us == 0 {
        buf.push_str("0us");
        return;
    }
    if us < 0 {
        buf.push('-');
    }
    let mut rem = us.unsigned_abs();
    let units: &[(u64, &str)] = &[
        (86_400_000_000, "d"),
        (3_600_000_000, "h"),
        (60_000_000, "m"),
        (1_000_000, "s"),
        (1_000, "ms"),
        (1, "us"),
    ];
    for &(div, suffix) in units {
        let q = rem / div;
        if q > 0 {
            buf.push_str(&q.to_string());
            buf.push_str(suffix);
            rem %= div;
        }
    }
}

fn write_iec_string(s: &str, buf: &mut String) {
    buf.push('\'');
    for ch in s.chars() {
        match ch {
            '$' => buf.push_str("$$"),
            '\'' => buf.push_str("$'"),
            '\n' => buf.push_str("$L"),
            '\r' => buf.push_str("$R"),
            '\t' => buf.push_str("$T"),
            '\x0C' => buf.push_str("$P"),
            c if c.is_control() => {
                let mut utf8_buf = [0u8; 4];
                for &b in c.encode_utf8(&mut utf8_buf).as_bytes() {
                    buf.push_str(&format!("${b:02X}"));
                }
            }
            c => buf.push(c),
        }
    }
    buf.push('\'');
}

fn needs_binary_child_parens(parent: BinaryOp, child: &ExprKind, is_left: bool) -> bool {
    let ExprKind::Binary { op: child_op, .. } = child else {
        return false;
    };
    if parent.is_relational() && child_op.is_relational() {
        return true;
    }
    if parent.is_equality() && child_op.is_equality() {
        return true;
    }
    if !is_left && binary_prec(*child_op) <= binary_prec(parent) {
        return true;
    }
    false
}

const fn expr_prec(kind: &ExprKind) -> u8 {
    match kind {
        ExprKind::Literal(_)
        | ExprKind::Ident(_)
        | ExprKind::Call { .. }
        | ExprKind::Foreign { .. } => 120,
        ExprKind::Index { .. } | ExprKind::Field { .. } => 110,
        ExprKind::Unary { .. } => 100,
        ExprKind::Binary { op, .. } => binary_prec(*op),
    }
}

const fn binary_prec(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 10,
        BinaryOp::Xor => 20,
        BinaryOp::And => 30,
        BinaryOp::BitAnd | BinaryOp::BitXor | BinaryOp::BitOr => 40,
        BinaryOp::Eq | BinaryOp::Ne => 50,
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 60,
        BinaryOp::Shl | BinaryOp::Shr => 70,
        BinaryOp::Add | BinaryOp::Sub => 80,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 90,
    }
}

const fn binary_op_st(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "MOD",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Shl => "SHL",
        BinaryOp::Shr => "SHR",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Eq => "=",
        BinaryOp::Ne => "<>",
        BinaryOp::BitAnd => "BIT_AND",
        BinaryOp::BitXor => "BIT_XOR",
        BinaryOp::BitOr => "BIT_OR",
        BinaryOp::And => "AND",
        BinaryOp::Xor => "XOR",
        BinaryOp::Or => "OR",
    }
}
