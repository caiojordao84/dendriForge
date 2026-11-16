//! Board / component / screen profile loader.
//!
//! Phase 1 loads the visual-MVP trio: `arduino-uno-r3`, `led`,
//! `cmd-pushbutton-no-start`. SVG paths are recorded as hooks; missing SVGs
//! warn rather than hard-fail (see `docs/phase-0/svg-readiness.md`).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

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
    pub svg_required_ids: Vec<String>,
    #[serde(default)]
    pub component_profile_id: Option<String>,
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
    /// Repository root (for resolving `core/...` SVG paths).
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

    pub fn load_board(&self, id: &str) -> Result<ProfileDocument, ProfileError> {
        self.load_in("boards", id)
    }

    pub fn load_component(&self, id: &str) -> Result<ProfileDocument, ProfileError> {
        self.load_in("components", id)
    }

    fn load_in(&self, family: &str, id: &str) -> Result<ProfileDocument, ProfileError> {
        let path = self.root.join("profiles").join(family).join(format!("{id}.toml"));
        if !path.exists() {
            return Err(ProfileError::NotFound(path.display().to_string()));
        }
        let text = fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    /// Resolve SVG hook for a loaded profile (presence check only).
    pub fn svg_hook(&self, profile: &ProfileDocument) -> Option<SvgHook> {
        let relative = profile
            .dendri
            .as_ref()
            .and_then(|d| d.svg_path.clone())
            .or_else(|| {
                profile.identification.image.as_ref().map(|img| {
                    // Heuristic: boards vs components by category/family.
                    if profile.identification.family.is_some() {
                        format!("core/components/{img}")
                    } else {
                        format!("core/boards/{img}")
                    }
                })
            })?;
        let required_ids = profile
            .dendri
            .as_ref()
            .map(|d| d.svg_required_ids.clone())
            .unwrap_or_default();
        let present = self.repo_root.join(&relative).exists();
        Some(SvgHook {
            profile_id: profile.identification.id.clone(),
            relative_path: relative,
            required_ids,
            present,
        })
    }

    /// Load the locked visual-MVP set and return SVG hooks.
    pub fn load_visual_mvp(&self) -> Result<VisualMvp, ProfileError> {
        let board = self.load_board("arduino-uno-r3")?;
        let led = self.load_component("led")?;
        let button = self.load_component("cmd-pushbutton-no-start")?;
        Ok(VisualMvp {
            board_hook: self.svg_hook(&board),
            led_hook: self.svg_hook(&led),
            button_hook: self.svg_hook(&button),
            board,
            led,
            button,
        })
    }
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
        ProfileStore::from_repo_root(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        )
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
        let led_hook = mvp.led_hook.unwrap();
        assert!(led_hook.present, "led.svg should exist");
        let btn = mvp.button_hook.unwrap();
        assert!(
            btn.present,
            "pushbutton SVG should exist at core/components/cmd-pushbutton-no-start.svg"
        );
        assert_eq!(btn.relative_path, "core/components/cmd-pushbutton-no-start.svg");
    }
}
