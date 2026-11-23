//! Three-level `Foreign` helpers, strict four-part diagnostic builder, cross-language
//! export guard, and hash-locked `pack.toml` loader (`01` §6.2, `04` §3.3, closes `DEF-008`).

use crate::comments::render_comment_for_language;
use crate::traits::{BoardContext, CommentPlacement, LanguageId, RenderedComment};
use dendri_ast::{
    Assumption, BoardProgram, CasePattern, CommentStyle, Expr, ExprKind, ForeignFidelity, LValue,
    LadderBoxKind, LadderCell, LadderCellKind, NodeId, PackLock, Pou, PouBody, ScalarType, Stmt,
    StmtKind, TypeDecl, TypeRef, is_valid_profile_id,
};
use dendri_span::{Diagnostic, ErrorCode, FileId, Span};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;
use thiserror::Error;

/// Supported `pack.toml` and `checksums.toml` schema version.
pub const PACK_SCHEMA_VERSION: &str = "2.0";

// ---------------------------------------------------------------------------
// Hash-Locked `pack.toml` Manifest & `F1 Declared` Lookup (`DEF-008`)
// ---------------------------------------------------------------------------

/// Fidelity tier declared in a `pack.toml` manifest (`01` §6.2, `04` §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PackFidelity {
    #[serde(rename = "f0_preserved", alias = "F0", alias = "f0")]
    F0Preserved,
    #[serde(rename = "f1_declared", alias = "F1", alias = "f1")]
    F1Declared,
    #[serde(rename = "f2_modeled", alias = "F2", alias = "f2")]
    F2Modeled,
}

impl PackFidelity {
    /// Converts into the canonical ASL [`ForeignFidelity`] enum.
    #[must_use]
    pub const fn to_foreign_fidelity(self) -> ForeignFidelity {
        match self {
            Self::F0Preserved => ForeignFidelity::F0,
            Self::F1Declared => ForeignFidelity::F1,
            Self::F2Modeled => ForeignFidelity::F2,
        }
    }
}

/// Kind of symbol exported by a `pack.toml` (`function`, `fb`, or `method`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackSymbolKind {
    Function,
    Fb,
    Method,
}

/// Typed parameter entry inside a `[[symbols]]` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackParam {
    pub name: String,
    pub ty: String,
}

impl PackParam {
    /// Resolves `self.ty` into a canonical ASL [`TypeRef`].
    #[must_use]
    pub fn type_ref(&self) -> Option<TypeRef> {
        parse_pack_type_ref(&self.ty)
    }
}

fn default_void_ty() -> String {
    "VOID".to_owned()
}

/// Single `F1`/`F2` declared symbol inside a `pack.toml` (`[[symbols]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackSymbol {
    pub name: String,
    pub kind: PackSymbolKind,
    #[serde(default)]
    pub params: Vec<PackParam>,
    #[serde(default = "default_void_ty")]
    pub return_ty: String,
    #[serde(default)]
    pub doc: String,
}

impl PackSymbol {
    /// Resolves `self.return_ty` into a canonical ASL [`TypeRef`].
    #[must_use]
    pub fn return_type_ref(&self) -> Option<TypeRef> {
        parse_pack_type_ref(&self.return_ty)
    }

    /// Resolves all declared parameter types into ASL [`TypeRef`] values.
    #[must_use]
    pub fn param_type_refs(&self) -> Vec<(&str, Option<TypeRef>)> {
        self.params
            .iter()
            .map(|p| (p.name.as_str(), p.type_ref()))
            .collect()
    }
}

/// Pack header metadata (`[pack]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub language: LanguageId,
    pub fidelity: PackFidelity,
    #[serde(default)]
    pub standards: Vec<String>,
    #[serde(default)]
    pub description: String,
}

/// Strict, hash-locked `pack.toml` manifest (`DEF-008`, `#[serde(deny_unknown_fields)]`).
///
/// `content_sha256` is NEVER read from inside the file itself; it is dynamically computed
/// over the raw UTF-8 file bytes on load via SHA-256 and verified against lock entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub schema_version: String,
    pub pack: PackMetadata,
    #[serde(default)]
    pub symbols: Vec<PackSymbol>,
    /// Dynamically computed lowercase 64-hex SHA-256 digest of the pack file bytes.
    #[serde(skip)]
    pub content_sha256: String,
}

/// Lockfile manifest (`assets/packs/checksums.toml`) pairing pack IDs/versions with SHA-256 digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackChecksums {
    pub schema_version: String,
    #[serde(default)]
    pub packs: Vec<PackLock>,
}

/// Errors produced when loading or verifying a `pack.toml` or `checksums.toml`.
#[derive(Debug, Error)]
pub enum PackError {
    #[error("I/O error reading pack `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("TOML schema error in pack manifest: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unsupported pack schema_version `{found}` (expected \"{PACK_SCHEMA_VERSION}\")")]
    UnsupportedVersion { found: String },
    #[error("invalid pack id `{id}` (expected kebab-case `^[a-z0-9]+(-[a-z0-9]+)*$`)")]
    InvalidPackId { id: String },
    #[error("duplicate symbol `{symbol}` in pack `{pack_id}`")]
    DuplicateSymbol { pack_id: String, symbol: String },
    #[error("invalid ASL type `{ty}` on symbol `{symbol}` in pack `{pack_id}`")]
    InvalidTypeRef {
        pack_id: String,
        symbol: String,
        ty: String,
    },
    #[error("pack `{pack_id}` SHA-256 mismatch: expected `{expected}`, computed `{actual}`")]
    ChecksumMismatch {
        pack_id: String,
        expected: String,
        actual: String,
    },
    #[error("pack lock id mismatch: lock expects `{expected}`, manifest declares `{actual}`")]
    LockIdMismatch { expected: String, actual: String },
    #[error(
        "pack `{pack_id}` version mismatch: lock expects `{expected}`, manifest declares `{actual}`"
    )]
    LockVersionMismatch {
        pack_id: String,
        expected: String,
        actual: String,
    },
}

