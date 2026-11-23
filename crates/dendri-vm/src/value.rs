//! Strictly-typed runtime values, exact fixed-width wrapping/checked arithmetic,
//! and deterministic IEEE-754 + `libm` math (`01` §2.1, `02` §2.1).

use crate::fb::FbState;
use crate::{OverflowPolicy, RuntimeError};
use dendri_ast::{ASL_ANALOG_FULL_SCALE, BinaryOp, FbKind, ScalarType, TypeRef, UnaryOp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Strictly-typed ASL v2 runtime value (zero silent cross-sign mismatches).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Void,
    Bool(bool),
    Sint(i8),
    Int(i16),
    Dint(i32),
    Lint(i64),
    Usint(u8),
    Uint(u16),
    Udint(u32),
    Ulint(u64),
    Byte(u8),
    Word(u16),
    Dword(u32),
    Lword(u64),
    Real(f32),
    Lreal(f64),
    TimeUs(i64),
    String { text: String, max_len: u32 },
    Analog(i32),
    Duty(u32),
    Code { value: i64, codec_id: String },
    Array(Vec<Value>),
    Struct(BTreeMap<String, Value>),
    Fb(FbState),
}

impl Value {
    /// Deterministic zero/default value for any ASL [`TypeRef`].
    #[must_use]
    pub fn default_for_type(
        ty: &TypeRef,
        struct_defs: &BTreeMap<String, BTreeMap<String, TypeRef>>,
        user_fb_fields: &BTreeMap<String, BTreeMap<String, TypeRef>>,
    ) -> Self {
        match ty {
            TypeRef::Void => Self::Void,
            TypeRef::Scalar { ty: s } => match s {
                ScalarType::Bool => Self::Bool(false),
                ScalarType::Sint => Self::Sint(0),
                ScalarType::Int => Self::Int(0),
                ScalarType::Dint => Self::Dint(0),
                ScalarType::Lint => Self::Lint(0),
                ScalarType::Usint => Self::Usint(0),
                ScalarType::Uint => Self::Uint(0),
                ScalarType::Udint => Self::Udint(0),
                ScalarType::Ulint => Self::Ulint(0),
                ScalarType::Byte => Self::Byte(0),
                ScalarType::Word => Self::Word(0),
                ScalarType::Dword => Self::Dword(0),
                ScalarType::Lword => Self::Lword(0),
                ScalarType::Real => Self::Real(0.0),
                ScalarType::Lreal => Self::Lreal(0.0),
                ScalarType::Time => Self::TimeUs(0),
                ScalarType::String { max_len } => Self::String {
                    text: String::new(),
                    max_len: *max_len,
                },
                ScalarType::Analog => Self::Analog(0),
                ScalarType::Duty => Self::Duty(0),
                ScalarType::Code { codec_id } => Self::Code {
                    value: 0,
                    codec_id: codec_id.clone(),
                },
            },
            TypeRef::Array { elem, len } => {
                let count = usize::try_from(*len).unwrap_or(0);
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(Self::default_for_type(elem, struct_defs, user_fb_fields));
                }
                Self::Array(items)
            }
            TypeRef::Named { name } => {
                if let Some(fields) = struct_defs.get(name) {
                    let mut map = BTreeMap::new();
                    for (fname, fty) in fields {
                        map.insert(
                            fname.clone(),
                            Self::default_for_type(fty, struct_defs, user_fb_fields),
                        );
                    }
                    Self::Struct(map)
                } else {
                    // Enum default = 0 (DINT)
                    Self::Dint(0)
                }
            }
            TypeRef::Fb { fb_kind } => {
                if let FbKind::User { fb_name } = fb_kind {
                    let mut fields = BTreeMap::new();
                    if let Some(defs) = user_fb_fields.get(fb_name) {
                        for (fname, fty) in defs {
                            fields.insert(
                                fname.clone(),
                                Self::default_for_type(fty, struct_defs, user_fb_fields),
                            );
                        }
                    }
                    Self::Fb(FbState::user(fb_name.clone(), fields))
                } else {
                    Self::Fb(FbState::from_kind(fb_kind))
                }
            }
        }
    }

    /// Coerces an untyped integer literal `i128` into the target scalar [`TypeRef`], if matching.
    pub fn coerce_to_type(self, target: &TypeRef) -> Result<(Self, bool), RuntimeError> {
        let duty_max = u32::try_from(ASL_ANALOG_FULL_SCALE).unwrap_or(u32::MAX);
        let mut duty_clamped = false;

        let out = match (target, self) {
            (
                TypeRef::Scalar {
                    ty: ScalarType::Sint,
                },
                Self::Dint(v),
            ) => Self::Sint(v as i8),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Int,
                },
                Self::Dint(v),
            ) => Self::Int(v as i16),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Lint,
                },
                Self::Dint(v),
            ) => Self::Lint(i64::from(v)),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Usint,
                },
                Self::Dint(v),
            ) => Self::Usint(v as u8),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Uint,
                },
                Self::Dint(v),
            ) => Self::Uint(v as u16),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Udint,
                },
                Self::Dint(v),
            ) => Self::Udint(v as u32),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Ulint,
                },
                Self::Dint(v),
            ) => Self::Ulint(v as u64),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Byte,
                },
                Self::Dint(v),
            ) => Self::Byte(v as u8),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Word,
                },
                Self::Dint(v),
            ) => Self::Word(v as u16),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Dword,
                },
                Self::Dint(v),
            ) => Self::Dword(v as u32),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Lword,
                },
                Self::Dint(v),
            ) => Self::Lword(v as u64),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Real,
                },
                Self::Lreal(v),
            ) => Self::Real(v as f32),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Analog,
                },
                Self::Dint(v),
            ) => Self::Analog(v),
            (
                TypeRef::Scalar {
                    ty: ScalarType::Duty,
                },
                Self::Dint(v),
            ) => {
                if v < 0 {
                    duty_clamped = true;
                    Self::Duty(0)
                } else {
                    let u = v as u32;
                    if u > duty_max {
                        duty_clamped = true;
                        Self::Duty(duty_max)
                    } else {
                        Self::Duty(u)
                    }
                }
            }
            (
                TypeRef::Scalar {
                    ty: ScalarType::Duty,
                },
                Self::Lint(v),
            ) => {
                if v < 0 {
                    duty_clamped = true;
                    Self::Duty(0)
                } else if v > i64::from(duty_max) {
                    duty_clamped = true;
                    Self::Duty(duty_max)
                } else {
                    Self::Duty(u32::try_from(v).unwrap_or(duty_max))
                }
            }
            (
                TypeRef::Scalar {
                    ty: ScalarType::Duty,
                },
                Self::Duty(u),
            ) => {
                if u > duty_max {
                    duty_clamped = true;
                    Self::Duty(duty_max)
                } else {
                    Self::Duty(u)
                }
            }
            (
                TypeRef::Scalar {
                    ty: ScalarType::String { max_len },
                },
                Self::String { text, .. },
            ) => Self::String {
                text: truncate_utf8_bytes(&text, usize::try_from(*max_len).unwrap_or(255)),
                max_len: *max_len,
            },
            (_, val) => val,
        };
        Ok((out, duty_clamped))
    }

    /// Extracts boolean value or returns [`RuntimeError::TypeError`].
    pub fn as_bool(&self) -> Result<bool, RuntimeError> {
        if let Self::Bool(b) = self {
            Ok(*b)
        } else {
            Err(RuntimeError::TypeError("expected BOOL".to_owned()))
        }
    }

    /// Extracts integer value as `i128` (used for array indexing & `for` loops).
    pub fn as_i128(&self) -> Result<i128, RuntimeError> {
        match self {
            Self::Sint(v) => Ok(i128::from(*v)),
            Self::Int(v) => Ok(i128::from(*v)),
            Self::Dint(v) => Ok(i128::from(*v)),
            Self::Lint(v) => Ok(i128::from(*v)),
            Self::Usint(v) => Ok(i128::from(*v)),
            Self::Uint(v) => Ok(i128::from(*v)),
            Self::Udint(v) => Ok(i128::from(*v)),
            Self::Ulint(v) => Ok(i128::from(*v)),
            Self::Byte(v) => Ok(i128::from(*v)),
            Self::Word(v) => Ok(i128::from(*v)),
            Self::Dword(v) => Ok(i128::from(*v)),
            Self::Lword(v) => Ok(i128::from(*v)),
            Self::TimeUs(v) => Ok(i128::from(*v)),
            Self::Analog(v) => Ok(i128::from(*v)),
            Self::Duty(v) => Ok(i128::from(*v)),
            Self::Code { value, .. } => Ok(i128::from(*value)),
            Self::Void
            | Self::Bool(_)
            | Self::Real(_)
            | Self::Lreal(_)
            | Self::String { .. }
            | Self::Array(_)
            | Self::Struct(_)
            | Self::Fb(_) => Err(RuntimeError::TypeError("expected integer value".to_owned())),
        }
    }

    /// Extracts floating-point value as `f64`.
    pub fn as_f64(&self) -> Result<f64, RuntimeError> {
        if let Self::Real(v) = self {
            Ok(f64::from(*v))
        } else if let Self::Lreal(v) = self {
            Ok(*v)
        } else {
            Err(RuntimeError::TypeError("expected REAL/LREAL".to_owned()))
        }
    }

    /// Extracts `TIME` duration in microseconds.
    pub fn as_time_us(&self) -> Result<i64, RuntimeError> {
        if let Self::TimeUs(us) = self {
            Ok(*us)
        } else {
            Err(RuntimeError::TypeError("expected TIME".to_owned()))
        }
    }

    /// Extracts UTF-8 string reference.
    pub fn as_str(&self) -> Result<&str, RuntimeError> {
        if let Self::String { text, .. } = self {
            Ok(text.as_str())
        } else {
            Err(RuntimeError::TypeError("expected STRING".to_owned()))
        }
    }

    /// Reconstructs an integer value matching the concrete integer variant of `template`.
    pub fn from_i128_like(template: &Self, val: i128) -> Result<Self, RuntimeError> {
        match template {
            Self::Sint(_) => Ok(Self::Sint(val as i8)),
            Self::Int(_) => Ok(Self::Int(val as i16)),
            Self::Dint(_) => Ok(Self::Dint(val as i32)),
            Self::Lint(_) => Ok(Self::Lint(val as i64)),
            Self::Usint(_) => Ok(Self::Usint(val as u8)),
            Self::Uint(_) => Ok(Self::Uint(val as u16)),
            Self::Udint(_) => Ok(Self::Udint(val as u32)),
            Self::Ulint(_) => Ok(Self::Ulint(val as u64)),
            Self::Byte(_) => Ok(Self::Byte(val as u8)),
            Self::Word(_) => Ok(Self::Word(val as u16)),
            Self::Dword(_) => Ok(Self::Dword(val as u32)),
            Self::Lword(_) => Ok(Self::Lword(val as u64)),
            Self::Void
            | Self::Bool(_)
            | Self::Real(_)
            | Self::Lreal(_)
            | Self::TimeUs(_)
            | Self::String { .. }
            | Self::Analog(_)
            | Self::Duty(_)
            | Self::Code { .. }
            | Self::Array(_)
            | Self::Struct(_)
            | Self::Fb(_) => Err(RuntimeError::TypeError(
                "expected integer template".to_owned(),
            )),
        }
    }
}

