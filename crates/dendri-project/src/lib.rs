//! Strict single-file multi-board `.toml` project loader, canonical serializer,
//! and atomic file persistence (`01-asl-ir-and-project-schema.md` §6.3, ADR-006).
//!
//! Every wire table enforces `#[serde(deny_unknown_fields)]` and parses canonical
//! expressions through `dendri-expr` directly into the typed `dendri-ast::Expr` IR.

mod convert;
mod wire;

use dendri_ast::Project;
use dendri_span::{Diagnostic, ErrorCode, FileId, SourceMap, Span};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

/// Error returned when loading or saving a `dendriForge` v2 `.toml` project.
#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("I/O failure on `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("project parse error: {}", .0.message)]
    Diagnostic(Box<Diagnostic>),
    #[error("failed to serialize project TOML: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl ProjectError {
    /// Extracts the underlying structured [`Diagnostic`] if this is a parse/schema failure.
    #[must_use]
    pub fn as_diagnostic(&self) -> Option<&Diagnostic> {
        match self {
            Self::Diagnostic(d) => Some(d),
            Self::Io { .. } | Self::Serialize(_) => None,
        }
    }
}

/// Loads a `dendriForge` v2 project from a TOML string into the canonical typed [`Project`] AST.
pub fn load_project_str(src: &str, file_id: Option<FileId>) -> Result<Project, ProjectError> {
    let raw: wire::RawProject = toml::from_str(src).map_err(|err| {
        let code = if err.message().contains("unknown field") {
            ErrorCode::ErrUnknownField
        } else {
            ErrorCode::ErrTomlSyntax
        };
        let mut diag = Diagnostic::new(code, err.message().to_owned());
        if let (Some(fid), Some(range)) = (file_id, err.span()) {
            let start = u32::try_from(range.start).unwrap_or(u32::MAX);
            let end = u32::try_from(range.end).unwrap_or(u32::MAX);
            diag = diag.with_location(fid, Span::new(start, end));
        }
        ProjectError::Diagnostic(Box::new(diag))
    })?;

    convert::raw_to_project(raw, file_id)
}

