//! Derived IEC 60848 / IEC 61131-3 ST/ASL statement projection for `SfcChart`
//! (`docs/lbc/sfc.md` §0 & §1.2–§1.3, `04-frontends-visual-and-sync.md` §5.2).
//!
//! IMPORTANT (ADR-026):
//! - ASL persists ONLY structural `PouBody::Chart(SfcChart)` with ZERO synthetic temporaries.
//! - [`project_chart_to_statements`] is strictly a **derived projection** for cross-language
//!   IEC 61131-3 ST export (e.g. Phase 6b RuSTy differential gate) and bit-for-bit verification
//!   against `dendri-vm`'s native in-memory SFC engine.
//!
//! Projected semantics match `dendri-vm` (`crates/dendri-vm/src/vm.rs::step_sfc_chart`) exactly:
//! 1. First-scan initialization of `initial == true` steps (with `T#0us` elapsed time and macro reset)
//!    OR advancing elapsed `<Step>.T` timers by `sfc_scan_dt` on subsequent scans.
//! 2. **Phase 1 — Frozen Pre-Evolution Transition Evaluation:**
//!    Snapshot active flags into frozen registers; evaluate transitions in pre-sorted
//!    `(priority ascending, original_index ascending)` order with full AND convergence barriers
//!    and deterministic OR priority claiming.
//! 3. **Phase 2a — Deactivate:** Clear all `from_steps` of fired transitions.
//! 4. **Phase 2b — Activate (STRICTLY AFTER Phase 2a):** Activate all `to_steps` of fired
//!    transitions and reset their timers to `T#0us` (IEC 60848 Rule 5 guarantee: simultaneous
//!    deactivate + activate keeps the step ACTIVE; macro-steps re-arm their clean reset flag).
//!    Optional `evolve_to_stability` loop bounded by `MAX_STABILITY_ITERS` (64).
//! 5. **Stage 3 — IEC 61131-3 Table 47 Action Qualifiers (`N`, `S`, `R`, `L`, `D`, `P`, `P1`, `P0`):**
//!    Automatic `N`/`L`/`D` coil reset on deactivation (unless driven by another active `N`/`L`/`D`
//!    or latched by `S`), `S` latching, `R` overriding both `N` and `S` on the same scan, and
//!    exact `P`/`P1`/`P0` edge detection.

use dendri_ast::{
    ActionQualifier, BinaryOp, CallArg, CasePattern, DataDecl, ElifArm, Expr, ExprKind, LValue,
    Literal, NodeId, ScalarType, SfcChart, SfcTransition, Stmt, StmtKind, StorageScope, SwitchCase,
    TypeRef, UnaryOp,
};
use dendri_span::Span;
use std::collections::{BTreeMap, BTreeSet};

/// Deterministic stability-evolution iteration cap matching `dendri-vm` `VmConfig::max_sfc_stability_steps`.
pub const MAX_STABILITY_ITERS: i128 = 64;

/// Default scan delta-time (`10ms`) initialized on `sfc_scan_dt` in [`project_chart_state_decls`].
pub const DEFAULT_SCAN_DT_US: i64 = 10_000;

/// Projects a structural [`SfcChart`] into pure ASL statements implementing exact IEC 60848
/// Rule 5 two-phase evolution, deterministic OR priority, AND divergence/convergence barriers,
/// macro-step entry/re-entry reset, and all 8 IEC 61131-3 Table 47 action qualifiers.
///
/// Uses clean single-underscore IEC 61131-3-compliant variable names (compatible with RuSTy `plc`)
/// and NEVER mutates or persists synthetic variables back into `PouBody::Chart`.
#[must_use]
pub fn project_chart_to_statements(chart: &SfcChart) -> Vec<Stmt> {
    let mut builder = ProjectionBuilder::new(chart);
    builder.build()
}