fn truncate_utf8_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    s.get(..end).unwrap_or("").to_owned()
}

/// Evaluates a unary operator with exact fixed-width wrapping/checked semantics.
pub fn eval_unary(
    op: UnaryOp,
    val: Value,
    overflow: OverflowPolicy,
) -> Result<Value, RuntimeError> {
    match op {
        UnaryOp::Not => Ok(Value::Bool(!val.as_bool()?)),
        UnaryOp::Pos => Ok(val),
        UnaryOp::Neg => match val {
            Value::Sint(x) => neg_signed!(x, Value::Sint, overflow),
            Value::Int(x) => neg_signed!(x, Value::Int, overflow),
            Value::Dint(x) => neg_signed!(x, Value::Dint, overflow),
            Value::Lint(x) => neg_signed!(x, Value::Lint, overflow),
            Value::Real(x) => Ok(Value::Real(-x)),
            Value::Lreal(x) => Ok(Value::Lreal(-x)),
            Value::Analog(x) => neg_signed!(x, Value::Analog, overflow),
            Value::TimeUs(x) => neg_signed!(x, Value::TimeUs, overflow),
            Value::Void
            | Value::Bool(_)
            | Value::Usint(_)
            | Value::Uint(_)
            | Value::Udint(_)
            | Value::Ulint(_)
            | Value::Byte(_)
            | Value::Word(_)
            | Value::Dword(_)
            | Value::Lword(_)
            | Value::String { .. }
            | Value::Duty(_)
            | Value::Code { .. }
            | Value::Array(_)
            | Value::Struct(_)
            | Value::Fb(_) => Err(RuntimeError::TypeError(
                "invalid operand for unary `-`".to_owned(),
            )),
        },
        UnaryOp::BitNot => match val {
            Value::Sint(x) => Ok(Value::Sint(!x)),
            Value::Int(x) => Ok(Value::Int(!x)),
            Value::Dint(x) => Ok(Value::Dint(!x)),
            Value::Lint(x) => Ok(Value::Lint(!x)),
            Value::Usint(x) => Ok(Value::Usint(!x)),
            Value::Uint(x) => Ok(Value::Uint(!x)),
            Value::Udint(x) => Ok(Value::Udint(!x)),
            Value::Ulint(x) => Ok(Value::Ulint(!x)),
            Value::Byte(x) => Ok(Value::Byte(!x)),
            Value::Word(x) => Ok(Value::Word(!x)),
            Value::Dword(x) => Ok(Value::Dword(!x)),
            Value::Lword(x) => Ok(Value::Lword(!x)),
            Value::Void
            | Value::Bool(_)
            | Value::Real(_)
            | Value::Lreal(_)
            | Value::TimeUs(_)
            | Value::String { .. }
            | Value::Analog(_)
            | Value::Duty(_)
            | Value::Code { .. }
            | Value::Array(_)
            | Value::Struct(_)
            | Value::Fb(_) => Err(RuntimeError::TypeError(
                "invalid operand for BIT_NOT".to_owned(),
            )),
        },
    }
}

