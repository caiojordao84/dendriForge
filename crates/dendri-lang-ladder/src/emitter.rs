//! Lossless canonical emitter for IEC 61131-3 Ladder Diagram (`docs/lbc/ladder.md` §1.1).
//!
//! Guarantees:
//! - Exact `LOSSLESS_ASL` (`parse(emit(M, ctx), ctx) == M`), preserving exact `NodeId`s,
//!   `rung.order`, `(row, col)` cell coordinates, nested `ParallelOr` branches, all contact/coil/
//!   box kinds, and verbatim `[[boards.comments]]` multiset (`C1–C5`).
//! - Zero banned machine magic comments (`// ASL_HW:`, `// ASL_DATA:`, `(* HW: ... *)`).
//! - Zero persisted synthetic `__rail_N` temporaries.

use crate::common::{compare_op_name, fb_kind_name, format_string_lit, render_ladder_text_comment};
use dendri_ast::{
    ActionQualifier, BoardProgram, CasePattern, Comment, CommentAnchor,
    CommentPlacement as AstCommentPlacement, DataDecl, Expr, ExprKind, FbKind, ForeignFidelity,
    HardwareBinding, HardwareKind, InterruptEdge, LValue, LadderBoxKind, LadderCell,
    LadderCellKind, LadderCoilKind, LadderContactKind, LadderNetwork, LadderRung, ModuleDecl,
    NodeId, ObKind, ParamDecl, ParamDirection, Pou, PouBody, PouKind, PullMode, ScalarType,
    SfcChart, SignalKind, Stmt, StmtKind, StorageScope, TypeDecl, TypeRef,
};
use dendri_expr::{print_expr, print_lvalue};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, ForeignLevel, ForeignSite, FrontendOutput, FrontendResult,
    LanguageId, foreign_export_blocked_diagnostic, relocate_inner_comments, restore_eol,
};
use dendri_span::Diagnostic;
use std::collections::{BTreeMap, BTreeSet};

/// Emits a [`BoardProgramBundle`] into canonical Ladder textual notation.
pub(crate) fn emit_ladder_bundle(
    bundle: &BoardProgramBundle,
    ctx: &BoardContext,
) -> FrontendResult<String> {
    check_non_box_foreign_export(&bundle.program, ctx)?;

    let (projected_comments, mut diags) =
        relocate_inner_comments(&bundle.comments, LanguageId::Ladder, Some(&ctx.board_id));

    let mut emitter = LadderEmitter::new(ctx, projected_comments, &mut diags);
    let lf_output = emitter.emit_program(&bundle.program);
    let final_output = restore_eol(&lf_output, bundle.presentation.eol_style);

    Ok(FrontendOutput::with_diagnostics(final_output, diags))
}

/// Checks non-Ladder-box `Foreign` nodes (`Decl::Foreign`, `Stmt::Foreign`, `Expr::Foreign`)
/// for cross-language export blocking.
///
/// Locked `LadderBoxKind::Foreign` boxes inside a `LadderNetwork` are natively preserved in
/// Ladder view (`04` §3.3, `docs/lbc/ladder.md` §1.2) and checked when exporting Ladder to
/// another language (`project_network_for_export`).
fn check_non_box_foreign_export(
    program: &BoardProgram,
    ctx: &BoardContext,
) -> Result<(), Vec<Diagnostic>> {
    if ctx.allow_foreign_export {
        return Ok(());
    }
    let mut blocked = Vec::new();
    check_types_foreign(&program.types, ctx, &mut blocked);
    for d in &program.data {
        if let Some(init) = &d.initial {
            check_expr_foreign(init, Some(&d.id), ctx, &mut blocked);
        }
    }
    for pou in &program.pous {
        check_pou_non_box_foreign(pou, ctx, &mut blocked);
    }
    for m in &program.modules {
        check_types_foreign(&m.types, ctx, &mut blocked);
        for d in &m.data {
            if let Some(init) = &d.initial {
                check_expr_foreign(init, Some(&d.id), ctx, &mut blocked);
            }
        }
        for pou in &m.pous {
            check_pou_non_box_foreign(pou, ctx, &mut blocked);
        }
    }
    if blocked.is_empty() {
        Ok(())
    } else {
        Err(blocked)
    }
}

fn check_types_foreign(types: &[TypeDecl], ctx: &BoardContext, out: &mut Vec<Diagnostic>) {
    for td in types {
        match td {
            TypeDecl::Struct { .. } | TypeDecl::Enum { .. } => {}
            TypeDecl::Foreign {
                id,
                name: _,
                language,
                fidelity,
                text,
                pack_hint,
                span,
            } => {
                if !language.eq_ignore_ascii_case("ladder") {
                    let site = ForeignSite {
                        level: ForeignLevel::Decl,
                        node_id: Some(id),
                        language,
                        fidelity: *fidelity,
                        text,
                        pack_hint: pack_hint.as_deref(),
                        span: *span,
                    };
                    out.push(foreign_export_blocked_diagnostic(
                        &site,
                        LanguageId::Ladder,
                        Some(&ctx.board_id),
                    ));
                }
            }
        }
    }
}

fn check_pou_non_box_foreign(pou: &Pou, ctx: &BoardContext, out: &mut Vec<Diagnostic>) {
    for local in &pou.locals {
        if let Some(init) = &local.initial {
            check_expr_foreign(init, Some(&local.id), ctx, out);
        }
    }
    match &pou.body {
        PouBody::Statements(stmts) => check_stmts_foreign(stmts, ctx, out),
        PouBody::Network(net) => {
            for rung in &net.rungs {
                check_ladder_cells_expr_foreign(&rung.cells, ctx, out);
            }
        }
        PouBody::Chart(chart) => {
            for step in &chart.steps {
                for act in &step.actions {
                    if let Some(dur) = &act.duration {
                        check_expr_foreign(dur, Some(&act.id), ctx, out);
                    }
                    if let Some(target) = &act.target {
                        check_lvalue_foreign(target, Some(&act.id), ctx, out);
                    }
                    check_stmts_foreign(&act.body, ctx, out);
                }
            }
            for tr in &chart.transitions {
                check_expr_foreign(&tr.receptivity, Some(&tr.id), ctx, out);
            }
        }
    }
}