/// Generates the matching typed state [`DataDecl`] declarations required by the statements
/// returned from [`project_chart_to_statements`] (for ST export or VM differential verification).
#[must_use]
pub fn project_chart_state_decls(chart: &SfcChart) -> Vec<DataDecl> {
    let mut decls = Vec::new();
    let mut next_id = 1u32;
    let mut push_decl = |name: String, ty: TypeRef, initial: Expr| {
        let id = NodeId::new(format!("proj_var_{next_id:04}"));
        next_id = next_id.saturating_add(1);
        decls.push(DataDecl {
            id,
            name,
            scope: StorageScope::Var,
            ty,
            initial: Some(initial),
            span: Span::default(),
        });
    };

    let bool_ty = TypeRef::scalar(ScalarType::Bool);
    let time_ty = TypeRef::scalar(ScalarType::Time);
    let dint_ty = TypeRef::scalar(ScalarType::Dint);

    push_decl(
        "sfc_initialized".to_owned(),
        bool_ty.clone(),
        bool_lit(false),
    );
    push_decl(
        "sfc_scan_dt".to_owned(),
        time_ty.clone(),
        time_lit(DEFAULT_SCAN_DT_US),
    );

    let mut step_keys = BTreeMap::new();
    let mut used_step_keys = BTreeSet::new();
    for step in &chart.steps {
        let key = unique_step_key(&step.name, &mut used_step_keys);
        step_keys.insert(step.name.clone(), key);
    }

    for step in &chart.steps {
        let key = step_keys
            .get(&step.name)
            .cloned()
            .unwrap_or_else(|| sanitize_ident(&step.name));
        push_decl(step_x_var(&key), bool_ty.clone(), bool_lit(false));
        push_decl(step_prev_var(&key), bool_ty.clone(), bool_lit(false));
        push_decl(step_frozen_var(&key), bool_ty.clone(), bool_lit(false));
        push_decl(step_t_var(&key), time_ty.clone(), time_lit(0));
        push_decl(step_claim_var(&key), bool_ty.clone(), bool_lit(false));
        if step.macro_chart.is_some() {
            push_decl(macro_done_var(&key), bool_ty.clone(), bool_lit(false));
            push_decl(macro_reset_var(&key), bool_ty.clone(), bool_lit(false));
        }
    }

    for tr_idx in 0..chart.transitions.len() {
        push_decl(tr_fired_var(tr_idx), bool_ty.clone(), bool_lit(false));
    }

    if chart.evolve_to_stability {
        push_decl("sfc_any_fired".to_owned(), bool_ty.clone(), bool_lit(false));
        push_decl("sfc_stab_iter".to_owned(), dint_ty, int_lit(0));
    }

    let coils = collect_coil_registry(chart);
    for key in coils.n_coils.keys() {
        push_decl(n_prev_var(key), bool_ty.clone(), bool_lit(false));
        push_decl(n_curr_var(key), bool_ty.clone(), bool_lit(false));
    }
    for key in &coils.s_coil_keys {
        push_decl(s_latch_var(key), bool_ty.clone(), bool_lit(false));
    }
    for key in coils.r_coils.keys() {
        push_decl(r_hit_var(key), bool_ty.clone(), bool_lit(false));
    }

    decls
}

struct CoilRegistry {
    /// Coils driven by `N`, `L`, or `D`: sanitized key -> target LValue
    n_coils: BTreeMap<String, LValue>,
    /// Coils latched by `S` (or cleared by `R` if also `S`-driven)
    s_coil_keys: BTreeSet<String>,
    /// Coils reset by `R`: sanitized key -> target LValue
    r_coils: BTreeMap<String, LValue>,
    /// Map from canonical target signature to sanitized coil key
    key_by_sig: BTreeMap<String, String>,
}

fn collect_coil_registry(chart: &SfcChart) -> CoilRegistry {
    let mut key_by_sig: BTreeMap<String, String> = BTreeMap::new();
    let mut used_keys: BTreeSet<String> = BTreeSet::new();
    let mut n_coils: BTreeMap<String, LValue> = BTreeMap::new();
    let mut s_coil_keys: BTreeSet<String> = BTreeSet::new();
    let mut r_coils: BTreeMap<String, LValue> = BTreeMap::new();

    for step in &chart.steps {
        for action in &step.actions {
            let Some(target) = &action.target else {
                continue;
            };
            let sig = lvalue_signature(target);
            let key = if let Some(existing) = key_by_sig.get(&sig) {
                existing.clone()
            } else {
                let base = match target {
                    LValue::Var { name, .. } => sanitize_ident(name),
                    LValue::Index { .. } | LValue::Field { .. } => {
                        format!("coil_{}", key_by_sig.len())
                    }
                };
                let mut candidate = base.clone();
                let mut suffix = 1u32;
                while !used_keys.insert(candidate.clone()) {
                    candidate = format!("{base}_{suffix}");
                    suffix = suffix.saturating_add(1);
                }
                key_by_sig.insert(sig, candidate.clone());
                candidate
            };

            match action.qualifier {
                ActionQualifier::N | ActionQualifier::L | ActionQualifier::D => {
                    n_coils.entry(key).or_insert_with(|| target.strip_spans());
                }
                ActionQualifier::S => {
                    s_coil_keys.insert(key);
                }
                ActionQualifier::R => {
                    r_coils.entry(key).or_insert_with(|| target.strip_spans());
                }
                ActionQualifier::P | ActionQualifier::P1 | ActionQualifier::P0 => {}
            }
        }
    }

    CoilRegistry {
        n_coils,
        s_coil_keys,
        r_coils,
        key_by_sig,
    }
}

