//! Lower GRAFCET / SFC charts to ASL setup/loop (step-token state machine).
//!
//! Strategy: one BOOL token per step (`__sfc_<id>`). Active steps run their
//! actions each scan; transitions fire when all source steps are active and the
//! receptivity is true.
//!
//! Transition apply is **two-phase**: all `__sfc_fire_*` flags are computed
//! first, then all token updates run. That keeps competing OR receptivities
//! stable within a scan (sources clear after every fire is decided).
//!
//! Nested / macro-steps ([`GrafcetStep::enclosed`](dendri_ast::GrafcetStep::enclosed)):
//! tokens are namespaced `__sfc_<parent>__<child>`; on rising edge of the parent
//! token the enclosed initials activate; while the parent is inactive all
//! enclosed tokens clear.

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::stmt::Stmt;
use dendri_ast::{
    DataEntry, DataType, GrafcetBlock, HardwareEntry, Metadata, Program, Scope,
};

const TOKEN_PREFIX: &str = "__sfc_";

pub fn step_token(step_id: &str) -> String {
    format!("{TOKEN_PREFIX}{step_id}")
}

fn token(prefix: &str, step_id: &str) -> String {
    if prefix.is_empty() {
        step_token(step_id)
    } else {
        format!("{TOKEN_PREFIX}{prefix}{step_id}")
    }
}

fn fire_name(prefix: &str, tr_id: &str) -> String {
    if prefix.is_empty() {
        format!("__sfc_fire_{tr_id}")
    } else {
        format!("__sfc_fire_{prefix}{tr_id}")
    }
}

fn child_prefix(parent_prefix: &str, step_id: &str) -> String {
    format!("{parent_prefix}{step_id}__")
}

fn prev_name(prefix: &str, step_id: &str) -> String {
    format!("__sfc_prev_{prefix}{step_id}")
}

fn ensure_bool(data: &mut Vec<DataEntry>, name: String, init_true: bool) {
    if data.iter().any(|d| d.name == name) {
        return;
    }
    data.push(DataEntry {
        name,
        data_type: DataType::Bool,
        scope: Scope::Var,
        init: if init_true { "TRUE" } else { "FALSE" }.into(),
        unit: "-".into(),
    });
}

fn ensure_temp_bool(data: &mut Vec<DataEntry>, name: String) {
    if data.iter().any(|d| d.name == name) {
        return;
    }
    data.push(DataEntry {
        name,
        data_type: DataType::Bool,
        scope: Scope::Temp,
        init: "FALSE".into(),
        unit: "-".into(),
    });
}

fn initial_ids(block: &GrafcetBlock) -> Vec<String> {
    if block.initial_steps.is_empty() {
        block
            .steps
            .iter()
            .filter(|s| s.initial)
            .map(|s| s.id.clone())
            .collect()
    } else {
        block.initial_steps.clone()
    }
}

fn and_expr(a: Expr, b: Expr) -> Expr {
    Expr::Binop {
        op: BinaryOp::And,
        left: Box::new(a),
        right: Box::new(b),
    }
}

fn not_var(name: String) -> Expr {
    Expr::Unary {
        op: UnaryOp::Not,
        expr: Box::new(Expr::Var { name }),
    }
}

fn var(name: String) -> Expr {
    Expr::Var { name }
}

fn assign_bool(target: String, value: bool) -> Stmt {
    Stmt::Assign {
        target,
        value: Expr::Bool { value },
    }
}

/// Collect every step id under `block` (including nested) with full prefix path.
fn collect_all_step_tokens(block: &GrafcetBlock, prefix: &str, out: &mut Vec<String>) {
    for step in &block.steps {
        out.push(token(prefix, &step.id));
        if let Some(enc) = step.enclosed.as_deref() {
            collect_all_step_tokens(enc, &child_prefix(prefix, &step.id), out);
        }
    }
}

struct LowerAcc {
    data: Vec<DataEntry>,
    setup: Vec<Stmt>,
    actions: Vec<Stmt>,
    fires: Vec<Stmt>,
    applies: Vec<Stmt>,
    macros: Vec<Stmt>,
}