fn check_ladder_cells_expr_foreign(
    cells: &[LadderCell],
    ctx: &BoardContext,
    out: &mut Vec<Diagnostic>,
) {
    for cell in cells {
        match &cell.kind {
            LadderCellKind::Empty | LadderCellKind::Wire => {}
            LadderCellKind::Contact { target, .. } | LadderCellKind::Coil { target, .. } => {
                check_lvalue_foreign(target, Some(&cell.id), ctx, out);
            }
            LadderCellKind::ParallelOr { branches } => {
                for br in branches {
                    check_ladder_cells_expr_foreign(br, ctx, out);
                }
            }
            LadderCellKind::Box(box_kind) => match box_kind {
                LadderBoxKind::Fb {
                    instance, inputs, ..
                } => {
                    check_lvalue_foreign(instance, Some(&cell.id), ctx, out);
                    for (_, e) in inputs {
                        check_expr_foreign(e, Some(&cell.id), ctx, out);
                    }
                }
                LadderBoxKind::Compare { left, right, .. } => {
                    check_expr_foreign(left, Some(&cell.id), ctx, out);
                    check_expr_foreign(right, Some(&cell.id), ctx, out);
                }
                LadderBoxKind::Call { args, output, .. } => {
                    for (_, e) in args {
                        check_expr_foreign(e, Some(&cell.id), ctx, out);
                    }
                    if let Some(lv) = output {
                        check_lvalue_foreign(lv, Some(&cell.id), ctx, out);
                    }
                }
                LadderBoxKind::Foreign { .. } => {
                    // Locked Foreign box is natively represented in Ladder view.
                }
            },
        }
    }
}

