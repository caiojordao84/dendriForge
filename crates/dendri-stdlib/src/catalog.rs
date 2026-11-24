//! Domain-split standard library catalog (`01-asl-ir-and-project-schema.md` §6.1).
//!
//! `std.iec.*` is platform-neutral. Platform modules (`std.arduino.*`, `std.plc.*`)
//! are visible ONLY when explicitly imported or implied by a frontend — a pure-PLC
//! project can never accidentally resolve MCU APIs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Logical domain owning a standard-library module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StdDomain {
    /// Platform-neutral IEC 61131-3 standard library (`std.iec.*`).
    Iec,
    /// Arduino MCU platform library (`std.arduino.*`).
    Arduino,
    /// Industrial PLC platform extension library (`std.plc.*`).
    Plc,
}

impl StdDomain {
    /// Canonical module prefix (e.g., `"std.iec"`).
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Iec => "std.iec",
            Self::Arduino => "std.arduino",
            Self::Plc => "std.plc",
        }
    }
}

/// Static ASL v2 type descriptor used inside stdlib signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StdTypeRef {
    Void,
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
    String,
    Analog,
    Duty,
    Code,
    /// Generic numeric parameter constrained by static typechecking (e.g., `ABS`, `MIN`, `MAX`).
    AnyNum,
    /// Symbolic hardware pin reference (e.g., `DIGITAL_READ(pin)`).
    PinRef,
}

/// Parameter direction matching IEC 61131-3 (`IN`, `OUT`, `IN_OUT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamDir {
    In,
    Out,
    InOut,
}

/// Typed parameter or Function Block field descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdParam {
    pub name: &'static str,
    pub direction: ParamDir,
    pub ty: StdTypeRef,
}

/// Stateless standard library function (`FC`) signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdFnSig {
    pub name: &'static str,
    pub params: &'static [StdParam],
    pub return_type: StdTypeRef,
}

/// Stateful standard library Function Block (`FB`) signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdFbSig {
    pub name: &'static str,
    pub fields: &'static [StdParam],
}

/// Either a stateless function or a stateful Function Block exported by a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdSymbol {
    Function(&'static StdFnSig),
    FunctionBlock(&'static StdFbSig),
}

impl StdSymbol {
    /// Canonical symbol identifier.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Function(f) => f.name,
            Self::FunctionBlock(fb) => fb.name,
        }
    }
}

/// A named domain-scoped standard library module (e.g., `std.iec.timers`).
#[derive(Debug, Clone)]
pub struct StdModule {
    pub path: &'static str,
    pub domain: StdDomain,
    pub functions: &'static [StdFnSig],
    pub function_blocks: &'static [StdFbSig],
}

impl StdModule {
    /// Looks up an exported symbol strictly inside this module.
    #[must_use]
    pub fn get_symbol(&self, name: &str) -> Option<StdSymbol> {
        if let Some(f) = self.functions.iter().find(|f| f.name == name) {
            return Some(StdSymbol::Function(f));
        }
        if let Some(fb) = self.function_blocks.iter().find(|fb| fb.name == name) {
            return Some(StdSymbol::FunctionBlock(fb));
        }
        None
    }
}

const fn p(name: &'static str, direction: ParamDir, ty: StdTypeRef) -> StdParam {
    StdParam {
        name,
        direction,
        ty,
    }
}

// --- std.iec.timers ---
static IEC_TIMER_FBS: &[StdFbSig] = &[
    StdFbSig {
        name: "TON",
        fields: &[
            p("IN", ParamDir::In, StdTypeRef::Bool),
            p("PT", ParamDir::In, StdTypeRef::Time),
            p("Q", ParamDir::Out, StdTypeRef::Bool),
            p("ET", ParamDir::Out, StdTypeRef::Time),
        ],
    },
    StdFbSig {
        name: "TOF",
        fields: &[
            p("IN", ParamDir::In, StdTypeRef::Bool),
            p("PT", ParamDir::In, StdTypeRef::Time),
            p("Q", ParamDir::Out, StdTypeRef::Bool),
            p("ET", ParamDir::Out, StdTypeRef::Time),
        ],
    },
    StdFbSig {
        name: "TP",
        fields: &[
            p("IN", ParamDir::In, StdTypeRef::Bool),
            p("PT", ParamDir::In, StdTypeRef::Time),
            p("Q", ParamDir::Out, StdTypeRef::Bool),
            p("ET", ParamDir::Out, StdTypeRef::Time),
        ],
    },
];

