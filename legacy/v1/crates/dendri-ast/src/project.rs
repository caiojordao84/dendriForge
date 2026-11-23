//! Single-file simulation project TOML (board + components + wiring + ASL).
//!
//! Extension: `.toml` (same as ASL program files; shape distinguishes them).
//! Embeds a full [`crate::program::Program`] under `[asl]`.
//!
//! Generated backends (C++ / MicroPython / ST / Ladder / SFC / Rust) are written
//! to `build/` on export — never embedded as `[[sources]]` in the project file.

use serde::{Deserialize, Serialize};

use crate::program::{Metadata, Program};

/// File extension for single-file simulation projects.
pub const PROJECT_EXTENSION: &str = "toml";

/// Portable simulation project: hardware instances, wiring, and ASL IR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationProject {
    pub metadata: Metadata,
    /// Primary board instance (profile id from `content/profiles/boards/`).
    pub board: BoardInstance,
    #[serde(default)]
    pub components: Vec<ComponentInstance>,
    #[serde(default)]
    pub wiring: Vec<Wire>,
    /// Deprecated ephemeral field — kept for in-memory sync/export only.
    /// Never serialized into project files (ASL is the sole source of truth).
    #[serde(default, skip_serializing)]
    pub sources: Vec<SourceScript>,
    /// ASL program representation (same schema as program `.toml`).
    pub asl: Program,
}

/// Canvas layout pose in **grid units** (1 u = 4 mm). Phase B1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayoutPose {
    /// Origin X (top-left of unrotated body AABB), grid units.
    #[serde(default)]
    pub x: f64,
    /// Origin Y (top-left of unrotated body AABB), grid units.
    #[serde(default)]
    pub y: f64,
    /// Rotation in degrees: `0 | 90 | 180 | 270`.
    #[serde(default, deserialize_with = "deserialize_rotation")]
    pub rotation: u16,
    #[serde(default)]
    pub mirror_x: bool,
    #[serde(default)]
    pub mirror_y: bool,
}

impl Default for LayoutPose {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            rotation: 0,
            mirror_x: false,
            mirror_y: false,
        }
    }
}

impl LayoutPose {
    pub fn with_xy(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            ..Self::default()
        }
    }

    /// Normalize to the nearest quadrant (`0/90/180/270`).
    pub fn normalize_rotation(deg: u16) -> u16 {
        let d = (deg as u32) % 360;
        let q = ((d as f64) / 90.0).round() as u32 % 4;
        (q * 90) as u16
    }

    pub fn rotate_cw_90(&mut self) {
        self.rotation = Self::normalize_rotation(self.rotation.wrapping_add(90));
    }
}

fn deserialize_rotation<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = u16::deserialize(deserializer)?;
    Ok(LayoutPose::normalize_rotation(v))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardInstance {
    /// Profile id, e.g. `arduino-uno-r3`.
    pub profile: String,
    #[serde(default = "default_board_id")]
    pub instance_id: String,
    /// Layout in grid units (Phase B1).
    #[serde(flatten)]
    pub layout: LayoutPose,
}

fn default_board_id() -> String {
    "board".into()
}