fn check_stmts_foreign(stmts: &[Stmt], ctx: &BoardContext, out: &mut Vec<Diagnostic>) {
    for s in stmts {
        match &s.kind {
            StmtKind::Assign { target, value } => {
                check_lvalue_foreign(target, Some(&s.id), ctx, out);
                check_expr_foreign(value, Some(&s.id), ctx, out);
            }
            StmtKind::Call { args, .. } => {
                for a in args {
                    check_expr_foreign(&a.value, Some(&s.id), ctx, out);
                }
            }
            StmtKind::FbInvoke { instance, inputs } => {
                check_lvalue_foreign(instance, Some(&s.id), ctx, out);
                for (_, e) in inputs {
                    check_expr_foreign(e, Some(&s.id), ctx, out);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                check_expr_foreign(condition, Some(&s.id), ctx, out);
                check_stmts_foreign(then_body, ctx, out);
                for arm in elif_arms {
                    check_expr_foreign(&arm.condition, Some(&s.id), ctx, out);
                    check_stmts_foreign(&arm.body, ctx, out);
                }
                check_stmts_foreign(else_body, ctx, out);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                check_expr_foreign(scrutinee, Some(&s.id), ctx, out);
                for c in cases {
                    for p in &c.patterns {
                        match p {
                            CasePattern::Value(e) => check_expr_foreign(e, Some(&s.id), ctx, out),
                            CasePattern::Range { start, end } => {
                                check_expr_foreign(start, Some(&s.id), ctx, out);
                                check_expr_foreign(end, Some(&s.id), ctx, out);
                            }
                        }
                    }
                    check_stmts_foreign(&c.body, ctx, out);
                }
                if let Some(def) = default {
                    check_stmts_foreign(def, ctx, out);
                }
            }
            StmtKind::While { condition, body } => {
                check_expr_foreign(condition, Some(&s.id), ctx, out);
                check_stmts_foreign(body, ctx, out);
            }
            StmtKind::For {
                from,
                to,
                step,
                body,
                ..
            } => {
                check_expr_foreign(from, Some(&s.id), ctx, out);
                check_expr_foreign(to, Some(&s.id), ctx, out);
                check_expr_foreign(step, Some(&s.id), ctx, out);
                check_stmts_foreign(body, ctx, out);
            }
            StmtKind::Return { value } => {
                if let Some(e) = value {
                    check_expr_foreign(e, Some(&s.id), ctx, out);
                }
            }
            StmtKind::Wait { duration } => check_expr_foreign(duration, Some(&s.id), ctx, out),
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => {
                if !language.eq_ignore_ascii_case("ladder") {
                    let site = ForeignSite {
                        level: ForeignLevel::Stmt,
                        node_id: Some(&s.id),
                        language,
                        fidelity: *fidelity,
                        text,
                        pack_hint: pack_hint.as_deref(),
                        span: s.span,
                    };
                    out.push(foreign_export_blocked_diagnostic(
                        &site,
                        LanguageId::Ladder,
                        Some(&ctx.board_id),
                    ));
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
}

fn check_lvalue_foreign(
    lv: &LValue,
    node_id: Option<&NodeId>,
    ctx: &BoardContext,
    out: &mut Vec<Diagnostic>,
) {
    match lv {
        LValue::Var { .. } => {}
        LValue::Index { base, index, .. } => {
            check_lvalue_foreign(base, node_id, ctx, out);
            check_expr_foreign(index, node_id, ctx, out);
        }
        LValue::Field { base, .. } => check_lvalue_foreign(base, node_id, ctx, out),
    }
}

fn check_expr_foreign(
    expr: &Expr,
    node_id: Option<&NodeId>,
    ctx: &BoardContext,
    out: &mut Vec<Diagnostic>,
) {
    match &expr.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
        ExprKind::Index { base, index } => {
            check_expr_foreign(base, node_id, ctx, out);
            check_expr_foreign(index, node_id, ctx, out);
        }
        ExprKind::Field { base, .. } => check_expr_foreign(base, node_id, ctx, out),
        ExprKind::Call { args, .. } => {
            for a in args {
                check_expr_foreign(&a.value, node_id, ctx, out);
            }
        }
        ExprKind::Unary { operand, .. } => check_expr_foreign(operand, node_id, ctx, out),
        ExprKind::Binary { left, right, .. } => {
            check_expr_foreign(left, node_id, ctx, out);
            check_expr_foreign(right, node_id, ctx, out);
        }
        ExprKind::Foreign { language, text } => {
            if !language.eq_ignore_ascii_case("ladder") {
                let site = ForeignSite {
                    level: ForeignLevel::Expr,
                    node_id,
                    language,
                    fidelity: ForeignFidelity::F0,
                    text,
                    pack_hint: None,
                    span: expr.span,
                };
                out.push(foreign_export_blocked_diagnostic(
                    &site,
                    LanguageId::Ladder,
                    Some(&ctx.board_id),
                ));
            }
        }
    }
}

#[derive(Default)]
struct NodeCommentBuckets {
    leading: Vec<usize>,
    trailing: Vec<usize>,
    dangling: Vec<usize>,
}

struct LadderEmitter<'a> {
    ctx: &'a BoardContext,
    comments: Vec<Comment>,
    by_node: BTreeMap<NodeId, NodeCommentBuckets>,
    non_node_comments: Vec<usize>,
    emitted_comments: BTreeSet<usize>,
    diags: &'a mut Vec<Diagnostic>,
    out: String,
}

impl<'a> LadderEmitter<'a> {
    fn new(ctx: &'a BoardContext, comments: Vec<Comment>, diags: &'a mut Vec<Diagnostic>) -> Self {
        let mut by_node: BTreeMap<NodeId, NodeCommentBuckets> = BTreeMap::new();
        let mut non_node_comments = Vec::new();
        for (idx, c) in comments.iter().enumerate() {
            match &c.anchor {
                CommentAnchor::Node { id } => {
                    let entry = by_node.entry(id.clone()).or_default();
                    match c.placement {
                        AstCommentPlacement::Trailing => {
                            if c.text.contains('\n') || !entry.trailing.is_empty() {
                                entry.leading.push(idx);
                            } else {
                                entry.trailing.push(idx);
                            }
                        }
                        AstCommentPlacement::Dangling => entry.dangling.push(idx),
                        AstCommentPlacement::Leading
                        | AstCommentPlacement::Header
                        | AstCommentPlacement::Title
                        | AstCommentPlacement::Body
                        | AstCommentPlacement::Inner => entry.leading.push(idx),
                    }
                }
                CommentAnchor::Project
                | CommentAnchor::Board { .. }
                | CommentAnchor::Component { .. }
                | CommentAnchor::Wire { .. } => {
                    non_node_comments.push(idx);
                }
            }
        }
        Self {
            ctx,
            comments,
            by_node,
            non_node_comments,
            emitted_comments: BTreeSet::new(),
            diags,
            out: String::new(),
        }
    }

    fn emit_comment_indices(&mut self, indices: &[usize], indent: &str) {
        let mut emitted_any = false;
        for &idx in indices {
            if !self.emitted_comments.insert(idx) {
                continue;
            }
            let Some(comment) = self.comments.get(idx) else {
                continue;
            };
            if emitted_any {
                // Separate distinct comment records with a blank line so contiguous `//`
                // records are never merged together on re-parse (`C1` multiset conservation).
                self.out.push('\n');
            }
            let rendered = render_ladder_text_comment(
                &comment.text,
                comment.origin.style,
                indent,
                false,
                self.ctx,
                self.diags,
            );
            self.out.push_str(&rendered);
            self.out.push('\n');
            emitted_any = true;
        }
    }

    fn emit_leading_for(&mut self, node_id: &NodeId, indent: &str) {
        let indices = self
            .by_node
            .get(node_id)
            .map(|b| b.leading.clone())
            .unwrap_or_default();
        if !indices.is_empty() {
            self.emit_comment_indices(&indices, indent);
        }
    }

    fn emit_dangling_for(&mut self, node_id: &NodeId, indent: &str) {
        let indices = self
            .by_node
            .get(node_id)
            .map(|b| b.dangling.clone())
            .unwrap_or_default();
        if !indices.is_empty() {
            self.emit_comment_indices(&indices, indent);
        }
    }

    fn emit_trailing_inline(&mut self, node_id: &NodeId) -> bool {
        let indices = self
            .by_node
            .get(node_id)
            .map(|b| b.trailing.clone())
            .unwrap_or_default();
        let mut emitted_line_comment = false;
        for idx in indices {
            if !self.emitted_comments.insert(idx) {
                continue;
            }
            let Some(comment) = self.comments.get(idx) else {
                continue;
            };
            self.out.push(' ');
            let rendered = render_ladder_text_comment(
                &comment.text,
                comment.origin.style,
                "",
                true,
                self.ctx,
                self.diags,
            );
            if rendered.starts_with("//") {
                emitted_line_comment = true;
            }
            self.out.push_str(&rendered);
            if emitted_line_comment {
                // Only one `//` trailing comment can appear on a line; any extra fall onto next line
                self.out.push('\n');
            }
        }
        emitted_line_comment
    }

    fn emit_program(&mut self, program: &BoardProgram) -> String {
        let top = self.non_node_comments.clone();
        self.emit_comment_indices(&top, "");

        for imp in &program.imports {
            self.out.push_str("IMPORT ");
            self.out.push_str(imp);
            self.out.push_str(";\n");
        }
        if !program.imports.is_empty() {
            self.out.push('\n');
        }

        if !program.hardware.is_empty() {
            self.emit_tag_table(&program.hardware);
            self.out.push('\n');
        }

        if !program.types.is_empty() {
            self.emit_type_block(&program.types, "");
            self.out.push('\n');
        }

        if !program.data.is_empty() {
            self.emit_global_data_blocks(&program.data, "");
            self.out.push('\n');
        }

        for m in &program.modules {
            self.emit_module(m);
            self.out.push('\n');
        }

        for (idx, pou) in program.pous.iter().enumerate() {
            if idx > 0 {
                self.out.push('\n');
            }
            self.emit_pou(pou, "");
        }

        // Preserve any orphaned/unattached comments at end of file so `C1` is never violated.
        let unemitted: Vec<usize> = (0..self.comments.len())
            .filter(|i| !self.emitted_comments.contains(i))
            .collect();
        if !unemitted.is_empty() {
            if !self.out.is_empty() && !self.out.ends_with("\n\n") {
                self.out.push('\n');
            }
            self.emit_comment_indices(&unemitted, "");
        }

        std::mem::take(&mut self.out)
    }

    fn emit_tag_table(&mut self, bindings: &[HardwareBinding]) {
        self.out.push_str("TAG_TABLE\n");
        for hw in bindings {
            self.emit_leading_for(&hw.id, "    ");
            self.out.push_str("    ");
            self.out.push_str(&hw.label);
            self.out.push_str(" (");
            self.out.push_str(hw.id.as_str());
            self.out.push_str(") AT ");
            let surface_pin = self
                .ctx
                .iec_for_port(&hw.pin)
                .filter(|iec| self.ctx.port_for_iec(iec) == Some(hw.pin.as_str()))
                .unwrap_or(hw.pin.as_str());
            self.out.push_str(surface_pin);
            self.out.push_str(" : ");
            self.out.push_str(hardware_kind_str(hw.kind));
            match hw.pull {
                PullMode::None => {}
                PullMode::PullUp => self.out.push_str(" PULL_UP"),
                PullMode::PullDown => self.out.push_str(" PULL_DOWN"),
            }
            if hw.active_low {
                self.out.push_str(" ACTIVE_LOW");
            }
            if let Some(codec) = &hw.codec {
                self.out.push_str(" CODEC ");
                self.out.push_str(codec);
            }
            if let Some(r) = &hw.range {
                self.out.push_str(" RANGE ");
                match r.signal {
                    SignalKind::Voltage => self.out.push_str("VOLTAGE "),
                    SignalKind::Current => self.out.push_str("CURRENT "),
                }
                self.out.push_str(&format_f64(r.low));
                self.out.push_str("..");
                self.out.push_str(&format_f64(r.high));
                if r.live_zero {
                    self.out.push_str(" LIVE_ZERO");
                }
                if r.hardware_normalizes_live_zero {
                    self.out.push_str(" HW_NORM");
                }
            }
            self.out.push(';');
            let broke = self.emit_trailing_inline(&hw.id);
            if !broke {
                self.out.push('\n');
            }
        }
        self.out.push_str("END_TAG_TABLE\n");
    }

    fn emit_type_block(&mut self, types: &[TypeDecl], indent: &str) {
        self.out.push_str(indent);
        self.out.push_str("TYPE\n");
        let inner = format!("{indent}    ");
        for td in types {
            match td {
                TypeDecl::Struct {
                    id,
                    name,
                    fields,
                    span: _,
                } => {
                    self.emit_leading_for(id, &inner);
                    self.out.push_str(&inner);
                    self.out.push_str("STRUCT ");
                    self.out.push_str(name);
                    self.out.push_str(" (");
                    self.out.push_str(id.as_str());
                    self.out.push_str(")\n");
                    for f in fields {
                        self.out.push_str(&inner);
                        self.out.push_str("    ");
                        self.out.push_str(&f.name);
                        self.out.push_str(" : ");
                        self.out.push_str(&format_type_ref(&f.ty));
                        self.out.push_str(";\n");
                    }
                    self.out.push_str(&inner);
                    self.out.push_str("END_STRUCT;\n");
                }
                TypeDecl::Enum {
                    id,
                    name,
                    backing,
                    variants,
                    span: _,
                } => {
                    self.emit_leading_for(id, &inner);
                    self.out.push_str(&inner);
                    self.out.push_str("ENUM ");
                    self.out.push_str(name);
                    self.out.push_str(" (");
                    self.out.push_str(id.as_str());
                    self.out.push_str(") : ");
                    self.out
                        .push_str(&format_type_ref(&TypeRef::scalar(backing.clone())));
                    self.out.push_str(" (");
                    for (idx, v) in variants.iter().enumerate() {
                        if idx > 0 {
                            self.out.push_str(", ");
                        }
                        self.out.push_str(&v.name);
                        self.out.push_str(" = ");
                        self.out.push_str(&v.value.to_string());
                    }
                    self.out.push_str(") END_ENUM;\n");
                }
                TypeDecl::Foreign {
                    id,
                    name,
                    language,
                    fidelity,
                    text,
                    pack_hint,
                    span: _,
                } => {
                    self.emit_leading_for(id, &inner);
                    self.out.push_str(&inner);
                    self.out.push_str("FOREIGN_TYPE");
                    if let Some(n) = name {
                        self.out.push(' ');
                        self.out.push_str(n);
                    }
                    self.out.push_str(" (");
                    self.out.push_str(id.as_str());
                    self.out.push_str(") ");
                    self.emit_foreign_payload(language, *fidelity, text, pack_hint.as_deref());
                    self.out.push_str(";\n");
                }
            }
        }
        self.out.push_str(indent);
        self.out.push_str("END_TYPE\n");
    }

    fn emit_global_data_blocks(&mut self, data: &[DataDecl], indent: &str) {
        self.emit_scoped_data_blocks(data, "VAR_GLOBAL", indent);
    }

    fn emit_scoped_data_blocks(&mut self, data: &[DataDecl], base_kw: &str, indent: &str) {
        let mut idx = 0usize;
        while let Some(first) = data.get(idx) {
            let scope = first.scope;
            let mut end = idx.saturating_add(1);
            while let Some(next) = data.get(end) {
                if next.scope == scope {
                    end = end.saturating_add(1);
                } else {
                    break;
                }
            }
            self.out.push_str(indent);
            if base_kw == "VAR" && scope == StorageScope::Temp {
                self.out.push_str("VAR_TEMP\n");
            } else {
                self.out.push_str(base_kw);
                match scope {
                    StorageScope::Var => {}
                    StorageScope::Const => self.out.push_str(" CONSTANT"),
                    StorageScope::Retain => self.out.push_str(" RETAIN"),
                    StorageScope::Persistent => self.out.push_str(" PERSISTENT"),
                    StorageScope::Temp => self.out.push_str(" TEMP"),
                }
                self.out.push('\n');
            }
            let inner = format!("{indent}    ");
            for d in data.get(idx..end).unwrap_or(&[]) {
                self.emit_leading_for(&d.id, &inner);
                self.out.push_str(&inner);
                self.out.push_str(&d.name);
                self.out.push_str(" (");
                self.out.push_str(d.id.as_str());
                self.out.push_str(") : ");
                self.out.push_str(&format_type_ref(&d.ty));
                if let Some(init) = &d.initial {
                    self.out.push_str(" := ");
                    self.out.push_str(&print_expr(init));
                }
                self.out.push(';');
                let broke = self.emit_trailing_inline(&d.id);
                if !broke {
                    self.out.push('\n');
                }
            }
            self.out.push_str(indent);
            self.out.push_str("END_VAR\n");
            idx = end;
        }
    }

    fn emit_module(&mut self, m: &ModuleDecl) {
        self.emit_leading_for(&m.id, "");
        self.out.push_str("MODULE ");
        self.out.push_str(&m.name);
        self.out.push_str(" (");
        self.out.push_str(m.id.as_str());
        self.out.push_str(")\n");
        for imp in &m.imports {
            self.out.push_str("    IMPORT ");
            self.out.push_str(imp);
            self.out.push_str(";\n");
        }
        for exp in &m.exports {
            self.out.push_str("    EXPORT ");
            self.out.push_str(exp);
            self.out.push_str(";\n");
        }
        if !m.types.is_empty() {
            self.emit_type_block(&m.types, "    ");
        }
        if !m.data.is_empty() {
            self.emit_global_data_blocks(&m.data, "    ");
        }
        for pou in &m.pous {
            self.emit_pou(pou, "    ");
        }
        self.out.push_str("END_MODULE\n");
    }

    fn emit_pou(&mut self, pou: &Pou, indent: &str) {
        self.emit_leading_for(&pou.id, indent);
        let kw = match &pou.body {
            PouBody::Network(_) => "NETWORK",
            PouBody::Statements(_) => "STRUCTURED_POU",
            PouBody::Chart(_) => "CHART_POU",
        };
        self.out.push_str(indent);
        self.out.push_str(kw);
        self.out.push(' ');
        self.out.push_str(&pou.name);
        self.out.push_str(" (");
        self.out.push_str(pou.id.as_str());
        self.out.push_str(") KIND ");
        self.emit_pou_kind(&pou.kind);
        if let Some(view) = pou.preferred_view {
            self.out.push_str(" VIEW ");
            self.out.push_str(view.as_str());
        }
        let broke = self.emit_trailing_inline(&pou.id);
        if !broke {
            self.out.push('\n');
        }

        let inner = format!("{indent}    ");
        self.emit_param_blocks(&pou.params, &inner);
        if !pou.locals.is_empty() {
            self.emit_scoped_data_blocks(&pou.locals, "VAR", &inner);
        }

        match &pou.body {
            PouBody::Network(net) => {
                self.emit_network(net, &inner);
                self.emit_dangling_for(&pou.id, &inner);
                self.out.push_str(indent);
                self.out.push_str("END_NETWORK\n");
            }
            PouBody::Statements(stmts) => {
                self.out.push_str(&inner);
                self.out.push_str("BEGIN\n");
                let stmt_indent = format!("{inner}    ");
                for s in stmts {
                    self.emit_stmt(s, &stmt_indent);
                }
                self.emit_dangling_for(&pou.id, &inner);
                self.out.push_str(indent);
                self.out.push_str("END_STRUCTURED_POU\n");
            }
            PouBody::Chart(chart) => {
                self.emit_chart(chart, &inner);
                self.out.push_str(indent);
                self.out.push_str("END_CHART_POU\n");
            }
        }
    }

    fn emit_param_blocks(&mut self, params: &[ParamDecl], indent: &str) {
        let mut idx = 0usize;
        while let Some(first) = params.get(idx) {
            let dir = first.direction;
            let mut end = idx.saturating_add(1);
            while let Some(next) = params.get(end) {
                if next.direction == dir {
                    end = end.saturating_add(1);
                } else {
                    break;
                }
            }
            let kw = match dir {
                ParamDirection::In => "VAR_INPUT",
                ParamDirection::Out => "VAR_OUTPUT",
                ParamDirection::InOut => "VAR_IN_OUT",
            };
            self.out.push_str(indent);
            self.out.push_str(kw);
            self.out.push('\n');
            for p in params.get(idx..end).unwrap_or(&[]) {
                self.out.push_str(indent);
                self.out.push_str("    ");
                self.out.push_str(&p.name);
                self.out.push_str(" : ");
                self.out.push_str(&format_type_ref(&p.ty));
                self.out.push_str(";\n");
            }
            self.out.push_str(indent);
            self.out.push_str("END_VAR\n");
            idx = end;
        }
    }

    fn emit_pou_kind(&mut self, kind: &PouKind) {
        match kind {
            PouKind::Ob { ob } => match ob {
                ObKind::Main => self.out.push_str("OB_MAIN"),
                ObKind::Startup => self.out.push_str("OB_STARTUP"),
                ObKind::CyclicInt { period_us } => {
                    self.out.push_str("OB_CYCLIC_INT(");
                    self.out.push_str(&period_us.to_string());
                    self.out.push(')');
                }
                ObKind::HwInt {
                    pin_label,
                    edge,
                    priority,
                } => {
                    self.out.push_str("OB_HW_INT(");
                    self.out.push_str(pin_label);
                    self.out.push_str(", ");
                    self.out.push_str(interrupt_edge_str(*edge));
                    self.out.push_str(", ");
                    self.out.push_str(&priority.to_string());
                    self.out.push(')');
                }
                ObKind::CommEvent { bus_id } => {
                    self.out.push_str("OB_COMM_EVENT(");
                    self.out.push_str(bus_id);
                    self.out.push(')');
                }
            },
            PouKind::Fc { return_type } => {
                self.out.push_str("FC : ");
                self.out.push_str(&format_type_ref(return_type));
            }
            PouKind::Fb => self.out.push_str("FB"),
        }
    }

    fn emit_network(&mut self, net: &LadderNetwork, indent: &str) {
        for rung in &net.rungs {
            self.emit_rung(rung, indent);
        }
    }

    fn emit_rung(&mut self, rung: &LadderRung, indent: &str) {
        self.emit_leading_for(&rung.id, indent);
        self.out.push_str(indent);
        self.out.push_str("RUNG ");
        self.out.push_str(&rung.order.to_string());
        self.out.push_str(" (");
        self.out.push_str(rung.id.as_str());
        self.out.push_str("):");

        if rung.cells.is_empty() {
            self.out.push(';');
            let broke = self.emit_trailing_inline(&rung.id);
            if !broke {
                self.out.push('\n');
            }
            return;
        }

        let has_cell_comments = self.cells_have_comments(&rung.cells);
        if has_cell_comments {
            self.out.push('\n');
            let cell_indent = format!("{indent}    ");
            self.emit_cell_series_multiline(&rung.cells, &cell_indent);
            self.out.push_str(&cell_indent);
            self.out.push(';');
        } else {
            self.out.push(' ');
            self.emit_cell_series_inline(&rung.cells);
            self.out.push(';');
        }

        let broke = self.emit_trailing_inline(&rung.id);
        if !broke {
            self.out.push('\n');
        }
    }

    fn cells_have_comments(&self, cells: &[LadderCell]) -> bool {
        for c in cells {
            if let Some(b) = self.by_node.get(&c.id)
                && (!b.leading.is_empty() || !b.trailing.is_empty() || !b.dangling.is_empty())
            {
                return true;
            }
            if let LadderCellKind::ParallelOr { branches } = &c.kind
                && branches.iter().any(|br| self.cells_have_comments(br))
            {
                return true;
            }
        }
        false
    }

    fn emit_cell_series_inline(&mut self, cells: &[LadderCell]) {
        for (idx, cell) in cells.iter().enumerate() {
            if idx > 0 {
                self.out.push_str(" -- ");
            }
            self.emit_cell_core(cell, None);
        }
    }

    fn emit_cell_series_multiline(&mut self, cells: &[LadderCell], indent: &str) {
        for (idx, cell) in cells.iter().enumerate() {
            if idx > 0 {
                self.out.push_str(indent);
                self.out.push_str("--\n");
            }
            self.emit_leading_for(&cell.id, indent);
            self.out.push_str(indent);
            self.emit_cell_core(cell, Some(indent));
            let broke = self.emit_trailing_inline(&cell.id);
            if !broke {
                self.out.push('\n');
            }
        }
    }

    fn emit_cell_core(&mut self, cell: &LadderCell, multiline_indent: Option<&str>) {
        match &cell.kind {
            LadderCellKind::Empty => {
                self.out.push_str("EMPTY ");
                self.emit_grid_pos(cell);
            }
            LadderCellKind::Wire => {
                self.out.push_str("WIRE ");
                self.emit_grid_pos(cell);
            }
            LadderCellKind::Contact { kind, target } => {
                let glyph = match kind {
                    LadderContactKind::NormallyOpen => "| |",
                    LadderContactKind::NormallyClosed => "|/|",
                    LadderContactKind::PositiveTransition => "|P|",
                    LadderContactKind::NegativeTransition => "|N|",
                };
                self.out.push_str(glyph);
                self.out.push(' ');
                self.out.push_str(&print_lvalue(target));
                self.out.push(' ');
                self.emit_grid_pos(cell);
            }
            LadderCellKind::Coil { kind, target } => {
                let glyph = match kind {
                    LadderCoilKind::Normal => "( )",
                    LadderCoilKind::Negated => "(/)",
                    LadderCoilKind::Set => "(S)",
                    LadderCoilKind::Reset => "(R)",
                    LadderCoilKind::PositiveTransition => "(P)",
                    LadderCoilKind::NegativeTransition => "(N)",
                };
                self.out.push_str(glyph);
                self.out.push(' ');
                self.out.push_str(&print_lvalue(target));
                self.out.push(' ');
                self.emit_grid_pos(cell);
            }
            LadderCellKind::ParallelOr { branches } => {
                self.out.push_str("BRANCH ");
                self.emit_grid_pos(cell);
                if branches.is_empty() {
                    self.out.push_str(" {}");
                } else if let Some(ind) = multiline_indent {
                    self.out.push_str(" {\n");
                    let br_indent = format!("{ind}    ");
                    for (idx, br) in branches.iter().enumerate() {
                        if idx > 0 {
                            self.out.push_str(&br_indent);
                            self.out.push_str("||\n");
                        }
                        self.emit_cell_series_multiline(br, &br_indent);
                    }
                    self.out.push_str(ind);
                    self.out.push('}');
                } else {
                    self.out.push_str(" { ");
                    for (idx, br) in branches.iter().enumerate() {
                        if idx > 0 {
                            self.out.push_str(" || ");
                        }
                        self.emit_cell_series_inline(br);
                    }
                    self.out.push_str(" }");
                }
            }
            LadderCellKind::Box(box_kind) => match box_kind {
                LadderBoxKind::Fb {
                    kind,
                    instance,
                    inputs,
                } => {
                    self.out.push_str("FB ");
                    self.out.push_str(fb_kind_name(kind));
                    self.out.push(' ');
                    self.out.push_str(&print_lvalue(instance));
                    self.out.push('(');
                    for (idx, (pin, expr)) in inputs.iter().enumerate() {
                        if idx > 0 {
                            self.out.push_str(", ");
                        }
                        self.out.push_str(pin);
                        self.out.push_str(" := ");
                        self.out.push_str(&print_expr(expr));
                    }
                    self.out.push_str(") ");
                    self.emit_grid_pos(cell);
                }
                LadderBoxKind::Compare { op, left, right } => {
                    self.out.push_str("CMP ");
                    self.out.push_str(compare_op_name(*op));
                    self.out.push('(');
                    self.out.push_str(&print_expr(left));
                    self.out.push_str(", ");
                    self.out.push_str(&print_expr(right));
                    self.out.push_str(") ");
                    self.emit_grid_pos(cell);
                }
                LadderBoxKind::Call {
                    callee,
                    args,
                    output,
                } => {
                    self.out.push_str("CALL ");
                    self.out.push_str(callee);
                    self.out.push('(');
                    for (idx, (name, expr)) in args.iter().enumerate() {
                        if idx > 0 {
                            self.out.push_str(", ");
                        }
                        if let Some(n) = name {
                            self.out.push_str(n);
                            self.out.push_str(" := ");
                        }
                        self.out.push_str(&print_expr(expr));
                    }
                    self.out.push(')');
                    if let Some(out_lv) = output {
                        self.out.push_str(" => ");
                        self.out.push_str(&print_lvalue(out_lv));
                    }
                    self.out.push(' ');
                    self.emit_grid_pos(cell);
                }
                LadderBoxKind::Foreign {
                    language,
                    fidelity,
                    text,
                    pack_hint,
                } => {
                    self.out.push_str("FOREIGN");
                    self.emit_foreign_payload(language, *fidelity, text, pack_hint.as_deref());
                    self.out.push(' ');
                    self.emit_grid_pos(cell);
                }
            },
        }
    }

    fn emit_grid_pos(&mut self, cell: &LadderCell) {
        self.out.push_str("@(");
        self.out.push_str(&cell.row.to_string());
        self.out.push_str(", ");
        self.out.push_str(&cell.col.to_string());
        self.out.push_str(", ");
        self.out.push_str(cell.id.as_str());
        self.out.push(')');
    }

    fn emit_foreign_payload(
        &mut self,
        language: &str,
        fidelity: ForeignFidelity,
        text: &str,
        pack_hint: Option<&str>,
    ) {
        self.out.push('(');
        self.out.push_str(&format_string_lit(language));
        self.out.push_str(", ");
        self.out.push_str(match fidelity {
            ForeignFidelity::F0 => "F0",
            ForeignFidelity::F1 => "F1",
            ForeignFidelity::F2 => "F2",
        });
        self.out.push_str(", ");
        self.out.push_str(&format_string_lit(text));
        if let Some(hint) = pack_hint {
            self.out.push_str(", ");
            self.out.push_str(&format_string_lit(hint));
        }
        self.out.push(')');
    }

    fn emit_stmt(&mut self, stmt: &Stmt, indent: &str) {
        self.emit_leading_for(&stmt.id, indent);
        self.out.push_str(indent);
        self.out.push('#');
        self.out.push_str(stmt.id.as_str());
        self.out.push_str(": ");
        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                self.out.push_str(&print_lvalue(target));
                self.out.push_str(" := ");
                self.out.push_str(&print_expr(value));
                self.out.push(';');
            }
            StmtKind::Call { callee, args } => {
                self.out.push_str("CALL ");
                self.out.push_str(callee);
                self.out.push('(');
                for (idx, a) in args.iter().enumerate() {
                    if idx > 0 {
                        self.out.push_str(", ");
                    }
                    if let Some(n) = &a.name {
                        self.out.push_str(n);
                        self.out.push_str(" := ");
                    }
                    self.out.push_str(&print_expr(&a.value));
                }
                self.out.push_str(");");
            }
            StmtKind::FbInvoke { instance, inputs } => {
                self.out.push_str("FB_INVOKE ");
                self.out.push_str(&print_lvalue(instance));
                self.out.push('(');
                for (idx, (k, v)) in inputs.iter().enumerate() {
                    if idx > 0 {
                        self.out.push_str(", ");
                    }
                    self.out.push_str(k);
                    self.out.push_str(" := ");
                    self.out.push_str(&print_expr(v));
                }
                self.out.push_str(");");
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                self.out.push_str("IF ");
                self.out.push_str(&print_expr(condition));
                self.out.push_str(" THEN\n");
                let inner = format!("{indent}    ");
                for s in then_body {
                    self.emit_stmt(s, &inner);
                }
                for arm in elif_arms {
                    self.out.push_str(indent);
                    self.out.push_str("ELSIF ");
                    self.out.push_str(&print_expr(&arm.condition));
                    self.out.push_str(" THEN\n");
                    for s in &arm.body {
                        self.emit_stmt(s, &inner);
                    }
                }
                if !else_body.is_empty() {
                    self.out.push_str(indent);
                    self.out.push_str("ELSE\n");
                    for s in else_body {
                        self.emit_stmt(s, &inner);
                    }
                }
                self.out.push_str(indent);
                self.out.push_str("END_IF;");
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                self.out.push_str("CASE ");
                self.out.push_str(&print_expr(scrutinee));
                self.out.push_str(" OF\n");
                let case_ind = format!("{indent}    ");
                let body_ind = format!("{indent}        ");
                for c in cases {
                    self.out.push_str(&case_ind);
                    self.out.push_str("WHEN ");
                    for (idx, p) in c.patterns.iter().enumerate() {
                        if idx > 0 {
                            self.out.push_str(", ");
                        }
                        match p {
                            CasePattern::Value(e) => self.out.push_str(&print_expr(e)),
                            CasePattern::Range { start, end } => {
                                self.out.push_str(&print_expr(start));
                                self.out.push_str("..");
                                self.out.push_str(&print_expr(end));
                            }
                        }
                    }
                    self.out.push_str(":\n");
                    for s in &c.body {
                        self.emit_stmt(s, &body_ind);
                    }
                }
                if let Some(def) = default {
                    self.out.push_str(&case_ind);
                    self.out.push_str("ELSE\n");
                    for s in def {
                        self.emit_stmt(s, &body_ind);
                    }
                }
                self.out.push_str(indent);
                self.out.push_str("END_CASE;");
            }
            StmtKind::While { condition, body } => {
                self.out.push_str("WHILE ");
                self.out.push_str(&print_expr(condition));
                self.out.push_str(" DO\n");
                let inner = format!("{indent}    ");
                for s in body {
                    self.emit_stmt(s, &inner);
                }
                self.out.push_str(indent);
                self.out.push_str("END_WHILE;");
            }
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            } => {
                self.out.push_str("FOR ");
                self.out.push_str(counter);
                self.out.push_str(" := ");
                self.out.push_str(&print_expr(from));
                self.out
                    .push_str(if *inclusive { " TO " } else { " UNTIL " });
                self.out.push_str(&print_expr(to));
                self.out.push_str(" BY ");
                self.out.push_str(&print_expr(step));
                self.out.push_str(" DO\n");
                let inner = format!("{indent}    ");
                for s in body {
                    self.emit_stmt(s, &inner);
                }
                self.out.push_str(indent);
                self.out.push_str("END_FOR;");
            }
            StmtKind::Break => self.out.push_str("EXIT;"),
            StmtKind::Continue => self.out.push_str("CONTINUE;"),
            StmtKind::Return { value } => {
                self.out.push_str("RETURN");
                if let Some(e) = value {
                    self.out.push(' ');
                    self.out.push_str(&print_expr(e));
                }
                self.out.push(';');
            }
            StmtKind::Wait { duration } => {
                self.out.push_str("WAIT ");
                self.out.push_str(&print_expr(duration));
                self.out.push(';');
            }
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => {
                self.out.push_str("FOREIGN");
                self.emit_foreign_payload(language, *fidelity, text, pack_hint.as_deref());
                self.out.push(';');
            }
        }
        let broke = self.emit_trailing_inline(&stmt.id);
        if !broke {
            self.out.push('\n');
        }
    }

    fn emit_chart(&mut self, chart: &SfcChart, indent: &str) {
        if chart.evolve_to_stability {
            self.out.push_str(indent);
            self.out.push_str("STABILITY;\n");
        }
        for step in &chart.steps {
            self.out.push_str(indent);
            self.out.push_str("STEP ");
            self.out.push_str(&step.name);
            self.out.push_str(" (");
            self.out.push_str(step.id.as_str());
            self.out.push(')');
            if step.initial {
                self.out.push_str(" INITIAL");
            }
            if let Some(mc) = &step.macro_chart {
                self.out.push_str(" MACRO ");
                self.out.push_str(mc);
            }
            self.out.push('\n');
            let act_ind = format!("{indent}    ");
            for act in &step.actions {
                self.out.push_str(&act_ind);
                self.out.push_str("ACTION (");
                self.out.push_str(act.id.as_str());
                self.out.push_str(") ");
                self.out.push_str(action_qualifier_str(act.qualifier));
                if let Some(dur) = &act.duration {
                    self.out.push_str(" FOR ");
                    self.out.push_str(&print_expr(dur));
                }
                if let Some(target) = &act.target {
                    self.out.push_str(" TARGET ");
                    self.out.push_str(&print_lvalue(target));
                }
                if act.body.is_empty() {
                    self.out.push_str(";\n");
                } else {
                    self.out.push_str(" DO\n");
                    let body_ind = format!("{act_ind}    ");
                    for s in &act.body {
                        self.emit_stmt(s, &body_ind);
                    }
                    self.out.push_str(&act_ind);
                    self.out.push_str("END_ACTION\n");
                }
            }
            self.out.push_str(indent);
            self.out.push_str("END_STEP\n");
        }
        for tr in &chart.transitions {
            self.out.push_str(indent);
            self.out.push_str("TRANSITION ");
            self.out.push_str(&tr.name);
            self.out.push_str(" (");
            self.out.push_str(tr.id.as_str());
            self.out.push_str(") PRIORITY ");
            self.out.push_str(&tr.priority.to_string());
            self.out.push_str(" FROM (");
            self.out.push_str(&tr.from_steps.join(", "));
            self.out.push_str(") TO (");
            self.out.push_str(&tr.to_steps.join(", "));
            self.out.push_str(") : ");
            self.out.push_str(&print_expr(&tr.receptivity));
            self.out.push_str(";\n");
        }
    }
}

