//! IEC 61131-3 Structured Text ↔ ASL AST (MVP subset).
//!
//! Parses and generates ST against the same [`Program`] AST that `dendri-sim` runs.
//! See `docs/phase-1/frontends.md` for coverage limits.

mod emit;
mod parse;

pub use emit::emit_st;
pub use parse::{parse_st, StParseError};

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::expr::Expr;
    use dendri_ast::{Metadata, Program, Stmt};

    #[test]
    fn emits_set_bool() {
        let p = Program {
            metadata: Metadata {
                project_name: Some("stub".into()),
                editor: None,
                author: None,
                notes: None,
                expect_error: None,
            },
            hardware: vec![],
            data: vec![],
            setup: vec![Stmt::Call {
                name: "SET_BOOL".into(),
                args: vec![Expr::Uint { value: 13 }, Expr::Bool { value: true }],
            }],
            loop_body: vec![],
            functions: vec![],
            ladder: vec![],
            grafcet: vec![],
        };
        let st = emit_st(&p);
        assert!(st.contains("SET_BOOL(13, TRUE)"));
        assert!(st.contains("PROGRAM MAIN"));
        assert!(st.contains("SETUP"));
    }

    #[test]
    fn parse_emit_roundtrip_blink() {
        use dendri_ast::{DataEntry, DataType, HardwareEntry, HwType, IoMode, Scope};

        let p = Program {
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
        };

        let text = emit_st(&p);
        let again = parse_st(&text).expect("parse emitted ST");
        assert_eq!(again.setup, p.setup);
        assert_eq!(again.loop_body, p.loop_body);
        assert_eq!(again.data, p.data);
        assert_eq!(again.hardware, p.hardware);
    }
}
