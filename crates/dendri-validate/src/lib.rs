//! Full static semantic validator for `dendriForge` v2 (`01-asl-ir-and-project-schema.md`).
//!
//! Covers all 45 frozen diagnostic codes across schema version, capabilities,
//! IDs/endpoints, hardware profiles/ports/voltage domains, channel codecs & bands,
//! module import graphs, type checking, OB/ISR/recursion safety, display charsets,
//! cross-language `Foreign` export gating, assumptions, and first-class comments.

pub mod profile_catalog;
mod validate_comments;
mod validate_types;

pub use profile_catalog::{
    BoardProfileMeta, ComponentProfileMeta, DisplayCharset, MemoryProfileCatalog, PortMeta,
    ProfileCatalog,
};

use crate::validate_comments::{CommentValidationCtx, validate_comment};
use crate::validate_types::{BoardTypeEnv, SymbolEntry, hardware_binding_type};
use dendri_ast::{
    AuthoringLanguage, Board, CodecError, CommentEncoding, Endpoint, Expr, ExprKind, HardwareKind,
    LadderBoxKind, LadderCell, LadderCellKind, NodeId, ObKind, Pou, PouBody, PouKind, Project,
    Stmt, StmtKind, TypeDecl, is_valid_instance_id, is_valid_profile_id,
};
use dendri_span::{Diagnostic, DiagnosticBag, ErrorCode, FileId, Span};
use dendri_stdlib::StdCatalog;
use std::collections::{BTreeMap, BTreeSet};

/// Supported ASL capability modules in the current engine release.
pub const SUPPORTED_CAPABILITIES: &[&str] = &["asl-core@1"];

/// Configurable validation options (export checks, encoding target, recursion limit).
#[derive(Debug, Clone)]
pub struct ValidateOptions {
    pub file_id: Option<FileId>,
    pub max_call_depth: usize,
    pub comment_encoding: CommentEncoding,
    /// When set, validates cross-language export rules (`ERR_FOREIGN_EXPORT_BLOCKED`, `INFO_COMMENT_RELOCATED`).
    pub export_target_language: Option<AuthoringLanguage>,
    /// Mirrors CLI `--allow-foreign` flag.
    pub allow_foreign_export: bool,
}

impl Default for ValidateOptions {
    fn default() -> Self {
        Self {
            file_id: None,
            max_call_depth: 64,
            comment_encoding: CommentEncoding::Utf8Passthrough,
            export_target_language: None,
            allow_foreign_export: false,
        }
    }
}