macro_rules! neg_signed {
    ($x:expr, $variant:path, $policy:expr) => {
        match $policy {
            OverflowPolicy::Wrap => Ok($variant($x.wrapping_neg())),
            OverflowPolicy::Trap => $x
                .checked_neg()
                .map($variant)
                .ok_or(RuntimeError::IntegerOverflow),
        }
    };
}
use neg_signed;

macro_rules! int_arith_op {
    ($op:expr, $a:expr, $b:expr, $variant:path, $policy:expr) => {
        match $op {
            BinaryOp::Add => match $policy {
                OverflowPolicy::Wrap => Ok($variant($a.wrapping_add($b))),
                OverflowPolicy::Trap => $a
                    .checked_add($b)
                    .map($variant)
                    .ok_or(RuntimeError::IntegerOverflow),
            },
            BinaryOp::Sub => match $policy {
                OverflowPolicy::Wrap => Ok($variant($a.wrapping_sub($b))),
                OverflowPolicy::Trap => $a
                    .checked_sub($b)
                    .map($variant)
                    .ok_or(RuntimeError::IntegerOverflow),
            },
            BinaryOp::Mul => match $policy {
                OverflowPolicy::Wrap => Ok($variant($a.wrapping_mul($b))),
                OverflowPolicy::Trap => $a
                    .checked_mul($b)
                    .map($variant)
                    .ok_or(RuntimeError::IntegerOverflow),
            },
            BinaryOp::Div => {
                if $b == 0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                match $policy {
                    OverflowPolicy::Wrap => Ok($variant($a.wrapping_div($b))),
                    OverflowPolicy::Trap => $a
                        .checked_div($b)
                        .map($variant)
                        .ok_or(RuntimeError::IntegerOverflow),
                }
            }
            BinaryOp::Mod => {
                if $b == 0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                match $policy {
                    OverflowPolicy::Wrap => Ok($variant($a.wrapping_rem($b))),
                    OverflowPolicy::Trap => $a
                        .checked_rem($b)
                        .map($variant)
                        .ok_or(RuntimeError::IntegerOverflow),
                }
            }
            BinaryOp::Shl => Ok($variant($a.wrapping_shl($b as u32))),
            BinaryOp::Shr => Ok($variant($a.wrapping_shr($b as u32))),
            BinaryOp::BitAnd => Ok($variant($a & $b)),
            BinaryOp::BitXor => Ok($variant($a ^ $b)),
            BinaryOp::BitOr => Ok($variant($a | $b)),
            BinaryOp::Lt => Ok(Value::Bool($a < $b)),
            BinaryOp::Le => Ok(Value::Bool($a <= $b)),
            BinaryOp::Gt => Ok(Value::Bool($a > $b)),
            BinaryOp::Ge => Ok(Value::Bool($a >= $b)),
            BinaryOp::Eq => Ok(Value::Bool($a == $b)),
            BinaryOp::Ne => Ok(Value::Bool($a != $b)),
            BinaryOp::And | BinaryOp::Xor | BinaryOp::Or => Err(RuntimeError::TypeError(
                "logical operator requires BOOL".to_owned(),
            )),
        }
    };
}

