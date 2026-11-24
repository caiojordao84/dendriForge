//! Per-board Arduino C++ <-> ASL v2 type mapping (`docs/lbc/cpp.md` §2).
//!
//! Strictly honors `BoardContext.int_bits` (`16` on AVR `arduino-uno-r3`, `32` on 32-bit targets).

use dendri_ast::{FbKind, ParamDecl, ParamDirection, ScalarType, StorageScope, TypeRef};
use dendri_frontend_core::BoardContext;

/// Resolves a standard IEC 61131-3 Function Block type name to [`FbKind`].
#[must_use]
pub(crate) fn builtin_fb_kind(name: &str) -> Option<FbKind> {
    match name {
        "TON" => Some(FbKind::Ton),
        "TOF" => Some(FbKind::Tof),
        "TP" => Some(FbKind::Tp),
        "CTU" => Some(FbKind::Ctu),
        "CTD" => Some(FbKind::Ctd),
        "CTUD" => Some(FbKind::Ctud),
        "R_TRIG" => Some(FbKind::RTrig),
        "F_TRIG" => Some(FbKind::FTrig),
        "SR" => Some(FbKind::Sr),
        "RS" => Some(FbKind::Rs),
        "UART_SEND" => Some(FbKind::UartSend),
        "UART_RECV" => Some(FbKind::UartRecv),
        "I2C_XFER" => Some(FbKind::I2cXfer),
        "SPI_XFER" => Some(FbKind::SpiXfer),
        "MB_CLIENT" => Some(FbKind::MbClient),
        "MB_SERVER" => Some(FbKind::MbServer),
        _ => None,
    }
}

/// Canonical C++ type name for an IEC Function Block [`FbKind`].
#[must_use]
pub(crate) fn fb_kind_to_cpp_name(kind: &FbKind) -> &str {
    match kind {
        FbKind::Ton => "TON",
        FbKind::Tof => "TOF",
        FbKind::Tp => "TP",
        FbKind::Ctu => "CTU",
        FbKind::Ctd => "CTD",
        FbKind::Ctud => "CTUD",
        FbKind::RTrig => "R_TRIG",
        FbKind::FTrig => "F_TRIG",
        FbKind::Sr => "SR",
        FbKind::Rs => "RS",
        FbKind::UartSend => "UART_SEND",
        FbKind::UartRecv => "UART_RECV",
        FbKind::I2cXfer => "I2C_XFER",
        FbKind::SpiXfer => "SPI_XFER",
        FbKind::MbClient => "MB_CLIENT",
        FbKind::MbServer => "MB_SERVER",
        FbKind::User { fb_name } => fb_name.as_str(),
    }
}

/// Maps C++ `double` to ASL `REAL` on 16-bit AVR (`f32`) or `LREAL` on 32-bit+ targets (`f64`).
#[must_use]
pub(crate) const fn c_double_asl_type(ctx: &BoardContext) -> ScalarType {
    if ctx.int_bits <= 16 {
        ScalarType::Real
    } else {
        ScalarType::Lreal
    }
}

/// Maps a single-word or multi-word C++ primitive type sequence to its canonical ASL [`TypeRef`].
///
/// Honors `ctx.int_bits` for `int` / `unsigned int` / `double`.
#[must_use]
pub(crate) fn resolve_primitive_cpp_type(words: &[&str], ctx: &BoardContext) -> Option<TypeRef> {
    match words {
        ["void"] => Some(TypeRef::Void),
        ["bool"] => Some(TypeRef::scalar(ScalarType::Bool)),
        ["int8_t"] | ["signed", "char"] | ["char"] => Some(TypeRef::scalar(ScalarType::Sint)),
        ["uint8_t"] | ["byte"] | ["unsigned", "char"] => Some(TypeRef::scalar(ScalarType::Usint)),
        ["int16_t"]
        | ["short"]
        | ["short", "int"]
        | ["signed", "short"]
        | ["signed", "short", "int"] => Some(TypeRef::scalar(ScalarType::Int)),
        ["uint16_t"] | ["word"] | ["unsigned", "short"] | ["unsigned", "short", "int"] => {
            Some(TypeRef::scalar(ScalarType::Uint))
        }
        ["int"] | ["signed"] | ["signed", "int"] => Some(TypeRef::scalar(ctx.c_int_asl_type())),
        ["unsigned"] | ["unsigned", "int"] => Some(TypeRef::scalar(ctx.c_uint_asl_type())),
        ["int32_t"]
        | ["long"]
        | ["long", "int"]
        | ["signed", "long"]
        | ["signed", "long", "int"] => Some(TypeRef::scalar(ScalarType::Dint)),
        ["uint32_t"] | ["unsigned", "long"] | ["unsigned", "long", "int"] => {
            Some(TypeRef::scalar(ScalarType::Udint))
        }
        ["int64_t"]
        | ["long", "long"]
        | ["long", "long", "int"]
        | ["signed", "long", "long"]
        | ["signed", "long", "long", "int"] => Some(TypeRef::scalar(ScalarType::Lint)),
        ["uint64_t"] | ["unsigned", "long", "long"] | ["unsigned", "long", "long", "int"] => {
            Some(TypeRef::scalar(ScalarType::Ulint))
        }
        ["float"] => Some(TypeRef::scalar(ScalarType::Real)),
        ["double"] => Some(TypeRef::scalar(c_double_asl_type(ctx))),
        _ => None,
    }
}

