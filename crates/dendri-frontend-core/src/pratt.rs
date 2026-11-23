//! Reusable, table-driven Pratt expression parser engine (`04` §1.2).
//!
//! Parameterized by operator precedence, associativity (including strict
//! non-chaining for relational/equality operators), prefix/postfix hooks,
//! and bounded recursion depth to protect all language frontends against
//! stack-overflow fuzz inputs.

/// Associativity rule for an infix operator in the Pratt precedence table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Associativity {
    /// Left-associative: `a + b + c` parses as `(a + b) + c`.
    Left,
    /// Right-associative: `a ** b ** c` parses as `a ** (b ** c)`.
    Right,
    /// Non-associative (non-chaining): `a < b < c` triggers [`PrattDriver::error_non_assoc_chain`].
    NonAssoc,
}

/// Precedence specification for a prefix unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefixSpec<Op> {
    pub op: Op,
    /// Right binding power applied to the unary operand.
    pub right_bp: u8,
}

/// Precedence specification for an infix binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InfixSpec<Op> {
    pub op: Op,
    /// Relative precedence level (`1..=120`; higher binds tighter).
    pub precedence: u8,
    pub assoc: Associativity,
}

impl<Op> InfixSpec<Op> {
    /// Converts `(precedence, assoc)` into Pratt `(left_bp, right_bp)` pairs.
    #[must_use]
    pub const fn binding_power(&self) -> (u8, u8) {
        let base = self.precedence.saturating_mul(2);
        match self.assoc {
            Associativity::Left | Associativity::NonAssoc => (base, base.saturating_add(1)),
            Associativity::Right => (base.saturating_add(1), base),
        }
    }
}

/// Default recursion guard depth for Pratt expression parsing.
pub const DEFAULT_PRATT_MAX_DEPTH: usize = 128;

/// Language-specific driver supplied to [`parse_pratt`] / [`parse_pratt_with_limit`].
pub trait PrattDriver {
    type Expr;
    type PrefixOp: Copy;
    type InfixOp: Copy;
    type Error;

    /// Inspects the next token for a prefix operator without consuming it.
    fn peek_prefix(&mut self) -> Option<PrefixSpec<Self::PrefixOp>>;

    /// Consumes the peeked prefix operator.
    fn consume_prefix(&mut self) -> Result<Self::PrefixOp, Self::Error>;

    /// Parses a primary expression (literal, identifier, parenthesized sub-expression, etc.).
    fn parse_primary(&mut self) -> Result<Self::Expr, Self::Error>;

    /// Parses zero or more postfix continuations (`a[i]`, `s.field`, `fn(args)`).
    fn parse_postfix(&mut self, expr: Self::Expr) -> Result<Self::Expr, Self::Error> {
        Ok(expr)
    }

    /// Inspects the next token for an infix operator without consuming it.
    fn peek_infix(&mut self) -> Option<InfixSpec<Self::InfixOp>>;

    /// Consumes the peeked infix operator.
    fn consume_infix(&mut self) -> Result<Self::InfixOp, Self::Error>;

    /// Constructs a unary expression node.
    fn build_prefix(
        &mut self,
        op: Self::PrefixOp,
        operand: Self::Expr,
    ) -> Result<Self::Expr, Self::Error>;

    /// Constructs a binary expression node.
    fn build_infix(
        &mut self,
        op: Self::InfixOp,
        left: Self::Expr,
        right: Self::Expr,
    ) -> Result<Self::Expr, Self::Error>;

    /// Produces a language-specific error when two [`Associativity::NonAssoc`] operators
    /// of equal precedence are chained (e.g. `a < b < c`).
    fn error_non_assoc_chain(&mut self, prev: Self::InfixOp, next: Self::InfixOp) -> Self::Error;

    /// Produces a language-specific error when expression nesting exceeds `max_depth`.
    fn error_recursion_limit(&mut self, max_depth: usize) -> Self::Error;
}

/// Parses an expression using the default recursion depth limit ([`DEFAULT_PRATT_MAX_DEPTH`]).
pub fn parse_pratt<D: PrattDriver>(driver: &mut D) -> Result<D::Expr, D::Error> {
    parse_pratt_with_limit(driver, DEFAULT_PRATT_MAX_DEPTH)
}

/// Parses an expression with an explicit recursion depth bound `max_depth`.
pub fn parse_pratt_with_limit<D: PrattDriver>(
    driver: &mut D,
    max_depth: usize,
) -> Result<D::Expr, D::Error> {
    parse_bp(driver, 0, 0, max_depth)
}

