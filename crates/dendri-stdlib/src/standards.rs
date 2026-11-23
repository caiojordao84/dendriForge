//! Strict loader and validator for `assets/standards.toml` (`03-asset-and-profile-contract.md` §7).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

/// Classification tier of an engineering standard in `assets/standards.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StandardTier {
    /// Core normative language/runtime standard (IEC 61131-3, IEC 60848, IEEE 754).
    Core,
    /// Graphical/document reference standard (IEC 60617, IEC 61082, IEC 81346).
    Reference,
    /// Component-level electrical/fluid standard (IEC 60947-5-1, IEC 60810, ISO 1219-1).
    Component,
    /// Industrial communication protocol standard (Modbus TCP).
    Protocol,
}

/// Single validated standard entry from `assets/standards.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardEntry {
    pub id: String,
    pub title: String,
    pub organization: String,
    pub tier: StandardTier,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStandardsFile {
    schema_version: String,
    standards: Vec<StandardEntry>,
}

/// Errors returned while loading or validating `assets/standards.toml`.
#[derive(Debug, Error)]
pub enum StandardsError {
    #[error("failed to read standards file `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse standards TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unsupported standards schema_version `{0}` (expected \"2.0\")")]
    UnsupportedVersion(String),
    #[error("standards registry cannot be empty")]
    EmptyRegistry,
    #[error("standard entry has empty `id` or `title`")]
    EmptyField,
    #[error("duplicate standard id `{0}` in standards registry")]
    DuplicateId(String),
}

/// Live validated registry of recognized engineering standards.
#[derive(Debug, Clone)]
pub struct StandardsRegistry {
    entries: BTreeMap<String, StandardEntry>,
}

impl StandardsRegistry {
    /// Parses and validates a `standards.toml` string with `#[serde(deny_unknown_fields)]`.
    pub fn load_from_str(content: &str) -> Result<Self, StandardsError> {
        let raw: RawStandardsFile = toml::from_str(content)?;
        if raw.schema_version != "2.0" {
            return Err(StandardsError::UnsupportedVersion(raw.schema_version));
        }
        if raw.standards.is_empty() {
            return Err(StandardsError::EmptyRegistry);
        }

        let mut entries = BTreeMap::new();
        for entry in raw.standards {
            if entry.id.trim().is_empty() || entry.title.trim().is_empty() {
                return Err(StandardsError::EmptyField);
            }
            if entries.contains_key(&entry.id) {
                return Err(StandardsError::DuplicateId(entry.id));
            }
            entries.insert(entry.id.clone(), entry);
        }
        Ok(Self { entries })
    }

    /// Loads and validates `standards.toml` from disk.
    pub fn load_from_path(path: &Path) -> Result<Self, StandardsError> {
        let text = std::fs::read_to_string(path).map_err(|source| StandardsError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::load_from_str(&text)
    }

    /// Returns `true` if `standard_id` is registered.
    #[must_use]
    pub fn contains(&self, standard_id: &str) -> bool {
        self.entries.contains_key(standard_id)
    }

    /// Returns the entry for `standard_id`, if registered.
    #[must_use]
    pub fn get(&self, standard_id: &str) -> Option<&StandardEntry> {
        self.entries.get(standard_id)
    }

    /// Number of registered standards.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if no standards are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_VALID: &str = r#"
schema_version = "2.0"

[[standards]]
id = "IEC 61131-3"
title = "Programmable controllers — Part 3"
organization = "IEC"
tier = "core"
scope = "Languages and FBs"
"#;

    #[test]
    fn parses_valid_standards_registry() {
        let reg = StandardsRegistry::load_from_str(SAMPLE_VALID).unwrap();
        assert_eq!(reg.len(), 1);
        assert!(reg.contains("IEC 61131-3"));
        assert!(!reg.contains("IEC 99999"));
        assert_eq!(reg.get("IEC 61131-3").unwrap().tier, StandardTier::Core);
    }

    #[test]
    fn rejects_unknown_fields_strictly() {
        let bad = r#"
schema_version = "2.0"
unexpected_key = true

[[standards]]
id = "IEC 61131-3"
title = "Part 3"
organization = "IEC"
tier = "core"
scope = "Core"
"#;
        let err = StandardsRegistry::load_from_str(bad).unwrap_err();
        assert!(matches!(err, StandardsError::Toml(_)));
    }

    #[test]
    fn rejects_duplicate_standard_ids() {
        let dup = r#"
schema_version = "2.0"

[[standards]]
id = "IEC 60617"
title = "A"
organization = "IEC"
tier = "reference"
scope = "Symbols"

[[standards]]
id = "IEC 60617"
title = "B"
organization = "IEC"
tier = "reference"
scope = "Duplicate"
"#;
        let err = StandardsRegistry::load_from_str(dup).unwrap_err();
        assert!(matches!(err, StandardsError::DuplicateId(id) if id == "IEC 60617"));
    }

    #[test]
    fn rejects_wrong_schema_version_and_empty_registry() {
        let wrong_ver = r#"
schema_version = "1.0"
standards = []
"#;
        assert!(matches!(
            StandardsRegistry::load_from_str(wrong_ver),
            Err(StandardsError::UnsupportedVersion(_))
        ));

        let empty = r#"
schema_version = "2.0"
standards = []
"#;
        assert!(matches!(
            StandardsRegistry::load_from_str(empty),
            Err(StandardsError::EmptyRegistry)
        ));
    }
}