/// Computes the lowercase 64-hex SHA-256 digest of `bytes`.
#[must_use]
pub fn compute_sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Parses an uppercase canonical ASL type string used in `pack.toml` into a [`TypeRef`].
#[must_use]
pub fn parse_pack_type_ref(raw: &str) -> Option<TypeRef> {
    let trimmed = raw.trim();
    match trimmed {
        "VOID" => Some(TypeRef::Void),
        "BOOL" => Some(TypeRef::scalar(ScalarType::Bool)),
        "SINT" => Some(TypeRef::scalar(ScalarType::Sint)),
        "INT" => Some(TypeRef::scalar(ScalarType::Int)),
        "DINT" => Some(TypeRef::scalar(ScalarType::Dint)),
        "LINT" => Some(TypeRef::scalar(ScalarType::Lint)),
        "USINT" => Some(TypeRef::scalar(ScalarType::Usint)),
        "UINT" => Some(TypeRef::scalar(ScalarType::Uint)),
        "UDINT" => Some(TypeRef::scalar(ScalarType::Udint)),
        "ULINT" => Some(TypeRef::scalar(ScalarType::Ulint)),
        "BYTE" => Some(TypeRef::scalar(ScalarType::Byte)),
        "WORD" => Some(TypeRef::scalar(ScalarType::Word)),
        "DWORD" => Some(TypeRef::scalar(ScalarType::Dword)),
        "LWORD" => Some(TypeRef::scalar(ScalarType::Lword)),
        "REAL" => Some(TypeRef::scalar(ScalarType::Real)),
        "LREAL" => Some(TypeRef::scalar(ScalarType::Lreal)),
        "TIME" => Some(TypeRef::scalar(ScalarType::Time)),
        "STRING" => Some(TypeRef::scalar(ScalarType::String { max_len: 255 })),
        "ANALOG" => Some(TypeRef::scalar(ScalarType::Analog)),
        "DUTY" => Some(TypeRef::scalar(ScalarType::Duty)),
        other => {
            if let Some(rest) = other
                .strip_prefix("STRING[")
                .and_then(|s| s.strip_suffix(']'))
            {
                let max_len = rest.parse::<u32>().ok()?;
                if max_len == 0 {
                    return None;
                }
                return Some(TypeRef::scalar(ScalarType::String { max_len }));
            }
            if let Some(codec) = other
                .strip_prefix("CODE<")
                .and_then(|s| s.strip_suffix('>'))
            {
                if codec.is_empty() {
                    return None;
                }
                return Some(TypeRef::scalar(ScalarType::Code {
                    codec_id: codec.to_owned(),
                }));
            }
            None
        }
    }
}

impl PackManifest {
    /// Parses, validates, and SHA-256 hashes a `pack.toml` document strictly (`deny_unknown_fields`).
    pub fn parse_str(toml_src: &str) -> Result<Self, PackError> {
        let mut manifest: Self = toml::from_str(toml_src)?;
        if manifest.schema_version != PACK_SCHEMA_VERSION {
            return Err(PackError::UnsupportedVersion {
                found: manifest.schema_version,
            });
        }
        if !is_valid_profile_id(&manifest.pack.id) {
            return Err(PackError::InvalidPackId {
                id: manifest.pack.id,
            });
        }

        let mut seen_symbols = BTreeSet::new();
        for sym in &manifest.symbols {
            if !seen_symbols.insert(sym.name.clone()) {
                return Err(PackError::DuplicateSymbol {
                    pack_id: manifest.pack.id.clone(),
                    symbol: sym.name.clone(),
                });
            }
            if parse_pack_type_ref(&sym.return_ty).is_none() {
                return Err(PackError::InvalidTypeRef {
                    pack_id: manifest.pack.id.clone(),
                    symbol: sym.name.clone(),
                    ty: sym.return_ty.clone(),
                });
            }
            for param in &sym.params {
                if parse_pack_type_ref(&param.ty).is_none() {
                    return Err(PackError::InvalidTypeRef {
                        pack_id: manifest.pack.id.clone(),
                        symbol: sym.name.clone(),
                        ty: param.ty.clone(),
                    });
                }
            }
        }

        manifest.content_sha256 = compute_sha256_hex(toml_src.as_bytes());
        Ok(manifest)
    }

    /// Parses a `pack.toml` string and verifies its dynamic SHA-256 digest against `expected_sha256`.
    pub fn parse_str_verified(toml_src: &str, expected_sha256: &str) -> Result<Self, PackError> {
        let manifest = Self::parse_str(toml_src)?;
        if !manifest
            .content_sha256
            .eq_ignore_ascii_case(expected_sha256.trim())
        {
            return Err(PackError::ChecksumMismatch {
                pack_id: manifest.pack.id,
                expected: expected_sha256.trim().to_ascii_lowercase(),
                actual: manifest.content_sha256,
            });
        }
        Ok(manifest)
    }

    /// Loads and hashes a `pack.toml` from disk.
    pub fn from_file(path: &Path) -> Result<Self, PackError> {
        let content = std::fs::read_to_string(path).map_err(|source| PackError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse_str(&content)
    }

    /// Loads a `pack.toml` from disk and verifies it against a [`PackLock`] entry (id, version, sha256).
    pub fn from_file_verified(path: &Path, lock: &PackLock) -> Result<Self, PackError> {
        let manifest = Self::from_file(path)?;
        manifest.verify_lock(lock)?;
        Ok(manifest)
    }

    /// Verifies that this manifest matches a project or checksum [`PackLock`] entry.
    pub fn verify_lock(&self, lock: &PackLock) -> Result<(), PackError> {
        if self.pack.id != lock.id {
            return Err(PackError::LockIdMismatch {
                expected: lock.id.clone(),
                actual: self.pack.id.clone(),
            });
        }
        if self.pack.version != lock.version {
            return Err(PackError::LockVersionMismatch {
                pack_id: self.pack.id.clone(),
                expected: lock.version.clone(),
                actual: self.pack.version.clone(),
            });
        }
        if !self.content_sha256.eq_ignore_ascii_case(lock.sha256.trim()) {
            return Err(PackError::ChecksumMismatch {
                pack_id: self.pack.id.clone(),
                expected: lock.sha256.trim().to_ascii_lowercase(),
                actual: self.content_sha256.clone(),
            });
        }
        Ok(())
    }

    /// Looks up a declared symbol (`F1`/`F2`) by exact name (e.g. `"Wire.begin"`, `"MB_CLIENT"`).
    #[must_use]
    pub fn lookup_symbol(&self, symbol_name: &str) -> Option<&PackSymbol> {
        self.symbols.iter().find(|s| s.name == symbol_name)
    }

    /// Returns the ASL [`ForeignFidelity`] supplied by this pack.
    #[must_use]
    pub const fn fidelity(&self) -> ForeignFidelity {
        self.pack.fidelity.to_foreign_fidelity()
    }
}

impl PackChecksums {
    /// Parses and validates a `checksums.toml` pack lockfile (`deny_unknown_fields`).
    pub fn parse_str(toml_src: &str) -> Result<Self, PackError> {
        let checksums: Self = toml::from_str(toml_src)?;
        if checksums.schema_version != PACK_SCHEMA_VERSION {
            return Err(PackError::UnsupportedVersion {
                found: checksums.schema_version,
            });
        }
        Ok(checksums)
    }