/// Validates a loaded [`Project`] against `catalog` and `options`, returning every
/// diagnostic in deterministic order.
#[must_use]
pub fn validate_project(
    project: &Project,
    catalog: &dyn ProfileCatalog,
    options: &ValidateOptions,
) -> DiagnosticBag {
    let mut bag = DiagnosticBag::new();
    let std_catalog = StdCatalog::builtin();

    // 1. Schema version & versioned capability negotiation (`01` §6.1)
    if project.meta.schema_version != "2.0" {
        bag.push(Diagnostic::new(
            ErrorCode::ErrUnsupportedVersion,
            format!(
                "unsupported schema_version `{}` (expected \"2.0\")",
                project.meta.schema_version
            ),
        ));
    }
    for cap in &project.meta.requires {
        if !SUPPORTED_CAPABILITIES.contains(&cap.as_str()) {
            bag.push(Diagnostic::new(
                ErrorCode::ErrUnsupportedCapability,
                format!(
                    "project requires unsupported capability `{cap}` (supported: {})",
                    SUPPORTED_CAPABILITIES.join(", ")
                ),
            ));
        }
    }

    // 2. Global instance ID uniqueness & profile validation
    let mut all_instance_ids = BTreeSet::new();
    let mut board_ids = BTreeSet::new();
    let mut component_ids = BTreeSet::new();
    let mut wire_ids = BTreeSet::new();
    let mut board_profile_by_inst: BTreeMap<&str, &BoardProfileMeta> = BTreeMap::new();
    let mut comp_profile_by_inst: BTreeMap<&str, &ComponentProfileMeta> = BTreeMap::new();
    let mut display_charsets: BTreeMap<String, DisplayCharset> = BTreeMap::new();

    for board in &project.boards {
        if !is_valid_instance_id(&board.id) {
            push_proj_diag(
                &mut bag,
                options.file_id,
                Some(&board.id),
                board.span,
                ErrorCode::ErrBadEndpoint,
                format!(
                    "invalid board instance id `{}` (expected `^[a-z][a-z0-9_]*$`)",
                    board.id
                ),
            );
        }
        if !all_instance_ids.insert(board.id.clone()) {
            push_proj_diag(
                &mut bag,
                options.file_id,
                Some(&board.id),
                board.span,
                ErrorCode::ErrDuplicateId,
                format!("duplicate instance id `{}`", board.id),
            );
        }
        board_ids.insert(board.id.clone());

        if !is_valid_profile_id(&board.profile) {
            push_proj_diag(
                &mut bag,
                options.file_id,
                Some(&board.id),
                board.span,
                ErrorCode::ErrUnknownProfile,
                format!("invalid board profile id `{}`", board.profile),
            );
        } else if let Some(meta) = catalog.board_profile(&board.profile) {
            board_profile_by_inst.insert(board.id.as_str(), meta);
        } else {
            push_proj_diag(
                &mut bag,
                options.file_id,
                Some(&board.id),
                board.span,
                ErrorCode::ErrUnknownProfile,
                format!("unknown board profile `{}`", board.profile),
            );
        }
    }

    for comp in &project.components {
        if !is_valid_instance_id(&comp.id) {
            push_proj_diag(
                &mut bag,
                options.file_id,
                None,
                comp.span,
                ErrorCode::ErrBadEndpoint,
                format!(
                    "invalid component instance id `{}` (expected `^[a-z][a-z0-9_]*$`)",
                    comp.id
                ),
            );
        }
        if !all_instance_ids.insert(comp.id.clone()) {
            push_proj_diag(
                &mut bag,
                options.file_id,
                None,
                comp.span,
                ErrorCode::ErrDuplicateId,
                format!("duplicate instance id `{}`", comp.id),
            );
        }
        component_ids.insert(comp.id.clone());

        if !is_valid_profile_id(&comp.profile) {
            push_proj_diag(
                &mut bag,
                options.file_id,
                None,
                comp.span,
                ErrorCode::ErrUnknownProfile,
                format!("invalid component profile id `{}`", comp.profile),
            );
        } else if let Some(meta) = catalog.component_profile(&comp.profile) {
            comp_profile_by_inst.insert(comp.id.as_str(), meta);
            if let Some(cs) = meta.display_charset {
                display_charsets.insert(comp.id.clone(), cs);
            }
        } else {
            push_proj_diag(
                &mut bag,
                options.file_id,
                None,
                comp.span,
                ErrorCode::ErrUnknownProfile,
                format!("unknown component profile `{}`", comp.profile),
            );
        }
    }

    // 3. Wire endpoints, port existence & voltage-domain safety
    let mut wired_ports_by_board: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for wire in &project.wires {
        if !wire_ids.insert(wire.id.clone()) {
            push_proj_diag(
                &mut bag,
                options.file_id,
                None,
                wire.span,
                ErrorCode::ErrDuplicateId,
                format!("duplicate wire id `{}`", wire.id),
            );
        }

        let from_meta = resolve_wire_endpoint(
            &wire.from,
            wire.span,
            &all_instance_ids,
            &board_profile_by_inst,
            &comp_profile_by_inst,
            &mut wired_ports_by_board,
            options.file_id,
            &mut bag,
        );
        let to_meta = resolve_wire_endpoint(
            &wire.to,
            wire.span,
            &all_instance_ids,
            &board_profile_by_inst,
            &comp_profile_by_inst,
            &mut wired_ports_by_board,
            options.file_id,
            &mut bag,
        );

        if let (Some(a), Some(b)) = (from_meta, to_meta)
            && (a.nominal_v > b.v_abs_max_v || b.nominal_v > a.v_abs_max_v)
        {
            push_proj_diag(
                &mut bag,
                options.file_id,
                None,
                wire.span,
                ErrorCode::ErrUnsafeVoltageDomain,
                format!(
                    "wire `{}` connects incompatible voltage domains: `{}` ({}V, abs max {}V) <-> `{}` ({}V, abs max {}V)",
                    wire.id,
                    wire.from,
                    a.nominal_v,
                    a.v_abs_max_v,
                    wire.to,
                    b.nominal_v,
                    b.v_abs_max_v
                ),
            );
        }
    }

    // 4. Library names for import resolution
    let library_names: BTreeSet<&str> = project.libraries.iter().map(|l| l.name.as_str()).collect();

    // 5. Top-level project comments
    let comment_ctx = CommentValidationCtx {
        file_id: options.file_id,
        board_ids: &board_ids,
        component_ids: &component_ids,
        wire_ids: &wire_ids,
        comment_encoding: options.comment_encoding,
        export_target_language: options.export_target_language,
    };
    let mut seen_comment_ids = BTreeSet::new();
    for comment in &project.comments {
        validate_comment(
            comment,
            None,
            None,
            &comment_ctx,
            &mut seen_comment_ids,
            &mut bag,
        );
    }

    // 6. Per-board semantic validation
    for board in &project.boards {
        let board_profile = board_profile_by_inst.get(board.id.as_str()).copied();
        let empty_wired = BTreeSet::new();
        let wired_ports = wired_ports_by_board.get(&board.id).unwrap_or(&empty_wired);

        validate_board(
            board,
            board_profile,
            wired_ports,
            &display_charsets,
            &library_names,
            &std_catalog,
            &comment_ctx,
            &mut seen_comment_ids,
            options,
            &mut bag,
        );
    }

    bag
}

