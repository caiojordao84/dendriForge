//! Language-agnostic bidirectional sync pipeline (`SyncEngine`), cross-language `Foreign`
//! export guard, and validated multi-domain mutation transactions (`04-frontends-visual-and-sync.md` §4.3, §6).

use crate::command::{BoardTransaction, EditDomain, SyncCommand, UndoRedoOutcome};
use crate::doc::{
    BoardDoc, DirtyViewBuffer, HardwareDecl, PresentationStore, build_validation_project,
};
use crate::merge::{
    NodeTopology, collect_board_topology, collect_pou_topology, merge_bundle_into_board,
    orphan_comments_for_deleted_set,
};
use dendri_ast::{
    Assumption, AuthoringLanguage, Board, BoardLayout, BoardProgram, Comment, CommentAnchor,
    CommentPlacement as AstCommentPlacement, DataDecl, EolStyle, NodeId, Pou, TypeDecl, TypeRef,
};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, FrontendOutput, FrontendResult, LanguageFrontend, Paradigm,
    check_program_foreign_export, normalize_eol_to_lf, relocate_inner_comments, restore_eol,
};
use dendri_span::{Diagnostic, ErrorCode, FileId, Severity};
use dendri_validate::{ProfileCatalog, ValidateOptions, validate_project};
use std::collections::BTreeSet;
use thiserror::Error;

/// Configuration parameters controlling validation performed by [`SyncEngine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    /// Static call-graph recursion depth limit passed to `dendri-validate`.
    pub max_call_depth: usize,
    /// Optional [`FileId`] attached to validation diagnostics.
    pub file_id: Option<FileId>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            max_call_depth: 64,
            file_id: None,
        }
    }
}

/// Target scope for [`SyncEngine::export_view`] and [`SyncEngine::project_view_or_dirty`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope<'a> {
    /// Export the entire board program (all POUs, modules, hardware, types, data, comments).
    Board,
    /// Export a single POU matched by exact POU `name` or `NodeId` string.
    Pou(&'a str),
}

/// Result returned when opening/switching to a view tab without clobbering uncommitted edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewProjection {
    /// View has no dirty buffer; freshly projected from the authoritative [`BoardDoc`].
    Clean(FrontendOutput<String>),
    /// View retains an uncommitted invalid buffer with inline diagnostics until fixed or reverted.
    Dirty(DirtyViewBuffer),
}

/// Summary returned upon a committed [`SyncEngine::apply_view_edit`] or [`SyncEngine::apply_command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncApplyReport {
    /// New monotonically incremented `BoardDoc.revision`.
    pub revision: u64,
    /// ID of the [`BoardTransaction`] pushed onto `BoardDoc.undo_stack`.
    pub transaction_id: u64,
    /// Domain of the committed edit.
    pub domain: EditDomain,
    /// Non-fatal warnings and informational diagnostics (e.g. `INFO_COMMENT_ORPHANED`, `WARN_ASSUMPTION_ACTIVE`).
    pub diagnostics: Vec<Diagnostic>,
    /// POU names created or updated by this edit (empty for non-POU table edits).
    pub updated_pous: Vec<String>,
}

/// Structured error returned when an edit or command is rejected without modifying `BoardDoc.board`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SyncEditError {
    /// Frontend failed to parse the view buffer; buffer and diagnostics are retained in `dirty_buffers`.
    #[error("parse failed for view `{view_key}` ({} diagnostic(s))", diagnostics.len())]
    ParseFailed {
        view_key: String,
        diagnostics: Vec<Diagnostic>,
    },
    /// Static semantic validation rejected the candidate board; authoritative board & revision are untouched.
    #[error("validation failed ({} diagnostic(s))", diagnostics.len())]
    ValidationFailed {
        view_key: Option<String>,
        diagnostics: Vec<Diagnostic>,
    },
    /// Targeted entity (`hardware_tag`, `data_decl`, `pou`, `comment`, `assumption`) does not exist.
    #[error("{entity} `{id}` not found")]
    NotFound { entity: String, id: String },
}

impl SyncEditError {
    /// Borrows all structured diagnostics attached to this failure (empty for [`Self::NotFound`]).
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            Self::ParseFailed { diagnostics, .. } | Self::ValidationFailed { diagnostics, .. } => {
                diagnostics
            }
            Self::NotFound { .. } => &[],
        }
    }
}

/// Language-agnostic non-destructive bidirectional sync engine (`04-frontends-visual-and-sync.md` §6).
///
/// Driven strictly through `&dyn LanguageFrontend` so `dendri-sync` has zero dependency on
/// individual language crates.
#[derive(Debug, Clone, Default)]
pub struct SyncEngine {
    config: SyncConfig,
}

