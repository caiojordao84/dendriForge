//! SFC / GRAFCET diagram TOML ↔ shared ASL AST (DEN-12 depth).
//!
//! Source text is a TOML [`GrafcetDocument`]. Lowering produces a step-token
//! state machine with two-phase transition apply (AND fork/join via multi
//! `from`/`to`). Nested charts remain deferred.

mod lower;

use dendri_ast::{GrafcetBlock, GrafcetDocument, Program, SourceLanguage};
use thiserror::Error;

pub use lower::lower_grafcet_to_program;

#[derive(Debug, Error)]
pub enum SfcError {
    #[error("sfc TOML parse: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("{0}")]
    Message(String),
}

pub fn parse_sfc(text: &str) -> Result<Program, SfcError> {
    let doc: GrafcetDocument = toml::from_str(text)?;
    Ok(document_to_program(doc))
}

pub fn emit_sfc(program: &Program) -> String {
    let doc = program_to_document(program);
    toml::to_string_pretty(&doc).unwrap_or_else(|_| String::new())
}

pub fn document_to_program(doc: GrafcetDocument) -> Program {
    let name = doc.name.clone();
    let hardware = doc.hardware.clone();
    let data = doc.data.clone();
    let setup = doc.setup.clone();
    let block = doc.into_block();
    let mut program = lower_grafcet_to_program(&block, hardware, data, setup);
    if program.metadata.project_name.is_none() {
        program.metadata.project_name = Some(format!("{name}-asl"));
    }
    program.metadata.editor = Some("dendriForge".into());
    program.grafcet = vec![block];
    program
}

pub fn program_to_document(program: &Program) -> GrafcetDocument {
    if let Some(block) = program.grafcet.first() {
        let mut doc = GrafcetDocument::from_block(block);
        doc.hardware = program.hardware.clone();
        doc.data = program.data.clone();
        doc.setup = program.setup.clone();
        return doc;
    }
    // Best-effort: one step holding the ASL loop body.
    let name = program
        .metadata
        .project_name
        .clone()
        .unwrap_or_else(|| "sfc".into());
    GrafcetDocument {
        name: name.clone(),
        initial_steps: vec!["S0".into()],
        steps: vec![dendri_ast::GrafcetStep {
            id: "S0".into(),
            initial: true,
            actions: program.loop_body.clone(),
            comment: Some("synthesized from ASL loop".into()),
            x: 0.0,
            y: 0.0,
        }],
        transitions: vec![dendri_ast::GrafcetTransition {
            id: "T0".into(),
            from: vec!["S0".into()],
            to: vec!["S0".into()],
            condition: dendri_ast::Expr::Bool { value: true },
            comment: Some("self".into()),
            x: 0.0,
            y: 40.0,
        }],
        io_map: vec![],
        hardware: program.hardware.clone(),
        data: program.data.clone(),
        setup: program.setup.clone(),
    }
}

pub fn block_from_program(program: &Program) -> Option<&GrafcetBlock> {
    program.grafcet.first()
}

