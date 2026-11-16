//! Bidirectional real-time sync between diagram models, TOML source text, and ASL.
//!
//! # Design
//!
//! A [`SyncSession`] holds one coherent project snapshot. Every edit enters through
//! a typed channel and is projected to the other views deterministically:
//!
//! ```text
//!   DiagramModel  ←→  [[sources]].text (TOML)  ←→  [asl] Program
//!         │                    │                      │
//!         └──────── SyncSession (single writer) ──────┘
//! ```
//!
//! - **Diagram → TOML/ASL:** serialize document → update matching `[[sources]]`
//!   and `asl.ladder` / `asl.grafcet`; re-lower into `setup`/`loop` when the
//!   visual language is the active authority.
//! - **TOML → Diagram/ASL:** parse source text → replace diagram + ASL blocks.
//! - **ASL → Diagram/TOML:** emit ladder/sfc documents from `asl.ladder` /
//!   `asl.grafcet` (or synthesize from loop).
//!
//! Determinism: serde TOML pretty-print order follows struct field order; tests
//! assert AST equality after round-trips (not byte equality of TOML).

use dendri_ast::{
    GrafcetBlock, GrafcetDocument, LadderBlock, LadderDocument, Program, SimulationProject,
    SourceLanguage, SourceScript,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Ladder(#[from] dendri_frontend_ladder::LadderError),
    #[error(transparent)]
    Sfc(#[from] dendri_frontend_sfc::SfcError),
    #[error("TOML: {0}")]
    Toml(String),
}

/// Which view initiated the latest edit (for UI highlighting / debug).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOrigin {
    Diagram,
    TomlSource,
    Asl,
}

/// Active visual language under edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualKind {
    Ladder,
    Sfc,
}

impl VisualKind {
    pub fn language(self) -> SourceLanguage {
        match self {
            Self::Ladder => SourceLanguage::Ladder,
            Self::Sfc => SourceLanguage::Sfc,
        }
    }
}

/// Live coherent project state with real-time projection helpers.
#[derive(Debug, Clone)]
pub struct SyncSession {
    pub project: SimulationProject,
    pub last_origin: Option<SyncOrigin>,
    /// When true, lowering from ladder/sfc overwrites `asl.setup`/`asl.loop`.
    pub lower_into_asl_body: bool,
}

impl SyncSession {
    pub fn new(project: SimulationProject) -> Self {
        Self {
            project,
            last_origin: None,
            lower_into_asl_body: true,
        }
    }

    pub fn from_project_toml(text: &str) -> Result<Self, SyncError> {
        let project: SimulationProject =
            toml::from_str(text).map_err(|e| SyncError::Toml(e.to_string()))?;
        Ok(Self::new(project))
    }

    pub fn emit_project_toml(&self) -> Result<String, SyncError> {
        dendri_emit::emit_project(&self.project).map_err(|e| SyncError::Toml(e.to_string()))
    }

    /// Ladder diagram model currently mirrored in ASL / sources.
    pub fn ladder_model(&self) -> Option<LadderBlock> {
        self.project.asl.ladder.first().cloned()
    }

    pub fn sfc_model(&self) -> Option<GrafcetBlock> {
        self.project.asl.grafcet.first().cloned()
    }

    pub fn source_text(&self, lang: SourceLanguage) -> Option<&str> {
        self.project
            .sources
            .iter()
            .find(|s| s.language == lang)
            .map(|s| s.text.as_str())
    }

    /// Apply a full ladder document (diagram edit). Updates sources + ASL.
    pub fn apply_ladder_document(&mut self, doc: LadderDocument) -> Result<(), SyncError> {
        let text = toml::to_string_pretty(&doc).map_err(|e| SyncError::Toml(e.to_string()))?;
        let program = dendri_frontend_ladder::document_to_program(doc);
        self.write_visual(SourceLanguage::Ladder, text, program, SyncOrigin::Diagram)
    }

    /// Apply a full GRAFCET document (diagram edit).
    pub fn apply_sfc_document(&mut self, doc: GrafcetDocument) -> Result<(), SyncError> {
        let text = toml::to_string_pretty(&doc).map_err(|e| SyncError::Toml(e.to_string()))?;
        let program = dendri_frontend_sfc::document_to_program(doc);
        self.write_visual(SourceLanguage::Sfc, text, program, SyncOrigin::Diagram)
    }