#[allow(clippy::too_many_arguments)]
fn resolve_wire_endpoint<'a>(
    raw: &str,
    span: Span,
    all_instance_ids: &BTreeSet<String>,
    boards: &BTreeMap<&str, &'a BoardProfileMeta>,
    comps: &BTreeMap<&str, &'a ComponentProfileMeta>,
    wired_ports_by_board: &mut BTreeMap<String, BTreeSet<String>>,
    file_id: Option<FileId>,
    bag: &mut DiagnosticBag,
) -> Option<&'a PortMeta> {
    let ep = match Endpoint::parse(raw) {
        Ok(ep) => ep,
        Err(err) => {
            push_proj_diag(
                bag,
                file_id,
                None,
                span,
                ErrorCode::ErrBadEndpoint,
                err.to_string(),
            );
            return None;
        }
    };
    if !all_instance_ids.contains(&ep.instance_id) {
        push_proj_diag(
            bag,
            file_id,
            None,
            span,
            ErrorCode::ErrBadEndpoint,
            format!(
                "wire endpoint `{raw}` references unknown instance `{}`",
                ep.instance_id
            ),
        );
        return None;
    }
    if let Some(bm) = boards.get(ep.instance_id.as_str()) {
        if let Some(pm) = bm.ports.get(&ep.port_id) {
            wired_ports_by_board
                .entry(ep.instance_id)
                .or_default()
                .insert(ep.port_id);
            return Some(pm);
        }
        push_proj_diag(
            bag,
            file_id,
            Some(&ep.instance_id),
            span,
            ErrorCode::ErrUnknownPort,
            format!("board profile `{}` has no port `{}`", bm.id, ep.port_id),
        );
        return None;
    }
    if let Some(cm) = comps.get(ep.instance_id.as_str()) {
        if let Some(pm) = cm.ports.get(&ep.port_id) {
            return Some(pm);
        }
        push_proj_diag(
            bag,
            file_id,
            None,
            span,
            ErrorCode::ErrUnknownPort,
            format!("component profile `{}` has no port `{}`", cm.id, ep.port_id),
        );
        return None;
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn validate_board(
    board: &Board,
    board_profile: Option<&BoardProfileMeta>,
    wired_ports: &BTreeSet<String>,
    display_charsets: &BTreeMap<String, DisplayCharset>,
    library_names: &BTreeSet<&str>,
    std_catalog: &StdCatalog,
    comment_ctx: &CommentValidationCtx<'_>,
    seen_comment_ids: &mut BTreeSet<String>,
    options: &ValidateOptions,
    bag: &mut DiagnosticBag,
) {
    let bid = board.id.as_str();
    let push = |bag: &mut DiagnosticBag, code: ErrorCode, span: Span, msg: String| {
        push_proj_diag(bag, options.file_id, Some(bid), span, code, msg);
    };

    // 1. Collect all NodeIds in this board (check duplicate NodeIds -> ERR_DUPLICATE_ID)
    let mut board_node_ids = BTreeSet::new();
    collect_board_node_ids(board, &mut board_node_ids, options.file_id, bag);

    // 2. Validate board profile codecs & bands (`ERR_CODEC_RESOLUTION_EXCEEDS_SCALE`, `ERR_BAND_LIMIT_OVERFLOW`)
    if let Some(bp) = board_profile {
        for codec in bp.codecs.values() {
            if let Err(err) = codec.validate() {
                let code = match err {
                    CodecError::ResolutionExceedsScale { .. } => {
                        ErrorCode::ErrCodecResolutionExceedsScale
                    }
                    CodecError::BandLimitOverflow { .. } => ErrorCode::ErrBandLimitOverflow,
                    CodecError::InvalidRange { .. } | CodecError::InvalidBandOrder { .. } => {
                        ErrorCode::ErrPinCapabilityMismatch
                    }
                };
                push(bag, code, board.span, err.to_string());
            }
        }
        for (port_id, band) in &bp.channel_bands {
            // Find codec used by hardware on this port, or first span/bipolar codec on board
            let bound_codec = board
                .program
                .hardware
                .iter()
                .find(|h| &h.pin == port_id)
                .and_then(|h| h.codec.as_deref())
                .and_then(|cid| bp.codecs.get(cid))
                .or_else(|| bp.codecs.values().next());
            if let Some(codec) = bound_codec
                && let Err(err) = band.validate_against(codec)
            {
                let code = match err {
                    CodecError::BandLimitOverflow { .. } => ErrorCode::ErrBandLimitOverflow,
                    CodecError::ResolutionExceedsScale { .. } => {
                        ErrorCode::ErrCodecResolutionExceedsScale
                    }
                    CodecError::InvalidRange { .. } | CodecError::InvalidBandOrder { .. } => {
                        ErrorCode::ErrPinCapabilityMismatch
                    }
                };
                push(bag, code, board.span, err.to_string());
            }
        }
    }

    // 3. Validate module names, imports & cycle detection (`ERR_IMPORT_CYCLE`, `ERR_UNRESOLVED_IMPORT`)
    let mut module_names = BTreeSet::new();
    for m in &board.program.modules {
        if !module_names.insert(m.name.clone()) {
            push(
                bag,
                ErrorCode::ErrDuplicateDecl,
                m.span,
                format!("duplicate module `{}` on board `{bid}`", m.name),
            );
        }
    }
    let check_import = |imp: &str, span: Span, bag: &mut DiagnosticBag| {
        if is_known_std_import(imp, std_catalog) {
            return;
        }
        if let Some(local_mod) = imp.strip_prefix("project.")
            && module_names.contains(local_mod)
        {
            return;
        }
        if let Some((lib_name, _rest)) = imp.split_once('.')
            && library_names.contains(lib_name)
        {
            return;
        }
        push(
            bag,
            ErrorCode::ErrUnresolvedImport,
            span,
            format!("unresolved module import `{imp}`"),
        );
    };
    for imp in &board.program.imports {
        check_import(imp, board.span, bag);
    }
    for m in &board.program.modules {
        for imp in &m.imports {
            check_import(imp, m.span, bag);
        }
    }
    check_module_import_cycles(board, options.file_id, bag);

    // 4. Validate [[hardware]] pin uniqueness, ports, capabilities, codecs & ranges
    let mut used_pins = BTreeSet::new();
    let mut hw_labels = BTreeSet::new();
    let mut globals = BTreeMap::new();

    for hw in &board.program.hardware {
        if !used_pins.insert(hw.pin.clone()) {
            push(
                bag,
                ErrorCode::ErrHardwarePinConflict,
                hw.span,
                format!(
                    "hardware pin `{}` is bound more than once on board `{bid}`",
                    hw.pin
                ),
            );
        }
        if !hw_labels.insert(hw.label.clone()) {
            push(
                bag,
                ErrorCode::ErrDuplicateDecl,
                hw.span,
                format!("duplicate hardware label `{}`", hw.label),
            );
        }
        if let Some(bp) = board_profile {
            if let Some(pm) = bp.ports.get(&hw.pin) {
                if !pm.supports_hardware_kind(hw.kind) {
                    push(
                        bag,
                        ErrorCode::ErrPinCapabilityMismatch,
                        hw.span,
                        format!(
                            "port `{}` on `{}` does not support {:?}",
                            hw.pin, bp.id, hw.kind
                        ),
                    );
                }
                if !pm.supports_pull(hw.pull) {
                    push(
                        bag,
                        ErrorCode::ErrPinCapabilityMismatch,
                        hw.span,
                        format!(
                            "port `{}` on `{}` does not support {:?}",
                            hw.pin, bp.id, hw.pull
                        ),
                    );
                }
            } else {
                push(
                    bag,
                    ErrorCode::ErrUnknownPort,
                    hw.span,
                    format!("board profile `{}` has no port `{}`", bp.id, hw.pin),
                );
            }

            if matches!(
                hw.kind,
                HardwareKind::AnalogIn | HardwareKind::AnalogOut | HardwareKind::PwmOut
            ) {
                match &hw.codec {
                    Some(cid) => {
                        if !bp.codecs.contains_key(cid) {
                            push(
                                bag,
                                ErrorCode::ErrPinCapabilityMismatch,
                                hw.span,
                                format!(
                                    "codec `{cid}` is not declared on board profile `{}`",
                                    bp.id
                                ),
                            );
                        }
                    }
                    None => {
                        push(
                            bag,
                            ErrorCode::ErrPinCapabilityMismatch,
                            hw.span,
                            format!(
                                "hardware `{}` ({:?}) requires a `codec` binding",
                                hw.label, hw.kind
                            ),
                        );
                    }
                }
            }
            if matches!(hw.kind, HardwareKind::AnalogIn | HardwareKind::AnalogOut)
                && hw.range.is_none()
            {
                push(
                    bag,
                    ErrorCode::ErrPinCapabilityMismatch,
                    hw.span,
                    format!(
                        "analog hardware `{}` requires a `range` descriptor",
                        hw.label
                    ),
                );
            }
        }
        let ty = hardware_binding_type(hw);
        globals.insert(
            hw.label.clone(),
            SymbolEntry::Hardware {
                binding: hw.clone(),
                ty,
            },
        );
    }

    // 5. Collect types, data, and POUs across program + embedded modules; check collisions & duplicates
    let mut env = BoardTypeEnv {
        board_id: bid,
        file_id: options.file_id,
        std_catalog,
        imports: board.program.imports.clone(),
        board_profile,
        wired_ports,
        display_charsets,
        structs: BTreeMap::new(),
        enums: BTreeMap::new(),
        foreign_types: BTreeSet::new(),
        user_fbs: BTreeMap::new(),
        user_fcs: BTreeMap::new(),
        globals,
    };

    let all_types = board
        .program
        .types
        .iter()
        .chain(board.program.modules.iter().flat_map(|m| m.types.iter()));
    let mut seen_types = BTreeSet::new();
    for td in all_types {
        match td {
            TypeDecl::Struct {
                name, fields, span, ..
            } => {
                if hw_labels.contains(name) {
                    push(
                        bag,
                        ErrorCode::ErrNameCollision,
                        *span,
                        format!("type `{name}` collides with hardware label `{name}`"),
                    );
                }
                if !seen_types.insert(name.clone()) {
                    push(
                        bag,
                        ErrorCode::ErrDuplicateDecl,
                        *span,
                        format!("duplicate type declaration `{name}`"),
                    );
                }
                let mut f_map = BTreeMap::new();
                for f in fields {
                    f_map.insert(f.name.clone(), f.ty.clone());
                }
                env.structs.insert(name.clone(), f_map);
            }
            TypeDecl::Enum {
                name,
                backing,
                span,
                ..
            } => {
                if !seen_types.insert(name.clone()) {
                    push(
                        bag,
                        ErrorCode::ErrDuplicateDecl,
                        *span,
                        format!("duplicate enum declaration `{name}`"),
                    );
                }
                env.enums.insert(name.clone(), backing.clone());
            }
            TypeDecl::Foreign { name, .. } => {
                if let Some(n) = name {
                    env.foreign_types.insert(n.clone());
                }
            }
        }
    }

    let all_data: Vec<&dendri_ast::DataDecl> = board
        .program
        .data
        .iter()
        .chain(board.program.modules.iter().flat_map(|m| m.data.iter()))
        .collect();
    let mut seen_data = BTreeSet::new();
    for d in &all_data {
        if hw_labels.contains(&d.name) {
            push(
                bag,
                ErrorCode::ErrNameCollision,
                d.span,
                format!(
                    "variable `{}` collides with hardware label `{}`",
                    d.name, d.name
                ),
            );
        } else if !seen_data.insert(d.name.clone()) {
            push(
                bag,
                ErrorCode::ErrDuplicateDecl,
                d.span,
                format!("duplicate data declaration `{}`", d.name),
            );
        } else {
            env.globals.insert(
                d.name.clone(),
                SymbolEntry::Data {
                    ty: d.ty.clone(),
                    scope: d.scope,
                },
            );
        }
    }

    let all_pous: Vec<&Pou> = board
        .program
        .pous
        .iter()
        .chain(board.program.modules.iter().flat_map(|m| m.pous.iter()))
        .collect();
    let mut seen_pous = BTreeSet::new();
    for pou in &all_pous {
        if hw_labels.contains(&pou.name) {
            push(
                bag,
                ErrorCode::ErrNameCollision,
                pou.span,
                format!(
                    "POU `{}` collides with hardware label `{}`",
                    pou.name, pou.name
                ),
            );
        }
        if !seen_pous.insert(pou.name.clone()) {
            push(
                bag,
                ErrorCode::ErrDuplicateDecl,
                pou.span,
                format!("duplicate POU declaration `{}`", pou.name),
            );
        }
        match &pou.kind {
            PouKind::Fc { return_type } => {
                env.user_fcs
                    .insert(pou.name.clone(), (pou.params.clone(), return_type.clone()));
            }
            PouKind::Fb => {
                let mut fb_fields = BTreeMap::new();
                for p in &pou.params {
                    fb_fields.insert(p.name.clone(), p.ty.clone());
                }
                env.user_fbs.insert(pou.name.clone(), fb_fields);
            }
            PouKind::Ob {
                ob: ObKind::HwInt { pin_label, .. },
            } => {
                if !hw_labels.contains(pin_label) {
                    push(
                        bag,
                        ErrorCode::ErrUnresolvedIsr,
                        pou.span,
                        format!(
                            "hardware interrupt OB `{}` references undeclared hardware label `{pin_label}`",
                            pou.name
                        ),
                    );
                }
            }
            PouKind::Ob {
                ob:
                    ObKind::Startup | ObKind::Main | ObKind::CyclicInt { .. } | ObKind::CommEvent { .. },
            } => {}
        }
    }

    // 6. Validate data declarations & every POU body
    let empty_locals = BTreeMap::new();
    env.validate_data_decls(&board.program.data, &empty_locals, bag);
    for m in &board.program.modules {
        env.validate_data_decls(&m.data, &empty_locals, bag);
    }
    for pou in &all_pous {
        env.validate_pou(pou, bag);
    }

    // 7. Static call-graph recursion & depth check (`ERR_RECURSION_DEPTH`)
    check_call_graph_recursion(&all_pous, options.max_call_depth, bid, options.file_id, bag);

    // 8. Cross-language export gate for `Foreign` nodes (`ERR_FOREIGN_EXPORT_BLOCKED`)
    if let Some(target_lang) = options.export_target_language
        && !options.allow_foreign_export
    {
        check_foreign_export_blocked(board, &all_pous, target_lang, options.file_id, bag);
    }

    // 9. Validate [[boards.assumptions]] (`WARN_ASSUMPTION_ACTIVE`)
    for assumption in &board.assumptions {
        if !board_node_ids.contains(&assumption.node) {
            push(
                bag,
                ErrorCode::ErrCommentAnchor,
                assumption.span,
                format!("assumption targets unresolved node `{}`", assumption.node),
            );
        }
        push(
            bag,
            ErrorCode::WarnAssumptionActive,
            assumption.span,
            format!(
                "active simulation assumption on `{}` ({:?}): {}",
                assumption.node, assumption.policy, assumption.note
            ),
        );
    }

    // 10. Validate [[boards.comments]]
    for comment in &board.comments {
        validate_comment(
            comment,
            Some(bid),
            Some(&board_node_ids),
            comment_ctx,
            seen_comment_ids,
            bag,
        );
    }
}

fn is_known_std_import(imp: &str, std_catalog: &StdCatalog) -> bool {
    if std_catalog.get_module(imp).is_some() {
        return true;
    }
    if let Some(prefix) = imp.strip_suffix(".*") {
        return std_catalog.modules().any(|m| {
            m.path.starts_with(prefix) && m.path.as_bytes().get(prefix.len()) == Some(&b'.')
        });
    }
    false
}

fn check_module_import_cycles(board: &Board, file_id: Option<FileId>, bag: &mut DiagnosticBag) {
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut spans: BTreeMap<&str, Span> = BTreeMap::new();
    for m in &board.program.modules {
        spans.insert(m.name.as_str(), m.span);
        let deps = m
            .imports
            .iter()
            .filter_map(|imp| imp.strip_prefix("project."))
            .collect();
        adj.insert(m.name.as_str(), deps);
    }

    let mut state: BTreeMap<&str, u8> = BTreeMap::new(); // 0=unvisited, 1=visiting, 2=done
    for m in &board.program.modules {
        if state.get(m.name.as_str()).copied().unwrap_or(0) == 0
            && dfs_cycle(m.name.as_str(), &adj, &mut state)
        {
            push_proj_diag(
                bag,
                file_id,
                Some(&board.id),
                spans.get(m.name.as_str()).copied().unwrap_or(board.span),
                ErrorCode::ErrImportCycle,
                format!("cyclic module import involving `project.{}`", m.name),
            );
        }
    }
}

fn dfs_cycle<'a>(
    node: &'a str,
    adj: &BTreeMap<&'a str, Vec<&'a str>>,
    state: &mut BTreeMap<&'a str, u8>,
) -> bool {
    state.insert(node, 1);
    if let Some(neighbors) = adj.get(node) {
        for &next in neighbors {
            match state.get(next).copied().unwrap_or(0) {
                1 => return true,
                0 if dfs_cycle(next, adj, state) => return true,
                _ => {}
            }
        }
    }
    state.insert(node, 2);
    false
}