// --- std.iec.counters ---
static IEC_COUNTER_FBS: &[StdFbSig] = &[
    StdFbSig {
        name: "CTU",
        fields: &[
            p("CU", ParamDir::In, StdTypeRef::Bool),
            p("R", ParamDir::In, StdTypeRef::Bool),
            p("PV", ParamDir::In, StdTypeRef::Dint),
            p("Q", ParamDir::Out, StdTypeRef::Bool),
            p("CV", ParamDir::Out, StdTypeRef::Dint),
        ],
    },
    StdFbSig {
        name: "CTD",
        fields: &[
            p("CD", ParamDir::In, StdTypeRef::Bool),
            p("LD", ParamDir::In, StdTypeRef::Bool),
            p("PV", ParamDir::In, StdTypeRef::Dint),
            p("Q", ParamDir::Out, StdTypeRef::Bool),
            p("CV", ParamDir::Out, StdTypeRef::Dint),
        ],
    },
    StdFbSig {
        name: "CTUD",
        fields: &[
            p("CU", ParamDir::In, StdTypeRef::Bool),
            p("CD", ParamDir::In, StdTypeRef::Bool),
            p("R", ParamDir::In, StdTypeRef::Bool),
            p("LD", ParamDir::In, StdTypeRef::Bool),
            p("PV", ParamDir::In, StdTypeRef::Dint),
            p("QU", ParamDir::Out, StdTypeRef::Bool),
            p("QD", ParamDir::Out, StdTypeRef::Bool),
            p("CV", ParamDir::Out, StdTypeRef::Dint),
        ],
    },
];

// --- std.iec.edges ---
static IEC_EDGE_FBS: &[StdFbSig] = &[
    StdFbSig {
        name: "R_TRIG",
        fields: &[
            p("CLK", ParamDir::In, StdTypeRef::Bool),
            p("Q", ParamDir::Out, StdTypeRef::Bool),
        ],
    },
    StdFbSig {
        name: "F_TRIG",
        fields: &[
            p("CLK", ParamDir::In, StdTypeRef::Bool),
            p("Q", ParamDir::Out, StdTypeRef::Bool),
        ],
    },
];

// --- std.iec.bistables ---
static IEC_BISTABLE_FBS: &[StdFbSig] = &[
    StdFbSig {
        name: "SR",
        fields: &[
            p("S1", ParamDir::In, StdTypeRef::Bool),
            p("R", ParamDir::In, StdTypeRef::Bool),
            p("Q1", ParamDir::Out, StdTypeRef::Bool),
        ],
    },
    StdFbSig {
        name: "RS",
        fields: &[
            p("S", ParamDir::In, StdTypeRef::Bool),
            p("R1", ParamDir::In, StdTypeRef::Bool),
            p("Q1", ParamDir::Out, StdTypeRef::Bool),
        ],
    },
];