    /// Finds the [`PackLock`] entry for `pack_id`.
    #[must_use]
    pub fn lock_for_id(&self, pack_id: &str) -> Option<&PackLock> {
        self.packs.iter().find(|p| p.id == pack_id)
    }
}

// ---------------------------------------------------------------------------
// Three-Level `Foreign` AST Inspection & Fidelity Resolution (`01` §6.2)
// ---------------------------------------------------------------------------

/// Syntactic AST level at which a `Foreign` node appears (`01` §6.2, `04` §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForeignLevel {
    Decl,
    Stmt,
    Expr,
}

impl ForeignLevel {
    /// Human-readable label for diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Decl => "Decl::Foreign",
            Self::Stmt => "Stmt::Foreign",
            Self::Expr => "Expr::Foreign",
        }
    }
}

/// Borrowed descriptor of a single `Foreign` occurrence inside a [`BoardProgram`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignSite<'a> {
    pub level: ForeignLevel,
    pub node_id: Option<&'a NodeId>,
    pub language: &'a str,
    pub fidelity: ForeignFidelity,
    pub text: &'a str,
    pub pack_hint: Option<&'a str>,
    pub span: Span,
}

/// Collects every `Decl::Foreign`, `Stmt::Foreign`, `Expr::Foreign`, and `LadderBoxKind::Foreign`
/// across a [`BoardProgram`] in deterministic traversal order.
#[must_use]
pub fn collect_foreign_sites(program: &BoardProgram) -> Vec<ForeignSite<'_>> {
    let mut sites = Vec::new();
    collect_from_types(&program.types, &mut sites);
    for decl in &program.data {
        if let Some(init) = &decl.initial {
            collect_from_expr(init, Some(&decl.id), &mut sites);
        }
    }
    for pou in &program.pous {
        collect_from_pou(pou, &mut sites);
    }
    for module in &program.modules {
        collect_from_types(&module.types, &mut sites);
        for decl in &module.data {
            if let Some(init) = &decl.initial {
                collect_from_expr(init, Some(&decl.id), &mut sites);
            }
        }
        for pou in &module.pous {
            collect_from_pou(pou, &mut sites);
        }
    }
    sites
}

fn collect_from_types<'a>(types: &'a [TypeDecl], out: &mut Vec<ForeignSite<'a>>) {
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
                out.push(ForeignSite {
                    level: ForeignLevel::Decl,
                    node_id: Some(id),
                    language,
                    fidelity: *fidelity,
                    text,
                    pack_hint: pack_hint.as_deref(),
                    span: *span,
                });
            }
        }
    }
}

fn collect_from_pou<'a>(pou: &'a Pou, out: &mut Vec<ForeignSite<'a>>) {
    for local in &pou.locals {
        if let Some(init) = &local.initial {
            collect_from_expr(init, Some(&local.id), out);
        }
    }
    match &pou.body {
        PouBody::Statements(stmts) => collect_from_stmts(stmts, out),
        PouBody::Network(net) => {
            for rung in &net.rungs {
                collect_from_ladder_cells(&rung.cells, out);
            }
        }
        PouBody::Chart(chart) => {
            for step in &chart.steps {
                for action in &step.actions {
                    if let Some(dur) = &action.duration {
                        collect_from_expr(dur, Some(&action.id), out);
                    }
                    if let Some(target) = &action.target {
                        collect_from_lvalue(target, Some(&action.id), out);
                    }
                    collect_from_stmts(&action.body, out);
                }
            }
            for trans in &chart.transitions {
                collect_from_expr(&trans.receptivity, Some(&trans.id), out);
            }
        }
    }
}

fn collect_from_ladder_cells<'a>(cells: &'a [LadderCell], out: &mut Vec<ForeignSite<'a>>) {
    for cell in cells {
        match &cell.kind {
            LadderCellKind::Empty | LadderCellKind::Wire => {}
            LadderCellKind::Contact { kind: _, target }
            | LadderCellKind::Coil { kind: _, target } => {
                collect_from_lvalue(target, Some(&cell.id), out);
            }
            LadderCellKind::ParallelOr { branches } => {
                for branch in branches {
                    collect_from_ladder_cells(branch, out);
                }
            }
            LadderCellKind::Box(box_kind) => match box_kind {
                LadderBoxKind::Fb {
                    kind: _,
                    instance,
                    inputs,
                } => {
                    collect_from_lvalue(instance, Some(&cell.id), out);
                    for (_, expr) in inputs {
                        collect_from_expr(expr, Some(&cell.id), out);
                    }
                }
                LadderBoxKind::Compare { left, right, .. } => {
                    collect_from_expr(left, Some(&cell.id), out);
                    collect_from_expr(right, Some(&cell.id), out);
                }
                LadderBoxKind::Call {
                    callee: _,
                    args,
                    output,
                } => {
                    for (_, expr) in args {
                        collect_from_expr(expr, Some(&cell.id), out);
                    }
                    if let Some(out_lv) = output {
                        collect_from_lvalue(out_lv, Some(&cell.id), out);
                    }
                }
                LadderBoxKind::Foreign {
                    language,
                    fidelity,
                    text,
                    pack_hint,
                } => {
                    out.push(ForeignSite {
                        level: ForeignLevel::Stmt,
                        node_id: Some(&cell.id),
                        language,
                        fidelity: *fidelity,
                        text,
                        pack_hint: pack_hint.as_deref(),
                        span: Span::default(),
                    });
                }
            },
        }
    }
}

