//! Non-destructive AST, comment (`C2`/`C4`), assumption, and presentation merge engine
//! (`04-frontends-visual-and-sync.md` §4.3, §6).
//!
//! Guarantees:
//! - Editing a single POU never clobbers untouched OBs (`OB_STARTUP`, `OB_HW_INT`), sibling POUs,
//!   `Foreign` nodes, `[[boards.assumptions]]`, `[boards.presentation]`, or board metadata.
//! - Re-import comment idempotence (`C4`): degraded projections matching via
//!   `frontend.matches_comment` never overwrite verbatim stored originals in `[[boards.comments]]`.
//! - Orphan preservation (`C2`): deleting any anchored `NodeId` transitions its comments to
//!   `dangling` on the nearest surviving parent scope (`INFO_COMMENT_ORPHANED`), never deleting
//!   user commentary.

use dendri_ast::{
    Assumption, Board, BoardPresentation, Comment, CommentAnchor,
    CommentPlacement as AstCommentPlacement, DataDecl, HardwareBinding, LadderCell, LadderCellKind,
    ModuleDecl, NodeId, Pou, PouBody, Stmt, StmtKind, TypeDecl,
};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, LanguageFrontend, matches_comment_with_ctx,
    normalize_eol_to_lf, orphan_comments_for_deleted_nodes,
};
use dendri_span::{Diagnostic, ErrorCode};
use std::collections::{BTreeMap, BTreeSet};

/// Complete `NodeId` inventory and parent-scope chain for a [`Board`] or subset of POUs.
#[derive(Debug, Clone, Default)]
pub(crate) struct NodeTopology {
    pub(crate) all_nodes: BTreeSet<NodeId>,
    pub(crate) parent_of: BTreeMap<NodeId, NodeId>,
}

impl NodeTopology {
    pub(crate) fn insert_node(&mut self, id: &NodeId, parent: Option<&NodeId>) {
        self.all_nodes.insert(id.clone());
        if let Some(p) = parent {
            self.parent_of.insert(id.clone(), p.clone());
        }
    }
}

/// Output of merging a parsed [`BoardProgramBundle`] into an existing [`Board`].
#[derive(Debug, Clone)]
pub(crate) struct MergeOutcome {
    pub(crate) board: Board,
    pub(crate) updated_pous: Vec<String>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) orphaned_comment_ids: BTreeSet<String>,
}

/// Collects every [`NodeId`] and its immediate enclosing parent [`NodeId`] across an entire [`Board`].
pub(crate) fn collect_board_topology(board: &Board) -> NodeTopology {
    let mut topo = NodeTopology::default();
    for hw in &board.program.hardware {
        topo.insert_node(&hw.id, None);
    }
    for td in &board.program.types {
        topo.insert_node(type_decl_id(td), None);
    }
    for decl in &board.program.data {
        topo.insert_node(&decl.id, None);
    }
    for module in &board.program.modules {
        collect_module_topology(module, &mut topo);
    }
    for pou in &board.program.pous {
        collect_pou_topology(pou, None, &mut topo);
    }
    topo
}

/// Collects every [`NodeId`] and parent link inside a [`ModuleDecl`].
pub(crate) fn collect_module_topology(module: &ModuleDecl, topo: &mut NodeTopology) {
    topo.insert_node(&module.id, None);
    for td in &module.types {
        topo.insert_node(type_decl_id(td), Some(&module.id));
    }
    for decl in &module.data {
        topo.insert_node(&decl.id, Some(&module.id));
    }
    for pou in &module.pous {
        collect_pou_topology(pou, Some(&module.id), topo);
    }
}

/// Collects every [`NodeId`] and parent link inside a [`Pou`].
pub(crate) fn collect_pou_topology(
    pou: &Pou,
    parent_module: Option<&NodeId>,
    topo: &mut NodeTopology,
) {
    topo.insert_node(&pou.id, parent_module);
    for local in &pou.locals {
        topo.insert_node(&local.id, Some(&pou.id));
    }
    match &pou.body {
        PouBody::Statements(stmts) => collect_stmts_topology(stmts, &pou.id, topo),
        PouBody::Network(net) => {
            for rung in &net.rungs {
                topo.insert_node(&rung.id, Some(&pou.id));
                for cell in &rung.cells {
                    collect_ladder_cell_topology(cell, &rung.id, topo);
                }
            }
        }
        PouBody::Chart(chart) => {
            for step in &chart.steps {
                topo.insert_node(&step.id, Some(&pou.id));
                for action in &step.actions {
                    topo.insert_node(&action.id, Some(&step.id));
                    collect_stmts_topology(&action.body, &action.id, topo);
                }
            }
            for trans in &chart.transitions {
                topo.insert_node(&trans.id, Some(&pou.id));
            }
        }
    }
}

