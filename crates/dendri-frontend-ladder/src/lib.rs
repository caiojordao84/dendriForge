//! Ladder diagram TOML ↔ shared ASL AST (DEN-12 depth beyond linear).
//!
//! Source text is a TOML [`LadderDocument`]. Lowering produces left-to-right
//! series power-flow (plus one-level parallel OR rails) and honest IEC FB calls
//! (`TON`/`TOF`/`TP`/`CTU`/`CTD`/`CTUD`/`R_TRIG`/`F_TRIG`) for `dendri-sim`.

mod lower;

use dendri_ast::{LadderBlock, LadderDocument, Program, SourceLanguage};
use thiserror::Error;

pub use lower::lower_ladder_to_program;

#[derive(Debug, Error)]
pub enum LadderError {
    #[error("ladder TOML parse: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("{0}")]
    Message(String),
}

/// Parse ladder diagram TOML into a full ASL [`Program`] (block + lowered body).
pub fn parse_ladder(text: &str) -> Result<Program, LadderError> {
    let doc: LadderDocument = toml::from_str(text)?;
    Ok(document_to_program(doc))
}

/// Emit ladder diagram TOML from ASL (prefers `program.ladder`; else synthesizes actions).
pub fn emit_ladder(program: &Program) -> String {
    let doc = program_to_document(program);
    toml::to_string_pretty(&doc).unwrap_or_else(|_| String::new())
}

pub fn document_to_program(doc: LadderDocument) -> Program {
    let name = doc.name.clone();
    let hardware = doc.hardware.clone();
    let data = doc.data.clone();
    let setup = doc.setup.clone();
    let block = doc.into_block();
    let mut program = lower_ladder_to_program(&block, hardware, data, setup);
    if program.metadata.project_name.is_none() {
        program.metadata.project_name = Some(format!("{name}-asl"));
    }
    program.metadata.editor = Some("dendriForge".into());
    program.ladder = vec![block];
    program
}

pub fn program_to_document(program: &Program) -> LadderDocument {
    if let Some(block) = program.ladder.first() {
        let mut doc = LadderDocument::from_block(block);
        doc.hardware = program.hardware.clone();
        doc.data = program.data.clone();
        doc.setup = program.setup.clone();
        return doc;
    }
    // Best-effort: pack loop stmts as a single always-powered action rung.
    LadderDocument {
        name: program
            .metadata
            .project_name
            .clone()
            .unwrap_or_else(|| "ladder".into()),
        rungs: vec![dendri_ast::LadderRung {
            id: "r1".into(),
            cells: vec![dendri_ast::LadderCell::Action {
                stmts: program.loop_body.clone(),
                col: 0,
            }],
            comment: Some("synthesized from ASL loop".into()),
            row: 0,
        }],
        io_map: vec![],
        hardware: program.hardware.clone(),
        data: program.data.clone(),
        setup: program.setup.clone(),
    }
}

pub fn block_from_program(program: &Program) -> Option<&LadderBlock> {
    program.ladder.first()
}