fn collect_from_stmts<'a>(stmts: &'a [Stmt], out: &mut Vec<ForeignSite<'a>>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                collect_from_lvalue(target, Some(&stmt.id), out);
                collect_from_expr(value, Some(&stmt.id), out);
            }
            StmtKind::Call { callee: _, args } => {
                for arg in args {
                    collect_from_expr(&arg.value, Some(&stmt.id), out);
                }
            }
            StmtKind::FbInvoke { instance, inputs } => {
                collect_from_lvalue(instance, Some(&stmt.id), out);
                for (_, expr) in inputs {
                    collect_from_expr(expr, Some(&stmt.id), out);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                collect_from_expr(condition, Some(&stmt.id), out);
                collect_from_stmts(then_body, out);
                for arm in elif_arms {
                    collect_from_expr(&arm.condition, Some(&stmt.id), out);
                    collect_from_stmts(&arm.body, out);
                }
                collect_from_stmts(else_body, out);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                collect_from_expr(scrutinee, Some(&stmt.id), out);
                for case in cases {
                    for pattern in &case.patterns {
                        match pattern {
                            CasePattern::Value(expr) => {
                                collect_from_expr(expr, Some(&stmt.id), out);
                            }
                            CasePattern::Range { start, end } => {
                                collect_from_expr(start, Some(&stmt.id), out);
                                collect_from_expr(end, Some(&stmt.id), out);
                            }
                        }
                    }
                    collect_from_stmts(&case.body, out);
                }
                if let Some(def_body) = default {
                    collect_from_stmts(def_body, out);
                }
            }
            StmtKind::While { condition, body } => {
                collect_from_expr(condition, Some(&stmt.id), out);
                collect_from_stmts(body, out);
            }
            StmtKind::For {
                counter: _,
                from,
                to,
                step,
                inclusive: _,
                body,
            } => {
                collect_from_expr(from, Some(&stmt.id), out);
                collect_from_expr(to, Some(&stmt.id), out);
                collect_from_expr(step, Some(&stmt.id), out);
                collect_from_stmts(body, out);
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Return { value } => {
                if let Some(expr) = value {
                    collect_from_expr(expr, Some(&stmt.id), out);
                }
            }
            StmtKind::Wait { duration } => {
                collect_from_expr(duration, Some(&stmt.id), out);
            }
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => {
                out.push(ForeignSite {
                    level: ForeignLevel::Stmt,
                    node_id: Some(&stmt.id),
                    language,
                    fidelity: *fidelity,
                    text,
                    pack_hint: pack_hint.as_deref(),
                    span: stmt.span,
                });
            }
        }
    }
}

fn collect_from_lvalue<'a>(
    lvalue: &'a LValue,
    enclosing_node: Option<&'a NodeId>,
    out: &mut Vec<ForeignSite<'a>>,
) {
    match lvalue {
        LValue::Var { .. } => {}
        LValue::Index {
            base,
            index,
            span: _,
        } => {
            collect_from_lvalue(base, enclosing_node, out);
            collect_from_expr(index, enclosing_node, out);
        }
        LValue::Field {
            base,
            field: _,
            span: _,
        } => {
            collect_from_lvalue(base, enclosing_node, out);
        }
    }
}

fn collect_from_expr<'a>(
    expr: &'a Expr,
    enclosing_node: Option<&'a NodeId>,
    out: &mut Vec<ForeignSite<'a>>,
) {
    match &expr.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
        ExprKind::Index { base, index } => {
            collect_from_expr(base, enclosing_node, out);
            collect_from_expr(index, enclosing_node, out);
        }
        ExprKind::Field { base, field: _ } => {
            collect_from_expr(base, enclosing_node, out);
        }
        ExprKind::Call { callee: _, args } => {
            for arg in args {
                collect_from_expr(&arg.value, enclosing_node, out);
            }
        }
        ExprKind::Unary { op: _, operand } => {
            collect_from_expr(operand, enclosing_node, out);
        }
        ExprKind::Binary { op: _, left, right } => {
            collect_from_expr(left, enclosing_node, out);
            collect_from_expr(right, enclosing_node, out);
        }
        ExprKind::Foreign { language, text } => {
            out.push(ForeignSite {
                level: ForeignLevel::Expr,
                node_id: enclosing_node,
                language,
                fidelity: ForeignFidelity::F0,
                text,
                pack_hint: None,
                span: expr.span,
            });
        }
    }
}

/// Resolves the highest [`ForeignFidelity`] available for `symbol_name` from loaded `packs`.
///
/// Returns `(ForeignFidelity::F0, None)` if no pack declares `symbol_name`.
#[must_use]
pub fn resolve_foreign_fidelity<'a>(
    language: LanguageId,
    symbol_name: &str,
    packs: &'a [PackManifest],
) -> (ForeignFidelity, Option<(&'a PackManifest, &'a PackSymbol)>) {
    let mut best: Option<(&PackManifest, &PackSymbol)> = None;
    for pack in packs {
        if pack.pack.language == language
            && let Some(sym) = pack.lookup_symbol(symbol_name)
        {
            let replace = match best {
                None => true,
                Some((prev_pack, _)) => pack.fidelity() > prev_pack.fidelity(),
            };
            if replace {
                best = Some((pack, sym));
            }
        }
    }
    match best {
        Some((pack, sym)) => (pack.fidelity(), Some((pack, sym))),
        None => (ForeignFidelity::F0, None),
    }
}

/// Finds an active simulation [`Assumption`] targeting `(node_id, expr_ordinal)`.
#[must_use]
pub fn find_active_assumption<'a>(
    assumptions: &'a [Assumption],
    node_id: &NodeId,
    expr_ordinal: Option<u32>,
) -> Option<&'a Assumption> {
    assumptions
        .iter()
        .find(|a| &a.node == node_id && a.expr_ordinal == expr_ordinal)
}

// ---------------------------------------------------------------------------
// Strict Four-Part Foreign Diagnostic Contract (What / Where / Why / What Now)
// ---------------------------------------------------------------------------

/// Input specification for the normative four-part `Foreign` diagnostic (`04` §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignDiagnosticSpec<'a> {
    pub code: ErrorCode,
    pub board_id: Option<&'a str>,
    pub file_id: Option<FileId>,
    pub span: Span,
    /// (1) WHAT: the foreign construct or symbol text.
    pub what: &'a str,
    /// (2) WHERE: node/POU location context paired with `span`.
    pub where_ctx: &'a str,
    /// (3) WHY: missing fidelity tier or cross-language export policy explanation.
    pub why: &'a str,
    /// (4) WHAT NOW: actionable remediation (record assumption, install pack, or replace).
    pub what_now: &'a str,
}