/// Loads a project from disk and registers its source text in `sources` for spanned diagnostics.
pub fn load_project_path(
    path: &Path,
    sources: &mut SourceMap,
) -> Result<(FileId, Project), ProjectError> {
    let text = std::fs::read_to_string(path).map_err(|source| ProjectError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let fid = sources.add_file(path.display().to_string(), text.clone());
    let project = load_project_str(&text, Some(fid))?;
    Ok((fid, project))
}

/// Serializes a typed [`Project`] into canonical pretty-printed TOML.
pub fn to_toml_string(project: &Project) -> Result<String, ProjectError> {
    let raw = convert::project_to_raw(project);
    let text = toml::to_string_pretty(&raw)?;
    Ok(text)
}

/// Atomically persists a [`Project`] to `path` via sibling temporary file + `sync_all` + `rename`
/// (`05-ide-and-roadmap.md` §1).
pub fn save_project_atomic(path: &Path, project: &Project) -> Result<(), ProjectError> {
    let content = to_toml_string(project)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project.toml");
    let pid = std::process::id();
    let tmp_path = parent.join(format!(".{file_name}.tmp.{pid}"));

    let write_res = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(source) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(ProjectError::Io {
            path: tmp_path.display().to_string(),
            source,
        });
    }

    std::fs::rename(&tmp_path, path).map_err(|source| {
        let _ = std::fs::remove_file(&tmp_path);
        ProjectError::Io {
            path: path.display().to_string(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{
        ActionQualifier, AuthoringLanguage, BoardLayout, CommentAnchor, CommentPlacement,
        CommentStyle, ExecutionKind, ForeignFidelity, PouBody,
    };
    use tempfile::TempDir;

    const FULL_MULTI_BOARD_TOML: &str = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Uno-PLC-Conveyor-Cell"
author = "Engineering Team"
description = "Heterogeneous MCU + PLC cell"

[[libraries]]
name = "conveyor_utils"
version = "1.0.0"
path = "libs/conveyor_utils.dendrilib"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"

[[packs]]
id = "arduino-servo"
version = "1.0.0"
path = "packs/arduino-servo/pack.toml"
sha256 = "1111111111111111111111111111111111111111111111111111111111111111"

[[comments]]
id = "c_p001"
anchor = { kind = "component", id = "btn_start" }
placement = "body"
text = " S1 — start button"
origin = { language = "asl", style = "visual" }

[[boards]]
id = "plc_main"
profile = "plc-1200-open-1214"
authoring_language = "ladder"
execution = "plc_scan"
scan_period_us = 10000

[boards.layout]
x = 2.0
y = 2.0
rotation = 0
mirror_x = false
mirror_y = false

[boards.program]
imports = ["std.iec.timers", "project.motor_ctrl"]

[[boards.program.hardware]]
id = "n_hw_start"
label = "START_BTN"
pin = "Ia.0"
kind = "digital_in"
pull = "none"
active_low = false

[[boards.program.hardware]]
id = "n_hw_press"
label = "PRESS_RAW"
pin = "IW64"
kind = "analog_in"
codec = "plc_span_27648"
range = { signal = "current", low = 4.0, high = 20.0, live_zero = true, hardware_normalizes_live_zero = true }

[[boards.program.data]]
id = "n_d_motor"
name = "motor_run"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.modules]]
id = "n_mod_motor"
name = "motor_ctrl"
imports = ["std.iec.timers"]
exports = ["RunStep"]

[[boards.program.modules.pous]]
id = "n_pou_chart"
name = "RunStep"
kind = { pou_kind = "fb" }
preferred_view = "sfc"
body_kind = "chart"

[boards.program.modules.pous.chart]
evolve_to_stability = false

[[boards.program.modules.pous.chart.steps]]
id = "n_s0"
name = "S0"
initial = true

[[boards.program.modules.pous.chart.steps.actions]]
id = "n_act_0"
qualifier = "N"
target = "motor_run"

[[boards.program.modules.pous.chart.transitions]]
id = "n_t0"
name = "T0"
from_steps = ["S0"]
to_steps = ["S0"]
priority = 0
receptivity = "START_BTN AND NOT false"

[[boards.program.pous]]
id = "n_pou_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
preferred_view = "ladder"
body_kind = "network"

[[boards.program.pous.network.rungs]]
id = "n_rung_0"
order = 0

[[boards.program.pous.network.rungs.cells]]
id = "n_cell_0"
row = 0
col = 0
kind = "contact"
contact_kind = "normally_open"
target = "START_BTN"

[[boards.program.pous.network.rungs.cells]]
id = "n_cell_1"
row = 0
col = 1
kind = "coil"
coil_kind = "normal"
target = "motor_run"

[[boards.program.pous]]
id = "n_pou_helper"
name = "Aux"
kind = { pou_kind = "fc", return_type = { kind = "scalar", ty = { name = "dint" } } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "foreign"
id = "n_0042"
language = "cpp"
fidelity = "F0"
text = "servo.write(angle);"
pack_hint = "arduino-servo"

[[boards.program.pous.body]]
kind = "return"
id = "n_0043"
value = "1 + 2 * 3"

[[boards.comments]]
id = "c_0007"
anchor = { kind = "node", id = "n_0042" }
placement = "leading"
text = " Drive actuator on start\n (24 V)"
origin = { language = "cpp", style = "line" }

[[boards.assumptions]]
node = "n_0042"
policy = "no_effect"
note = "Servo not modeled yet"

[boards.presentation]
eol_style = "lf"
[boards.presentation.blank_lines_before]
n_0042 = 1
[boards.presentation.origin_files]
n_mod_motor = "motor_ctrl.st"

[[components]]
id = "btn_start"
profile = "cmd-pushbutton-no-start"
label = "S1 Start"
[components.layout]
x = 34.0
y = 18.0
[components.params]

[[wires]]
id = "w1"
from = "btn_start.NO"
to = "plc_main.Ia.0"
signal = "digital"
waypoints = [[10.0, 20.0]]
"#;

    #[test]
    fn loads_and_losslessly_round_trips_full_multi_board_project() {
        let proj = load_project_str(FULL_MULTI_BOARD_TOML, None).unwrap();
        assert_eq!(proj.meta.schema_version, "2.0");
        assert_eq!(proj.libraries.len(), 1);
        assert_eq!(proj.packs.len(), 1);
        assert_eq!(proj.comments.len(), 1);
        assert_eq!(proj.comments[0].origin.style, CommentStyle::Visual);
        assert_eq!(
            proj.comments[0].anchor,
            CommentAnchor::Component {
                id: "btn_start".to_owned()
            }
        );

        let board = &proj.boards[0];
        assert_eq!(board.authoring_language, AuthoringLanguage::Ladder);
        assert_eq!(board.execution, ExecutionKind::PlcScan);
        assert_eq!(
            board.layout,
            BoardLayout {
                x: 2.0,
                y: 2.0,
                rotation: 0,
                mirror_x: false,
                mirror_y: false
            }
        );
        assert_eq!(board.comments[0].placement, CommentPlacement::Leading);
        assert_eq!(board.comments[0].text, " Drive actuator on start\n (24 V)");
        assert!(matches!(board.program.pous[0].body, PouBody::Network(_)));
        assert!(matches!(
            board.program.modules[0].pous[0].body,
            PouBody::Chart(_)
        ));
        if let PouBody::Chart(ref chart) = board.program.modules[0].pous[0].body {
            assert_eq!(chart.steps[0].actions[0].qualifier, ActionQualifier::N);
        }
        if let PouBody::Statements(ref stmts) = board.program.pous[1].body {
            assert!(matches!(
                stmts[0].kind,
                dendri_ast::StmtKind::Foreign {
                    fidelity: ForeignFidelity::F0,
                    ..
                }
            ));
        }

        // Round-trip: Project -> TOML -> Project
        let serialized = to_toml_string(&proj).unwrap();
        let reparsed = load_project_str(&serialized, None).unwrap();
        assert_eq!(reparsed.meta, proj.meta);
        assert_eq!(reparsed.comments.len(), proj.comments.len());
        assert_eq!(reparsed.comments[0].text, proj.comments[0].text);
        assert_eq!(reparsed.boards[0].comments[0].text, board.comments[0].text);
        assert_eq!(reparsed.boards[0].assumptions.len(), 1);
        assert_eq!(reparsed.boards[0].presentation, board.presentation);
        assert_eq!(reparsed.wires[0].from, "btn_start.NO");
    }

    #[test]
    fn rejects_unknown_fields_with_err_unknown_field_and_span() {
        let mut sm = SourceMap::new();
        let bad = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Bad"
rogue_unknown_key = true
"#;
        let fid = sm.add_file("bad.toml", bad);
        let err = load_project_str(bad, Some(fid)).unwrap_err();
        let diag = err.as_diagnostic().unwrap();
        assert_eq!(diag.code, ErrorCode::ErrUnknownField);
        assert!(diag.span.is_some());
    }

    #[test]
    fn atomic_save_and_reload_succeeds_cleanly() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cell.toml");
        let proj = load_project_str(FULL_MULTI_BOARD_TOML, None).unwrap();
        save_project_atomic(&path, &proj).unwrap();

        let mut sm = SourceMap::new();
        let (_fid, loaded) = load_project_path(&path, &mut sm).unwrap();
        assert_eq!(loaded.meta.name, "Uno-PLC-Conveyor-Cell");
    }
}
