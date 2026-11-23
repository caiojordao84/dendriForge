//! Pratt expression parser (`01-asl-ir-and-project-schema.md` §4.2).

use crate::lexer::{LexError, Token, TokenKind, tokenize};
use dendri_ast::{BinaryOp, CallArg, Expr, ExprKind, LValue, Literal, UnaryOp};
use dendri_span::Span;
use thiserror::Error;

/// Errors returned while parsing an ASL v2 expression or `LValue`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExprError {
    #[error(transparent)]
    Lex(#[from] LexError),
    #[error("unexpected end of expression at {span}")]
    UnexpectedEof { span: Span },
    #[error("unexpected token `{found}` at {span} (expected {expected})")]
    UnexpectedToken {
        found: String,
        expected: &'static str,
        span: Span,
    },
    #[error("chained comparison operators are forbidden in ASL v2 at {span}")]
    ChainedComparison { span: Span },
    #[error("FOREIGN(...) requires exactly two string literal arguments at {span}")]
    InvalidForeignSyntax { span: Span },
    #[error("expression at {span} is not a valid assignable LValue")]
    InvalidLValue { span: Span },
    #[error("maximum expression nesting depth exceeded at {span}")]
    MaxDepthExceeded { span: Span },
}

const MAX_EXPR_DEPTH: usize = 128;

/// Parses a complete canonical ASL v2 expression string into a spanned [`Expr`].
pub fn parse_expr(src: &str) -> Result<Expr, ExprError> {
    let tokens = tokenize(src)?;
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_bp(0, 0)?;
    if parser.peek_kind() != &TokenKind::Eof {
        let tok = parser.peek();
        return Err(ExprError::UnexpectedToken {
            found: format!("{:?}", tok.kind),
            expected: "end of expression",
            span: tok.span,
        });
    }
    Ok(expr)
}

/// Parses an assignable `LValue` (`x`, `arr[i]`, `s.field`, `motors[i + 1].speed`).
pub fn parse_lvalue(src: &str) -> Result<LValue, ExprError> {
    let expr = parse_expr(src)?;
    expr_to_lvalue(expr)
}