    /// Parse edited `[[sources]]` ladder/sfc TOML text and project outward.
    pub fn apply_source_toml(
        &mut self,
        kind: VisualKind,
        text: &str,
    ) -> Result<(), SyncError> {
        let program = match kind {
            VisualKind::Ladder => dendri_frontend_ladder::parse_ladder(text)?,
            VisualKind::Sfc => dendri_frontend_sfc::parse_sfc(text)?,
        };
        self.write_visual(kind.language(), text.to_string(), program, SyncOrigin::TomlSource)
    }

    /// Project from embedded ASL ladder/grafcet (or synthesize) into sources + models.
    pub fn apply_asl_program(&mut self, program: Program) -> Result<(), SyncError> {
        self.project.asl = program.clone();
        // Refresh ladder source
        let ladder_text = dendri_frontend_ladder::emit_ladder(&program);
        self.upsert_source(SourceLanguage::Ladder, "rungs", ladder_text);
        // Refresh sfc source
        let sfc_text = dendri_frontend_sfc::emit_sfc(&program);
        self.upsert_source(SourceLanguage::Sfc, "chart", sfc_text);
        self.last_origin = Some(SyncOrigin::Asl);
        Ok(())
    }

    /// Ensure ladder + sfc `[[sources]]` texts match ASL blocks (fill empties / refresh).
    pub fn sync_visual_sources_from_asl(&mut self) -> Result<(), SyncError> {
        let program = self.project.asl.clone();
        self.apply_asl_program(program)
    }

    /// Round-trip check: diagram → TOML → parse → compare blocks.
    pub fn assert_ladder_roundtrip(&self) -> Result<(), SyncError> {
        let text = self
            .source_text(SourceLanguage::Ladder)
            .ok_or_else(|| SyncError::Message("no ladder source".into()))?;
        let again = dendri_frontend_ladder::parse_ladder(text)?;
        let Some(orig) = self.project.asl.ladder.first() else {
            return Err(SyncError::Message("no asl.ladder".into()));
        };
        if again.ladder.first() != Some(orig) {
            return Err(SyncError::Message("ladder round-trip mismatch".into()));
        }
        Ok(())
    }

    pub fn assert_sfc_roundtrip(&self) -> Result<(), SyncError> {
        let text = self
            .source_text(SourceLanguage::Sfc)
            .ok_or_else(|| SyncError::Message("no sfc source".into()))?;
        let again = dendri_frontend_sfc::parse_sfc(text)?;
        let Some(orig) = self.project.asl.grafcet.first() else {
            return Err(SyncError::Message("no asl.grafcet".into()));
        };
        if again.grafcet.first() != Some(orig) {
            return Err(SyncError::Message("sfc round-trip mismatch".into()));
        }
        Ok(())
    }

    fn write_visual(
        &mut self,
        lang: SourceLanguage,
        text: String,
        program: Program,
        origin: SyncOrigin,
    ) -> Result<(), SyncError> {
        let name = match lang {
            SourceLanguage::Ladder => "rungs",
            SourceLanguage::Sfc => "chart",
            _ => "visual",
        };
        self.upsert_source(lang, name, text);

        if self.lower_into_asl_body {
            // Merge: keep other dialect blocks; replace body from lowered program.
            let mut asl = program;
            match lang {
                SourceLanguage::Ladder => {
                    asl.grafcet = self.project.asl.grafcet.clone();
                }
                SourceLanguage::Sfc => {
                    asl.ladder = self.project.asl.ladder.clone();
                }
                _ => {}
            }
            // Preserve project metadata name if present.
            if asl.metadata.project_name.is_none() {
                asl.metadata.project_name = self.project.asl.metadata.project_name.clone();
            }
            self.project.asl = asl;
        } else {
            match lang {
                SourceLanguage::Ladder => {
                    self.project.asl.ladder = program.ladder;
                }
                SourceLanguage::Sfc => {
                    self.project.asl.grafcet = program.grafcet;
                }
                _ => {}
            }
        }
        self.last_origin = Some(origin);
        Ok(())
    }

    fn upsert_source(&mut self, language: SourceLanguage, default_name: &str, text: String) {
        if let Some(src) = self
            .project
            .sources
            .iter_mut()
            .find(|s| s.language == language)
        {
            src.text = text;
        } else {
            self.project.sources.push(SourceScript {
                language,
                name: Some(default_name.into()),
                text,
                path_hint: None,
            });
        }
    }
}