fn parse_bp<D: PrattDriver>(
    driver: &mut D,
    min_bp: u8,
    depth: usize,
    max_depth: usize,
) -> Result<D::Expr, D::Error> {
    if depth >= max_depth {
        return Err(driver.error_recursion_limit(max_depth));
    }

    let mut lhs = if let Some(prefix) = driver.peek_prefix() {
        let op = driver.consume_prefix()?;
        let rhs = parse_bp(driver, prefix.right_bp, depth.saturating_add(1), max_depth)?;
        driver.build_prefix(op, rhs)?
    } else {
        let primary = driver.parse_primary()?;
        driver.parse_postfix(primary)?
    };

    let mut prev_non_assoc: Option<(u8, D::InfixOp)> = None;

    while let Some(spec) = driver.peek_infix() {
        let (l_bp, r_bp) = spec.binding_power();
        if l_bp < min_bp {
            break;
        }
        if spec.assoc == Associativity::NonAssoc
            && let Some((prev_prec, prev_op)) = prev_non_assoc
            && prev_prec == spec.precedence
        {
            return Err(driver.error_non_assoc_chain(prev_op, spec.op));
        }

        let op = driver.consume_infix()?;
        let rhs = parse_bp(driver, r_bp, depth.saturating_add(1), max_depth)?;
        lhs = driver.build_infix(op, lhs, rhs)?;

        prev_non_assoc = if spec.assoc == Associativity::NonAssoc {
            Some((spec.precedence, op))
        } else {
            None
        };
    }

    Ok(lhs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Op {
        Add,
        Mul,
        Lt,
        Eq,
        Neg,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Tok {
        Num(i64),
        Op(Op),
    }

    struct MiniDriver {
        tokens: Vec<Tok>,
        pos: usize,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum MiniErr {
        UnexpectedEof,
        UnexpectedToken,
        Chained(Op, Op),
        Depth(usize),
    }

    impl PrattDriver for MiniDriver {
        type Expr = String;
        type PrefixOp = Op;
        type InfixOp = Op;
        type Error = MiniErr;

        fn peek_prefix(&mut self) -> Option<PrefixSpec<Op>> {
            match self.tokens.get(self.pos).copied() {
                Some(Tok::Op(Op::Neg)) => Some(PrefixSpec {
                    op: Op::Neg,
                    right_bp: 100,
                }),
                Some(Tok::Num(_) | Tok::Op(Op::Add | Op::Mul | Op::Lt | Op::Eq)) | None => None,
            }
        }

        fn consume_prefix(&mut self) -> Result<Op, MiniErr> {
            match self.tokens.get(self.pos).copied() {
                Some(Tok::Op(op)) => {
                    self.pos += 1;
                    Ok(op)
                }
                Some(Tok::Num(_)) => Err(MiniErr::UnexpectedToken),
                None => Err(MiniErr::UnexpectedEof),
            }
        }

        fn parse_primary(&mut self) -> Result<String, MiniErr> {
            match self.tokens.get(self.pos).copied() {
                Some(Tok::Num(n)) => {
                    self.pos += 1;
                    Ok(n.to_string())
                }
                Some(Tok::Op(_)) => Err(MiniErr::UnexpectedToken),
                None => Err(MiniErr::UnexpectedEof),
            }
        }

        fn peek_infix(&mut self) -> Option<InfixSpec<Op>> {
            let op = match self.tokens.get(self.pos).copied() {
                Some(Tok::Op(op)) => op,
                Some(Tok::Num(_)) | None => return None,
            };
            match op {
                Op::Eq => Some(InfixSpec {
                    op,
                    precedence: 2,
                    assoc: Associativity::NonAssoc,
                }),
                Op::Lt => Some(InfixSpec {
                    op,
                    precedence: 3,
                    assoc: Associativity::NonAssoc,
                }),
                Op::Add => Some(InfixSpec {
                    op,
                    precedence: 4,
                    assoc: Associativity::Left,
                }),
                Op::Mul => Some(InfixSpec {
                    op,
                    precedence: 5,
                    assoc: Associativity::Left,
                }),
                Op::Neg => None,
            }
        }

        fn consume_infix(&mut self) -> Result<Op, MiniErr> {
            self.consume_prefix()
        }

        fn build_prefix(&mut self, _op: Op, operand: String) -> Result<String, MiniErr> {
            Ok(format!("(-{operand})"))
        }

        fn build_infix(&mut self, op: Op, left: String, right: String) -> Result<String, MiniErr> {
            let sym = match op {
                Op::Add => "+",
                Op::Mul => "*",
                Op::Lt => "<",
                Op::Eq => "==",
                Op::Neg => "-",
            };
            Ok(format!("({left} {sym} {right})"))
        }

        fn error_non_assoc_chain(&mut self, prev: Op, next: Op) -> MiniErr {
            MiniErr::Chained(prev, next)
        }

        fn error_recursion_limit(&mut self, max_depth: usize) -> MiniErr {
            MiniErr::Depth(max_depth)
        }
    }

    #[test]
    fn pratt_respects_precedence_and_rejects_non_assoc_chains() {
        // 1 + 2 * -3 == 4 < 5  =>  ((1 + (2 * (-3))) == (4 < 5))
        let mut d = MiniDriver {
            tokens: vec![
                Tok::Num(1),
                Tok::Op(Op::Add),
                Tok::Num(2),
                Tok::Op(Op::Mul),
                Tok::Op(Op::Neg),
                Tok::Num(3),
                Tok::Op(Op::Eq),
                Tok::Num(4),
                Tok::Op(Op::Lt),
                Tok::Num(5),
            ],
            pos: 0,
        };
        assert_eq!(
            parse_pratt(&mut d).unwrap(),
            "((1 + (2 * (-3))) == (4 < 5))"
        );

        // Chained `1 < 2 < 3` MUST be rejected
        let mut chained = MiniDriver {
            tokens: vec![
                Tok::Num(1),
                Tok::Op(Op::Lt),
                Tok::Num(2),
                Tok::Op(Op::Lt),
                Tok::Num(3),
            ],
            pos: 0,
        };
        assert_eq!(
            parse_pratt(&mut chained),
            Err(MiniErr::Chained(Op::Lt, Op::Lt))
        );
    }

    #[test]
    fn pratt_enforces_recursion_limit() {
        let mut tokens = vec![Tok::Op(Op::Neg); 16];
        tokens.push(Tok::Num(1));
        let mut d = MiniDriver { tokens, pos: 0 };
        assert_eq!(parse_pratt_with_limit(&mut d, 8), Err(MiniErr::Depth(8)));
    }
}
