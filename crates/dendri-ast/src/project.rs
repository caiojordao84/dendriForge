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
    /// Optional human label for the instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
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