fn format_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Void => "VOID".to_owned(),
        TypeRef::Scalar { ty: s } => match s {
            ScalarType::Bool => "BOOL".to_owned(),
            ScalarType::Sint => "SINT".to_owned(),
            ScalarType::Int => "INT".to_owned(),
            ScalarType::Dint => "DINT".to_owned(),
            ScalarType::Lint => "LINT".to_owned(),
            ScalarType::Usint => "USINT".to_owned(),
            ScalarType::Uint => "UINT".to_owned(),
            ScalarType::Udint => "UDINT".to_owned(),
            ScalarType::Ulint => "ULINT".to_owned(),
            ScalarType::Byte => "BYTE".to_owned(),
            ScalarType::Word => "WORD".to_owned(),
            ScalarType::Dword => "DWORD".to_owned(),
            ScalarType::Lword => "LWORD".to_owned(),
            ScalarType::Real => "REAL".to_owned(),
            ScalarType::Lreal => "LREAL".to_owned(),
            ScalarType::Time => "TIME".to_owned(),
            ScalarType::String { max_len } => format!("STRING[{max_len}]"),
            ScalarType::Analog => "ANALOG".to_owned(),
            ScalarType::Duty => "DUTY".to_owned(),
            ScalarType::Code { codec_id } => format!("CODE<{codec_id}>"),
        },
        TypeRef::Array { elem, len } => format!("ARRAY[{len}] OF {}", format_type_ref(elem)),
        TypeRef::Named { name } => name.clone(),
        TypeRef::Fb { fb_kind } => match fb_kind {
            FbKind::User { fb_name } => format!("FB {fb_name}"),
            FbKind::Ton
            | FbKind::Tof
            | FbKind::Tp
            | FbKind::Ctu
            | FbKind::Ctd
            | FbKind::Ctud
            | FbKind::RTrig
            | FbKind::FTrig
            | FbKind::Sr
            | FbKind::Rs
            | FbKind::UartSend
            | FbKind::UartRecv
            | FbKind::I2cXfer
            | FbKind::SpiXfer
            | FbKind::MbClient
            | FbKind::MbServer => fb_kind_name(fb_kind).to_owned(),
        },
    }
}

const fn hardware_kind_str(kind: HardwareKind) -> &'static str {
    match kind {
        HardwareKind::DigitalIn => "DIGITAL_IN",
        HardwareKind::DigitalOut => "DIGITAL_OUT",
        HardwareKind::DigitalBidirectional => "DIGITAL_BIDIRECTIONAL",
        HardwareKind::AnalogIn => "ANALOG_IN",
        HardwareKind::AnalogOut => "ANALOG_OUT",
        HardwareKind::PwmOut => "PWM_OUT",
        HardwareKind::BusPin => "BUS_PIN",
    }
}

const fn interrupt_edge_str(edge: InterruptEdge) -> &'static str {
    match edge {
        InterruptEdge::Rising => "RISING",
        InterruptEdge::Falling => "FALLING",
        InterruptEdge::Change => "CHANGE",
        InterruptEdge::Low => "LOW",
        InterruptEdge::High => "HIGH",
    }
}

const fn action_qualifier_str(q: ActionQualifier) -> &'static str {
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

fn format_f64(val: f64) -> String {
    let s = format!("{val}");
    if !s.contains('.') && !s.contains('e') && !s.contains('E') {
        format!("{s}.0")
    } else {
        s
    }
}
