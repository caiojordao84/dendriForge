//! MicroPython ↔ ASL AST frontend (priority-2 sim opcode set).
//!
//! Supported surface:
//! - `from machine import Pin[, PWM, ADC, UART, I2C, SPI]` / `from time import sleep_ms`
//! - pin constants + `*_pin` / `*_pwm` / `*_adc` objects
//! - `ASL_HW` / `ASL_DATA` comments for label + type round-trip
//! - `def setup():` / `def loop():` plus entry `setup(); while True: loop()`
//! - `.value(0|1)` / `PWM`/`duty_u16` / `ADC`/`read_u16` / `uart_0.write` / `sleep_ms`
//! - IEC FBs as calls: `TON`/`TOF`/`CTU`/`R_TRIG`/…
//! - `CFG_I2C` / `CFG_SPI` ASL call form (constructors accepted on parse)
//!
//! Not supported yet: classes, async, ISR decorators,
//! full I2C/SPI transfer APIs (DEN-12+).

mod emit;
mod parse;

pub use emit::emit_micropython;
pub use parse::{parse_micropython, MpyParseError};

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::expr::Expr;
    use dendri_ast::{
        DataEntry, DataType, HardwareEntry, HwType, IoMode, Metadata, Program, Scope, Stmt,
    };

    fn blink_program() -> Program {
        Program {
            metadata: Metadata {
                asl_version: Some(dendri_ast::ASL_VERSION.into()),
                project_name: Some("S01-blink-set-bool-wait".into()),
                editor: Some("dendriForge".into()),
                author: None,
                notes: None,
                expect_error: None,
            },
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
                    args: vec![Expr::Var { name: "ledPin".into() }],
                },
                Stmt::Call {
                    name: "SET_BOOL".into(),
                    args: vec![Expr::Var { name: "ledPin".into() }, Expr::Bool { value: false }],
                },
            ],
            loop_body: vec![
                Stmt::Call {
                    name: "SET_BOOL".into(),
                    args: vec![Expr::Var { name: "ledPin".into() }, Expr::Bool { value: true }],
                },
                Stmt::Call {
                    name: "WAIT_TIME".into(),
                    args: vec![Expr::Var {
                        name: "ON_TIME".into(),
                    }],
                },
                Stmt::Call {
                    name: "SET_BOOL".into(),
                    args: vec![Expr::Var { name: "ledPin".into() }, Expr::Bool { value: false }],
                },
                Stmt::Call {
                    name: "WAIT_TIME".into(),
                    args: vec![Expr::Var {
                        name: "OFF_TIME".into(),
                    }],
                },
            ],
            functions: vec![],
            ladder: vec![],
            grafcet: vec![],
        }
    }

    #[test]
    fn emit_parse_roundtrip_blink() {
        let p = blink_program();
        let text = emit_micropython(&p);
        let again = parse_micropython(&text).expect("parse emitted MicroPython");
        assert_eq!(again.setup, p.setup, "setup mismatch; emitted:\n{text}");
        assert_eq!(again.loop_body, p.loop_body, "loop mismatch; emitted:\n{text}");
        assert_eq!(again.data, p.data);
        assert_eq!(again.hardware, p.hardware);
    }
}