/// Builds a [`Diagnostic`] conforming strictly to the four-part Foreign diagnostic contract:
/// **(1) WHAT**, **(2) WHERE**, **(3) WHY**, and **(4) WHAT NOW** (`04` §3.3).
#[must_use]
pub fn build_four_part_diagnostic(spec: &ForeignDiagnosticSpec<'_>) -> Diagnostic {
    let message = format!(
        "WHAT: {}; WHERE: {} (span {}); WHY: {}",
        spec.what, spec.where_ctx, spec.span, spec.why
    );
    let mut diag =
        Diagnostic::new(spec.code, message).with_hint(format!("WHAT NOW: {}", spec.what_now));
    if let Some(board_id) = spec.board_id {
        diag = diag.with_board_id(board_id);
    }
    if let Some(file_id) = spec.file_id {
        diag = diag.with_location(file_id, spec.span);
    } else {
        diag.span = Some(spec.span);
    }
    diag
}

/// Verifies that `diag` satisfies the normative four-part contract (WHAT, WHERE, WHY, WHAT NOW, span).
#[must_use]
pub fn is_four_part_diagnostic(diag: &Diagnostic) -> bool {
    diag.span.is_some()
        && diag.message.contains("WHAT:")
        && diag.message.contains("WHERE:")
        && diag.message.contains("WHY:")
        && diag
            .hint
            .as_deref()
            .is_some_and(|h| h.contains("WHAT NOW:"))
}

/// Constructs a four-part `ERR_FOREIGN_EXPORT_BLOCKED` diagnostic for cross-language export.
#[must_use]
pub fn foreign_export_blocked_diagnostic(
    site: &ForeignSite<'_>,
    target: LanguageId,
    board_id: Option<&str>,
) -> Diagnostic {
    let where_ctx = match site.node_id {
        Some(id) => format!("{} `{id}`", site.level.as_str()),
        None => site.level.as_str().to_owned(),
    };
    let what = format!("foreign `{}` construct `{}`", site.language, site.text);
    let why = format!(
        "construct has fidelity {:?} (not cross-language translatable to `{}`)",
        site.fidelity,
        target.as_str()
    );
    let pack_suggestion = match site.pack_hint {
        Some(pack) => format!("install an F2-modeled `{pack}` pack, "),
        None => "install a modeled pack, ".to_owned(),
    };
    let what_now = format!(
        "pass `--allow-foreign` to emit a demarcated \"NOT TRANSLATED\" comment block, {pack_suggestion}record an explicit assumption in `[[boards.assumptions]]`, or replace with a portable ASL construct"
    );
    build_four_part_diagnostic(&ForeignDiagnosticSpec {
        code: ErrorCode::ErrForeignExportBlocked,
        board_id,
        file_id: None,
        span: site.span,
        what: &what,
        where_ctx: &where_ctx,
        why: &why,
        what_now: &what_now,
    })
}

/// Constructs a four-part `ERR_FOREIGN_UNTYPED` diagnostic for an untyped `Expr::Foreign`.
#[must_use]
pub fn foreign_untyped_diagnostic(site: &ForeignSite<'_>, board_id: Option<&str>) -> Diagnostic {
    let where_ctx = match site.node_id {
        Some(id) => format!("{} inside node `{id}`", site.level.as_str()),
        None => site.level.as_str().to_owned(),
    };
    let what = format!(
        "untyped foreign `{}` expression `{}`",
        site.language, site.text
    );
    let why = format!(
        "construct is at {:?} fidelity with no contextual type and no matching `F1` pack signature",
        site.fidelity
    );
    let what_now = "install a hash-locked `F1` `pack.toml` declaring this symbol's return type, assign the expression to an explicitly typed variable, or record an assumption in `[[boards.assumptions]]`";
    build_four_part_diagnostic(&ForeignDiagnosticSpec {
        code: ErrorCode::ErrForeignUntyped,
        board_id,
        file_id: None,
        span: site.span,
        what: &what,
        where_ctx: &where_ctx,
        why: &why,
        what_now,
    })
}

/// Constructs a `WARN_ASSUMPTION_ACTIVE` diagnostic for an auditable simulation assumption.
#[must_use]
pub fn assumption_active_diagnostic(assumption: &Assumption, board_id: Option<&str>) -> Diagnostic {
    let mut diag = Diagnostic::new(
        ErrorCode::WarnAssumptionActive,
        format!(
            "active simulation assumption ({:?}) on node `{}`: {}",
            assumption.policy, assumption.node, assumption.note
        ),
    )
    .with_hint(
        "Active assumptions are permitted during development but block release-mode validation.",
    );
    if let Some(bid) = board_id {
        diag = diag.with_board_id(bid);
    }
    diag.span = Some(assumption.span);
    diag
}

// ---------------------------------------------------------------------------
// Cross-Language Export Guard (`04` §3.3)
// ---------------------------------------------------------------------------

/// Decision returned by [`check_foreign_export`] for a single [`ForeignSite`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForeignExportDecision {
    /// Same-language round-trip: emit raw foreign construct verbatim (NEVER blocked).
    VerbatimSameLanguage(String),
    /// Cross-language export permitted via `ctx.allow_foreign_export == true`:
    /// emit a clearly demarcated `"NOT TRANSLATED"` comment block rendered safely for the target.
    DemarcatedNotTranslated(RenderedComment),
}

/// Evaluates the cross-language export guard for a single [`ForeignSite`] (`04` §3.3).
///
/// - Same-language round-trips are NEVER blocked and return [`ForeignExportDecision::VerbatimSameLanguage`].
/// - Cross-language exports fail with four-part `ERR_FOREIGN_EXPORT_BLOCKED` unless
///   `ctx.allow_foreign_export == true`, in which case a hazard-safe `"NOT TRANSLATED"` comment block
///   is returned.
pub fn check_foreign_export(
    site: &ForeignSite<'_>,
    target_language: LanguageId,
    ctx: &BoardContext,
) -> Result<ForeignExportDecision, Diagnostic> {
    if site.language.eq_ignore_ascii_case(target_language.as_str()) {
        return Ok(ForeignExportDecision::VerbatimSameLanguage(
            site.text.to_owned(),
        ));
    }

    if !ctx.allow_foreign_export {
        return Err(foreign_export_blocked_diagnostic(
            site,
            target_language,
            Some(&ctx.board_id),
        ));
    }

    let pack_suffix = match site.pack_hint {
        Some(pack) => format!(", pack `{pack}`"),
        None => String::new(),
    };
    let demarcated_body = format!(
        " NOT TRANSLATED [{} {:?} from `{}`{}]:\n {}",
        site.level.as_str(),
        site.fidelity,
        site.language,
        pack_suffix,
        site.text
    );
    let rendered = render_comment_for_language(
        target_language,
        &demarcated_body,
        CommentStyle::Block,
        CommentPlacement::Leading,
        ctx,
    );
    Ok(ForeignExportDecision::DemarcatedNotTranslated(rendered))
}

