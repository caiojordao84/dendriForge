//! Board / component / screen profile loader.
//!
//! Loads TOML profiles from `content/profiles/{boards,components,screens}/`.
//! SVG hooks prefer `content/assets/svg/` (DEN-13 / DEN-14); missing assets warn
//! rather than hard-fail (see `docs/phase-0/svg-readiness.md`).
//!
//! Legacy TOON under `core/` remains read-only; use [`toon`] to migrate.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

pub mod toon;

pub use toon::{migrate_family, convert_toon_file, MigrateReport, ProfileFamily, ToonError};

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("profile not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileDocument {
    #[serde(default)]
    pub metadata: ProfileMetadata,
    pub identification: Identification,
    #[serde(default)]
    pub dendri: Option<DendriMeta>,
    #[serde(default)]
    pub simulation: Option<SimulationMeta>,
    /// Raw leftover tables preserved for forward compatibility.
    #[serde(flatten)]
    pub rest: toml::map::Map<String, toml::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProfileMetadata {
    #[serde(default)]
    pub project_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Identification {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DendriMeta {
    #[serde(default)]
    pub visual_mvp: bool,
    #[serde(default)]
    pub svg_path: Option<String>,
    #[serde(default)]
    pub svg_status: Option<String>,
    #[serde(default)]
    pub svg_required_ids: Vec<String>,
    #[serde(default)]
    pub svg_note: Option<String>,
    #[serde(default)]
    pub component_profile_id: Option<String>,
    #[serde(default)]
    pub board_profile_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SimulationMeta {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub states: Vec<String>,
}

/// SVG hook recorded for later visual sim (does not load the SVG bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgHook {
    pub profile_id: String,
    pub relative_path: String,
    pub required_ids: Vec<String>,
    pub present: bool,
}

#[derive(Debug, Clone)]
pub struct ProfileStore {
    root: PathBuf,
    /// Repository root (for resolving SVG paths).
    repo_root: PathBuf,
}

impl ProfileStore {
    /// `content_root` is typically `<repo>/content`.
    pub fn new(content_root: impl Into<PathBuf>) -> Self {
        let root = content_root.into();
        let repo_root = root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.clone());
        Self { root, repo_root }
    }

    pub fn from_repo_root(repo: impl Into<PathBuf>) -> Self {
        let repo_root = repo.into();
        Self {
            root: repo_root.join("content"),
            repo_root,
        }
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn content_root(&self) -> &Path {
        &self.root
    }

    pub fn load_board(&self, id: &str) -> Result<ProfileDocument, ProfileError> {
        self.load_in("boards", id)
    }

    pub fn load_component(&self, id: &str) -> Result<ProfileDocument, ProfileError> {
        self.load_in("components", id)
    }

    pub fn load_screen(&self, id: &str) -> Result<ProfileDocument, ProfileError> {
        self.load_in("screens", id)
    }

    fn load_in(&self, family: &str, id: &str) -> Result<ProfileDocument, ProfileError> {
        let path = self
            .root
            .join("profiles")
            .join(family)
            .join(format!("{id}.toml"));
        if !path.exists() {
            return Err(ProfileError::NotFound(path.display().to_string()));
        }
        let text = fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    /// List profile ids (file stems) under `content/profiles/{family}/`.
    pub fn list_ids(&self, family: &str) -> Result<Vec<String>, ProfileError> {
        let dir = self.root.join("profiles").join(family);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids: Vec<String> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("toml") {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Resolve SVG path preferring `content/assets/svg/`, then profile `svg_path`,
    /// then legacy `core/{family}/{image}` (DEN-13 / DEN-14).
    pub fn svg_hook_for(&self, family: &str, profile: &ProfileDocument) -> Option<SvgHook> {
        let required_ids = profile
            .dendri
            .as_ref()
            .map(|d| d.svg_required_ids.clone())
            .unwrap_or_default();
        let profile_id = profile.identification.id.clone();

        let image = profile
            .identification
            .image
            .clone()
            .unwrap_or_else(|| format!("{profile_id}.svg"));

        let preferred =
            format!("content/assets/svg/{family}/{image}");
        let declared = profile
            .dendri
            .as_ref()
            .and_then(|d| d.svg_path.clone());
        let legacy = format!("core/{family}/{image}");

        let mut candidates: Vec<String> = Vec::new();
        candidates.push(preferred.clone());
        if let Some(d) = declared {
            if d != preferred {
                candidates.push(d);
            }
        }
        if legacy != preferred {
            candidates.push(legacy);
        }
        // Siemens historical filename.
        if family == "boards" && image == "siemens-s7-1200-cpu-1214c.svg" {
            candidates.push("core/boards/siemens-s7-1200-cpu-1214c-corrigido.svg".into());
            candidates.push("content/assets/svg/boards/siemens-s7-1200-cpu-1214c-corrigido.svg".into());
        }

        for rel in &candidates {
            if self.repo_root.join(rel).exists() {
                return Some(SvgHook {
                    profile_id,
                    relative_path: rel.clone(),
                    required_ids,
                    present: true,
                });
            }
        }

        Some(SvgHook {
            profile_id,
            relative_path: preferred,
            required_ids,
            present: false,
        })
    }

    /// Resolve SVG hook for a loaded profile (presence check only).
    pub fn svg_hook(&self, profile: &ProfileDocument) -> Option<SvgHook> {
        let family = if profile
            .dendri
            .as_ref()
            .and_then(|d| d.board_profile_id.as_ref())
            .is_some()
            || profile.identification.family.is_none()
                && profile.identification.category.as_deref() != Some("instrument")
                && profile.identification.category.as_deref() != Some("function-block")
        {
            // Heuristic: boards lack `family`; components/screens set it.
            if profile.identification.family.is_some() {
                "components"
            } else {
                "boards"
            }
        } else if profile.identification.family.is_some() {
            "components"
        } else {
            "boards"
        };

        // Screens live under screens/ but share component-like identification.
        let family = if self
            .root
            .join("profiles")
            .join("screens")
            .join(format!("{}.toml", profile_file_stem(profile)))
            .exists()
        {
            "screens"
        } else {
            family
        };

        self.svg_hook_for(family, profile)
    }

    /// Load the locked visual-MVP set and return SVG hooks.
    pub fn load_visual_mvp(&self) -> Result<VisualMvp, ProfileError> {
        let board = self.load_board("arduino-uno-r3")?;
        let led = self.load_component("led")?;
        let button = self.load_component("cmd-pushbutton-no-start")?;
        Ok(VisualMvp {
            board_hook: self.svg_hook_for("boards", &board),
            led_hook: self.svg_hook_for("components", &led),
            button_hook: self.svg_hook_for("components", &button),
            board,
            led,
            button,
        })
    }

    /// Validate that every profile under `content/profiles/` parses.
    pub fn validate_all(&self) -> Result<ValidateAllReport, ProfileError> {
        let mut report = ValidateAllReport::default();
        for family in ["boards", "components", "screens"] {
            for id in self.list_ids(family)? {
                match self.load_in(family, &id) {
                    Ok(doc) => {
                        report.loaded += 1;
                        if let Some(hook) = self.svg_hook_for(family, &doc) {
                            if hook.present {
                                report.svg_present += 1;
                            } else {
                                report.svg_missing += 1;
                                report.missing_svgs.push(hook.relative_path);
                            }
                        }
                    }
                    Err(e) => {
                        report.errors.push(format!("{family}/{id}: {e}"));
                    }
                }
            }
        }
        Ok(report)
    }
}

fn profile_file_stem(profile: &ProfileDocument) -> String {
    profile
        .dendri
        .as_ref()
        .and_then(|d| {
            d.board_profile_id
                .clone()
                .or_else(|| d.component_profile_id.clone())
        })
        .unwrap_or_else(|| profile.identification.id.clone())
}

#[derive(Debug, Clone, Default)]
pub struct ValidateAllReport {
    pub loaded: usize,
    pub svg_present: usize,
    pub svg_missing: usize,
    pub missing_svgs: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct VisualMvp {
    pub board: ProfileDocument,
    pub led: ProfileDocument,
    pub button: ProfileDocument,
    pub board_hook: Option<SvgHook>,
    pub led_hook: Option<SvgHook>,
    pub button_hook: Option<SvgHook>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ProfileStore {
        ProfileStore::from_repo_root(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    #[test]
    fn loads_mvp_trio() {
        let s = store();
        let mvp = s.load_visual_mvp().unwrap();
        assert_eq!(mvp.board.identification.id, "arduino-uno");
        assert_eq!(mvp.led.identification.id, "led");
        assert_eq!(mvp.button.identification.id, "cmd-pushbutton-no-start");
        let board_hook = mvp.board_hook.unwrap();
        assert!(board_hook.present, "arduino-uno-r3.svg should exist");
        assert!(
            board_hook
                .relative_path
                .starts_with("content/assets/svg/boards/"),
            "board SVG should resolve under content/assets/svg/: {}",
            board_hook.relative_path
        );
        let led_hook = mvp.led_hook.unwrap();
        assert!(led_hook.present, "led.svg should exist");
        assert!(led_hook
            .relative_path
            .starts_with("content/assets/svg/components/"));
        let btn = mvp.button_hook.unwrap();
        assert!(
            btn.present,
            "pushbutton SVG should exist under content/assets/svg/"
        );
        assert_eq!(
            btn.relative_path,
            "content/assets/svg/components/cmd-pushbutton-no-start.svg"
        );
    }

    #[test]
    fn loads_all_migrated_profiles() {
        let s = store();
        let boards = s.list_ids("boards").unwrap();
        let components = s.list_ids("components").unwrap();
        let screens = s.list_ids("screens").unwrap();
        assert_eq!(boards.len(), 60, "all boards migrated");
        assert_eq!(components.len(), 150, "all components migrated");
        assert_eq!(screens.len(), 2, "all screens migrated");
        let report = s.validate_all().unwrap();
        assert!(
            report.errors.is_empty(),
            "profile load errors: {:?}",
            report.errors
        );
        assert_eq!(report.loaded, 212);
        assert!(report.svg_present >= 6);
        // Screens load through dedicated API.
        let screen = s.load_screen("ind-hmi-numeric-indicator").unwrap();
        assert_eq!(screen.identification.id, "ind-hmi-numeric-indicator");
    }
}
