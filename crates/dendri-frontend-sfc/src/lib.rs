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
            enclosed: None,
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
                    enclosed: None,
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
                    enclosed: None,
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
    fn nested_macro_step_lower_and_roundtrip() {
        let enclosed = GrafcetBlock {
            name: "doors".into(),
            initial_steps: vec!["D_open".into()],
            steps: vec![
                GrafcetStep {
                    id: "D_open".into(),
                    initial: true,
                    actions: vec![Stmt::Assign {
                        target: "doorOpen".into(),
                        value: Expr::Bool { value: true },
                    }],
                    comment: None,
                    x: 0.0,
                    y: 0.0,
                    enclosed: None,
                },
                GrafcetStep {
                    id: "D_done".into(),
                    initial: false,
                    actions: vec![Stmt::Assign {
                        target: "doorOpen".into(),
                        value: Expr::Bool { value: false },
                    }],
                    comment: None,
                    x: 0.0,
                    y: 80.0,
                    enclosed: None,
                },
            ],
            transitions: vec![GrafcetTransition {
                id: "Td".into(),
                from: vec!["D_open".into()],
                to: vec!["D_done".into()],
                condition: Expr::Bool { value: true },
                comment: None,
                x: 0.0,
                y: 40.0,
            }],
            io_map: vec![],
        };
        let doc = GrafcetDocument {
            name: "nested_rt".into(),
            initial_steps: vec!["S_idle".into()],
            steps: vec![
                GrafcetStep {
                    id: "S_idle".into(),
                    initial: true,
                    actions: vec![],
                    comment: None,
                    x: 0.0,
                    y: 0.0,
                    enclosed: None,
                },
                GrafcetStep {
                    id: "S_doors".into(),
                    initial: false,
                    actions: vec![],
                    comment: Some("macro".into()),
                    x: 0.0,
                    y: 80.0,
                    enclosed: Some(Box::new(enclosed)),
                },
            ],
            transitions: vec![
                GrafcetTransition {
                    id: "T_enter".into(),
                    from: vec!["S_idle".into()],
                    to: vec!["S_doors".into()],
                    condition: Expr::Bool { value: true },
                    comment: None,
                    x: 0.0,
                    y: 40.0,
                },
                GrafcetTransition {
                    id: "T_leave".into(),
                    from: vec!["S_doors".into()],
                    to: vec!["S_idle".into()],
                    condition: Expr::Var {
                        name: "__sfc_S_doors__D_done".into(),
                    },
                    comment: None,
                    x: 0.0,
                    y: 120.0,
                },
            ],
            io_map: vec![],
            hardware: vec![],
            data: vec![DataEntry {
                name: "doorOpen".into(),
                data_type: DataType::Bool,
                scope: Scope::Var,
                init: "FALSE".into(),
                unit: "-".into(),
            }],
            setup: vec![],
        };
        let text = toml::to_string_pretty(&doc).unwrap();
        let program = parse_sfc(&text).unwrap();
        assert!(program.grafcet[0].steps.iter().any(|s| s.enclosed.is_some()));
        let dump = format!("{:?}", program.loop_body);
        assert!(dump.contains("__sfc_S_doors__D_open"), "{dump}");
        assert!(dump.contains("__sfc_prev_S_doors"), "{dump}");
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
                    enclosed: None,
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
                    enclosed: None,
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
                    enclosed: None,
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

    fn elevator_doc() -> GrafcetDocument {
        use dendri_ast::GrafcetBlock;
        fn gb(pin: u8) -> Expr {
            Expr::Call {
                name: "GET_BOOL".into(),
                args: vec![Expr::Uint { value: pin as u64 }],
            }
        }
        fn set_pin(pin: u8, val: bool) -> Stmt {
            Stmt::Call {
                name: "SET_BOOL".into(),
                args: vec![Expr::Uint { value: pin as u64 }, Expr::Bool { value: val }],
            }
        }
        let doors = GrafcetBlock {
            name: "door_seq".into(),
            initial_steps: vec!["D_opening".into()],
            steps: vec![
                GrafcetStep {
                    id: "D_opening".into(),
                    initial: true,
                    comment: Some("open doors + clear call".into()),
                    x: 0.0,
                    y: 0.0,
                    enclosed: None,
                    actions: vec![
                        set_pin(10, false),
                        set_pin(11, false),
                        set_pin(12, true),
                        set_pin(13, false),
                        Stmt::If {
                            condition: gb(0),
                            then: vec![Stmt::Assign {
                                target: "call0".into(),
                                value: Expr::Bool { value: false },
                            }],
                            elif: vec![],
                            else_body: None,
                        },
                        Stmt::If {
                            condition: gb(1),
                            then: vec![Stmt::Assign {
                                target: "call1".into(),
                                value: Expr::Bool { value: false },
                            }],
                            elif: vec![],
                            else_body: None,
                        },
                    ],
                },
                GrafcetStep {
                    id: "D_dwell".into(),
                    initial: false,
                    comment: Some("door open dwell".into()),
                    x: 0.0,
                    y: 80.0,
                    enclosed: None,
                    actions: vec![
                        set_pin(12, true),
                        set_pin(13, false),
                        Stmt::Call {
                            name: "WAIT_TIME".into(),
                            args: vec![Expr::Var { name: "DOOR_DELAY".into() }],
                        },
                    ],
                },
                GrafcetStep {
                    id: "D_closing".into(),
                    initial: false,
                    comment: Some("close; IR reopens".into()),
                    x: 0.0,
                    y: 160.0,
                    enclosed: None,
                    actions: vec![set_pin(12, false), set_pin(13, true)],
                },
                GrafcetStep {
                    id: "D_done".into(),
                    initial: false,
                    comment: Some("doors secured".into()),
                    x: 0.0,
                    y: 240.0,
                    enclosed: None,
                    actions: vec![set_pin(12, false), set_pin(13, true)],
                },
            ],
            transitions: vec![
                GrafcetTransition {
                    id: "Td_open".into(),
                    from: vec!["D_opening".into()],
                    to: vec!["D_dwell".into()],
                    condition: Expr::Bool { value: true },
                    comment: None,
                    x: 0.0,
                    y: 40.0,
                },
                GrafcetTransition {
                    id: "Td_dwell".into(),
                    from: vec!["D_dwell".into()],
                    to: vec!["D_closing".into()],
                    condition: Expr::Bool { value: true },
                    comment: None,
                    x: 0.0,
                    y: 120.0,
                },
                GrafcetTransition {
                    id: "Td_ir".into(),
                    from: vec!["D_closing".into()],
                    to: vec!["D_opening".into()],
                    condition: Expr::Var { name: "irBlocked".into() },
                    comment: Some("IR barrier".into()),
                    x: -40.0,
                    y: 200.0,
                },
                GrafcetTransition {
                    id: "Td_ok".into(),
                    from: vec!["D_closing".into()],
                    to: vec!["D_done".into()],
                    condition: Expr::Unary {
                        op: dendri_ast::expr::UnaryOp::Not,
                        expr: Box::new(Expr::Var { name: "irBlocked".into() }),
                    },
                    comment: None,
                    x: 40.0,
                    y: 200.0,
                },
            ],
            io_map: vec![],
        };
        GrafcetDocument {
            name: "G04-elevator-nested".into(),
            initial_steps: vec!["S_idle".into(), "S_monitor".into()],
            steps: vec![
                GrafcetStep {
                    id: "S_monitor".into(),
                    initial: true,
                    comment: Some("latch calls + IR".into()),
                    x: 120.0,
                    y: 0.0,
                    enclosed: None,
                    actions: vec![
                        Stmt::If {
                            condition: gb(2),
                            then: vec![Stmt::Assign {
                                target: "call0".into(),
                                value: Expr::Bool { value: true },
                            }],
                            elif: vec![],
                            else_body: None,
                        },
                        Stmt::If {
                            condition: gb(3),
                            then: vec![Stmt::Assign {
                                target: "call1".into(),
                                value: Expr::Bool { value: true },
                            }],
                            elif: vec![],
                            else_body: None,
                        },
                        Stmt::Assign {
                            target: "irBlocked".into(),
                            value: gb(4),
                        },
                    ],
                },
                GrafcetStep {
                    id: "S_idle".into(),
                    initial: true,
                    comment: Some("idle / interlock".into()),
                    x: 0.0,
                    y: 0.0,
                    enclosed: None,
                    actions: vec![
                        set_pin(10, false),
                        set_pin(11, false),
                        set_pin(12, false),
                        set_pin(13, true),
                    ],
                },
                GrafcetStep {
                    id: "S_move_up".into(),
                    initial: false,
                    comment: Some("motor up".into()),
                    x: 0.0,
                    y: 80.0,
                    enclosed: None,
                    actions: vec![set_pin(10, true), set_pin(11, false), set_pin(12, false), set_pin(13, false)],
                },
                GrafcetStep {
                    id: "S_move_down".into(),
                    initial: false,
                    comment: Some("motor down".into()),
                    x: 0.0,
                    y: 80.0,
                    enclosed: None,
                    actions: vec![set_pin(10, false), set_pin(11, true), set_pin(12, false), set_pin(13, false)],
                },
                GrafcetStep {
                    id: "S_doors".into(),
                    initial: false,
                    comment: Some("macro-step: door sequence".into()),
                    x: 0.0,
                    y: 160.0,
                    enclosed: Some(Box::new(doors)),
                    actions: vec![],
                },
            ],
            transitions: vec![
                GrafcetTransition {
                    id: "T_up".into(),
                    from: vec!["S_idle".into()],
                    to: vec!["S_move_up".into()],
                    condition: Expr::Binop {
                        op: dendri_ast::expr::BinaryOp::And,
                        left: Box::new(Expr::Var { name: "call1".into() }),
                        right: Box::new(gb(0)),
                    },
                    comment: Some("call floor1 while at 0".into()),
                    x: 0.0,
                    y: 40.0,
                },
                GrafcetTransition {
                    id: "T_dn".into(),
                    from: vec!["S_idle".into()],
                    to: vec!["S_move_down".into()],
                    condition: Expr::Binop {
                        op: dendri_ast::expr::BinaryOp::And,
                        left: Box::new(Expr::Var { name: "call0".into() }),
                        right: Box::new(gb(1)),
                    },
                    comment: Some("call floor0 while at 1".into()),
                    x: 40.0,
                    y: 40.0,
                },
                GrafcetTransition {
                    id: "T_arr1".into(),
                    from: vec!["S_move_up".into()],
                    to: vec!["S_doors".into()],
                    condition: gb(1),
                    comment: None,
                    x: 0.0,
                    y: 120.0,
                },
                GrafcetTransition {
                    id: "T_arr0".into(),
                    from: vec!["S_move_down".into()],
                    to: vec!["S_doors".into()],
                    condition: gb(0),
                    comment: None,
                    x: 40.0,
                    y: 120.0,
                },
                GrafcetTransition {
                    id: "T_doors_done".into(),
                    from: vec!["S_doors".into()],
                    to: vec!["S_idle".into()],
                    condition: Expr::Var {
                        name: "__sfc_S_doors__D_done".into(),
                    },
                    comment: Some("nested chart finished".into()),
                    x: 0.0,
                    y: 200.0,
                },
            ],
            io_map: vec![],
            hardware: vec![
                HardwareEntry { pin: 0, hw_type: HwType::Digital, io_mode: IoMode::Di, label: "sensorFloor0".into(), target_role: "sensor-pos".into() },
                HardwareEntry { pin: 1, hw_type: HwType::Digital, io_mode: IoMode::Di, label: "sensorFloor1".into(), target_role: "sensor-pos".into() },
                HardwareEntry { pin: 2, hw_type: HwType::Digital, io_mode: IoMode::Di, label: "callBtn0".into(), target_role: "btn-no".into() },
                HardwareEntry { pin: 3, hw_type: HwType::Digital, io_mode: IoMode::Di, label: "callBtn1".into(), target_role: "btn-no".into() },
                HardwareEntry { pin: 4, hw_type: HwType::Digital, io_mode: IoMode::Di, label: "irBarrier".into(), target_role: "ir-rx".into() },
                HardwareEntry { pin: 10, hw_type: HwType::Digital, io_mode: IoMode::Do, label: "motorUp".into(), target_role: "relay-coil".into() },
                HardwareEntry { pin: 11, hw_type: HwType::Digital, io_mode: IoMode::Do, label: "motorDn".into(), target_role: "relay-coil".into() },
                HardwareEntry { pin: 12, hw_type: HwType::Digital, io_mode: IoMode::Do, label: "doorOpen".into(), target_role: "relay-coil".into() },
                HardwareEntry { pin: 13, hw_type: HwType::Digital, io_mode: IoMode::Do, label: "doorClose".into(), target_role: "relay-coil".into() },
            ],
            data: vec![
                DataEntry { name: "DOOR_DELAY".into(), data_type: DataType::Time, scope: Scope::Const, init: "T#200ms".into(), unit: "ms".into() },
                DataEntry { name: "call0".into(), data_type: DataType::Bool, scope: Scope::Var, init: "FALSE".into(), unit: "-".into() },
                DataEntry { name: "call1".into(), data_type: DataType::Bool, scope: Scope::Var, init: "FALSE".into(), unit: "-".into() },
                DataEntry { name: "irBlocked".into(), data_type: DataType::Bool, scope: Scope::Var, init: "FALSE".into(), unit: "-".into() },
            ],
            setup: vec![
                Stmt::Call { name: "CFG_DIGITAL_INPUT".into(), args: vec![Expr::Uint { value: 0 }] },
                Stmt::Call { name: "CFG_DIGITAL_INPUT".into(), args: vec![Expr::Uint { value: 1 }] },
                Stmt::Call { name: "CFG_DIGITAL_INPUT".into(), args: vec![Expr::Uint { value: 2 }] },
                Stmt::Call { name: "CFG_DIGITAL_INPUT".into(), args: vec![Expr::Uint { value: 3 }] },
                Stmt::Call { name: "CFG_DIGITAL_INPUT".into(), args: vec![Expr::Uint { value: 4 }] },
                Stmt::Call { name: "CFG_DIGITAL_OUTPUT".into(), args: vec![Expr::Uint { value: 10 }] },
                Stmt::Call { name: "CFG_DIGITAL_OUTPUT".into(), args: vec![Expr::Uint { value: 11 }] },
                Stmt::Call { name: "CFG_DIGITAL_OUTPUT".into(), args: vec![Expr::Uint { value: 12 }] },
                Stmt::Call { name: "CFG_DIGITAL_OUTPUT".into(), args: vec![Expr::Uint { value: 13 }] },
            ],
        }
    }

    #[test]
    fn elevator_nested_sim_and_roundtrip() {
        use dendri_sim::{SimConfig, Simulator};
        let doc = elevator_doc();
        let text = toml::to_string_pretty(&doc).unwrap();
        let program = parse_sfc(&text).unwrap();
        let emitted = emit_sfc(&program);
        let again = parse_sfc(&emitted).unwrap();
        assert_eq!(again.grafcet, program.grafcet);

        let mut sim = Simulator::from_program(
            program,
            &SimConfig { max_cycles: 40, ..SimConfig::default() },
        );
        sim.ensure_setup().unwrap();
        // At floor 0
        sim.set_pin(0, true).unwrap();
        sim.set_pin(1, false).unwrap();
        sim.step().unwrap();
        assert!(sim.var("__sfc_S_idle").and_then(|v| v.as_bool().ok()).unwrap_or(false));

        // Call floor 1 — latch + fire this scan; motor action next scan
        sim.set_pin(3, true).unwrap();
        sim.step().unwrap();
        assert!(sim.var("call1").and_then(|v| v.as_bool().ok()).unwrap_or(false));
        assert_eq!(
            sim.var("__sfc_S_move_up").and_then(|v| v.as_bool().ok()),
            Some(true),
            "should enter move_up: {}",
            sim.trace_text()
        );
        sim.step().unwrap();
        assert!(sim.pin(10), "motor up should be on: {}", sim.trace_text());

        // Arrive floor 1
        sim.set_pin(0, false).unwrap();
        sim.set_pin(1, true).unwrap();
        sim.set_pin(3, false).unwrap();
        for _ in 0..12 {
            sim.step().unwrap();
            if sim.var("__sfc_S_idle").and_then(|v| v.as_bool().ok()) == Some(true)
                && sim.var("call1").and_then(|v| v.as_bool().ok()) == Some(false)
            {
                break;
            }
        }
        assert_eq!(sim.var("call1").and_then(|v| v.as_bool().ok()), Some(false), "call cleared after doors");
        assert!(
            sim.var("__sfc_S_idle").and_then(|v| v.as_bool().ok()).unwrap_or(false),
            "back to idle after nested doors: {}",
            sim.trace_text()
        );
        assert!(!sim.pin(10) && !sim.pin(11), "motors off at idle");
        let trace = sim.trace_text();
        assert!(trace.contains("PIN_WRITE pin=12"), "door open commanded: {trace}");
    }

    #[test]
    fn g04_fixture_file_roundtrip_and_sim() {
        use dendri_sim::{SimConfig, Simulator};
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/fixtures/sfc/G04-elevator-nested.sfc.toml");
        let src = std::fs::read_to_string(&path).expect("G04 fixture");
        let program = parse_sfc(&src).unwrap();
        assert!(
            program.grafcet[0]
                .steps
                .iter()
                .any(|s| s.id == "S_doors" && s.enclosed.is_some()),
            "G04 must include nested door macro-step"
        );
        let emitted = emit_sfc(&program);
        let again = parse_sfc(&emitted).unwrap();
        assert_eq!(again.grafcet, program.grafcet);

        let mut sim = Simulator::from_program(
            program,
            &SimConfig {
                max_cycles: 40,
                ..SimConfig::default()
            },
        );
        sim.ensure_setup().unwrap();
        sim.set_pin(0, true).unwrap();
        sim.step().unwrap();
        sim.set_pin(3, true).unwrap();
        sim.step().unwrap();
        sim.step().unwrap();
        assert!(sim.pin(10), "G04 motor up: {}", sim.trace_text());
        sim.set_pin(0, false).unwrap();
        sim.set_pin(1, true).unwrap();
        sim.set_pin(3, false).unwrap();
        for _ in 0..12 {
            sim.step().unwrap();
            if sim.var("__sfc_S_idle").and_then(|v| v.as_bool().ok()) == Some(true)
                && sim.var("call1").and_then(|v| v.as_bool().ok()) == Some(false)
            {
                break;
            }
        }
        assert_eq!(sim.var("call1").and_then(|v| v.as_bool().ok()), Some(false));
        assert!(sim.var("__sfc_S_idle").and_then(|v| v.as_bool().ok()).unwrap_or(false));
    }
}
