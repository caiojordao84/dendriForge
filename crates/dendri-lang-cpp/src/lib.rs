//! 100% pure-Rust Arduino C++ subset frontend (`dendri-lang-cpp`) implementing
//! [`dendri_frontend_core::LanguageFrontend`] (`docs/lbc/cpp.md`, `04-frontends-visual-and-sync.md`).
//!
//! Zero C/FFI dependencies, zero banned machine magic comments, zero `M = 10^9`
//! literal leaks, strict `[cpu] int_bits` fidelity, three-level `Foreign` preservation + `F1` pack
//! resolution, and full ADR-027 verbatim comment conservation (`C1–C5`).

mod board_pins;
mod cpp_ast;
mod emitter;
mod lexer;
mod matrix;
mod parser;
mod sema;
mod tabs;
mod types;

pub use emitter::{emit_codec_conversion_cpp, emit_fixed_adc_read_cpp};
pub use matrix::feature_matrix_entries;
pub use tabs::merge_arduino_tabs;

use dendri_ast::{
    CommentAnchor, CommentEncoding, CommentStyle, ModuleDecl, NodeId, PouBody, Stmt, StmtKind,
    TypeDecl,
};
use dendri_frontend_core::comments::{matches_comment_with_caps, render_comment_for_language};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, CommentPlacement, FrontendOutput, FrontendResult,
    LanguageCapabilities, LanguageFrontend, LanguageId, Paradigm, RenderedComment,
};
use std::collections::BTreeMap;

/// Wave 1 pure-Rust Arduino C++ subset frontend plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CppFrontend {
    comment_encoding: CommentEncoding,
}

impl CppFrontend {
    /// Constructs a [`CppFrontend`] with the default [`CommentEncoding::Utf8Passthrough`] policy.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            comment_encoding: CommentEncoding::Utf8Passthrough,
        }
    }

    /// Constructs a [`CppFrontend`] with an explicit [`CommentEncoding`] policy.
    #[must_use]
    pub const fn with_comment_encoding(comment_encoding: CommentEncoding) -> Self {
        Self { comment_encoding }
    }
}

impl LanguageFrontend for CppFrontend {
    fn id(&self) -> LanguageId {
        LanguageId::Cpp
    }

    fn paradigm(&self) -> Paradigm {
        Paradigm::Textual
    }

    fn parse(&self, src: &str, ctx: &BoardContext) -> FrontendResult<BoardProgramBundle> {
        let lexed = lexer::lex_cpp(src, &ctx.board_id)?;
        let unit = parser::parse_cpp_tokens(src, &lexed.tokens, &ctx.board_id)?;
        sema::lower_and_check(src, &unit, &lexed, ctx)
    }

    fn emit(&self, bundle: &BoardProgramBundle, ctx: &BoardContext) -> FrontendResult<String> {
        emitter::emit_cpp(bundle, ctx)
    }

    fn render_comment(
        &self,
        text: &str,
        style: CommentStyle,
        placement: CommentPlacement,
        ctx: &BoardContext,
    ) -> RenderedComment {
        render_comment_for_language(LanguageId::Cpp, text, style, placement, ctx)
    }

    fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool {
        matches_comment_with_caps(LanguageId::Cpp, &self.capabilities(), rendered, stored_text)
    }

    fn capabilities(&self) -> LanguageCapabilities {
        LanguageCapabilities::cpp(self.comment_encoding)
    }
}