fn expr_to_lvalue(expr: Expr) -> Result<LValue, ExprError> {
    let span = expr.span;
    match expr.kind {
        ExprKind::Ident(name) => Ok(LValue::Var { name, span }),
        ExprKind::Index { base, index } => Ok(LValue::Index {
            base: Box::new(expr_to_lvalue(*base)?),
            index,
            span,
        }),
        ExprKind::Field { base, field } => Ok(LValue::Field {
            base: Box::new(expr_to_lvalue(*base)?),
            field,
            span,
        }),
        ExprKind::Literal(_)
        | ExprKind::Call { .. }
        | ExprKind::Unary { .. }
        | ExprKind::Binary { .. }
        | ExprKind::Foreign { .. } => Err(ExprError::InvalidLValue { span }),
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    const fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        static EOF: Token = Token {
            kind: TokenKind::Eof,
            span: Span::point(0),
        };
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .unwrap_or(&EOF)
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn peek_next_kind(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos.saturating_add(1)).map(|t| &t.kind)
    }

    fn bump(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos = self.pos.saturating_add(1);
        }
        tok
    }

    fn expect(
        &mut self,
        expected_kind: &TokenKind,
        expected_desc: &'static str,
    ) -> Result<Token, ExprError> {
        let tok = self.peek().clone();
        if &tok.kind == expected_kind {
            self.bump();
            Ok(tok)
        } else if tok.kind == TokenKind::Eof {
            Err(ExprError::UnexpectedEof { span: tok.span })
        } else {
            Err(ExprError::UnexpectedToken {
                found: format!("{:?}", tok.kind),
                expected: expected_desc,
                span: tok.span,
            })
        }
    }

    /// Pratt binding powers matching `01-asl-ir-and-project-schema.md` §4.2 (High -> Low):
    /// - 11: `OR`          (10, 11)
    /// - 10: `XOR`         (20, 21)
    /// -  9: `AND`         (30, 31)
    /// -  8: Bitwise       (40, 41)
    /// -  7: `==`, `!=`    (50, 51) [non-chaining]
    /// -  6: `<`, `<=`, `>`, `>=` (60, 61) [non-chaining]
    /// -  5: `SHL`, `SHR`  (70, 71)
    /// -  4: `+`, `-`      (80, 81)
    /// -  3: `*`, `/`, `MOD` (90, 91)
    /// -  2: Unary prefix  (100)
    /// -  1: Postfix       (110)
    fn parse_bp(&mut self, min_bp: u8, depth: usize) -> Result<Expr, ExprError> {
        if depth > MAX_EXPR_DEPTH {
            return Err(ExprError::MaxDepthExceeded {
                span: self.peek().span,
            });
        }

        let mut lhs = self.parse_prefix(depth)?;

        loop {
            // Postfix operators (Precedence 1): `[idx]`, `.field`
            if self.peek_kind() == &TokenKind::LBracket {
                self.bump();
                let index = self.parse_bp(0, depth.saturating_add(1))?;
                let close = self.expect(&TokenKind::RBracket, "`]`")?;
                let span = lhs.span.merge(close.span);
                lhs = Expr::new(
                    ExprKind::Index {
                        base: Box::new(lhs),
                        index: Box::new(index),
                    },
                    span,
                );
                continue;
            }
            if self.peek_kind() == &TokenKind::Dot {
                self.bump();
                let field_tok = self.bump();
                let TokenKind::Ident(field) = field_tok.kind else {
                    return Err(ExprError::UnexpectedToken {
                        found: format!("{:?}", field_tok.kind),
                        expected: "field identifier",
                        span: field_tok.span,
                    });
                };
                let span = lhs.span.merge(field_tok.span);
                lhs = Expr::new(
                    ExprKind::Field {
                        base: Box::new(lhs),
                        field,
                    },
                    span,
                );
                continue;
            }

            let Some((op, l_bp, r_bp)) = infix_binding_power(self.peek_kind()) else {
                break;
            };
            if l_bp < min_bp {
                break;
            }

            let op_tok = self.bump();
            let rhs = self.parse_bp(r_bp, depth.saturating_add(1))?;

            // Enforce non-chaining relational & equality rules (`01` §4.2)
            if op.is_relational()
                && let Some((next_op, _, _)) = infix_binding_power(self.peek_kind())
                && next_op.is_relational()
            {
                return Err(ExprError::ChainedComparison {
                    span: self.peek().span,
                });
            }
            if op.is_equality()
                && let Some((next_op, _, _)) = infix_binding_power(self.peek_kind())
                && next_op.is_equality()
            {
                return Err(ExprError::ChainedComparison {
                    span: self.peek().span,
                });
            }

            let span = lhs.span.merge(op_tok.span).merge(rhs.span);
            lhs = Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                },
                span,
            );
        }

        Ok(lhs)
    }

    fn parse_prefix(&mut self, depth: usize) -> Result<Expr, ExprError> {
        let tok = self.bump();
        match tok.kind {
            TokenKind::Bool(b) => Ok(Expr::new(ExprKind::Literal(Literal::Bool(b)), tok.span)),
            TokenKind::Int(n) => Ok(Expr::new(ExprKind::Literal(Literal::Int(n)), tok.span)),
            TokenKind::Float(f) => Ok(Expr::new(ExprKind::Literal(Literal::Float(f)), tok.span)),
            TokenKind::TimeUs(us) => {
                Ok(Expr::new(ExprKind::Literal(Literal::TimeUs(us)), tok.span))
            }
            TokenKind::String(s) => Ok(Expr::new(ExprKind::Literal(Literal::String(s)), tok.span)),

            TokenKind::Ident(name) => {
                if self.peek_kind() == &TokenKind::LParen {
                    self.bump(); // '('
                    let mut args = Vec::new();
                    if self.peek_kind() != &TokenKind::RParen {
                        loop {
                            let arg = self.parse_call_arg(depth.saturating_add(1))?;
                            args.push(arg);
                            if self.peek_kind() == &TokenKind::Comma {
                                self.bump();
                            } else {
                                break;
                            }
                        }
                    }
                    let close = self.expect(&TokenKind::RParen, "`)`")?;
                    Ok(Expr::new(
                        ExprKind::Call { callee: name, args },
                        tok.span.merge(close.span),
                    ))
                } else {
                    Ok(Expr::new(ExprKind::Ident(name), tok.span))
                }
            }

            TokenKind::Foreign => {
                self.expect(&TokenKind::LParen, "`(`")?;
                let lang_tok = self.bump();
                let TokenKind::String(language) = lang_tok.kind else {
                    return Err(ExprError::InvalidForeignSyntax {
                        span: tok.span.merge(lang_tok.span),
                    });
                };
                self.expect(&TokenKind::Comma, "`,`")?;
                let text_tok = self.bump();
                let TokenKind::String(text) = text_tok.kind else {
                    return Err(ExprError::InvalidForeignSyntax {
                        span: tok.span.merge(text_tok.span),
                    });
                };
                let close = self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Expr::new(
                    ExprKind::Foreign { language, text },
                    tok.span.merge(close.span),
                ))
            }

            TokenKind::LParen => {
                let inner = self.parse_bp(0, depth.saturating_add(1))?;
                let close = self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Expr::new(inner.kind, tok.span.merge(close.span)))
            }

            TokenKind::Minus => {
                let operand = self.parse_bp(100, depth.saturating_add(1))?;
                let span = tok.span.merge(operand.span);
                Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            TokenKind::Plus => {
                let operand = self.parse_bp(100, depth.saturating_add(1))?;
                let span = tok.span.merge(operand.span);
                Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Pos,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            TokenKind::Not => {
                let operand = self.parse_bp(100, depth.saturating_add(1))?;
                let span = tok.span.merge(operand.span);
                Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }
            TokenKind::BitNot => {
                let operand = self.parse_bp(100, depth.saturating_add(1))?;
                let span = tok.span.merge(operand.span);
                Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::BitNot,
                        operand: Box::new(operand),
                    },
                    span,
                ))
            }

            TokenKind::Eof => Err(ExprError::UnexpectedEof { span: tok.span }),

            TokenKind::Mod
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::BitAnd
            | TokenKind::BitXor
            | TokenKind::BitOr
            | TokenKind::And
            | TokenKind::Xor
            | TokenKind::Or
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::ColonEq
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::EqEq
            | TokenKind::BangEq => Err(ExprError::UnexpectedToken {
                found: format!("{:?}", tok.kind),
                expected: "expression",
                span: tok.span,
            }),
        }
    }

    fn parse_call_arg(&mut self, depth: usize) -> Result<CallArg, ExprError> {
        // Lookahead for IEC named parameter `IDENT := expr`
        if let TokenKind::Ident(_) = self.peek_kind()
            && self.peek_next_kind() == Some(&TokenKind::ColonEq)
        {
            let name_tok = self.bump();
            let TokenKind::Ident(name) = name_tok.kind else {
                return Err(ExprError::UnexpectedToken {
                    found: format!("{:?}", name_tok.kind),
                    expected: "argument name",
                    span: name_tok.span,
                });
            };
            self.bump(); // ':='
            let value = self.parse_bp(0, depth)?;
            let span = name_tok.span.merge(value.span);
            return Ok(CallArg {
                name: Some(name),
                value,
                span,
            });
        }
        let value = self.parse_bp(0, depth)?;
        let span = value.span;
        Ok(CallArg {
            name: None,
            value,
            span,
        })
    }
}