fn lower_chart(block: &GrafcetBlock, prefix: &str, gate: Option<Expr>, acc: &mut LowerAcc) {
    let initial = initial_ids(block);

    for step in &block.steps {
        let name = token(prefix, &step.id);
        let is_init = gate.is_none() && (initial.iter().any(|i| i == &step.id) || step.initial);
        ensure_bool(&mut acc.data, name.clone(), is_init);
        // Root chart: force token match initials in setup.
        if gate.is_none() {
            acc.setup.push(assign_bool(name, is_init));
        }
    }

    // 1) Actions of active steps (gated when nested).
    for step in &block.steps {
        if step.actions.is_empty() {
            continue;
        }
        let mut cond = var(token(prefix, &step.id));
        if let Some(g) = &gate {
            cond = and_expr(g.clone(), cond);
        }
        acc.actions.push(Stmt::If {
            condition: cond,
            then: step.actions.clone(),
            elif: vec![],
            else_body: None,
        });
    }

    // 2a) Fire flags
    for tr in &block.transitions {
        let mut cond = tr.condition.clone();
        for src in &tr.from {
            cond = and_expr(cond, var(token(prefix, src)));
        }
        if let Some(g) = &gate {
            cond = and_expr(g.clone(), cond);
        }
        let fire = fire_name(prefix, &tr.id);
        ensure_temp_bool(&mut acc.data, fire.clone());
        acc.fires.push(Stmt::Assign {
            target: fire,
            value: cond,
        });
    }

    // 2b) Apply fires
    for tr in &block.transitions {
        let fire = fire_name(prefix, &tr.id);
        let mut then_body = Vec::new();
        for src in &tr.from {
            then_body.push(assign_bool(token(prefix, src), false));
        }
        for dst in &tr.to {
            then_body.push(assign_bool(token(prefix, dst), true));
        }
        acc.applies.push(Stmt::If {
            condition: var(fire),
            then: then_body,
            elif: vec![],
            else_body: None,
        });
    }

    // 3) Nested / macro-steps
    for step in &block.steps {
        let Some(enclosed) = step.enclosed.as_deref() else {
            continue;
        };
        let parent_tok = token(prefix, &step.id);
        let prev = prev_name(prefix, &step.id);
        ensure_bool(&mut acc.data, prev.clone(), false);

        let child_pfx = child_prefix(prefix, &step.id);
        let child_initials = initial_ids(enclosed);

        // Ensure child tokens exist (inactive until parent rising edge).
        for child in &enclosed.steps {
            ensure_bool(&mut acc.data, token(&child_pfx, &child.id), false);
        }

        // Rising edge of parent → activate enclosed initials, clear others.
        let mut rise_body = Vec::new();
        for child in &enclosed.steps {
            let is_init =
                child_initials.iter().any(|i| i == &child.id) || child.initial;
            rise_body.push(assign_bool(token(&child_pfx, &child.id), is_init));
        }
        // Also clear deeper nested tokens under children that are not initial.
        let mut all_deep = Vec::new();
        for child in &enclosed.steps {
            if let Some(enc2) = child.enclosed.as_deref() {
                collect_all_step_tokens(enc2, &child_prefix(&child_pfx, &child.id), &mut all_deep);
            }
        }
        for deep in &all_deep {
            rise_body.push(assign_bool(deep.clone(), false));
        }

        let rise_cond = and_expr(var(parent_tok.clone()), not_var(prev.clone()));
        acc.macros.push(Stmt::If {
            condition: rise_cond,
            then: rise_body,
            elif: vec![],
            else_body: None,
        });

        // Parent inactive → clear every enclosed token (all depths).
        let mut clear_body = Vec::new();
        let mut clear_tokens = Vec::new();
        collect_all_step_tokens(enclosed, &child_pfx, &mut clear_tokens);
        for t in clear_tokens {
            clear_body.push(assign_bool(t, false));
        }
        acc.macros.push(Stmt::If {
            condition: not_var(parent_tok.clone()),
            then: clear_body,
            elif: vec![],
            else_body: None,
        });

        // Remember parent activity for next scan.
        acc.macros.push(Stmt::Assign {
            target: prev,
            value: var(parent_tok.clone()),
        });

        // Recurse: enclosed chart gated by parent token.
        let nested_gate = match &gate {
            Some(g) => and_expr(g.clone(), var(parent_tok)),
            None => var(parent_tok),
        };
        lower_chart(enclosed, &child_pfx, Some(nested_gate), acc);
    }
}

pub fn lower_grafcet_to_program(
    block: &GrafcetBlock,
    hardware: Vec<HardwareEntry>,
    mut data: Vec<DataEntry>,
    mut setup: Vec<Stmt>,
) -> Program {
    let mut acc = LowerAcc {
        data: std::mem::take(&mut data),
        setup: std::mem::take(&mut setup),
        actions: Vec::new(),
        fires: Vec::new(),
        applies: Vec::new(),
        macros: Vec::new(),
    };
    lower_chart(block, "", None, &mut acc);

    let mut loop_body = Vec::new();
    loop_body.append(&mut acc.actions);
    loop_body.append(&mut acc.fires);
    loop_body.append(&mut acc.applies);
    loop_body.append(&mut acc.macros);

    Program {
        metadata: Metadata {
            project_name: Some(format!("{}-asl", block.name)),
            editor: Some("dendriForge".into()),
            author: None,
            notes: Some("lowered from grafcet/sfc".into()),
            expect_error: None,
        },
        hardware,
        data: acc.data,
        setup: acc.setup,
        loop_body,
        functions: vec![],
        ladder: vec![],
        grafcet: vec![block.clone()],
    }
}
