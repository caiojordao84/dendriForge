//! Canonical lossless textual emitter for IEC 60848 / IEC 61131-3 GRAFCET / SFC
//! (`docs/lbc/sfc.md` §1.1, §4, §6, §7).
//!
//! Guarantees:
//! - `LOSSLESS_ASL`: `parse(emit(bundle, ctx), ctx) == bundle` (modulo spans & canonical `NodeId`s).
//! - Zero banned machine magic comments (ADR-020).
//! - Zero `M = 10^9` literals (`04` §2 #3).
//! - Cross-language `Foreign` export guard (`ERR_FOREIGN_EXPORT_BLOCKED` unless `allow_foreign_export`).
//! - Verbatim `C1–C5` human comment conservation with `INFO_COMMENT_RELOCATED` and EOL restoration.

use crate::comments::render_sfc_textual_comment;
use crate::validate::validate_bundle_for_emit;
use dendri_ast::{
    ActionQualifier, AuthoringLanguage, BinaryOp, BoardProgram, CasePattern, ChannelRange, Comment,
    CommentAnchor, CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, Expr, ExprKind,
    FbKind, ForeignFidelity, HardwareBinding, HardwareKind, InterruptEdge, LValue, Literal,
    ModuleDecl, NodeId, ObKind, ParamDecl, ParamDirection, Pou, PouBody, PouKind, PullMode,
    ScalarType, SfcAction, SfcChart, SfcStep, SfcTransition, SignalKind, Stmt, StmtKind,
    StorageScope, TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::comments::{relocate_inner_comments, restore_eol};
use dendri_frontend_core::cursor::{is_ident_continue, is_ident_start};
use dendri_frontend_core::foreign::{
    ForeignExportDecision, ForeignLevel, ForeignSite, check_foreign_export,
    check_program_foreign_export,
};
use dendri_frontend_core::traits::{
    BoardContext, BoardProgramBundle, CommentPlacement, FrontendOutput, FrontendResult, LanguageId,
};
use dendri_span::Diagnostic;
use std::collections::{BTreeMap, BTreeSet};

/// Emits canonical IEC 60848 / IEC 61131-3 textual SFC from a [`BoardProgramBundle`].
pub(crate) fn emit_sfc(bundle: &BoardProgramBundle, ctx: &BoardContext) -> FrontendResult<String> {
    // 1. Validate unique NodeIds and comment invariants (`ERR_DUPLICATE_ID`, `ERR_COMMENT_FIELD`, `ERR_COMMENT_ANCHOR`)
    validate_bundle_for_emit(&bundle.program, &bundle.comments, ctx)?;

    // 2. Enforce cross-language Foreign export guard (`ERR_FOREIGN_EXPORT_BLOCKED` unless `allow_foreign_export`)
    check_program_foreign_export(&bundle.program, LanguageId::Sfc, ctx)?;

    // 3. Relocate any `Inner` comments to `Leading` (`INFO_COMMENT_RELOCATED`)
    let (projected_comments, mut diagnostics) =
        relocate_inner_comments(&bundle.comments, LanguageId::Sfc, Some(&ctx.board_id));

    let mut emitter = Emitter::new(&bundle.program, &projected_comments, ctx, &mut diagnostics);
    emitter.emit_program(&bundle.program)?;

    let text = restore_eol(&emitter.out, bundle.presentation.eol_style);
    Ok(FrontendOutput::with_diagnostics(text, diagnostics))
}

struct Emitter<'a> {
    ctx: &'a BoardContext,
    diagnostics: &'a mut Vec<Diagnostic>,
    leading_by_node: BTreeMap<NodeId, Vec<&'a Comment>>,
    trailing_by_node: BTreeMap<NodeId, Vec<&'a Comment>>,
    dangling_by_node: BTreeMap<NodeId, Vec<&'a Comment>>,
    file_header_comments: Vec<&'a Comment>,
    fb_vars: BTreeSet<String>,
    out: String,
    indent: usize,
    last_emitted_was_line_comment: bool,
}

impl<'a> Emitter<'a> {
    fn new(
        program: &BoardProgram,
        comments: &'a [Comment],
        ctx: &'a BoardContext,
        diagnostics: &'a mut Vec<Diagnostic>,
    ) -> Self {
        let mut leading_by_node: BTreeMap<NodeId, Vec<&'a Comment>> = BTreeMap::new();
        let mut trailing_by_node: BTreeMap<NodeId, Vec<&'a Comment>> = BTreeMap::new();
        let mut dangling_by_node: BTreeMap<NodeId, Vec<&'a Comment>> = BTreeMap::new();
        let mut file_header_comments = Vec::new();

        for c in comments {
            match &c.anchor {
                CommentAnchor::Node { id } => match c.placement {
                    AstCommentPlacement::Trailing => {
                        trailing_by_node.entry(id.clone()).or_default().push(c);
                    }
                    AstCommentPlacement::Dangling => {
                        dangling_by_node.entry(id.clone()).or_default().push(c);
                    }
                    AstCommentPlacement::Leading
                    | AstCommentPlacement::Header
                    | AstCommentPlacement::Title
                    | AstCommentPlacement::Body
                    | AstCommentPlacement::Inner => {
                        leading_by_node.entry(id.clone()).or_default().push(c);
                    }
                },
                CommentAnchor::Project
                | CommentAnchor::Board { .. }
                | CommentAnchor::Component { .. }
                | CommentAnchor::Wire { .. } => {
                    file_header_comments.push(c);
                }
            }
        }

        let mut fb_vars = BTreeSet::new();
        for d in &program.data {
            if matches!(d.ty, TypeRef::Fb { .. }) {
                fb_vars.insert(d.name.clone());
            }
        }
        for m in &program.modules {
            for d in &m.data {
                if matches!(d.ty, TypeRef::Fb { .. }) {
                    fb_vars.insert(d.name.clone());
                }
            }
        }

        Self {
            ctx,
            diagnostics,
            leading_by_node,
            trailing_by_node,
            dangling_by_node,
            file_header_comments,
            fb_vars,
            out: String::new(),
            indent: 0,
            last_emitted_was_line_comment: false,
        }
    }