/// Returns `true` iff `word` can start a built-in C++ primitive type specifier.
#[must_use]
pub(crate) fn is_cpp_primitive_type_keyword(word: &str) -> bool {
    matches!(
        word,
        "void"
            | "bool"
            | "char"
            | "signed"
            | "unsigned"
            | "short"
            | "int"
            | "long"
            | "float"
            | "double"
            | "int8_t"
            | "uint8_t"
            | "int16_t"
            | "uint16_t"
            | "int32_t"
            | "uint32_t"
            | "int64_t"
            | "uint64_t"
            | "byte"
            | "word"
            | "size_t"
    )
}

/// Maps an ASL [`ScalarType`] to its exact fixed-width C++ type name for lossless emission.
#[must_use]
pub(crate) const fn scalar_to_cpp_type(scalar: &ScalarType, ctx: &BoardContext) -> &'static str {
    match scalar {
        ScalarType::Bool => "bool",
        ScalarType::Sint => "int8_t",
        ScalarType::Int => "int16_t",
        ScalarType::Dint => "int32_t",
        ScalarType::Lint => "int64_t",
        ScalarType::Usint | ScalarType::Byte => "uint8_t",
        ScalarType::Uint | ScalarType::Word => "uint16_t",
        ScalarType::Udint | ScalarType::Dword | ScalarType::Time | ScalarType::Duty => "uint32_t",
        ScalarType::Ulint | ScalarType::Lword => "uint64_t",
        ScalarType::Real => "float",
        ScalarType::Lreal => {
            if ctx.int_bits <= 16 {
                "float"
            } else {
                "double"
            }
        }
        ScalarType::String { .. } => "const char*",
        ScalarType::Analog => "int32_t",
        ScalarType::Code { .. } => {
            if ctx.int_bits <= 16 {
                "int16_t"
            } else {
                "int32_t"
            }
        }
    }
}

/// Formats an ASL [`TypeRef`] for C++ casts / return types (`int16_t`, `float`, `void`, `const char*`, etc.).
#[must_use]
pub(crate) fn type_ref_to_cpp_type_name(ty: &TypeRef, ctx: &BoardContext) -> String {
    match ty {
        TypeRef::Void => "void".to_owned(),
        TypeRef::Scalar { ty: s } => scalar_to_cpp_type(s, ctx).to_owned(),
        TypeRef::Array { elem, .. } => format!("{}*", type_ref_to_cpp_type_name(elem, ctx)),
        TypeRef::Named { name } => name.clone(),
        TypeRef::Fb { fb_kind } => fb_kind_to_cpp_name(fb_kind).to_owned(),
    }
}

/// Formats a variable or struct field declaration `<type> <name>[<len>]` cleanly in idiomatic C++.
#[must_use]
pub(crate) fn format_cpp_var_declarator(
    ty: &TypeRef,
    name: &str,
    scope: Option<StorageScope>,
    ctx: &BoardContext,
) -> String {
    match ty {
        TypeRef::Scalar {
            ty: ScalarType::String { max_len },
        } => {
            if *max_len == 255 && scope == Some(StorageScope::Const) {
                format!("const char* {name}")
            } else {
                let prefix = if scope == Some(StorageScope::Const) {
                    "const "
                } else {
                    ""
                };
                format!("{prefix}char {name}[{max_len}]")
            }
        }
        TypeRef::Array { elem, len } => {
            let prefix = if scope == Some(StorageScope::Const) {
                "const "
            } else {
                ""
            };
            format!(
                "{prefix}{} {name}[{len}]",
                type_ref_to_cpp_type_name(elem, ctx)
            )
        }
        TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Named { .. } | TypeRef::Fb { .. } => {
            let prefix = if scope == Some(StorageScope::Const) {
                "const "
            } else {
                ""
            };
            format!("{prefix}{} {name}", type_ref_to_cpp_type_name(ty, ctx))
        }
    }
}

