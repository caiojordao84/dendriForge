//! Source-neutral `[provenance]` & license compliance gate (`dendri provenance-check`, Gate 5).
//!
//! Enforces `03-asset-and-profile-contract.md` §4.1:
//! - `source = "original"` is the default norm; third-party fields are forbidden on original assets.
//! - `source = "third-party"` requires `origin`, SPDX `license`, `attribution`, `modified`,
//!   AND a matching notice in `THIRD_PARTY_NOTICES`.
//! - Any asset with `ml_restricted = true` is strictly excluded from AI/ML datasets and corpora.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Provenance origin of an asset (`03-asset-and-profile-contract.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceSource {
    /// Original drawing/profile authored for `dendriForge` (normative default).
    Original,
    /// Asset derived from a third-party work (requires license + `THIRD_PARTY_NOTICES`).
    ThirdParty,
}

/// Strict `[provenance]` metadata table (`#[serde(deny_unknown_fields)]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub source: ProvenanceSource,
    /// Required on schematic SVGs: closest IEC 60617 / ISO 1219 symbol ID for identification only.
    pub symbol_ref: Option<String>,
    pub reviewed_by: String,
    pub reviewed_on: String,
    /// `true` if any license term restricts machine-learning / AI corpus use.
    pub ml_restricted: bool,
    // Required ONLY when `source = "third-party"`:
    pub origin: Option<String>,
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub modified: Option<bool>,
}

impl Provenance {
    /// Returns `true` iff this asset may be included in AI/ML datasets, indexes, or embeddings.
    #[must_use]
    pub const fn is_ml_corpus_allowed(&self) -> bool {
        !self.ml_restricted
    }
}

#[derive(Debug, Deserialize)]
struct ProfileProvenanceEnvelope {
    provenance: Provenance,
}

/// Single provenance or notice-file violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceViolation {
    pub location: String,
    pub message: String,
}

impl fmt::Display for ProvenanceViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

/// Report produced by [`run_provenance_check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenanceReport {
    pub checked_profiles: usize,
    pub third_party_profiles: usize,
    pub ml_restricted_profiles: usize,
}

/// Error returned by provenance parsing or validation.
#[derive(Debug, Error)]
pub enum ProvenanceError {
    #[error("failed to read `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse `[provenance]` TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("provenance check failed with {} violation(s)", .0.len())]
    Violations(Vec<ProvenanceViolation>),
}

/// Parses a standalone `[provenance]` TOML snippet strictly (used by unit tests and fuzzers).
pub fn parse_provenance_str(input: &str) -> Result<Provenance, ProvenanceError> {
    let wrapped: ProfileProvenanceEnvelope = toml::from_str(input)?;
    Ok(wrapped.provenance)
}

