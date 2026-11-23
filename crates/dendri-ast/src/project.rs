//! Multi-board `.toml` project model, OBs/POUs, modules, libraries, packs, and assumptions
//! (`01-asl-ir-and-project-schema.md` §2.4–§2.5, §5–§6).

use crate::comment::Comment;
use crate::ident::NodeId;
use crate::numeric::ChannelRange;
use crate::stmt::Stmt;
use crate::types::{DataDecl, TypeDecl, TypeRef};
use crate::visual::{LadderNetwork, SfcChart};
use dendri_span::Span;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Supported Wave 1–3 authoring language views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthoringLanguage {
    Asl,
    Cpp,
    St,
    Ladder,
    Sfc,
    Micropython,
    Rust,
}

impl AuthoringLanguage {
    /// Canonical lowercase identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asl => "asl",
            Self::Cpp => "cpp",
            Self::St => "st",
            Self::Ladder => "ladder",
            Self::Sfc => "sfc",
            Self::Micropython => "micropython",
            Self::Rust => "rust",
        }
    }
}

/// Non-exhaustive per-board execution model (`01` §5, `02` §2.2, `DEF-005`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionKind {
    Cooperative,
    PlcScan,
}

/// Hardware I/O pin direction/domain kind (`01` §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareKind {
    DigitalIn,
    DigitalOut,
    DigitalBidirectional,
    AnalogIn,
    AnalogOut,
    PwmOut,
    BusPin,
}

/// Internal pull resistor configuration (`01` §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullMode {
    #[default]
    None,
    PullUp,
    PullDown,
}

/// Hardware I/O Tag Table entry (`[[boards.program.hardware]]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareBinding {
    pub id: NodeId,
    pub label: String,
    pub pin: String,
    pub kind: HardwareKind,
    #[serde(default)]
    pub pull: PullMode,
    #[serde(default)]
    pub active_low: bool,
    /// Required for `AnalogIn`, `AnalogOut`, `PwmOut`: codec ID from the board profile.
    #[serde(default)]
    pub codec: Option<String>,
    /// Required for `AnalogIn`, `AnalogOut`: physical signal range & live-zero metadata.
    #[serde(default)]
    pub range: Option<ChannelRange>,
    #[serde(skip)]
    pub span: Span,
}

/// Hardware interrupt edge trigger (`OB_HW_INT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptEdge {
    Rising,
    Falling,
    Change,
    Low,
    High,
}

/// Organization Block (OB) classification (`01` §2.5, ADR-018).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObKind {
    Startup,
    Main,
    CyclicInt {
        period_us: u64,
    },
    HwInt {
        pin_label: String,
        edge: InterruptEdge,
        priority: u8,
    },
    CommEvent {
        bus_id: String,
    },
}

/// POU classification (`OB`, stateless `FC`, or stateful `FB`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "pou_kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PouKind {
    Ob { ob: ObKind },
    Fc { return_type: TypeRef },
    Fb,
}

/// Parameter direction for `FC` / `FB` declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamDirection {
    In,
    InOut,
    Out,
}

/// Typed parameter declaration on a POU (`FC` or `FB`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamDecl {
    pub name: String,
    pub direction: ParamDirection,
    pub ty: TypeRef,
}

/// First-class structural POU body (`01` §2.5, ADR-026).
#[derive(Debug, Clone, PartialEq)]
pub enum PouBody {
    Statements(Vec<Stmt>),
    Network(LadderNetwork),
    Chart(SfcChart),
}

/// Single Program Organization Unit (`[[boards.program.pous]]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Pou {
    pub id: NodeId,
    pub name: String,
    pub kind: PouKind,
    pub preferred_view: Option<AuthoringLanguage>,
    pub params: Vec<ParamDecl>,
    pub locals: Vec<DataDecl>,
    pub body: PouBody,
    pub span: Span,
}

/// Fidelity tier for three-level `Foreign` constructs (`01` §6.2, ADR-028).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ForeignFidelity {
    /// Preserved verbatim for lossless same-language round-trips.
    F0,
    /// Signatures/types declared by a hash-locked `pack.toml`.
    F1,
    /// Modeled by an ASL reference implementation or native physics/bus model.
    F2,
}