    fn push_indent(&mut self) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
    }

    fn emit_block_comments(&mut self, comments: &[&Comment]) {
        for c in comments {
            let placement = CommentPlacement::from_comment(c).unwrap_or(CommentPlacement::Leading);
            let rendered = render_sfc_textual_comment(&c.text, c.origin.style, placement, self.ctx);
            self.diagnostics.extend(rendered.diagnostics);

            // Prevent `merge_contiguous_leading_comments` from merging two distinct Line comments
            if rendered.emitted_style == CommentStyle::Line && self.last_emitted_was_line_comment {
                self.out.push('\n');
            }

            for (idx, line) in rendered.rendered.split('\n').enumerate() {
                if idx > 0 {
                    self.out.push('\n');
                }
                self.push_indent();
                self.out.push_str(line);
            }
            self.out.push('\n');
            self.last_emitted_was_line_comment = rendered.emitted_style == CommentStyle::Line;
        }
    }

    fn emit_leading_comments(&mut self, id: &NodeId) {
        let mut items: Vec<&Comment> = self.leading_by_node.get(id).cloned().unwrap_or_default();

        // Any multiline trailing comment that cannot fit cleanly on one line moves before the node
        // so C1 verbatim text multiset stays strictly invariant.
        if let Some(trailings) = self.trailing_by_node.get(id) {
            for c in trailings {
                if c.text.contains('\n') {
                    items.push(c);
                }
            }
        }

        if !items.is_empty() {
            self.emit_block_comments(&items);
        }
    }

    fn emit_trailing_comments_inline(&mut self, id: &NodeId) {
        let Some(trailings) = self.trailing_by_node.get(id).cloned() else {
            return;
        };
        let single_line: Vec<&Comment> = trailings
            .into_iter()
            .filter(|c| !c.text.contains('\n'))
            .collect();
        for (idx, c) in single_line.iter().enumerate() {
            let is_last = idx.saturating_add(1) == single_line.len();
            let preferred_style = if is_last {
                c.origin.style
            } else {
                CommentStyle::ParenBlock
            };
            let rendered = render_sfc_textual_comment(
                &c.text,
                preferred_style,
                CommentPlacement::Trailing,
                self.ctx,
            );
            self.diagnostics.extend(rendered.diagnostics);
            self.out.push(' ');
            self.out.push_str(&rendered.rendered);
        }
    }

    fn emit_dangling_comments(&mut self, id: &NodeId) {
        if let Some(danglings) = self.dangling_by_node.get(id).cloned() {
            self.emit_block_comments(&danglings);
        }
    }

    fn emit_program(&mut self, program: &BoardProgram) -> Result<(), Vec<Diagnostic>> {
        if !self.file_header_comments.is_empty() {
            let headers = self.file_header_comments.clone();
            self.emit_block_comments(&headers);
            self.out.push('\n');
            self.last_emitted_was_line_comment = false;
        }

        // 1. Imports
        for imp in &program.imports {
            self.out.push_str("IMPORT ");
            self.out.push_str(imp);
            self.out.push_str(";\n");
            self.last_emitted_was_line_comment = false;
        }
        if !program.imports.is_empty() {
            self.out.push('\n');
        }

        // 2. Types (Struct, Enum, Decl::Foreign)
        self.emit_types(&program.types)?;

        // 3. Hardware I/O bindings (inside VAR_GLOBAL, zero banned magic comments!)
        if !program.hardware.is_empty() {
            self.push_indent();
            self.out.push_str("VAR_GLOBAL\n");
            self.last_emitted_was_line_comment = false;
            self.indent = self.indent.saturating_add(1);
            for hw in &program.hardware {
                self.emit_hardware_binding(hw);
            }
            self.indent = self.indent.saturating_sub(1);
            self.push_indent();
            self.out.push_str("END_VAR\n\n");
            self.last_emitted_was_line_comment = false;
        }

        // 4. Global Data declarations (grouped by contiguous StorageScope to preserve order)
        self.emit_global_data(&program.data)?;

        // 5. Embedded modules
        for m in &program.modules {
            self.emit_module(m)?;
        }

        // 6. POUs
        for (idx, pou) in program.pous.iter().enumerate() {
            if idx > 0 {
                self.out.push('\n');
            }
            self.emit_pou(pou)?;
        }

        Ok(())
    }

    fn emit_types(&mut self, types: &[TypeDecl]) -> Result<(), Vec<Diagnostic>> {
        if types.is_empty() {
            return Ok(());
        }
        let mut in_type_block = false;
        for td in types {
            match td {
                TypeDecl::Struct {
                    id, name, fields, ..
                } => {
                    if !in_type_block {
                        self.push_indent();
                        self.out.push_str("TYPE\n");
                        self.indent = self.indent.saturating_add(1);
                        in_type_block = true;
                        self.last_emitted_was_line_comment = false;
                    }
                    self.emit_leading_comments(id);
                    self.push_indent();
                    self.out.push_str(name);
                    self.out.push_str(" : STRUCT\n");
                    self.last_emitted_was_line_comment = false;
                    self.indent = self.indent.saturating_add(1);
                    for f in fields {
                        self.push_indent();
                        self.out.push_str(&f.name);
                        self.out.push_str(" : ");
                        write_type_ref(&f.ty, &mut self.out);
                        self.out.push_str(";\n");
                    }
                    self.indent = self.indent.saturating_sub(1);
                    self.push_indent();
                    self.out.push_str("END_STRUCT;");
                    self.emit_trailing_comments_inline(id);
                    self.out.push('\n');
                }
                TypeDecl::Enum {
                    id,
                    name,
                    backing,
                    variants,
                    ..
                } => {
                    if !in_type_block {
                        self.push_indent();
                        self.out.push_str("TYPE\n");
                        self.indent = self.indent.saturating_add(1);
                        in_type_block = true;
                        self.last_emitted_was_line_comment = false;
                    }
                    self.emit_leading_comments(id);
                    self.push_indent();
                    self.out.push_str(name);
                    self.out.push_str(" : (");
                    for (idx, v) in variants.iter().enumerate() {
                        if idx > 0 {
                            self.out.push_str(", ");
                        }
                        self.out.push_str(&v.name);
                        self.out.push_str(" := ");
                        self.out.push_str(&v.value.to_string());
                    }
                    self.out.push_str(") : ");
                    write_type_ref(&TypeRef::scalar(backing.clone()), &mut self.out);
                    self.out.push(';');
                    self.emit_trailing_comments_inline(id);
                    self.out.push('\n');
                    self.last_emitted_was_line_comment = false;
                }
                TypeDecl::Foreign {
                    id,
                    name,
                    language,
                    fidelity,
                    text,
                    pack_hint,
                    span,
                } => {
                    if in_type_block {
                        self.indent = self.indent.saturating_sub(1);
                        self.push_indent();
                        self.out.push_str("END_TYPE\n\n");
                        in_type_block = false;
                    }
                    self.emit_leading_comments(id);
                    let site = ForeignSite {
                        level: ForeignLevel::Decl,
                        node_id: Some(id),
                        language,
                        fidelity: *fidelity,
                        text,
                        pack_hint: pack_hint.as_deref(),
                        span: *span,
                    };
                    match check_foreign_export(&site, LanguageId::Sfc, self.ctx) {
                        Ok(ForeignExportDecision::VerbatimSameLanguage(_)) => {
                            self.push_indent();
                            self.out.push_str("FOREIGN_TYPE ");
                            if let Some(n) = name {
                                self.out.push_str("NAME ");
                                self.out.push_str(n);
                                self.out.push(' ');
                            }
                            if let Some(p) = pack_hint {
                                self.out.push_str("PACK ");
                                write_escaped_string(p, &mut self.out);
                                self.out.push(' ');
                            }
                            self.out.push_str(fidelity_str(*fidelity));
                            self.out.push('(');
                            write_escaped_string(language, &mut self.out);
                            self.out.push_str(", ");
                            write_escaped_string(text, &mut self.out);
                            self.out.push_str(");");
                            self.emit_trailing_comments_inline(id);
                            self.out.push('\n');
                            self.last_emitted_was_line_comment = false;
                        }
                        Ok(ForeignExportDecision::DemarcatedNotTranslated(rendered)) => {
                            self.diagnostics.extend(rendered.diagnostics);
                            let wrapped = render_sfc_textual_comment(
                                &rendered.rendered,
                                CommentStyle::ParenBlock,
                                CommentPlacement::Leading,
                                self.ctx,
                            );
                            self.diagnostics.extend(wrapped.diagnostics);
                            for line in wrapped.rendered.split('\n') {
                                self.push_indent();
                                self.out.push_str(line);
                                self.out.push('\n');
                            }
                            self.last_emitted_was_line_comment =
                                wrapped.emitted_style == CommentStyle::Line;
                        }
                        Err(diag) => return Err(vec![diag]),
                    }
                }
            }
        }
        if in_type_block {
            self.indent = self.indent.saturating_sub(1);
            self.push_indent();
            self.out.push_str("END_TYPE\n\n");
            self.last_emitted_was_line_comment = false;
        }
        Ok(())
    }

    fn emit_hardware_binding(&mut self, hw: &HardwareBinding) {
        self.emit_leading_comments(&hw.id);
        self.push_indent();
        self.out.push_str(&hw.label);
        self.out.push_str(" AT ");

        let (addr_str, inferred_kind) = if let Some(iec) = self.ctx.iec_for_port(&hw.pin) {
            let inferred = if iec.starts_with("%IX") {
                Some(HardwareKind::DigitalIn)
            } else if iec.starts_with("%QX") {
                Some(HardwareKind::DigitalOut)
            } else if iec.starts_with("%IW") || iec.starts_with("%ID") {
                Some(HardwareKind::AnalogIn)
            } else if iec.starts_with("%QW") || iec.starts_with("%QD") {
                Some(HardwareKind::AnalogOut)
            } else {
                None
            };
            (iec.to_owned(), inferred)
        } else if is_plain_ident(&hw.pin) {
            (hw.pin.clone(), None)
        } else {
            let mut quoted = String::new();
            write_escaped_string(&hw.pin, &mut quoted);
            (quoted, None)
        };

        self.out.push_str(&addr_str);
        self.out.push_str(" : ");

        let (ty_str, ty_inferred_kind, codec_encoded_in_type) = match hw.kind {
            HardwareKind::DigitalIn
            | HardwareKind::DigitalOut
            | HardwareKind::DigitalBidirectional
            | HardwareKind::BusPin => ("BOOL".to_owned(), HardwareKind::DigitalIn, false),
            HardwareKind::AnalogIn | HardwareKind::AnalogOut => match &hw.codec {
                Some(c) => (format!("CODE<{c}>"), HardwareKind::AnalogIn, true),
                None => ("ANALOG".to_owned(), HardwareKind::AnalogIn, false),
            },
            HardwareKind::PwmOut => ("DUTY".to_owned(), HardwareKind::PwmOut, false),
        };
        self.out.push_str(&ty_str);

        let effective_inferred = inferred_kind.unwrap_or(ty_inferred_kind);
        let mut attrs = Vec::new();
        if hw.kind != effective_inferred {
            attrs.push(hw_kind_str(hw.kind).to_owned());
        }
        match hw.pull {
            PullMode::None => {}
            PullMode::PullUp => attrs.push("PULL := PULL_UP".to_owned()),
            PullMode::PullDown => attrs.push("PULL := PULL_DOWN".to_owned()),
        }
        if hw.active_low {
            attrs.push("ACTIVE_LOW := TRUE".to_owned());
        }
        if !codec_encoded_in_type && let Some(c) = &hw.codec {
            let mut s = String::from("CODEC := ");
            write_escaped_string(c, &mut s);
            attrs.push(s);
        }
        if let Some(r) = &hw.range {
            attrs.push(format!("RANGE := {}", format_channel_range(r)));
        }

        if !attrs.is_empty() {
            self.out.push_str(" (");
            self.out.push_str(&attrs.join(", "));
            self.out.push(')');
        }

        self.out.push(';');
        self.emit_trailing_comments_inline(&hw.id);
        self.out.push('\n');
        self.last_emitted_was_line_comment = false;
    }

    fn emit_global_data(&mut self, data: &[DataDecl]) -> Result<(), Vec<Diagnostic>> {
        let mut idx = 0usize;
        while let Some(first) = data.get(idx) {
            let scope = first.scope;
            self.push_indent();
            self.out.push_str("VAR_GLOBAL");
            match scope {
                StorageScope::Var | StorageScope::Temp => {}
                StorageScope::Const => self.out.push_str(" CONSTANT"),
                StorageScope::Retain => self.out.push_str(" RETAIN"),
                StorageScope::Persistent => self.out.push_str(" PERSISTENT"),
            }
            self.out.push('\n');
            self.last_emitted_was_line_comment = false;
            self.indent = self.indent.saturating_add(1);
            while let Some(d) = data.get(idx) {
                if d.scope != scope {
                    break;
                }
                self.emit_data_decl(d)?;
                idx = idx.saturating_add(1);
            }
            self.indent = self.indent.saturating_sub(1);
            self.push_indent();
            self.out.push_str("END_VAR\n\n");
            self.last_emitted_was_line_comment = false;
        }
        Ok(())
    }

    fn emit_data_decl(&mut self, d: &DataDecl) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&d.id);
        self.push_indent();
        self.out.push_str(&d.name);
        self.out.push_str(" : ");
        write_type_ref(&d.ty, &mut self.out);
        if let Some(init) = &d.initial {
            self.out.push_str(" := ");
            self.write_expr(init, 0, Some(&d.id))?;
        }
        self.out.push(';');
        self.emit_trailing_comments_inline(&d.id);
        self.out.push('\n');
        self.last_emitted_was_line_comment = false;
        Ok(())
    }

    fn emit_pou_var_blocks(
        &mut self,
        params: &[ParamDecl],
        locals: &[DataDecl],
    ) -> Result<(), Vec<Diagnostic>> {
        let mut pidx = 0usize;
        while let Some(first) = params.get(pidx) {
            let dir = first.direction;
            self.push_indent();
            match dir {
                ParamDirection::In => self.out.push_str("VAR_INPUT\n"),
                ParamDirection::Out => self.out.push_str("VAR_OUTPUT\n"),
                ParamDirection::InOut => self.out.push_str("VAR_IN_OUT\n"),
            }
            self.last_emitted_was_line_comment = false;
            self.indent = self.indent.saturating_add(1);
            while let Some(p) = params.get(pidx) {
                if p.direction != dir {
                    break;
                }
                self.push_indent();
                self.out.push_str(&p.name);
                self.out.push_str(" : ");
                write_type_ref(&p.ty, &mut self.out);
                self.out.push_str(";\n");
                pidx = pidx.saturating_add(1);
            }
            self.indent = self.indent.saturating_sub(1);
            self.push_indent();
            self.out.push_str("END_VAR\n");
            self.last_emitted_was_line_comment = false;
        }

        let mut lidx = 0usize;
        while let Some(first) = locals.get(lidx) {
            let scope = first.scope;
            self.push_indent();
            match scope {
                StorageScope::Temp => self.out.push_str("VAR_TEMP\n"),
                StorageScope::Var => self.out.push_str("VAR\n"),
                StorageScope::Const => self.out.push_str("VAR CONSTANT\n"),
                StorageScope::Retain => self.out.push_str("VAR RETAIN\n"),
                StorageScope::Persistent => self.out.push_str("VAR PERSISTENT\n"),
            }
            self.last_emitted_was_line_comment = false;
            self.indent = self.indent.saturating_add(1);
            while let Some(d) = locals.get(lidx) {
                if d.scope != scope {
                    break;
                }
                self.emit_data_decl(d)?;
                lidx = lidx.saturating_add(1);
            }
            self.indent = self.indent.saturating_sub(1);
            self.push_indent();
            self.out.push_str("END_VAR\n");
            self.last_emitted_was_line_comment = false;
        }
        Ok(())
    }

    fn emit_module(&mut self, m: &ModuleDecl) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&m.id);
        self.push_indent();
        self.out.push_str("MODULE ");
        self.out.push_str(&m.name);
        self.emit_trailing_comments_inline(&m.id);
        self.out.push('\n');
        self.last_emitted_was_line_comment = false;

        self.indent = self.indent.saturating_add(1);
        for imp in &m.imports {
            self.push_indent();
            self.out.push_str("IMPORT ");
            self.out.push_str(imp);
            self.out.push_str(";\n");
        }
        for exp in &m.exports {
            self.push_indent();
            self.out.push_str("EXPORT ");
            self.out.push_str(exp);
            self.out.push_str(";\n");
        }
        self.emit_types(&m.types)?;
        self.emit_global_data(&m.data)?;
        for pou in &m.pous {
            self.emit_pou(pou)?;
        }
        self.emit_dangling_comments(&m.id);
        self.indent = self.indent.saturating_sub(1);

        self.push_indent();
        self.out.push_str("END_MODULE\n\n");
        self.last_emitted_was_line_comment = false;
        Ok(())
    }

    fn emit_pou(&mut self, pou: &Pou) -> Result<(), Vec<Diagnostic>> {
        let saved_fb_vars = self.fb_vars.clone();
        for p in &pou.params {
            if matches!(p.ty, TypeRef::Fb { .. }) {
                self.fb_vars.insert(p.name.clone());
            }
        }
        for l in &pou.locals {
            if matches!(l.ty, TypeRef::Fb { .. }) {
                self.fb_vars.insert(l.name.clone());
            }
        }

        let res = match &pou.body {
            PouBody::Chart(chart) => self.emit_sfc_chart_pou(pou, chart),
            PouBody::Statements(stmts) => self.emit_stmt_pou(pou, stmts),
            PouBody::Network(_) => {
                // Non-SFC Ladder POUs are outside SFC paradigm; emit empty chart stub only if ever passed
                let mut d = Diagnostic::new(
                    dendri_span::ErrorCode::ErrUnsupportedCapability,
                    format!(
                        "POU `{}` has a Ladder Diagram (`PouBody::Network`) body; use `dendri-lang-ladder` to emit Ladder POUs",
                        pou.name
                    ),
                )
                .with_board_id(&self.ctx.board_id);
                d.span = Some(pou.span);
                Err(vec![d])
            }
        };

        self.fb_vars = saved_fb_vars;
        res
    }

    fn emit_sfc_chart_pou(&mut self, pou: &Pou, chart: &SfcChart) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&pou.id);
        self.push_indent();
        self.out.push_str("SFC_CHART ");
        self.out.push_str(&pou.name);

        let mut opts = Vec::new();
        if chart.evolve_to_stability {
            opts.push("STABILITY := TRUE".to_owned());
        }
        if !matches!(pou.kind, PouKind::Ob { ob: ObKind::Main }) {
            opts.push(format!("KIND := {}", format_pou_kind(&pou.kind)));
        }
        if let Some(view) = pou.preferred_view {
            opts.push(format!("VIEW := {}", format_authoring_language(view)));
        }
        if !opts.is_empty() {
            self.out.push_str(" (");
            self.out.push_str(&opts.join(", "));
            self.out.push(')');
        }
        self.emit_trailing_comments_inline(&pou.id);
        self.out.push('\n');
        self.last_emitted_was_line_comment = false;

        self.indent = self.indent.saturating_add(1);
        self.emit_pou_var_blocks(&pou.params, &pou.locals)?;

        for step in &chart.steps {
            self.emit_sfc_step(step)?;
        }
        for tr in &chart.transitions {
            self.emit_sfc_transition(tr)?;
        }
        self.emit_dangling_comments(&pou.id);
        self.indent = self.indent.saturating_sub(1);

        self.push_indent();
        self.out.push_str("END_SFC_CHART\n");
        self.last_emitted_was_line_comment = false;
        Ok(())
    }

    fn emit_sfc_step(&mut self, step: &SfcStep) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&step.id);
        self.push_indent();
        if step.initial {
            self.out.push_str("INITIAL_STEP ");
        } else {
            self.out.push_str("STEP ");
        }
        self.out.push_str(&step.name);
        if let Some(sub) = &step.macro_chart {
            self.out.push_str(" MACRO ");
            self.out.push_str(sub);
        }
        self.out.push(':');
        self.emit_trailing_comments_inline(&step.id);
        self.out.push('\n');
        self.last_emitted_was_line_comment = false;

        self.indent = self.indent.saturating_add(1);
        for action in &step.actions {
            self.emit_sfc_action(action)?;
        }
        self.emit_dangling_comments(&step.id);
        self.indent = self.indent.saturating_sub(1);

        self.push_indent();
        self.out.push_str("END_STEP\n");
        self.last_emitted_was_line_comment = false;
        Ok(())
    }

    fn emit_sfc_action(&mut self, action: &SfcAction) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&action.id);
        self.push_indent();
        self.out.push_str(qualifier_str(action.qualifier));
        if let Some(dur) = &action.duration {
            self.out.push('(');
            self.write_expr(dur, 0, Some(&action.id))?;
            self.out.push(')');
        }
        self.out.push_str(": ");

        match (&action.target, action.body.is_empty()) {
            (Some(target), true) => {
                self.write_lvalue(target, Some(&action.id))?;
                self.out.push(';');
                self.emit_trailing_comments_inline(&action.id);
                self.out.push('\n');
            }
            (Some(target), false) => {
                self.write_lvalue(target, Some(&action.id))?;
                self.out.push_str(" {\n");
                self.last_emitted_was_line_comment = false;
                self.indent = self.indent.saturating_add(1);
                for st in &action.body {
                    self.emit_stmt(st)?;
                }
                self.emit_dangling_comments(&action.id);
                self.indent = self.indent.saturating_sub(1);
                self.push_indent();
                self.out.push('}');
                self.emit_trailing_comments_inline(&action.id);
                self.out.push('\n');
            }
            (None, _) => {
                self.out.push_str("{\n");
                self.last_emitted_was_line_comment = false;
                self.indent = self.indent.saturating_add(1);
                for st in &action.body {
                    self.emit_stmt(st)?;
                }
                self.emit_dangling_comments(&action.id);
                self.indent = self.indent.saturating_sub(1);
                self.push_indent();
                self.out.push('}');
                self.emit_trailing_comments_inline(&action.id);
                self.out.push('\n');
            }
        }
        self.last_emitted_was_line_comment = false;
        Ok(())
    }

    fn emit_sfc_transition(&mut self, tr: &SfcTransition) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&tr.id);
        self.push_indent();
        self.out.push_str("TRANSITION ");
        self.out.push_str(&tr.name);
        if tr.priority != 0 {
            self.out
                .push_str(&format!(" (PRIORITY := {})", tr.priority));
        }
        self.out.push_str(" FROM ");
        write_step_list(&tr.from_steps, &mut self.out);
        self.out.push_str(" TO ");
        write_step_list(&tr.to_steps, &mut self.out);
        self.out.push_str(" := ");
        self.write_expr(&tr.receptivity, 0, Some(&tr.id))?;
        self.out.push_str(";\n");
        self.last_emitted_was_line_comment = false;
        self.emit_dangling_comments(&tr.id);
        self.push_indent();
        self.out.push_str("END_TRANSITION");
        self.emit_trailing_comments_inline(&tr.id);
        self.out.push('\n');
        self.last_emitted_was_line_comment = false;
        Ok(())
    }

    fn emit_stmt_pou(&mut self, pou: &Pou, stmts: &[Stmt]) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&pou.id);
        self.push_indent();
        let (open_kw, close_kw) = match &pou.kind {
            PouKind::Ob { .. } => ("PROGRAM", "END_PROGRAM"),
            PouKind::Fb => ("FUNCTION_BLOCK", "END_FUNCTION_BLOCK"),
            PouKind::Fc { .. } => ("FUNCTION", "END_FUNCTION"),
        };
        self.out.push_str(open_kw);
        self.out.push(' ');
        self.out.push_str(&pou.name);

        let mut opts = Vec::new();
        if let PouKind::Ob { ob } = &pou.kind
            && !matches!(ob, ObKind::Main)
        {
            opts.push(format!("KIND := {}", format_pou_kind(&pou.kind)));
        }
        if let Some(view) = pou.preferred_view {
            opts.push(format!("VIEW := {}", format_authoring_language(view)));
        }
        if !opts.is_empty() {
            self.out.push_str(" (");
            self.out.push_str(&opts.join(", "));
            self.out.push(')');
        }
        if let PouKind::Fc { return_type } = &pou.kind {
            self.out.push_str(" : ");
            write_type_ref(return_type, &mut self.out);
        }
        self.emit_trailing_comments_inline(&pou.id);
        self.out.push('\n');
        self.last_emitted_was_line_comment = false;

        self.indent = self.indent.saturating_add(1);
        self.emit_pou_var_blocks(&pou.params, &pou.locals)?;
        for st in stmts {
            self.emit_stmt(st)?;
        }
        self.emit_dangling_comments(&pou.id);
        self.indent = self.indent.saturating_sub(1);

        self.push_indent();
        self.out.push_str(close_kw);
        self.out.push('\n');
        self.last_emitted_was_line_comment = false;
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&stmt.id);
        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                self.push_indent();
                self.write_lvalue(target, Some(&stmt.id))?;
                self.out.push_str(" := ");
                self.write_expr(value, 0, Some(&stmt.id))?;
                self.out.push(';');
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::Call { callee, args } => {
                self.push_indent();
                self.out.push_str(callee);
                self.out.push('(');
                for (idx, arg) in args.iter().enumerate() {
                    if idx > 0 {
                        self.out.push_str(", ");
                    }
                    if let Some(name) = &arg.name {
                        self.out.push_str(name);
                        self.out.push_str(" := ");
                    }
                    self.write_expr(&arg.value, 0, Some(&stmt.id))?;
                }
                self.out.push_str(");");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::FbInvoke { instance, inputs } => {
                self.push_indent();
                let known_fb = match instance {
                    LValue::Var { name, .. } => self.fb_vars.contains(name),
                    LValue::Index { .. } | LValue::Field { .. } => true,
                };
                if !known_fb {
                    self.out.push_str("INVOKE ");
                }
                self.write_lvalue(instance, Some(&stmt.id))?;
                self.out.push('(');
                for (idx, (k, v)) in inputs.iter().enumerate() {
                    if idx > 0 {
                        self.out.push_str(", ");
                    }
                    self.out.push_str(k);
                    self.out.push_str(" := ");
                    self.write_expr(v, 0, Some(&stmt.id))?;
                }
                self.out.push_str(");");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                self.push_indent();
                self.out.push_str("IF ");
                self.write_expr(condition, 0, Some(&stmt.id))?;
                self.out.push_str(" THEN\n");
                self.last_emitted_was_line_comment = false;
                self.indent = self.indent.saturating_add(1);
                for s in then_body {
                    self.emit_stmt(s)?;
                }
                self.indent = self.indent.saturating_sub(1);
                for arm in elif_arms {
                    self.push_indent();
                    self.out.push_str("ELSIF ");
                    self.write_expr(&arm.condition, 0, Some(&stmt.id))?;
                    self.out.push_str(" THEN\n");
                    self.last_emitted_was_line_comment = false;
                    self.indent = self.indent.saturating_add(1);
                    for s in &arm.body {
                        self.emit_stmt(s)?;
                    }
                    self.indent = self.indent.saturating_sub(1);
                }
                if !else_body.is_empty() {
                    self.push_indent();
                    self.out.push_str("ELSE\n");
                    self.last_emitted_was_line_comment = false;
                    self.indent = self.indent.saturating_add(1);
                    for s in else_body {
                        self.emit_stmt(s)?;
                    }
                    self.indent = self.indent.saturating_sub(1);
                }
                self.push_indent();
                self.out.push_str("END_IF;");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                self.push_indent();
                self.out.push_str("CASE ");
                self.write_expr(scrutinee, 0, Some(&stmt.id))?;
                self.out.push_str(" OF\n");
                self.last_emitted_was_line_comment = false;
                self.indent = self.indent.saturating_add(1);
                for c in cases {
                    self.push_indent();
                    for (idx, pat) in c.patterns.iter().enumerate() {
                        if idx > 0 {
                            self.out.push_str(", ");
                        }
                        match pat {
                            CasePattern::Value(e) => self.write_expr(e, 0, Some(&stmt.id))?,
                            CasePattern::Range { start, end } => {
                                self.write_expr(start, 0, Some(&stmt.id))?;
                                self.out.push_str("..");
                                self.write_expr(end, 0, Some(&stmt.id))?;
                            }
                        }
                    }
                    self.out.push_str(":\n");
                    self.indent = self.indent.saturating_add(1);
                    for s in &c.body {
                        self.emit_stmt(s)?;
                    }
                    self.indent = self.indent.saturating_sub(1);
                }
                if let Some(def) = default {
                    self.push_indent();
                    self.out.push_str("ELSE\n");
                    self.indent = self.indent.saturating_add(1);
                    for s in def {
                        self.emit_stmt(s)?;
                    }
                    self.indent = self.indent.saturating_sub(1);
                }
                self.indent = self.indent.saturating_sub(1);
                self.push_indent();
                self.out.push_str("END_CASE;");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::While { condition, body } => {
                self.push_indent();
                self.out.push_str("WHILE ");
                self.write_expr(condition, 0, Some(&stmt.id))?;
                self.out.push_str(" DO\n");
                self.last_emitted_was_line_comment = false;
                self.indent = self.indent.saturating_add(1);
                for s in body {
                    self.emit_stmt(s)?;
                }
                self.indent = self.indent.saturating_sub(1);
                self.push_indent();
                self.out.push_str("END_WHILE;");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            } => {
                self.push_indent();
                self.out.push_str("FOR ");
                self.out.push_str(counter);
                self.out.push_str(" := ");
                self.write_expr(from, 0, Some(&stmt.id))?;
                self.out
                    .push_str(if *inclusive { " TO " } else { " UNTIL " });
                self.write_expr(to, 0, Some(&stmt.id))?;
                if !matches!(step.kind, ExprKind::Literal(Literal::Int(1))) {
                    self.out.push_str(" BY ");
                    self.write_expr(step, 0, Some(&stmt.id))?;
                }
                self.out.push_str(" DO\n");
                self.last_emitted_was_line_comment = false;
                self.indent = self.indent.saturating_add(1);
                for s in body {
                    self.emit_stmt(s)?;
                }
                self.indent = self.indent.saturating_sub(1);
                self.push_indent();
                self.out.push_str("END_FOR;");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::Break => {
                self.push_indent();
                self.out.push_str("EXIT;");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::Continue => {
                self.push_indent();
                self.out.push_str("CONTINUE;");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::Return { value } => {
                self.push_indent();
                self.out.push_str("RETURN");
                if let Some(v) = value {
                    self.out.push(' ');
                    self.write_expr(v, 0, Some(&stmt.id))?;
                }
                self.out.push(';');
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::Wait { duration } => {
                self.push_indent();
                self.out.push_str("WAIT(");
                self.write_expr(duration, 0, Some(&stmt.id))?;
                self.out.push_str(");");
                self.emit_trailing_comments_inline(&stmt.id);
                self.out.push('\n');
            }
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => {
                let site = ForeignSite {
                    level: ForeignLevel::Stmt,
                    node_id: Some(&stmt.id),
                    language,
                    fidelity: *fidelity,
                    text,
                    pack_hint: pack_hint.as_deref(),
                    span: stmt.span,
                };
                match check_foreign_export(&site, LanguageId::Sfc, self.ctx) {
                    Ok(ForeignExportDecision::VerbatimSameLanguage(_)) => {
                        self.push_indent();
                        self.out.push_str("FOREIGN_STMT(");
                        self.out.push_str(fidelity_str(*fidelity));
                        self.out.push_str(", ");
                        write_escaped_string(language, &mut self.out);
                        self.out.push_str(", ");
                        write_escaped_string(text, &mut self.out);
                        if let Some(p) = pack_hint {
                            self.out.push_str(", PACK := ");
                            write_escaped_string(p, &mut self.out);
                        }
                        self.out.push_str(");");
                        self.emit_trailing_comments_inline(&stmt.id);
                        self.out.push('\n');
                    }
                    Ok(ForeignExportDecision::DemarcatedNotTranslated(rendered)) => {
                        self.diagnostics.extend(rendered.diagnostics);
                        let wrapped = render_sfc_textual_comment(
                            &rendered.rendered,
                            CommentStyle::ParenBlock,
                            CommentPlacement::Leading,
                            self.ctx,
                        );
                        self.diagnostics.extend(wrapped.diagnostics);
                        for line in wrapped.rendered.split('\n') {
                            self.push_indent();
                            self.out.push_str(line);
                            self.out.push('\n');
                        }
                    }
                    Err(diag) => return Err(vec![diag]),
                }
            }
        }
        self.last_emitted_was_line_comment = false;
        Ok(())
    }

    fn write_lvalue(
        &mut self,
        lvalue: &LValue,
        enclosing: Option<&NodeId>,
    ) -> Result<(), Vec<Diagnostic>> {
        match lvalue {
            LValue::Var { name, .. } => {
                self.out.push_str(name);
                Ok(())
            }
            LValue::Index { base, index, .. } => {
                self.write_lvalue(base, enclosing)?;
                self.out.push('[');
                self.write_expr(index, 0, enclosing)?;
                self.out.push(']');
                Ok(())
            }
            LValue::Field { base, field, .. } => {
                self.write_lvalue(base, enclosing)?;
                self.out.push('.');
                self.out.push_str(field);
                Ok(())
            }
        }
    }

    fn write_expr(
        &mut self,
        expr: &Expr,
        parent_prec: u8,
        enclosing: Option<&NodeId>,
    ) -> Result<(), Vec<Diagnostic>> {
        let my_prec = expr_prec(&expr.kind);
        let need_parens = my_prec < parent_prec;
        if need_parens {
            self.out.push('(');
        }

        match &expr.kind {
            ExprKind::Literal(lit) => write_literal(lit, &mut self.out),
            ExprKind::Ident(name) => self.out.push_str(name),
            ExprKind::Index { base, index } => {
                let wrap_base = expr_prec(&base.kind) < 110;
                if wrap_base {
                    self.out.push('(');
                }
                self.write_expr(base, 0, enclosing)?;
                if wrap_base {
                    self.out.push(')');
                }
                self.out.push('[');
                self.write_expr(index, 0, enclosing)?;
                self.out.push(']');
            }
            ExprKind::Field { base, field } => {
                let wrap_base = expr_prec(&base.kind) < 110;
                if wrap_base {
                    self.out.push('(');
                }
                self.write_expr(base, 0, enclosing)?;
                if wrap_base {
                    self.out.push(')');
                }
                self.out.push('.');
                self.out.push_str(field);
            }
            ExprKind::Call { callee, args } => {
                self.out.push_str(callee);
                self.out.push('(');
                for (idx, arg) in args.iter().enumerate() {
                    if idx > 0 {
                        self.out.push_str(", ");
                    }
                    if let Some(name) = &arg.name {
                        self.out.push_str(name);
                        self.out.push_str(" := ");
                    }
                    self.write_expr(&arg.value, 0, enclosing)?;
                }
                self.out.push(')');
            }
            ExprKind::Unary { op, operand } => {
                match op {
                    UnaryOp::Neg => self.out.push('-'),
                    UnaryOp::Pos => self.out.push('+'),
                    UnaryOp::Not => self.out.push_str("NOT "),
                    UnaryOp::BitNot => self.out.push_str("BIT_NOT "),
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
                    self.out.push('(');
                    self.write_expr(operand, 0, enclosing)?;
                    self.out.push(')');
                } else {
                    self.write_expr(operand, 100, enclosing)?;
                }
            }
            ExprKind::Binary { op, left, right } => {
                let prec = binary_prec(*op);
                let wrap_left = needs_binary_child_parens(*op, &left.kind, true);
                if wrap_left {
                    self.out.push('(');
                    self.write_expr(left, 0, enclosing)?;
                    self.out.push(')');
                } else {
                    self.write_expr(left, prec, enclosing)?;
                }
                self.out.push(' ');
                self.out.push_str(binary_op_str(*op));
                self.out.push(' ');
                let wrap_right = needs_binary_child_parens(*op, &right.kind, false);
                if wrap_right {
                    self.out.push('(');
                    self.write_expr(right, 0, enclosing)?;
                    self.out.push(')');
                } else {
                    self.write_expr(right, prec.saturating_add(1), enclosing)?;
                }
            }
            ExprKind::Foreign { language, text } => {
                let site = ForeignSite {
                    level: ForeignLevel::Expr,
                    node_id: enclosing,
                    language,
                    fidelity: ForeignFidelity::F0,
                    text,
                    pack_hint: None,
                    span: expr.span,
                };
                match check_foreign_export(&site, LanguageId::Sfc, self.ctx) {
                    Ok(ForeignExportDecision::VerbatimSameLanguage(_)) => {
                        self.out.push_str("FOREIGN(");
                        write_escaped_string(language, &mut self.out);
                        self.out.push_str(", ");
                        write_escaped_string(text, &mut self.out);
                        self.out.push(')');
                    }
                    Ok(ForeignExportDecision::DemarcatedNotTranslated(rendered)) => {
                        self.diagnostics.extend(rendered.diagnostics);
                        let wrapped = render_sfc_textual_comment(
                            &rendered.rendered,
                            CommentStyle::ParenBlock,
                            CommentPlacement::Inner { token_ordinal: 0 },
                            self.ctx,
                        );
                        self.diagnostics.extend(wrapped.diagnostics);
                        self.out.push_str(&wrapped.rendered);
                        self.out.push_str(" FALSE");
                    }
                    Err(diag) => return Err(vec![diag]),
                }
            }
        }

        if need_parens {
            self.out.push(')');
        }
        Ok(())
    }
}

fn write_step_list(steps: &[String], out: &mut String) {
    if steps.len() == 1
        && let Some(single) = steps.first()
    {
        out.push_str(single);
        return;
    }
    out.push('(');
    for (idx, s) in steps.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(s);
    }
    out.push(')');
}

fn write_type_ref(ty: &TypeRef, out: &mut String) {
    match ty {
        TypeRef::Void => out.push_str("VOID"),
        TypeRef::Scalar { ty: s } => match s {
            ScalarType::Bool => out.push_str("BOOL"),
            ScalarType::Sint => out.push_str("SINT"),
            ScalarType::Int => out.push_str("INT"),
            ScalarType::Dint => out.push_str("DINT"),
            ScalarType::Lint => out.push_str("LINT"),
            ScalarType::Usint => out.push_str("USINT"),
            ScalarType::Uint => out.push_str("UINT"),
            ScalarType::Udint => out.push_str("UDINT"),
            ScalarType::Ulint => out.push_str("ULINT"),
            ScalarType::Byte => out.push_str("BYTE"),
            ScalarType::Word => out.push_str("WORD"),
            ScalarType::Dword => out.push_str("DWORD"),
            ScalarType::Lword => out.push_str("LWORD"),
            ScalarType::Real => out.push_str("REAL"),
            ScalarType::Lreal => out.push_str("LREAL"),
            ScalarType::Time => out.push_str("TIME"),
            ScalarType::String { max_len } => {
                out.push_str(&format!("STRING[{max_len}]"));
            }
            ScalarType::Analog => out.push_str("ANALOG"),
            ScalarType::Duty => out.push_str("DUTY"),
            ScalarType::Code { codec_id } => {
                out.push_str("CODE<");
                out.push_str(codec_id);
                out.push('>');
            }
        },
        TypeRef::Array { elem, len } => {
            out.push_str(&format!("ARRAY[{len}] OF "));
            write_type_ref(elem, out);
        }
        TypeRef::Named { name } => out.push_str(name),
        TypeRef::Fb { fb_kind } => match fb_kind {
            FbKind::Ton => out.push_str("TON"),
            FbKind::Tof => out.push_str("TOF"),
            FbKind::Tp => out.push_str("TP"),
            FbKind::Ctu => out.push_str("CTU"),
            FbKind::Ctd => out.push_str("CTD"),
            FbKind::Ctud => out.push_str("CTUD"),
            FbKind::RTrig => out.push_str("R_TRIG"),
            FbKind::FTrig => out.push_str("F_TRIG"),
            FbKind::Sr => out.push_str("SR"),
            FbKind::Rs => out.push_str("RS"),
            FbKind::UartSend => out.push_str("UART_SEND"),
            FbKind::UartRecv => out.push_str("UART_RECV"),
            FbKind::I2cXfer => out.push_str("I2C_XFER"),
            FbKind::SpiXfer => out.push_str("SPI_XFER"),
            FbKind::MbClient => out.push_str("MB_CLIENT"),
            FbKind::MbServer => out.push_str("MB_SERVER"),
            FbKind::User { fb_name } => {
                out.push_str("FB<");
                out.push_str(fb_name);
                out.push('>');
            }
        },
    }
}

fn write_literal(lit: &Literal, out: &mut String) {
    match lit {
        Literal::Bool(true) => out.push_str("TRUE"),
        Literal::Bool(false) => out.push_str("FALSE"),
        Literal::Int(n) => out.push_str(&n.to_string()),
        Literal::Float(f) => {
            let s = format!("{f}");
            out.push_str(&s);
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                out.push_str(".0");
            }
        }
        Literal::TimeUs(us) => {
            out.push_str("T#");
            if *us == 0 {
                out.push_str("0us");
            } else if *us % 1_000_000 == 0 {
                out.push_str(&(*us / 1_000_000).to_string());
                out.push('s');
            } else if *us % 1_000 == 0 {
                out.push_str(&(*us / 1_000).to_string());
                out.push_str("ms");
            } else {
                out.push_str(&us.to_string());
                out.push_str("us");
            }
        }
        Literal::String(s) => write_escaped_string(s, out),
    }
}

fn write_escaped_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn format_channel_range(r: &ChannelRange) -> String {
    let sig = match r.signal {
        SignalKind::Voltage => "VOLTAGE",
        SignalKind::Current => "CURRENT",
    };
    let mut s = format!("{sig}({} .. {}", format_f64(r.low), format_f64(r.high));
    if r.live_zero {
        s.push_str(", LIVE_ZERO");
    }
    if r.hardware_normalizes_live_zero {
        s.push_str(", HW_NORM");
    }
    s.push(')');
    s
}

fn format_f64(v: f64) -> String {
    let s = format!("{v}");
    if !s.contains('.') && !s.contains('e') && !s.contains('E') {
        format!("{s}.0")
    } else {
        s
    }
}

fn format_pou_kind(kind: &PouKind) -> String {
    match kind {
        PouKind::Ob { ob } => match ob {
            ObKind::Main => "MAIN".to_owned(),
            ObKind::Startup => "STARTUP".to_owned(),
            ObKind::CyclicInt { period_us } => format!("CYCLIC_INT({period_us})"),
            ObKind::HwInt {
                pin_label,
                edge,
                priority,
            } => format!("HW_INT({pin_label}, {}, {priority})", edge_str(*edge)),
            ObKind::CommEvent { bus_id } => {
                let mut q = String::new();
                write_escaped_string(bus_id, &mut q);
                format!("COMM_EVENT({q})")
            }
        },
        PouKind::Fc { return_type } => {
            let mut ty = String::new();
            write_type_ref(return_type, &mut ty);
            format!("FC({ty})")
        }
        PouKind::Fb => "FB".to_owned(),
    }
}

const fn format_authoring_language(lang: AuthoringLanguage) -> &'static str {
    match lang {
        AuthoringLanguage::Asl => "ASL",
        AuthoringLanguage::Cpp => "CPP",
        AuthoringLanguage::St => "ST",
        AuthoringLanguage::Ladder => "LADDER",
        AuthoringLanguage::Sfc => "SFC",
        AuthoringLanguage::Micropython => "MICROPYTHON",
        AuthoringLanguage::Rust => "RUST",
    }
}