/// Validates a single [`Provenance`] block against `03-asset-and-profile-contract.md` §4.1.
pub fn validate_provenance(
    location: &str,
    prov: &Provenance,
    require_symbol_ref: bool,
    third_party_notices: &str,
    violations: &mut Vec<ProvenanceViolation>,
) {
    if prov.reviewed_by.trim().is_empty() {
        violations.push(ProvenanceViolation {
            location: location.to_owned(),
            message: "`provenance.reviewed_by` must be non-empty".to_owned(),
        });
    }
    if !is_iso_date(&prov.reviewed_on) {
        violations.push(ProvenanceViolation {
            location: location.to_owned(),
            message: format!(
                "`provenance.reviewed_on` (`{}`) must use ISO YYYY-MM-DD format",
                prov.reviewed_on
            ),
        });
    }
    if require_symbol_ref
        && prov
            .symbol_ref
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
    {
        violations.push(ProvenanceViolation {
            location: location.to_owned(),
            message: "schematic asset MUST declare non-empty `provenance.symbol_ref`".to_owned(),
        });
    }

    match prov.source {
        ProvenanceSource::Original => {
            if prov.origin.is_some()
                || prov.license.is_some()
                || prov.attribution.is_some()
                || prov.modified.is_some()
            {
                violations.push(ProvenanceViolation {
                    location: location.to_owned(),
                    message: "original asset must not set third-party fields (`origin`, `license`, `attribution`, `modified`)".to_owned(),
                });
            }
        }
        ProvenanceSource::ThirdParty => {
            let origin = prov.origin.as_deref().unwrap_or("").trim();
            if origin.is_empty() {
                violations.push(ProvenanceViolation {
                    location: location.to_owned(),
                    message: "third-party asset requires non-empty `provenance.origin`".to_owned(),
                });
            } else if !third_party_notices.contains(origin) {
                violations.push(ProvenanceViolation {
                    location: location.to_owned(),
                    message: format!(
                        "third-party origin `{origin}` is missing a matching entry in THIRD_PARTY_NOTICES"
                    ),
                });
            }
            if prov.license.as_deref().is_none_or(|s| s.trim().is_empty()) {
                violations.push(ProvenanceViolation {
                    location: location.to_owned(),
                    message: "third-party asset requires a non-empty SPDX `provenance.license`"
                        .to_owned(),
                });
            }
            if prov
                .attribution
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
            {
                violations.push(ProvenanceViolation {
                    location: location.to_owned(),
                    message: "third-party asset requires non-empty `provenance.attribution`"
                        .to_owned(),
                });
            }
            if prov.modified.is_none() {
                violations.push(ProvenanceViolation {
                    location: location.to_owned(),
                    message: "third-party asset requires explicit boolean `provenance.modified`"
                        .to_owned(),
                });
            }
        }
    }
}

/// Runs repository-wide provenance and `THIRD_PARTY_NOTICES` verification.
pub fn run_provenance_check(
    workspace_root: &Path,
    assets_dir: &Path,
) -> Result<ProvenanceReport, ProvenanceError> {
    let notices_path = workspace_root.join("THIRD_PARTY_NOTICES");
    let notices_text =
        std::fs::read_to_string(&notices_path).map_err(|source| ProvenanceError::Io {
            path: notices_path.display().to_string(),
            source,
        })?;

    let mut violations = Vec::new();
    let mut checked_profiles = 0usize;
    let mut third_party_profiles = 0usize;
    let mut ml_restricted_profiles = 0usize;

    for (subdir, require_symbol_ref) in
        [("components", true), ("boards", false), ("screens", false)]
    {
        let dir = assets_dir.join(subdir);
        if !dir.is_dir() {
            continue;
        }
        let mut profiles = Vec::new();
        collect_profile_tomls(&dir, &mut profiles)?;
        profiles.sort();

        for profile_path in profiles {
            checked_profiles = checked_profiles.saturating_add(1);
            let text =
                std::fs::read_to_string(&profile_path).map_err(|source| ProvenanceError::Io {
                    path: profile_path.display().to_string(),
                    source,
                })?;
            let rel = profile_path
                .strip_prefix(workspace_root)
                .unwrap_or(&profile_path)
                .display()
                .to_string();
            match parse_provenance_str(&text) {
                Ok(prov) => {
                    if prov.source == ProvenanceSource::ThirdParty {
                        third_party_profiles = third_party_profiles.saturating_add(1);
                    }
                    if !prov.is_ml_corpus_allowed() {
                        ml_restricted_profiles = ml_restricted_profiles.saturating_add(1);
                    }
                    validate_provenance(
                        &rel,
                        &prov,
                        require_symbol_ref,
                        &notices_text,
                        &mut violations,
                    );
                }
                Err(err) => {
                    violations.push(ProvenanceViolation {
                        location: rel,
                        message: err.to_string(),
                    });
                }
            }
        }
    }

    if !violations.is_empty() {
        return Err(ProvenanceError::Violations(violations));
    }

    Ok(ProvenanceReport {
        checked_profiles,
        third_party_profiles,
        ml_restricted_profiles,
    })
}

