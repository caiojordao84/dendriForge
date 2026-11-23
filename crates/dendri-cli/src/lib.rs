//! Reusable verification and governance library backing the `dendri` CLI.
//!
//! Zero `env!("CARGO_MANIFEST_DIR")` in production code (Invariant 6): runtime
//! paths resolve strictly from CLI flags, environment variables, executable
//! ancestry, or the current working directory.

pub mod ledger;
pub mod provenance;

use std::path::{Path, PathBuf};

/// Resolves the workspace/repository root at runtime without compile-time machine paths.
///
/// Precedence:
/// 1. Explicit `--root` argument.
/// 2. `DENDRIFORGE_ROOT` environment variable.
/// 3. Current working directory (or ancestor containing `docs/milestone.toml`).
#[must_use]
pub fn resolve_workspace_root(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Ok(env_root) = std::env::var("DENDRIFORGE_ROOT")
        && !env_root.trim().is_empty()
    {
        return PathBuf::from(env_root);
    }
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors() {
            if ancestor.join("docs").join("milestone.toml").is_file() {
                return ancestor.to_path_buf();
            }
        }
        return cwd;
    }
    PathBuf::from(".")
}

/// Resolves the runtime `assets/` directory (`05-ide-and-roadmap.md` §1).
///
/// Precedence:
/// 1. Explicit `--assets-dir` argument.
/// 2. `DENDRIFORGE_ASSETS` environment variable.
/// 3. `<exe_dir>/assets` if it exists.
/// 4. `<workspace_root>/assets`.
#[must_use]
pub fn resolve_assets_dir(explicit_assets: Option<&Path>, workspace_root: &Path) -> PathBuf {
    if let Some(p) = explicit_assets {
        return p.to_path_buf();
    }
    if let Ok(env_assets) = std::env::var("DENDRIFORGE_ASSETS")
        && !env_assets.trim().is_empty()
    {
        return PathBuf::from(env_assets);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        let candidate = exe_dir.join("assets");
        if candidate.is_dir() {
            return candidate;
        }
    }
    workspace_root.join("assets")
}
