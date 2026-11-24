//! Single authoritative versioned board document (`BoardDoc`) and per-view dirty buffer tracking
//! (`04-frontends-visual-and-sync.md` §6).

use crate::command::{UndoRedoOutcome, UndoStack};
use dendri_ast::{
    Board, BoardPresentation, HardwareBinding, LibraryLock, PackLock, Project, ProjectMeta,
};
use dendri_frontend_core::LanguageId;
use dendri_project::{ProjectError, load_project_str};
use dendri_span::{Diagnostic, DiagnosticBag, FileId, Severity};
use dendri_validate::{ProfileCatalog, ValidateOptions, validate_project};
use std::collections::BTreeMap;

/// Canonical alias for a board I/O Tag Table declaration (`[[boards.program.hardware]]`).
pub type HardwareDecl = HardwareBinding;

/// Canonical alias for optional non-executable presentation hints (`[boards.presentation]`).
pub type PresentationStore = BoardPresentation;

/// Uncommitted invalid view buffer retained safely across tab switches until fixed or reverted
/// (`04-frontends-visual-and-sync.md` §6 #2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyViewBuffer {
    /// Logical view or POU tab identifier (e.g., `"cpp:OB_MAIN"`, `"ladder:Conveyor_Ctrl"`).
    pub view_key: String,
    /// Frontend language that produced the diagnostics for this buffer.
    pub language: LanguageId,
    /// Exact user-edited source text retained verbatim without silent clobbering.
    pub source: String,
    /// `BoardDoc.revision` at the time this edit failed parse or validation.
    pub base_revision: u64,
    /// Inline parse and/or validation diagnostics explaining why the edit was not committed.
    pub diagnostics: Vec<Diagnostic>,
}

impl DirtyViewBuffer {
    /// Returns `true` if `BoardDoc` has advanced to a newer revision since this dirty buffer was recorded.
    #[must_use]
    pub const fn is_stale(&self, current_revision: u64) -> bool {
        self.base_revision != current_revision
    }

    /// Number of [`Severity::Error`] diagnostics attached to this dirty buffer.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }
}

/// Single authoritative versioned document per board (`04-frontends-visual-and-sync.md` §6 #1).
///
/// Holds:
/// - `revision`: strictly monotonic revision counter incremented on every committed mutation, undo, and redo.
/// - `board`: authoritative ASL v2 [`Board`] (`program`, `comments`, `assumptions`, `presentation`, metadata).
/// - `dirty_buffers`: per-view uncommitted buffers with inline diagnostics (never silently overwritten on tab switch).
/// - `undo_stack`: unified multi-domain [`UndoStack`] recording atomic [`crate::BoardTransaction`] snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardDoc {
    pub revision: u64,
    pub board: Board,
    pub dirty_buffers: BTreeMap<String, DirtyViewBuffer>,
    pub undo_stack: UndoStack,
    /// External library lock entries carried from the owning [`Project`] for import validation.
    pub libraries: Vec<LibraryLock>,
    /// External pack lock entries carried from the owning [`Project`].
    pub packs: Vec<PackLock>,
}

impl BoardDoc {
    /// Wraps an authoritative [`Board`] at `revision = 0` with empty dirty buffers and undo history.
    #[must_use]
    pub fn new(board: Board) -> Self {
        Self {
            revision: 0,
            board,
            dirty_buffers: BTreeMap::new(),
            undo_stack: UndoStack::new(),
            libraries: Vec::new(),
            packs: Vec::new(),
        }
    }

    /// Wraps a [`Board`] with a bounded undo history depth.
    #[must_use]
    pub fn with_max_undo_depth(board: Board, max_depth: usize) -> Self {
        Self {
            revision: 0,
            board,
            dirty_buffers: BTreeMap::new(),
            undo_stack: UndoStack::with_max_depth(max_depth),
            libraries: Vec::new(),
            packs: Vec::new(),
        }
    }

    /// Extracts a single board by `board_id` from a multi-board [`Project`], preserving library and pack locks.
    #[must_use]
    pub fn from_project(project: &Project, board_id: &str) -> Option<Self> {
        let board = project.boards.iter().find(|b| b.id == board_id)?.clone();
        Some(Self {
            revision: 0,
            board,
            dirty_buffers: BTreeMap::new(),
            undo_stack: UndoStack::new(),
            libraries: project.libraries.clone(),
            packs: project.packs.clone(),
        })
    }

    /// Loads a `.toml` project via `dendri-project` and extracts the [`BoardDoc`] for `board_id`.
    pub fn from_project_toml(
        toml_src: &str,
        board_id: &str,
        file_id: Option<FileId>,
    ) -> Result<Option<Self>, ProjectError> {
        let project = load_project_str(toml_src, file_id)?;
        Ok(Self::from_project(&project, board_id))
    }

    /// Writes `self.board` back into the matching slot of a multi-board [`Project`] without touching
    /// sibling boards, components, wires, libraries, packs, or project-level comments.
    ///
    /// Returns `true` if a board with matching `id` was found and updated.
    pub fn write_back_to_project(&self, project: &mut Project) -> bool {
        if let Some(slot) = project.boards.iter_mut().find(|b| b.id == self.board.id) {
            *slot = self.board.clone();
            true
        } else {
            false
        }
    }