macro_rules! float_op {
    ($op:expr, $a:expr, $b:expr, $variant:path) => {
        match $op {
            BinaryOp::Add => Ok($variant($a + $b)),
            BinaryOp::Sub => Ok($variant($a - $b)),
            BinaryOp::Mul => Ok($variant($a * $b)),
            BinaryOp::Div => {
                if $b == 0.0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                Ok($variant($a / $b))
            }
            BinaryOp::Mod => {
                if $b == 0.0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                Ok($variant($a % $b))
            }
            BinaryOp::Lt => Ok(Value::Bool($a < $b)),
            BinaryOp::Le => Ok(Value::Bool($a <= $b)),
            BinaryOp::Gt => Ok(Value::Bool($a > $b)),
            BinaryOp::Ge => Ok(Value::Bool($a >= $b)),
            BinaryOp::Eq => Ok(Value::Bool($a == $b)),
            BinaryOp::Ne => Ok(Value::Bool($a != $b)),
            BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr
            | BinaryOp::And
            | BinaryOp::Xor
            | BinaryOp::Or => Err(RuntimeError::TypeError(
                "operator unsupported for floating-point".to_owned(),
            )),
        }
    };
}

/// Evaluates a binary operator strictly on matched types (`01` §2.1).
pub fn eval_binary(
    op: BinaryOp,
    lhs: Value,
    rhs: Value,
    overflow: OverflowPolicy,
) -> Result<Value, RuntimeError> {
    match (lhs, rhs) {
        (Value::Bool(a), Value::Bool(b)) => match op {
            BinaryOp::And => Ok(Value::Bool(a && b)),
            BinaryOp::Xor => Ok(Value::Bool(a ^ b)),
            BinaryOp::Or => Ok(Value::Bool(a || b)),
            BinaryOp::Eq => Ok(Value::Bool(a == b)),
            BinaryOp::Ne => Ok(Value::Bool(a != b)),
            BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Mod
            | BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr => Err(RuntimeError::TypeError(
                "invalid binary operator on BOOL".to_owned(),
            )),
        },
        (Value::Sint(a), Value::Sint(b)) => int_arith_op!(op, a, b, Value::Sint, overflow),
        (Value::Int(a), Value::Int(b)) => int_arith_op!(op, a, b, Value::Int, overflow),
        (Value::Dint(a), Value::Dint(b)) => int_arith_op!(op, a, b, Value::Dint, overflow),
        (Value::Lint(a), Value::Lint(b)) => int_arith_op!(op, a, b, Value::Lint, overflow),
        (Value::Usint(a), Value::Usint(b)) => int_arith_op!(op, a, b, Value::Usint, overflow),
        (Value::Uint(a), Value::Uint(b)) => int_arith_op!(op, a, b, Value::Uint, overflow),
        (Value::Udint(a), Value::Udint(b)) => int_arith_op!(op, a, b, Value::Udint, overflow),
        (Value::Ulint(a), Value::Ulint(b)) => int_arith_op!(op, a, b, Value::Ulint, overflow),
        (Value::Byte(a), Value::Byte(b)) => int_arith_op!(op, a, b, Value::Byte, overflow),
        (Value::Word(a), Value::Word(b)) => int_arith_op!(op, a, b, Value::Word, overflow),
        (Value::Dword(a), Value::Dword(b)) => int_arith_op!(op, a, b, Value::Dword, overflow),
        (Value::Lword(a), Value::Lword(b)) => int_arith_op!(op, a, b, Value::Lword, overflow),
        (Value::TimeUs(a), Value::TimeUs(b)) => int_arith_op!(op, a, b, Value::TimeUs, overflow),
        (Value::Analog(a), Value::Analog(b)) => match op {
            BinaryOp::Lt => Ok(Value::Bool(a < b)),
            BinaryOp::Le => Ok(Value::Bool(a <= b)),
            BinaryOp::Gt => Ok(Value::Bool(a > b)),
            BinaryOp::Ge => Ok(Value::Bool(a >= b)),
            BinaryOp::Eq => Ok(Value::Bool(a == b)),
            BinaryOp::Ne => Ok(Value::Bool(a != b)),
            BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Mod
            | BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr
            | BinaryOp::And
            | BinaryOp::Xor
            | BinaryOp::Or => Err(RuntimeError::TypeError(
                "arithmetic on ANALOG requires explicit conversion".to_owned(),
            )),
        },
        (Value::Duty(a), Value::Duty(b)) => match op {
            BinaryOp::Lt => Ok(Value::Bool(a < b)),
            BinaryOp::Le => Ok(Value::Bool(a <= b)),
            BinaryOp::Gt => Ok(Value::Bool(a > b)),
            BinaryOp::Ge => Ok(Value::Bool(a >= b)),
            BinaryOp::Eq => Ok(Value::Bool(a == b)),
            BinaryOp::Ne => Ok(Value::Bool(a != b)),
            BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Mod
            | BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr
            | BinaryOp::And
            | BinaryOp::Xor
            | BinaryOp::Or => Err(RuntimeError::TypeError(
                "arithmetic on DUTY requires explicit conversion".to_owned(),
            )),
        },
        (Value::Real(a), Value::Real(b)) => float_op!(op, a, b, Value::Real),
        (Value::Lreal(a), Value::Lreal(b)) => float_op!(op, a, b, Value::Lreal),
        (Value::String { text: a, .. }, Value::String { text: b, .. }) => match op {
            BinaryOp::Eq => Ok(Value::Bool(a == b)),
            BinaryOp::Ne => Ok(Value::Bool(a != b)),
            BinaryOp::Lt => Ok(Value::Bool(a < b)),
            BinaryOp::Le => Ok(Value::Bool(a <= b)),
            BinaryOp::Gt => Ok(Value::Bool(a > b)),
            BinaryOp::Ge => Ok(Value::Bool(a >= b)),
            BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Mod
            | BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::BitAnd
            | BinaryOp::BitXor
            | BinaryOp::BitOr
            | BinaryOp::And
            | BinaryOp::Xor
            | BinaryOp::Or => Err(RuntimeError::TypeError(
                "unsupported binary operator on STRING".to_owned(),
            )),
        },
        // Allow literal Dint coerced against concrete fixed-width integer on either side
        (lhs_v, Value::Dint(r)) if is_fixed_int_or_time(&lhs_v) => {
            let coerced = Value::from_i128_like(&lhs_v, i128::from(r))?;
            eval_binary(op, lhs_v, coerced, overflow)
        }
        (Value::Dint(l), rhs_v) if is_fixed_int_or_time(&rhs_v) => {
            let coerced = Value::from_i128_like(&rhs_v, i128::from(l))?;
            eval_binary(op, coerced, rhs_v, overflow)
        }
        (Value::Real(a), Value::Lreal(b)) => float_op!(op, f64::from(a), b, Value::Lreal),
        (Value::Lreal(a), Value::Real(b)) => float_op!(op, a, f64::from(b), Value::Lreal),
        (a, b) => Err(RuntimeError::TypeError(format!(
            "binary operator {op:?} type mismatch: {a:?} vs {b:?}"
        ))),
    }
}