pub fn language() -> SourceLanguage {
    SourceLanguage::Sfc
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::expr::Expr;
    use dendri_ast::stmt::Stmt;
    use dendri_ast::{
        DataEntry, DataType, GrafcetStep, GrafcetTransition, HardwareEntry, HwType, IoMode, Scope,
    };

    fn blink_doc() -> GrafcetDocument {
        GrafcetDocument {
            name: "blink_sfc".into(),
            initial_steps: vec!["S_on".into()],
            steps: vec![
                GrafcetStep {
                    id: "S_on".into(),
                    initial: true,
                    x: 0.0,
                    y: 0.0,
                    comment: Some("LED on".into()),
                    actions: vec![
                        Stmt::Call {
                            name: "SET_BOOL".into(),
                            args: vec![
                                Expr::Uint { value: 13 },
                                Expr::Bool { value: true },
                            ],
                        },
                        Stmt::Call {
                            name: "WAIT_TIME".into(),
                            args: vec![Expr::Var {
                                name: "ON_TIME".into(),
                            }],
                        },
                    ],
                },
                GrafcetStep {
                    id: "S_off".into(),
                    initial: false,
                    x: 0.0,
                    y: 80.0,
                    comment: Some("LED off".into()),
                    actions: vec![
                        Stmt::Call {
                            name: "SET_BOOL".into(),
                            args: vec![
                                Expr::Uint { value: 13 },
                                Expr::Bool { value: false },
                            ],
                        },
                        Stmt::Call {
                            name: "WAIT_TIME".into(),
                            args: vec![Expr::Var {
                                name: "OFF_TIME".into(),
                            }],
                        },
                    ],
                },
            ],
            transitions: vec![
                GrafcetTransition {
                    id: "T1".into(),
                    from: vec!["S_on".into()],
                    to: vec!["S_off".into()],
                    condition: Expr::Bool { value: true },
                    comment: None,
                    x: 0.0,
                    y: 40.0,
                },
                GrafcetTransition {
                    id: "T2".into(),
                    from: vec!["S_off".into()],
                    to: vec!["S_on".into()],
                    condition: Expr::Bool { value: true },
                    comment: None,
                    x: 0.0,
                    y: 120.0,
                },
            ],
            io_map: vec![],
            hardware: vec![HardwareEntry {
                pin: 13,
                hw_type: HwType::Digital,
                io_mode: IoMode::Do,
                label: "ledPin".into(),
                target_role: "digital-out".into(),
            }],
            data: vec![
                DataEntry {
                    name: "ON_TIME".into(),
                    data_type: DataType::Time,
                    scope: Scope::Const,
                    init: "T#100ms".into(),
                    unit: "ms".into(),
                },
                DataEntry {
                    name: "OFF_TIME".into(),
                    data_type: DataType::Time,
                    scope: Scope::Const,
                    init: "T#100ms".into(),
                    unit: "ms".into(),
                },
            ],
            setup: vec![
                Stmt::Call {
                    name: "CFG_DIGITAL_OUTPUT".into(),
                    args: vec![Expr::Uint { value: 13 }],
                },
                Stmt::Call {
                    name: "SET_BOOL".into(),
                    args: vec![Expr::Uint { value: 13 }, Expr::Bool { value: false }],
                },
            ],
        }
    }

    #[test]
    fn roundtrip_document_toml() {
        let doc = blink_doc();
        let text = toml::to_string_pretty(&doc).unwrap();
        let again: GrafcetDocument = toml::from_str(&text).unwrap();
        assert_eq!(again, doc);
    }

    #[test]
    fn parse_emit_preserves_block() {
        let doc = blink_doc();
        let text = toml::to_string_pretty(&doc).unwrap();
        let program = parse_sfc(&text).unwrap();
        let emitted = emit_sfc(&program);
        let again = parse_sfc(&emitted).unwrap();
        assert_eq!(again.grafcet, program.grafcet);
    }

    #[test]
    fn and_divergence_two_phase_lower() {
        let doc = GrafcetDocument {
            name: "fork".into(),
            initial_steps: vec!["S0".into()],
            steps: vec![
                GrafcetStep {
                    id: "S0".into(),
                    initial: true,
                    actions: vec![],
                    comment: None,
                    x: 0.0,
                    y: 0.0,
                },
                GrafcetStep {
                    id: "Sa".into(),
                    initial: false,
                    actions: vec![Stmt::Assign {
                        target: "a".into(),
                        value: Expr::Bool { value: true },
                    }],
                    comment: None,
                    x: -40.0,
                    y: 80.0,
                },
                GrafcetStep {
                    id: "Sb".into(),
                    initial: false,
                    actions: vec![Stmt::Assign {
                        target: "b".into(),
                        value: Expr::Bool { value: true },
                    }],
                    comment: None,
                    x: 40.0,
                    y: 80.0,
                },
            ],
            transitions: vec![GrafcetTransition {
                id: "T_fork".into(),
                from: vec!["S0".into()],
                to: vec!["Sa".into(), "Sb".into()],
                condition: Expr::Bool { value: true },
                comment: Some("AND divergence".into()),
                x: 0.0,
                y: 40.0,
            }],
            io_map: vec![],
            hardware: vec![],
            data: vec![],
            setup: vec![],
        };
        let p = document_to_program(doc);
        // Fire assign appears before any token clear/set for that transition.
        let dump = format!("{:?}", p.loop_body);
        let fire_pos = dump.find("__sfc_fire_T_fork").expect("fire flag");
        let clear_pos = dump.find("__sfc_S0").expect("source token");
        assert!(
            fire_pos < clear_pos,
            "two-phase: fire before apply\n{dump}"
        );
        assert!(dump.contains("__sfc_Sa"), "{dump}");
        assert!(dump.contains("__sfc_Sb"), "{dump}");
    }
}