fn check_call_graph_recursion(
    pous: &[&Pou],
    max_depth: usize,
    board_id: &str,
    file_id: Option<FileId>,
    bag: &mut DiagnosticBag,
) {
    let mut call_edges: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    let mut pou_spans: BTreeMap<&str, Span> = BTreeMap::new();
    let pou_names: BTreeSet<&str> = pous.iter().map(|p| p.name.as_str()).collect();

    for pou in pous {
        pou_spans.insert(pou.name.as_str(), pou.span);
        let mut callees = BTreeSet::new();
        collect_pou_callees(pou, &mut callees);
        let local_callees: BTreeSet<String> = callees
            .into_iter()
            .filter(|c| pou_names.contains(c.as_str()))
            .collect();
        call_edges.insert(pou.name.as_str(), local_callees);
    }

    let mut reported = BTreeSet::new();
    for pou in pous {
        let mut stack = Vec::new();
        if exceeds_call_depth(pou.name.as_str(), &call_edges, max_depth, &mut stack)
            && reported.insert(pou.name.as_str())
        {
            push_proj_diag(
                bag,
                file_id,
                Some(board_id),
                pou_spans
                    .get(pou.name.as_str())
                    .copied()
                    .unwrap_or(pou.span),
                ErrorCode::ErrRecursionDepth,
                format!(
                    "POU `{}` has unbounded recursion or exceeds max_call_depth ({max_depth})",
                    pou.name
                ),
            );
        }
    }
}