/// Parses a multi-file Arduino sketch containing `.ino` tabs plus optional `.h` / `.cpp` tab pairs
/// (`docs/lbc/cpp.md` §5), mapping `.h` / `.cpp` pairs 1:1 to embedded `[[boards.program.modules]]`
/// and populating `[boards.presentation.origin_files]`.
pub fn parse_multi_file_sketch(
    tabs: &[(&str, &str)],
    ctx: &BoardContext,
) -> FrontendResult<BoardProgramBundle> {
    let frontend = CppFrontend::new();

    let mut ino_tabs: Vec<(&str, &str)> = Vec::new();
    let mut header_tabs: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    let mut cpp_tabs: BTreeMap<String, (&str, &str)> = BTreeMap::new();

    for &(name, content) in tabs {
        if tabs::has_extension_ignore_ascii_case(name, ".ino") {
            ino_tabs.push((name, content));
        } else if tabs::has_extension_ignore_ascii_case(name, ".h")
            || tabs::has_extension_ignore_ascii_case(name, ".hpp")
        {
            let stem = file_stem_ascii(name);
            header_tabs.insert(stem, (name, content));
        } else if tabs::has_extension_ignore_ascii_case(name, ".cpp") {
            let stem = file_stem_ascii(name);
            cpp_tabs.insert(stem, (name, content));
        } else {
            ino_tabs.push((name, content));
        }
    }

    let merged_ino = merge_arduino_tabs(&ino_tabs);
    let mut main_out = frontend.parse(&merged_ino, ctx)?;

    // Parse each `.h` / `.cpp` module unit pair
    let mut mod_stems: Vec<String> = header_tabs.keys().chain(cpp_tabs.keys()).cloned().collect();
    mod_stems.sort();
    mod_stems.dedup();

    for (idx, stem) in mod_stems.into_iter().enumerate() {
        let mut exports = Vec::new();
        let mut combined_mod_src = String::new();
        let mut origin_file = format!("{stem}.cpp");

        if let Some(&(h_name, h_src)) = header_tabs.get(&stem) {
            origin_file = h_name.to_owned();
            if let Ok(h_lexed) = lexer::lex_cpp(h_src, &ctx.board_id)
                && let Ok(h_unit) = parser::parse_cpp_tokens(h_src, &h_lexed.tokens, &ctx.board_id)
            {
                for item in &h_unit.items {
                    match item {
                        cpp_ast::CppTopItem::FunctionProto { name, .. }
                        | cpp_ast::CppTopItem::Function { name, .. }
                        | cpp_ast::CppTopItem::Struct { name, .. }
                        | cpp_ast::CppTopItem::Enum { name, .. }
                        | cpp_ast::CppTopItem::GlobalVar { name, .. }
                        | cpp_ast::CppTopItem::DefineConst { name, .. } => {
                            if !exports.contains(name) {
                                exports.push(name.clone());
                            }
                        }
                        cpp_ast::CppTopItem::Include { .. }
                        | cpp_ast::CppTopItem::ForeignDecl { .. } => {}
                    }
                }
            }
            combined_mod_src.push_str(h_src);
            combined_mod_src.push('\n');
        }
        if let Some(&(c_name, c_src)) = cpp_tabs.get(&stem) {
            origin_file = c_name.to_owned();
            combined_mod_src.push_str(c_src);
        }

        let mut mod_out = frontend.parse(&combined_mod_src, ctx)?;
        main_out.diagnostics.extend(mod_out.diagnostics);

        let mod_num = idx.saturating_add(1);
        let prefix = format!("m{mod_num}_");
        prefix_bundle_ids(&mut mod_out.value, &prefix);

        let mod_id = NodeId::new(format!("n_mod_{mod_num:04}"));
        main_out
            .value
            .presentation
            .origin_files
            .insert(mod_id.clone(), origin_file.clone());
        for pou in &mod_out.value.program.pous {
            main_out
                .value
                .presentation
                .origin_files
                .insert(pou.id.clone(), origin_file.clone());
        }

        main_out.value.program.modules.push(ModuleDecl {
            id: mod_id,
            name: stem,
            imports: mod_out.value.program.imports,
            exports,
            types: mod_out.value.program.types,
            data: mod_out.value.program.data,
            pous: mod_out.value.program.pous,
            span: dendri_span::Span::default(),
        });
        main_out.value.comments.extend(mod_out.value.comments);
    }

    Ok(FrontendOutput::with_diagnostics(
        main_out.value,
        main_out.diagnostics,
    ))
}

fn prefix_node_id(id: &mut NodeId, prefix: &str) {
    *id = NodeId::new(format!("{prefix}{}", id.as_str()));
}

fn prefix_stmts(stmts: &mut [Stmt], prefix: &str) {
    for stmt in stmts {
        prefix_node_id(&mut stmt.id, prefix);
        match &mut stmt.kind {
            StmtKind::If {
                then_body,
                elif_arms,
                else_body,
                ..
            } => {
                prefix_stmts(then_body, prefix);
                for arm in elif_arms {
                    prefix_stmts(&mut arm.body, prefix);
                }
                prefix_stmts(else_body, prefix);
            }
            StmtKind::Switch { cases, default, .. } => {
                for case in cases {
                    prefix_stmts(&mut case.body, prefix);
                }
                if let Some(def) = default {
                    prefix_stmts(def, prefix);
                }
            }
            StmtKind::For { body, .. } | StmtKind::While { body, .. } => {
                prefix_stmts(body, prefix);
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

fn prefix_bundle_ids(bundle: &mut BoardProgramBundle, prefix: &str) {
    for hw in &mut bundle.program.hardware {
        prefix_node_id(&mut hw.id, prefix);
    }
    for td in &mut bundle.program.types {
        match td {
            TypeDecl::Struct { id, .. }
            | TypeDecl::Enum { id, .. }
            | TypeDecl::Foreign { id, .. } => prefix_node_id(id, prefix),
        }
    }
    for d in &mut bundle.program.data {
        prefix_node_id(&mut d.id, prefix);
    }
    for pou in &mut bundle.program.pous {
        prefix_node_id(&mut pou.id, prefix);
        for l in &mut pou.locals {
            prefix_node_id(&mut l.id, prefix);
        }
        match &mut pou.body {
            PouBody::Statements(stmts) => prefix_stmts(stmts, prefix),
            PouBody::Network(net) => {
                for rung in &mut net.rungs {
                    prefix_node_id(&mut rung.id, prefix);
                }
            }
            PouBody::Chart(chart) => {
                for step in &mut chart.steps {
                    prefix_node_id(&mut step.id, prefix);
                    for act in &mut step.actions {
                        prefix_node_id(&mut act.id, prefix);
                    }
                }
                for tr in &mut chart.transitions {
                    prefix_node_id(&mut tr.id, prefix);
                }
            }
        }
    }
    for comment in &mut bundle.comments {
        comment.id = format!("{prefix}{}", comment.id);
        match &mut comment.anchor {
            CommentAnchor::Node { id } => prefix_node_id(id, prefix),
            CommentAnchor::Board { .. }
            | CommentAnchor::Project
            | CommentAnchor::Component { .. }
            | CommentAnchor::Wire { .. } => {}
        }
    }
}

fn file_stem_ascii(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.rsplit_once('.')
        .map_or_else(|| base.to_owned(), |(stem, _)| stem.to_owned())
}