fn collect_stmts_topology(stmts: &[Stmt], parent: &NodeId, topo: &mut NodeTopology) {
    for stmt in stmts {
        topo.insert_node(&stmt.id, Some(parent));
        match &stmt.kind {
            StmtKind::If {
                condition: _,
                then_body,
                elif_arms,
                else_body,
            } => {
                collect_stmts_topology(then_body, &stmt.id, topo);
                for arm in elif_arms {
                    collect_stmts_topology(&arm.body, &stmt.id, topo);
                }
                collect_stmts_topology(else_body, &stmt.id, topo);
            }
            StmtKind::Switch {
                scrutinee: _,
                cases,
                default,
            } => {
                for case in cases {
                    collect_stmts_topology(&case.body, &stmt.id, topo);
                }
                if let Some(def_body) = default {
                    collect_stmts_topology(def_body, &stmt.id, topo);
                }
            }
            StmtKind::While { condition: _, body }
            | StmtKind::For {
                counter: _,
                from: _,
                to: _,
                step: _,
                inclusive: _,
                body,
            } => {
                collect_stmts_topology(body, &stmt.id, topo);
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

fn collect_ladder_cell_topology(cell: &LadderCell, parent: &NodeId, topo: &mut NodeTopology) {
    topo.insert_node(&cell.id, Some(parent));
    match &cell.kind {
        LadderCellKind::ParallelOr { branches } => {
            for branch in branches {
                for sub_cell in branch {
                    collect_ladder_cell_topology(sub_cell, &cell.id, topo);
                }
            }
        }
        LadderCellKind::Empty
        | LadderCellKind::Wire
        | LadderCellKind::Contact { .. }
        | LadderCellKind::Coil { .. }
        | LadderCellKind::Box(_) => {}
    }
}

/// Walks `parent_of` upwards from `deleted` to find the nearest ancestor [`NodeId`] present in `surviving`.
pub(crate) fn find_surviving_parent(
    deleted: &NodeId,
    parent_of: &BTreeMap<NodeId, NodeId>,
    surviving: &BTreeSet<NodeId>,
) -> Option<NodeId> {
    let mut visited = BTreeSet::new();
    let mut current = deleted;
    while visited.insert(current.clone()) {
        let parent = parent_of.get(current)?;
        if surviving.contains(parent) {
            return Some(parent.clone());
        }
        current = parent;
    }
    None
}

/// Non-destructively orphans comments anchored on any node in `deleted_nodes`:
/// - Nodes with a surviving ancestor [`NodeId`] transition to `CommentPlacement::Dangling` on that
///   parent via [`orphan_comments_for_deleted_nodes`] (`INFO_COMMENT_ORPHANED`).
/// - Deleted top-level board nodes with no enclosing [`NodeId`] ancestor transition to
///   `CommentAnchor::Board { id: board_id }` with `CommentPlacement::Body` (satisfies `ERR_COMMENT_FIELD`
///   for non-node anchors) and emit `INFO_COMMENT_ORPHANED`.
///
/// User commentary is NEVER deleted.
pub(crate) fn orphan_comments_for_deleted_set(
    comments: &mut [Comment],
    deleted_nodes: &BTreeSet<NodeId>,
    parent_of: &BTreeMap<NodeId, NodeId>,
    surviving_nodes: &BTreeSet<NodeId>,
    board_id: &str,
) -> (Vec<Diagnostic>, BTreeSet<String>) {
    let mut deleted_to_parent = BTreeMap::new();
    let mut board_level_deleted = BTreeSet::new();

    for deleted in deleted_nodes {
        if let Some(parent) = find_surviving_parent(deleted, parent_of, surviving_nodes) {
            deleted_to_parent.insert(deleted.clone(), parent);
        } else {
            board_level_deleted.insert(deleted.clone());
        }
    }

    let mut orphaned_ids = BTreeSet::new();
    for comment in comments.iter() {
        if let CommentAnchor::Node { id } = &comment.anchor
            && (deleted_to_parent.contains_key(id) || board_level_deleted.contains(id))
        {
            orphaned_ids.insert(comment.id.clone());
        }
    }

    let mut diagnostics =
        orphan_comments_for_deleted_nodes(comments, &deleted_to_parent, Some(board_id));

    for comment in comments.iter_mut() {
        if let CommentAnchor::Node { id } = &comment.anchor
            && board_level_deleted.contains(id)
        {
            let deleted_label = id.to_string();
            comment.anchor = CommentAnchor::Board {
                id: board_id.to_owned(),
            };
            comment.placement = AstCommentPlacement::Body;
            comment.token_ordinal = None;

            let mut diag = Diagnostic::new(
                ErrorCode::InfoCommentOrphaned,
                format!(
                    "comment `{}` orphaned after deleting top-level node `{deleted_label}`; preserved on board `{board_id}`",
                    comment.id
                ),
            )
            .with_board_id(board_id);
            diag.span = Some(comment.span);
            diagnostics.push(diag);
        }
    }

    (diagnostics, orphaned_ids)
}

/// Non-destructively merges a parsed [`BoardProgramBundle`] from `frontend` into `existing`.
pub(crate) fn merge_bundle_into_board(
    existing: &Board,
    parsed: &BoardProgramBundle,
    frontend: &dyn LanguageFrontend,
    ctx: &BoardContext,
) -> MergeOutcome {
    let mut candidate = existing.clone();
    let parsed_nodes = collect_program_node_ids(&parsed.program);

    let mut id_remap: BTreeMap<NodeId, NodeId> = BTreeMap::new();
    let mut claimed_targets: BTreeSet<NodeId> = BTreeSet::new();
    let mut old_edited_scope = NodeTopology::default();
    let mut old_edited_pou_of: BTreeMap<NodeId, NodeId> = BTreeMap::new();
    let mut new_edited_pou_of: BTreeMap<NodeId, NodeId> = BTreeMap::new();
    let mut updated_pous: Vec<String> = Vec::new();

    // 1. Imports (union-merge preserving insertion order)
    for imp in &parsed.program.imports {
        if !candidate.program.imports.contains(imp) {
            candidate.program.imports.push(imp.clone());
        }
    }

    // 2. Hardware I/O Tag Table (non-destructive reconciliation by id, then label)
    for inc_hw in &parsed.program.hardware {
        merge_hardware_binding(
            &mut candidate.program.hardware,
            inc_hw,
            &parsed_nodes,
            &mut claimed_targets,
            &mut id_remap,
        );
    }

    // 3. Types (preserves untouched Struct, Enum, and Decl::Foreign)
    for inc_td in &parsed.program.types {
        merge_type_decl(
            &mut candidate.program.types,
            inc_td,
            &parsed_nodes,
            &mut claimed_targets,
            &mut id_remap,
        );
    }

    // 4. Global Data Table (preserves untouched DataDecls)
    for inc_data in &parsed.program.data {
        merge_data_decl(
            &mut candidate.program.data,
            inc_data,
            &parsed_nodes,
            &mut claimed_targets,
            &mut id_remap,
        );
    }

    // 5. Top-level POUs (and single-POU edits targeting a module-owned POU)
    for inc_pou in &parsed.program.pous {
        if let Some(slot) = find_pou_mut(&mut candidate.program.pous, inc_pou) {
            record_pou_membership(slot, None, &mut old_edited_scope, &mut old_edited_pou_of);
            let merged = reconcile_pou(
                slot,
                inc_pou,
                &parsed_nodes,
                &mut claimed_targets,
                &mut id_remap,
            );
            record_new_pou_membership(&merged, &mut new_edited_pou_of);
            updated_pous.push(merged.name.clone());
            *slot = merged;
        } else if let Some((module_id, slot)) =
            find_module_pou_mut(&mut candidate.program.modules, inc_pou)
        {
            record_pou_membership(
                slot,
                Some(&module_id),
                &mut old_edited_scope,
                &mut old_edited_pou_of,
            );
            let merged = reconcile_pou(
                slot,
                inc_pou,
                &parsed_nodes,
                &mut claimed_targets,
                &mut id_remap,
            );
            record_new_pou_membership(&merged, &mut new_edited_pou_of);
            updated_pous.push(merged.name.clone());
            *slot = merged;
        } else {
            record_new_pou_membership(inc_pou, &mut new_edited_pou_of);
            updated_pous.push(inc_pou.name.clone());
            candidate.program.pous.push(inc_pou.clone());
        }
    }

    // 6. Embedded Modules (preserves untouched modules and untouched sibling POUs)
    for inc_mod in &parsed.program.modules {
        if let Some(slot) = find_module_mut(&mut candidate.program.modules, inc_mod) {
            let stable_mod_id = slot.id.clone();
            if inc_mod.id != stable_mod_id
                && !parsed_nodes.contains(&stable_mod_id)
                && claimed_targets.insert(stable_mod_id.clone())
            {
                id_remap.insert(inc_mod.id.clone(), stable_mod_id.clone());
            }
            for imp in &inc_mod.imports {
                if !slot.imports.contains(imp) {
                    slot.imports.push(imp.clone());
                }
            }
            for exp in &inc_mod.exports {
                if !slot.exports.contains(exp) {
                    slot.exports.push(exp.clone());
                }
            }
            for inc_td in &inc_mod.types {
                merge_type_decl(
                    &mut slot.types,
                    inc_td,
                    &parsed_nodes,
                    &mut claimed_targets,
                    &mut id_remap,
                );
            }
            for inc_data in &inc_mod.data {
                merge_data_decl(
                    &mut slot.data,
                    inc_data,
                    &parsed_nodes,
                    &mut claimed_targets,
                    &mut id_remap,
                );
            }
            for inc_pou in &inc_mod.pous {
                if let Some(pou_slot) = find_pou_mut(&mut slot.pous, inc_pou) {
                    record_pou_membership(
                        pou_slot,
                        Some(&stable_mod_id),
                        &mut old_edited_scope,
                        &mut old_edited_pou_of,
                    );
                    let merged = reconcile_pou(
                        pou_slot,
                        inc_pou,
                        &parsed_nodes,
                        &mut claimed_targets,
                        &mut id_remap,
                    );
                    record_new_pou_membership(&merged, &mut new_edited_pou_of);
                    updated_pous.push(merged.name.clone());
                    *pou_slot = merged;
                } else {
                    record_new_pou_membership(inc_pou, &mut new_edited_pou_of);
                    updated_pous.push(inc_pou.name.clone());
                    slot.pous.push(inc_pou.clone());
                }
            }
        } else {
            for pou in &inc_mod.pous {
                record_new_pou_membership(pou, &mut new_edited_pou_of);
                updated_pous.push(pou.name.clone());
            }
            candidate.program.modules.push(inc_mod.clone());
        }
    }

    // 7. Remap incoming comments, assumptions, and presentation hints to stable NodeIds
    let incoming_comments: Vec<Comment> = parsed
        .comments
        .iter()
        .map(|c| remap_comment(c, &id_remap))
        .collect();
    let incoming_assumptions: Vec<Assumption> = parsed
        .assumptions
        .iter()
        .map(|a| remap_assumption(a, &id_remap))
        .collect();
    let incoming_presentation = remap_presentation(&parsed.presentation, &id_remap);

    // 8. Compute post-merge topology and deleted-node -> surviving-parent map
    let new_topology = collect_board_topology(&candidate);
    let mut deleted_to_parent: BTreeMap<NodeId, NodeId> = BTreeMap::new();
    for old_node in &old_edited_scope.all_nodes {
        if !new_topology.all_nodes.contains(old_node)
            && let Some(parent) = find_surviving_parent(
                old_node,
                &old_edited_scope.parent_of,
                &new_topology.all_nodes,
            )
        {
            deleted_to_parent.insert(old_node.clone(), parent);
        }
    }

    // 9. Merge comments with C4 re-import idempotence, fresh-NodeId re-binding, and C2 orphan preservation
    let (merged_comments, diagnostics, orphaned_comment_ids) = merge_comments(
        &existing.comments,
        &incoming_comments,
        &old_edited_scope.all_nodes,
        &old_edited_pou_of,
        &new_edited_pou_of,
        &deleted_to_parent,
        &new_topology.all_nodes,
        frontend,
        ctx,
        &candidate.id,
    );
    candidate.comments = merged_comments;

    // 10. Merge assumptions (preserve all assumptions on untouched/surviving nodes)
    merge_assumptions(
        &mut candidate.assumptions,
        &incoming_assumptions,
        &old_edited_scope.all_nodes,
        &new_topology.all_nodes,
    );

    // 11. Merge presentation hints (preserve existing eol_style and untouched hints)
    merge_presentation(&mut candidate.presentation, &incoming_presentation);

    MergeOutcome {
        board: candidate,
        updated_pous,
        diagnostics,
        orphaned_comment_ids,
    }
}

fn collect_program_node_ids(program: &dendri_ast::BoardProgram) -> BTreeSet<NodeId> {
    let mut topo = NodeTopology::default();
    for hw in &program.hardware {
        topo.insert_node(&hw.id, None);
    }
    for td in &program.types {
        topo.insert_node(type_decl_id(td), None);
    }
    for decl in &program.data {
        topo.insert_node(&decl.id, None);
    }
    for module in &program.modules {
        collect_module_topology(module, &mut topo);
    }
    for pou in &program.pous {
        collect_pou_topology(pou, None, &mut topo);
    }
    topo.all_nodes
}

pub(crate) const fn type_decl_id(td: &TypeDecl) -> &NodeId {
    match td {
        TypeDecl::Struct { id, .. } | TypeDecl::Enum { id, .. } | TypeDecl::Foreign { id, .. } => {
            id
        }
    }
}

fn type_decl_name(td: &TypeDecl) -> Option<&str> {
    match td {
        TypeDecl::Struct { name, .. } | TypeDecl::Enum { name, .. } => Some(name.as_str()),
        TypeDecl::Foreign { name, .. } => name.as_deref(),
    }
}

fn set_type_decl_id(td: &mut TypeDecl, new_id: NodeId) {
    match td {
        TypeDecl::Struct { id, .. } | TypeDecl::Enum { id, .. } | TypeDecl::Foreign { id, .. } => {
            *id = new_id;
        }
    }
}

fn record_pou_membership(
    pou: &Pou,
    parent_module: Option<&NodeId>,
    scope: &mut NodeTopology,
    pou_of: &mut BTreeMap<NodeId, NodeId>,
) {
    let mut local = NodeTopology::default();
    collect_pou_topology(pou, parent_module, &mut local);
    for node_id in &local.all_nodes {
        scope.all_nodes.insert(node_id.clone());
        pou_of.insert(node_id.clone(), pou.id.clone());
    }
    for (child, parent) in local.parent_of {
        scope.parent_of.insert(child, parent);
    }
}

fn record_new_pou_membership(pou: &Pou, pou_of: &mut BTreeMap<NodeId, NodeId>) {
    let mut local = NodeTopology::default();
    collect_pou_topology(pou, None, &mut local);
    for node_id in local.all_nodes {
        pou_of.insert(node_id, pou.id.clone());
    }
}

fn try_remap_stable_id(
    incoming_id: &NodeId,
    existing_id: &NodeId,
    parsed_nodes: &BTreeSet<NodeId>,
    claimed_targets: &mut BTreeSet<NodeId>,
    id_remap: &mut BTreeMap<NodeId, NodeId>,
) -> Option<NodeId> {
    if incoming_id != existing_id
        && !parsed_nodes.contains(existing_id)
        && claimed_targets.insert(existing_id.clone())
    {
        id_remap.insert(incoming_id.clone(), existing_id.clone());
        Some(existing_id.clone())
    } else {
        None
    }
}

fn merge_hardware_binding(
    target: &mut Vec<HardwareBinding>,
    incoming: &HardwareBinding,
    parsed_nodes: &BTreeSet<NodeId>,
    claimed_targets: &mut BTreeSet<NodeId>,
    id_remap: &mut BTreeMap<NodeId, NodeId>,
) {
    let matched_idx = target
        .iter()
        .position(|h| h.id == incoming.id)
        .or_else(|| target.iter().position(|h| h.label == incoming.label));

    if let Some(idx) = matched_idx
        && let Some(existing) = target.get_mut(idx)
    {
        let mut merged = incoming.clone();
        if let Some(stable) = try_remap_stable_id(
            &merged.id,
            &existing.id,
            parsed_nodes,
            claimed_targets,
            id_remap,
        ) {
            merged.id = stable;
        }
        if merged.kind == existing.kind {
            if merged.codec.is_none() {
                merged.codec = existing.codec.clone();
            }
            if merged.range.is_none() {
                merged.range = existing.range.clone();
            }
        }
        *existing = merged;
    } else {
        target.push(incoming.clone());
    }
}

fn merge_type_decl(
    target: &mut Vec<TypeDecl>,
    incoming: &TypeDecl,
    parsed_nodes: &BTreeSet<NodeId>,
    claimed_targets: &mut BTreeSet<NodeId>,
    id_remap: &mut BTreeMap<NodeId, NodeId>,
) {
    let inc_id = type_decl_id(incoming);
    let inc_name = type_decl_name(incoming);
    let matched_idx = target
        .iter()
        .position(|t| type_decl_id(t) == inc_id)
        .or_else(|| {
            inc_name.and_then(|name| target.iter().position(|t| type_decl_name(t) == Some(name)))
        });

    if let Some(idx) = matched_idx
        && let Some(existing) = target.get_mut(idx)
    {
        let existing_id = type_decl_id(existing).clone();
        let mut merged = incoming.clone();
        if let Some(stable) = try_remap_stable_id(
            type_decl_id(&merged),
            &existing_id,
            parsed_nodes,
            claimed_targets,
            id_remap,
        ) {
            set_type_decl_id(&mut merged, stable);
        }
        *existing = merged;
    } else {
        target.push(incoming.clone());
    }
}

fn merge_data_decl(
    target: &mut Vec<DataDecl>,
    incoming: &DataDecl,
    parsed_nodes: &BTreeSet<NodeId>,
    claimed_targets: &mut BTreeSet<NodeId>,
    id_remap: &mut BTreeMap<NodeId, NodeId>,
) {
    let matched_idx = target
        .iter()
        .position(|d| d.id == incoming.id)
        .or_else(|| target.iter().position(|d| d.name == incoming.name));

    if let Some(idx) = matched_idx
        && let Some(existing) = target.get_mut(idx)
    {
        let mut merged = incoming.clone();
        if let Some(stable) = try_remap_stable_id(
            &merged.id,
            &existing.id,
            parsed_nodes,
            claimed_targets,
            id_remap,
        ) {
            merged.id = stable;
        }
        *existing = merged;
    } else {
        target.push(incoming.clone());
    }
}

fn find_pou_mut<'a>(pous: &'a mut [Pou], incoming: &Pou) -> Option<&'a mut Pou> {
    if let Some(idx) = pous.iter().position(|p| p.id == incoming.id) {
        return pous.get_mut(idx);
    }
    if let Some(idx) = pous.iter().position(|p| p.name == incoming.name) {
        return pous.get_mut(idx);
    }
    None
}

fn find_module_pou_mut<'a>(
    modules: &'a mut [ModuleDecl],
    incoming: &Pou,
) -> Option<(NodeId, &'a mut Pou)> {
    let found = modules
        .iter()
        .enumerate()
        .find_map(|(mi, module)| {
            module
                .pous
                .iter()
                .position(|p| p.id == incoming.id)
                .map(|pi| (mi, pi))
        })
        .or_else(|| {
            modules.iter().enumerate().find_map(|(mi, module)| {
                module
                    .pous
                    .iter()
                    .position(|p| p.name == incoming.name)
                    .map(|pi| (mi, pi))
            })
        });

    let (mod_idx, pou_idx) = found?;
    let module = modules.get_mut(mod_idx)?;
    let mod_id = module.id.clone();
    let pou = module.pous.get_mut(pou_idx)?;
    Some((mod_id, pou))
}

fn find_module_mut<'a>(
    modules: &'a mut [ModuleDecl],
    incoming: &ModuleDecl,
) -> Option<&'a mut ModuleDecl> {
    if let Some(idx) = modules.iter().position(|m| m.id == incoming.id) {
        return modules.get_mut(idx);
    }
    if let Some(idx) = modules.iter().position(|m| m.name == incoming.name) {
        return modules.get_mut(idx);
    }
    None
}

