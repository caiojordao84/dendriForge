//! Strict `#[serde(deny_unknown_fields)]` TOML wire DTOs for `dendri-project`.

use dendri_ast::{
    ActionQualifier, Assumption, AuthoringLanguage, BoardLayout, BoardPresentation, Comment,
    ExecutionKind, FbKind, ForeignFidelity, HardwareBinding, LadderCoilKind, LadderCompareOp,
    LadderContactKind, LibraryLock, NodeId, PackLock, ParamDecl, PouKind, ProjectMeta,
    StorageScope, TypeDecl, TypeRef, WireSignal,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use toml::Spanned;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawProject {
    pub project: Spanned<ProjectMeta>,
    #[serde(default)]
    pub libraries: Vec<LibraryLock>,
    #[serde(default)]
    pub packs: Vec<PackLock>,
    #[serde(default)]
    pub comments: Vec<Spanned<Comment>>,
    #[serde(default)]
    pub boards: Vec<Spanned<RawBoard>>,
    #[serde(default)]
    pub components: Vec<Spanned<RawComponentInstance>>,
    #[serde(default)]
    pub wires: Vec<Spanned<RawWire>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutProject {
    pub project: ProjectMeta,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub libraries: Vec<LibraryLock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub packs: Vec<PackLock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub boards: Vec<OutBoard>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<OutComponentInstance>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub wires: Vec<OutWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBoard {
    pub id: String,
    pub profile: String,
    pub authoring_language: AuthoringLanguage,
    pub execution: ExecutionKind,
    #[serde(default)]
    pub scan_period_us: Option<u64>,
    #[serde(default)]
    pub layout: BoardLayout,
    #[serde(default)]
    pub program: RawBoardProgram,
    #[serde(default)]
    pub comments: Vec<Spanned<Comment>>,
    #[serde(default)]
    pub assumptions: Vec<Spanned<Assumption>>,
    #[serde(default)]
    pub presentation: BoardPresentation,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutBoard {
    pub id: String,
    pub profile: String,
    pub authoring_language: AuthoringLanguage,
    pub execution: ExecutionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_period_us: Option<u64>,
    pub layout: BoardLayout,
    pub program: OutBoardProgram,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<Assumption>,
    pub presentation: BoardPresentation,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBoardProgram {
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub hardware: Vec<Spanned<HardwareBinding>>,
    #[serde(default)]
    pub types: Vec<Spanned<TypeDecl>>,
    #[serde(default)]
    pub data: Vec<Spanned<RawDataDecl>>,
    #[serde(default)]
    pub modules: Vec<Spanned<RawModuleDecl>>,
    #[serde(default)]
    pub pous: Vec<Spanned<RawPou>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutBoardProgram {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hardware: Vec<HardwareBinding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<TypeDecl>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<OutDataDecl>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<OutModuleDecl>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pous: Vec<OutPou>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawModuleDecl {
    pub id: NodeId,
    pub name: String,
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub exports: Vec<String>,
    #[serde(default)]
    pub types: Vec<Spanned<TypeDecl>>,
    #[serde(default)]
    pub data: Vec<Spanned<RawDataDecl>>,
    #[serde(default)]
    pub pous: Vec<Spanned<RawPou>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutModuleDecl {
    pub id: NodeId,
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<TypeDecl>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<OutDataDecl>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pous: Vec<OutPou>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawDataDecl {
    pub id: NodeId,
    pub name: String,
    pub scope: StorageScope,
    pub ty: TypeRef,
    #[serde(default)]
    pub initial: Option<Spanned<String>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutDataDecl {
    pub id: NodeId,
    pub name: String,
    pub scope: StorageScope,
    pub ty: TypeRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RawPouBodyKind {
    Statements,
    Network,
    Chart,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPou {
    pub id: NodeId,
    pub name: String,
    pub kind: PouKind,
    #[serde(default)]
    pub preferred_view: Option<AuthoringLanguage>,
    #[serde(default)]
    pub params: Vec<ParamDecl>,
    #[serde(default)]
    pub locals: Vec<Spanned<RawDataDecl>>,
    pub body_kind: RawPouBodyKind,
    #[serde(default)]
    pub body: Vec<Spanned<RawStmt>>,
    #[serde(default)]
    pub network: Option<RawLadderNetwork>,
    #[serde(default)]
    pub chart: Option<RawSfcChart>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutPou {
    pub id: NodeId,
    pub name: String,
    pub kind: PouKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_view: Option<AuthoringLanguage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamDecl>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locals: Vec<OutDataDecl>,
    pub body_kind: RawPouBodyKind,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<OutStmt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<OutLadderNetwork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart: Option<OutSfcChart>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCallArg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawNamedExpr {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawElifArm {
    pub condition: String,
    #[serde(default)]
    pub body: Vec<RawStmt>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutElifArm {
    pub condition: String,
    pub body: Vec<OutStmt>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RawCasePattern {
    Value { expr: String },
    Range { start: String, end: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSwitchCase {
    pub patterns: Vec<RawCasePattern>,
    #[serde(default)]
    pub body: Vec<RawStmt>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutSwitchCase {
    pub patterns: Vec<RawCasePattern>,
    pub body: Vec<OutStmt>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RawStmt {
    Assign {
        id: NodeId,
        target: String,
        value: String,
    },
    Call {
        id: NodeId,
        callee: String,
        #[serde(default)]
        args: Vec<RawCallArg>,
    },
    FbInvoke {
        id: NodeId,
        instance: String,
        #[serde(default)]
        inputs: Vec<RawNamedExpr>,
    },
    If {
        id: NodeId,
        condition: String,
        #[serde(default)]
        then_body: Vec<RawStmt>,
        #[serde(default)]
        elif_arms: Vec<RawElifArm>,
        #[serde(default)]
        else_body: Vec<RawStmt>,
    },
    Switch {
        id: NodeId,
        scrutinee: String,
        #[serde(default)]
        cases: Vec<RawSwitchCase>,
        #[serde(default)]
        default: Option<Vec<RawStmt>>,
    },
    While {
        id: NodeId,
        condition: String,
        #[serde(default)]
        body: Vec<RawStmt>,
    },
    For {
        id: NodeId,
        counter: String,
        from: String,
        to: String,
        step: String,
        #[serde(default)]
        inclusive: bool,
        #[serde(default)]
        body: Vec<RawStmt>,
    },
    Break {
        id: NodeId,
    },
    Continue {
        id: NodeId,
    },
    Return {
        id: NodeId,
        #[serde(default)]
        value: Option<String>,
    },
    Wait {
        id: NodeId,
        duration: String,
    },
    Foreign {
        id: NodeId,
        language: String,
        fidelity: ForeignFidelity,
        text: String,
        #[serde(default)]
        pack_hint: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum OutStmt {
    Assign {
        id: NodeId,
        target: String,
        value: String,
    },
    Call {
        id: NodeId,
        callee: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        args: Vec<RawCallArg>,
    },
    FbInvoke {
        id: NodeId,
        instance: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        inputs: Vec<RawNamedExpr>,
    },
    If {
        id: NodeId,
        condition: String,
        then_body: Vec<OutStmt>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        elif_arms: Vec<OutElifArm>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        else_body: Vec<OutStmt>,
    },
    Switch {
        id: NodeId,
        scrutinee: String,
        cases: Vec<OutSwitchCase>,
        #[serde(skip_serializing_if = "Option::is_none")]
        default: Option<Vec<OutStmt>>,
    },
    While {
        id: NodeId,
        condition: String,
        body: Vec<OutStmt>,
    },
    For {
        id: NodeId,
        counter: String,
        from: String,
        to: String,
        step: String,
        inclusive: bool,
        body: Vec<OutStmt>,
    },
    Break {
        id: NodeId,
    },
    Continue {
        id: NodeId,
    },
    Return {
        id: NodeId,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    Wait {
        id: NodeId,
        duration: String,
    },
    Foreign {
        id: NodeId,
        language: String,
        fidelity: ForeignFidelity,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pack_hint: Option<String>,
    },
}

// --- Ladder wire types ---

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawLadderNetwork {
    #[serde(default)]
    pub rungs: Vec<RawLadderRung>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutLadderNetwork {
    pub rungs: Vec<OutLadderRung>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawLadderRung {
    pub id: NodeId,
    pub order: u32,
    #[serde(default)]
    pub cells: Vec<Spanned<RawLadderCell>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutLadderRung {
    pub id: NodeId,
    pub order: u32,
    pub cells: Vec<OutLadderCell>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RawLadderCell {
    Empty {
        id: NodeId,
        row: u32,
        col: u32,
    },
    Wire {
        id: NodeId,
        row: u32,
        col: u32,
    },
    Contact {
        id: NodeId,
        row: u32,
        col: u32,
        contact_kind: LadderContactKind,
        target: String,
    },
    Coil {
        id: NodeId,
        row: u32,
        col: u32,
        coil_kind: LadderCoilKind,
        target: String,
    },
    ParallelOr {
        id: NodeId,
        row: u32,
        col: u32,
        branches: Vec<Vec<RawLadderCell>>,
    },
    FbBox {
        id: NodeId,
        row: u32,
        col: u32,
        fb_kind: FbKind,
        instance: String,
        #[serde(default)]
        inputs: Vec<RawNamedExpr>,
    },
    CompareBox {
        id: NodeId,
        row: u32,
        col: u32,
        op: LadderCompareOp,
        left: String,
        right: String,
    },
    CallBox {
        id: NodeId,
        row: u32,
        col: u32,
        callee: String,
        #[serde(default)]
        args: Vec<RawCallArg>,
        #[serde(default)]
        output: Option<String>,
    },
    ForeignBox {
        id: NodeId,
        row: u32,
        col: u32,
        language: String,
        fidelity: ForeignFidelity,
        text: String,
        #[serde(default)]
        pack_hint: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum OutLadderCell {
    Empty {
        id: NodeId,
        row: u32,
        col: u32,
    },
    Wire {
        id: NodeId,
        row: u32,
        col: u32,
    },
    Contact {
        id: NodeId,
        row: u32,
        col: u32,
        contact_kind: LadderContactKind,
        target: String,
    },
    Coil {
        id: NodeId,
        row: u32,
        col: u32,
        coil_kind: LadderCoilKind,
        target: String,
    },
    ParallelOr {
        id: NodeId,
        row: u32,
        col: u32,
        branches: Vec<Vec<OutLadderCell>>,
    },
    FbBox {
        id: NodeId,
        row: u32,
        col: u32,
        fb_kind: FbKind,
        instance: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        inputs: Vec<RawNamedExpr>,
    },
    CompareBox {
        id: NodeId,
        row: u32,
        col: u32,
        op: LadderCompareOp,
        left: String,
        right: String,
    },
    CallBox {
        id: NodeId,
        row: u32,
        col: u32,
        callee: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        args: Vec<RawCallArg>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    ForeignBox {
        id: NodeId,
        row: u32,
        col: u32,
        language: String,
        fidelity: ForeignFidelity,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pack_hint: Option<String>,
    },
}

// --- GRAFCET / SFC wire types ---

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSfcChart {
    #[serde(default)]
    pub steps: Vec<RawSfcStep>,
    #[serde(default)]
    pub transitions: Vec<RawSfcTransition>,
    #[serde(default)]
    pub evolve_to_stability: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutSfcChart {
    pub steps: Vec<OutSfcStep>,
    pub transitions: Vec<OutSfcTransition>,
    pub evolve_to_stability: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSfcStep {
    pub id: NodeId,
    pub name: String,
    #[serde(default)]
    pub initial: bool,
    #[serde(default)]
    pub actions: Vec<RawSfcAction>,
    #[serde(default)]
    pub macro_chart: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutSfcStep {
    pub id: NodeId,
    pub name: String,
    pub initial: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<OutSfcAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macro_chart: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSfcAction {
    pub id: NodeId,
    pub qualifier: ActionQualifier,
    #[serde(default)]
    pub duration: Option<Spanned<String>>,
    #[serde(default)]
    pub target: Option<Spanned<String>>,
    #[serde(default)]
    pub body: Vec<Spanned<RawStmt>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutSfcAction {
    pub id: NodeId,
    pub qualifier: ActionQualifier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<OutStmt>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSfcTransition {
    pub id: NodeId,
    pub name: String,
    pub from_steps: Vec<String>,
    pub to_steps: Vec<String>,
    #[serde(default)]
    pub priority: u32,
    pub receptivity: Spanned<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutSfcTransition {
    pub id: NodeId,
    pub name: String,
    pub from_steps: Vec<String>,
    pub to_steps: Vec<String>,
    pub priority: u32,
    pub receptivity: String,
}

// --- Component & Wire ---

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawComponentInstance {
    pub id: String,
    pub profile: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub layout: BoardLayout,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutComponentInstance {
    pub id: String,
    pub profile: String,
    pub label: String,
    pub layout: BoardLayout,
    pub params: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawWire {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub signal: WireSignal,
    #[serde(default)]
    pub waypoints: Vec<[f64; 2]>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OutWire {
    pub id: String,
    pub from: String,
    pub to: String,
    pub signal: WireSignal,
    pub waypoints: Vec<[f64; 2]>,
}