/// Formats a POU parameter declaration for C++ (`T name` for `In`, `T& name` for `InOut`/`Out`).
#[must_use]
pub(crate) fn format_cpp_param(param: &ParamDecl, ctx: &BoardContext) -> String {
    let by_ref = matches!(param.direction, ParamDirection::InOut | ParamDirection::Out);
    match &param.ty {
        TypeRef::Scalar {
            ty: ScalarType::String { .. },
        } => format!("const char* {}", param.name),
        TypeRef::Array { elem, len } => {
            format!(
                "{} {}[{len}]",
                type_ref_to_cpp_type_name(elem, ctx),
                param.name
            )
        }
        TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Named { .. } | TypeRef::Fb { .. } => {
            let ty_str = type_ref_to_cpp_type_name(&param.ty, ctx);
            if by_ref {
                format!("{ty_str}& {}", param.name)
            } else {
                format!("{ty_str} {}", param.name)
            }
        }
    }
}

/// Returns `true` iff `actual` is assignable/compatible with `expected` under strict ASL/C++ rules.
///
/// Rejects mixed-sign (`INT` vs `UINT`) and implicit narrowing (`DINT` -> `INT`), while permitting
/// `STRING[N]` capacity compatibility and `CODE<...>` <-> native integer codes for Arduino `analogRead`.
#[must_use]
pub(crate) fn types_compatible_cpp(expected: &TypeRef, actual: &TypeRef) -> bool {
    if expected == actual {
        return true;
    }
    match (expected, actual) {
        (
            TypeRef::Scalar {
                ty: ScalarType::String { .. },
            },
            TypeRef::Scalar {
                ty: ScalarType::String { .. },
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Code { .. },
            },
            TypeRef::Scalar {
                ty: ScalarType::Code { .. },
            },
        ) => true,
        // Arduino idiomatic `int raw = analogRead(A0);` / `analogWrite(5, val);`
        (
            TypeRef::Scalar {
                ty:
                    ScalarType::Int
                    | ScalarType::Uint
                    | ScalarType::Dint
                    | ScalarType::Udint
                    | ScalarType::Usint
                    | ScalarType::Lint
                    | ScalarType::Ulint,
            },
            TypeRef::Scalar {
                ty: ScalarType::Code { .. },
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Code { .. },
            },
            TypeRef::Scalar {
                ty:
                    ScalarType::Int
                    | ScalarType::Uint
                    | ScalarType::Dint
                    | ScalarType::Udint
                    | ScalarType::Usint
                    | ScalarType::Sint
                    | ScalarType::Lint
                    | ScalarType::Ulint,
            },
        ) => true,
        // Bitstring aliases `BYTE` <-> `USINT`, `WORD` <-> `UINT`, `DWORD` <-> `UDINT`, `LWORD` <-> `ULINT`
        (
            TypeRef::Scalar {
                ty: ScalarType::Byte,
            },
            TypeRef::Scalar {
                ty: ScalarType::Usint,
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Usint,
            },
            TypeRef::Scalar {
                ty: ScalarType::Byte,
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Word,
            },
            TypeRef::Scalar {
                ty: ScalarType::Uint,
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Uint,
            },
            TypeRef::Scalar {
                ty: ScalarType::Word,
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Dword,
            },
            TypeRef::Scalar {
                ty: ScalarType::Udint,
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Udint,
            },
            TypeRef::Scalar {
                ty: ScalarType::Dword,
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Lword,
            },
            TypeRef::Scalar {
                ty: ScalarType::Ulint,
            },
        )
        | (
            TypeRef::Scalar {
                ty: ScalarType::Ulint,
            },
            TypeRef::Scalar {
                ty: ScalarType::Lword,
            },
        ) => true,
        (
            TypeRef::Void
            | TypeRef::Scalar { .. }
            | TypeRef::Array { .. }
            | TypeRef::Named { .. }
            | TypeRef::Fb { .. },
            _,
        ) => false,
    }
}

/// Validates whether an explicit C++ cast `static_cast<to>(from)` / `(to)from` is permitted.
///
/// Rejects casts across incompatible domains (e.g. `STRING` -> `INT`, `STRUCT` -> `INT`,
/// or `ANALOG`/`DUTY` -> plain scalar without explicit codec intrinsic).
#[must_use]
pub(crate) fn is_valid_cpp_cast(from: &TypeRef, to: &TypeRef) -> bool {
    if from == to {
        return true;
    }
    let is_castable_scalar = |s: &ScalarType| {
        s.is_plain_numeric()
            || s.is_bitstring()
            || matches!(s, ScalarType::Bool | ScalarType::Code { .. })
    };
    match (from, to) {
        (TypeRef::Scalar { ty: f }, TypeRef::Scalar { ty: t }) => {
            is_castable_scalar(f) && is_castable_scalar(t)
        }
        (
            TypeRef::Void
            | TypeRef::Scalar { .. }
            | TypeRef::Array { .. }
            | TypeRef::Named { .. }
            | TypeRef::Fb { .. },
            _,
        ) => false,
    }
}