fn reconcile_pou(
    existing: &Pou,
    incoming: &Pou,
    parsed_nodes: &BTreeSet<NodeId>,
    claimed_targets: &mut BTreeSet<NodeId>,
    id_remap: &mut BTreeMap<NodeId, NodeId>,
) -> Pou {
    let mut merged = incoming.clone();
    if let Some(stable) = try_remap_stable_id(
        &merged.id,
        &existing.id,
        parsed_nodes,
        claimed_targets,
        id_remap,
    ) {
        merged.id = stable;
    }
    if merged.preferred_view.is_none() {
        merged.preferred_view = existing.preferred_view;
    }

    // Preserve stable NodeIds for semantically named POU locals (`by name`).
    for local in &mut merged.locals {
        if let Some(existing_local) = existing.locals.iter().find(|l| l.name == local.name)
            && let Some(stable) = try_remap_stable_id(
                &local.id,
                &existing_local.id,
                parsed_nodes,
                claimed_targets,
                id_remap,
            )
        {
            local.id = stable;
        }
    }

    // Preserve stable NodeIds for semantically named GRAFCET/SFC steps and transitions (`by name`)
    // so `[boards.presentation].node_coords`, comments, and assumptions remain attached across re-parses.
    if let (PouBody::Chart(existing_chart), PouBody::Chart(merged_chart)) =
        (&existing.body, &mut merged.body)
    {
        for step in &mut merged_chart.steps {
            if let Some(existing_step) = existing_chart.steps.iter().find(|s| s.name == step.name)
                && let Some(stable) = try_remap_stable_id(
                    &step.id,
                    &existing_step.id,
                    parsed_nodes,
                    claimed_targets,
                    id_remap,
                )
            {
                step.id = stable;
            }
        }
        for trans in &mut merged_chart.transitions {
            if let Some(existing_trans) = existing_chart
                .transitions
                .iter()
                .find(|t| t.name == trans.name)
                && let Some(stable) = try_remap_stable_id(
                    &trans.id,
                    &existing_trans.id,
                    parsed_nodes,
                    claimed_targets,
                    id_remap,
                )
            {
                trans.id = stable;
            }
        }
    }

    merged
}

