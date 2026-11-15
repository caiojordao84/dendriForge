//! DendriForge visual editor MVP — Ladder / SFC ↔ TOML real-time sync.
//!
//! # Editor choice
//!
//! **iced 0.13** (pure Rust, Elm architecture). Requires **rustc ≥ 1.86** because
//! the Wayland clipboard stack (`calloop` / `wayland-protocols`) raised its MSRV.
//! The workspace toolchain is pinned to **1.86.0** for this reason.
//!
//! Sync logic lives in `dendri-sync` (headless, CI-tested). This binary is the
//! interactive shell: diagram palette + live TOML pane. Full canvas polish is
//! incremental; sync + IR are the solid path.

use std::path::PathBuf;

use dendri_ast::{LadderCell, SourceLanguage};
use dendri_sync::{SyncOrigin, SyncSession, VisualKind};
use iced::widget::{button, column, container, row, scrollable, text, text_editor, Column, Space};
use iced::{Element, Fill, Font, Task, Theme};

fn main() -> iced::Result {
    iced::application("DendriForge — Ladder / SFC", Editor::update, Editor::view)
        .theme(|_| Theme::Dark)
        .window_size((1100.0, 720.0))
        .run_with(Editor::boot)
}

struct Editor {
    session: SyncSession,
    kind: VisualKind,
    toml_content: text_editor::Content,
    status: String,
    path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum Message {
    SwitchLadder,
    SwitchSfc,
    TomlAction(text_editor::Action),
    AddContactNo,
    AddCoil,
    AddActionSetBool,
    AddSfcStep,
    ReloadFromAsl,
    Save,
    OpenDefault,
}

impl Editor {
    fn boot() -> (Self, Task<Message>) {
        let path = default_project_path();
        let session = load_session(&path).unwrap_or_else(|e| {
            let mut s = SyncSession::new(empty_project());
            s.project.metadata.notes = Some(format!("load failed: {e}"));
            s
        });
        let kind = VisualKind::Ladder;
        let initial = session
            .source_text(SourceLanguage::Ladder)
            .unwrap_or("")
            .to_string();
        let mut editor = Self {
            session,
            kind,
            toml_content: text_editor::Content::with_text(&initial),
            status: format!("loaded {}", path.display()),
            path: Some(path),
        };
        if editor
            .session
            .source_text(SourceLanguage::Ladder)
            .map(|t| t.trim().is_empty())
            .unwrap_or(true)
        {
            let _ = editor.session.sync_visual_sources_from_asl();
            editor.refresh_toml_from_session();
        }
        (editor, Task::none())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SwitchLadder => {
                self.kind = VisualKind::Ladder;
                self.refresh_toml_from_session();
                self.status = "view: Ladder".into();
            }
            Message::SwitchSfc => {
                self.kind = VisualKind::Sfc;
                self.refresh_toml_from_session();
                self.status = "view: SFC/GRAFCET".into();
            }
            Message::TomlAction(action) => {
                self.toml_content.perform(action);
                let text = self.toml_content.text();
                match self.session.apply_source_toml(self.kind, &text) {
                    Ok(()) => {
                        self.status = format!(
                            "synced from TOML ({:?})",
                            self.session.last_origin.unwrap_or(SyncOrigin::TomlSource)
                        );
                    }
                    Err(e) => {
                        self.status = format!("TOML sync pending: {e}");
                    }
                }
            }
            Message::AddContactNo => {
                if let Err(e) = self.mutate_ladder(|doc| {
                    if let Some(rung) = doc.rungs.first_mut() {
                        let col = rung.cells.len() as u32;
                        rung.cells.push(LadderCell::ContactNo {
                            operand: "btn".into(),
                            col,
                        });
                    }
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (contact)".into();
                }
            }
            Message::AddCoil => {
                if let Err(e) = self.mutate_ladder(|doc| {
                    if let Some(rung) = doc.rungs.first_mut() {
                        let col = rung.cells.len() as u32;
                        rung.cells.push(LadderCell::Coil {
                            operand: "led".into(),
                            col,
                        });
                    }
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (coil)".into();
                }
            }
            Message::AddActionSetBool => {
                if let Err(e) = self.mutate_ladder(|doc| {
                    use dendri_ast::expr::Expr;
                    use dendri_ast::stmt::Stmt;
                    if let Some(rung) = doc.rungs.first_mut() {
                        let col = rung.cells.len() as u32;
                        rung.cells.push(LadderCell::Action {
                            col,
                            stmts: vec![Stmt::Call {
                                name: "SET_BOOL".into(),
                                args: vec![
                                    Expr::Uint { value: 13 },
                                    Expr::Var {
                                        name: "led".into(),
                                    },
                                ],
                            }],
                        });
                    }
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (action)".into();
                }
            }
            Message::AddSfcStep => {
                if let Err(e) = self.mutate_sfc(|doc| {
                    let n = doc.steps.len();
                    let id = format!("S{n}");
                    let y = (n as f32) * 80.0;
                    doc.steps.push(dendri_ast::GrafcetStep {
                        id: id.clone(),
                        initial: n == 0,
                        actions: vec![],
                        comment: Some("new step".into()),
                        x: 0.0,
                        y,
                    });
                    if n == 0 {
                        doc.initial_steps = vec![id];
                    }
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (step)".into();
                }
            }
            Message::ReloadFromAsl => match self.session.sync_visual_sources_from_asl() {
                Ok(()) => {
                    self.refresh_toml_from_session();
                    self.status = "projected ASL → diagram/TOML".into();
                }
                Err(e) => self.status = format!("ASL sync error: {e}"),
            },
            Message::Save => match self.session.emit_project_toml() {
                Ok(toml) => {
                    if let Some(path) = &self.path {
                        match std::fs::write(path, toml) {
                            Ok(()) => self.status = format!("saved {}", path.display()),
                            Err(e) => self.status = format!("save error: {e}"),
                        }
                    } else {
                        self.status = "no path — not saved".into();
                    }
                }
                Err(e) => self.status = format!("emit error: {e}"),
            },
            Message::OpenDefault => {
                let path = default_project_path();
                match load_session(&path) {
                    Ok(session) => {
                        self.session = session;
                        self.path = Some(path.clone());
                        self.refresh_toml_from_session();
                        self.status = format!("opened {}", path.display());
                    }
                    Err(e) => self.status = format!("open failed: {e}"),
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let toolbar = row![
            button("Ladder").on_press(Message::SwitchLadder),
            button("SFC").on_press(Message::SwitchSfc),
            Space::with_width(12),
            button("Open P01").on_press(Message::OpenDefault),
            button("ASL → views").on_press(Message::ReloadFromAsl),
            button("Save").on_press(Message::Save),
        ]
        .spacing(8);

        let palette = match self.kind {
            VisualKind::Ladder => row![
                button("+ NO contact").on_press(Message::AddContactNo),
                button("+ Coil").on_press(Message::AddCoil),
                button("+ SET_BOOL").on_press(Message::AddActionSetBool),
            ]
            .spacing(8),
            VisualKind::Sfc => row![button("+ Step").on_press(Message::AddSfcStep)].spacing(8),
        };

        let diagram = container(scrollable(self.diagram_view()))
            .padding(12)
            .width(Fill)
            .height(Fill);

        let toml_pane = column![
            text("TOML source (live sync)").size(16),
            text_editor(&self.toml_content)
                .height(Fill)
                .on_action(Message::TomlAction),
        ]
        .spacing(8)
        .width(Fill)
        .height(Fill);

        let body = row![diagram, toml_pane].spacing(12).height(Fill);
        let status = text(&self.status).size(14);

        column![toolbar, palette, body, status]
            .spacing(10)
            .padding(12)
            .into()
    }

    fn diagram_view(&self) -> Element<'_, Message> {
        match self.kind {
            VisualKind::Ladder => {
                let mut col = Column::new().spacing(8);
                col = col.push(text("Ladder rungs").size(18));
                if let Some(block) = self.session.ladder_model() {
                    for rung in &block.rungs {
                        let mut line = String::from("|");
                        for cell in &rung.cells {
                            line.push_str(&format!("-[{}]-", cell_label(cell)));
                        }
                        line.push('|');
                        col = col.push(text(line).font(Font::MONOSPACE));
                        if let Some(c) = &rung.comment {
                            col = col.push(text(format!("  // {c}")).size(12));
                        }
                    }
                } else {
                    col = col.push(text("(empty — edit TOML or use palette)"));
                }
                col.into()
            }
            VisualKind::Sfc => {
                let mut col = Column::new().spacing(8);
                col = col.push(text("GRAFCET / SFC").size(18));
                if let Some(block) = self.session.sfc_model() {
                    for step in &block.steps {
                        let mark = if step.initial { "[*]" } else { "[ ]" };
                        col = col.push(text(format!(
                            "{mark} {}  ({:.0},{:.0})  actions={}",
                            step.id,
                            step.x,
                            step.y,
                            step.actions.len()
                        )));
                    }
                    for tr in &block.transitions {
                        col = col.push(text(format!(
                            "  -- {} -> {:?} -> {:?}",
                            tr.id, tr.from, tr.to
                        )));
                    }
                } else {
                    col = col.push(text("(empty chart)"));
                }
                col.into()
            }
        }
    }

    fn refresh_toml_from_session(&mut self) {
        let text = self
            .session
            .source_text(self.kind.language())
            .unwrap_or("")
            .to_string();
        self.toml_content = text_editor::Content::with_text(&text);
    }

    fn mutate_ladder(
        &mut self,
        f: impl FnOnce(&mut dendri_ast::LadderDocument),
    ) -> Result<(), String> {
        let mut doc = if let Some(block) = self.session.ladder_model() {
            let mut d = dendri_ast::LadderDocument::from_block(&block);
            d.hardware = self.session.project.asl.hardware.clone();
            d.data = self.session.project.asl.data.clone();
            d.setup = self.session.project.asl.setup.clone();
            d
        } else {
            dendri_ast::LadderDocument {
                name: "rungs".into(),
                rungs: vec![dendri_ast::LadderRung {
                    id: "r1".into(),
                    cells: vec![],
                    comment: None,
                    row: 0,
                }],
                io_map: vec![],
                hardware: self.session.project.asl.hardware.clone(),
                data: self.session.project.asl.data.clone(),
                setup: self.session.project.asl.setup.clone(),
            }
        };
        f(&mut doc);
        self.session
            .apply_ladder_document(doc)
            .map_err(|e| e.to_string())?;
        self.kind = VisualKind::Ladder;
        self.refresh_toml_from_session();
        Ok(())
    }

    fn mutate_sfc(
        &mut self,
        f: impl FnOnce(&mut dendri_ast::GrafcetDocument),
    ) -> Result<(), String> {
        let mut doc = if let Some(block) = self.session.sfc_model() {
            let mut d = dendri_ast::GrafcetDocument::from_block(&block);
            d.hardware = self.session.project.asl.hardware.clone();
            d.data = self.session.project.asl.data.clone();
            d.setup = self.session.project.asl.setup.clone();
            d
        } else {
            dendri_ast::GrafcetDocument {
                name: "chart".into(),
                initial_steps: vec![],
                steps: vec![],
                transitions: vec![],
                io_map: vec![],
                hardware: self.session.project.asl.hardware.clone(),
                data: self.session.project.asl.data.clone(),
                setup: self.session.project.asl.setup.clone(),
            }
        };
        f(&mut doc);
        self.session
            .apply_sfc_document(doc)
            .map_err(|e| e.to_string())?;
        self.kind = VisualKind::Sfc;
        self.refresh_toml_from_session();
        Ok(())
    }
}

fn cell_label(cell: &LadderCell) -> String {
    match cell {
        LadderCell::ContactNo { operand, .. } => format!("][{operand}"),
        LadderCell::ContactNc { operand, .. } => format!("]/[{operand}"),
        LadderCell::Coil { operand, .. } => format!("({operand})"),
        LadderCell::CoilSet { operand, .. } => format!("(S:{operand})"),
        LadderCell::CoilReset { operand, .. } => format!("(R:{operand})"),
        LadderCell::Action { stmts, .. } => format!("{{A*{}}}", stmts.len()),
        LadderCell::Wire { .. } => "--".into(),
        LadderCell::Empty { .. } => "..".into(),
        LadderCell::FbTimer { fb, instance, .. } => format!("[{fb:?}:{instance}]"),
        LadderCell::FbCounter { fb, instance, .. } => format!("[{fb:?}:{instance}]"),
        LadderCell::FbCompare { op, .. } => format!("[{op}]"),
    }
}

fn default_project_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/fixtures/projects/P01-uno-led-blink.dendri.toml")
}

fn load_session(path: &std::path::Path) -> Result<SyncSession, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    SyncSession::from_project_toml(&text).map_err(|e| e.to_string())
}

fn empty_project() -> dendri_ast::SimulationProject {
    dendri_ast::SimulationProject {
        metadata: dendri_ast::Metadata {
            project_name: Some("untitled".into()),
            editor: Some("dendriForge".into()),
            author: None,
            notes: None,
            expect_error: None,
        },
        board: dendri_ast::BoardInstance {
            profile: "arduino-uno-r3".into(),
            instance_id: "board".into(),
        },
        components: vec![],
        wiring: vec![],
        sources: vec![],
        asl: dendri_ast::Program {
            metadata: dendri_ast::Metadata {
                project_name: Some("untitled-asl".into()),
                editor: Some("dendriForge".into()),
                author: None,
                notes: None,
                expect_error: None,
            },
            hardware: vec![],
            data: vec![],
            setup: vec![],
            loop_body: vec![],
            functions: vec![],
            ladder: vec![],
            grafcet: vec![],
        },
    }
}