/// Guards an entire [`BoardProgram`] before cross-language emission.
///
/// Returns `Ok(())` when either no cross-language `Foreign` sites exist or `ctx.allow_foreign_export`
/// is enabled; otherwise returns `Err(diagnostics)` containing one four-part
/// `ERR_FOREIGN_EXPORT_BLOCKED` diagnostic per blocked site.
pub fn check_program_foreign_export(
    program: &BoardProgram,
    target_language: LanguageId,
    ctx: &BoardContext,
) -> Result<(), Vec<Diagnostic>> {
    if ctx.allow_foreign_export {
        return Ok(());
    }
    let mut blocked = Vec::new();
    for site in collect_foreign_sites(program) {
        if !site.language.eq_ignore_ascii_case(target_language.as_str()) {
            blocked.push(foreign_export_blocked_diagnostic(
                &site,
                target_language,
                Some(&ctx.board_id),
            ));
        }
    }
    if blocked.is_empty() {
        Ok(())
    } else {
        Err(blocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{ObKind, PouKind};

    const SAMPLE_WIRE_PACK: &str = r#"
schema_version = "2.0"

[pack]
id = "arduino-wire"
name = "Arduino Wire (I2C) Library Pack"
version = "1.0.0"
language = "cpp"
fidelity = "f1_declared"
standards = ["IEC 61131-3"]
description = "F1 declared signatures for Arduino Wire.h"

[[symbols]]
name = "Wire.beginTransmission"
kind = "method"
params = [{ name = "address", ty = "USINT" }]
return_ty = "VOID"
doc = "Begins buffering a controller-write transmission."

[[symbols]]
name = "Wire.read"
kind = "method"
params = []
return_ty = "INT"
doc = "Dequeues next received byte."
"#;

    #[test]
    fn pack_toml_parses_strictly_computes_sha256_and_enforces_lock() {
        let pack = PackManifest::parse_str(SAMPLE_WIRE_PACK).unwrap();
        assert_eq!(pack.pack.id, "arduino-wire");
        assert_eq!(pack.pack.language, LanguageId::Cpp);
        assert_eq!(pack.fidelity(), ForeignFidelity::F1);
        assert_eq!(pack.content_sha256.len(), 64);

        // Verified load succeeds with exact computed SHA-256
        let expected_hash = compute_sha256_hex(SAMPLE_WIRE_PACK.as_bytes());
        let verified = PackManifest::parse_str_verified(SAMPLE_WIRE_PACK, &expected_hash).unwrap();
        assert_eq!(verified.content_sha256, expected_hash);

        // Tampered hash MUST be rejected
        let bad_hash = "0".repeat(64);
        assert!(matches!(
            PackManifest::parse_str_verified(SAMPLE_WIRE_PACK, &bad_hash),
            Err(PackError::ChecksumMismatch { .. })
        ));

        // Lock verification checks id, version, and sha256
        let lock = PackLock {
            id: "arduino-wire".to_owned(),
            version: "1.0.0".to_owned(),
            path: "assets/packs/arduino-wire.pack.toml".to_owned(),
            sha256: expected_hash,
        };
        assert!(pack.verify_lock(&lock).is_ok());

        // F1 signature lookup returns exact typed ASL TypeRefs
        let sym = pack.lookup_symbol("Wire.beginTransmission").unwrap();
        assert_eq!(sym.kind, PackSymbolKind::Method);
        assert_eq!(sym.return_type_ref(), Some(TypeRef::Void));
        assert_eq!(
            sym.param_type_refs(),
            vec![("address", Some(TypeRef::scalar(ScalarType::Usint)))]
        );

        let packs = [pack];
        let (fidelity, found) = resolve_foreign_fidelity(LanguageId::Cpp, "Wire.read", &packs);
        assert_eq!(fidelity, ForeignFidelity::F1);
        assert_eq!(
            found.unwrap().1.return_type_ref(),
            Some(TypeRef::scalar(ScalarType::Int))
        );

        // Unknown symbol stays F0
        let (f0, none) = resolve_foreign_fidelity(LanguageId::Cpp, "Unknown.fn", &packs);
        assert_eq!(f0, ForeignFidelity::F0);
        assert!(none.is_none());
    }

    #[test]
    fn pack_toml_rejects_unknown_fields_and_bad_versions_and_bad_types() {
        let with_unknown = format!("{SAMPLE_WIRE_PACK}\nunexpected_key = true\n");
        assert!(matches!(
            PackManifest::parse_str(&with_unknown),
            Err(PackError::Toml(_))
        ));

        let bad_ver =
            SAMPLE_WIRE_PACK.replace(r#"schema_version = "2.0""#, r#"schema_version = "9.9""#);
        assert!(matches!(
            PackManifest::parse_str(&bad_ver),
            Err(PackError::UnsupportedVersion { .. })
        ));

        let bad_ty = SAMPLE_WIRE_PACK.replace(r#"ty = "USINT""#, r#"ty = "NOT_A_TYPE""#);
        assert!(matches!(
            PackManifest::parse_str(&bad_ty),
            Err(PackError::InvalidTypeRef { .. })
        ));
    }

    #[test]
    fn pack_checksums_toml_parses_strictly() {
        let src = r#"
schema_version = "2.0"

[[packs]]
id = "arduino-wire"
version = "1.0.0"
path = "assets/packs/arduino-wire.pack.toml"
sha256 = "2a9ef1d59c9eb0b293bc04ef7fc854fc8b747e181af6e4a96bb0274fbd45b491"
"#;
        let checksums = PackChecksums::parse_str(src).unwrap();
        assert_eq!(checksums.packs.len(), 1);
        assert_eq!(
            checksums.lock_for_id("arduino-wire").unwrap().version,
            "1.0.0"
        );
    }

    #[test]
    fn collects_all_three_foreign_levels_and_enforces_export_guard_with_four_part_diag() {
        let program = BoardProgram {
            imports: vec!["std.arduino.*".to_owned()],
            hardware: vec![],
            types: vec![TypeDecl::Foreign {
                id: NodeId::new("n_decl_1"),
                name: Some("Servo".to_owned()),
                language: "cpp".to_owned(),
                fidelity: ForeignFidelity::F1,
                text: "Servo s;".to_owned(),
                pack_hint: Some("arduino-servo".to_owned()),
                span: Span::new(0, 8),
            }],
            data: vec![],
            modules: vec![],
            pous: vec![Pou {
                id: NodeId::new("n_ob_main"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: None,
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![
                    Stmt::new(
                        NodeId::new("n_stmt_1"),
                        StmtKind::Foreign {
                            language: "cpp".to_owned(),
                            fidelity: ForeignFidelity::F1,
                            text: "s.write(90);".to_owned(),
                            pack_hint: Some("arduino-servo".to_owned()),
                        },
                        Span::new(10, 22),
                    ),
                    Stmt::new(
                        NodeId::new("n_stmt_2"),
                        StmtKind::Assign {
                            target: dendri_ast::LValue::Var {
                                name: "pos".to_owned(),
                                span: Span::new(24, 27),
                            },
                            value: Expr::new(
                                ExprKind::Foreign {
                                    language: "cpp".to_owned(),
                                    text: "s.read()".to_owned(),
                                },
                                Span::new(30, 38),
                            ),
                        },
                        Span::new(24, 39),
                    ),
                ]),
                span: Span::new(0, 40),
            }],
        };

        let sites = collect_foreign_sites(&program);
        assert_eq!(sites.len(), 3);
        assert_eq!(sites[0].level, ForeignLevel::Decl);
        assert_eq!(sites[1].level, ForeignLevel::Stmt);
        assert_eq!(sites[2].level, ForeignLevel::Expr);

        // Same-language C++ export is NEVER blocked!
        let mut ctx = BoardContext::arduino_uno_r3("mcu_1");
        assert!(check_program_foreign_export(&program, LanguageId::Cpp, &ctx).is_ok());
        assert_eq!(
            check_foreign_export(&sites[1], LanguageId::Cpp, &ctx).unwrap(),
            ForeignExportDecision::VerbatimSameLanguage("s.write(90);".to_owned())
        );

        // Cross-language export to ST fails with four-part ERR_FOREIGN_EXPORT_BLOCKED
        let errs = check_program_foreign_export(&program, LanguageId::St, &ctx).unwrap_err();
        assert_eq!(errs.len(), 3);
        for diag in &errs {
            assert_eq!(diag.code, ErrorCode::ErrForeignExportBlocked);
            assert!(is_four_part_diagnostic(diag), "not 4-part: {diag:?}");
        }

        // Enabling allow_foreign_export emits demarcated "NOT TRANSLATED" comment block
        ctx.allow_foreign_export = true;
        assert!(check_program_foreign_export(&program, LanguageId::St, &ctx).is_ok());
        let decision = check_foreign_export(&sites[1], LanguageId::St, &ctx).unwrap();
        match decision {
            ForeignExportDecision::DemarcatedNotTranslated(rendered) => {
                assert!(rendered.rendered.contains("NOT TRANSLATED"));
                assert!(rendered.rendered.contains("s.write(90);"));
            }
            ForeignExportDecision::VerbatimSameLanguage(_) => {
                panic!("expected demarcated cross-language output");
            }
        }

        // Untyped Foreign expression four-part diagnostic check
        let untyped = foreign_untyped_diagnostic(&sites[2], Some("mcu_1"));
        assert_eq!(untyped.code, ErrorCode::ErrForeignUntyped);
        assert!(is_four_part_diagnostic(&untyped));
    }

    #[test]
    fn parses_st_modbus_fb_pack_and_collects_foreign_in_network_chart_and_modules() {
        let modbus_pack = r#"
schema_version = "2.0"

[pack]
id = "plc-modbus-tcp"
name = "IEC 61131-3 Modbus TCP Communication Pack"
version = "1.0.0"
language = "st"
fidelity = "f1_declared"
standards = ["IEC 61131-3", "Modbus TCP"]
description = "F1 declared signatures for MB_CLIENT and MB_SERVER."

[[symbols]]
name = "MB_CLIENT"
kind = "fb"
params = [
    { name = "REQ", ty = "BOOL" },
    { name = "REMOTE_IP", ty = "STRING" },
    { name = "STATUS", ty = "WORD" },
]
return_ty = "VOID"
doc = "Modbus TCP client FB."
"#;
        let parsed = PackManifest::parse_str(modbus_pack).unwrap();
        assert_eq!(parsed.pack.language, LanguageId::St);
        let fb = parsed.lookup_symbol("MB_CLIENT").unwrap();
        assert_eq!(fb.kind, PackSymbolKind::Fb);
        assert_eq!(fb.params.len(), 3);
        assert_eq!(
            fb.params[1].type_ref(),
            Some(TypeRef::scalar(ScalarType::String { max_len: 255 }))
        );
        assert_eq!(
            fb.params[2].type_ref(),
            Some(TypeRef::scalar(ScalarType::Word))
        );

        let mut ctx = BoardContext::plc_1200_open_1214("plc_main");
        ctx.loaded_packs.push(parsed);
        let (pack_ref, sym_ref) = ctx.lookup_pack_symbol(LanguageId::St, "MB_CLIENT").unwrap();
        assert_eq!(pack_ref.pack.id, "plc-modbus-tcp");
        assert_eq!(sym_ref.name, "MB_CLIENT");
        assert!(
            ctx.lookup_pack_symbol(LanguageId::Cpp, "MB_CLIENT")
                .is_none()
        );

        // Collect foreign nodes nested inside LadderNetwork, SfcChart, and ModuleDecl
        let program = BoardProgram {
            imports: vec![],
            hardware: vec![],
            types: vec![],
            data: vec![],
            modules: vec![dendri_ast::ModuleDecl {
                id: NodeId::new("n_mod"),
                name: "sub".to_owned(),
                imports: vec![],
                exports: vec![],
                types: vec![TypeDecl::Foreign {
                    id: NodeId::new("n_mod_f"),
                    name: None,
                    language: "cpp".to_owned(),
                    fidelity: ForeignFidelity::F0,
                    text: "class Custom {};".to_owned(),
                    pack_hint: None,
                    span: Span::new(0, 16),
                }],
                data: vec![],
                pous: vec![],
                span: Span::default(),
            }],
            pous: vec![
                Pou {
                    id: NodeId::new("n_lad"),
                    name: "OB_MAIN".to_owned(),
                    kind: PouKind::Ob { ob: ObKind::Main },
                    preferred_view: None,
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Network(dendri_ast::LadderNetwork {
                        rungs: vec![dendri_ast::LadderRung {
                            id: NodeId::new("n_rung"),
                            order: 0,
                            cells: vec![LadderCell {
                                id: NodeId::new("n_cell_f"),
                                row: 0,
                                col: 0,
                                kind: LadderCellKind::Box(LadderBoxKind::Foreign {
                                    language: "cpp".to_owned(),
                                    fidelity: ForeignFidelity::F0,
                                    text: "PORTB |= (1<<5);".to_owned(),
                                    pack_hint: None,
                                }),
                            }],
                        }],
                    }),
                    span: Span::default(),
                },
                Pou {
                    id: NodeId::new("n_sfc"),
                    name: "SFC_CTRL".to_owned(),
                    kind: PouKind::Fb,
                    preferred_view: None,
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Chart(dendri_ast::SfcChart {
                        steps: vec![],
                        transitions: vec![dendri_ast::SfcTransition {
                            id: NodeId::new("n_tr"),
                            name: "T1".to_owned(),
                            from_steps: vec!["S0".to_owned()],
                            to_steps: vec!["S1".to_owned()],
                            priority: 0,
                            receptivity: Expr::new(
                                ExprKind::Foreign {
                                    language: "cpp".to_owned(),
                                    text: "sensorReady()".to_owned(),
                                },
                                Span::new(50, 63),
                            ),
                        }],
                        evolve_to_stability: false,
                    }),
                    span: Span::default(),
                },
            ],
        };
        let sites = collect_foreign_sites(&program);
        assert_eq!(sites.len(), 3);
        assert_eq!(sites[0].level, ForeignLevel::Stmt);
        assert_eq!(sites[1].level, ForeignLevel::Expr);
        assert_eq!(sites[2].level, ForeignLevel::Decl);
    }

    #[test]
    fn collects_expr_foreign_inside_lvalue_index_and_switch_case_patterns() {
        let foreign_expr = |text: &str, start: u32| {
            Expr::new(
                ExprKind::Foreign {
                    language: "cpp".to_owned(),
                    text: text.to_owned(),
                },
                Span::new(start, start + 8),
            )
        };
        let indexed_lvalue = |base: &str, text: &str, start: u32| LValue::Index {
            base: Box::new(LValue::Var {
                name: base.to_owned(),
                span: Span::new(start, start + 3),
            }),
            index: Box::new(foreign_expr(text, start + 4)),
            span: Span::new(start, start + 14),
        };

        let program = BoardProgram {
            imports: vec![],
            hardware: vec![],
            types: vec![],
            data: vec![],
            modules: vec![],
            pous: vec![
                Pou {
                    id: NodeId::new("n_main"),
                    name: "OB_MAIN".to_owned(),
                    kind: PouKind::Ob { ob: ObKind::Main },
                    preferred_view: None,
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Statements(vec![
                        Stmt::new(
                            NodeId::new("n_assign"),
                            StmtKind::Assign {
                                target: indexed_lvalue("arr", "idx()", 0),
                                value: Expr::new(
                                    ExprKind::Literal(dendri_ast::Literal::Int(1)),
                                    Span::new(15, 16),
                                ),
                            },
                            Span::new(0, 17),
                        ),
                        Stmt::new(
                            NodeId::new("n_switch"),
                            StmtKind::Switch {
                                scrutinee: Expr::new(
                                    ExprKind::Ident("mode".to_owned()),
                                    Span::new(20, 24),
                                ),
                                cases: vec![dendri_ast::SwitchCase {
                                    patterns: vec![
                                        CasePattern::Value(foreign_expr("MY_CONST", 30)),
                                        CasePattern::Range {
                                            start: foreign_expr("LO_CONST", 40),
                                            end: foreign_expr("HI_CONST", 50),
                                        },
                                    ],
                                    body: vec![],
                                }],
                                default: None,
                            },
                            Span::new(20, 65),
                        ),
                        Stmt::new(
                            NodeId::new("n_fb"),
                            StmtKind::FbInvoke {
                                instance: indexed_lvalue("timers", "t_idx()", 70),
                                inputs: vec![],
                            },
                            Span::new(70, 90),
                        ),
                    ]),
                    span: Span::default(),
                },
                Pou {
                    id: NodeId::new("n_lad_lv"),
                    name: "LAD_LV".to_owned(),
                    kind: PouKind::Fb,
                    preferred_view: None,
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Network(dendri_ast::LadderNetwork {
                        rungs: vec![dendri_ast::LadderRung {
                            id: NodeId::new("n_r1"),
                            order: 0,
                            cells: vec![LadderCell {
                                id: NodeId::new("n_contact"),
                                row: 0,
                                col: 0,
                                kind: LadderCellKind::Contact {
                                    kind: dendri_ast::LadderContactKind::NormallyOpen,
                                    target: indexed_lvalue("inputs", "pin_idx()", 100),
                                },
                            }],
                        }],
                    }),
                    span: Span::default(),
                },
                Pou {
                    id: NodeId::new("n_sfc_lv"),
                    name: "SFC_LV".to_owned(),
                    kind: PouKind::Fb,
                    preferred_view: None,
                    params: vec![],
                    locals: vec![],
                    body: PouBody::Chart(dendri_ast::SfcChart {
                        steps: vec![dendri_ast::SfcStep {
                            id: NodeId::new("n_s0"),
                            name: "S0".to_owned(),
                            initial: true,
                            actions: vec![dendri_ast::SfcAction {
                                id: NodeId::new("n_a0"),
                                qualifier: dendri_ast::ActionQualifier::N,
                                duration: None,
                                target: Some(indexed_lvalue("coils", "coil_idx()", 120)),
                                body: vec![],
                            }],
                            macro_chart: None,
                        }],
                        transitions: vec![],
                        evolve_to_stability: false,
                    }),
                    span: Span::default(),
                },
            ],
        };

        let sites = collect_foreign_sites(&program);
        let texts: Vec<&str> = sites.iter().map(|s| s.text).collect();
        assert_eq!(
            texts,
            vec![
                "idx()",
                "MY_CONST",
                "LO_CONST",
                "HI_CONST",
                "t_idx()",
                "pin_idx()",
                "coil_idx()",
            ]
        );
        let ctx = BoardContext::arduino_uno_r3("mcu_1");
        let errs = check_program_foreign_export(&program, LanguageId::St, &ctx).unwrap_err();
        assert_eq!(errs.len(), 7);
        assert!(
            errs.iter()
                .all(|d| d.code == ErrorCode::ErrForeignExportBlocked && is_four_part_diagnostic(d))
        );
    }
}