const fn infix_binding_power(kind: &TokenKind) -> Option<(BinaryOp, u8, u8)> {
    match kind {
        TokenKind::Or => Some((BinaryOp::Or, 10, 11)),
        TokenKind::Xor => Some((BinaryOp::Xor, 20, 21)),
        TokenKind::And => Some((BinaryOp::And, 30, 31)),
        TokenKind::BitAnd => Some((BinaryOp::BitAnd, 40, 41)),
        TokenKind::BitXor => Some((BinaryOp::BitXor, 40, 41)),
        TokenKind::BitOr => Some((BinaryOp::BitOr, 40, 41)),
        TokenKind::EqEq => Some((BinaryOp::Eq, 50, 51)),
        TokenKind::BangEq => Some((BinaryOp::Ne, 50, 51)),
        TokenKind::Lt => Some((BinaryOp::Lt, 60, 61)),
        TokenKind::Le => Some((BinaryOp::Le, 60, 61)),
        TokenKind::Gt => Some((BinaryOp::Gt, 60, 61)),
        TokenKind::Ge => Some((BinaryOp::Ge, 60, 61)),
        TokenKind::Shl => Some((BinaryOp::Shl, 70, 71)),
        TokenKind::Shr => Some((BinaryOp::Shr, 70, 71)),
        TokenKind::Plus => Some((BinaryOp::Add, 80, 81)),
        TokenKind::Minus => Some((BinaryOp::Sub, 80, 81)),
        TokenKind::Star => Some((BinaryOp::Mul, 90, 91)),
        TokenKind::Slash => Some((BinaryOp::Div, 90, 91)),
        TokenKind::Mod => Some((BinaryOp::Mod, 90, 91)),
        TokenKind::Bool(_)
        | TokenKind::Int(_)
        | TokenKind::Float(_)
        | TokenKind::TimeUs(_)
        | TokenKind::String(_)
        | TokenKind::Ident(_)
        | TokenKind::Foreign
        | TokenKind::Not
        | TokenKind::BitNot
        | TokenKind::LParen
        | TokenKind::RParen
        | TokenKind::LBracket
        | TokenKind::RBracket
        | TokenKind::Comma
        | TokenKind::Dot
        | TokenKind::ColonEq
        | TokenKind::Eof => None,
    }
}