/// Canonical ASL conversion callee name for an explicit cast `from -> to`.
#[must_use]
pub(crate) fn cast_callee_name(from: Option<&TypeRef>, to: &TypeRef) -> String {
    match (from, to) {
        (
            Some(TypeRef::Scalar {
                ty: ScalarType::Bool,
            }),
            TypeRef::Scalar {
                ty: ScalarType::Int,
            },
        ) => "BOOL_TO_INT".to_owned(),
        (
            Some(TypeRef::Scalar {
                ty: ScalarType::Int,
            }),
            TypeRef::Scalar {
                ty: ScalarType::Dint,
            },
        ) => "INT_TO_DINT".to_owned(),
        (
            Some(TypeRef::Scalar {
                ty: ScalarType::Dint,
            }),
            TypeRef::Scalar {
                ty: ScalarType::Real,
            },
        ) => "DINT_TO_REAL".to_owned(),
        (
            Some(TypeRef::Scalar {
                ty: ScalarType::Real,
            }),
            TypeRef::Scalar {
                ty: ScalarType::Dint,
            },
        ) => "REAL_TO_DINT".to_owned(),
        (_, TypeRef::Scalar { ty: s }) => format!("TO_{}", scalar_upper_name(s)),
        (_, other) => format!("TO_{other}"),
    }
}

/// Resolves a `TO_<SCALAR>` or `<SRC>_TO_<DST>` cast callee back to its target ASL [`TypeRef`].
#[must_use]
pub(crate) fn cast_target_type_from_callee(callee: &str) -> Option<TypeRef> {
    match callee {
        "BOOL_TO_INT" | "TO_INT" => Some(TypeRef::scalar(ScalarType::Int)),
        "INT_TO_DINT" | "REAL_TO_DINT" | "TO_DINT" => Some(TypeRef::scalar(ScalarType::Dint)),
        "DINT_TO_REAL" | "TO_REAL" => Some(TypeRef::scalar(ScalarType::Real)),
        "TO_BOOL" => Some(TypeRef::scalar(ScalarType::Bool)),
        "TO_SINT" => Some(TypeRef::scalar(ScalarType::Sint)),
        "TO_LINT" => Some(TypeRef::scalar(ScalarType::Lint)),
        "TO_USINT" => Some(TypeRef::scalar(ScalarType::Usint)),
        "TO_UINT" => Some(TypeRef::scalar(ScalarType::Uint)),
        "TO_UDINT" => Some(TypeRef::scalar(ScalarType::Udint)),
        "TO_ULINT" => Some(TypeRef::scalar(ScalarType::Ulint)),
        "TO_LREAL" => Some(TypeRef::scalar(ScalarType::Lreal)),
        "TO_BYTE" => Some(TypeRef::scalar(ScalarType::Byte)),
        "TO_WORD" => Some(TypeRef::scalar(ScalarType::Word)),
        "TO_DWORD" => Some(TypeRef::scalar(ScalarType::Dword)),
        "TO_LWORD" => Some(TypeRef::scalar(ScalarType::Lword)),
        _ => None,
    }
}

const fn scalar_upper_name(s: &ScalarType) -> &'static str {
    match s {
        ScalarType::Bool => "BOOL",
        ScalarType::Sint => "SINT",
        ScalarType::Int => "INT",
        ScalarType::Dint => "DINT",
        ScalarType::Lint => "LINT",
        ScalarType::Usint => "USINT",
        ScalarType::Uint => "UINT",
        ScalarType::Udint => "UDINT",
        ScalarType::Ulint => "ULINT",
        ScalarType::Byte => "BYTE",
        ScalarType::Word => "WORD",
        ScalarType::Dword => "DWORD",
        ScalarType::Lword => "LWORD",
        ScalarType::Real => "REAL",
        ScalarType::Lreal => "LREAL",
        ScalarType::Time => "TIME",
        ScalarType::String { .. } => "STRING",
        ScalarType::Analog => "ANALOG",
        ScalarType::Duty => "DUTY",
        ScalarType::Code { .. } => "CODE",
    }
}

/// Returns `true` iff `ty` is an integer-like target suitable for unsuffixed integer literals.
#[must_use]
pub(crate) fn is_int_like_type(ty: &TypeRef) -> bool {
    matches!(
        ty,
        TypeRef::Scalar {
            ty: ScalarType::Sint
                | ScalarType::Int
                | ScalarType::Dint
                | ScalarType::Lint
                | ScalarType::Usint
                | ScalarType::Uint
                | ScalarType::Udint
                | ScalarType::Ulint
                | ScalarType::Byte
                | ScalarType::Word
                | ScalarType::Dword
                | ScalarType::Lword
                | ScalarType::Code { .. }
                | ScalarType::Duty
                | ScalarType::Analog
                | ScalarType::Time
        }
    )
}