struct ProjectionBuilder<'a> {
    chart: &'a SfcChart,
    step_keys: BTreeMap<String, String>,
    macro_steps: BTreeSet<String>,
    coils: CoilRegistry,
    next_node_id: u32,
}

impl<'a> ProjectionBuilder<'a> {
    fn new(chart: &'a SfcChart) -> Self {
        let mut step_keys = BTreeMap::new();
        let mut used_step_keys = BTreeSet::new();
        let mut macro_steps = BTreeSet::new();
        for step in &chart.steps {
            let key = unique_step_key(&step.name, &mut used_step_keys);
            step_keys.insert(step.name.clone(), key);
            if step.macro_chart.is_some() {
                macro_steps.insert(step.name.clone());
            }
        }
        let coils = collect_coil_registry(chart);
        Self {
            chart,
            step_keys,
            macro_steps,
            coils,
            next_node_id: 1,
        }
    }

    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId::new(format!("proj_{:04}", self.next_node_id));
        self.next_node_id = self.next_node_id.saturating_add(1);
        id
    }

    fn stmt(&mut self, kind: StmtKind) -> Stmt {
        Stmt::new(self.alloc_id(), kind, Span::default())
    }

    fn assign_var(&mut self, name: String, value: Expr) -> Stmt {
        self.stmt(StmtKind::Assign {
            target: LValue::Var {
                name,
                span: Span::default(),
            },
            value,
        })
    }

    fn assign_lvalue(&mut self, target: LValue, value: Expr) -> Stmt {
        self.stmt(StmtKind::Assign {
            target: target.strip_spans(),
            value,
        })
    }

    fn step_key(&self, step_name: &str) -> String {
        self.step_keys
            .get(step_name)
            .cloned()
            .unwrap_or_else(|| sanitize_ident(step_name))
    }

    fn build(&mut self) -> Vec<Stmt> {
        let mut out = Vec::new();

        // 1. Stage 1: First-scan initialization OR step elapsed timer advance
        out.push(self.build_init_or_timer_advance());

        // 2. Stage 2: Exact IEC 60848 Rule 5 Two-Phase Evolution (with optional stability loop)
        if self.chart.evolve_to_stability {
            out.push(self.assign_var("sfc_any_fired".to_owned(), bool_lit(true)));
            out.push(self.assign_var("sfc_stab_iter".to_owned(), int_lit(0)));
            let mut loop_body = self.build_rule5_single_pass(true);
            let inc_stmt = self.assign_var(
                "sfc_stab_iter".to_owned(),
                bin_expr(BinaryOp::Add, ident_expr("sfc_stab_iter"), int_lit(1)),
            );
            loop_body.push(self.stmt(StmtKind::If {
                condition: ident_expr("sfc_any_fired"),
                then_body: vec![inc_stmt],
                elif_arms: Vec::new(),
                else_body: Vec::new(),
            }));
            let while_cond = bin_expr(
                BinaryOp::And,
                ident_expr("sfc_any_fired"),
                bin_expr(
                    BinaryOp::Lt,
                    ident_expr("sfc_stab_iter"),
                    int_lit(MAX_STABILITY_ITERS),
                ),
            );
            out.push(self.stmt(StmtKind::While {
                condition: while_cond,
                body: loop_body,
            }));
        } else {
            out.extend(self.build_rule5_single_pass(false));
        }

        // 3. Stage 3: Active macro-steps & IEC 61131-3 Table 47 Action Qualifiers
        out.extend(self.build_actions_stage());

        // 4. Stage 4: Update previous-scan step activity flags for P/P1/P0 edge detection
        for step in &self.chart.steps {
            let key = self.step_key(&step.name);
            out.push(self.assign_var(step_prev_var(&key), ident_expr(&step_x_var(&key))));
        }

        out
    }

    fn build_init_or_timer_advance(&mut self) -> Stmt {
        let mut init_body = Vec::new();
        init_body.push(self.assign_var("sfc_initialized".to_owned(), bool_lit(true)));
        for step in &self.chart.steps {
            let key = self.step_key(&step.name);
            init_body.push(self.assign_var(step_x_var(&key), bool_lit(step.initial)));
            init_body.push(self.assign_var(step_prev_var(&key), bool_lit(false)));
            init_body.push(self.assign_var(step_t_var(&key), time_lit(0)));
            if step.initial && step.macro_chart.is_some() {
                init_body.push(self.assign_var(macro_reset_var(&key), bool_lit(true)));
            }
        }

        let mut advance_body = Vec::new();
        for step in &self.chart.steps {
            let key = self.step_key(&step.name);
            let t_var = step_t_var(&key);
            let inc = self.assign_var(
                t_var.clone(),
                bin_expr(BinaryOp::Add, ident_expr(&t_var), ident_expr("sfc_scan_dt")),
            );
            advance_body.push(self.stmt(StmtKind::If {
                condition: ident_expr(&step_x_var(&key)),
                then_body: vec![inc],
                elif_arms: Vec::new(),
                else_body: Vec::new(),
            }));
        }

        self.stmt(StmtKind::If {
            condition: unary_expr(UnaryOp::Not, ident_expr("sfc_initialized")),
            then_body: init_body,
            elif_arms: Vec::new(),
            else_body: advance_body,
        })
    }

    fn build_rule5_single_pass(&mut self, track_any_fired: bool) -> Vec<Stmt> {
        let mut stmts = Vec::new();

        // Freeze pre-evolution step active flags & reset OR-priority claim flags
        for step in &self.chart.steps {
            let key = self.step_key(&step.name);
            stmts.push(self.assign_var(step_frozen_var(&key), ident_expr(&step_x_var(&key))));
            stmts.push(self.assign_var(step_claim_var(&key), bool_lit(false)));
        }
        if track_any_fired {
            stmts.push(self.assign_var("sfc_any_fired".to_owned(), bool_lit(false)));
        }

        // Sort transitions strictly by `(priority ascending, original_index ascending)`
        let mut sorted: Vec<(usize, &SfcTransition)> =
            self.chart.transitions.iter().enumerate().collect();
        sorted.sort_by_key(|&(orig_idx, tr)| (tr.priority, orig_idx));

        // Phase 1: Frozen pre-evolution receptivity evaluation with AND barrier & OR priority
        for (sorted_idx, (_, tr)) in sorted.iter().enumerate() {
            stmts.push(self.assign_var(tr_fired_var(sorted_idx), bool_lit(false)));
            if tr.from_steps.is_empty() {
                continue;
            }

            // Build `all_from_active AND none_claimed` guard
            let mut guard_terms: Vec<Expr> =
                Vec::with_capacity(tr.from_steps.len().saturating_mul(2));
            for from_name in &tr.from_steps {
                let key = self.step_key(from_name);
                guard_terms.push(ident_expr(&step_frozen_var(&key)));
            }
            for from_name in &tr.from_steps {
                let key = self.step_key(from_name);
                guard_terms.push(unary_expr(UnaryOp::Not, ident_expr(&step_claim_var(&key))));
            }
            let enabled_guard = conjoin_exprs(guard_terms);

            let receptivity = self.rewrite_expr(&tr.receptivity, true);

            let mut fire_body = Vec::new();
            fire_body.push(self.assign_var(tr_fired_var(sorted_idx), bool_lit(true)));
            for from_name in &tr.from_steps {
                let key = self.step_key(from_name);
                fire_body.push(self.assign_var(step_claim_var(&key), bool_lit(true)));
            }
            if track_any_fired {
                fire_body.push(self.assign_var("sfc_any_fired".to_owned(), bool_lit(true)));
            }

            // Nested IF prevents evaluating receptivity when upstream barrier/priority blocks it
            let inner_if = self.stmt(StmtKind::If {
                condition: receptivity,
                then_body: fire_body,
                elif_arms: Vec::new(),
                else_body: Vec::new(),
            });
            stmts.push(self.stmt(StmtKind::If {
                condition: enabled_guard,
                then_body: vec![inner_if],
                elif_arms: Vec::new(),
                else_body: Vec::new(),
            }));
        }

        // Phase 2a: Deactivate ALL `from_steps` of fired transitions (STRICTLY BEFORE Phase 2b)
        for (sorted_idx, (_, tr)) in sorted.iter().enumerate() {
            if tr.from_steps.is_empty() {
                continue;
            }
            let mut clears = Vec::with_capacity(tr.from_steps.len());
            for from_name in &tr.from_steps {
                let key = self.step_key(from_name);
                clears.push(self.assign_var(step_x_var(&key), bool_lit(false)));
            }
            stmts.push(self.stmt(StmtKind::If {
                condition: ident_expr(&tr_fired_var(sorted_idx)),
                then_body: clears,
                elif_arms: Vec::new(),
                else_body: Vec::new(),
            }));
        }

        // Phase 2b: Activate ALL `to_steps` of fired transitions STRICTLY AFTER all Phase 2a clears
        // (Rule 5 guarantee: simultaneous deactivate + activate leaves the step ACTIVE!)
        for (sorted_idx, (_, tr)) in sorted.iter().enumerate() {
            if tr.to_steps.is_empty() {
                continue;
            }
            let mut sets = Vec::new();
            for to_name in &tr.to_steps {
                let key = self.step_key(to_name);
                sets.push(self.assign_var(step_x_var(&key), bool_lit(true)));
                sets.push(self.assign_var(step_t_var(&key), time_lit(0)));
                if self.macro_steps.contains(to_name) {
                    sets.push(self.assign_var(macro_reset_var(&key), bool_lit(true)));
                }
            }
            stmts.push(self.stmt(StmtKind::If {
                condition: ident_expr(&tr_fired_var(sorted_idx)),
                then_body: sets,
                elif_arms: Vec::new(),
                else_body: Vec::new(),
            }));
        }

        stmts
    }

    fn build_actions_stage(&mut self) -> Vec<Stmt> {
        let mut stmts = Vec::new();

        // Clear per-scan N/L/D drive flags and R override flags
        let n_keys: Vec<String> = self.coils.n_coils.keys().cloned().collect();
        for key in &n_keys {
            stmts.push(self.assign_var(n_curr_var(key), bool_lit(false)));
        }
        let r_keys: Vec<String> = self.coils.r_coils.keys().cloned().collect();
        for key in &r_keys {
            stmts.push(self.assign_var(r_hit_var(key), bool_lit(false)));
        }

        for step in &self.chart.steps {
            let skey = self.step_key(&step.name);
            let is_active = ident_expr(&step_x_var(&skey));
            let was_active = ident_expr(&step_prev_var(&skey));
            let elapsed = ident_expr(&step_t_var(&skey));

            // Step active macro-step's enclosed sub-chart with clean entry/re-entry reset
            if let Some(sub_chart) = &step.macro_chart {
                let call_stmt = self.stmt(StmtKind::Call {
                    callee: sub_chart.clone(),
                    args: vec![CallArg {
                        name: Some("RESET".to_owned()),
                        value: ident_expr(&macro_reset_var(&skey)),
                        span: Span::default(),
                    }],
                });
                let clear_reset = self.assign_var(macro_reset_var(&skey), bool_lit(false));
                stmts.push(self.stmt(StmtKind::If {
                    condition: is_active.clone(),
                    then_body: vec![call_stmt, clear_reset],
                    elif_arms: Vec::new(),
                    else_body: Vec::new(),
                }));
            }

            for action in &step.actions {
                let fire_cond = match action.qualifier {
                    ActionQualifier::N | ActionQualifier::S | ActionQualifier::R => {
                        is_active.clone()
                    }
                    ActionQualifier::L => {
                        let limit = action
                            .duration
                            .as_ref()
                            .map_or_else(|| time_lit(0), |d| self.rewrite_expr(d, false));
                        bin_expr(
                            BinaryOp::And,
                            is_active.clone(),
                            bin_expr(BinaryOp::Lt, elapsed.clone(), limit),
                        )
                    }
                    ActionQualifier::D => {
                        let delay = action
                            .duration
                            .as_ref()
                            .map_or_else(|| time_lit(0), |d| self.rewrite_expr(d, false));
                        bin_expr(
                            BinaryOp::And,
                            is_active.clone(),
                            bin_expr(BinaryOp::Ge, elapsed.clone(), delay),
                        )
                    }
                    ActionQualifier::P | ActionQualifier::P1 => bin_expr(
                        BinaryOp::And,
                        is_active.clone(),
                        unary_expr(UnaryOp::Not, was_active.clone()),
                    ),
                    ActionQualifier::P0 => bin_expr(
                        BinaryOp::And,
                        unary_expr(UnaryOp::Not, is_active.clone()),
                        was_active.clone(),
                    ),
                };

                let mut then_body = Vec::new();
                if let Some(target) = &action.target {
                    let sig = lvalue_signature(target);
                    let coil_key = self
                        .coils
                        .key_by_sig
                        .get(&sig)
                        .cloned()
                        .unwrap_or_else(|| sanitize_ident(target.root_name()));
                    match action.qualifier {
                        ActionQualifier::N | ActionQualifier::L | ActionQualifier::D => {
                            then_body.push(self.assign_var(n_curr_var(&coil_key), bool_lit(true)));
                        }
                        ActionQualifier::S => {
                            then_body.push(self.assign_var(s_latch_var(&coil_key), bool_lit(true)));
                            then_body.push(self.assign_lvalue(target.clone(), bool_lit(true)));
                        }
                        ActionQualifier::R => {
                            then_body.push(self.assign_var(r_hit_var(&coil_key), bool_lit(true)));
                        }
                        ActionQualifier::P | ActionQualifier::P1 | ActionQualifier::P0 => {
                            then_body.push(self.assign_lvalue(target.clone(), bool_lit(true)));
                        }
                    }
                }

                for body_stmt in &action.body {
                    then_body.push(self.rewrite_stmt(body_stmt));
                }

                if !then_body.is_empty() {
                    stmts.push(self.stmt(StmtKind::If {
                        condition: fire_cond,
                        then_body,
                        elif_arms: Vec::new(),
                        else_body: Vec::new(),
                    }));
                }
            }
        }

        // Apply N / L / D automatic coil activation & auto-reset on deactivation (respecting S-latch)
        let n_entries: Vec<(String, LValue)> = self
            .coils
            .n_coils
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, target) in n_entries {
            let has_s_latch = self.coils.s_coil_keys.contains(&key);
            let reset_cond = if has_s_latch {
                bin_expr(
                    BinaryOp::And,
                    ident_expr(&n_prev_var(&key)),
                    unary_expr(UnaryOp::Not, ident_expr(&s_latch_var(&key))),
                )
            } else {
                ident_expr(&n_prev_var(&key))
            };
            let set_true = self.assign_lvalue(target.clone(), bool_lit(true));
            let reset_false = self.assign_lvalue(target, bool_lit(false));
            stmts.push(self.stmt(StmtKind::If {
                condition: ident_expr(&n_curr_var(&key)),
                then_body: vec![set_true],
                elif_arms: vec![ElifArm {
                    condition: reset_cond,
                    body: vec![reset_false],
                }],
                else_body: Vec::new(),
            }));
            stmts.push(self.assign_var(n_prev_var(&key), ident_expr(&n_curr_var(&key))));
        }

        // Apply Overriding Reset (`R`) strictly last so `R` overrides both `N`/`L`/`D` and `S`!
        let r_entries: Vec<(String, LValue)> = self
            .coils
            .r_coils
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, target) in r_entries {
            let mut r_body = Vec::new();
            if self.coils.s_coil_keys.contains(&key) {
                r_body.push(self.assign_var(s_latch_var(&key), bool_lit(false)));
            }
            r_body.push(self.assign_lvalue(target, bool_lit(false)));
            stmts.push(self.stmt(StmtKind::If {
                condition: ident_expr(&r_hit_var(&key)),
                then_body: r_body,
                elif_arms: Vec::new(),
                else_body: Vec::new(),
            }));
        }

        stmts
    }

    fn rewrite_stmt(&mut self, stmt: &Stmt) -> Stmt {
        let id = self.alloc_id();
        let kind = match &stmt.kind {
            StmtKind::Assign { target, value } => StmtKind::Assign {
                target: self.rewrite_lvalue(target),
                value: self.rewrite_expr(value, false),
            },
            StmtKind::Call { callee, args } => StmtKind::Call {
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|a| CallArg {
                        name: a.name.clone(),
                        value: self.rewrite_expr(&a.value, false),
                        span: Span::default(),
                    })
                    .collect(),
            },
            StmtKind::FbInvoke { instance, inputs } => StmtKind::FbInvoke {
                instance: self.rewrite_lvalue(instance),
                inputs: inputs
                    .iter()
                    .map(|(k, v)| (k.clone(), self.rewrite_expr(v, false)))
                    .collect(),
            },
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => StmtKind::If {
                condition: self.rewrite_expr(condition, false),
                then_body: then_body.iter().map(|s| self.rewrite_stmt(s)).collect(),
                elif_arms: elif_arms
                    .iter()
                    .map(|arm| ElifArm {
                        condition: self.rewrite_expr(&arm.condition, false),
                        body: arm.body.iter().map(|s| self.rewrite_stmt(s)).collect(),
                    })
                    .collect(),
                else_body: else_body.iter().map(|s| self.rewrite_stmt(s)).collect(),
            },
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => StmtKind::Switch {
                scrutinee: self.rewrite_expr(scrutinee, false),
                cases: cases
                    .iter()
                    .map(|c| SwitchCase {
                        patterns: c
                            .patterns
                            .iter()
                            .map(|pat| match pat {
                                CasePattern::Value(e) => {
                                    CasePattern::Value(self.rewrite_expr(e, false))
                                }
                                CasePattern::Range { start, end } => CasePattern::Range {
                                    start: self.rewrite_expr(start, false),
                                    end: self.rewrite_expr(end, false),
                                },
                            })
                            .collect(),
                        body: c.body.iter().map(|s| self.rewrite_stmt(s)).collect(),
                    })
                    .collect(),
                default: default
                    .as_ref()
                    .map(|d| d.iter().map(|s| self.rewrite_stmt(s)).collect()),
            },
            StmtKind::While { condition, body } => StmtKind::While {
                condition: self.rewrite_expr(condition, false),
                body: body.iter().map(|s| self.rewrite_stmt(s)).collect(),
            },
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            } => StmtKind::For {
                counter: counter.clone(),
                from: self.rewrite_expr(from, false),
                to: self.rewrite_expr(to, false),
                step: self.rewrite_expr(step, false),
                inclusive: *inclusive,
                body: body.iter().map(|s| self.rewrite_stmt(s)).collect(),
            },
            StmtKind::Break => StmtKind::Break,
            StmtKind::Continue => StmtKind::Continue,
            StmtKind::Return { value } => StmtKind::Return {
                value: value.as_ref().map(|e| self.rewrite_expr(e, false)),
            },
            StmtKind::Wait { duration } => StmtKind::Wait {
                duration: self.rewrite_expr(duration, false),
            },
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            } => StmtKind::Foreign {
                language: language.clone(),
                fidelity: *fidelity,
                text: text.clone(),
                pack_hint: pack_hint.clone(),
            },
        };
        Stmt::new(id, kind, Span::default())
    }

    fn rewrite_lvalue(&self, lv: &LValue) -> LValue {
        match lv {
            LValue::Var { name, .. } => LValue::Var {
                name: name.clone(),
                span: Span::default(),
            },
            LValue::Index { base, index, .. } => LValue::Index {
                base: Box::new(self.rewrite_lvalue(base)),
                index: Box::new(self.rewrite_expr(index, false)),
                span: Span::default(),
            },
            LValue::Field { base, field, .. } => LValue::Field {
                base: Box::new(self.rewrite_lvalue(base)),
                field: field.clone(),
                span: Span::default(),
            },
        }
    }

    fn rewrite_expr(&self, expr: &Expr, use_frozen: bool) -> Expr {
        let kind = match &expr.kind {
            ExprKind::Literal(lit) => ExprKind::Literal(lit.clone()),
            ExprKind::Ident(name) => ExprKind::Ident(name.clone()),
            ExprKind::Index { base, index } => ExprKind::Index {
                base: Box::new(self.rewrite_expr(base, use_frozen)),
                index: Box::new(self.rewrite_expr(index, use_frozen)),
            },
            ExprKind::Field { base, field } => {
                if let ExprKind::Ident(step_name) = &base.kind
                    && let Some(skey) = self.step_keys.get(step_name)
                {
                    if field == "X" {
                        let vname = if use_frozen {
                            step_frozen_var(skey)
                        } else {
                            step_x_var(skey)
                        };
                        return ident_expr(&vname);
                    }
                    if field == "T" {
                        return ident_expr(&step_t_var(skey));
                    }
                    if field == "DONE" && self.macro_steps.contains(step_name) {
                        return ident_expr(&macro_done_var(skey));
                    }
                }
                ExprKind::Field {
                    base: Box::new(self.rewrite_expr(base, use_frozen)),
                    field: field.clone(),
                }
            }
            ExprKind::Call { callee, args } => ExprKind::Call {
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|a| CallArg {
                        name: a.name.clone(),
                        value: self.rewrite_expr(&a.value, use_frozen),
                        span: Span::default(),
                    })
                    .collect(),
            },
            ExprKind::Unary { op, operand } => ExprKind::Unary {
                op: *op,
                operand: Box::new(self.rewrite_expr(operand, use_frozen)),
            },
            ExprKind::Binary { op, left, right } => ExprKind::Binary {
                op: *op,
                left: Box::new(self.rewrite_expr(left, use_frozen)),
                right: Box::new(self.rewrite_expr(right, use_frozen)),
            },
            ExprKind::Foreign { language, text } => ExprKind::Foreign {
                language: language.clone(),
                text: text.clone(),
            },
        };
        Expr::new(kind, Span::default())
    }
}