    /// Builds a single-board [`Project`] wrapper suitable for static semantic validation via `dendri-validate`.
    #[must_use]
    pub fn to_validation_project(&self) -> Project {
        build_validation_project(&self.board, &self.libraries, &self.packs)
    }

    /// Runs full `dendri-validate` static semantic checks on the committed `self.board`.
    #[must_use]
    pub fn validate(
        &self,
        catalog: &dyn ProfileCatalog,
        options: &ValidateOptions,
    ) -> DiagnosticBag {
        let project = self.to_validation_project();
        validate_project(&project, catalog, options)
    }

    /// Returns the current monotonic document revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the retained [`DirtyViewBuffer`] for `view_key`, if one exists.
    #[must_use]
    pub fn dirty_buffer(&self, view_key: &str) -> Option<&DirtyViewBuffer> {
        self.dirty_buffers.get(view_key)
    }

    /// Returns `true` iff `view_key` currently has an uncommitted invalid edit buffer.
    #[must_use]
    pub fn has_dirty_buffer(&self, view_key: &str) -> bool {
        self.dirty_buffers.contains_key(view_key)
    }

    /// Returns `true` iff any view currently retains an uncommitted dirty buffer.
    #[must_use]
    pub fn has_any_dirty_buffers(&self) -> bool {
        !self.dirty_buffers.is_empty()
    }

    /// Returns all currently dirty view keys in deterministic sorted order.
    #[must_use]
    pub fn dirty_view_keys(&self) -> Vec<&str> {
        self.dirty_buffers.keys().map(String::as_str).collect()
    }

    /// Explicitly discards and returns the dirty buffer for `view_key` (user-triggered revert).
    ///
    /// Dirty buffers are NEVER cleared automatically on tab switch; they are cleared only when
    /// a subsequent edit on `view_key` succeeds or the user explicitly invokes this method.
    pub fn revert_dirty_buffer(&mut self, view_key: &str) -> Option<DirtyViewBuffer> {
        self.dirty_buffers.remove(view_key)
    }

    /// Explicitly discards and returns all dirty buffers in deterministic key order.
    pub fn revert_all_dirty_buffers(&mut self) -> Vec<DirtyViewBuffer> {
        let buffers = std::mem::take(&mut self.dirty_buffers);
        buffers.into_values().collect()
    }

    /// Records or updates a dirty buffer for `view_key` without mutating `self.board` or `self.revision`.
    pub(crate) fn record_dirty_buffer(
        &mut self,
        view_key: String,
        language: LanguageId,
        source: String,
        diagnostics: Vec<Diagnostic>,
    ) {
        let entry = DirtyViewBuffer {
            view_key: view_key.clone(),
            language,
            source,
            base_revision: self.revision,
            diagnostics,
        };
        self.dirty_buffers.insert(view_key, entry);
    }

    /// Clears a dirty buffer after a valid edit on `view_key` commits cleanly.
    pub(crate) fn clear_dirty_buffer(&mut self, view_key: &str) {
        let _ = self.dirty_buffers.remove(view_key);
    }

    /// Returns `true` iff there is a committed transaction available to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.undo_stack.can_undo()
    }

    /// Returns `true` iff there is an undone transaction available to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.undo_stack.can_redo()
    }

    /// Atomically undoes the latest committed transaction on this document.
    ///
    /// Restores the exact pre-transaction [`Board`], increments `self.revision` monotonically,
    /// and preserves all `self.dirty_buffers` untouched.
    pub fn undo(&mut self) -> Option<UndoRedoOutcome> {
        let tx = self.undo_stack.pop_undo()?;
        self.board = tx.before_board.clone();
        self.revision = self.revision.saturating_add(1);
        let outcome = UndoRedoOutcome {
            transaction_id: tx.id,
            label: tx.label.clone(),
            domain: tx.domain,
            revision: self.revision,
        };
        self.undo_stack.push_redo(tx);
        Some(outcome)
    }

    /// Atomically reapplies the latest undone transaction on this document.
    ///
    /// Restores the exact post-transaction [`Board`], increments `self.revision` monotonically,
    /// and preserves all `self.dirty_buffers` untouched.
    pub fn redo(&mut self) -> Option<UndoRedoOutcome> {
        let tx = self.undo_stack.pop_redo()?;
        self.board = tx.after_board.clone();
        self.revision = self.revision.saturating_add(1);
        let outcome = UndoRedoOutcome {
            transaction_id: tx.id,
            label: tx.label.clone(),
            domain: tx.domain,
            revision: self.revision,
        };
        self.undo_stack.push_past_without_clearing_redo(tx);
        Some(outcome)
    }
}

/// Wraps a candidate [`Board`] with library/pack locks into a minimal [`Project`] for `dendri-validate`.
pub(crate) fn build_validation_project(
    board: &Board,
    libraries: &[LibraryLock],
    packs: &[PackLock],
) -> Project {
    Project {
        meta: ProjectMeta {
            schema_version: "2.0".to_owned(),
            asl_version: "0.1.0".to_owned(),
            requires: vec!["asl-core@1".to_owned()],
            name: format!("sync_{}", board.id),
            author: String::new(),
            description: String::new(),
        },
        libraries: libraries.to_vec(),
        packs: packs.to_vec(),
        comments: Vec::new(),
        boards: vec![board.clone()],
        components: Vec::new(),
        wires: Vec::new(),
    }
}