pub fn language() -> SourceLanguage {
    SourceLanguage::Ladder
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::expr::Expr;
    use dendri_ast::stmt::Stmt;
    use dendri_ast::{DataEntry, DataType, HardwareEntry, HwType, IoMode, LadderCell, LadderRung, Scope};

    fn blink_doc() -> LadderDocument {
        LadderDocument {
            name: "blink".into(),
            rungs: vec![LadderRung {
                id: "r1".into(),
                row: 0,
                comment: Some("free-run blink actions".into()),
                cells: vec![LadderCell::Action {
                    col: 0,
                    stmts: vec![
                        Stmt::Assign {
                            target: "ledState".into(),
                            value: Expr::Unary {
                                op: dendri_ast::expr::UnaryOp::Not,
                                expr: Box::new(Expr::Var {
                                    name: "ledState".into(),
                                }),
                            },
                        },
                        Stmt::Call {
                            name: "SET_BOOL".into(),
                            args: vec![
                                Expr::Uint { value: 13 },
                                Expr::Var {
                                    name: "ledState".into(),
                                },
                            ],
                        },
                        Stmt::Call {
                            name: "WAIT_TIME".into(),
                            args: vec![Expr::Var {
                                name: "INTERVAL".into(),
                            }],
                        },
                    ],
                }],
            }],
            io_map: vec![],
            hardware: vec![HardwareEntry {
                pin: 13,
                hw_type: HwType::Digital,
                io_mode: IoMode::Do,
                label: "ledPin".into(),
                target_role: "digital-out".into(),
                pull: Default::default(),
            }],
            data: vec![
                DataEntry {
                    name: "INTERVAL".into(),
                    data_type: DataType::Time,
                    scope: Scope::Const,
                    init: "T#100ms".into(),
                    unit: "ms".into(),
                },
                DataEntry {
                    name: "ledState".into(),
                    data_type: DataType::Bool,
                    scope: Scope::Var,
                    init: "FALSE".into(),
                    unit: "-".into(),
                },
            ],
            setup: vec![Stmt::Call {
                name: "CFG_DIGITAL_OUTPUT".into(),
                args: vec![Expr::Uint { value: 13 }],
            }],
        }
    }

    #[test]
    fn roundtrip_document_toml() {
        let doc = blink_doc();
        let text = toml::to_string_pretty(&doc).unwrap();
        let again: LadderDocument = toml::from_str(&text).unwrap();
        assert_eq!(again, doc);
    }

    #[test]
    fn parse_emit_preserves_block() {
        let doc = blink_doc();
        let text = toml::to_string_pretty(&doc).unwrap();
        let program = parse_ladder(&text).unwrap();
        let emitted = emit_ladder(&program);
        let again = parse_ladder(&emitted).unwrap();
        assert_eq!(again.ladder, program.ladder);
        assert_eq!(again.loop_body, program.loop_body);
    }

    #[test]
    fn contact_coil_lowers() {
        let doc = LadderDocument {
            name: "btn_led".into(),
            rungs: vec![LadderRung {
                id: "r1".into(),
                row: 0,
                comment: None,
                cells: vec![
                    LadderCell::ContactNo {
                        operand: "btn".into(),
                        col: 0,
                    },
                    LadderCell::Coil {
                        operand: "led".into(),
                        col: 1,
                    },
                    LadderCell::Action {
                        col: 2,
                        stmts: vec![Stmt::Call {
                            name: "SET_BOOL".into(),
                            args: vec![
                                Expr::Uint { value: 13 },
                                Expr::Var {
                                    name: "led".into(),
                                },
                            ],
                        }],
                    },
                ],
            }],
            io_map: vec![],
            hardware: vec![],
            data: vec![
                DataEntry {
                    name: "btn".into(),
                    data_type: DataType::Bool,
                    scope: Scope::Var,
                    init: "FALSE".into(),
                    unit: "-".into(),
                },
                DataEntry {
                    name: "led".into(),
                    data_type: DataType::Bool,
                    scope: Scope::Var,
                    init: "FALSE".into(),
                    unit: "-".into(),
                },
            ],
            setup: vec![],
        };
        let p = document_to_program(doc);
        assert!(!p.loop_body.is_empty());
        assert_eq!(p.ladder.len(), 1);
    }

    #[test]
    fn parallel_or_lowers_to_or_binop() {
        use dendri_ast::LadderBranch;
        let doc = LadderDocument {
            name: "or_rail".into(),
            rungs: vec![LadderRung {
                id: "r1".into(),
                row: 0,
                comment: None,
                cells: vec![
                    LadderCell::Parallel {
                        col: 0,
                        branches: vec![
                            LadderBranch {
                                id: "b0".into(),
                                cells: vec![LadderCell::ContactNo {
                                    operand: "a".into(),
                                    col: 0,
                                }],
                            },
                            LadderBranch {
                                id: "b1".into(),
                                cells: vec![LadderCell::ContactNo {
                                    operand: "b".into(),
                                    col: 0,
                                }],
                            },
                        ],
                    },
                    LadderCell::Coil {
                        operand: "out".into(),
                        col: 1,
                    },
                ],
            }],
            io_map: vec![],
            hardware: vec![],
            data: vec![],
            setup: vec![],
        };
        let p = document_to_program(doc);
        let text = format!("{:?}", p.loop_body);
        assert!(text.contains("Or"), "{text}");
        assert!(text.contains("out"), "{text}");
    }

    #[test]
    fn iec_fbs_lower_to_sim_opcodes() {
        use dendri_ast::{CounterFbKind, TimerFbKind};
        let doc = LadderDocument {
            name: "fbs".into(),
            rungs: vec![
                LadderRung {
                    id: "r_ton".into(),
                    row: 0,
                    comment: None,
                    cells: vec![
                        LadderCell::ContactNo {
                            operand: "in".into(),
                            col: 0,
                        },
                        LadderCell::FbTimer {
                            fb: TimerFbKind::Tof,
                            instance: "tof1".into(),
                            args: vec![Expr::Var {
                                name: "PT".into(),
                            }],
                            col: 1,
                        },
                    ],
                },
                LadderRung {
                    id: "r_edge".into(),
                    row: 1,
                    comment: None,
                    cells: vec![
                        LadderCell::ContactNo {
                            operand: "btn".into(),
                            col: 0,
                        },
                        LadderCell::FbEdge {
                            rising: true,
                            instance: "rt1".into(),
                            operand: None,
                            col: 1,
                        },
                    ],
                },
                LadderRung {
                    id: "r_ctu".into(),
                    row: 2,
                    comment: None,
                    cells: vec![
                        LadderCell::ContactNo {
                            operand: "pulse".into(),
                            col: 0,
                        },
                        LadderCell::FbCounter {
                            fb: CounterFbKind::Ctu,
                            instance: "ctu1".into(),
                            args: vec![
                                Expr::Var {
                                    name: "reset".into(),
                                },
                                Expr::Var {
                                    name: "pv".into(),
                                },
                            ],
                            col: 1,
                        },
                    ],
                },
            ],
            io_map: vec![],
            hardware: vec![],
            data: vec![],
            setup: vec![],
        };
        let p = document_to_program(doc);
        let dump = format!("{:?}", p.loop_body);
        assert!(dump.contains("TOF"), "{dump}");
        assert!(dump.contains("R_TRIG"), "{dump}");
        assert!(dump.contains("CTU"), "{dump}");
    }
}