fn remap_node_id(id: &NodeId, remap: &BTreeMap<NodeId, NodeId>) -> NodeId {
    remap.get(id).cloned().unwrap_or_else(|| id.clone())
}

fn remap_comment(comment: &Comment, remap: &BTreeMap<NodeId, NodeId>) -> Comment {
    let mut out = comment.clone();
    out.text = normalize_eol_to_lf(&out.text);
    if let CommentAnchor::Node { id } = &out.anchor {
        out.anchor = CommentAnchor::Node {
            id: remap_node_id(id, remap),
        };
    }
    out
}

fn remap_assumption(assumption: &Assumption, remap: &BTreeMap<NodeId, NodeId>) -> Assumption {
    let mut out = assumption.clone();
    out.node = remap_node_id(&out.node, remap);
    out
}

fn remap_presentation(
    pres: &BoardPresentation,
    remap: &BTreeMap<NodeId, NodeId>,
) -> BoardPresentation {
    BoardPresentation {
        eol_style: pres.eol_style,
        blank_lines_before: pres
            .blank_lines_before
            .iter()
            .map(|(k, v)| (remap_node_id(k, remap), (*v).min(2)))
            .collect(),
        origin_files: pres
            .origin_files
            .iter()
            .map(|(k, v)| (remap_node_id(k, remap), v.clone()))
            .collect(),
        node_coords: pres
            .node_coords
            .iter()
            .map(|(k, v)| (remap_node_id(k, remap), *v))
            .collect(),
    }
}

