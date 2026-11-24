//! 100% pure-Rust Non-Destructive Bidirectional Sync Engine for `dendriForge` v2
//! (`04-frontends-visual-and-sync.md` §4.3, §6).
//!
//! Architecture:
//! - [`BoardDoc`]: Single authoritative versioned document per board (`revision: u64`,
//!   `board: Board`, `dirty_buffers: BTreeMap<String, DirtyViewBuffer>`, `undo_stack: UndoStack`).
//! - [`SyncEngine`]: Language-agnostic bidirectional pipeline driven strictly through
//!   `&dyn LanguageFrontend` (zero circular dependency on individual language crates).
//! - **Zero Silent Clobbering:** Single-POU edits merge ONLY the edited POU while preserving
//!   100% of untouched `OB_STARTUP`, `OB_HW_INT`, other POUs, `Foreign` nodes,
//!   `[[boards.assumptions]]`, `[boards.presentation]`, and board metadata.
//! - **ADR-027 Comment Conservation (`C2`, `C4`):** Degraded projections matching via
//!   `frontend.matches_comment` never overwrite verbatim stored originals; deleting any anchored
//!   `NodeId` transitions its comments to `dangling` on the nearest surviving parent scope
//!   (`INFO_COMMENT_ORPHANED`).
//! - **Cross-Language Export Gate:** [`SyncEngine::export_view`] enforces
//!   `check_program_foreign_export` (`ERR_FOREIGN_EXPORT_BLOCKED` unless `allow_foreign = true`;
//!   same-language emission is never blocked).
//! - **Unified Multi-Domain Undo/Redo:** [`UndoStack`], [`BoardTransaction`], and [`SyncCommand`]
//!   span Code, Ladder, GRAFCET, I/O Tag Table, Data Table, Comments, Assumptions, Presentation,
//!   and Canvas edits with monotonic revisions and dirty-buffer safety.

pub mod command;
pub mod doc;
pub mod engine;
mod merge;

pub use command::{BoardTransaction, EditDomain, SyncCommand, UndoRedoOutcome, UndoStack};
pub use doc::{BoardDoc, DirtyViewBuffer, HardwareDecl, PresentationStore};
pub use engine::{
    ExportScope, SyncApplyReport, SyncConfig, SyncEditError, SyncEngine, ViewProjection,
};
