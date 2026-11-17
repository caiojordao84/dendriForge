//! ASL — Abstract Syntax Language IR types (Phase 0 contract freeze).
//!
//! These types are the source of truth for the TOML-based IR consumed by
//! parsers, validators, emitters, and the scan-cycle simulator in Phase 1.

pub mod error;
pub mod expr;
pub mod grafcet;
pub mod ladder;
pub mod lit;
pub mod program;
pub mod project;
pub mod stmt;
pub mod types;

pub use error::{ErrorCode, ErrorSeverity};
pub use expr::Expr;
pub use grafcet::{
    GrafcetBlock, GrafcetDocument, GrafcetIoBinding, GrafcetStep, GrafcetTransition,
};
pub use ladder::{
    CounterFbKind, LadderBlock, LadderBranch, LadderCell, LadderDocument, LadderIoBinding,
    LadderRung, TimerFbKind,
};
pub use lit::Literal;
pub use program::{
    DataEntry, FunctionDef, HardwareEntry, Metadata, Program, Scope, TargetRole,
};
pub use project::{
    BoardInstance, ComponentInstance, SimulationProject, SourceLanguage, SourceScript, Wire,
    PROJECT_EXTENSION,
};
pub use stmt::{ElifArm, Stmt};
pub use types::{DataType, HwType, IoMode};