fn exceeds_call_depth<'a>(
    node: &'a str,
    edges: &'a BTreeMap<&'a str, BTreeSet<String>>,
    max_depth: usize,
    stack: &mut Vec<&'a str>,
) -> bool {
    if stack.contains(&node) || stack.len() > max_depth {
        return true;
    }
    stack.push(node);
    if let Some(callees) = edges.get(node) {
        for callee in callees {
            if exceeds_call_depth(callee.as_str(), edges, max_depth, stack) {
                stack.pop();
                return true;
            }
        }
    }
    stack.pop();
    false
}

fn collect_pou_callees(pou: &Pou, out: &mut BTreeSet<String>) {
    match &pou.body {
        PouBody::Statements(stmts) => collect_stmts_callees(stmts, out),
        PouBody::Network(net) => {
            for r in &net.rungs {
                for c in &r.cells {
                    collect_ladder_cell_callees(c, out);
                }
            }
        }
        PouBody::Chart(chart) => {
            for s in &chart.steps {
                for a in &s.actions {
                    collect_stmts_callees(&a.body, out);
                }
            }
            for t in &chart.transitions {
                collect_expr_callees(&t.receptivity, out);
            }
        }
    }
}

fn collect_stmts_callees(stmts: &[Stmt], out: &mut BTreeSet<String>) {
    for s in stmts {
        match &s.kind {
            StmtKind::Assign { value, .. } => collect_expr_callees(value, out),
            StmtKind::Call { callee, args } => {
                out.insert(callee.clone());
                for a in args {
                    collect_expr_callees(&a.value, out);
                }
            }
            StmtKind::FbInvoke { inputs, .. } => {
                for (_, e) in inputs {
                    collect_expr_callees(e, out);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                collect_expr_callees(condition, out);
                collect_stmts_callees(then_body, out);
                for arm in elif_arms {
                    collect_expr_callees(&arm.condition, out);
                    collect_stmts_callees(&arm.body, out);
                }
                collect_stmts_callees(else_body, out);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                collect_expr_callees(scrutinee, out);
                for c in cases {
                    collect_stmts_callees(&c.body, out);
                }
                if let Some(d) = default {
                    collect_stmts_callees(d, out);
                }
            }
            StmtKind::While { condition, body } => {
                collect_expr_callees(condition, out);
                collect_stmts_callees(body, out);
            }
            StmtKind::For {
                from,
                to,
                step,
                body,
                ..
            } => {
                collect_expr_callees(from, out);
                collect_expr_callees(to, out);
                collect_expr_callees(step, out);
                collect_stmts_callees(body, out);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    collect_expr_callees(v, out);
                }
            }
            StmtKind::Wait { duration } => collect_expr_callees(duration, out),
            StmtKind::Break | StmtKind::Continue | StmtKind::Foreign { .. } => {}
        }
    }
}

