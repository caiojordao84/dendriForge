//! Hash-locked `assets/packs/*.pack.toml` and `checksums.toml` verification gate (`DEF-008`).

use dendri_frontend_core::{PackChecksums, PackManifest};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Summary report emitted by a passing `dendri packs-check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacksCheckReport {
    pub packs_verified: usize,
    pub total_symbols: usize,
    pub pack_ids: Vec<String>,
}

/// Errors reported by `dendri packs-check` or verified pack loading.
#[derive(Debug, Error)]
pub enum PacksCheckError {
    #[error("I/O error reading `{path}`: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("pack verification failed with {0:?}")]
    Violations(Vec<String>),
}

/// Loads and verifies every pack in `<assets_dir>/packs/` against `checksums.toml`.
///
/// Enforces strict 1:1 bijection between `*.pack.toml` files on disk and entries in
/// `checksums.toml`, strict schema validation, and exact SHA-256 digest matching.
pub fn load_verified_packs(
    assets_dir: &Path,
) -> Result<(Vec<PackManifest>, PacksCheckReport), PacksCheckError> {
    let packs_dir = assets_dir.join("packs");
    let checksums_path = packs_dir.join("checksums.toml");
    let checksums_raw =
        fs::read_to_string(&checksums_path).map_err(|source| PacksCheckError::Io {
            path: checksums_path.display().to_string(),
            source,
        })?;

    let checksums = match PackChecksums::parse_str(&checksums_raw) {
        Ok(c) => c,
        Err(err) => {
            return Err(PacksCheckError::Violations(vec![format!(
                "invalid `{}`: {err}",
                checksums_path.display()
            )]));
        }
    };

    let mut violations: Vec<String> = Vec::new();
    let mut disk_pack_files: BTreeSet<String> = BTreeSet::new();

    let entries = fs::read_dir(&packs_dir).map_err(|source| PacksCheckError::Io {
        path: packs_dir.display().to_string(),
        source,
    })?;
    for entry_res in entries {
        let entry = entry_res.map_err(|source| PacksCheckError::Io {
            path: packs_dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if let Some(fname) = path.file_name().and_then(|n| n.to_str())
            && fname.ends_with(".pack.toml")
        {
            disk_pack_files.insert(fname.to_owned());
        }
    }

    if checksums.packs.is_empty() {
        violations.push("`checksums.toml` declares zero packs".to_owned());
    }

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut verified_files: BTreeSet<String> = BTreeSet::new();
    let mut manifests: Vec<PackManifest> = Vec::new();

    for lock in &checksums.packs {
        if !seen_ids.insert(lock.id.clone()) {
            violations.push(format!("duplicate pack id `{}` in checksums.toml", lock.id));
            continue;
        }
        let expected_file = format!("{}.pack.toml", lock.id);
        let pack_path = packs_dir.join(&expected_file);
        if !pack_path.is_file() {
            violations.push(format!(
                "pack `{}` listed in checksums.toml is missing file `{}`",
                lock.id,
                pack_path.display()
            ));
            continue;
        }
        verified_files.insert(expected_file);
        match PackManifest::from_file_verified(&pack_path, lock) {
            Ok(manifest) => manifests.push(manifest),
            Err(err) => violations.push(format!("pack `{}` failed verification: {err}", lock.id)),
        }
    }

    for disk_file in &disk_pack_files {
        if !verified_files.contains(disk_file) {
            violations.push(format!(
                "untracked pack file `{}` is missing from `checksums.toml`",
                packs_dir.join(disk_file).display()
            ));
        }
    }

    if !violations.is_empty() {
        return Err(PacksCheckError::Violations(violations));
    }

    let total_symbols: usize = manifests.iter().map(|m| m.symbols.len()).sum();
    let pack_ids: Vec<String> = manifests.iter().map(|m| m.pack.id.clone()).collect();
    let report = PacksCheckReport {
        packs_verified: manifests.len(),
        total_symbols,
        pack_ids,
    };
    Ok((manifests, report))
}

/// Runs the `dendri packs-check` verification gate on `<assets_dir>/packs/`.
pub fn run_packs_check(assets_dir: &Path) -> Result<PacksCheckReport, PacksCheckError> {
    let (_, report) = load_verified_packs(assets_dir)?;
    Ok(report)
}