/// Checks `C4` re-import idempotence using `frontend.matches_comment`, falling back to
/// `matches_comment_with_ctx` when the active [`BoardContext`] overrides the frontend's static
/// `comment_encoding` or `st_dialect` defaults.
pub(crate) fn comment_matches_stored(
    frontend: &dyn LanguageFrontend,
    ctx: &BoardContext,
    parsed_text: &str,
    stored_text: &str,
) -> bool {
    if frontend.matches_comment(parsed_text, stored_text) {
        return true;
    }
    let caps = frontend.capabilities();
    let ctx_differs = caps.comment_encoding != ctx.comment_encoding
        || caps.st_dialect.is_some_and(|d| d != ctx.st_dialect);
    ctx_differs && matches_comment_with_ctx(frontend.id(), ctx, parsed_text, stored_text)
}

#[derive(Debug, Clone)]
enum ExistingCommentDecision {
    Pending,
    Keep(Comment),
    Drop,
}

#[allow(clippy::too_many_arguments)]
fn merge_comments(
    existing: &[Comment],
    incoming: &[Comment],
    old_edited_nodes: &BTreeSet<NodeId>,
    old_edited_pou_of: &BTreeMap<NodeId, NodeId>,
    new_edited_pou_of: &BTreeMap<NodeId, NodeId>,
    deleted_to_parent: &BTreeMap<NodeId, NodeId>,
    surviving_nodes: &BTreeSet<NodeId>,
    frontend: &dyn LanguageFrontend,
    ctx: &BoardContext,
    board_id: &str,
) -> (Vec<Comment>, Vec<Diagnostic>, BTreeSet<String>) {
    let mut decisions: Vec<ExistingCommentDecision> =
        vec![ExistingCommentDecision::Pending; existing.len()];
    let mut consumed_incoming: BTreeSet<usize> = BTreeSet::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut orphaned_ids: BTreeSet<String> = BTreeSet::new();

    // Pass 1a: Exact ID + exact surviving anchor match across all existing comments (C4 idempotence).
    // Requiring matching anchor avoids false collisions when a parser reuses sequential IDs like
    // `"c_1"` after deleting an earlier node.
    for (idx, ec) in existing.iter().enumerate() {
        if !anchor_is_valid(&ec.anchor, surviving_nodes) {
            continue;
        }
        if let Some((in_idx, pc)) = incoming.iter().enumerate().find(|(i, pc)| {
            !consumed_incoming.contains(i) && pc.id == ec.id && pc.anchor == ec.anchor
        }) {
            consumed_incoming.insert(in_idx);
            let mut updated = ec.clone();
            if comment_matches_stored(frontend, ctx, &pc.text, &ec.text) {
                // C4: degraded projection matches verbatim original -> preserve original text & origin!
                updated.span = pc.span;
            } else {
                updated.text = pc.text.clone();
                updated.origin = pc.origin.clone();
                updated.placement = pc.placement;
                updated.token_ordinal = pc.token_ordinal;
                updated.span = pc.span;
            }
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(updated);
            }
        }
    }

    // Pass 1b: Explicit ID move across surviving anchors ONLY when text still matches via C4
    // (never steals an unrelated comment whose parser recycled a sequential ID).
    for (idx, ec) in existing.iter().enumerate() {
        if !matches!(decisions.get(idx), Some(ExistingCommentDecision::Pending)) {
            continue;
        }
        if let Some((in_idx, pc)) = incoming.iter().enumerate().find(|(i, pc)| {
            !consumed_incoming.contains(i)
                && pc.id == ec.id
                && anchor_is_valid(&pc.anchor, surviving_nodes)
                && comment_matches_stored(frontend, ctx, &pc.text, &ec.text)
        }) {
            consumed_incoming.insert(in_idx);
            let mut updated = ec.clone();
            updated.anchor = pc.anchor.clone();
            if ec.placement != AstCommentPlacement::Inner
                || pc.placement == AstCommentPlacement::Inner
            {
                updated.placement = pc.placement;
                updated.token_ordinal = pc.token_ordinal;
            }
            updated.span = pc.span;
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(updated);
            }
        }
    }

    // Pass 2: Untouched-scope preservation (non-node anchors & nodes outside edited POUs).
    // Never drop; consume matching incoming projections on identical anchors to prevent duplication.
    for (idx, ec) in existing.iter().enumerate() {
        if !matches!(decisions.get(idx), Some(ExistingCommentDecision::Pending)) {
            continue;
        }
        let is_untouched = match &ec.anchor {
            CommentAnchor::Node { id } => !old_edited_nodes.contains(id),
            CommentAnchor::Project
            | CommentAnchor::Board { .. }
            | CommentAnchor::Component { .. }
            | CommentAnchor::Wire { .. } => true,
        };
        if is_untouched {
            if let Some(in_idx) = incoming.iter().enumerate().find_map(|(i, pc)| {
                (!consumed_incoming.contains(&i)
                    && pc.anchor == ec.anchor
                    && comment_matches_stored(frontend, ctx, &pc.text, &ec.text))
                .then_some(i)
            }) {
                consumed_incoming.insert(in_idx);
            }
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(ec.clone());
            }
        }
    }

    // Pass 3a: C4 idempotent match on SURVIVING edited-scope nodes with identical (anchor, placement, ordinal).
    // Runs BEFORE deleted-node re-binding/orphaning so surviving anchors claim identical texts first.
    for (idx, ec) in existing.iter().enumerate() {
        if !matches!(decisions.get(idx), Some(ExistingCommentDecision::Pending)) {
            continue;
        }
        if !anchor_is_valid(&ec.anchor, surviving_nodes) {
            continue;
        }
        if let Some((in_idx, pc)) = incoming.iter().enumerate().find(|(i, pc)| {
            !consumed_incoming.contains(i)
                && pc.anchor == ec.anchor
                && pc.placement == ec.placement
                && pc.token_ordinal == ec.token_ordinal
                && comment_matches_stored(frontend, ctx, &pc.text, &ec.text)
        }) {
            consumed_incoming.insert(in_idx);
            let mut kept = ec.clone();
            kept.span = pc.span;
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(kept);
            }
        }
    }

    // Pass 3b: C4 idempotent match on SURVIVING edited-scope nodes with same anchor (e.g. relocated Inner).
    for (idx, ec) in existing.iter().enumerate() {
        if !matches!(decisions.get(idx), Some(ExistingCommentDecision::Pending)) {
            continue;
        }
        if !anchor_is_valid(&ec.anchor, surviving_nodes) {
            continue;
        }
        if let Some((in_idx, pc)) = incoming.iter().enumerate().find(|(i, pc)| {
            !consumed_incoming.contains(i)
                && pc.anchor == ec.anchor
                && comment_matches_stored(frontend, ctx, &pc.text, &ec.text)
        }) {
            consumed_incoming.insert(in_idx);
            let mut kept = ec.clone();
            kept.span = pc.span;
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(kept);
            }
        }
    }

    // Pass 4a: Fresh child-NodeId re-binding (same edited POU, matching placement + token_ordinal, C4 match).
    // Real textual frontends regenerate fresh statement/rung/step NodeIds on re-parse; re-bind those
    // comments cleanly instead of falsely orphaning and duplicating them!
    for (idx, ec) in existing.iter().enumerate() {
        if !matches!(decisions.get(idx), Some(ExistingCommentDecision::Pending)) {
            continue;
        }
        let CommentAnchor::Node { id: old_id } = &ec.anchor else {
            continue;
        };
        if !deleted_to_parent.contains_key(old_id) {
            continue;
        }
        let Some(old_pou_id) = old_edited_pou_of.get(old_id) else {
            continue;
        };
        if let Some((in_idx, pc)) = incoming.iter().enumerate().find(|(i, pc)| {
            !consumed_incoming.contains(i)
                && pc.placement == ec.placement
                && pc.token_ordinal == ec.token_ordinal
                && matches!(
                    &pc.anchor,
                    CommentAnchor::Node { id: new_id }
                        if new_edited_pou_of.get(new_id) == Some(old_pou_id)
                            && surviving_nodes.contains(new_id)
                )
                && comment_matches_stored(frontend, ctx, &pc.text, &ec.text)
        }) {
            consumed_incoming.insert(in_idx);
            let mut rebound = ec.clone();
            rebound.anchor = pc.anchor.clone();
            rebound.span = pc.span;
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(rebound);
            }
        }
    }

    // Pass 4b: Fresh child-NodeId re-binding (same edited POU, any placement — e.g. cross-language
    // relocated Inner comment on a regenerated statement NodeId — matching via C4).
    for (idx, ec) in existing.iter().enumerate() {
        if !matches!(decisions.get(idx), Some(ExistingCommentDecision::Pending)) {
            continue;
        }
        let CommentAnchor::Node { id: old_id } = &ec.anchor else {
            continue;
        };
        if !deleted_to_parent.contains_key(old_id) {
            continue;
        }
        let Some(old_pou_id) = old_edited_pou_of.get(old_id) else {
            continue;
        };
        if let Some((in_idx, pc)) = incoming.iter().enumerate().find(|(i, pc)| {
            !consumed_incoming.contains(i)
                && matches!(
                    &pc.anchor,
                    CommentAnchor::Node { id: new_id }
                        if new_edited_pou_of.get(new_id) == Some(old_pou_id)
                            && surviving_nodes.contains(new_id)
                )
                && comment_matches_stored(frontend, ctx, &pc.text, &ec.text)
        }) {
            consumed_incoming.insert(in_idx);
            let mut rebound = ec.clone();
            rebound.anchor = pc.anchor.clone();
            if ec.placement != AstCommentPlacement::Inner
                || pc.placement == AstCommentPlacement::Inner
            {
                rebound.placement = pc.placement;
                rebound.token_ordinal = pc.token_ordinal;
            }
            rebound.span = pc.span;
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(rebound);
            }
        }
    }

    // Pass 5: True C2 orphan preservation for genuinely deleted nodes whose comments were not
    // re-bound onto any surviving or freshly synthesized node inside the edited POU.
    for (idx, ec) in existing.iter().enumerate() {
        if !matches!(decisions.get(idx), Some(ExistingCommentDecision::Pending)) {
            continue;
        }
        if let CommentAnchor::Node { id } = &ec.anchor
            && deleted_to_parent.contains_key(id)
        {
            let mut batch = [ec.clone()];
            let orphan_diags =
                orphan_comments_for_deleted_nodes(&mut batch, deleted_to_parent, Some(board_id));
            diagnostics.extend(orphan_diags);
            orphaned_ids.insert(ec.id.clone());

            if let Some(orphaned) = batch.into_iter().next() {
                if let Some(in_idx) = incoming.iter().enumerate().find_map(|(i, pc)| {
                    (!consumed_incoming.contains(&i)
                        && pc.anchor == orphaned.anchor
                        && pc.placement == AstCommentPlacement::Dangling
                        && comment_matches_stored(frontend, ctx, &pc.text, &orphaned.text))
                    .then_some(i)
                }) {
                    consumed_incoming.insert(in_idx);
                }
                if let Some(slot) = decisions.get_mut(idx) {
                    *slot = ExistingCommentDecision::Keep(orphaned);
                }
            }
        }
    }

    // Pass 6: Positional text edit on surviving edited-scope nodes when parser regenerated comment IDs,
    // plus unconditional preservation of existing `Dangling` comments.
    for (idx, ec) in existing.iter().enumerate() {
        if !matches!(decisions.get(idx), Some(ExistingCommentDecision::Pending)) {
            continue;
        }
        if let Some((in_idx, pc)) = incoming.iter().enumerate().find(|(i, pc)| {
            !consumed_incoming.contains(i)
                && pc.anchor == ec.anchor
                && pc.placement == ec.placement
                && pc.token_ordinal == ec.token_ordinal
        }) {
            consumed_incoming.insert(in_idx);
            let mut updated = ec.clone();
            updated.text = pc.text.clone();
            updated.origin = pc.origin.clone();
            updated.span = pc.span;
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(updated);
            }
        } else if ec.placement == AstCommentPlacement::Dangling {
            // Dangling comments live in the Comments Panel and are NEVER silently discarded on edit.
            if let Some(slot) = decisions.get_mut(idx) {
                *slot = ExistingCommentDecision::Keep(ec.clone());
            }
        } else if let Some(slot) = decisions.get_mut(idx) {
            *slot = ExistingCommentDecision::Drop;
        }
    }

    // Assemble kept comments in deterministic original order.
    let mut merged: Vec<Comment> =
        Vec::with_capacity(existing.len().saturating_add(incoming.len()));
    let mut used_ids: BTreeSet<String> = BTreeSet::new();
    for decision in decisions {
        match decision {
            ExistingCommentDecision::Keep(comment) => {
                used_ids.insert(comment.id.clone());
                merged.push(comment);
            }
            ExistingCommentDecision::Pending | ExistingCommentDecision::Drop => {}
        }
    }

    // Pass 7: Append genuinely new incoming comments, guaranteeing unique comment IDs.
    let mut next_sync_seq: u64 = 1;
    for (in_idx, pc) in incoming.iter().enumerate() {
        if consumed_incoming.contains(&in_idx) {
            continue;
        }
        let mut added = pc.clone();
        if added.id.is_empty() || used_ids.contains(&added.id) {
            loop {
                let candidate_id = format!("c_sync_{next_sync_seq:04}");
                next_sync_seq = next_sync_seq.saturating_add(1);
                if !used_ids.contains(&candidate_id) {
                    added.id = candidate_id;
                    break;
                }
            }
        }
        used_ids.insert(added.id.clone());
        merged.push(added);
    }

    (merged, diagnostics, orphaned_ids)
}

