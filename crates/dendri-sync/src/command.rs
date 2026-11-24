//! Unified multi-domain Command, Transaction, and Undo/Redo stack (`04-frontends-visual-and-sync.md` §6 #2).

use crate::doc::{BoardDoc, HardwareDecl, PresentationStore};
use dendri_ast::{
    Assumption, AuthoringLanguage, Board, BoardLayout, Comment, CommentAnchor,
    CommentPlacement as AstCommentPlacement, DataDecl, EolStyle, NodeId, Pou,
};
use dendri_frontend_core::{LanguageId, Paradigm};
use dendri_span::Diagnostic;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Authoring surface / domain that originated a committed [`BoardTransaction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditDomain {
    /// Textual code views (Arduino C++, IEC 61131-3 ST, etc.).
    Code,
    /// IEC 61131-3 Ladder Diagram visual editor.
    Ladder,
    /// IEC 60848 GRAFCET / SFC visual editor.
    Grafcet,
    /// Board Hardware I/O Tag Table (`[[boards.program.hardware]]`).
    IoTagTable,
    /// Board Global Data Table (`[[boards.program.data]]`).
    DataTable,
    /// First-class verbatim human comments (`[[boards.comments]]`).
    Comments,
    /// Explicit simulation assumptions (`[[boards.assumptions]]`).
    Assumptions,
    /// Non-executable presentation metadata (`[boards.presentation]`).
    Presentation,
    /// Canvas board placement / layout metadata (`[boards.layout]`).
    Canvas,
    /// Structural POU management (adding/removing POUs or changing `preferred_view`).
    Structure,
    /// Atomic multi-command batch spanning one or more domains.
    Composite,
}

impl EditDomain {
    /// Canonical lowercase identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Ladder => "ladder",
            Self::Grafcet => "grafcet",
            Self::IoTagTable => "io_tag_table",
            Self::DataTable => "data_table",
            Self::Comments => "comments",
            Self::Assumptions => "assumptions",
            Self::Presentation => "presentation",
            Self::Canvas => "canvas",
            Self::Structure => "structure",
            Self::Composite => "composite",
        }
    }

    /// Maps a [`Paradigm`] returned by `&dyn LanguageFrontend` to its corresponding [`EditDomain`].
    #[must_use]
    pub const fn from_paradigm(paradigm: Paradigm) -> Self {
        match paradigm {
            Paradigm::Textual => Self::Code,
            Paradigm::Ladder => Self::Ladder,
            Paradigm::Grafcet => Self::Grafcet,
        }
    }
}

impl fmt::Display for EditDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// First-class mutation command recorded in every [`BoardTransaction`].
#[derive(Debug, Clone, PartialEq)]
pub enum SyncCommand {
    /// Committed bidirectional edit from a [`dendri_frontend_core::LanguageFrontend`] view.
    ViewEdit {
        view_key: String,
        language: LanguageId,
        paradigm: Paradigm,
        updated_pous: Vec<String>,
    },
    /// Adds a new hardware I/O tag binding to `[[boards.program.hardware]]`.
    AddHardwareTag(HardwareDecl),
    /// Updates an existing hardware I/O tag binding matched by `id`.
    UpdateHardwareTag(HardwareDecl),
    /// Removes a hardware I/O tag binding by `id`, non-destructively preserving its comments.
    RemoveHardwareTag { id: NodeId },
    /// Inserts or updates a global board variable / FB instance in `[[boards.program.data]]`.
    UpsertDataDecl(DataDecl),
    /// Removes a global board variable by `id`, non-destructively preserving its comments.
    RemoveDataDecl { id: NodeId },
    /// Inserts or replaces a POU by `id` (or `name`) while preserving all sibling POUs.
    UpsertPou(Pou),
    /// Explicitly removes a POU by `id`, non-destructively preserving all comments inside it.
    RemovePou { id: NodeId },
    /// Updates a POU's per-POU `preferred_view` (`cpp | st | ladder | sfc`).
    SetPouPreferredView {
        pou_id: NodeId,
        preferred_view: Option<AuthoringLanguage>,
    },
    /// Records or updates an explicit simulation assumption in `[[boards.assumptions]]`.
    RecordAssumption(Assumption),
    /// Removes a simulation assumption identified by `(node, expr_ordinal)`.
    RemoveAssumption {
        node: NodeId,
        expr_ordinal: Option<u32>,
    },
    /// Appends a new first-class verbatim comment to `[[boards.comments]]`.
    AddComment(Comment),
    /// Updates a comment's verbatim text (LF line endings, zero trimming/normalization).
    UpdateCommentText { id: String, text: String },
    /// Re-anchors or updates placement of an existing comment (e.g., adopting a `dangling` comment).
    ReanchorComment {
        id: String,
        anchor: CommentAnchor,
        placement: AstCommentPlacement,
        token_ordinal: Option<u32>,
    },
    /// Explicitly deletes a comment by `id` (e.g., from the Comments Panel).
    RemoveComment { id: String },
    /// Updates the preserved same-language EOL style in `[boards.presentation]`.
    SetEolStyle(EolStyle),
    /// Sets or clears optional `blank_lines_before` spacing (`0..=2`) for `node`.
    SetBlankLinesBefore {
        node: NodeId,
        blank_lines: Option<u8>,
    },
    /// Sets or clears multi-file `origin_files` path for `node`.
    SetOriginFile { node: NodeId, path: Option<String> },
    /// Sets or clears visual diagram `(x, y)` coordinates for `node`.
    SetNodeCoords {
        node: NodeId,
        coords: Option<[f64; 2]>,
    },
    /// Replaces the non-executable [`PresentationStore`] atomically (never touches comments or ASL).
    ReplacePresentation(PresentationStore),
    /// Updates the board's canvas placement/layout (`[boards.layout]`).
    UpdateBoardLayout(BoardLayout),
    /// Applies multiple commands atomically inside one validated transaction and one undo step.
    Batch {
        label: String,
        commands: Vec<SyncCommand>,
    },
}

impl SyncCommand {
    /// Returns the primary [`EditDomain`] associated with this command.
    #[must_use]
    pub const fn domain(&self) -> EditDomain {
        match self {
            Self::ViewEdit { paradigm, .. } => EditDomain::from_paradigm(*paradigm),
            Self::AddHardwareTag(_)
            | Self::UpdateHardwareTag(_)
            | Self::RemoveHardwareTag { .. } => EditDomain::IoTagTable,
            Self::UpsertDataDecl(_) | Self::RemoveDataDecl { .. } => EditDomain::DataTable,
            Self::UpsertPou(_) | Self::RemovePou { .. } | Self::SetPouPreferredView { .. } => {
                EditDomain::Structure
            }
            Self::RecordAssumption(_) | Self::RemoveAssumption { .. } => EditDomain::Assumptions,
            Self::AddComment(_)
            | Self::UpdateCommentText { .. }
            | Self::ReanchorComment { .. }
            | Self::RemoveComment { .. } => EditDomain::Comments,
            Self::SetEolStyle(_)
            | Self::SetBlankLinesBefore { .. }
            | Self::SetOriginFile { .. }
            | Self::SetNodeCoords { .. }
            | Self::ReplacePresentation(_) => EditDomain::Presentation,
            Self::UpdateBoardLayout(_) => EditDomain::Canvas,
            Self::Batch { .. } => EditDomain::Composite,
        }
    }

    /// Produces a concise human-readable label for Undo/Redo menus and audit logs.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::ViewEdit {
                view_key,
                language,
                updated_pous,
                ..
            } => {
                if updated_pous.is_empty() {
                    format!("Edit view `{view_key}` ({language})")
                } else {
                    format!("Edit {} ({language})", updated_pous.join(", "))
                }
            }
            Self::AddHardwareTag(hw) => format!("Add hardware tag `{}` on `{}`", hw.label, hw.pin),
            Self::UpdateHardwareTag(hw) => {
                format!("Update hardware tag `{}` ({})", hw.label, hw.id)
            }
            Self::RemoveHardwareTag { id } => format!("Remove hardware tag `{id}`"),
            Self::UpsertDataDecl(decl) => format!("Upsert data `{}`", decl.name),
            Self::RemoveDataDecl { id } => format!("Remove data `{id}`"),
            Self::UpsertPou(pou) => format!("Upsert POU `{}`", pou.name),
            Self::RemovePou { id } => format!("Remove POU `{id}`"),
            Self::SetPouPreferredView {
                pou_id,
                preferred_view,
            } => match preferred_view {
                Some(view) => format!("Set preferred view of `{pou_id}` to `{}`", view.as_str()),
                None => format!("Clear preferred view of `{pou_id}`"),
            },
            Self::RecordAssumption(a) => format!("Record assumption on `{}`", a.node),
            Self::RemoveAssumption { node, expr_ordinal } => match expr_ordinal {
                Some(ord) => format!("Remove assumption on `{node}`#{ord}"),
                None => format!("Remove assumption on `{node}`"),
            },
            Self::AddComment(c) => format!("Add comment `{}`", c.id),
            Self::UpdateCommentText { id, .. } => format!("Edit comment `{id}`"),
            Self::ReanchorComment { id, .. } => format!("Re-anchor comment `{id}`"),
            Self::RemoveComment { id } => format!("Delete comment `{id}`"),
            Self::SetEolStyle(eol) => format!("Set EOL style to `{eol:?}`"),
            Self::SetBlankLinesBefore { node, blank_lines } => match blank_lines {
                Some(n) => format!("Set blank lines before `{node}` to {n}"),
                None => format!("Clear blank lines before `{node}`"),
            },
            Self::SetOriginFile { node, path } => match path {
                Some(p) => format!("Set origin file of `{node}` to `{p}`"),
                None => format!("Clear origin file of `{node}`"),
            },
            Self::SetNodeCoords { node, coords } => match coords {
                Some(_) => format!("Update coordinates of `{node}`"),
                None => format!("Clear coordinates of `{node}`"),
            },
            Self::ReplacePresentation(_) => "Update board presentation hints".to_owned(),
            Self::UpdateBoardLayout(_) => "Update board canvas layout".to_owned(),
            Self::Batch { label, .. } => label.clone(),
        }
    }
}

/// Atomic versioned transaction stored in [`UndoStack`].
#[derive(Debug, Clone, PartialEq)]
pub struct BoardTransaction {
    /// Strictly increasing transaction identifier within the owning [`UndoStack`].
    pub id: u64,
    /// Human-readable label describing the committed edit.
    pub label: String,
    /// Originating domain (Code, Ladder, GRAFCET, I/O Tag Table, Comments, Assumptions, Presentation, Canvas).
    pub domain: EditDomain,
    /// Exact [`SyncCommand`] executed to produce `after_board` from `before_board`.
    pub command: SyncCommand,
    /// `BoardDoc.revision` immediately prior to committing this transaction.
    pub before_revision: u64,
    /// `BoardDoc.revision` immediately after committing this transaction.
    pub after_revision: u64,
    /// Exact authoritative [`Board`] snapshot prior to this transaction.
    pub before_board: Board,
    /// Exact authoritative [`Board`] snapshot after this transaction.
    pub after_board: Board,
    /// Non-fatal warnings/info emitted when this transaction committed.
    pub diagnostics: Vec<Diagnostic>,
}

/// Summary returned by an atomic [`UndoStack::undo`] / [`UndoStack::redo`] operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoRedoOutcome {
    pub transaction_id: u64,
    pub label: String,
    pub domain: EditDomain,
    /// New monotonically incremented `BoardDoc.revision`.
    pub revision: u64,
}

/// Unified multi-domain Undo/Redo stack preserving exact [`Board`] snapshots and monotonic revisions.
#[derive(Debug, Clone, PartialEq)]
pub struct UndoStack {
    past: Vec<BoardTransaction>,
    future: Vec<BoardTransaction>,
    max_depth: Option<usize>,
    next_tx_id: u64,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoStack {
    /// Creates an unbounded undo/redo stack.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            max_depth: None,
            next_tx_id: 1,
        }
    }

    /// Creates an undo/redo stack retaining at most `max_depth` committed transactions.
    #[must_use]
    pub const fn with_max_depth(max_depth: usize) -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            max_depth: Some(max_depth),
            next_tx_id: 1,
        }
    }

    /// Allocates the next monotonic transaction ID.
    pub fn allocate_id(&mut self) -> u64 {
        let id = self.next_tx_id;
        self.next_tx_id = self.next_tx_id.saturating_add(1);
        id
    }

    /// Pushes a newly committed transaction onto the undo stack and clears any redo branch.
    pub fn push(&mut self, tx: BoardTransaction) {
        self.future.clear();
        if tx.id >= self.next_tx_id {
            self.next_tx_id = tx.id.saturating_add(1);
        }
        if let Some(limit) = self.max_depth {
            if limit == 0 {
                return;
            }
            self.past.push(tx);
            if let Some(excess) = self.past.len().checked_sub(limit)
                && excess > 0
            {
                self.past.drain(0..excess);
            }
        } else {
            self.past.push(tx);
        }
    }

    /// Returns `true` iff at least one transaction can be undone.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    /// Returns `true` iff at least one transaction can be redone.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Number of transactions currently on the undo stack.
    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.past.len()
    }

    /// Number of transactions currently on the redo stack.
    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.future.len()
    }

    /// Peeks at the next transaction that would be undone.
    #[must_use]
    pub fn peek_undo(&self) -> Option<&BoardTransaction> {
        self.past.last()
    }

    /// Peeks at the next transaction that would be redone.
    #[must_use]
    pub fn peek_redo(&self) -> Option<&BoardTransaction> {
        self.future.last()
    }

    /// Borrows all committed transactions in chronological order (oldest first).
    #[must_use]
    pub fn past(&self) -> &[BoardTransaction] {
        &self.past
    }

    /// Borrows all undone transactions (next-to-redo is last).
    #[must_use]
    pub fn future(&self) -> &[BoardTransaction] {
        &self.future
    }

    /// Atomically undoes the latest transaction from a standalone [`UndoStack`] onto `doc`.
    ///
    /// Restores `tx.before_board`, increments `doc.revision` monotonically, and preserves
    /// `doc.dirty_buffers` untouched.
    pub fn undo(&mut self, doc: &mut BoardDoc) -> Option<UndoRedoOutcome> {
        let tx = self.past.pop()?;
        doc.board = tx.before_board.clone();
        doc.revision = doc.revision.saturating_add(1);
        let outcome = UndoRedoOutcome {
            transaction_id: tx.id,
            label: tx.label.clone(),
            domain: tx.domain,
            revision: doc.revision,
        };
        self.future.push(tx);
        Some(outcome)
    }

    /// Atomically redoes the latest undone transaction from a standalone [`UndoStack`] onto `doc`.
    ///
    /// Restores `tx.after_board`, increments `doc.revision` monotonically, and preserves
    /// `doc.dirty_buffers` untouched.
    pub fn redo(&mut self, doc: &mut BoardDoc) -> Option<UndoRedoOutcome> {
        let tx = self.future.pop()?;
        doc.board = tx.after_board.clone();
        doc.revision = doc.revision.saturating_add(1);
        let outcome = UndoRedoOutcome {
            transaction_id: tx.id,
            label: tx.label.clone(),
            domain: tx.domain,
            revision: doc.revision,
        };
        self.past.push(tx);
        Some(outcome)
    }

    /// Clears both undo and redo history.
    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }

    pub(crate) fn pop_undo(&mut self) -> Option<BoardTransaction> {
        self.past.pop()
    }

    pub(crate) fn push_redo(&mut self, tx: BoardTransaction) {
        self.future.push(tx);
    }

    pub(crate) fn pop_redo(&mut self) -> Option<BoardTransaction> {
        self.future.pop()
    }

    pub(crate) fn push_past_without_clearing_redo(&mut self, tx: BoardTransaction) {
        self.past.push(tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{BoardPresentation, BoardProgram, ExecutionKind};
    use dendri_span::Span;

    fn minimal_board() -> Board {
        Board {
            id: "b_1".to_owned(),
            profile: "arduino-uno-r3".to_owned(),
            authoring_language: AuthoringLanguage::Cpp,
            execution: ExecutionKind::Cooperative,
            scan_period_us: None,
            layout: BoardLayout::default(),
            program: BoardProgram::default(),
            comments: Vec::new(),
            assumptions: Vec::new(),
            presentation: BoardPresentation::default(),
            span: Span::default(),
        }
    }

    #[test]
    fn zero_depth_undo_stack_never_panics_and_domains_map_cleanly() {
        assert_eq!(
            EditDomain::from_paradigm(Paradigm::Textual),
            EditDomain::Code
        );
        assert_eq!(
            EditDomain::from_paradigm(Paradigm::Ladder),
            EditDomain::Ladder
        );
        assert_eq!(
            EditDomain::from_paradigm(Paradigm::Grafcet),
            EditDomain::Grafcet
        );

        let mut stack = UndoStack::with_max_depth(0);
        let b = minimal_board();
        let id = stack.allocate_id();
        stack.push(BoardTransaction {
            id,
            label: "noop".to_owned(),
            domain: EditDomain::Presentation,
            command: SyncCommand::SetEolStyle(EolStyle::Crlf),
            before_revision: 0,
            after_revision: 1,
            before_board: b.clone(),
            after_board: b,
            diagnostics: Vec::new(),
        });
        assert!(!stack.can_undo());
        assert_eq!(stack.undo_len(), 0);
        assert_eq!(stack.redo_len(), 0);
        stack.clear();
    }
}
