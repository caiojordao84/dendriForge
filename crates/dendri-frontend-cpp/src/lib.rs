//! Arduino-style C++ ↔ ASL AST frontend (MVP subset).
//!
//! Supported surface (blink-class programs):
//! - `const int pinLabel = N;` hardware pins
//! - `const unsigned long NAME = N UL;` → ASL `TIME` consts (`T#Nms`)
//! - `bool` / `int` vars
//! - `void setup()` / `void loop()`
//! - `pinMode` / `digitalWrite` / `digitalRead` / `delay` / `millis`
//! - `if` / `else` / `while` / simple `for` / assign / call
//!
//! Not supported yet: classes, templates, pointers, `#include` semantics,
//! multi-file sketches, AVR/ESP HAL specifics beyond Arduino core aliases.

mod emit;
mod parse;

pub use emit::emit_cpp;
pub use parse::{parse_cpp, CppParseError};

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
            loop_body: vec![
                Stmt::Call {
                    name: "SET_BOOL".into(),
                    args: vec![Expr::Uint { value: 13 }, Expr::Bool { value: true }],
                },
                Stmt::Call {
                    name: "WAIT_TIME".into(),
                    args: vec![Expr::Var {
                        name: "ON_TIME".into(),
                    }],
                },
                Stmt::Call {
                    name: "SET_BOOL".into(),
                    args: vec![Expr::Uint { value: 13 }, Expr::Bool { value: false }],
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
        let text = emit_cpp(&p);
        let again = parse_cpp(&text).expect("parse emitted C++");
        assert_eq!(again.setup, p.setup);
        assert_eq!(again.loop_body, p.loop_body);
        assert_eq!(again.data, p.data);
        assert_eq!(again.hardware, p.hardware);
    }

    #[test]
    fn parse_handwritten_blink() {
        let src = r#"
// project: handmade
const int ledPin = 13;
const unsigned long ON_TIME = 100UL;
const unsigned long OFF_TIME = 100UL;

void setup() {
  pinMode(ledPin, OUTPUT);
  digitalWrite(ledPin, LOW);
}

void loop() {
  digitalWrite(ledPin, HIGH);
  delay(ON_TIME);
  digitalWrite(ledPin, LOW);
  delay(OFF_TIME);
}
"#;
        let p = parse_cpp(src).expect("parse");
        assert_eq!(p.setup.len(), 2);
        assert_eq!(p.loop_body.len(), 4);
        assert!(matches!(
            &p.setup[0],
            Stmt::Call { name, args } if name == "CFG_DIGITAL_OUTPUT"
                && args == &vec![Expr::Uint { value: 13 }]
        ));
    }
}
