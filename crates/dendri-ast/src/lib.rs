//! Canonical typed Intermediate Representation (IR) for **ASL v2**
//! (`01-asl-ir-and-project-schema.md`).
//!
//! Design invariants:
//! - One typed AST in memory: zero `ExprText`, zero `VendorExtension`, zero
//!   stored synthetic `__rail_N` / `__sfc_*` temporaries.
//! - Exact fixed-point analog contract (`ASL_ANALOG_FULL_SCALE = 1_000_000_000`).
//! - First-class verbatim human comments and explicit simulation assumptions.
//! - Structural `PouBody::{Statements, Network, Chart}` bodies.

pub mod comment;
pub mod expr;
pub mod ident;
pub mod numeric;
pub mod project;
pub mod stmt;
pub mod types;
pub mod visual;

pub use comment::{
    Comment, CommentAnchor, CommentEncoding, CommentOrigin, CommentPlacement, CommentStyle,
    comment_multiset, orphan_comments_for_node,
};
pub use expr::{BinaryOp, CallArg, Expr, ExprKind, LValue, Literal, UnaryOp};
pub use ident::{Endpoint, EndpointError, NodeId, is_valid_instance_id, is_valid_profile_id};
pub use numeric::{
    ASL_ANALOG_FULL_SCALE, AnalogStatus, ChannelBand, ChannelRange, Codec, CodecError, CodecKind,
    SignalKind, composed_clamped_rational, gcd_u64, lcm_u128,
};
pub use project::{
    Assumption, AssumptionPolicy, AuthoringLanguage, Board, BoardLayout, BoardPresentation,
    BoardProgram, ComponentInstance, EolStyle, ExecutionKind, ForeignFidelity, HardwareBinding,
    HardwareKind, InterruptEdge, LibraryLock, ModuleDecl, ObKind, PackLock, ParamDecl,
    ParamDirection, Pou, PouBody, PouKind, Project, ProjectMeta, PullMode, Wire, WireSignal,
};
pub use stmt::{CasePattern, ElifArm, Stmt, StmtKind, SwitchCase};
pub use types::{
    DataDecl, EnumVariant, FbKind, ScalarType, StorageScope, StructField, TypeDecl, TypeRef,
};
pub use visual::{
    ActionQualifier, LadderBoxKind, LadderCell, LadderCellKind, LadderCoilKind, LadderCompareOp,
    LadderContactKind, LadderNetwork, LadderRung, SfcAction, SfcChart, SfcStep, SfcTransition,
};