const fn is_fixed_int_or_time(v: &Value) -> bool {
    matches!(
        v,
        Value::Sint(_)
            | Value::Int(_)
            | Value::Lint(_)
            | Value::Usint(_)
            | Value::Uint(_)
            | Value::Udint(_)
            | Value::Ulint(_)
            | Value::Byte(_)
            | Value::Word(_)
            | Value::Dword(_)
            | Value::Lword(_)
    )
}

/// Pure-Rust deterministic `libm` transcendental evaluation (`02` §2.1).
pub fn eval_stdlib_math(name: &str, args: &[Value]) -> Result<Option<Value>, RuntimeError> {
    match name {
        "SQRT" => {
            let x = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("SQRT expects 1 arg".to_owned()))?
                .as_f64()?;
            Ok(Some(Value::Lreal(libm::sqrt(x))))
        }
        "SIN" => {
            let x = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("SIN expects 1 arg".to_owned()))?
                .as_f64()?;
            Ok(Some(Value::Lreal(libm::sin(x))))
        }
        "COS" => {
            let x = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("COS expects 1 arg".to_owned()))?
                .as_f64()?;
            Ok(Some(Value::Lreal(libm::cos(x))))
        }
        "TAN" => {
            let x = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("TAN expects 1 arg".to_owned()))?
                .as_f64()?;
            Ok(Some(Value::Lreal(libm::tan(x))))
        }
        "ATAN2" => {
            let y = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("ATAN2 expects 2 args".to_owned()))?
                .as_f64()?;
            let x = args
                .get(1)
                .ok_or_else(|| RuntimeError::TypeError("ATAN2 expects 2 args".to_owned()))?
                .as_f64()?;
            Ok(Some(Value::Lreal(libm::atan2(y, x))))
        }
        "ABS" => {
            let v = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("ABS expects 1 arg".to_owned()))?;
            let out = match v {
                Value::Sint(x) => Value::Sint(x.wrapping_abs()),
                Value::Int(x) => Value::Int(x.wrapping_abs()),
                Value::Dint(x) => Value::Dint(x.wrapping_abs()),
                Value::Lint(x) => Value::Lint(x.wrapping_abs()),
                Value::Real(x) => Value::Real(libm::fabsf(*x)),
                Value::Lreal(x) => Value::Lreal(libm::fabs(*x)),
                Value::Usint(_) | Value::Uint(_) | Value::Udint(_) | Value::Ulint(_) => v.clone(),
                Value::Void
                | Value::Bool(_)
                | Value::Byte(_)
                | Value::Word(_)
                | Value::Dword(_)
                | Value::Lword(_)
                | Value::TimeUs(_)
                | Value::String { .. }
                | Value::Analog(_)
                | Value::Duty(_)
                | Value::Code { .. }
                | Value::Array(_)
                | Value::Struct(_)
                | Value::Fb(_) => {
                    return Err(RuntimeError::TypeError("ABS expects numeric".to_owned()));
                }
            };
            Ok(Some(out))
        }
        "MIN" | "MAX" => {
            let a = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("MIN/MAX expects 2 args".to_owned()))?;
            let b = args
                .get(1)
                .ok_or_else(|| RuntimeError::TypeError("MIN/MAX expects 2 args".to_owned()))?;
            let cmp =
                eval_binary(BinaryOp::Lt, a.clone(), b.clone(), OverflowPolicy::Wrap)?.as_bool()?;
            let pick_a = if name == "MIN" { cmp } else { !cmp };
            Ok(Some(if pick_a { a.clone() } else { b.clone() }))
        }
        "LIMIT" | "CLAMP" => {
            let (val, lo, hi) = if name == "LIMIT" {
                (args.get(1), args.first(), args.get(2))
            } else {
                (args.first(), args.get(1), args.get(2))
            };
            let v = val
                .ok_or_else(|| RuntimeError::TypeError("LIMIT/CLAMP expects 3 args".to_owned()))?;
            let lo =
                lo.ok_or_else(|| RuntimeError::TypeError("LIMIT/CLAMP expects 3 args".to_owned()))?;
            let hi =
                hi.ok_or_else(|| RuntimeError::TypeError("LIMIT/CLAMP expects 3 args".to_owned()))?;
            let lt_lo = eval_binary(BinaryOp::Lt, v.clone(), lo.clone(), OverflowPolicy::Wrap)?
                .as_bool()?;
            if lt_lo {
                return Ok(Some(lo.clone()));
            }
            let gt_hi = eval_binary(BinaryOp::Gt, v.clone(), hi.clone(), OverflowPolicy::Wrap)?
                .as_bool()?;
            if gt_hi {
                return Ok(Some(hi.clone()));
            }
            Ok(Some(v.clone()))
        }
        "NORM_X" => {
            let min = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("NORM_X expects 3 args".to_owned()))?
                .as_f64()?;
            let val = args
                .get(1)
                .ok_or_else(|| RuntimeError::TypeError("NORM_X expects 3 args".to_owned()))?
                .as_f64()?;
            let max = args
                .get(2)
                .ok_or_else(|| RuntimeError::TypeError("NORM_X expects 3 args".to_owned()))?
                .as_f64()?;
            let denom = max - min;
            if denom == 0.0 {
                return Err(RuntimeError::DivisionByZero);
            }
            Ok(Some(Value::Real(((val - min) / denom) as f32)))
        }
        "SCALE_X" => {
            let min = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("SCALE_X expects 3 args".to_owned()))?
                .as_f64()?;
            let val = args
                .get(1)
                .ok_or_else(|| RuntimeError::TypeError("SCALE_X expects 3 args".to_owned()))?
                .as_f64()?;
            let max = args
                .get(2)
                .ok_or_else(|| RuntimeError::TypeError("SCALE_X expects 3 args".to_owned()))?
                .as_f64()?;
            // Separate multiply + add: never implicit FMA (`02` §2.1)
            let span = max - min;
            let scaled = val * span;
            Ok(Some(Value::Real((scaled + min) as f32)))
        }
        "TIME_DIFF" => {
            // Exact unsigned wrapping difference for both `TIME` (64-bit us) and 32-bit `MILLIS`/`MICROS` (`02` §2.3)
            let a = args
                .first()
                .ok_or_else(|| RuntimeError::TypeError("TIME_DIFF expects 2 args".to_owned()))?;
            let b = args
                .get(1)
                .ok_or_else(|| RuntimeError::TypeError("TIME_DIFF expects 2 args".to_owned()))?;
            match (a, b) {
                (Value::TimeUs(now), Value::TimeUs(prev)) => {
                    let diff = (*now as u64).wrapping_sub(*prev as u64) as i64;
                    Ok(Some(Value::TimeUs(diff)))
                }
                (Value::Ulint(now), Value::Ulint(prev)) => {
                    Ok(Some(Value::Ulint(now.wrapping_sub(*prev))))
                }
                _ => {
                    let now = a.as_i128()? as u32;
                    let prev = b.as_i128()? as u32;
                    Ok(Some(Value::Udint(now.wrapping_sub(prev))))
                }
            }
        }
        _ => Ok(None),
    }
}