// --- std.iec.math ---
static IEC_MATH_FNS: &[StdFnSig] = &[
    StdFnSig {
        name: "ABS",
        params: &[p("X", ParamDir::In, StdTypeRef::AnyNum)],
        return_type: StdTypeRef::AnyNum,
    },
    StdFnSig {
        name: "MIN",
        params: &[
            p("A", ParamDir::In, StdTypeRef::AnyNum),
            p("B", ParamDir::In, StdTypeRef::AnyNum),
        ],
        return_type: StdTypeRef::AnyNum,
    },
    StdFnSig {
        name: "MAX",
        params: &[
            p("A", ParamDir::In, StdTypeRef::AnyNum),
            p("B", ParamDir::In, StdTypeRef::AnyNum),
        ],
        return_type: StdTypeRef::AnyNum,
    },
    StdFnSig {
        name: "LIMIT",
        params: &[
            p("MN", ParamDir::In, StdTypeRef::AnyNum),
            p("IN", ParamDir::In, StdTypeRef::AnyNum),
            p("MX", ParamDir::In, StdTypeRef::AnyNum),
        ],
        return_type: StdTypeRef::AnyNum,
    },
    StdFnSig {
        name: "CLAMP",
        params: &[
            p("X", ParamDir::In, StdTypeRef::AnyNum),
            p("LO", ParamDir::In, StdTypeRef::AnyNum),
            p("HI", ParamDir::In, StdTypeRef::AnyNum),
        ],
        return_type: StdTypeRef::AnyNum,
    },
    StdFnSig {
        name: "MAP_RANGE",
        params: &[
            p("X", ParamDir::In, StdTypeRef::Real),
            p("IN_MIN", ParamDir::In, StdTypeRef::Real),
            p("IN_MAX", ParamDir::In, StdTypeRef::Real),
            p("OUT_MIN", ParamDir::In, StdTypeRef::Real),
            p("OUT_MAX", ParamDir::In, StdTypeRef::Real),
        ],
        return_type: StdTypeRef::Real,
    },
    StdFnSig {
        name: "SQRT",
        params: &[p("X", ParamDir::In, StdTypeRef::Lreal)],
        return_type: StdTypeRef::Lreal,
    },
    StdFnSig {
        name: "SIN",
        params: &[p("X", ParamDir::In, StdTypeRef::Lreal)],
        return_type: StdTypeRef::Lreal,
    },
    StdFnSig {
        name: "COS",
        params: &[p("X", ParamDir::In, StdTypeRef::Lreal)],
        return_type: StdTypeRef::Lreal,
    },
    StdFnSig {
        name: "TAN",
        params: &[p("X", ParamDir::In, StdTypeRef::Lreal)],
        return_type: StdTypeRef::Lreal,
    },
    StdFnSig {
        name: "ATAN2",
        params: &[
            p("Y", ParamDir::In, StdTypeRef::Lreal),
            p("X", ParamDir::In, StdTypeRef::Lreal),
        ],
        return_type: StdTypeRef::Lreal,
    },
];

// --- std.iec.convert ---
static IEC_CONVERT_FNS: &[StdFnSig] = &[
    StdFnSig {
        name: "BOOL_TO_INT",
        params: &[p("IN", ParamDir::In, StdTypeRef::Bool)],
        return_type: StdTypeRef::Int,
    },
    StdFnSig {
        name: "INT_TO_DINT",
        params: &[p("IN", ParamDir::In, StdTypeRef::Int)],
        return_type: StdTypeRef::Dint,
    },
    StdFnSig {
        name: "DINT_TO_REAL",
        params: &[p("IN", ParamDir::In, StdTypeRef::Dint)],
        return_type: StdTypeRef::Real,
    },
    StdFnSig {
        name: "REAL_TO_DINT",
        params: &[p("IN", ParamDir::In, StdTypeRef::Real)],
        return_type: StdTypeRef::Dint,
    },
    StdFnSig {
        name: "ANALOG_TO_CODE",
        params: &[
            p("VAL", ParamDir::In, StdTypeRef::Analog),
            p("CODEC", ParamDir::In, StdTypeRef::String),
        ],
        return_type: StdTypeRef::Code,
    },
    StdFnSig {
        name: "CODE_TO_ANALOG",
        params: &[
            p("CODE", ParamDir::In, StdTypeRef::Code),
            p("CODEC", ParamDir::In, StdTypeRef::String),
        ],
        return_type: StdTypeRef::Analog,
    },
    StdFnSig {
        name: "ANALOG_TO_DUTY",
        params: &[p("VAL", ParamDir::In, StdTypeRef::Analog)],
        return_type: StdTypeRef::Duty,
    },
    StdFnSig {
        name: "TIME_DIFF",
        params: &[
            p("NOW", ParamDir::In, StdTypeRef::Time),
            p("PREV", ParamDir::In, StdTypeRef::Time),
        ],
        return_type: StdTypeRef::Time,
    },
];