impl SyncEngine {
    /// Creates a [`SyncEngine`] with default validation configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a [`SyncEngine`] with explicit [`SyncConfig`].
    #[must_use]
    pub const fn with_config(config: SyncConfig) -> Self {
        Self { config }
    }

    /// Returns the active [`SyncConfig`].
    #[must_use]
    pub const fn config(&self) -> &SyncConfig {
        &self.config
    }

    /// Executes the full bidirectional view edit pipeline:
    /// `Parse -> Non-Destructive Diff/Merge into candidate -> Validate -> Commit & Increment revision`.
    ///
    /// Guarantees:
    /// - Parse or validation failure leaves `doc.board`, `doc.revision`, and `doc.undo_stack`
    ///   completely untouched, and retains `src` + inline diagnostics in `doc.dirty_buffers[view_key]`.
    /// - Successful commit updates ONLY the edited POU(s) while preserving untouched OBs
    ///   (`OB_STARTUP`, `OB_HW_INT`), sibling POUs, `Foreign` nodes, `[[boards.assumptions]]`,
    ///   `[boards.presentation]`, and board metadata.
    /// - Enforces `C4` comment re-import idempotence (`frontend.matches_comment`) and `C2` orphan
    ///   preservation (`INFO_COMMENT_ORPHANED`).
    /// - Clears `doc.dirty_buffers[view_key]` on success without touching other dirty views.
    pub fn apply_view_edit(
        &self,
        doc: &mut BoardDoc,
        view_key: impl Into<String>,
        src: &str,
        frontend: &dyn LanguageFrontend,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        let view_key = view_key.into();

        // 1. Parse strictly via `&dyn LanguageFrontend`
        let parse_out = match frontend.parse(src, ctx) {
            Ok(out) => {
                if out.has_errors() {
                    doc.record_dirty_buffer(
                        view_key.clone(),
                        frontend.id(),
                        src.to_owned(),
                        out.diagnostics.clone(),
                    );
                    return Err(SyncEditError::ParseFailed {
                        view_key,
                        diagnostics: out.diagnostics,
                    });
                }
                out
            }
            Err(diags) => {
                doc.record_dirty_buffer(
                    view_key.clone(),
                    frontend.id(),
                    src.to_owned(),
                    diags.clone(),
                );
                return Err(SyncEditError::ParseFailed {
                    view_key,
                    diagnostics: diags,
                });
            }
        };

        // 2. Non-destructive Diff/Merge into a cloned candidate Board (zero silent clobbering)
        let merge_out = merge_bundle_into_board(&doc.board, &parse_out.value, frontend, ctx);

        // 3. Validate the merged candidate against board profile & semantics before committing
        let val_opts = self.validate_options(ctx);
        let val_project = build_validation_project(&merge_out.board, &doc.libraries, &doc.packs);
        let val_bag = validate_project(&val_project, catalog, &val_opts);

        let all_diags = combine_diagnostics(
            parse_out.diagnostics,
            merge_out.diagnostics,
            val_bag.all(),
            &merge_out.orphaned_comment_ids,
        );

        if val_bag.has_errors() {
            doc.record_dirty_buffer(
                view_key.clone(),
                frontend.id(),
                src.to_owned(),
                all_diags.clone(),
            );
            return Err(SyncEditError::ValidationFailed {
                view_key: Some(view_key),
                diagnostics: all_diags,
            });
        }

        // 4. Commit atomically, clear this view's dirty buffer, bump revision, record transaction
        let before_board = doc.board.clone();
        let before_revision = doc.revision;
        doc.board = merge_out.board;
        doc.revision = doc.revision.saturating_add(1);
        doc.clear_dirty_buffer(&view_key);

        let updated_pous = merge_out.updated_pous;
        let command = SyncCommand::ViewEdit {
            view_key,
            language: frontend.id(),
            paradigm: frontend.paradigm(),
            updated_pous: updated_pous.clone(),
        };
        let domain = command.domain();
        let label = command.describe();
        let tx_id = doc.undo_stack.allocate_id();
        doc.undo_stack.push(BoardTransaction {
            id: tx_id,
            label,
            domain,
            command,
            before_revision,
            after_revision: doc.revision,
            before_board,
            after_board: doc.board.clone(),
            diagnostics: all_diags.clone(),
        });

        Ok(SyncApplyReport {
            revision: doc.revision,
            transaction_id: tx_id,
            domain,
            diagnostics: all_diags,
            updated_pous,
        })
    }