fn unique_step_key(raw: &str, used: &mut BTreeSet<String>) -> String {
    let base = sanitize_ident(raw);
    let mut candidate = base.clone();
    let mut suffix = 1u32;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}_{suffix}");
        suffix = suffix.saturating_add(1);
    }
    candidate
}

fn sanitize_ident(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_underscore = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_underscore = false;
        } else if !prev_underscore && !out.is_empty() {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        let _ = out.pop();
    }
    if out.is_empty() {
        "step".to_owned()
    } else {
        out
    }
}

fn lvalue_signature(lv: &LValue) -> String {
    match lv {
        LValue::Var { name, .. } => name.clone(),
        LValue::Index { base, index, .. } => {
            format!("{}[{:?}]", lvalue_signature(base), index.strip_spans().kind)
        }
        LValue::Field { base, field, .. } => format!("{}.{field}", lvalue_signature(base)),
    }
}

fn step_x_var(key: &str) -> String {
    format!("sfc_step_{key}_x")
}

fn step_prev_var(key: &str) -> String {
    format!("sfc_step_{key}_prev")
}

fn step_frozen_var(key: &str) -> String {
    format!("sfc_step_{key}_frozen")
}

fn step_t_var(key: &str) -> String {
    format!("sfc_step_{key}_t")
}

