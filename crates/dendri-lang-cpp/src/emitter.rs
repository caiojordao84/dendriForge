//! Canonical idiomatic Arduino C++ emitter (`docs/lbc/cpp.md` §1–§7, `04` §2–§4).
//!
//! Guarantees:
//! - ZERO banned machine magic comments (hardware & data reconcile purely from native C++).
//! - ZERO `ASL_ANALOG_FULL_SCALE` (`M = 10^9`) literals; cross-platform ADC/code conversions
//!   emit via [`ReducedRationalPlan::emit_cpp`].
//! - Complete ADR-027 verbatim comment conservation (`C1–C5`), hazard-safe rendering, inner
//!   comment relocation, and original EOL restoration (`LF` / `CRLF`).
//! - Cross-language `Foreign` export guard (`ERR_FOREIGN_EXPORT_BLOCKED` unless `allow_foreign_export`).

use crate::board_pins::{hardware_label_to_cpp_arg, is_implicit_pin_label, port_to_cpp_pin_expr};
use crate::types::{
    cast_target_type_from_callee, format_cpp_param, format_cpp_var_declarator, scalar_to_cpp_type,
    type_ref_to_cpp_type_name,
};
use dendri_ast::{
    ActionQualifier, BinaryOp, CallArg, CasePattern, Codec, CodecError, CodecKind, Comment,
    CommentAnchor, CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, Expr, ExprKind,
    ForeignFidelity, HardwareBinding, HardwareKind, InterruptEdge, LValue, LadderBoxKind,
    LadderCell, LadderCellKind, LadderCoilKind, LadderCompareOp, LadderContactKind, LadderNetwork,
    Literal, NodeId, ObKind, Pou, PouBody, PouKind, PullMode, SfcChart, Stmt, StmtKind,
    StorageScope, SwitchCase, TypeDecl, UnaryOp,
};
use dendri_frontend_core::comments::{
    relocate_inner_comments, render_comment_for_language, restore_eol,
};
use dendri_frontend_core::foreign::{
    ForeignExportDecision, ForeignLevel, ForeignSite, check_foreign_export,
    check_program_foreign_export,
};
use dendri_frontend_core::numeric_emit::ReducedRationalPlan;
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, CommentPlacement, FrontendOutput, FrontendResult, LanguageId,
};
use dendri_span::{Diagnostic, Span};
use std::collections::{BTreeMap, BTreeSet};

/// Emits a self-contained C++ reduced-rational conversion expression from `src` codec to `dst` codec
/// using [`ReducedRationalPlan::emit_cpp`], NEVER emitting `M = 10^9`.
pub fn emit_codec_conversion_cpp(
    src: &Codec,
    dst: &Codec,
    expr: &str,
) -> Result<String, CodecError> {
    let plan = ReducedRationalPlan::compute(src, dst)?;
    Ok(plan.emit_cpp(expr))
}

/// Emits a fixed-resolution board ADC read scaled from `native_bits` (e.g. `10` on Arduino Uno R3)
/// to `active_bits` via [`ReducedRationalPlan::emit_cpp`], returning plain `analogRead(pin)` when
/// `native_bits == active_bits`.
pub fn emit_fixed_adc_read_cpp(
    native_bits: u8,
    active_bits: u8,
    pin_expr: &str,
) -> Result<String, CodecError> {
    let raw_call = format!("analogRead({pin_expr})");
    if native_bits == active_bits {
        return Ok(raw_call);
    }
    let src_d = 1u64.checked_shl(u32::from(native_bits)).unwrap_or(u64::MAX);
    let dst_d = 1u64.checked_shl(u32::from(active_bits)).unwrap_or(u64::MAX);
    let src = Codec::new(
        format!("adc_{native_bits}bit"),
        CodecKind::Binned,
        src_d,
        src_d.saturating_sub(1),
    )?;
    let dst = Codec::new(
        format!("adc_{active_bits}bit"),
        CodecKind::Binned,
        dst_d,
        dst_d.saturating_sub(1),
    )?;
    emit_codec_conversion_cpp(&src, &dst, &raw_call)
}

/// Emits canonical idiomatic Arduino C++ from a [`BoardProgramBundle`].
pub(crate) fn emit_cpp(bundle: &BoardProgramBundle, ctx: &BoardContext) -> FrontendResult<String> {
    // 1. Enforce cross-language Foreign export gate (same-language `"cpp"` is NEVER blocked!)
    check_program_foreign_export(&bundle.program, LanguageId::Cpp, ctx)?;

    // 2. Relocate cross-language Inner comments (`INFO_COMMENT_RELOCATED`)
    let (projected_comments, reloc_diags) =
        relocate_inner_comments(&bundle.comments, LanguageId::Cpp, Some(&ctx.board_id));

    let mut emitter = Emitter::new(bundle, &projected_comments, ctx, reloc_diags);
    emitter.emit_program()?;

    let final_text = restore_eol(&emitter.out, bundle.presentation.eol_style);
    Ok(FrontendOutput::with_diagnostics(
        final_text,
        emitter.diagnostics,
    ))
}

#[derive(Default)]
struct NodeCommentSlots {
    leading: Vec<usize>,
    inner: Vec<usize>,
    trailing: Vec<usize>,
    dangling: Vec<usize>,
}