fn collect_profile_tomls(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ProvenanceError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ProvenanceError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for entry_res in entries {
        let entry = entry_res.map_err(|source| ProvenanceError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_profile_tomls(&path, out)?;
        } else if path.file_name().and_then(|s| s.to_str()) == Some("profile.toml") {
            out.push(path);
        }
    }
    Ok(())
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 {
        return false;
    }
    let digits_ok = b
        .get(0..4)
        .is_some_and(|y| y.iter().all(u8::is_ascii_digit))
        && b.get(4) == Some(&b'-')
        && b.get(5..7)
            .is_some_and(|m| m.iter().all(u8::is_ascii_digit))
        && b.get(7) == Some(&b'-')
        && b.get(8..10)
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    if !digits_ok {
        return false;
    }
    let month: u32 = s.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(0);
    let day: u32 = s.get(8..10).and_then(|d| d.parse().ok()).unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn accepts_original_component_provenance() {
        let prov = parse_provenance_str(
            r#"
[provenance]
source = "original"
symbol_ref = "IEC 60617-S00669"
reviewed_by = "caio"
reviewed_on = "2026-11-23"
ml_restricted = false
"#,
        )
        .unwrap();
        assert!(prov.is_ml_corpus_allowed());
        let mut violations = Vec::new();
        validate_provenance(
            "assets/components/led/profile.toml",
            &prov,
            true,
            "",
            &mut violations,
        );
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn rejects_original_with_third_party_fields_or_missing_symbol_ref() {
        let prov = parse_provenance_str(
            r#"
[provenance]
source = "original"
reviewed_by = "caio"
reviewed_on = "2026-11-23"
ml_restricted = false
license = "MIT"
"#,
        )
        .unwrap();
        let mut violations = Vec::new();
        validate_provenance("comp", &prov, true, "", &mut violations);
        assert_eq!(violations.len(), 2);
    }

    #[test]
    fn enforces_third_party_fields_notices_and_ml_exclusion() {
        let prov = parse_provenance_str(
            r#"
[provenance]
source = "third-party"
symbol_ref = "IEC 60617-S00213"
reviewed_by = "caio"
reviewed_on = "2026-11-23"
ml_restricted = true
origin = "Example Open Symbols v1.2"
license = "CC-BY-4.0"
attribution = "Copyright Example Contributors"
modified = true
"#,
        )
        .unwrap();
        assert!(!prov.is_ml_corpus_allowed());

        // Missing notice fails
        let mut v_missing = Vec::new();
        validate_provenance("comp", &prov, true, "no notices", &mut v_missing);
        assert_eq!(v_missing.len(), 1);
        assert!(v_missing[0].message.contains("THIRD_PARTY_NOTICES"));

        // Present notice passes
        let mut v_ok = Vec::new();
        validate_provenance(
            "comp",
            &prov,
            true,
            "Example Open Symbols v1.2 — CC-BY-4.0",
            &mut v_ok,
        );
        assert!(v_ok.is_empty());
    }

    #[test]
    fn rejects_unknown_provenance_field() {
        let err = parse_provenance_str(
            r#"
[provenance]
source = "original"
reviewed_by = "caio"
reviewed_on = "2026-11-23"
ml_restricted = false
rogue_field = 123
"#,
        )
        .unwrap_err();
        assert!(matches!(err, ProvenanceError::Toml(_)));
    }

    #[test]
    fn repo_scan_verifies_third_party_notices_and_component_profiles() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("THIRD_PARTY_NOTICES"), "# Notices\n").unwrap();
        std::fs::create_dir_all(dir.path().join("assets/components/led")).unwrap();
        std::fs::write(
            dir.path().join("assets/components/led/profile.toml"),
            r#"
[provenance]
source = "original"
symbol_ref = "IEC 60617-S00669"
reviewed_by = "caio"
reviewed_on = "2026-11-23"
ml_restricted = false
"#,
        )
        .unwrap();

        let report = run_provenance_check(dir.path(), &dir.path().join("assets")).unwrap();
        assert_eq!(report.checked_profiles, 1);
        assert_eq!(report.third_party_profiles, 0);
        assert_eq!(report.ml_restricted_profiles, 0);
    }
}