fn step_claim_var(key: &str) -> String {
    format!("sfc_claim_{key}")
}

fn macro_done_var(key: &str) -> String {
    format!("sfc_macro_{key}_done")
}

fn macro_reset_var(key: &str) -> String {
    format!("sfc_macro_{key}_reset")
}

fn tr_fired_var(idx: usize) -> String {
    format!("sfc_tr_{idx}_fired")
}

fn n_prev_var(key: &str) -> String {
    format!("sfc_n_prev_{key}")
}

fn n_curr_var(key: &str) -> String {
    format!("sfc_n_curr_{key}")
}

fn s_latch_var(key: &str) -> String {
    format!("sfc_s_latch_{key}")
}

fn r_hit_var(key: &str) -> String {
    format!("sfc_r_hit_{key}")
}

fn bool_lit(b: bool) -> Expr {
    Expr::new(ExprKind::Literal(Literal::Bool(b)), Span::default())
}

fn int_lit(n: i128) -> Expr {
    Expr::new(ExprKind::Literal(Literal::Int(n)), Span::default())
}

fn time_lit(us: i64) -> Expr {
    Expr::new(ExprKind::Literal(Literal::TimeUs(us)), Span::default())
}

fn ident_expr(name: &str) -> Expr {
    Expr::new(ExprKind::Ident(name.to_owned()), Span::default())
}

fn unary_expr(op: UnaryOp, operand: Expr) -> Expr {
    Expr::new(
        ExprKind::Unary {
            op,
            operand: Box::new(operand),
        },
        Span::default(),
    )
}

fn bin_expr(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::new(
        ExprKind::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        },
        Span::default(),
    )
}

fn conjoin_exprs(terms: Vec<Expr>) -> Expr {
    let mut iter = terms.into_iter();
    let Some(first) = iter.next() else {
        return bool_lit(true);
    };
    iter.fold(first, |acc, next| bin_expr(BinaryOp::And, acc, next))
}