/// Sync visual `[[sources]]` from ASL and ensure `asl.ladder` / `asl.grafcet` blocks exist.
/// Does **not** overwrite `setup`/`loop` (preserves authoritative textual ASL body).
pub fn fill_visual_sources(project: &mut SimulationProject) {
    let ladder_text = dendri_frontend_ladder::emit_ladder(&project.asl);
    let sfc_text = dendri_frontend_sfc::emit_sfc(&project.asl);
    upsert(project, SourceLanguage::Ladder, "rungs", ladder_text.clone());
    upsert(project, SourceLanguage::Sfc, "chart", sfc_text.clone());

    if project.asl.ladder.is_empty() {
        if let Ok(p) = dendri_frontend_ladder::parse_ladder(&ladder_text) {
            project.asl.ladder = p.ladder;
        }
    }
    if project.asl.grafcet.is_empty() {
        if let Ok(p) = dendri_frontend_sfc::parse_sfc(&sfc_text) {
            project.asl.grafcet = p.grafcet;
        }
    }
}

fn upsert(project: &mut SimulationProject, language: SourceLanguage, name: &str, text: String) {
    if let Some(src) = project.sources.iter_mut().find(|s| s.language == language) {
        if src.text.trim().is_empty() {
            src.text = text;
        }
    } else {
        project.sources.push(SourceScript {
            language,
            name: Some(name.into()),
            text,
            path_hint: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::expr::Expr;
    use dendri_ast::stmt::Stmt;
    use dendri_ast::{
        BoardInstance, DataEntry, DataType, HardwareEntry, HwType, IoMode, LadderCell, LadderRung,
        Metadata, Scope,
    };

    fn bare_project() -> SimulationProject {
        SimulationProject {
            metadata: Metadata {
                project_name: Some("sync-test".into()),
                editor: Some("dendriForge".into()),
                author: None,
                notes: None,
                expect_error: None,
            },
            board: BoardInstance {
                profile: "arduino-uno-r3".into(),
                instance_id: "board".into(),
            },
            components: vec![],
            wiring: vec![],
            sources: vec![
                SourceScript {
                    language: SourceLanguage::Ladder,
                    name: Some("rungs".into()),
                    text: String::new(),
                    path_hint: None,
                },
                SourceScript {
                    language: SourceLanguage::Sfc,
                    name: Some("chart".into()),
                    text: String::new(),
                    path_hint: None,
                },
            ],
            asl: Program {
                metadata: Metadata {
                    project_name: Some("sync-test-asl".into()),
                    editor: Some("dendriForge".into()),
                    author: None,
                    notes: None,
                    expect_error: None,
                },
                hardware: vec![HardwareEntry {
                    pin: 13,
                    hw_type: HwType::Digital,
                    io_mode: IoMode::Do,
                    label: "ledPin".into(),
                    target_role: "digital-out".into(),
                }],
                data: vec![DataEntry {
                    name: "led".into(),
                    data_type: DataType::Bool,
                    scope: Scope::Var,
                    init: "FALSE".into(),
                    unit: "-".into(),
                }],
                setup: vec![],
                loop_body: vec![],
                functions: vec![],
                ladder: vec![],
                grafcet: vec![],
            },
        }
    }

    #[test]
    fn diagram_to_toml_to_asl() {
        let mut session = SyncSession::new(bare_project());
        let doc = LadderDocument {
            name: "rungs".into(),
            rungs: vec![LadderRung {
                id: "r1".into(),
                row: 0,
                comment: None,
                cells: vec![
                    LadderCell::ContactNo {
                        operand: "led".into(),
                        col: 0,
                    },
                    LadderCell::Coil {
                        operand: "led".into(),
                        col: 1,
                    },
                ],
            }],
            io_map: vec![],
            hardware: session.project.asl.hardware.clone(),
            data: session.project.asl.data.clone(),
            setup: vec![],
        };
        session.apply_ladder_document(doc).unwrap();
        assert!(!session.source_text(SourceLanguage::Ladder).unwrap().is_empty());
        assert_eq!(session.project.asl.ladder.len(), 1);
        session.assert_ladder_roundtrip().unwrap();

        // TOML edit path
        let text = session.source_text(SourceLanguage::Ladder).unwrap().to_string();
        session
            .apply_source_toml(VisualKind::Ladder, &text)
            .unwrap();
        assert_eq!(session.last_origin, Some(SyncOrigin::TomlSource));
    }

    #[test]
    fn asl_projects_to_sources() {
        let mut session = SyncSession::new(bare_project());
        session.project.asl.loop_body = vec![Stmt::Call {
            name: "SET_BOOL".into(),
            args: vec![Expr::Uint { value: 13 }, Expr::Bool { value: true }],
        }];
        session.sync_visual_sources_from_asl().unwrap();
        assert!(session
            .source_text(SourceLanguage::Ladder)
            .unwrap()
            .contains("SET_BOOL"));
        assert!(session
            .source_text(SourceLanguage::Sfc)
            .unwrap()
            .contains("S0"));
    }
}