fn collect_ladder_cell_callees(cell: &LadderCell, out: &mut BTreeSet<String>) {
    match &cell.kind {
        LadderCellKind::ParallelOr { branches } => {
            for br in branches {
                for c in br {
                    collect_ladder_cell_callees(c, out);
                }
            }
        }
        LadderCellKind::Box(LadderBoxKind::Call { callee, args, .. }) => {
            out.insert(callee.clone());
            for (_, e) in args {
                collect_expr_callees(e, out);
            }
        }
        LadderCellKind::Box(LadderBoxKind::Compare { left, right, .. }) => {
            collect_expr_callees(left, out);
            collect_expr_callees(right, out);
        }
        LadderCellKind::Box(LadderBoxKind::Fb { inputs, .. }) => {
            for (_, e) in inputs {
                collect_expr_callees(e, out);
            }
        }
        LadderCellKind::Empty
        | LadderCellKind::Wire
        | LadderCellKind::Contact { .. }
        | LadderCellKind::Coil { .. }
        | LadderCellKind::Box(LadderBoxKind::Foreign { .. }) => {}
    }
}

fn collect_expr_callees(expr: &Expr, out: &mut BTreeSet<String>) {
    match &expr.kind {
        ExprKind::Call { callee, args } => {
            out.insert(callee.clone());
            for a in args {
                collect_expr_callees(&a.value, out);
            }
        }
        ExprKind::Index { base, index } => {
            collect_expr_callees(base, out);
            collect_expr_callees(index, out);
        }
        ExprKind::Field { base, .. } | ExprKind::Unary { operand: base, .. } => {
            collect_expr_callees(base, out);
        }
        ExprKind::Binary { left, right, .. } => {
            collect_expr_callees(left, out);
            collect_expr_callees(right, out);
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::Foreign { .. } => {}
    }
}

fn check_foreign_export_blocked(
    board: &Board,
    pous: &[&Pou],
    target: AuthoringLanguage,
    file_id: Option<FileId>,
    bag: &mut DiagnosticBag,
) {
    let target_str = target.as_str();
    let mut emit_blocked = |lang: &str, text: &str, span: Span| {
        if lang != target_str {
            push_proj_diag(
                bag,
                file_id,
                Some(&board.id),
                span,
                ErrorCode::ErrForeignExportBlocked,
                format!(
                    "cannot export `{lang}` Foreign construct `{text}` to `{target_str}` without `--allow-foreign`"
                ),
            );
        }
    };

    for td in board
        .program
        .types
        .iter()
        .chain(board.program.modules.iter().flat_map(|m| m.types.iter()))
    {
        if let TypeDecl::Foreign {
            language,
            text,
            span,
            ..
        } = td
        {
            emit_blocked(language, text, *span);
        }
    }
    for pou in pous {
        match &pou.body {
            PouBody::Statements(stmts) => check_stmts_foreign(stmts, &mut emit_blocked),
            PouBody::Network(net) => {
                for r in &net.rungs {
                    for c in &r.cells {
                        check_cell_foreign(c, pou.span, &mut emit_blocked);
                    }
                }
            }
            PouBody::Chart(chart) => {
                for s in &chart.steps {
                    for a in &s.actions {
                        check_stmts_foreign(&a.body, &mut emit_blocked);
                    }
                }
            }
        }
    }
}

fn check_stmts_foreign(stmts: &[Stmt], cb: &mut impl FnMut(&str, &str, Span)) {
    for s in stmts {
        match &s.kind {
            StmtKind::Foreign { language, text, .. } => cb(language, text, s.span),
            StmtKind::Assign { value, .. } => check_expr_foreign(value, cb),
            StmtKind::Call { args, .. } => {
                for a in args {
                    check_expr_foreign(&a.value, cb);
                }
            }
            StmtKind::FbInvoke { inputs, .. } => {
                for (_, e) in inputs {
                    check_expr_foreign(e, cb);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                check_expr_foreign(condition, cb);
                check_stmts_foreign(then_body, cb);
                for arm in elif_arms {
                    check_expr_foreign(&arm.condition, cb);
                    check_stmts_foreign(&arm.body, cb);
                }
                check_stmts_foreign(else_body, cb);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                check_expr_foreign(scrutinee, cb);
                for c in cases {
                    check_stmts_foreign(&c.body, cb);
                }
                if let Some(d) = default {
                    check_stmts_foreign(d, cb);
                }
            }
            StmtKind::While { condition, body } => {
                check_expr_foreign(condition, cb);
                check_stmts_foreign(body, cb);
            }
            StmtKind::For {
                from,
                to,
                step,
                body,
                ..
            } => {
                check_expr_foreign(from, cb);
                check_expr_foreign(to, cb);
                check_expr_foreign(step, cb);
                check_stmts_foreign(body, cb);
            }
            StmtKind::Return { value } => {
                if let Some(v) = value {
                    check_expr_foreign(v, cb);
                }
            }
            StmtKind::Wait { duration } => check_expr_foreign(duration, cb),
            StmtKind::Break | StmtKind::Continue => {}
        }
    }
}

fn check_cell_foreign(cell: &LadderCell, span: Span, cb: &mut impl FnMut(&str, &str, Span)) {
    match &cell.kind {
        LadderCellKind::Box(LadderBoxKind::Foreign { language, text, .. }) => {
            cb(language, text, span);
        }
        LadderCellKind::ParallelOr { branches } => {
            for br in branches {
                for c in br {
                    check_cell_foreign(c, span, cb);
                }
            }
        }
        LadderCellKind::Empty
        | LadderCellKind::Wire
        | LadderCellKind::Contact { .. }
        | LadderCellKind::Coil { .. }
        | LadderCellKind::Box(
            LadderBoxKind::Fb { .. } | LadderBoxKind::Compare { .. } | LadderBoxKind::Call { .. },
        ) => {}
    }
}

fn check_expr_foreign(expr: &Expr, cb: &mut impl FnMut(&str, &str, Span)) {
    match &expr.kind {
        ExprKind::Foreign { language, text } => cb(language, text, expr.span),
        ExprKind::Index { base, index } => {
            check_expr_foreign(base, cb);
            check_expr_foreign(index, cb);
        }
        ExprKind::Field { base, .. } | ExprKind::Unary { operand: base, .. } => {
            check_expr_foreign(base, cb);
        }
        ExprKind::Call { args, .. } => {
            for a in args {
                check_expr_foreign(&a.value, cb);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            check_expr_foreign(left, cb);
            check_expr_foreign(right, cb);
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
    }
}

fn collect_board_node_ids(
    board: &Board,
    out: &mut BTreeSet<NodeId>,
    file_id: Option<FileId>,
    bag: &mut DiagnosticBag,
) {
    let mut record = |id: &NodeId, span: Span| {
        if !out.insert(id.clone()) {
            push_proj_diag(
                bag,
                file_id,
                Some(&board.id),
                span,
                ErrorCode::ErrDuplicateId,
                format!("duplicate NodeId `{id}` on board `{}`", board.id),
            );
        }
    };

    for h in &board.program.hardware {
        record(&h.id, h.span);
    }
    for td in &board.program.types {
        match td {
            TypeDecl::Struct { id, span, .. }
            | TypeDecl::Enum { id, span, .. }
            | TypeDecl::Foreign { id, span, .. } => record(id, *span),
        }
    }
    for d in &board.program.data {
        record(&d.id, d.span);
    }
    for m in &board.program.modules {
        record(&m.id, m.span);
        for td in &m.types {
            match td {
                TypeDecl::Struct { id, span, .. }
                | TypeDecl::Enum { id, span, .. }
                | TypeDecl::Foreign { id, span, .. } => record(id, *span),
            }
        }
        for d in &m.data {
            record(&d.id, d.span);
        }
        for p in &m.pous {
            collect_pou_node_ids(p, &mut record);
        }
    }
    for p in &board.program.pous {
        collect_pou_node_ids(p, &mut record);
    }
}

fn collect_pou_node_ids(pou: &Pou, record: &mut impl FnMut(&NodeId, Span)) {
    record(&pou.id, pou.span);
    for l in &pou.locals {
        record(&l.id, l.span);
    }
    match &pou.body {
        PouBody::Statements(stmts) => collect_stmts_node_ids(stmts, record),
        PouBody::Network(net) => {
            for r in &net.rungs {
                record(&r.id, pou.span);
                for c in &r.cells {
                    collect_cell_node_ids(c, pou.span, record);
                }
            }
        }
        PouBody::Chart(chart) => {
            for s in &chart.steps {
                record(&s.id, pou.span);
                for a in &s.actions {
                    record(&a.id, pou.span);
                    collect_stmts_node_ids(&a.body, record);
                }
            }
            for t in &chart.transitions {
                record(&t.id, pou.span);
            }
        }
    }
}

fn collect_stmts_node_ids(stmts: &[Stmt], record: &mut impl FnMut(&NodeId, Span)) {
    for s in stmts {
        record(&s.id, s.span);
        match &s.kind {
            StmtKind::If {
                then_body,
                elif_arms,
                else_body,
                ..
            } => {
                collect_stmts_node_ids(then_body, record);
                for arm in elif_arms {
                    collect_stmts_node_ids(&arm.body, record);
                }
                collect_stmts_node_ids(else_body, record);
            }
            StmtKind::Switch { cases, default, .. } => {
                for c in cases {
                    collect_stmts_node_ids(&c.body, record);
                }
                if let Some(d) = default {
                    collect_stmts_node_ids(d, record);
                }
            }
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => {
                collect_stmts_node_ids(body, record);
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

fn collect_cell_node_ids(cell: &LadderCell, span: Span, record: &mut impl FnMut(&NodeId, Span)) {
    record(&cell.id, span);
    if let LadderCellKind::ParallelOr { branches } = &cell.kind {
        for br in branches {
            for c in br {
                collect_cell_node_ids(c, span, record);
            }
        }
    }
}

fn push_proj_diag(
    bag: &mut DiagnosticBag,
    file_id: Option<FileId>,
    board_id: Option<&str>,
    span: Span,
    code: ErrorCode,
    message: String,
) {
    let mut diag = Diagnostic::new(code, message);
    if let Some(bid) = board_id {
        diag = diag.with_board_id(bid);
    }
    if let Some(fid) = file_id {
        diag = diag.with_location(fid, span);
    } else {
        diag.span = Some(span);
    }
    bag.push(diag);
}
