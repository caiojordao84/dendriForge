//! ASL v2 static type system (`01-asl-ir-and-project-schema.md` §2, ADR-005).

use crate::ident::NodeId;
use crate::project::ForeignFidelity;
use dendri_span::Span;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Fixed-width primitive scalar types in ASL v2.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScalarType {
    Bool,
    Sint,
    Int,
    Dint,
    Lint,
    Usint,
    Uint,
    Udint,
    Ulint,
    Byte,
    Word,
    Dword,
    Lword,
    Real,
    Lreal,
    Time,
    String {
        #[serde(default = "default_string_max_len")]
        max_len: u32,
    },
    Analog,
    Duty,
    Code {
        codec_id: String,
    },
}

const fn default_string_max_len() -> u32 {
    255
}

impl ScalarType {
    /// Returns `true` for signed two's-complement integer types (`SINT`..`LINT`).
    #[must_use]
    pub const fn is_signed_int(&self) -> bool {
        matches!(self, Self::Sint | Self::Int | Self::Dint | Self::Lint)
    }

    /// Returns `true` for unsigned integer types (`USINT`..`ULINT`).
    #[must_use]
    pub const fn is_unsigned_int(&self) -> bool {
        matches!(self, Self::Usint | Self::Uint | Self::Udint | Self::Ulint)
    }

    /// Returns `true` for bit-string types (`BYTE`..`LWORD`).
    #[must_use]
    pub const fn is_bitstring(&self) -> bool {
        matches!(self, Self::Byte | Self::Word | Self::Dword | Self::Lword)
    }

    /// Returns `true` for IEEE-754 floating-point types (`REAL`, `LREAL`).
    #[must_use]
    pub const fn is_float(&self) -> bool {
        matches!(self, Self::Real | Self::Lreal)
    }

    /// Returns `true` for integer or floating-point arithmetic scalars (NOT `ANALOG`/`DUTY`/`CODE`).
    #[must_use]
    pub const fn is_plain_numeric(&self) -> bool {
        self.is_signed_int() || self.is_unsigned_int() || self.is_float()
    }
}

/// First-class IEC 61131-3 & communication Function Block kinds (`01` §2.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub enum FbKind {
    Ton,
    Tof,
    Tp,
    Ctu,
    Ctd,
    Ctud,
    RTrig,
    FTrig,
    Sr,
    Rs,
    UartSend,
    UartRecv,
    I2cXfer,
    SpiXfer,
    MbClient,
    MbServer,
    User { fb_name: String },
}

/// Type reference used in declarations, parameters, fields, and return signatures.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TypeRef {
    Void,
    Scalar { ty: ScalarType },
    Array { elem: Box<TypeRef>, len: u32 },
    Named { name: String },
    Fb { fb_kind: FbKind },
}

impl TypeRef {
    /// Convenience constructor for scalar types.
    #[must_use]
    pub const fn scalar(ty: ScalarType) -> Self {
        Self::Scalar { ty }
    }
}

impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Void => f.write_str("VOID"),
            Self::Scalar { ty } => match ty {
                ScalarType::Bool => f.write_str("BOOL"),
                ScalarType::Sint => f.write_str("SINT"),
                ScalarType::Int => f.write_str("INT"),
                ScalarType::Dint => f.write_str("DINT"),
                ScalarType::Lint => f.write_str("LINT"),
                ScalarType::Usint => f.write_str("USINT"),
                ScalarType::Uint => f.write_str("UINT"),
                ScalarType::Udint => f.write_str("UDINT"),
                ScalarType::Ulint => f.write_str("ULINT"),
                ScalarType::Byte => f.write_str("BYTE"),
                ScalarType::Word => f.write_str("WORD"),
                ScalarType::Dword => f.write_str("DWORD"),
                ScalarType::Lword => f.write_str("LWORD"),
                ScalarType::Real => f.write_str("REAL"),
                ScalarType::Lreal => f.write_str("LREAL"),
                ScalarType::Time => f.write_str("TIME"),
                ScalarType::String { max_len } => write!(f, "STRING[{max_len}]"),
                ScalarType::Analog => f.write_str("ANALOG"),
                ScalarType::Duty => f.write_str("DUTY"),
                ScalarType::Code { codec_id } => write!(f, "CODE<{codec_id}>"),
            },
            Self::Array { elem, len } => write!(f, "ARRAY[{len}] OF {elem}"),
            Self::Named { name } => f.write_str(name),
            Self::Fb { fb_kind } => match fb_kind {
                FbKind::Ton => f.write_str("TON"),
                FbKind::Tof => f.write_str("TOF"),
                FbKind::Tp => f.write_str("TP"),
                FbKind::Ctu => f.write_str("CTU"),
                FbKind::Ctd => f.write_str("CTD"),
                FbKind::Ctud => f.write_str("CTUD"),
                FbKind::RTrig => f.write_str("R_TRIG"),
                FbKind::FTrig => f.write_str("F_TRIG"),
                FbKind::Sr => f.write_str("SR"),
                FbKind::Rs => f.write_str("RS"),
                FbKind::UartSend => f.write_str("UART_SEND"),
                FbKind::UartRecv => f.write_str("UART_RECV"),
                FbKind::I2cXfer => f.write_str("I2C_XFER"),
                FbKind::SpiXfer => f.write_str("SPI_XFER"),
                FbKind::MbClient => f.write_str("MB_CLIENT"),
                FbKind::MbServer => f.write_str("MB_SERVER"),
                FbKind::User { fb_name } => f.write_str(fb_name),
            },
        }
    }
}

/// Variable storage lifetime/immutability scope (`01` §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageScope {
    Const,
    Var,
    Retain,
    Persistent,
    Temp,
}

/// Typed field inside a flat `STRUCT` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructField {
    pub name: String,
    pub ty: TypeRef,
}

/// Named variant inside an `ENUM` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnumVariant {
    pub name: String,
    pub value: i64,
}

/// User-defined type or declaration-level `Foreign` (`Decl::Foreign`, `01` §6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TypeDecl {
    Struct {
        id: NodeId,
        name: String,
        fields: Vec<StructField>,
        #[serde(skip)]
        span: Span,
    },
    Enum {
        id: NodeId,
        name: String,
        backing: ScalarType,
        variants: Vec<EnumVariant>,
        #[serde(skip)]
        span: Span,
    },
    /// Declaration-level foreign construct (`Decl::Foreign`, e.g., `Servo s;` or C++ class w/ methods).
    Foreign {
        id: NodeId,
        name: Option<String>,
        language: String,
        fidelity: ForeignFidelity,
        text: String,
        pack_hint: Option<String>,
        #[serde(skip)]
        span: Span,
    },
}

/// Variable / constant / FB instance declaration (`[[data]]`).
#[derive(Debug, Clone, PartialEq)]
pub struct DataDecl {
    pub id: NodeId,
    pub name: String,
    pub scope: StorageScope,
    pub ty: TypeRef,
    pub initial: Option<crate::expr::Expr>,
    pub span: Span,
}