// --- std.iec.comm ---
static IEC_COMM_FBS: &[StdFbSig] = &[
    StdFbSig {
        name: "UART_SEND",
        fields: &[
            p("REQ", ParamDir::In, StdTypeRef::Bool),
            p("DATA", ParamDir::In, StdTypeRef::String),
            p("DONE", ParamDir::Out, StdTypeRef::Bool),
            p("ERR", ParamDir::Out, StdTypeRef::Bool),
        ],
    },
    StdFbSig {
        name: "UART_RECV",
        fields: &[
            p("EN_R", ParamDir::In, StdTypeRef::Bool),
            p("NDR", ParamDir::Out, StdTypeRef::Bool),
            p("DATA", ParamDir::Out, StdTypeRef::String),
            p("ERR", ParamDir::Out, StdTypeRef::Bool),
        ],
    },
    StdFbSig {
        name: "I2C_XFER",
        fields: &[
            p("REQ", ParamDir::In, StdTypeRef::Bool),
            p("ADDR", ParamDir::In, StdTypeRef::Usint),
            p("DONE", ParamDir::Out, StdTypeRef::Bool),
            p("ERR", ParamDir::Out, StdTypeRef::Bool),
        ],
    },
    StdFbSig {
        name: "SPI_XFER",
        fields: &[
            p("REQ", ParamDir::In, StdTypeRef::Bool),
            p("TX", ParamDir::In, StdTypeRef::Byte),
            p("RX", ParamDir::Out, StdTypeRef::Byte),
            p("DONE", ParamDir::Out, StdTypeRef::Bool),
        ],
    },
    StdFbSig {
        name: "MB_CLIENT",
        fields: &[
            p("REQ", ParamDir::In, StdTypeRef::Bool),
            p("DISCONNECT", ParamDir::In, StdTypeRef::Bool),
            p("REMOTE_IP", ParamDir::In, StdTypeRef::String),
            p("REMOTE_PORT", ParamDir::In, StdTypeRef::Uint),
            p("UNIT_ID", ParamDir::In, StdTypeRef::Usint),
            p("FC", ParamDir::In, StdTypeRef::Usint),
            p("ADDR", ParamDir::In, StdTypeRef::Uint),
            p("QTY", ParamDir::In, StdTypeRef::Uint),
            p("DATA_WR", ParamDir::In, StdTypeRef::Uint),
            p("DATA_RD", ParamDir::Out, StdTypeRef::Uint),
            p("DONE", ParamDir::Out, StdTypeRef::Bool),
            p("BUSY", ParamDir::Out, StdTypeRef::Bool),
            p("ERR", ParamDir::Out, StdTypeRef::Bool),
            p("STATUS", ParamDir::Out, StdTypeRef::Word),
        ],
    },
    StdFbSig {
        name: "MB_SERVER",
        fields: &[
            p("ENABLE", ParamDir::In, StdTypeRef::Bool),
            p("BIND_IP", ParamDir::In, StdTypeRef::String),
            p("PORT", ParamDir::In, StdTypeRef::Uint),
            p("UNIT_ID", ParamDir::In, StdTypeRef::Usint),
            p("DISCONNECT", ParamDir::In, StdTypeRef::Bool),
            p("NDR", ParamDir::Out, StdTypeRef::Bool),
            p("DR", ParamDir::Out, StdTypeRef::Bool),
            p("CONNECTED", ParamDir::Out, StdTypeRef::Bool),
            p("LAST_FC", ParamDir::Out, StdTypeRef::Usint),
            p("LAST_ADDR", ParamDir::Out, StdTypeRef::Uint),
            p("ERR", ParamDir::Out, StdTypeRef::Bool),
            p("STATUS", ParamDir::Out, StdTypeRef::Word),
        ],
    },
];

// --- std.arduino.gpio ---
static ARDUINO_GPIO_FNS: &[StdFnSig] = &[
    StdFnSig {
        name: "DIGITAL_READ",
        params: &[p("PIN", ParamDir::In, StdTypeRef::PinRef)],
        return_type: StdTypeRef::Bool,
    },
    StdFnSig {
        name: "DIGITAL_WRITE",
        params: &[
            p("PIN", ParamDir::In, StdTypeRef::PinRef),
            p("VAL", ParamDir::In, StdTypeRef::Bool),
        ],
        return_type: StdTypeRef::Void,
    },
    StdFnSig {
        name: "ADC_CODE",
        params: &[p("PIN", ParamDir::In, StdTypeRef::PinRef)],
        return_type: StdTypeRef::Code,
    },
    StdFnSig {
        name: "PWM_WRITE",
        params: &[
            p("PIN", ParamDir::In, StdTypeRef::PinRef),
            p("DUTY", ParamDir::In, StdTypeRef::Duty),
        ],
        return_type: StdTypeRef::Void,
    },
    StdFnSig {
        name: "ANALOG_READ_RESOLUTION",
        params: &[p("BITS", ParamDir::In, StdTypeRef::Usint)],
        return_type: StdTypeRef::Void,
    },
];

