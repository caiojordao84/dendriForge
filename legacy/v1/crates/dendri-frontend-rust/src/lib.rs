//! Dendri HAL Rust subset ↔ ASL AST frontend (P2 user scripting).
//!
//! Supported surface (first slice — coexists with P1 langs; ASL remains IR):
//! - `const name: u32 = N;` hardware pins (`// ASL_HW:` for DI/DO + non-`*Pin` labels)
//! - `const` / `let mut` with `// ASL_DATA:` for TIME/UINT/INT/BOOL/REAL honesty
//! - `fn setup()` / `fn loop_()` / user `fn name(...) -> bool`
//! - `pin_mode` / `digital_write` / `digital_read` / `delay_ms` / `millis`
//! - `analog_write` / `analog_read` / `serial_*` / `CFG_PWM_*` / `CFG_ADC_*`
//! - `CFG_I2C` / `CFG_SPI` (ASL call form)
//! - IEC FBs as calls: `TON` / `TOF` / `TP` / `CTU` / `CTD` / `CTUD` / `R_TRIG` / `F_TRIG`
//! - `if` / `else if` / `else` / `while` / `for x in 0..N` / assign / call
//!
//! Not supported yet: crates/modules, traits, async/Embassy runtime, full HAL,
//! macros, generics, or a second execution engine (no Rhai — sim runs ASL).

mod emit;
mod parse;

pub use emit::emit_rust;
pub use parse::{parse_rust, RustParseError};

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
        let text = emit_rust(&p);
        let again = parse_rust(&text).expect("parse emitted Rust");
        assert_eq!(again.setup, p.setup);
        assert_eq!(again.loop_body, p.loop_body);
        assert_eq!(again.data, p.data);
        assert_eq!(again.hardware, p.hardware);
    }

    #[test]
    fn parse_handwritten_blink() {
        let src = r#"
// project: handmade
const ledPin: u32 = 13;
const ON_TIME: u32 = 100;
const OFF_TIME: u32 = 100;

fn setup() {
  pin_mode(ledPin, OUTPUT);
  digital_write(ledPin, LOW);
}

fn loop_() {
  digital_write(ledPin, HIGH);
  delay_ms(ON_TIME);
  digital_write(ledPin, LOW);
  delay_ms(OFF_TIME);
}
"#;
        let p = parse_rust(src).expect("parse");
        assert_eq!(p.setup.len(), 2);
        assert_eq!(p.loop_body.len(), 4);
        assert!(matches!(
            &p.setup[0],
            Stmt::Call { name, args } if name == "CFG_DIGITAL_OUTPUT"
                && args == &vec![Expr::Var { name: "ledPin".into() }]
        ));
    }
}
