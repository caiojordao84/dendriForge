//! Lower GRAFCET / SFC charts to ASL setup/loop (step-token state machine).
//!
//! MVP strategy: one BOOL token per step (`__sfc_<id>`). Active steps run their
//! actions each scan; transitions fire when all source steps are active and the
//! receptivity is true, then deactivate sources and activate targets.

use dendri_ast::expr::{BinaryOp, Expr};
use dendri_ast::stmt::Stmt;
use dendri_ast::{
    DataEntry, DataType, GrafcetBlock, HardwareEntry, Metadata, Program, Scope,
};

const TOKEN_PREFIX: &str = "__sfc_";

pub fn step_token(step_id: &str) -> String {
    format!("{TOKEN_PREFIX}{step_id}")
}

pub fn lower_grafcet_to_program(
    block: &GrafcetBlock,
    hardware: Vec<HardwareEntry>,
    mut data: Vec<DataEntry>,
    mut setup: Vec<Stmt>,
) -> Program {
    let initial: Vec<String> = if block.initial_steps.is_empty() {
        block
            .steps
            .iter()
            .filter(|s| s.initial)
            .map(|s| s.id.clone())
            .collect()
    } else {
        block.initial_steps.clone()
    };

    for step in &block.steps {
        let name = step_token(&step.id);
        if data.iter().any(|d| d.name == name) {
            continue;
        }
        let is_init = initial.iter().any(|i| i == &step.id) || step.initial;
        data.push(DataEntry {
            name,
            data_type: DataType::Bool,
            scope: Scope::Var,
            init: if is_init { "TRUE" } else { "FALSE" }.into(),
            unit: "-".into(),
        });
    }

    // Ensure tokens match initial set in setup (in case data init is ignored later).
    for step in &block.steps {
        let tok = step_token(&step.id);
        let is_init = initial.iter().any(|i| i == &step.id) || step.initial;
        setup.push(Stmt::Assign {
            target: tok,
            value: Expr::Bool { value: is_init },
        });
    }

    let mut loop_body = Vec::new();

    // 1) Execute actions of active steps.
    for step in &block.steps {
        if step.actions.is_empty() {
            continue;
        }
        loop_body.push(Stmt::If {
            condition: Expr::Var {
                name: step_token(&step.id),
            },
            then: step.actions.clone(),
            elif: vec![],
            else_body: None,
        });
    }

    // 2) Evaluate transitions (collect fires, then apply — single pass via temps).
    for tr in &block.transitions {
        let mut cond = tr.condition.clone();
        for src in &tr.from {
            cond = Expr::Binop {
                op: BinaryOp::And,
                left: Box::new(cond),
                right: Box::new(Expr::Var {
                    name: step_token(src),
                }),
            };
        }
        let fire = format!("__sfc_fire_{}", tr.id);
        if !data.iter().any(|d| d.name == fire) {
            data.push(DataEntry {
                name: fire.clone(),
                data_type: DataType::Bool,
                scope: Scope::Temp,
                init: "FALSE".into(),
                unit: "-".into(),
            });
        }
        loop_body.push(Stmt::Assign {
            target: fire.clone(),
            value: cond,
        });

        let mut then_body = Vec::new();
        for src in &tr.from {
            then_body.push(Stmt::Assign {
                target: step_token(src),
                value: Expr::Bool { value: false },
            });
        }
        for dst in &tr.to {
            then_body.push(Stmt::Assign {
                target: step_token(dst),
                value: Expr::Bool { value: true },
            });
        }
        loop_body.push(Stmt::If {
            condition: Expr::Var { name: fire },
            then: then_body,
            elif: vec![],
            else_body: None,
        });
    }

    Program {
        metadata: Metadata {
            project_name: Some(format!("{}-asl", block.name)),
            editor: Some("dendriForge".into()),
            author: None,
            notes: Some("lowered from grafcet/sfc".into()),
            expect_error: None,
        },
        hardware,
        data,
        setup,
        loop_body,
        functions: vec![],
        ladder: vec![],
        grafcet: vec![block.clone()],
    }
}
