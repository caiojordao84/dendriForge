//! Official Pratt expression parser and canonical printer for ASL v2
//! (`01-asl-ir-and-project-schema.md` §4.2, ADR-007).
//!
//! Guarantee: for every canonical expression `e`,
//! `parse_expr(&print_expr(&e)) == Ok(e)` (modulo byte spans), verified by `proptest`.

pub mod lexer;
pub mod parser;
pub mod printer;

pub use parser::{ExprError, parse_expr, parse_lvalue};
pub use printer::{print_expr, print_lvalue};

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{BinaryOp, CallArg, Expr, ExprKind, LValue, Literal, UnaryOp};
    use dendri_span::Span;
    use proptest::prelude::*;

    fn ident_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex("[a-z][a-z0-9_]{0,10}")
            .unwrap()
            .prop_filter("not a keyword", |s| !lexer::is_reserved_keyword(s))
    }

    fn string_literal_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex("[a-zA-Z0-9 _.,:/()\\n\\t\"\\\\çãé]{0,20}").unwrap()
    }

    fn leaf_expr_strategy() -> impl Strategy<Value = Expr> {
        prop_oneof![
            any::<bool>()
                .prop_map(|b| Expr::new(ExprKind::Literal(Literal::Bool(b)), Span::default())),
            (0i128..=1_000_000_000i128)
                .prop_map(|n| Expr::new(ExprKind::Literal(Literal::Int(n)), Span::default())),
            (-10_000i32..=10_000i32).prop_map(|n| {
                // Use exact binary-clean tenths/quarters so decimal f64 round-trip is exact
                let f = f64::from(n) / 4.0;
                Expr::new(ExprKind::Literal(Literal::Float(f.abs())), Span::default())
            }),
            (0i64..=10_000_000i64)
                .prop_map(|us| Expr::new(ExprKind::Literal(Literal::TimeUs(us)), Span::default())),
            string_literal_strategy()
                .prop_map(|s| Expr::new(ExprKind::Literal(Literal::String(s)), Span::default())),
            ident_strategy().prop_map(|id| Expr::new(ExprKind::Ident(id), Span::default())),
            (ident_strategy(), string_literal_strategy()).prop_map(|(lang, text)| {
                Expr::new(
                    ExprKind::Foreign {
                        language: lang,
                        text,
                    },
                    Span::default(),
                )
            }),
        ]
    }

    fn expr_strategy() -> impl Strategy<Value = Expr> {
        leaf_expr_strategy().prop_recursive(4, 32, 4, |inner| {
            prop_oneof![
                // Unary ops
                (
                    prop_oneof![
                        Just(UnaryOp::Neg),
                        Just(UnaryOp::Pos),
                        Just(UnaryOp::Not),
                        Just(UnaryOp::BitNot),
                    ],
                    inner.clone(),
                )
                    .prop_map(|(op, operand)| Expr::new(
                        ExprKind::Unary {
                            op,
                            operand: Box::new(operand),
                        },
                        Span::default()
                    )),
                // Binary ops (all 19 variants!)
                (
                    prop_oneof![
                        Just(BinaryOp::Mul),
                        Just(BinaryOp::Div),
                        Just(BinaryOp::Mod),
                        Just(BinaryOp::Add),
                        Just(BinaryOp::Sub),
                        Just(BinaryOp::Shl),
                        Just(BinaryOp::Shr),
                        Just(BinaryOp::Lt),
                        Just(BinaryOp::Le),
                        Just(BinaryOp::Gt),
                        Just(BinaryOp::Ge),
                        Just(BinaryOp::Eq),
                        Just(BinaryOp::Ne),
                        Just(BinaryOp::BitAnd),
                        Just(BinaryOp::BitXor),
                        Just(BinaryOp::BitOr),
                        Just(BinaryOp::And),
                        Just(BinaryOp::Xor),
                        Just(BinaryOp::Or),
                    ],
                    inner.clone(),
                    inner.clone(),
                )
                    .prop_map(|(op, left, right)| Expr::new(
                        ExprKind::Binary {
                            op,
                            left: Box::new(left),
                            right: Box::new(right),
                        },
                        Span::default()
                    )),
                // Indexing
                (inner.clone(), inner.clone()).prop_map(|(base, index)| Expr::new(
                    ExprKind::Index {
                        base: Box::new(base),
                        index: Box::new(index),
                    },
                    Span::default()
                )),
                // Field access
                (inner.clone(), ident_strategy()).prop_map(|(base, field)| Expr::new(
                    ExprKind::Field {
                        base: Box::new(base),
                        field,
                    },
                    Span::default()
                )),
                // Function calls (positional & named args)
                (
                    ident_strategy(),
                    prop::collection::vec((prop::option::of(ident_strategy()), inner), 0..=3),
                )
                    .prop_map(|(callee, args)| Expr::new(
                        ExprKind::Call {
                            callee,
                            args: args
                                .into_iter()
                                .map(|(name, value)| CallArg {
                                    name,
                                    value,
                                    span: Span::default(),
                                })
                                .collect(),
                        },
                        Span::default()
                    )),
            ]
        })
    }

    proptest! {
        #[test]
        fn canonical_print_then_parse_is_lossless_identity(expr in expr_strategy()) {
            let printed = print_expr(&expr);
            let reparsed = parse_expr(&printed)
                .unwrap_or_else(|err| panic!("failed to reparse `{printed}`: {err}"));
            prop_assert_eq!(reparsed.strip_spans(), expr.strip_spans(), "printed: {}", printed);
        }
    }

    #[test]
    fn precedence_relational_binds_tighter_than_equality() {
        // Fixes v1's universal `==` vs `<` bug: `a < b == c < d` MUST parse as `(a < b) == (c < d)`
        let parsed = parse_expr("a < b == c < d").unwrap().strip_spans();
        assert!(matches!(
            parsed.kind,
            ExprKind::Binary {
                op: BinaryOp::Eq,
                ..
            }
        ));
        assert_eq!(print_expr(&parsed), "a < b == c < d");
    }

    #[test]
    fn chained_comparisons_are_hard_syntax_errors() {
        assert!(matches!(
            parse_expr("a < b < c"),
            Err(ExprError::ChainedComparison { .. })
        ));
        assert!(matches!(
            parse_expr("a <= b > c"),
            Err(ExprError::ChainedComparison { .. })
        ));
        assert!(matches!(
            parse_expr("a == b == c"),
            Err(ExprError::ChainedComparison { .. })
        ));
        assert!(matches!(
            parse_expr("a != b == c"),
            Err(ExprError::ChainedComparison { .. })
        ));
    }

    #[test]
    fn parses_iec_radix_time_escapes_and_foreign() {
        assert_eq!(
            parse_expr("16#FF").unwrap().kind,
            ExprKind::Literal(Literal::Int(255))
        );
        assert_eq!(
            parse_expr("2#1010_0101").unwrap().kind,
            ExprKind::Literal(Literal::Int(0b1010_0101))
        );
        assert_eq!(
            parse_expr("T#1s500ms").unwrap().kind,
            ExprKind::Literal(Literal::TimeUs(1_500_000))
        );
        assert_eq!(
            parse_expr("T#500us").unwrap().kind,
            ExprKind::Literal(Literal::TimeUs(500))
        );
        assert_eq!(
            parse_expr(r#""olá\n\t\"\\\u{00E7}""#).unwrap().kind,
            ExprKind::Literal(Literal::String("olá\n\t\"\\ç".to_owned()))
        );
        assert_eq!(
            parse_expr(r#"FOREIGN("cpp", "servo.read()")"#)
                .unwrap()
                .kind,
            ExprKind::Foreign {
                language: "cpp".to_owned(),
                text: "servo.read()".to_owned(),
            }
        );
    }

    #[test]
    fn lvalue_round_trip_supports_nested_field_and_index() {
        let lv = parse_lvalue("motors[i + 1].speed").unwrap();
        assert_eq!(lv.root_name(), "motors");
        assert_eq!(print_lvalue(&lv), "motors[i + 1].speed");
        assert_eq!(
            parse_lvalue(&print_lvalue(&lv)).unwrap().strip_spans(),
            lv.strip_spans()
        );
        assert!(matches!(
            lv,
            LValue::Field { ref field, .. } if field == "speed"
        ));
    }
}