struct Emitter<'a> {
    bundle: &'a BoardProgramBundle,
    comments: &'a [Comment],
    emitted_comments: Vec<bool>,
    board_headers: Vec<usize>,
    node_comments: BTreeMap<NodeId, NodeCommentSlots>,
    ctx: &'a BoardContext,
    hw_pin_map: BTreeMap<String, String>,
    hw_codec_map: BTreeMap<String, String>,
    out: String,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Emitter<'a> {
    fn new(
        bundle: &'a BoardProgramBundle,
        comments: &'a [Comment],
        ctx: &'a BoardContext,
        initial_diags: Vec<Diagnostic>,
    ) -> Self {
        let mut board_headers = Vec::new();
        let mut node_comments: BTreeMap<NodeId, NodeCommentSlots> = BTreeMap::new();

        for (idx, c) in comments.iter().enumerate() {
            match &c.anchor {
                CommentAnchor::Node { id } => {
                    let slot = node_comments.entry(id.clone()).or_default();
                    match c.placement {
                        AstCommentPlacement::Leading
                        | AstCommentPlacement::Header
                        | AstCommentPlacement::Title
                        | AstCommentPlacement::Body => slot.leading.push(idx),
                        AstCommentPlacement::Inner => slot.inner.push(idx),
                        AstCommentPlacement::Trailing => slot.trailing.push(idx),
                        AstCommentPlacement::Dangling => slot.dangling.push(idx),
                    }
                }
                CommentAnchor::Project
                | CommentAnchor::Board { .. }
                | CommentAnchor::Component { .. }
                | CommentAnchor::Wire { .. } => {
                    if c.placement == AstCommentPlacement::Header {
                        board_headers.push(idx);
                    }
                }
            }
        }

        let mut hw_pin_map = BTreeMap::new();
        let mut hw_codec_map = BTreeMap::new();
        for hw in &bundle.program.hardware {
            hw_pin_map.insert(hw.label.clone(), hw.pin.clone());
            if let Some(cid) = &hw.codec {
                hw_codec_map.insert(hw.label.clone(), cid.clone());
            }
        }

        Self {
            bundle,
            comments,
            emitted_comments: vec![false; comments.len()],
            board_headers,
            node_comments,
            ctx,
            hw_pin_map,
            hw_codec_map,
            out: String::new(),
            diagnostics: initial_diags,
        }
    }

    fn push_indent(&mut self, indent: usize) {
        for _ in 0..indent {
            self.out.push_str("  ");
        }
    }

    fn render_one_comment(
        &mut self,
        idx: usize,
        placement: CommentPlacement,
    ) -> Option<(String, CommentStyle)> {
        let c = self.comments.get(idx)?;
        if let Some(slot) = self.emitted_comments.get_mut(idx) {
            *slot = true;
        }
        let rendered = render_comment_for_language(
            LanguageId::Cpp,
            &c.text,
            c.origin.style,
            placement,
            self.ctx,
        );
        self.diagnostics.extend(rendered.diagnostics);
        Some((rendered.rendered, rendered.emitted_style))
    }

    /// Emits a list of standalone comments (leading/header/dangling), inserting blank lines
    /// between separate records whenever consecutive line comments would otherwise merge on re-parse!
    fn emit_comment_list(&mut self, indices: &[usize], indent: usize, placement: CommentPlacement) {
        let mut prev_was_line_style = false;
        for (pos, &idx) in indices.iter().enumerate() {
            let Some((rendered, emitted_style)) = self.render_one_comment(idx, placement) else {
                continue;
            };
            let is_line_style = matches!(emitted_style, CommentStyle::Line | CommentStyle::DocLine);
            if pos > 0 && (prev_was_line_style || is_line_style) {
                self.out.push('\n');
            }
            for (line_idx, line) in rendered.split('\n').enumerate() {
                // Only indent the opening line of a `/* ... */` / `/** ... */` block comment;
                // indenting continuation lines would mutate verbatim inner whitespace (ADR-027 C1).
                if line_idx == 0 || is_line_style {
                    self.push_indent(indent);
                }
                self.out.push_str(line);
                self.out.push('\n');
            }
            prev_was_line_style = is_line_style;
        }
    }

    fn emit_leading_comments(&mut self, node_id: &NodeId, indent: usize) {
        let indices = self
            .node_comments
            .get(node_id)
            .map(|s| s.leading.clone())
            .unwrap_or_default();
        if !indices.is_empty() {
            self.emit_comment_list(&indices, indent, CommentPlacement::Leading);
        }
    }

    fn emit_dangling_comments(&mut self, node_id: &NodeId, indent: usize) {
        let indices = self
            .node_comments
            .get(node_id)
            .map(|s| s.dangling.clone())
            .unwrap_or_default();
        if !indices.is_empty() {
            self.emit_comment_list(&indices, indent, CommentPlacement::Dangling);
        }
    }

    fn emit_inner_comments_inline(&mut self, node_id: &NodeId) {
        let indices = self
            .node_comments
            .get(node_id)
            .map(|s| s.inner.clone())
            .unwrap_or_default();
        for idx in indices {
            let ord = self
                .comments
                .get(idx)
                .and_then(|c| c.token_ordinal)
                .unwrap_or(0);
            if let Some((rendered, _)) =
                self.render_one_comment(idx, CommentPlacement::Inner { token_ordinal: ord })
            {
                self.out.push(' ');
                self.out.push_str(&rendered);
            }
        }
    }

    fn emit_trailing_comments(&mut self, node_id: &NodeId, indent: usize) {
        let indices = self
            .node_comments
            .get(node_id)
            .map(|s| s.trailing.clone())
            .unwrap_or_default();
        let mut on_fresh_line = false;
        let mut prev_was_line_style = false;
        for (i, idx) in indices.into_iter().enumerate() {
            let is_multiline = self
                .comments
                .get(idx)
                .is_some_and(|c| c.text.contains('\n'));
            if let Some((rendered, style)) =
                self.render_one_comment(idx, CommentPlacement::Trailing)
            {
                let is_line_style = matches!(style, CommentStyle::Line | CommentStyle::DocLine);
                if i == 0 && (!is_multiline || !is_line_style) {
                    self.out.push(' ');
                    self.out.push_str(&rendered);
                    on_fresh_line = false;
                } else {
                    if !on_fresh_line {
                        self.out.push('\n');
                    }
                    if i > 0 && (prev_was_line_style || is_line_style) {
                        self.out.push('\n');
                    }
                    for (line_idx, line) in rendered.split('\n').enumerate() {
                        if line_idx == 0 || is_line_style {
                            self.push_indent(indent);
                        }
                        self.out.push_str(line);
                        self.out.push('\n');
                    }
                    on_fresh_line = true;
                }
                prev_was_line_style = is_line_style;
            }
        }
        if !on_fresh_line {
            self.out.push('\n');
        }
    }

    fn emit_program(&mut self) -> Result<(), Vec<Diagnostic>> {
        let prog = &self.bundle.program;

        // 1. Board header comments
        let headers = self.board_headers.clone();
        if !headers.is_empty() {
            self.emit_comment_list(&headers, 0, CommentPlacement::Header);
            self.out.push('\n');
        }

        // 2. `#include` directives from `program.imports` + foreign `#include` decls
        let mut emitted_any_include = false;
        let mut emitted_arduino_h = false;
        for imp in &prog.imports {
            if let Some(inc_line) = asl_import_to_cpp_include(imp) {
                if inc_line == "#include <Arduino.h>" {
                    emitted_arduino_h = true;
                }
                self.out.push_str(&inc_line);
                self.out.push('\n');
                emitted_any_include = true;
            }
        }
        // Derived projection from structural Ladder/SFC POUs with board hardware emits
        // `pinMode` / `digitalRead` / `digitalWrite`, which require `#include <Arduino.h>`.
        let has_derived_visual_hw = !prog.hardware.is_empty()
            && prog
                .pous
                .iter()
                .any(|p| matches!(p.body, PouBody::Network(_) | PouBody::Chart(_)));
        if has_derived_visual_hw && !emitted_arduino_h {
            self.out.push_str("#include <Arduino.h>\n");
            emitted_any_include = true;
        }
        for td in &prog.types {
            if let TypeDecl::Foreign {
                id,
                language,
                fidelity,
                text,
                pack_hint,
                span,
                ..
            } = td
                && text.trim_start().starts_with("#include")
            {
                self.emit_leading_comments(id, 0);
                self.emit_foreign_node(
                    ForeignLevel::Decl,
                    Some(id),
                    language,
                    *fidelity,
                    text,
                    pack_hint.as_deref(),
                    *span,
                    0,
                )?;
                self.emit_trailing_comments(id, 0);
                emitted_any_include = true;
            }
        }
        if emitted_any_include {
            self.out.push('\n');
        }

        // 3. User types (`struct`, `enum`, and non-include `TypeDecl::Foreign`)
        let mut emitted_any_type = false;
        for td in &prog.types {
            if let TypeDecl::Foreign { text, .. } = td
                && text.trim_start().starts_with("#include")
            {
                continue;
            }
            self.emit_type_decl(td)?;
            emitted_any_type = true;
        }
        if emitted_any_type {
            self.out.push('\n');
        }

        // 4. Idiomatic hardware pin constants (`const uint8_t ledPin = 13;`, zero magic comments!)
        let mut emitted_any_pin = false;
        for hw in &prog.hardware {
            if is_implicit_pin_label(&hw.label, &hw.pin) {
                continue;
            }
            self.emit_leading_comments(&hw.id, 0);
            let pin_expr = port_to_cpp_pin_expr(&hw.pin);
            self.out
                .push_str(&format!("const uint8_t {} = {pin_expr};", hw.label));
            self.emit_trailing_comments(&hw.id, 0);
            emitted_any_pin = true;
        }
        if emitted_any_pin {
            self.out.push('\n');
        }

        // 5. Global data declarations
        let mut emitted_any_data = false;
        for decl in &prog.data {
            self.emit_data_decl(decl, 0, true)?;
            emitted_any_data = true;
        }
        if emitted_any_data {
            self.out.push('\n');
        }

        // 6. Check if we need a synthesized `void setup()` when `OB_STARTUP` is absent
        let has_startup = prog.pous.iter().any(|p| {
            matches!(
                p.kind,
                PouKind::Ob {
                    ob: ObKind::Startup
                }
            )
        });
        let has_hw_int = prog.pous.iter().any(|p| {
            matches!(
                p.kind,
                PouKind::Ob {
                    ob: ObKind::HwInt { .. }
                }
            )
        });
        let needs_synthesized_setup = !has_startup
            && (has_hw_int || self.has_hardware_needing_setup(&prog.hardware, &prog.pous));

        if needs_synthesized_setup {
            self.out.push_str("void setup() {\n");
            self.emit_hardware_setup_calls(1);
            self.out.push_str("}\n\n");
        }

        // 7. Emit all POUs in order
        for (idx, pou) in prog.pous.iter().enumerate() {
            if idx > 0 {
                self.out.push('\n');
            }
            self.emit_pou(pou)?;
        }

        // 8. Emit any remaining unvisited comments (e.g., anchored to implicit hardware pins) + board footers
        let mut remaining: Vec<usize> = Vec::new();
        for (idx, was_emitted) in self.emitted_comments.iter().enumerate() {
            if !*was_emitted {
                remaining.push(idx);
            }
        }
        if !remaining.is_empty() {
            if !self.out.is_empty() && !self.out.ends_with("\n\n") {
                self.out.push('\n');
            }
            self.emit_comment_list(&remaining, 0, CommentPlacement::Body);
        }

        Ok(())
    }

    fn has_hardware_needing_setup(&self, hardware: &[HardwareBinding], pous: &[Pou]) -> bool {
        let mut used_labels = BTreeSet::new();
        for pou in pous {
            if let PouBody::Statements(stmts) = &pou.body {
                collect_used_hw_calls(stmts, &mut used_labels);
            }
        }
        for hw in hardware {
            if matches!(hw.pull, PullMode::PullUp | PullMode::PullDown) {
                return true;
            }
            if matches!(
                hw.kind,
                HardwareKind::DigitalIn
                    | HardwareKind::DigitalOut
                    | HardwareKind::DigitalBidirectional
            ) && !used_labels.contains(&hw.label)
            {
                return true;
            }
        }
        false
    }

    fn emit_hardware_setup_calls(&mut self, indent: usize) {
        for hw in &self.bundle.program.hardware {
            let mode = match hw.kind {
                HardwareKind::DigitalOut
                | HardwareKind::DigitalBidirectional
                | HardwareKind::PwmOut => Some("OUTPUT"),
                HardwareKind::DigitalIn => match hw.pull {
                    PullMode::None => Some("INPUT"),
                    PullMode::PullUp => Some("INPUT_PULLUP"),
                    PullMode::PullDown => Some("INPUT_PULLDOWN"),
                },
                HardwareKind::AnalogIn | HardwareKind::AnalogOut | HardwareKind::BusPin => None,
            };
            if let Some(mode_kw) = mode {
                let pin_arg = hardware_label_to_cpp_arg(&hw.label, &hw.pin);
                self.push_indent(indent);
                self.out
                    .push_str(&format!("pinMode({pin_arg}, {mode_kw});\n"));
            }
        }
        for pou in &self.bundle.program.pous {
            if let PouKind::Ob {
                ob: ObKind::HwInt {
                    pin_label, edge, ..
                },
            } = &pou.kind
            {
                let pin_arg = self.hw_pin_map.get(pin_label).map_or_else(
                    || pin_label.clone(),
                    |p| hardware_label_to_cpp_arg(pin_label, p),
                );
                let edge_kw = match edge {
                    InterruptEdge::Rising => "RISING",
                    InterruptEdge::Falling => "FALLING",
                    InterruptEdge::Change => "CHANGE",
                    InterruptEdge::Low => "LOW",
                    InterruptEdge::High => "HIGH",
                };
                self.push_indent(indent);
                self.out.push_str(&format!(
                    "attachInterrupt(digitalPinToInterrupt({pin_arg}), {}, {edge_kw});\n",
                    pou.name
                ));
            }
        }
    }

    fn emit_type_decl(&mut self, td: &TypeDecl) -> Result<(), Vec<Diagnostic>> {
        match td {
            TypeDecl::Struct {
                id, name, fields, ..
            } => {
                self.emit_leading_comments(id, 0);
                self.out.push_str(&format!("struct {name} {{\n"));
                for f in fields {
                    self.push_indent(1);
                    let decl = format_cpp_var_declarator(&f.ty, &f.name, None, self.ctx);
                    self.out.push_str(&decl);
                    self.out.push_str(";\n");
                }
                self.out.push_str("};");
                self.emit_trailing_comments(id, 0);
                Ok(())
            }
            TypeDecl::Enum {
                id,
                name,
                backing,
                variants,
                ..
            } => {
                self.emit_leading_comments(id, 0);
                let backing_cpp = scalar_to_cpp_type(backing, self.ctx);
                self.out
                    .push_str(&format!("enum {name} : {backing_cpp} {{\n"));
                for (i, v) in variants.iter().enumerate() {
                    self.push_indent(1);
                    self.out.push_str(&format!("{} = {}", v.name, v.value));
                    if i.saturating_add(1) < variants.len() {
                        self.out.push(',');
                    }
                    self.out.push('\n');
                }
                self.out.push_str("};");
                self.emit_trailing_comments(id, 0);
                Ok(())
            }
            TypeDecl::Foreign {
                id,
                language,
                fidelity,
                text,
                pack_hint,
                span,
                ..
            } => {
                self.emit_leading_comments(id, 0);
                self.emit_foreign_node(
                    ForeignLevel::Decl,
                    Some(id),
                    language,
                    *fidelity,
                    text,
                    pack_hint.as_deref(),
                    *span,
                    0,
                )?;
                self.emit_trailing_comments(id, 0);
                Ok(())
            }
        }
    }

    fn emit_data_decl(
        &mut self,
        decl: &DataDecl,
        indent: usize,
        is_global: bool,
    ) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&decl.id, indent);
        self.push_indent(indent);

        // Local `StorageScope::Var` emits as `static` so scope round-trips losslessly!
        if !is_global && decl.scope == StorageScope::Var {
            self.out.push_str("static ");
        }

        let declarator =
            format_cpp_var_declarator(&decl.ty, &decl.name, Some(decl.scope), self.ctx);
        self.out.push_str(&declarator);

        if let Some(init) = &decl.initial {
            self.out.push_str(" = ");
            let init_s = self.format_expr(init)?;
            self.out.push_str(&init_s);
        }
        self.emit_inner_comments_inline(&decl.id);
        self.out.push(';');
        self.emit_trailing_comments(&decl.id, indent);
        Ok(())
    }

    fn emit_pou(&mut self, pou: &Pou) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&pou.id, 0);

        let (ret_str, fn_name) = match &pou.kind {
            PouKind::Ob {
                ob: ObKind::Startup,
            } => ("void".to_owned(), "setup".to_owned()),
            PouKind::Ob { ob: ObKind::Main } => ("void".to_owned(), "loop".to_owned()),
            PouKind::Ob {
                ob: ObKind::HwInt { .. } | ObKind::CyclicInt { .. } | ObKind::CommEvent { .. },
            } => ("void".to_owned(), pou.name.clone()),
            PouKind::Fc { return_type } => (
                type_ref_to_cpp_type_name(return_type, self.ctx),
                pou.name.clone(),
            ),
            PouKind::Fb => ("void".to_owned(), pou.name.clone()),
        };

        let params_str: Vec<String> = pou
            .params
            .iter()
            .map(|p| format_cpp_param(p, self.ctx))
            .collect();
        self.out.push_str(&format!(
            "{ret_str} {fn_name}({}) {{\n",
            params_str.join(", ")
        ));

        // Emit hardware `pinMode` / `attachInterrupt` calls at top of `setup()`
        if matches!(
            pou.kind,
            PouKind::Ob {
                ob: ObKind::Startup
            }
        ) {
            self.emit_hardware_setup_calls(1);
        }

        // Emit local variable declarations
        for local in &pou.locals {
            self.emit_data_decl(local, 1, false)?;
        }

        // Emit POU body
        match &pou.body {
            PouBody::Statements(stmts) => {
                for stmt in stmts {
                    self.emit_stmt(stmt, 1)?;
                }
            }
            PouBody::Network(net) => {
                let rail_var = self.choose_ladder_rail_name(pou);
                self.emit_ladder_projection(net, &rail_var, 1)?;
            }
            PouBody::Chart(chart) => {
                self.emit_chart_projection(chart, 1)?;
            }
        }

        self.emit_dangling_comments(&pou.id, 1);
        self.out.push('}');
        self.emit_trailing_comments(&pou.id, 0);
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt, indent: usize) -> Result<(), Vec<Diagnostic>> {
        self.emit_leading_comments(&stmt.id, indent);

        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                self.push_indent(indent);
                let lhs = self.format_lvalue(target)?;
                let rhs = self.format_expr(value)?;
                self.out.push_str(&format!("{lhs} = {rhs}"));
                self.emit_inner_comments_inline(&stmt.id);
                self.out.push(';');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::Call { callee, args } => {
                self.push_indent(indent);
                let call_str = self.format_stmt_call(callee, args)?;
                self.out.push_str(&call_str);
                self.emit_inner_comments_inline(&stmt.id);
                self.out.push(';');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::FbInvoke { instance, inputs } => {
                self.push_indent(indent);
                let inst = self.format_lvalue(instance)?;
                if inputs.is_empty() {
                    self.out.push_str(&format!("{inst}()"));
                } else {
                    let mut parts = Vec::with_capacity(inputs.len());
                    for (k, v) in inputs {
                        let vs = self.format_expr(v)?;
                        parts.push(format!(".{k} = {vs}"));
                    }
                    self.out
                        .push_str(&format!("{inst}({{{}}})", parts.join(", ")));
                }
                self.emit_inner_comments_inline(&stmt.id);
                self.out.push(';');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                let cond_s = self.format_expr(condition)?;
                self.push_indent(indent);
                self.out.push_str(&format!("if ({cond_s}) {{\n"));
                for s in then_body {
                    self.emit_stmt(s, indent.saturating_add(1))?;
                }
                self.push_indent(indent);
                self.out.push('}');
                for arm in elif_arms {
                    let ac = self.format_expr(&arm.condition)?;
                    self.out.push_str(&format!(" else if ({ac}) {{\n"));
                    for s in &arm.body {
                        self.emit_stmt(s, indent.saturating_add(1))?;
                    }
                    self.push_indent(indent);
                    self.out.push('}');
                }
                if !else_body.is_empty() {
                    self.out.push_str(" else {\n");
                    for s in else_body {
                        self.emit_stmt(s, indent.saturating_add(1))?;
                    }
                    self.push_indent(indent);
                    self.out.push('}');
                }
                self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                if switch_requires_if_chain(cases) {
                    self.emit_switch_as_if_chain(
                        stmt,
                        scrutinee,
                        cases,
                        default.as_deref(),
                        indent,
                    )?;
                } else {
                    let scrut_s = self.format_expr(scrutinee)?;
                    self.push_indent(indent);
                    self.out.push_str(&format!("switch ({scrut_s}) {{\n"));
                    for c in cases {
                        for pat in &c.patterns {
                            match pat {
                                CasePattern::Value(v) => {
                                    let vs = self.format_expr(v)?;
                                    self.push_indent(indent.saturating_add(1));
                                    self.out.push_str(&format!("case {vs}:\n"));
                                }
                                CasePattern::Range { start, end } => {
                                    if let Some((lo, hi)) = small_const_switch_range(start, end) {
                                        let mut n = lo;
                                        while n <= hi {
                                            self.push_indent(indent.saturating_add(1));
                                            self.out.push_str(&format!("case {n}:\n"));
                                            n = n.saturating_add(1);
                                        }
                                    }
                                }
                            }
                        }
                        for s in &c.body {
                            self.emit_stmt(s, indent.saturating_add(2))?;
                        }
                        let ends_with_return = c
                            .body
                            .last()
                            .is_some_and(|s| matches!(s.kind, StmtKind::Return { .. }));
                        if !ends_with_return {
                            self.push_indent(indent.saturating_add(2));
                            self.out.push_str("break;\n");
                        }
                    }
                    if let Some(def_body) = default {
                        self.push_indent(indent.saturating_add(1));
                        self.out.push_str("default:\n");
                        for s in def_body {
                            self.emit_stmt(s, indent.saturating_add(2))?;
                        }
                        let ends_with_return = def_body
                            .last()
                            .is_some_and(|s| matches!(s.kind, StmtKind::Return { .. }));
                        if !ends_with_return {
                            self.push_indent(indent.saturating_add(2));
                            self.out.push_str("break;\n");
                        }
                    }
                    self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                    self.push_indent(indent);
                    self.out.push('}');
                    self.emit_trailing_comments(&stmt.id, indent);
                }
            }
            StmtKind::While { condition, body } => {
                let cond_s = self.format_expr(condition)?;
                self.push_indent(indent);
                self.out.push_str(&format!("while ({cond_s}) {{\n"));
                for s in body {
                    self.emit_stmt(s, indent.saturating_add(1))?;
                }
                self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                self.push_indent(indent);
                self.out.push('}');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            } => {
                let from_s = self.format_expr(from)?;
                let to_s = self.format_expr(to)?;
                let is_neg_step = matches!(step.kind, ExprKind::Literal(Literal::Int(k)) if k < 0)
                    || matches!(
                        step.kind,
                        ExprKind::Unary {
                            op: UnaryOp::Neg,
                            ..
                        }
                    );
                let cmp = match (is_neg_step, *inclusive) {
                    (false, false) => "<",
                    (false, true) => "<=",
                    (true, false) => ">",
                    (true, true) => ">=",
                };
                let step_s = match &step.kind {
                    ExprKind::Literal(Literal::Int(1)) => format!("{counter}++"),
                    ExprKind::Literal(Literal::Int(-1)) => format!("{counter}--"),
                    ExprKind::Literal(Literal::Int(k)) if *k < 0 => {
                        format!("{counter} -= {}", k.unsigned_abs())
                    }
                    ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand,
                    } => format!("{counter} -= {}", self.format_expr(operand)?),
                    ExprKind::Literal(_)
                    | ExprKind::Ident(_)
                    | ExprKind::Index { .. }
                    | ExprKind::Field { .. }
                    | ExprKind::Call { .. }
                    | ExprKind::Unary {
                        op: UnaryOp::Pos | UnaryOp::Not | UnaryOp::BitNot,
                        ..
                    }
                    | ExprKind::Binary { .. }
                    | ExprKind::Foreign { .. } => {
                        format!("{counter} += {}", self.format_expr(step)?)
                    }
                };
                self.push_indent(indent);
                self.out.push_str(&format!(
                    "for ({counter} = {from_s}; {counter} {cmp} {to_s}; {step_s}) {{\n"
                ));
                for s in body {
                    self.emit_stmt(s, indent.saturating_add(1))?;
                }
                self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
                self.push_indent(indent);
                self.out.push('}');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::Break => {
                self.push_indent(indent);
                self.out.push_str("break");
                self.emit_inner_comments_inline(&stmt.id);
                self.out.push(';');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::Continue => {
                self.push_indent(indent);
                self.out.push_str("continue");
                self.emit_inner_comments_inline(&stmt.id);
                self.out.push(';');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::Return { value } => {
                self.push_indent(indent);
                if let Some(v) = value {
                    let vs = self.format_expr(v)?;
                    self.out.push_str(&format!("return {vs}"));
                } else {
                    self.out.push_str("return");
                }
                self.emit_inner_comments_inline(&stmt.id);
                self.out.push(';');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::Wait { duration } => {
                self.push_indent(indent);
                if let ExprKind::Literal(Literal::TimeUs(us)) = &duration.kind {
                    if *us >= 0 && *us % 1_000 == 0 {
                        self.out.push_str(&format!("delay({})", us / 1_000));
                    } else {
                        self.out
                            .push_str(&format!("delayMicroseconds({})", (*us).max(0)));
                    }
                } else {
                    let ds = self.format_expr(duration)?;
                    self.out.push_str(&format!("delayMicroseconds({ds})"));
                }
                self.emit_inner_comments_inline(&stmt.id);
                self.out.push(';');
                self.emit_trailing_comments(&stmt.id, indent);
            }
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => {
                self.emit_foreign_node(
                    ForeignLevel::Stmt,
                    Some(&stmt.id),
                    language,
                    *fidelity,
                    text,
                    pack_hint.as_deref(),
                    stmt.span,
                    indent,
                )?;
                self.emit_trailing_comments(&stmt.id, indent);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_foreign_node(
        &mut self,
        level: ForeignLevel,
        node_id: Option<&NodeId>,
        language: &str,
        fidelity: ForeignFidelity,
        text: &str,
        pack_hint: Option<&str>,
        span: Span,
        indent: usize,
    ) -> Result<(), Vec<Diagnostic>> {
        let site = ForeignSite {
            level,
            node_id,
            language,
            fidelity,
            text,
            pack_hint,
            span,
        };
        let decision =
            check_foreign_export(&site, LanguageId::Cpp, self.ctx).map_err(|diag| vec![diag])?;
        match decision {
            ForeignExportDecision::VerbatimSameLanguage(raw) => {
                for (i, line) in raw.split('\n').enumerate() {
                    if i > 0 {
                        self.out.push('\n');
                    }
                    self.push_indent(indent);
                    self.out.push_str(line);
                }
            }
            ForeignExportDecision::DemarcatedNotTranslated(rendered) => {
                self.diagnostics.extend(rendered.diagnostics);
                for (i, line) in rendered.rendered.split('\n').enumerate() {
                    if i > 0 {
                        self.out.push('\n');
                    }
                    self.push_indent(indent);
                    self.out.push_str(line);
                }
            }
        }
        Ok(())
    }

    fn format_stmt_call(
        &mut self,
        callee: &str,
        args: &[CallArg],
    ) -> Result<String, Vec<Diagnostic>> {
        match callee {
            "DIGITAL_WRITE" => {
                if let (Some(pin_arg), Some(val_arg)) = (args.first(), args.get(1)) {
                    let pin_s = self.format_pin_expr(&pin_arg.value)?;
                    let val_s = if let ExprKind::Literal(Literal::Bool(b)) = &val_arg.value.kind {
                        if *b {
                            "HIGH".to_owned()
                        } else {
                            "LOW".to_owned()
                        }
                    } else {
                        self.format_expr(&val_arg.value)?
                    };
                    return Ok(format!("digitalWrite({pin_s}, {val_s})"));
                }
            }
            "PWM_WRITE" => {
                if let (Some(pin_arg), Some(duty_arg)) = (args.first(), args.get(1)) {
                    let pin_s = self.format_pin_expr(&pin_arg.value)?;
                    // Canonical chain: `PWM_WRITE(pin, ANALOG_TO_DUTY(CODE_TO_ANALOG(val, codec_id)))`
                    if let Some((val_expr, src_codec_id)) = unwrap_pwm_code_chain(&duty_arg.value) {
                        let val_s = self.format_expr(val_expr)?;
                        if src_codec_id == "pwm_8bit" {
                            return Ok(format!("analogWrite({pin_s}, {val_s})"));
                        }
                        if let (Some(src), Some(dst)) = (
                            self.resolve_codec_for_emit(src_codec_id),
                            self.resolve_codec_for_emit("pwm_8bit"),
                        ) && let Ok(plan) = ReducedRationalPlan::compute(&src, &dst)
                        {
                            return Ok(format!("analogWrite({pin_s}, {})", plan.emit_cpp(&val_s)));
                        }
                        return Ok(format!("analogWrite({pin_s}, {val_s})"));
                    }
                    let duty_s = self.format_expr(&duty_arg.value)?;
                    return Ok(format!("analogWrite({pin_s}, {duty_s})"));
                }
            }
            "ANALOG_READ_RESOLUTION" => {
                if let Some(bits_arg) = args.first() {
                    let bits_s = self.format_expr(&bits_arg.value)?;
                    return Ok(format!("analogReadResolution({bits_s})"));
                }
            }
            "WAIT_TIME" => {
                if let Some(ms_arg) = args.first() {
                    let ms_s = self.format_expr(&ms_arg.value)?;
                    return Ok(format!("delay({ms_s})"));
                }
            }
            "WAIT_TIME_US" => {
                if let Some(us_arg) = args.first() {
                    let us_s = self.format_expr(&us_arg.value)?;
                    return Ok(format!("delayMicroseconds({us_s})"));
                }
            }
            _ => {}
        }
        self.format_expr_call(callee, args)
    }

    fn format_expr_call(
        &mut self,
        callee: &str,
        args: &[CallArg],
    ) -> Result<String, Vec<Diagnostic>> {
        match callee {
            "DIGITAL_READ" => {
                if let Some(pin_arg) = args.first() {
                    let pin_s = self.format_pin_expr(&pin_arg.value)?;
                    return Ok(format!("digitalRead({pin_s})"));
                }
            }
            "ADC_CODE" => {
                if let Some(pin_arg) = args.first() {
                    let pin_s = self.format_pin_expr(&pin_arg.value)?;
                    return Ok(format!("analogRead({pin_s})"));
                }
            }
            "MILLIS" => return Ok("millis()".to_owned()),
            "MICROS" => return Ok("micros()".to_owned()),
            "SEL" => {
                // `SEL(cond, else_expr, then_expr)` -> `(cond ? then_expr : else_expr)`
                if let (Some(c), Some(in0), Some(in1)) = (args.first(), args.get(1), args.get(2)) {
                    let cs = self.format_expr(&c.value)?;
                    let ts = self.format_expr(&in1.value)?;
                    let es = self.format_expr(&in0.value)?;
                    return Ok(format!("({cs} ? {ts} : {es})"));
                }
            }
            // Cross-platform reduced-rational ADC conversion: NEVER emit M = 10^9!
            "ANALOG_TO_CODE" => {
                if let (Some(analog_arg), Some(dst_arg)) = (args.first(), args.get(1))
                    && let ExprKind::Literal(Literal::String(dst_id)) = &dst_arg.value.kind
                {
                    // Pattern 1: `ANALOG_TO_CODE(CODE_TO_ANALOG(x, src_id), dst_id)`
                    if let ExprKind::Call {
                        callee: inner_fn,
                        args: inner_args,
                    } = &analog_arg.value.kind
                        && inner_fn == "CODE_TO_ANALOG"
                        && let (Some(x_arg), Some(src_arg)) =
                            (inner_args.first(), inner_args.get(1))
                        && let ExprKind::Literal(Literal::String(src_id)) = &src_arg.value.kind
                        && let (Some(src_c), Some(dst_c)) = (
                            self.resolve_codec_for_emit(src_id),
                            self.resolve_codec_for_emit(dst_id),
                        )
                        && let Ok(plan) = ReducedRationalPlan::compute(&src_c, &dst_c)
                    {
                        let x_s = self.format_expr(&x_arg.value)?;
                        return Ok(plan.emit_cpp(&x_s));
                    }
                    // Pattern 2: `ANALOG_TO_CODE(ADC_CODE(pin), dst_id)`
                    if let ExprKind::Call {
                        callee: inner_fn,
                        args: inner_args,
                    } = &analog_arg.value.kind
                        && inner_fn == "ADC_CODE"
                        && let Some(pin_arg) = inner_args.first()
                    {
                        let src_id = if let ExprKind::Ident(lbl) = &pin_arg.value.kind {
                            self.hw_codec_map
                                .get(lbl)
                                .cloned()
                                .or_else(|| {
                                    self.ctx.default_adc_codec.as_ref().map(|c| c.id.clone())
                                })
                                .unwrap_or_else(|| "uno_10bit".to_owned())
                        } else {
                            self.ctx
                                .default_adc_codec
                                .as_ref()
                                .map_or_else(|| "uno_10bit".to_owned(), |c| c.id.clone())
                        };
                        if let (Some(src_c), Some(dst_c)) = (
                            self.resolve_codec_for_emit(&src_id),
                            self.resolve_codec_for_emit(dst_id),
                        ) && let Ok(plan) = ReducedRationalPlan::compute(&src_c, &dst_c)
                        {
                            let pin_s = self.format_pin_expr(&pin_arg.value)?;
                            let read_call = format!("analogRead({pin_s})");
                            return Ok(plan.emit_cpp(&read_call));
                        }
                    }
                }
            }
            _ => {}
        }

        // Cast callees (`TO_UDINT`, `INT_TO_DINT`, `DINT_TO_REAL`, `REAL_TO_DINT`, `BOOL_TO_INT`, ...)
        if let Some(target_ty) = cast_target_type_from_callee(callee)
            && args.len() == 1
            && let Some(arg0) = args.first()
        {
            let inner = self.format_expr(&arg0.value)?;
            let cpp_ty = type_ref_to_cpp_type_name(&target_ty, self.ctx);
            return Ok(format!("static_cast<{cpp_ty}>({inner})"));
        }

        // Map canonical `std.iec.math` back to idiomatic Arduino/C++ math names
        let cpp_callee = match callee {
            "ABS" => "abs",
            "MIN" => "min",
            "MAX" => "max",
            "CLAMP" => "constrain",
            "MAP_RANGE" => "map",
            "SQRT" => "sqrt",
            "SIN" => "sin",
            "COS" => "cos",
            "TAN" => "tan",
            "ATAN2" => "atan2",
            other => other,
        };

        let mut arg_strs = Vec::with_capacity(args.len());
        for a in args {
            arg_strs.push(self.format_expr(&a.value)?);
        }
        Ok(format!("{cpp_callee}({})", arg_strs.join(", ")))
    }

    fn format_pin_expr(&mut self, expr: &Expr) -> Result<String, Vec<Diagnostic>> {
        if let ExprKind::Ident(label) = &expr.kind
            && let Some(pin) = self.hw_pin_map.get(label)
        {
            return Ok(hardware_label_to_cpp_arg(label, pin));
        }
        self.format_expr(expr)
    }

    fn format_lvalue(&mut self, lval: &LValue) -> Result<String, Vec<Diagnostic>> {
        match lval {
            LValue::Var { name, .. } => Ok(name.clone()),
            LValue::Index { base, index, .. } => {
                let b = self.format_lvalue(base)?;
                let i = self.format_expr(index)?;
                Ok(format!("{b}[{i}]"))
            }
            LValue::Field { base, field, .. } => {
                let b = self.format_lvalue(base)?;
                Ok(format!("{b}.{field}"))
            }
        }
    }

    fn format_expr(&mut self, expr: &Expr) -> Result<String, Vec<Diagnostic>> {
        match &expr.kind {
            ExprKind::Literal(lit) => Ok(format_literal(lit)),
            ExprKind::Ident(name) => Ok(name.clone()),
            ExprKind::Index { base, index } => {
                let b = self.format_expr(base)?;
                let i = self.format_expr(index)?;
                Ok(format!("{b}[{i}]"))
            }
            ExprKind::Field { base, field } => {
                let b = self.format_expr(base)?;
                Ok(format!("{b}.{field}"))
            }
            ExprKind::Call { callee, args } => self.format_expr_call(callee, args),
            ExprKind::Unary { op, operand } => {
                let inner = self.format_expr(operand)?;
                let sym = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Pos => "+",
                    UnaryOp::Not => "!",
                    UnaryOp::BitNot => "~",
                };
                if (sym == "-" && inner.starts_with('-')) || (sym == "+" && inner.starts_with('+'))
                {
                    Ok(format!("({sym} {inner})"))
                } else {
                    Ok(format!("({sym}{inner})"))
                }
            }
            ExprKind::Binary { op, left, right } => {
                let l = self.format_expr(left)?;
                let r = self.format_expr(right)?;
                let sym = match op {
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "%",
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Shl => "<<",
                    BinaryOp::Shr => ">>",
                    BinaryOp::Lt => "<",
                    BinaryOp::Le => "<=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Ge => ">=",
                    BinaryOp::Eq => "==",
                    BinaryOp::Ne => "!=",
                    BinaryOp::BitAnd => "&",
                    BinaryOp::BitXor => "^",
                    BinaryOp::BitOr => "|",
                    BinaryOp::And => "&&",
                    BinaryOp::Xor => "!=",
                    BinaryOp::Or => "||",
                };
                Ok(format!("({l} {sym} {r})"))
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
                let decision =
                    check_foreign_export(&site, LanguageId::Cpp, self.ctx).map_err(|d| vec![d])?;
                match decision {
                    ForeignExportDecision::VerbatimSameLanguage(raw) => Ok(raw),
                    ForeignExportDecision::DemarcatedNotTranslated(rendered) => {
                        self.diagnostics.extend(rendered.diagnostics);
                        Ok(format!("{} 0", rendered.rendered))
                    }
                }
            }
        }
    }

    fn resolve_codec_for_emit(&self, codec_id: &str) -> Option<Codec> {
        if let Some(c) = self.ctx.codec(codec_id) {
            return Some(c.clone());
        }
        match codec_id {
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

    /// Emits a `Switch` containing large (> 64 values) or non-constant `CasePattern::Range`
    /// arms as a standard ISO C++ `if / else if / else` chain so zero range values are dropped.
    fn emit_switch_as_if_chain(
        &mut self,
        stmt: &Stmt,
        scrutinee: &Expr,
        cases: &[SwitchCase],
        default: Option<&[Stmt]>,
        indent: usize,
    ) -> Result<(), Vec<Diagnostic>> {
        let scrut_raw = self.format_expr(scrutinee)?;
        let wrap_block = !matches!(scrutinee.kind, ExprKind::Ident(_) | ExprKind::Literal(_));
        let (scrut_ref, body_indent) = if wrap_block {
            self.push_indent(indent);
            self.out.push_str("{\n");
            let inner_indent = indent.saturating_add(1);
            self.push_indent(inner_indent);
            self.out
                .push_str(&format!("auto _switch_val = {scrut_raw};\n"));
            ("_switch_val".to_owned(), inner_indent)
        } else {
            (scrut_raw, indent)
        };

        for (i, c) in cases.iter().enumerate() {
            let mut pat_conds = Vec::with_capacity(c.patterns.len());
            for pat in &c.patterns {
                match pat {
                    CasePattern::Value(v) => {
                        let vs = self.format_expr(v)?;
                        pat_conds.push(format!("({scrut_ref} == {vs})"));
                    }
                    CasePattern::Range { start, end } => {
                        let lo_s = self.format_expr(start)?;
                        let hi_s = self.format_expr(end)?;
                        pat_conds.push(format!(
                            "(({scrut_ref} >= {lo_s}) && ({scrut_ref} <= {hi_s}))"
                        ));
                    }
                }
            }
            let cond = if pat_conds.is_empty() {
                "false".to_owned()
            } else {
                pat_conds.join(" || ")
            };
            if i == 0 {
                self.push_indent(body_indent);
                self.out.push_str(&format!("if ({cond}) {{\n"));
            } else {
                self.out.push_str(&format!(" else if ({cond}) {{\n"));
            }
            for s in &c.body {
                self.emit_stmt(s, body_indent.saturating_add(1))?;
            }
            self.push_indent(body_indent);
            self.out.push('}');
        }

        if let Some(def_body) = default {
            if cases.is_empty() {
                for s in def_body {
                    self.emit_stmt(s, body_indent)?;
                }
            } else {
                self.out.push_str(" else {\n");
                for s in def_body {
                    self.emit_stmt(s, body_indent.saturating_add(1))?;
                }
                self.push_indent(body_indent);
                self.out.push('}');
            }
        }

        if wrap_block {
            self.out.push('\n');
            self.emit_dangling_comments(&stmt.id, body_indent);
            self.push_indent(indent);
            self.out.push('}');
        } else {
            self.emit_dangling_comments(&stmt.id, indent.saturating_add(1));
        }
        self.emit_trailing_comments(&stmt.id, indent);
        Ok(())
    }

    /// Chooses a collision-free local boolean rail variable name for Ladder projection.
    fn choose_ladder_rail_name(&self, pou: &Pou) -> String {
        let mut taken: BTreeSet<&str> = BTreeSet::new();
        for hw in &self.bundle.program.hardware {
            taken.insert(&hw.label);
        }
        for d in &self.bundle.program.data {
            taken.insert(&d.name);
        }
        for p in &pou.params {
            taken.insert(&p.name);
        }
        for l in &pou.locals {
            taken.insert(&l.name);
        }
        if !taken.contains("_rail") {
            return "_rail".to_owned();
        }
        let mut n = 1usize;
        loop {
            let candidate = format!("_rail_{n}");
            if !taken.contains(candidate.as_str()) {
                return candidate;
            }
            n = n.saturating_add(1);
        }
    }

    /// Derived readable C++ projection for cross-view Ladder Diagram (`PouBody::Network`).
    ///
    /// Sorts rungs by `order` and cells by `(col, row)`, and materializes a strictly local
    /// C++ power-rail boolean (`bool _rail = true;`) so mid-rung side effects like
    /// `[a] --(R a)-- (b)` preserve pre-reset power flow (`04` §5.1).
    fn emit_ladder_projection(
        &mut self,
        net: &LadderNetwork,
        rail_var: &str,
        indent: usize,
    ) -> Result<(), Vec<Diagnostic>> {
        let mut sorted_rungs: Vec<&_> = net.rungs.iter().collect();
        sorted_rungs.sort_by_key(|r| r.order);
        if sorted_rungs.is_empty() {
            return Ok(());
        }

        self.push_indent(indent);
        self.out.push_str(&format!("bool {rail_var} = true;\n"));

        for (rung_idx, rung) in sorted_rungs.into_iter().enumerate() {
            self.emit_leading_comments(&rung.id, indent);
            if rung_idx > 0 {
                self.push_indent(indent);
                self.out.push_str(&format!("{rail_var} = true;\n"));
            }
            let mut sorted_cells: Vec<&LadderCell> = rung.cells.iter().collect();
            sorted_cells.sort_by_key(|c| (c.col, c.row));
            for cell in sorted_cells {
                self.emit_leading_comments(&cell.id, indent);
                self.emit_ladder_cell_projection(cell, rail_var, indent)?;
            }
        }
        Ok(())
    }

    fn format_pure_ladder_branch(
        &mut self,
        branch: &[LadderCell],
        indent: usize,
    ) -> Result<String, Vec<Diagnostic>> {
        let mut sorted: Vec<&LadderCell> = branch.iter().collect();
        sorted.sort_by_key(|c| (c.col, c.row));
        let mut parts = Vec::new();
        for c in sorted {
            self.emit_leading_comments(&c.id, indent);
            match &c.kind {
                LadderCellKind::Empty => parts.push("false".to_owned()),
                LadderCellKind::Wire => {}
                LadderCellKind::Contact { kind, target } => {
                    let t = self.format_ladder_contact_target(target)?;
                    parts.push(match kind {
                        LadderContactKind::NormallyClosed => format!("(!{t})"),
                        LadderContactKind::NormallyOpen
                        | LadderContactKind::PositiveTransition
                        | LadderContactKind::NegativeTransition => t,
                    });
                }
                LadderCellKind::Box(LadderBoxKind::Compare { op, left, right }) => {
                    let l = self.format_expr(left)?;
                    let r = self.format_expr(right)?;
                    let sym = match op {
                        LadderCompareOp::Eq => "==",
                        LadderCompareOp::Ne => "!=",
                        LadderCompareOp::Lt => "<",
                        LadderCompareOp::Le => "<=",
                        LadderCompareOp::Gt => ">",
                        LadderCompareOp::Ge => ">=",
                    };
                    parts.push(format!("({l} {sym} {r})"));
                }
                LadderCellKind::ParallelOr { branches } => {
                    if branches.is_empty() {
                        parts.push("false".to_owned());
                    } else {
                        let mut sub = Vec::with_capacity(branches.len());
                        for b in branches {
                            sub.push(self.format_pure_ladder_branch(b, indent)?);
                        }
                        parts.push(format!("({})", sub.join(" || ")));
                    }
                }
                LadderCellKind::Coil { .. }
                | LadderCellKind::Box(
                    LadderBoxKind::Fb { .. }
                    | LadderBoxKind::Call { .. }
                    | LadderBoxKind::Foreign { .. },
                ) => {}
            }
        }
        if parts.is_empty() {
            Ok("true".to_owned())
        } else if parts.len() == 1 {
            Ok(parts
                .into_iter()
                .next()
                .unwrap_or_else(|| "true".to_owned()))
        } else {
            Ok(format!("({})", parts.join(" && ")))
        }
    }

    fn format_ladder_contact_target(&mut self, target: &LValue) -> Result<String, Vec<Diagnostic>> {
        if let LValue::Var { name, .. } = target
            && let Some(pin) = self.hw_pin_map.get(name)
        {
            let pin_arg = hardware_label_to_cpp_arg(name, pin);
            return Ok(format!("digitalRead({pin_arg})"));
        }
        self.format_lvalue(target)
    }

    fn emit_ladder_cell_projection(
        &mut self,
        cell: &LadderCell,
        rail_var: &str,
        indent: usize,
    ) -> Result<(), Vec<Diagnostic>> {
        match &cell.kind {
            LadderCellKind::Empty => {
                self.push_indent(indent);
                self.out.push_str(&format!("{rail_var} = false;\n"));
            }
            LadderCellKind::Wire => {}
            LadderCellKind::Contact { kind, target } => {
                let t = self.format_ladder_contact_target(target)?;
                let expr = match kind {
                    LadderContactKind::NormallyOpen
                    | LadderContactKind::PositiveTransition
                    | LadderContactKind::NegativeTransition => t,
                    LadderContactKind::NormallyClosed => format!("(!{t})"),
                };
                self.push_indent(indent);
                self.out
                    .push_str(&format!("{rail_var} = {rail_var} && {expr};\n"));
            }
            LadderCellKind::ParallelOr { branches } => {
                if branches.is_empty() {
                    self.push_indent(indent);
                    self.out.push_str(&format!("{rail_var} = false;\n"));
                } else if branches.iter().all(|br| br.iter().all(is_pure_ladder_cell)) {
                    let mut br_exprs = Vec::with_capacity(branches.len());
                    for br in branches {
                        br_exprs.push(self.format_pure_ladder_branch(br, indent)?);
                    }
                    self.push_indent(indent);
                    self.out.push_str(&format!(
                        "{rail_var} = {rail_var} && ({});\n",
                        br_exprs.join(" || ")
                    ));
                } else {
                    self.push_indent(indent);
                    self.out.push_str("{\n");
                    let inner = indent.saturating_add(1);
                    self.push_indent(inner);
                    self.out.push_str(&format!("bool _br_in = {rail_var};\n"));
                    self.push_indent(inner);
                    self.out.push_str("bool _br_out = false;\n");
                    for br in branches {
                        self.push_indent(inner);
                        self.out.push_str(&format!("{rail_var} = _br_in;\n"));
                        let mut sorted_br: Vec<&LadderCell> = br.iter().collect();
                        sorted_br.sort_by_key(|c| (c.col, c.row));
                        for c in sorted_br {
                            self.emit_leading_comments(&c.id, inner);
                            self.emit_ladder_cell_projection(c, rail_var, inner)?;
                        }
                        self.push_indent(inner);
                        self.out
                            .push_str(&format!("_br_out = _br_out || {rail_var};\n"));
                    }
                    self.push_indent(inner);
                    self.out.push_str(&format!("{rail_var} = _br_out;\n"));
                    self.push_indent(indent);
                    self.out.push_str("}\n");
                }
            }
            LadderCellKind::Coil { kind, target } => {
                self.push_indent(indent);
                if let LValue::Var { name, .. } = target
                    && let Some(pin) = self.hw_pin_map.get(name)
                {
                    let pin_arg = hardware_label_to_cpp_arg(name, pin);
                    match kind {
                        LadderCoilKind::Normal => {
                            self.out
                                .push_str(&format!("digitalWrite({pin_arg}, {rail_var});\n"));
                        }
                        LadderCoilKind::Negated => {
                            self.out
                                .push_str(&format!("digitalWrite({pin_arg}, !{rail_var});\n"));
                        }
                        LadderCoilKind::Set | LadderCoilKind::PositiveTransition => {
                            self.out.push_str(&format!(
                                "if ({rail_var}) {{ digitalWrite({pin_arg}, HIGH); }}\n"
                            ));
                        }
                        LadderCoilKind::Reset | LadderCoilKind::NegativeTransition => {
                            self.out.push_str(&format!(
                                "if ({rail_var}) {{ digitalWrite({pin_arg}, LOW); }}\n"
                            ));
                        }
                    }
                } else {
                    let t = self.format_lvalue(target)?;
                    match kind {
                        LadderCoilKind::Normal => {
                            self.out.push_str(&format!("{t} = {rail_var};\n"));
                        }
                        LadderCoilKind::Negated => {
                            self.out.push_str(&format!("{t} = !{rail_var};\n"));
                        }
                        LadderCoilKind::Set | LadderCoilKind::PositiveTransition => {
                            self.out
                                .push_str(&format!("if ({rail_var}) {{ {t} = true; }}\n"));
                        }
                        LadderCoilKind::Reset | LadderCoilKind::NegativeTransition => {
                            self.out
                                .push_str(&format!("if ({rail_var}) {{ {t} = false; }}\n"));
                        }
                    }
                }
            }
            LadderCellKind::Box(box_kind) => match box_kind {
                LadderBoxKind::Compare { op, left, right } => {
                    let l = self.format_expr(left)?;
                    let r = self.format_expr(right)?;
                    let sym = match op {
                        LadderCompareOp::Eq => "==",
                        LadderCompareOp::Ne => "!=",
                        LadderCompareOp::Lt => "<",
                        LadderCompareOp::Le => "<=",
                        LadderCompareOp::Gt => ">",
                        LadderCompareOp::Ge => ">=",
                    };
                    self.push_indent(indent);
                    self.out
                        .push_str(&format!("{rail_var} = {rail_var} && ({l} {sym} {r});\n"));
                }
                LadderBoxKind::Fb {
                    instance, inputs, ..
                } => {
                    let inst = self.format_lvalue(instance)?;
                    let mut parts = Vec::with_capacity(inputs.len());
                    for (k, v) in inputs {
                        parts.push(format!(".{k} = {}", self.format_expr(v)?));
                    }
                    self.push_indent(indent);
                    self.out
                        .push_str(&format!("{inst}({{{}}});\n", parts.join(", ")));
                    self.push_indent(indent);
                    self.out
                        .push_str(&format!("{rail_var} = {rail_var} && {inst}.Q;\n"));
                }
                LadderBoxKind::Call {
                    callee,
                    args,
                    output,
                } => {
                    let call_args: Vec<CallArg> = args
                        .iter()
                        .map(|(n, e)| CallArg {
                            name: n.clone(),
                            value: e.clone(),
                            span: e.span,
                        })
                        .collect();
                    let call_s = self.format_expr_call(callee, &call_args)?;
                    self.push_indent(indent);
                    if let Some(out_lv) = output {
                        let ol = self.format_lvalue(out_lv)?;
                        self.out
                            .push_str(&format!("if ({rail_var}) {{ {ol} = {call_s}; }}\n"));
                    } else {
                        self.out
                            .push_str(&format!("if ({rail_var}) {{ {call_s}; }}\n"));
                    }
                }
                LadderBoxKind::Foreign {
                    language,
                    fidelity,
                    text,
                    pack_hint,
                } => {
                    self.push_indent(indent);
                    self.out.push_str(&format!("if ({rail_var}) {{\n"));
                    self.emit_foreign_node(
                        ForeignLevel::Stmt,
                        Some(&cell.id),
                        language,
                        *fidelity,
                        text,
                        pack_hint.as_deref(),
                        Span::default(),
                        indent.saturating_add(1),
                    )?;
                    self.out.push('\n');
                    self.push_indent(indent);
                    self.out.push_str("}\n");
                }
            },
        }
        Ok(())
    }

    /// Derived readable C++ projection for cross-view GRAFCET / SFC (`PouBody::Chart`).
    fn emit_chart_projection(
        &mut self,
        chart: &SfcChart,
        indent: usize,
    ) -> Result<(), Vec<Diagnostic>> {
        for step in &chart.steps {
            self.emit_leading_comments(&step.id, indent);
            self.push_indent(indent);
            self.out.push_str(&format!("if ({}.X) {{\n", step.name));
            for action in &step.actions {
                self.emit_leading_comments(&action.id, indent.saturating_add(1));
                if let Some(target) = &action.target {
                    let t = self.format_lvalue(target)?;
                    self.push_indent(indent.saturating_add(1));
                    let val = match action.qualifier {
                        ActionQualifier::R => "false",
                        ActionQualifier::N
                        | ActionQualifier::S
                        | ActionQualifier::L
                        | ActionQualifier::D
                        | ActionQualifier::P
                        | ActionQualifier::P1
                        | ActionQualifier::P0 => "true",
                    };
                    self.out.push_str(&format!("{t} = {val};\n"));
                }
                for s in &action.body {
                    self.emit_stmt(s, indent.saturating_add(1))?;
                }
            }
            self.push_indent(indent);
            self.out.push_str("}\n");
        }
        for tr in &chart.transitions {
            self.emit_leading_comments(&tr.id, indent);
            let rec = self.format_expr(&tr.receptivity)?;
            self.push_indent(indent);
            let from_cond = if tr.from_steps.is_empty() {
                "true".to_owned()
            } else {
                tr.from_steps
                    .iter()
                    .map(|s| format!("{s}.X"))
                    .collect::<Vec<_>>()
                    .join(" && ")
            };
            self.out
                .push_str(&format!("if (({from_cond}) && ({rec})) {{\n"));
            for from in &tr.from_steps {
                self.push_indent(indent.saturating_add(1));
                self.out.push_str(&format!("{from}.X = false;\n"));
            }
            for to in &tr.to_steps {
                self.push_indent(indent.saturating_add(1));
                self.out.push_str(&format!("{to}.X = true;\n"));
            }
            self.push_indent(indent);
            self.out.push_str("}\n");
        }
        Ok(())
    }
}

fn asl_import_to_cpp_include(imp: &str) -> Option<String> {
    match imp {
        "std.arduino.*" => Some("#include <Arduino.h>".to_owned()),
        "std.iec.*" => Some("#include <stdint.h>".to_owned()),
        "std.iec.math" => Some("#include <math.h>".to_owned()),
        other => {
            if let Some(local) = other.strip_prefix("project.") {
                return Some(format!("#include \"{local}.h\""));
            }
            if let Some(rest) = other.strip_prefix("std.") {
                return Some(format!("#include <std/{}.h>", rest.replace('.', "/")));
            }
            None
        }
    }
}

fn unwrap_pwm_code_chain(expr: &Expr) -> Option<(&Expr, &str)> {
    let ExprKind::Call {
        callee: outer,
        args: outer_args,
    } = &expr.kind
    else {
        return None;
    };
    if outer != "ANALOG_TO_DUTY" {
        return None;
    }
    let inner = &outer_args.first()?.value;
    let ExprKind::Call {
        callee: mid,
        args: mid_args,
    } = &inner.kind
    else {
        return None;
    };
    if mid != "CODE_TO_ANALOG" {
        return None;
    }
    let val_expr = &mid_args.first()?.value;
    let ExprKind::Literal(Literal::String(codec_id)) = &mid_args.get(1)?.value.kind else {
        return None;
    };
    Some((val_expr, codec_id.as_str()))
}

fn format_literal(lit: &Literal) -> String {
    match lit {
        Literal::Bool(true) => "true".to_owned(),
        Literal::Bool(false) => "false".to_owned(),
        Literal::Int(n) => n.to_string(),
        Literal::Float(v) => {
            let s = format!("{v}");
            if s.contains('.') || s.contains('e') || s.contains('E') {
                s
            } else {
                format!("{s}.0")
            }
        }
        Literal::TimeUs(us) => us.to_string(),
        Literal::String(s) => format!("\"{}\"", escape_cpp_string(s)),
    }
}

fn escape_cpp_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out
}

fn collect_used_hw_calls(stmts: &[Stmt], out: &mut BTreeSet<String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Call { callee, args } => {
                if matches!(callee.as_str(), "DIGITAL_WRITE" | "PWM_WRITE")
                    && let Some(arg0) = args.first()
                    && let ExprKind::Ident(lbl) = &arg0.value.kind
                {
                    out.insert(lbl.clone());
                }
                for a in args {
                    collect_used_hw_in_expr(&a.value, out);
                }
            }
            StmtKind::Assign { value, .. } | StmtKind::Wait { duration: value } => {
                collect_used_hw_in_expr(value, out);
            }
            StmtKind::FbInvoke { inputs, .. } => {
                for (_, e) in inputs {
                    collect_used_hw_in_expr(e, out);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                collect_used_hw_in_expr(condition, out);
                collect_used_hw_calls(then_body, out);
                for arm in elif_arms {
                    collect_used_hw_in_expr(&arm.condition, out);
                    collect_used_hw_calls(&arm.body, out);
                }
                collect_used_hw_calls(else_body, out);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                collect_used_hw_in_expr(scrutinee, out);
                for c in cases {
                    collect_used_hw_calls(&c.body, out);
                }
                if let Some(d) = default {
                    collect_used_hw_calls(d, out);
                }
            }
            StmtKind::While { condition, body } => {
                collect_used_hw_in_expr(condition, out);
                collect_used_hw_calls(body, out);
            }
            StmtKind::For {
                from,
                to,
                step,
                body,
                ..
            } => {
                collect_used_hw_in_expr(from, out);
                collect_used_hw_in_expr(to, out);
                collect_used_hw_in_expr(step, out);
                collect_used_hw_calls(body, out);
            }
            StmtKind::Return { value } => {
                if let Some(e) = value {
                    collect_used_hw_in_expr(e, out);
                }
            }
            StmtKind::Break | StmtKind::Continue | StmtKind::Foreign { .. } => {}
        }
    }
}

fn collect_used_hw_in_expr(expr: &Expr, out: &mut BTreeSet<String>) {
    match &expr.kind {
        ExprKind::Call { callee, args } => {
            if matches!(callee.as_str(), "DIGITAL_READ" | "ADC_CODE")
                && let Some(arg0) = args.first()
                && let ExprKind::Ident(lbl) = &arg0.value.kind
            {
                out.insert(lbl.clone());
            }
            for a in args {
                collect_used_hw_in_expr(&a.value, out);
            }
        }
        ExprKind::Index { base, index } => {
            collect_used_hw_in_expr(base, out);
            collect_used_hw_in_expr(index, out);
        }
        ExprKind::Field { base, .. } | ExprKind::Unary { operand: base, .. } => {
            collect_used_hw_in_expr(base, out);
        }
        ExprKind::Binary { left, right, .. } => {
            collect_used_hw_in_expr(left, out);
            collect_used_hw_in_expr(right, out);
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::Foreign { .. } => {}
    }
}

/// Returns `Some((lo, hi))` iff a `CasePattern::Range` has integer literal bounds spanning
/// at most 64 discrete values (`lo <= hi` and `hi - lo < 64`).
fn small_const_switch_range(start: &Expr, end: &Expr) -> Option<(i128, i128)> {
    if let (ExprKind::Literal(Literal::Int(lo)), ExprKind::Literal(Literal::Int(hi))) =
        (&start.kind, &end.kind)
        && *lo <= *hi
        && hi.saturating_sub(*lo) < 64
    {
        Some((*lo, *hi))
    } else {
        None
    }
}

/// Returns `true` iff any arm of a `Switch` contains a `CasePattern::Range` exceeding 64
/// discrete values, inverted bounds, or non-constant bounds (requiring ISO C++ `if/else if`).
fn switch_requires_if_chain(cases: &[SwitchCase]) -> bool {
    cases.iter().any(|c| {
        c.patterns.iter().any(|p| match p {
            CasePattern::Value(_) => false,
            CasePattern::Range { start, end } => small_const_switch_range(start, end).is_none(),
        })
    })
}

/// Returns `true` iff `cell` is a pure boolean power-flow element with no side effects.
fn is_pure_ladder_cell(cell: &LadderCell) -> bool {
    match &cell.kind {
        LadderCellKind::Empty
        | LadderCellKind::Wire
        | LadderCellKind::Contact { .. }
        | LadderCellKind::Box(LadderBoxKind::Compare { .. }) => true,
        LadderCellKind::ParallelOr { branches } => {
            branches.iter().all(|br| br.iter().all(is_pure_ladder_cell))
        }
        LadderCellKind::Coil { .. }
        | LadderCellKind::Box(
            LadderBoxKind::Fb { .. } | LadderBoxKind::Call { .. } | LadderBoxKind::Foreign { .. },
        ) => false,
    }
}