fn anchor_is_valid(anchor: &CommentAnchor, surviving_nodes: &BTreeSet<NodeId>) -> bool {
    match anchor {
        CommentAnchor::Node { id } => surviving_nodes.contains(id),
        CommentAnchor::Project
        | CommentAnchor::Board { .. }
        | CommentAnchor::Component { .. }
        | CommentAnchor::Wire { .. } => true,
    }
}

fn merge_assumptions(
    target: &mut Vec<Assumption>,
    incoming: &[Assumption],
    old_edited_nodes: &BTreeSet<NodeId>,
    surviving_nodes: &BTreeSet<NodeId>,
) {
    // Preserve all assumptions on untouched/surviving nodes; drop only assumptions whose target node
    // belonged to the edited POU and was explicitly deleted by this edit.
    target.retain(|a| !old_edited_nodes.contains(&a.node) || surviving_nodes.contains(&a.node));

    for inc in incoming {
        if let Some(slot) = target
            .iter_mut()
            .find(|a| a.node == inc.node && a.expr_ordinal == inc.expr_ordinal)
        {
            *slot = inc.clone();
        } else {
            target.push(inc.clone());
        }
    }
}

fn merge_presentation(target: &mut BoardPresentation, incoming: &BoardPresentation) {
    // Preserve `target.eol_style` across single-POU merges (changed explicitly via SetEolStyle / ReplacePresentation).
    for (node, lines) in &incoming.blank_lines_before {
        target
            .blank_lines_before
            .insert(node.clone(), (*lines).min(2));
    }
    for (node, path) in &incoming.origin_files {
        target.origin_files.insert(node.clone(), path.clone());
    }
    for (node, coords) in &incoming.node_coords {
        target.node_coords.insert(node.clone(), *coords);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_surviving_parent_walks_chain_and_handles_cycles_safely() {
        let mut parent_of = BTreeMap::new();
        parent_of.insert(NodeId::new("n_leaf"), NodeId::new("n_mid"));
        parent_of.insert(NodeId::new("n_mid"), NodeId::new("n_pou"));

        let surviving: BTreeSet<NodeId> = [NodeId::new("n_pou")].into_iter().collect();
        assert_eq!(
            find_surviving_parent(&NodeId::new("n_leaf"), &parent_of, &surviving),
            Some(NodeId::new("n_pou"))
        );

        // Adversarial cycle in parent_of must terminate cleanly with None.
        let mut cyclic = BTreeMap::new();
        cyclic.insert(NodeId::new("a"), NodeId::new("b"));
        cyclic.insert(NodeId::new("b"), NodeId::new("a"));
        assert_eq!(
            find_surviving_parent(&NodeId::new("a"), &cyclic, &surviving),
            None
        );
    }
}
