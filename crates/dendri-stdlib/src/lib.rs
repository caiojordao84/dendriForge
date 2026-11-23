//! Domain-split ASL v2 standard library (`std.iec.*` vs platform modules)
//! and validated engineering standards registry (`assets/standards.toml`).
//!
//! Layer 0 crate: zero internal workspace dependencies.

pub mod catalog;
pub mod standards;

pub use catalog::{
    ParamDir, StdCatalog, StdDomain, StdFbSig, StdFnSig, StdModule, StdParam, StdSymbol, StdTypeRef,
};
pub use standards::{StandardEntry, StandardTier, StandardsError, StandardsRegistry};
