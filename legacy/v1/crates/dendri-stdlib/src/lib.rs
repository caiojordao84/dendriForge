//! IEC 61131-3 stdlib catalog for ASL (Abstract Syntax Language).
//!
//! Data file: `data/catalog.toml` (compiled in via `include_str!`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub naming: String,
    pub ops: Vec<StdlibOp>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StdlibOp {
    pub canonical: String,
    pub category: String,
    pub signature: String,
    pub returns: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub iec_norm: Option<String>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();
static ALIAS_INDEX: OnceLock<HashMap<String, usize>> = OnceLock::new();
static CANON_INDEX: OnceLock<HashMap<String, usize>> = OnceLock::new();

const CATALOG_TOML: &str = include_str!("../data/catalog.toml");

pub fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        toml::from_str(CATALOG_TOML).expect("dendri-stdlib catalog.toml must parse")
    })
}

fn canon_index() -> &'static HashMap<String, usize> {
    CANON_INDEX.get_or_init(|| {
        catalog()
            .ops
            .iter()
            .enumerate()
            .map(|(i, op)| (op.canonical.clone(), i))
            .collect()
    })
}

fn alias_index() -> &'static HashMap<String, usize> {
    ALIAS_INDEX.get_or_init(|| {
        let mut m = HashMap::new();
        for (i, op) in catalog().ops.iter().enumerate() {
            for a in &op.aliases {
                m.insert(a.clone(), i);
            }
        }
        m
    })
}

/// Resolve alias or canonical name → IEC canonical identifier.
pub fn resolve_name(name: &str) -> Option<&'static str> {
    let idx = canon_index()
        .get(name)
        .copied()
        .or_else(|| alias_index().get(name).copied())?;
    Some(catalog().ops[idx].canonical.as_str())
}

pub fn lookup(canonical_or_alias: &str) -> Option<&'static StdlibOp> {
    let idx = canon_index()
        .get(canonical_or_alias)
        .copied()
        .or_else(|| alias_index().get(canonical_or_alias).copied())?;
    Some(&catalog().ops[idx])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_and_resolves_aliases() {
        let cat = catalog();
        assert!(cat.naming.contains("IEC 61131-3"));
        assert!(cat.ops.len() > 40);
        assert_eq!(resolve_name("digitalWrite"), Some("SET_BOOL"));
        assert_eq!(resolve_name("SET_BOOL"), Some("SET_BOOL"));
        assert_eq!(resolve_name("TON"), Some("TON"));
        assert!(lookup("CLAMP").is_some());
    }

    #[test]
    fn modbus_tcp_ops_resolve() {
        assert_eq!(resolve_name("CFG_MODBUS_TCP"), Some("CFG_MODBUS_TCP"));
        assert_eq!(resolve_name("modbusTcpBegin"), Some("CFG_MODBUS_TCP"));
        assert_eq!(resolve_name("MB_FC3"), Some("MB_READ_HOLDING"));
        assert_eq!(resolve_name("MB_FC6"), Some("MB_WRITE_HOLDING"));
        assert_eq!(resolve_name("MB_FC1"), Some("MB_READ_COIL"));
        assert_eq!(resolve_name("MB_FC5"), Some("MB_WRITE_COIL"));
        assert_eq!(resolve_name("MB_FC4"), Some("MB_READ_INPUT"));
        assert_eq!(resolve_name("MB_FC2"), Some("MB_READ_DISCRETE"));
        assert_eq!(lookup("CFG_MODBUS_TCP").unwrap().category, "modbus");
    }
}