impl Default for BoardInstance {
    fn default() -> Self {
        Self {
            profile: "arduino-uno-r3".into(),
            instance_id: default_board_id(),
            layout: LayoutPose::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentInstance {
    pub instance_id: String,
    /// Profile id, e.g. `led`, `cmd-pushbutton-no-start`.
    pub profile: String,
    /// Optional human label for the instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Free-form param overrides (color, etc.).
    #[serde(default)]
    pub params: toml::map::Map<String, toml::Value>,
    /// Layout in grid units (Phase B1).
    #[serde(flatten)]
    pub layout: LayoutPose,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wire {
    /// Endpoint, e.g. `board.D13` or `led1.A`.
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    /// Intermediate waypoints in **grid units** (Manhattan). Phase B2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waypoints: Vec<[f64; 2]>,
}

impl Wire {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            signal: None,
            waypoints: Vec::new(),
        }
    }

    pub fn with_signal(mut self, signal: impl Into<String>) -> Self {
        self.signal = Some(signal.into());
        self
    }
}

/// Generated source / diagram artifact (export only — not stored in project TOML).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceScript {
    /// Frontend language hook — see [`SourceLanguage`].
    pub language: SourceLanguage,
    #[serde(default)]
    pub name: Option<String>,
    /// Generated source text.
    #[serde(default)]
    pub text: String,
    /// Optional path hint for tooling (`build/arduino/main.cpp`, …).
    #[serde(default)]
    pub path_hint: Option<String>,
}

/// Language tags for generators / export. Text P1 = DEN-7; Ladder/SFC = DEN-8; Rust = P2 (DEN-24).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLanguage {
    /// Embedded / Arduino-style C++ (DEN-7).
    Cpp,
    /// MicroPython (DEN-7).
    Micropython,
    /// IEC 61131-3 Structured Text (DEN-7).
    St,
    /// Dendri HAL Rust subset (DEN-24 / P2) — lowers to ASL; does not replace P1.
    Rust,
    /// IEC 61131-3 Ladder (DEN-8).
    Ladder,
    /// IEC 60848 / GRAFCET SFC (DEN-8).
    Sfc,
    /// ASL itself (already the IR under `[asl]`).
    Asl,
}

impl SourceLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpp => "cpp",
            Self::Micropython => "micropython",
            Self::St => "st",
            Self::Rust => "rust",
            Self::Ladder => "ladder",
            Self::Sfc => "sfc",
            Self::Asl => "asl",
        }
    }

    /// Relative export path under `build/`.
    pub fn build_path_hint(self) -> &'static str {
        match self {
            Self::Cpp => "build/arduino/main.cpp",
            Self::Micropython => "build/micropython/main.py",
            Self::St => "build/st/main.st",
            Self::Rust => "build/rust/main.rs",
            Self::Ladder => "build/ladder/ladder.toml",
            Self::Sfc => "build/sfc/sfc.toml",
            Self::Asl => "build/asl/program.toml",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_roundtrip_toml() {
        let board = BoardInstance {
            profile: "arduino-uno-r3".into(),
            instance_id: "board".into(),
            layout: LayoutPose {
                x: 1.0,
                y: 2.0,
                rotation: 90,
                mirror_x: true,
                mirror_y: false,
            },
        };
        let text = toml::to_string(&board).unwrap();
        assert!(text.contains("x = 1.0") || text.contains("x = 1"));
        assert!(text.contains("rotation = 90"));
        assert!(text.contains("mirror_x = true"));
        let back: BoardInstance = toml::from_str(&text).unwrap();
        assert_eq!(back.layout.rotation, 90);
        assert!(back.layout.mirror_x);
        assert!((back.layout.x - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rotation_snaps_to_quadrant() {
        assert_eq!(LayoutPose::normalize_rotation(45), 90);
        assert_eq!(LayoutPose::normalize_rotation(0), 0);
        assert_eq!(LayoutPose::normalize_rotation(270), 270);
        assert_eq!(LayoutPose::normalize_rotation(360), 0);
    }

    #[test]
    fn wire_waypoints_roundtrip_toml() {
        let w = Wire {
            from: "board.D13".into(),
            to: "led1.A".into(),
            signal: Some("digital".into()),
            waypoints: vec![[8.0, 0.0], [8.0, 2.0]],
        };
        let text = toml::to_string(&w).unwrap();
        assert!(text.contains("waypoints"));
        let back: Wire = toml::from_str(&text).unwrap();
        assert_eq!(back.waypoints.len(), 2);
        assert!((back.waypoints[0][0] - 8.0).abs() < 1e-9);
    }

    #[test]
    fn wire_without_waypoints_loads() {
        let w: Wire = toml::from_str(
            r#"
from = "board.D13"
to = "led1.A"
signal = "digital"
"#,
        )
        .unwrap();
        assert!(w.waypoints.is_empty());
        assert_eq!(w.signal.as_deref(), Some("digital"));
    }
}