    /// Applies any structured [`SyncCommand`] (I/O Tag Table, Data Table, POUs, Comments,
    /// Assumptions, PresentationStore, Canvas Layout, or atomic `Batch`) through static
    /// validation, monotonic revision bumping, and the unified [`crate::UndoStack`].
    ///
    /// Dirty buffers in open views are NEVER overwritten or cleared by table/metadata commands.
    pub fn apply_command(
        &self,
        doc: &mut BoardDoc,
        command: SyncCommand,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        let mut candidate = doc.board.clone();
        let mut pre_diags = Vec::new();
        let mut orphaned_ids = BTreeSet::new();
        let mut updated_pous = Vec::new();

        apply_command_to_candidate(
            &mut candidate,
            &command,
            &mut pre_diags,
            &mut orphaned_ids,
            &mut updated_pous,
        )?;

        let val_opts = self.validate_options(ctx);
        let val_project = build_validation_project(&candidate, &doc.libraries, &doc.packs);
        let val_bag = validate_project(&val_project, catalog, &val_opts);

        let all_diags = combine_diagnostics(Vec::new(), pre_diags, val_bag.all(), &orphaned_ids);

        if val_bag.has_errors() {
            return Err(SyncEditError::ValidationFailed {
                view_key: None,
                diagnostics: all_diags,
            });
        }

        let before_board = doc.board.clone();
        let before_revision = doc.revision;
        doc.board = candidate;
        doc.revision = doc.revision.saturating_add(1);

        let domain = command.domain();
        let label = command.describe();
        let tx_id = doc.undo_stack.allocate_id();
        doc.undo_stack.push(BoardTransaction {
            id: tx_id,
            label,
            domain,
            command,
            before_revision,
            after_revision: doc.revision,
            before_board,
            after_board: doc.board.clone(),
            diagnostics: all_diags.clone(),
        });

        Ok(SyncApplyReport {
            revision: doc.revision,
            transaction_id: tx_id,
            domain,
            diagnostics: all_diags,
            updated_pous,
        })
    }

    /// Projects `doc` into `frontend`'s target language with strict cross-language `Foreign` export
    /// gating (`check_program_foreign_export`) and `Inner` comment relocation (`INFO_COMMENT_RELOCATED`).
    ///
    /// - Exporting a board/POU containing `Foreign` to a *different* language returns
    ///   four-part `ERR_FOREIGN_EXPORT_BLOCKED` unless `allow_foreign == true`.
    /// - Same-language emission is NEVER blocked regardless of `allow_foreign`.
    /// - Never mutates `doc.board` or `doc.dirty_buffers`.
    pub fn export_view(
        &self,
        doc: &BoardDoc,
        scope: ExportScope<'_>,
        frontend: &dyn LanguageFrontend,
        ctx: &BoardContext,
        allow_foreign: bool,
    ) -> FrontendResult<String> {
        let mut bundle = build_export_bundle(&doc.board, scope)?;

        let mut effective_ctx = ctx.clone();
        effective_ctx.allow_foreign_export = allow_foreign;

        // Enforce cross-language Foreign export gate (same-language is NEVER blocked)
        check_program_foreign_export(&bundle.program, frontend.id(), &effective_ctx)?;

        // Projection-only relocation of Inner comments on cross-language / visual targets
        let (projected_comments, reloc_diags) =
            relocate_inner_comments(&bundle.comments, frontend.id(), Some(&doc.board.id));
        bundle.comments = projected_comments;

        let mut emit_out = frontend.emit(&bundle, &effective_ctx)?;

        // Restore original EOL style on same-language textual emission (`01` §7)
        if frontend.paradigm() == Paradigm::Textual
            && frontend.id().to_authoring_language() == doc.board.authoring_language
        {
            emit_out.value = restore_eol(&emit_out.value, doc.board.presentation.eol_style);
        }

        let mut all_diags = reloc_diags;
        all_diags.extend(emit_out.diagnostics);
        Ok(FrontendOutput::with_diagnostics(emit_out.value, all_diags))
    }

    /// Returns the retained [`DirtyViewBuffer`] if `view_key` is dirty (preventing silent discard
    /// on tab switch), or projects a clean view via [`Self::export_view`] otherwise.
    pub fn project_view_or_dirty(
        &self,
        doc: &BoardDoc,
        view_key: &str,
        scope: ExportScope<'_>,
        frontend: &dyn LanguageFrontend,
        ctx: &BoardContext,
        allow_foreign: bool,
    ) -> Result<ViewProjection, Vec<Diagnostic>> {
        if let Some(dirty) = doc.dirty_buffer(view_key) {
            return Ok(ViewProjection::Dirty(dirty.clone()));
        }
        let clean = self.export_view(doc, scope, frontend, ctx, allow_foreign)?;
        Ok(ViewProjection::Clean(clean))
    }

