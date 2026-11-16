//! Single-file simulation project TOML (hardware + wiring + sources + ASL).
//!
//! Extension: `.dendri.toml`. Embeds a full [`crate::program::Program`] under `[asl]`
//! so a project can be shared and reopened without external dependency loss.
//!
//! P1 text frontends (C++ / MicroPython / ST) fill [`SourceScript::text`] via
//! `dendri-frontends` (DEN-7). Ladder / SFC diagram TOML is filled by DEN-8.

use serde::{Deserialize, Serialize};

use crate::program::{Metadata, Program};

/// File extension for single-file simulation projects.
pub const PROJECT_EXTENSION: &str = "dendri.toml";

/// Portable simulation project: hardware instances, wiring, original sources, ASL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationProject {
    pub metadata: Metadata,
    /// Primary board instance (profile id from `content/profiles/boards/`).
    pub board: BoardInstance,
    #[serde(default)]
    pub components: Vec<ComponentInstance>,
    #[serde(default)]
    pub wiring: Vec<Wire>,
    /// Original user scripts / diagrams. Language tags are IR hooks for P1 frontends.
    #[serde(default)]
    pub sources: Vec<SourceScript>,
    /// ASL program representation (same schema as `.asl.toml`).
    pub asl: Program,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardInstance {
    /// Profile id, e.g. `arduino-uno-r3`.
    pub profile: String,
    #[serde(default = "default_board_id")]
    pub instance_id: String,
}

fn default_board_id() -> String {
    "board".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentInstance {
    pub instance_id: String,
    /// Profile id, e.g. `led`, `cmd-pushbutton-no-start`.
    pub profile: String,
    /// Free-form param overrides (color, etc.).
    #[serde(default)]
    pub params: toml::map::Map<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wire {
    /// Endpoint, e.g. `board.D13` or `led1.A`.
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub signal: Option<String>,
}

/// Original source script / visual document carried inside the project file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceScript {
    /// Frontend language hook — see [`SourceLanguage`].
    pub language: SourceLanguage,
    #[serde(default)]
    pub name: Option<String>,
    /// Inline source text (may be empty until the frontend lands).
    #[serde(default)]
    pub text: String,
    /// Optional path hint for tooling (not required for reopen — text is canonical).
    #[serde(default)]
    pub path_hint: Option<String>,
}

/// P1 language tags. Text frontends landed in DEN-7; Ladder/SFC = DEN-8; Rust = P2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLanguage {
    /// Embedded / Arduino-style C++ (DEN-7).
    Cpp,
    /// MicroPython (DEN-7).
    Micropython,
    /// IEC 61131-3 Structured Text (DEN-7).
    St,
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
            Self::Ladder => "ladder",
            Self::Sfc => "sfc",
            Self::Asl => "asl",
        }
    }
}