/// Simulation policy recorded in `[[boards.assumptions]]` for an unmodeled `Foreign` node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssumptionPolicy {
    NoEffect,
    ReturnValue,
    UserModel,
}

/// Explicit, auditable simulation assumption (`[[boards.assumptions]]`, `WARN_ASSUMPTION_ACTIVE`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assumption {
    pub node: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr_ordinal: Option<u32>,
    pub policy: AssumptionPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub note: String,
    #[serde(skip)]
    pub span: Span,
}

/// Embedded project module (`[[boards.program.modules]]`, `01` §6.1 & §6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDecl {
    pub id: NodeId,
    pub name: String,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub types: Vec<TypeDecl>,
    pub data: Vec<DataDecl>,
    pub pous: Vec<Pou>,
    pub span: Span,
}

/// Sole executable ASL program for a board (`[boards.program]`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardProgram {
    pub imports: Vec<String>,
    pub hardware: Vec<HardwareBinding>,
    pub types: Vec<TypeDecl>,
    pub data: Vec<DataDecl>,
    pub modules: Vec<ModuleDecl>,
    pub pous: Vec<Pou>,
}

/// Canvas placement geometry for a board or component instance.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardLayout {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub rotation: u16,
    #[serde(default)]
    pub mirror_x: bool,
    #[serde(default)]
    pub mirror_y: bool,
}

/// Preserved source EOL style for same-language emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EolStyle {
    #[default]
    Lf,
    Crlf,
}

/// Optional NON-EXECUTABLE presentation metadata (`[boards.presentation]` — NEVER comments!).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardPresentation {
    #[serde(default)]
    pub eol_style: EolStyle,
    /// Optional blank-line spacing hint (`0..=2`) per `NodeId`.
    #[serde(default)]
    pub blank_lines_before: BTreeMap<NodeId, u8>,
    /// Multi-file user source layout map (`NodeId` -> relative file path, e.g. `"motor_ctrl.h"`).
    #[serde(default)]
    pub origin_files: BTreeMap<NodeId, String>,
    /// Optional visual coordinates `(x, y)` for GRAFCET steps or diagram items keyed by `NodeId`.
    #[serde(default)]
    pub node_coords: BTreeMap<NodeId, [f64; 2]>,
}

/// Single MCU or PLC board instance inside a multi-board project (`[[boards]]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    pub id: String,
    pub profile: String,
    pub authoring_language: AuthoringLanguage,
    pub execution: ExecutionKind,
    pub scan_period_us: Option<u64>,
    pub layout: BoardLayout,
    pub program: BoardProgram,
    pub comments: Vec<Comment>,
    pub assumptions: Vec<Assumption>,
    pub presentation: BoardPresentation,
    pub span: Span,
}

/// External `.dendrilib` lock entry (`[[libraries]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryLock {
    pub name: String,
    pub version: String,
    pub path: String,
    pub sha256: String,
}

/// External `pack.toml` lock entry (`[[packs]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackLock {
    pub id: String,
    pub version: String,
    pub path: String,
    pub sha256: String,
}

/// External component placed on the simulation canvas (`[[components]]`).
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentInstance {
    pub id: String,
    pub profile: String,
    pub label: String,
    pub layout: BoardLayout,
    pub params: BTreeMap<String, String>,
    pub span: Span,
}

/// Wire domain classification (`[[wires]]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireSignal {
    #[default]
    Digital,
    Analog,
    Power,
    Ground,
    AcPhase,
    Comm,
    Pneumatic,
    Hydraulic,
}

/// Physical wire connecting `<instance_id>.<port_id>` endpoints (`[[wires]]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    pub id: String,
    pub from: String,
    pub to: String,
    pub signal: WireSignal,
    pub waypoints: Vec<[f64; 2]>,
    pub span: Span,
}

/// Top-level `[project]` header (`01` §6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMeta {
    pub schema_version: String,
    pub asl_version: String,
    pub requires: Vec<String>,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
}

/// Complete single-file multi-board `dendriForge` v2 project.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub meta: ProjectMeta,
    pub libraries: Vec<LibraryLock>,
    pub packs: Vec<PackLock>,
    pub comments: Vec<Comment>,
    pub boards: Vec<Board>,
    pub components: Vec<ComponentInstance>,
    pub wires: Vec<Wire>,
}