// --- std.arduino.time ---
static ARDUINO_TIME_FNS: &[StdFnSig] = &[
    StdFnSig {
        name: "WAIT_TIME",
        params: &[p("DURATION_MS", ParamDir::In, StdTypeRef::Udint)],
        return_type: StdTypeRef::Void,
    },
    StdFnSig {
        name: "WAIT_TIME_US",
        params: &[p("DURATION_US", ParamDir::In, StdTypeRef::Ulint)],
        return_type: StdTypeRef::Void,
    },
    StdFnSig {
        name: "MILLIS",
        params: &[],
        return_type: StdTypeRef::Udint,
    },
    StdFnSig {
        name: "MICROS",
        params: &[],
        return_type: StdTypeRef::Udint,
    },
];

// --- std.plc.scale ---
static PLC_SCALE_FNS: &[StdFnSig] = &[
    StdFnSig {
        name: "NORM_X",
        params: &[
            p("MIN", ParamDir::In, StdTypeRef::Real),
            p("VALUE", ParamDir::In, StdTypeRef::Real),
            p("MAX", ParamDir::In, StdTypeRef::Real),
        ],
        return_type: StdTypeRef::Real,
    },
    StdFnSig {
        name: "SCALE_X",
        params: &[
            p("MIN", ParamDir::In, StdTypeRef::Real),
            p("VALUE", ParamDir::In, StdTypeRef::Real),
            p("MAX", ParamDir::In, StdTypeRef::Real),
        ],
        return_type: StdTypeRef::Real,
    },
];

static BUILTIN_MODULES: &[StdModule] = &[
    StdModule {
        path: "std.iec.timers",
        domain: StdDomain::Iec,
        functions: &[],
        function_blocks: IEC_TIMER_FBS,
    },
    StdModule {
        path: "std.iec.counters",
        domain: StdDomain::Iec,
        functions: &[],
        function_blocks: IEC_COUNTER_FBS,
    },
    StdModule {
        path: "std.iec.edges",
        domain: StdDomain::Iec,
        functions: &[],
        function_blocks: IEC_EDGE_FBS,
    },
    StdModule {
        path: "std.iec.bistables",
        domain: StdDomain::Iec,
        functions: &[],
        function_blocks: IEC_BISTABLE_FBS,
    },
    StdModule {
        path: "std.iec.math",
        domain: StdDomain::Iec,
        functions: IEC_MATH_FNS,
        function_blocks: &[],
    },
    StdModule {
        path: "std.iec.convert",
        domain: StdDomain::Iec,
        functions: IEC_CONVERT_FNS,
        function_blocks: &[],
    },
    StdModule {
        path: "std.iec.comm",
        domain: StdDomain::Iec,
        functions: &[],
        function_blocks: IEC_COMM_FBS,
    },
    StdModule {
        path: "std.arduino.gpio",
        domain: StdDomain::Arduino,
        functions: ARDUINO_GPIO_FNS,
        function_blocks: &[],
    },
    StdModule {
        path: "std.arduino.time",
        domain: StdDomain::Arduino,
        functions: ARDUINO_TIME_FNS,
        function_blocks: &[],
    },
    StdModule {
        path: "std.plc.scale",
        domain: StdDomain::Plc,
        functions: PLC_SCALE_FNS,
        function_blocks: &[],
    },
];

/// Authoritative domain-split standard library registry.
#[derive(Debug, Clone)]
pub struct StdCatalog {
    modules: BTreeMap<&'static str, &'static StdModule>,
}

impl Default for StdCatalog {
    fn default() -> Self {
        Self::builtin()
    }
}