    /// Atomically undoes the latest transaction on `doc` (monotonic revision, dirty-buffer safe).
    pub fn undo(&self, doc: &mut BoardDoc) -> Option<UndoRedoOutcome> {
        doc.undo()
    }

    /// Atomically redoes the latest undone transaction on `doc` (monotonic revision, dirty-buffer safe).
    pub fn redo(&self, doc: &mut BoardDoc) -> Option<UndoRedoOutcome> {
        doc.redo()
    }

    // -----------------------------------------------------------------------
    // First-Class Domain Mutation Helpers (I/O Tag Table, Data, POUs,
    // Assumptions, Verbatim Comments, PresentationStore, Canvas Layout)
    // -----------------------------------------------------------------------

    /// Adds a hardware I/O tag declaration (`[[boards.program.hardware]]`).
    pub fn add_hardware_tag(
        &self,
        doc: &mut BoardDoc,
        decl: HardwareDecl,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::AddHardwareTag(decl), ctx, catalog)
    }

    /// Updates an existing hardware I/O tag declaration matched by `decl.id`.
    pub fn update_hardware_tag(
        &self,
        doc: &mut BoardDoc,
        decl: HardwareDecl,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::UpdateHardwareTag(decl), ctx, catalog)
    }

    /// Removes a hardware I/O tag declaration by `id` while preserving its comments (`INFO_COMMENT_ORPHANED`).
    pub fn remove_hardware_tag(
        &self,
        doc: &mut BoardDoc,
        id: NodeId,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::RemoveHardwareTag { id }, ctx, catalog)
    }

    /// Inserts or updates a global board variable / FB instance (`[[boards.program.data]]`).
    pub fn upsert_data_decl(
        &self,
        doc: &mut BoardDoc,
        decl: DataDecl,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::UpsertDataDecl(decl), ctx, catalog)
    }

    /// Removes a global board variable by `id` while preserving its comments (`INFO_COMMENT_ORPHANED`).
    pub fn remove_data_decl(
        &self,
        doc: &mut BoardDoc,
        id: NodeId,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::RemoveDataDecl { id }, ctx, catalog)
    }

    /// Inserts or updates a POU structurally while preserving all sibling POUs.
    pub fn upsert_pou(
        &self,
        doc: &mut BoardDoc,
        pou: Pou,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::UpsertPou(pou), ctx, catalog)
    }

    /// Explicitly removes a POU by `id`, preserving all comments inside it (`INFO_COMMENT_ORPHANED`).
    pub fn remove_pou(
        &self,
        doc: &mut BoardDoc,
        id: NodeId,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::RemovePou { id }, ctx, catalog)
    }

    /// Updates a POU's per-POU `preferred_view`.
    pub fn set_pou_preferred_view(
        &self,
        doc: &mut BoardDoc,
        pou_id: NodeId,
        preferred_view: Option<AuthoringLanguage>,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(
            doc,
            SyncCommand::SetPouPreferredView {
                pou_id,
                preferred_view,
            },
            ctx,
            catalog,
        )
    }

    /// Records or updates an explicit simulation assumption (`[[boards.assumptions]]`).
    pub fn record_assumption(
        &self,
        doc: &mut BoardDoc,
        assumption: Assumption,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::RecordAssumption(assumption), ctx, catalog)
    }

    /// Removes a simulation assumption targeting `(node, expr_ordinal)`.
    pub fn remove_assumption(
        &self,
        doc: &mut BoardDoc,
        node: NodeId,
        expr_ordinal: Option<u32>,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(
            doc,
            SyncCommand::RemoveAssumption { node, expr_ordinal },
            ctx,
            catalog,
        )
    }

    /// Appends a new first-class verbatim comment (`[[boards.comments]]`).
    pub fn add_comment(
        &self,
        doc: &mut BoardDoc,
        comment: Comment,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::AddComment(comment), ctx, catalog)
    }

    /// Updates a comment's verbatim text by `id`.
    pub fn update_comment_text(
        &self,
        doc: &mut BoardDoc,
        id: impl Into<String>,
        text: impl Into<String>,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(
            doc,
            SyncCommand::UpdateCommentText {
                id: id.into(),
                text: text.into(),
            },
            ctx,
            catalog,
        )
    }

    /// Re-anchors or updates placement of an existing comment (e.g. adopting a `dangling` comment).
    #[allow(clippy::too_many_arguments)]
    pub fn reanchor_comment(
        &self,
        doc: &mut BoardDoc,
        id: impl Into<String>,
        anchor: CommentAnchor,
        placement: AstCommentPlacement,
        token_ordinal: Option<u32>,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(
            doc,
            SyncCommand::ReanchorComment {
                id: id.into(),
                anchor,
                placement,
                token_ordinal,
            },
            ctx,
            catalog,
        )
    }

    /// Explicitly removes a comment by `id` (e.g., user action in Comments Panel).
    pub fn remove_comment(
        &self,
        doc: &mut BoardDoc,
        id: impl Into<String>,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(
            doc,
            SyncCommand::RemoveComment { id: id.into() },
            ctx,
            catalog,
        )
    }

    /// Updates the board's preserved same-language EOL style in `[boards.presentation]`.
    pub fn set_eol_style(
        &self,
        doc: &mut BoardDoc,
        eol_style: EolStyle,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::SetEolStyle(eol_style), ctx, catalog)
    }

    /// Updates optional `blank_lines_before` spacing (`0..=2`) for `node` in [`PresentationStore`].
    pub fn set_blank_lines_before(
        &self,
        doc: &mut BoardDoc,
        node: NodeId,
        blank_lines: Option<u8>,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(
            doc,
            SyncCommand::SetBlankLinesBefore { node, blank_lines },
            ctx,
            catalog,
        )
    }

    /// Updates multi-file `origin_files` path for `node` in [`PresentationStore`].
    pub fn set_origin_file(
        &self,
        doc: &mut BoardDoc,
        node: NodeId,
        path: Option<String>,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::SetOriginFile { node, path }, ctx, catalog)
    }

    /// Updates visual diagram `(x, y)` coordinates for `node` in [`PresentationStore`].
    pub fn set_node_coords(
        &self,
        doc: &mut BoardDoc,
        node: NodeId,
        coords: Option<[f64; 2]>,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(
            doc,
            SyncCommand::SetNodeCoords { node, coords },
            ctx,
            catalog,
        )
    }

    /// Replaces the non-executable [`PresentationStore`] atomically (never touches comments or ASL).
    pub fn replace_presentation(
        &self,
        doc: &mut BoardDoc,
        presentation: PresentationStore,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(
            doc,
            SyncCommand::ReplacePresentation(presentation),
            ctx,
            catalog,
        )
    }

    /// Updates the board's canvas placement metadata (`[boards.layout]`).
    pub fn update_board_layout(
        &self,
        doc: &mut BoardDoc,
        layout: BoardLayout,
        ctx: &BoardContext,
        catalog: &dyn ProfileCatalog,
    ) -> Result<SyncApplyReport, SyncEditError> {
        self.apply_command(doc, SyncCommand::UpdateBoardLayout(layout), ctx, catalog)
    }

    fn validate_options(&self, ctx: &BoardContext) -> ValidateOptions {
        ValidateOptions {
            file_id: self.config.file_id,
            max_call_depth: self.config.max_call_depth,
            comment_encoding: ctx.comment_encoding,
            export_target_language: None,
            allow_foreign_export: ctx.allow_foreign_export,
        }
    }
}