const fn edge_str(edge: InterruptEdge) -> &'static str {
    match edge {
        InterruptEdge::Rising => "RISING",
        InterruptEdge::Falling => "FALLING",
        InterruptEdge::Change => "CHANGE",
        InterruptEdge::Low => "LOW",
        InterruptEdge::High => "HIGH",
    }
}

const fn hw_kind_str(kind: HardwareKind) -> &'static str {
    match kind {
        HardwareKind::DigitalIn => "DIGITAL_IN",
        HardwareKind::DigitalOut => "DIGITAL_OUT",
        HardwareKind::DigitalBidirectional => "DIGITAL_BIDIR",
        HardwareKind::AnalogIn => "ANALOG_IN",
        HardwareKind::AnalogOut => "ANALOG_OUT",
        HardwareKind::PwmOut => "PWM_OUT",
        HardwareKind::BusPin => "BUS_PIN",
    }
}

const fn qualifier_str(q: ActionQualifier) -> &'static str {
    match q {
        ActionQualifier::N => "N",
        ActionQualifier::S => "S",
        ActionQualifier::R => "R",
        ActionQualifier::L => "L",
        ActionQualifier::D => "D",
        ActionQualifier::P => "P",
        ActionQualifier::P1 => "P1",
        ActionQualifier::P0 => "P0",
    }
}

const fn fidelity_str(f: ForeignFidelity) -> &'static str {
    match f {
        ForeignFidelity::F0 => "F0",
        ForeignFidelity::F1 => "F1",
        ForeignFidelity::F2 => "F2",
    }
}

fn is_plain_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    is_ident_start(first) && chars.all(is_ident_continue)
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

const fn binary_op_str(op: BinaryOp) -> &'static str {
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