impl StdCatalog {
    /// Builds the deterministic built-in standard library catalog.
    #[must_use]
    pub fn builtin() -> Self {
        let mut modules = BTreeMap::new();
        for module in BUILTIN_MODULES {
            modules.insert(module.path, module);
        }
        Self { modules }
    }

    /// Looks up a standard module by full path (e.g., `"std.iec.timers"`).
    #[must_use]
    pub fn get_module(&self, path: &str) -> Option<&'static StdModule> {
        self.modules.get(path).copied()
    }

    /// Returns all registered modules in deterministic lexicographic order.
    pub fn modules(&self) -> impl Iterator<Item = &'static StdModule> + '_ {
        self.modules.values().copied()
    }

    /// Returns all module paths belonging to a given [`StdDomain`].
    pub fn modules_for_domain(&self, domain: StdDomain) -> Vec<&'static str> {
        self.modules
            .values()
            .filter(|m| m.domain == domain)
            .map(|m| m.path)
            .collect()
    }

    /// Resolves `symbol_name` strictly within the explicitly `imported_modules` set.
    ///
    /// Supports both exact module paths (e.g. `"std.iec.timers"`) and domain wildcards
    /// (e.g. `"std.iec.*"` or `"std.arduino.*"`). Symbols from non-imported domains
    /// are NEVER visible.
    #[must_use]
    pub fn resolve_symbol(
        &self,
        imported_modules: &[&str],
        symbol_name: &str,
    ) -> Option<StdSymbol> {
        for &import in imported_modules {
            if let Some(prefix) = import.strip_suffix(".*") {
                for (&path, &module) in &self.modules {
                    if path.starts_with(prefix)
                        && path.as_bytes().get(prefix.len()) == Some(&b'.')
                        && let Some(sym) = module.get_symbol(symbol_name)
                    {
                        return Some(sym);
                    }
                }
            } else if let Some(&module) = self.modules.get(import)
                && let Some(sym) = module.get_symbol(symbol_name)
            {
                return Some(sym);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_plc_imports_cannot_resolve_arduino_symbols() {
        let cat = StdCatalog::builtin();
        let plc_imports = &["std.iec.*"];

        assert!(cat.resolve_symbol(plc_imports, "TON").is_some());
        assert!(cat.resolve_symbol(plc_imports, "CTUD").is_some());
        assert!(cat.resolve_symbol(plc_imports, "ANALOG_TO_CODE").is_some());
        assert!(cat.resolve_symbol(plc_imports, "UART_SEND").is_some());

        // Platform isolation invariant (01 §6.1): Arduino & PLC platform APIs MUST NOT leak!
        assert_eq!(cat.resolve_symbol(plc_imports, "DIGITAL_READ"), None);
        assert_eq!(cat.resolve_symbol(plc_imports, "MILLIS"), None);
        assert_eq!(cat.resolve_symbol(plc_imports, "NORM_X"), None);
    }

    #[test]
    fn explicit_platform_imports_resolve_cleanly() {
        let cat = StdCatalog::builtin();
        let arduino_imports = &["std.iec.*", "std.arduino.*"];
        assert!(
            cat.resolve_symbol(arduino_imports, "DIGITAL_READ")
                .is_some()
        );
        assert!(
            cat.resolve_symbol(arduino_imports, "WAIT_TIME_US")
                .is_some()
        );
        assert_eq!(cat.resolve_symbol(arduino_imports, "NORM_X"), None);

        let plc_ext_imports = &["std.iec.timers", "std.plc.scale"];
        assert!(cat.resolve_symbol(plc_ext_imports, "TON").is_some());
        assert!(cat.resolve_symbol(plc_ext_imports, "NORM_X").is_some());
        assert_eq!(cat.resolve_symbol(plc_ext_imports, "CTU"), None);
        assert_eq!(cat.resolve_symbol(plc_ext_imports, "MILLIS"), None);
    }

    #[test]
    fn all_modules_match_their_domain_prefix() {
        let cat = StdCatalog::builtin();
        for module in cat.modules() {
            let expected = format!("{}.", module.domain.prefix());
            assert!(
                module.path.starts_with(&expected),
                "module `{}` does not match domain prefix `{expected}`",
                module.path
            );
        }
    }
}