fn apply_command_to_candidate(
    candidate: &mut Board,
    command: &SyncCommand,
    pre_diags: &mut Vec<Diagnostic>,
    orphaned_ids: &mut BTreeSet<String>,
    updated_pous: &mut Vec<String>,
) -> Result<(), SyncEditError> {
    match command {
        SyncCommand::ViewEdit { view_key, .. } => {
            return Err(SyncEditError::ValidationFailed {
                view_key: Some(view_key.clone()),
                diagnostics: vec![
                    Diagnostic::new(
                        ErrorCode::ErrUnknownField,
                        "SyncCommand::ViewEdit must be applied through SyncEngine::apply_view_edit",
                    )
                    .with_board_id(&candidate.id),
                ],
            });
        }
        SyncCommand::AddHardwareTag(decl) => {
            candidate.program.hardware.push(decl.clone());
        }
        SyncCommand::UpdateHardwareTag(decl) => {
            if let Some(slot) = candidate
                .program
                .hardware
                .iter_mut()
                .find(|h| h.id == decl.id)
            {
                *slot = decl.clone();
            } else {
                return Err(SyncEditError::NotFound {
                    entity: "hardware_tag".to_owned(),
                    id: decl.id.to_string(),
                });
            }
        }
        SyncCommand::RemoveHardwareTag { id } => {
            let before_topo = collect_board_topology(candidate);
            let Some(pos) = candidate.program.hardware.iter().position(|h| &h.id == id) else {
                return Err(SyncEditError::NotFound {
                    entity: "hardware_tag".to_owned(),
                    id: id.to_string(),
                });
            };
            candidate.program.hardware.remove(pos);
            let after_topo = collect_board_topology(candidate);
            let deleted: BTreeSet<NodeId> = [id.clone()].into_iter().collect();
            let (diags, newly_orphaned) = orphan_comments_for_deleted_set(
                &mut candidate.comments,
                &deleted,
                &before_topo.parent_of,
                &after_topo.all_nodes,
                &candidate.id,
            );
            pre_diags.extend(diags);
            orphaned_ids.extend(newly_orphaned);
            candidate.assumptions.retain(|a| &a.node != id);
        }
        SyncCommand::UpsertDataDecl(decl) => {
            let before_topo = collect_board_topology(candidate);
            let replaced_id = if let Some(slot) = candidate
                .program
                .data
                .iter_mut()
                .find(|d| d.id == decl.id || d.name == decl.name)
            {
                let old_id = slot.id.clone();
                *slot = decl.clone();
                (old_id != decl.id).then_some(old_id)
            } else {
                candidate.program.data.push(decl.clone());
                None
            };
            if let Some(old_id) = replaced_id {
                let after_topo = collect_board_topology(candidate);
                let deleted: BTreeSet<NodeId> = [old_id.clone()].into_iter().collect();
                let (diags, newly_orphaned) = orphan_comments_for_deleted_set(
                    &mut candidate.comments,
                    &deleted,
                    &before_topo.parent_of,
                    &after_topo.all_nodes,
                    &candidate.id,
                );
                pre_diags.extend(diags);
                orphaned_ids.extend(newly_orphaned);
                candidate.assumptions.retain(|a| a.node != old_id);
            }
        }
        SyncCommand::RemoveDataDecl { id } => {
            let before_topo = collect_board_topology(candidate);
            let Some(pos) = candidate.program.data.iter().position(|d| &d.id == id) else {
                return Err(SyncEditError::NotFound {
                    entity: "data_decl".to_owned(),
                    id: id.to_string(),
                });
            };
            candidate.program.data.remove(pos);
            let after_topo = collect_board_topology(candidate);
            let deleted: BTreeSet<NodeId> = [id.clone()].into_iter().collect();
            let (diags, newly_orphaned) = orphan_comments_for_deleted_set(
                &mut candidate.comments,
                &deleted,
                &before_topo.parent_of,
                &after_topo.all_nodes,
                &candidate.id,
            );
            pre_diags.extend(diags);
            orphaned_ids.extend(newly_orphaned);
            candidate.assumptions.retain(|a| &a.node != id);
        }
        SyncCommand::UpsertPou(pou) => {
            let before_topo = collect_board_topology(candidate);
            let mut old_pou_topo = NodeTopology::default();
            let mut replaced = false;

            if let Some(slot) = candidate
                .program
                .pous
                .iter_mut()
                .find(|p| p.id == pou.id || p.name == pou.name)
            {
                collect_pou_topology(slot, None, &mut old_pou_topo);
                *slot = pou.clone();
                replaced = true;
            } else {
                for module in &mut candidate.program.modules {
                    let mod_id = module.id.clone();
                    if let Some(slot) = module
                        .pous
                        .iter_mut()
                        .find(|p| p.id == pou.id || p.name == pou.name)
                    {
                        collect_pou_topology(slot, Some(&mod_id), &mut old_pou_topo);
                        *slot = pou.clone();
                        replaced = true;
                        break;
                    }
                }
            }

            if !replaced {
                candidate.program.pous.push(pou.clone());
            }
            updated_pous.push(pou.name.clone());

            if replaced {
                let after_topo = collect_board_topology(candidate);
                let deleted: BTreeSet<NodeId> = old_pou_topo
                    .all_nodes
                    .into_iter()
                    .filter(|nid| !after_topo.all_nodes.contains(nid))
                    .collect();
                if !deleted.is_empty() {
                    let (diags, newly_orphaned) = orphan_comments_for_deleted_set(
                        &mut candidate.comments,
                        &deleted,
                        &before_topo.parent_of,
                        &after_topo.all_nodes,
                        &candidate.id,
                    );
                    pre_diags.extend(diags);
                    orphaned_ids.extend(newly_orphaned);
                    candidate.assumptions.retain(|a| !deleted.contains(&a.node));
                }
            }
        }
        SyncCommand::RemovePou { id } => {
            let before_topo = collect_board_topology(candidate);
            let mut removed_topo = NodeTopology::default();
            let mut removed = false;

            if let Some(pos) = candidate.program.pous.iter().position(|p| &p.id == id) {
                let removed_pou = candidate.program.pous.remove(pos);
                collect_pou_topology(&removed_pou, None, &mut removed_topo);
                removed = true;
            } else {
                for module in &mut candidate.program.modules {
                    let mod_id = module.id.clone();
                    if let Some(pos) = module.pous.iter().position(|p| &p.id == id) {
                        let removed_pou = module.pous.remove(pos);
                        collect_pou_topology(&removed_pou, Some(&mod_id), &mut removed_topo);
                        removed = true;
                        break;
                    }
                }
            }

            if !removed {
                return Err(SyncEditError::NotFound {
                    entity: "pou".to_owned(),
                    id: id.to_string(),
                });
            }

            let after_topo = collect_board_topology(candidate);
            let (diags, newly_orphaned) = orphan_comments_for_deleted_set(
                &mut candidate.comments,
                &removed_topo.all_nodes,
                &before_topo.parent_of,
                &after_topo.all_nodes,
                &candidate.id,
            );
            pre_diags.extend(diags);
            orphaned_ids.extend(newly_orphaned);
            candidate
                .assumptions
                .retain(|a| !removed_topo.all_nodes.contains(&a.node));
        }
        SyncCommand::SetPouPreferredView {
            pou_id,
            preferred_view,
        } => {
            let mut found = false;
            if let Some(pou) = candidate.program.pous.iter_mut().find(|p| &p.id == pou_id) {
                pou.preferred_view = *preferred_view;
                updated_pous.push(pou.name.clone());
                found = true;
            } else {
                for module in &mut candidate.program.modules {
                    if let Some(pou) = module.pous.iter_mut().find(|p| &p.id == pou_id) {
                        pou.preferred_view = *preferred_view;
                        updated_pous.push(pou.name.clone());
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return Err(SyncEditError::NotFound {
                    entity: "pou".to_owned(),
                    id: pou_id.to_string(),
                });
            }
        }
        SyncCommand::RecordAssumption(assumption) => {
            if let Some(slot) = candidate
                .assumptions
                .iter_mut()
                .find(|a| a.node == assumption.node && a.expr_ordinal == assumption.expr_ordinal)
            {
                *slot = assumption.clone();
            } else {
                candidate.assumptions.push(assumption.clone());
            }
        }
        SyncCommand::RemoveAssumption { node, expr_ordinal } => {
            let Some(pos) = candidate
                .assumptions
                .iter()
                .position(|a| &a.node == node && a.expr_ordinal == *expr_ordinal)
            else {
                let id_desc = match expr_ordinal {
                    Some(ord) => format!("{node}#{ord}"),
                    None => node.to_string(),
                };
                return Err(SyncEditError::NotFound {
                    entity: "assumption".to_owned(),
                    id: id_desc,
                });
            };
            candidate.assumptions.remove(pos);
        }
        SyncCommand::AddComment(comment) => {
            let mut normalized = comment.clone();
            normalized.text = normalize_eol_to_lf(&normalized.text);
            candidate.comments.push(normalized);
        }
        SyncCommand::UpdateCommentText { id, text } => {
            if let Some(slot) = candidate.comments.iter_mut().find(|c| &c.id == id) {
                slot.text = normalize_eol_to_lf(text);
            } else {
                return Err(SyncEditError::NotFound {
                    entity: "comment".to_owned(),
                    id: id.clone(),
                });
            }
        }
        SyncCommand::ReanchorComment {
            id,
            anchor,
            placement,
            token_ordinal,
        } => {
            if let Some(slot) = candidate.comments.iter_mut().find(|c| &c.id == id) {
                slot.anchor = anchor.clone();
                slot.placement = *placement;
                slot.token_ordinal = *token_ordinal;
            } else {
                return Err(SyncEditError::NotFound {
                    entity: "comment".to_owned(),
                    id: id.clone(),
                });
            }
        }
        SyncCommand::RemoveComment { id } => {
            let Some(pos) = candidate.comments.iter().position(|c| &c.id == id) else {
                return Err(SyncEditError::NotFound {
                    entity: "comment".to_owned(),
                    id: id.clone(),
                });
            };
            candidate.comments.remove(pos);
        }
        SyncCommand::SetEolStyle(eol) => {
            candidate.presentation.eol_style = *eol;
        }
        SyncCommand::SetBlankLinesBefore { node, blank_lines } => match blank_lines {
            Some(lines) => {
                candidate
                    .presentation
                    .blank_lines_before
                    .insert(node.clone(), (*lines).min(2));
            }
            None => {
                let _ = candidate.presentation.blank_lines_before.remove(node);
            }
        },
        SyncCommand::SetOriginFile { node, path } => match path {
            Some(p) => {
                candidate
                    .presentation
                    .origin_files
                    .insert(node.clone(), p.clone());
            }
            None => {
                let _ = candidate.presentation.origin_files.remove(node);
            }
        },
        SyncCommand::SetNodeCoords { node, coords } => match coords {
            Some(xy) => {
                candidate.presentation.node_coords.insert(node.clone(), *xy);
            }
            None => {
                let _ = candidate.presentation.node_coords.remove(node);
            }
        },
        SyncCommand::ReplacePresentation(pres) => {
            let mut clamped = pres.clone();
            for val in clamped.blank_lines_before.values_mut() {
                *val = (*val).min(2);
            }
            candidate.presentation = clamped;
        }
        SyncCommand::UpdateBoardLayout(layout) => {
            candidate.layout = layout.clone();
        }
        SyncCommand::Batch { label: _, commands } => {
            for sub in commands {
                apply_command_to_candidate(candidate, sub, pre_diags, orphaned_ids, updated_pous)?;
            }
        }
    }
    Ok(())
}

/// Combines parse, merge, and validation diagnostics while deduplicating redundant
/// `INFO_COMMENT_ORPHANED` messages already emitted with richer node-deletion context.
fn combine_diagnostics(
    parse_diags: Vec<Diagnostic>,
    merge_diags: Vec<Diagnostic>,
    validation_diags: &[Diagnostic],
    newly_orphaned_comment_ids: &BTreeSet<String>,
) -> Vec<Diagnostic> {
    let mut all = Vec::with_capacity(
        parse_diags
            .len()
            .saturating_add(merge_diags.len())
            .saturating_add(validation_diags.len()),
    );
    all.extend(parse_diags);
    all.extend(merge_diags);
    for diag in validation_diags {
        if diag.code == ErrorCode::InfoCommentOrphaned
            && diag.severity == Severity::Info
            && newly_orphaned_comment_ids
                .iter()
                .any(|cid| diag.message.contains(&format!("`{cid}`")))
        {
            continue;
        }
        all.push(diag.clone());
    }
    all
}

fn build_export_bundle(
    board: &Board,
    scope: ExportScope<'_>,
) -> Result<BoardProgramBundle, Vec<Diagnostic>> {
    match scope {
        ExportScope::Board => Ok(BoardProgramBundle::from_board(board)),
        ExportScope::Pou(target) => {
            let found_pou = board
                .program
                .pous
                .iter()
                .find(|p| p.name == target || p.id.as_str() == target)
                .or_else(|| {
                    board.program.modules.iter().find_map(|m| {
                        m.pous
                            .iter()
                            .find(|p| p.name == target || p.id.as_str() == target)
                    })
                });

            let Some(pou) = found_pou else {
                return Err(vec![
                    Diagnostic::new(
                        ErrorCode::ErrUndeclaredSymbol,
                        format!("POU `{target}` not found on board `{}`", board.id),
                    )
                    .with_board_id(&board.id),
                ]);
            };

            let mut pou_topo = NodeTopology::default();
            collect_pou_topology(pou, None, &mut pou_topo);

            let referenced_type_names = collect_pou_named_types(pou);
            let scoped_types: Vec<TypeDecl> = board
                .program
                .types
                .iter()
                .filter(|td| match td {
                    TypeDecl::Struct { .. } | TypeDecl::Enum { .. } => true,
                    TypeDecl::Foreign { name, .. } => name
                        .as_deref()
                        .is_some_and(|n| referenced_type_names.contains(n)),
                })
                .cloned()
                .collect();

            let scoped_comments: Vec<Comment> = board
                .comments
                .iter()
                .filter(|c| match &c.anchor {
                    CommentAnchor::Node { id } => pou_topo.all_nodes.contains(id),
                    CommentAnchor::Project
                    | CommentAnchor::Board { .. }
                    | CommentAnchor::Component { .. }
                    | CommentAnchor::Wire { .. } => false,
                })
                .cloned()
                .collect();

            let scoped_assumptions: Vec<Assumption> = board
                .assumptions
                .iter()
                .filter(|a| pou_topo.all_nodes.contains(&a.node))
                .cloned()
                .collect();

            Ok(BoardProgramBundle {
                program: BoardProgram {
                    imports: board.program.imports.clone(),
                    hardware: board.program.hardware.clone(),
                    types: scoped_types,
                    data: board.program.data.clone(),
                    modules: Vec::new(),
                    pous: vec![pou.clone()],
                },
                comments: scoped_comments,
                assumptions: scoped_assumptions,
                presentation: board.presentation.clone(),
            })
        }
    }
}

fn collect_pou_named_types(pou: &Pou) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for param in &pou.params {
        collect_type_ref_names(&param.ty, &mut names);
    }
    for local in &pou.locals {
        collect_type_ref_names(&local.ty, &mut names);
    }
    if let dendri_ast::PouKind::Fc { return_type } = &pou.kind {
        collect_type_ref_names(return_type, &mut names);
    }
    names
}

fn collect_type_ref_names(ty: &TypeRef, out: &mut BTreeSet<String>) {
    match ty {
        TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Fb { .. } => {}
        TypeRef::Named { name } => {
            out.insert(name.clone());
        }
        TypeRef::Array { elem, len: _ } => collect_type_ref_names(elem, out),
    }
}
