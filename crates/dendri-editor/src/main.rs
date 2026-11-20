//! DendriForge visual editor — iced shell matching Caio HTML canvas mockups.
//!
//! Shared chrome: brand, breadcrumb, Pause/Step/Reset, tabs
//! Simulation | Code | Ladder | GRAFCET, Export, status footer.
//! Simulation is the priority content (Uno + LED + pull-up button).
//!
//! Sync logic: `dendri-sync`. Canvas binding: `dendri-canvas`.

mod theme;

use std::path::PathBuf;
use std::time::Duration;

use dendri_ast::{LadderCell, SourceLanguage};
use dendri_canvas::SimCanvas;
use dendri_sim::SimConfig;
use dendri_sync::{SyncOrigin, SyncSession, VisualKind};
use iced::widget::{
    button, column, container, row, scrollable, text, text_editor, text_input, Column, Space,
};
use iced::{Color, Element, Fill, Font, Length, Subscription, Task, Theme};
use theme::{
    chrome_btn, export_btn, main_bg, palette_card, palette_card_active, panel_bg, pause_btn,
    surface_bg, surface_low_bg, surface_lowest_bg, tab_active, tab_idle, transport_btn,
    ACCENT_CYAN, ACCENT_EMERALD, ACCENT_ORANGE, ACCENT_SKY, BORDER, SURFACE_LOWEST, TEXT_HEADING,
    TEXT_MAIN, TEXT_MUTED,
};

fn main() -> iced::Result {
    iced::application(
        "DendriForge — Simulation / Code / Ladder / GRAFCET",
        Editor::update,
        Editor::view,
    )
    .theme(|_| Theme::Dark)
    .subscription(Editor::subscription)
    .window_size((1280.0, 820.0))
    .run_with(Editor::boot)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Simulation,
    Code,
    Ladder,
    Grafcet,
}

struct Editor {
    session: SyncSession,
    kind: VisualKind,
    mode: Mode,
    toml_content: text_editor::Content,
    project_toml: text_editor::Content,
    status: String,
    path: Option<PathBuf>,
    canvas: Option<SimCanvas>,
    canvas_note: String,
    running: bool,
    elapsed_ms: u64,
    palette_filter: String,
    selected_part: Option<&'static str>,
    left_collapsed: bool,
}

#[derive(Debug, Clone)]
enum Message {
    SwitchMode(Mode),
    TomlAction(text_editor::Action),
    ProjectTomlAction(text_editor::Action),
    PaletteFilter(String),
    SelectPart(&'static str),
    ToggleLeftDock,
    TogglePause,
    Tick,
    AddContactNo,
    AddCoil,
    AddActionSetBool,
    AddParallelOr,
    AddTon,
    AddRTrig,
    RemoveLastLadderCell,
    AddSfcStep,
    AddSfcTransition,
    ReloadFromAsl,
    Save,
    OpenDefault,
    OpenCanvasDemo,
    SimPress,
    SimRelease,
    SimStep,
    SimReset,
    Export,
}

impl Editor {
    fn boot() -> (Self, Task<Message>) {
        let path = canvas_demo_path();
        let session = load_session(&path).unwrap_or_else(|e| {
            let mut s = SyncSession::new(empty_project());
            s.project.metadata.notes = Some(format!("load failed: {e}"));
            s
        });
        let kind = VisualKind::Ladder;
        let ladder_text = session
            .source_text(SourceLanguage::Ladder)
            .unwrap_or("")
            .to_string();
        let project_text = session
            .emit_project_toml()
            .unwrap_or_else(|_| String::new());
        let mut editor = Self {
            session,
            kind,
            mode: Mode::Simulation,
            toml_content: text_editor::Content::with_text(&ladder_text),
            project_toml: text_editor::Content::with_text(&project_text),
            status: format!("loaded {}", path.display()),
            path: Some(path),
            canvas: None,
            canvas_note: String::new(),
            running: true,
            elapsed_ms: 0,
            palette_filter: String::new(),
            selected_part: Some("Red LED"),
            left_collapsed: false,
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
        editor.refresh_project_toml();
        editor.boot_canvas_from_session();
        (editor, Task::none())
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.running {
            iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SwitchMode(mode) => {
                self.mode = mode;
                match mode {
                    Mode::Ladder => {
                        self.kind = VisualKind::Ladder;
                        self.refresh_toml_from_session();
                    }
                    Mode::Grafcet => {
                        self.kind = VisualKind::Sfc;
                        self.refresh_toml_from_session();
                    }
                    Mode::Code => {
                        self.refresh_project_toml();
                    }
                    Mode::Simulation => {
                        if self.canvas.is_none() {
                            self.boot_canvas_from_session();
                        }
                    }
                }
                self.status = format!("view: {}", mode_label(mode));
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
            Message::ProjectTomlAction(action) => {
                self.project_toml.perform(action);
                self.status = "project.toml edited (save to persist)".into();
            }
            Message::PaletteFilter(s) => self.palette_filter = s,
            Message::SelectPart(name) => {
                self.selected_part = Some(name);
                self.status = format!("palette: {name}");
            }
            Message::ToggleLeftDock => {
                self.left_collapsed = !self.left_collapsed;
            }
            Message::TogglePause => {
                self.running = !self.running;
                self.status = if self.running {
                    "sim: running".into()
                } else {
                    "sim: paused".into()
                };
            }
            Message::Tick => {
                // Advance wall-clock only. Auto-stepping + rebuilding SVGs every
                // tick trips iced damage sorting panics on some backends.
                if self.running {
                    self.elapsed_ms = self.elapsed_ms.saturating_add(100);
                }
            }
            Message::AddContactNo => {
                if let Err(e) = self.mutate_ladder(|doc| {
                    if let Some(rung) = doc.rungs.first_mut() {
                        let col = rung.cells.len() as u32;
                        rung.cells.push(LadderCell::ContactNo {
                            operand: "pressed".into(),
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
                            operand: "ledPin".into(),
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
                                    Expr::Var {
                                        name: "ledPin".into(),
                                    },
                                    Expr::Var {
                                        name: "pressed".into(),
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
            Message::AddParallelOr => {
                if let Err(e) = self.mutate_ladder(|doc| {
                    use dendri_ast::{LadderBranch, LadderCell};
                    if let Some(rung) = doc.rungs.first_mut() {
                        let col = rung.cells.len() as u32;
                        rung.cells.push(LadderCell::Parallel {
                            col,
                            branches: vec![
                                LadderBranch {
                                    id: "b0".into(),
                                    cells: vec![LadderCell::ContactNo {
                                        operand: "a".into(),
                                        col: 0,
                                    }],
                                },
                                LadderBranch {
                                    id: "b1".into(),
                                    cells: vec![LadderCell::ContactNo {
                                        operand: "b".into(),
                                        col: 0,
                                    }],
                                },
                            ],
                        });
                    }
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (parallel OR)".into();
                }
            }
            Message::AddTon => {
                if let Err(e) = self.mutate_ladder(|doc| {
                    use dendri_ast::expr::Expr;
                    use dendri_ast::{LadderCell, TimerFbKind};
                    if let Some(rung) = doc.rungs.first_mut() {
                        let col = rung.cells.len() as u32;
                        rung.cells.push(LadderCell::FbTimer {
                            fb: TimerFbKind::Ton,
                            instance: "ton1".into(),
                            args: vec![Expr::Var {
                                name: "PT".into(),
                            }],
                            col,
                        });
                    }
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (TON)".into();
                }
            }
            Message::AddRTrig => {
                if let Err(e) = self.mutate_ladder(|doc| {
                    use dendri_ast::LadderCell;
                    if let Some(rung) = doc.rungs.first_mut() {
                        let col = rung.cells.len() as u32;
                        rung.cells.push(LadderCell::FbEdge {
                            rising: true,
                            instance: "rt1".into(),
                            operand: None,
                            col,
                        });
                    }
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (R_TRIG)".into();
                }
            }
            Message::RemoveLastLadderCell => {
                if let Err(e) = self.mutate_ladder(|doc| {
                    if let Some(rung) = doc.rungs.first_mut() {
                        let _ = rung.cells.pop();
                    }
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (remove cell)".into();
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
                        enclosed: None,
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
            Message::AddSfcTransition => {
                if let Err(e) = self.mutate_sfc(|doc| {
                    use dendri_ast::expr::Expr;
                    if doc.steps.len() < 2 {
                        return;
                    }
                    let from = doc.steps[doc.steps.len() - 2].id.clone();
                    let to = doc.steps[doc.steps.len() - 1].id.clone();
                    let id = format!("T{}", doc.transitions.len());
                    let y = doc.steps[doc.steps.len() - 1].y - 40.0;
                    doc.transitions.push(dendri_ast::GrafcetTransition {
                        id,
                        from: vec![from],
                        to: vec![to],
                        condition: Expr::Bool { value: true },
                        comment: Some("palette".into()),
                        x: 0.0,
                        y,
                    });
                }) {
                    self.status = format!("edit failed: {e}");
                } else {
                    self.status = "diagram → TOML/ASL (transition)".into();
                }
            }
            Message::ReloadFromAsl => match self.session.sync_visual_sources_from_asl() {
                Ok(()) => {
                    self.refresh_toml_from_session();
                    self.refresh_project_toml();
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
                        self.refresh_project_toml();
                        self.boot_canvas_from_session();
                        self.status = format!("opened {}", path.display());
                    }
                    Err(e) => self.status = format!("open failed: {e}"),
                }
            }
            Message::OpenCanvasDemo => {
                let path = canvas_demo_path();
                match load_session(&path) {
                    Ok(session) => {
                        self.session = session;
                        self.path = Some(path.clone());
                        self.mode = Mode::Simulation;
                        self.refresh_toml_from_session();
                        self.refresh_project_toml();
                        self.boot_canvas_from_session();
                        self.status = format!("opened canvas demo {}", path.display());
                    }
                    Err(e) => self.status = format!("open failed: {e}"),
                }
            }
            Message::SimPress => {
                if let Some(c) = &mut self.canvas {
                    c.press_button();
                    let level = if c.binding.button_active_low {
                        "LOW (active-low pull-up)"
                    } else {
                        "HIGH"
                    };
                    self.canvas_note = format!("button pressed → pin {level}");
                    self.status = "sim: button pressed".into();
                    // Do not rebuild SVG handles here — iced damage sorting panics
                    // when SVG markup changes under tiny-skia/wgpu on this stack.
                }
            }
            Message::SimRelease => {
                if let Some(c) = &mut self.canvas {
                    c.release_button();
                    let level = if c.binding.button_active_low {
                        "HIGH (MCU pull-up)"
                    } else {
                        "LOW"
                    };
                    self.canvas_note = format!("button released → pin {level}");
                    self.status = "sim: button released".into();
                }
            }
            Message::SimStep => {
                if let Some(c) = &mut self.canvas {
                    match c.step() {
                        Ok(frame) => {
                            self.canvas_note = frame.to_snapshot().trim().replace('\n', " | ");
                            self.elapsed_ms = self.elapsed_ms.saturating_add(1);
                            self.status = format!(
                                "sim: scan {}  led={}  btn={}",
                                frame.cycle,
                                if frame.led_on { "on" } else { "off" },
                                if frame.button_pressed {
                                    "pressed"
                                } else {
                                    "released"
                                }
                            );
                        }
                        Err(e) => self.status = format!("sim step error: {e}"),
                    }
                }
            }
            Message::SimReset => {
                self.elapsed_ms = 0;
                self.boot_canvas_from_session();
                self.status = "sim: reset canvas from project ASL".into();
            }
            Message::Export => match self.session.emit_project_toml() {
                Ok(toml) => {
                    let out = export_path();
                    match std::fs::write(&out, toml) {
                        Ok(()) => self.status = format!("exported {}", out.display()),
                        Err(e) => self.status = format!("export error: {e}"),
                    }
                }
                Err(e) => self.status = format!("export emit error: {e}"),
            },
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let shell = column![
            self.top_bar(),
            self.workspace(),
            self.status_footer(),
        ]
        .spacing(0)
        .width(Fill)
        .height(Fill);

        container(shell)
            .width(Fill)
            .height(Fill)
            .style(|_t| container::Style {
                background: Some(main_bg()),
                ..container::Style::default()
            })
            .into()
    }

    fn top_bar(&self) -> Element<'_, Message> {
        let brand = row![
            container(text("Δ").size(16))
                .padding([4, 8])
                .style(|_t| container::Style {
                    background: Some(iced::Background::Color(ACCENT_ORANGE)),
                    text_color: Some(Color::from_rgb(0.008, 0.024, 0.090)),
                    border: iced::Border {
                        radius: 4.0.into(),
                        ..iced::Border::default()
                    },
                    ..container::Style::default()
                }),
            text("DendriForge").size(18).color(TEXT_HEADING),
            container(text("v0.6.0").size(10).font(Font::MONOSPACE).color(TEXT_MUTED))
                .padding([2, 6])
                .style(|_t| container::Style {
                    background: Some(surface_low_bg()),
                    border: iced::Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..container::Style::default()
                }),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let project_name = self
            .session
            .project
            .metadata
            .project_name
            .as_deref()
            .unwrap_or("BUTTON_STATE_CHANGE");
        let breadcrumb = row![
            text("PROJECTS").size(11).font(Font::MONOSPACE).color(TEXT_MUTED),
            text(" / ").size(11).font(Font::MONOSPACE).color(BORDER),
            text("ARDUINO_UNO_R3")
                .size(11)
                .font(Font::MONOSPACE)
                .color(TEXT_MUTED),
            text(" / ").size(11).font(Font::MONOSPACE).color(BORDER),
            text(project_name.to_ascii_uppercase())
                .size(11)
                .font(Font::MONOSPACE)
                .color(ACCENT_ORANGE),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center);

        let left = row![brand, Space::with_width(12), breadcrumb]
            .spacing(8)
            .align_y(iced::Alignment::Center);

        let run_label = if self.running {
            "RUNNING (16MHz)"
        } else {
            "PAUSED (16MHz)"
        };
        let run_color = if self.running {
            ACCENT_EMERALD
        } else {
            TEXT_MUTED
        };
        let clock = container(
            row![
                text("●").size(10).color(run_color),
                text(run_label)
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(run_color),
                text("|").size(10).color(TEXT_MUTED),
                text(format!("T: {}", format_elapsed(self.elapsed_ms)))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding([4, 10])
        .style(|_t| container::Style {
            background: Some(surface_lowest_bg()),
            border: iced::Border {
                color: BORDER,
                width: 1.0,
                radius: 999.0.into(),
            },
            ..container::Style::default()
        });

        let pause_label = if self.running { "PAUSE" } else { "RESUME" };
        let transport = row![
            button(text(pause_label).size(12))
                .padding([4, 10])
                .style(pause_btn)
                .on_press(Message::TogglePause),
            button(text("STEP 1ms").size(11).font(Font::MONOSPACE))
                .padding([4, 8])
                .style(transport_btn)
                .on_press(Message::SimStep),
            button(text("RESET").size(11).font(Font::MONOSPACE))
                .padding([4, 8])
                .style(transport_btn)
                .on_press(Message::SimReset),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        let center = row![clock, transport]
            .spacing(8)
            .align_y(iced::Alignment::Center);

        let tabs = container(
            row![
                mode_tab("Simulation", Mode::Simulation, self.mode),
                mode_tab("Code", Mode::Code, self.mode),
                mode_tab("Ladder", Mode::Ladder, self.mode),
                mode_tab("GRAFCET", Mode::Grafcet, self.mode),
            ]
            .spacing(2),
        )
        .padding(2)
        .style(|_t| container::Style {
            background: Some(surface_lowest_bg()),
            border: iced::Border {
                color: BORDER,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        });

        let right = row![
            tabs,
            button(text("Export").size(12))
                .padding([4, 10])
                .style(export_btn)
                .on_press(Message::Export),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        container(
            row![
                container(left).width(Length::FillPortion(3)),
                container(center)
                    .width(Length::FillPortion(2))
                    .center_x(Fill),
                container(right)
                    .width(Length::FillPortion(2))
                    .align_right(Fill),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .width(Fill),
        )
        .padding([8, 12])
        .width(Fill)
        .style(|_t| container::Style {
            background: Some(panel_bg()),
            border: iced::Border {
                color: BORDER,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
    }

    fn status_footer(&self) -> Element<'_, Message> {
        let left = row![
            text("●").size(10).color(ACCENT_EMERALD),
            text("AVR Core: Normal")
                .size(11)
                .font(Font::MONOSPACE)
                .color(TEXT_MAIN),
            text("SRAM: 184 / 2048 B (8.9%)")
                .size(11)
                .font(Font::MONOSPACE)
                .color(TEXT_MUTED),
            text("Flash: 1,842 / 32,256 B (5.7%)")
                .size(11)
                .font(Font::MONOSPACE)
                .color(TEXT_MUTED),
            text(&self.status)
                .size(11)
                .font(Font::MONOSPACE)
                .color(ACCENT_SKY),
        ]
        .spacing(16)
        .align_y(iced::Alignment::Center);

        let right = row![
            text("Grid: 10mm Snap")
                .size(11)
                .font(Font::MONOSPACE)
                .color(TEXT_MUTED),
            text("|").size(11).color(BORDER),
            text("Help & Pinout Docs")
                .size(11)
                .font(Font::MONOSPACE)
                .color(ACCENT_ORANGE),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        container(
            row![left, Space::with_width(Fill), right]
                .align_y(iced::Alignment::Center)
                .width(Fill),
        )
        .padding([4, 12])
        .width(Fill)
        .style(|_t| container::Style {
            background: Some(panel_bg()),
            border: iced::Border {
                color: BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        match self.mode {
            Mode::Simulation => self.simulation_workspace(),
            Mode::Code => self.code_workspace(),
            Mode::Ladder => self.ladder_workspace(),
            Mode::Grafcet => self.grafcet_workspace(),
        }
    }

    fn simulation_workspace(&self) -> Element<'_, Message> {
        let body = if self.left_collapsed {
            row![self.sim_canvas_pane()].spacing(0).height(Fill)
        } else {
            row![self.component_palette(), self.sim_canvas_pane()]
                .spacing(0)
                .height(Fill)
        };
        container(body).width(Fill).height(Fill).into()
    }

    fn component_palette(&self) -> Element<'_, Message> {
        let header = column![
            row![
                text("COMPONENTS & PARTS")
                    .size(12)
                    .color(TEXT_HEADING),
                Space::with_width(Fill),
                text("7 items").size(10).font(Font::MONOSPACE).color(TEXT_MUTED),
                button(text("«").size(12))
                    .padding([2, 6])
                    .style(chrome_btn)
                    .on_press(Message::ToggleLeftDock),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            text_input("Search components...", &self.palette_filter)
                .on_input(Message::PaletteFilter)
                .padding(6)
                .size(12),
            row![
                chip("All", true),
                chip("Basic", false),
                chip("Sensors", false),
                chip("Integrated", false),
            ]
            .spacing(4),
        ]
        .spacing(8)
        .padding(12);

        let parts = [
            ("Resistor", "Axial THT (catalog)", "PASSIVE"),
            ("Red LED", "5mm diffuse · int. 220Ω", "OUTPUT"),
            ("Tactile Switch", "4-pin momentary", "INPUT"),
            ("Capacitor", "100nF / 10μF", "PASSIVE"),
            ("Uno R3", "ATmega328P 16MHz", "MCU"),
            ("Piezo Buzzer", "5V active sounder", "OUTPUT"),
            ("Slide Switch", "SPDT 3-pin toggle", "SWITCH"),
        ];

        let filter = self.palette_filter.to_ascii_lowercase();
        let mut grid: Column<'_, Message> = Column::new().spacing(8);
        let mut row_buf: Vec<Element<'_, Message>> = Vec::new();
        for (name, desc, kind) in parts {
            if !filter.is_empty()
                && !name.to_ascii_lowercase().contains(&filter)
                && !desc.to_ascii_lowercase().contains(&filter)
            {
                continue;
            }
            let active = self.selected_part == Some(name);
            let style = if active {
                palette_card_active
            } else {
                palette_card
            };
            let card = button(
                column![
                    row![
                        text(name).size(12).color(TEXT_MAIN),
                        Space::with_width(Fill),
                        text(kind).size(9).font(Font::MONOSPACE).color(TEXT_MUTED),
                    ]
                    .width(Fill),
                    text(desc).size(10).font(Font::MONOSPACE).color(TEXT_MUTED),
                ]
                .spacing(4)
                .width(Fill),
            )
            .padding(10)
            .width(Fill)
            .style(style)
            .on_press(Message::SelectPart(name));
            row_buf.push(card.into());
            if row_buf.len() == 2 {
                let a = row_buf.remove(0);
                let b = row_buf.remove(0);
                grid = grid.push(row![a, b].spacing(8));
            }
        }
        if let Some(last) = row_buf.pop() {
            grid = grid.push(row![last, Space::with_width(Fill)].spacing(8));
        }

        container(
            column![
                header,
                scrollable(container(grid).padding(12)).height(Fill),
            ]
            .height(Fill),
        )
        .width(Length::Fixed(280.0))
        .height(Fill)
        .style(|_t| container::Style {
            background: Some(panel_bg()),
            border: iced::Border {
                color: BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
    }

    fn sim_canvas_pane(&self) -> Element<'_, Message> {
        let mut col = Column::new().spacing(12).padding(16);
        if self.left_collapsed {
            col = col.push(
                button(text("» Components").size(11))
                    .padding([4, 8])
                    .style(chrome_btn)
                    .on_press(Message::ToggleLeftDock),
            );
        }
        col = col.push(
            text("Board canvas — Uno R3 · LED (integrated R) · button D2↔GND")
                .size(14)
                .color(TEXT_HEADING),
        );
        col = col.push(
            text("D2 · MCU pull-up  ·  pressed → active-LOW  ·  ASL: pressed = NOT GET_BOOL(btnPin)")
                .size(11)
                .font(Font::MONOSPACE)
                .color(ACCENT_CYAN),
        );

        match &self.canvas {
            None => {
                col = col.push(
                    text("Canvas not loaded — open C01 or a project with MVP wiring.")
                        .color(TEXT_MUTED),
                );
                col = col.push(
                    row![
                        button("Open C01")
                            .style(chrome_btn)
                            .on_press(Message::OpenCanvasDemo),
                        button("Open P01")
                            .style(chrome_btn)
                            .on_press(Message::OpenDefault),
                    ]
                    .spacing(8),
                );
            }
            Some(c) => {
                let frame = c.current_frame();
                let pull = if c.binding.button_active_low {
                    "pullup"
                } else {
                    "none"
                };
                col = col.push(
                    text(format!(
                        "cycle={}  t={}ms  btn=D{} ({})  led=D{}  button={}  led={}",
                        frame.cycle,
                        frame.time_ms,
                        c.binding.button_pin,
                        pull,
                        c.binding.led_pin,
                        if frame.button_pressed {
                            "pressed"
                        } else {
                            "released"
                        },
                        if frame.led_on { "ON" } else { "off" },
                    ))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
                );
                if !self.canvas_note.is_empty() {
                    col = col.push(text(&self.canvas_note).size(11).color(TEXT_MUTED));
                }

                // iced SVG widgets panic on damage-sort under this stack; show a
                // schematic board plane + live LED/button indicators instead.
                // SVG binding remains in dendri-canvas (CI snapshots).
                let led_lit = frame.led_on;
                let btn_down = frame.button_pressed;
                let board_plane = container(
                    column![
                        text("Arduino Uno R3").size(16).color(ACCENT_CYAN),
                        text("ATmega328P · 16MHz")
                            .size(11)
                            .font(Font::MONOSPACE)
                            .color(TEXT_MUTED),
                        Space::with_height(12),
                        text(format!(
                            "D2  btnPin   DI  pull={}  level={}",
                            if c.binding.button_active_low {
                                "pullup"
                            } else {
                                "none"
                            },
                            if c.sim.pin(c.binding.button_pin) {
                                "HIGH"
                            } else {
                                "LOW"
                            }
                        ))
                        .size(12)
                        .font(Font::MONOSPACE)
                        .color(ACCENT_CYAN),
                        text(format!(
                            "D13 ledPin   DO  {}",
                            if led_lit { "HIGH · LED_BUILTIN" } else { "LOW" }
                        ))
                        .size(12)
                        .font(Font::MONOSPACE)
                        .color(ACCENT_ORANGE),
                        Space::with_height(12),
                        text("Wiring")
                            .size(12)
                            .color(TEXT_HEADING),
                        text("D2 ── btn_start.NO")
                            .size(12)
                            .font(Font::MONOSPACE)
                            .color(ACCENT_EMERALD),
                        text("GND ── btn_start.COM")
                            .size(12)
                            .font(Font::MONOSPACE)
                            .color(TEXT_MUTED),
                        text("D13 ── led1.A  (integrated 220Ω)")
                            .size(12)
                            .font(Font::MONOSPACE)
                            .color(ACCENT_SKY),
                        text("GND ── led1.K")
                            .size(12)
                            .font(Font::MONOSPACE)
                            .color(TEXT_MUTED),
                        Space::with_height(8),
                        text("Callout: D2 · MCU pull-up · pressed → active-LOW")
                            .size(11)
                            .font(Font::MONOSPACE)
                            .color(ACCENT_ORANGE),
                    ]
                    .spacing(4)
                    .padding(16),
                )
                .width(Length::Fixed(480.0))
                .height(Length::Fixed(300.0))
                .style(|_t| container::Style {
                    background: Some(surface_lowest_bg()),
                    border: iced::Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..container::Style::default()
                });

                let live_led = container(
                    column![
                        text("led1").size(11).font(Font::MONOSPACE).color(TEXT_MUTED),
                        text(if led_lit { "● ON" } else { "○ off" })
                            .size(22)
                            .color(if led_lit {
                                Color::from_rgb(1.0, 0.35, 0.2)
                            } else {
                                TEXT_MUTED
                            }),
                        text("integrated_resistance=220")
                            .size(10)
                            .font(Font::MONOSPACE)
                            .color(TEXT_MUTED),
                    ]
                    .spacing(4)
                    .align_x(iced::Alignment::Center),
                )
                .padding(16)
                .width(Length::Fixed(160.0))
                .style(move |_t| container::Style {
                    background: Some(iced::Background::Color(if led_lit {
                        Color::from_rgb(0.35, 0.08, 0.05)
                    } else {
                        SURFACE_LOWEST
                    })),
                    border: iced::Border {
                        color: if led_lit { ACCENT_ORANGE } else { BORDER },
                        width: 2.0,
                        radius: 4.0.into(),
                    },
                    ..container::Style::default()
                });

                let live_btn = container(
                    column![
                        text("btn_start")
                            .size(11)
                            .font(Font::MONOSPACE)
                            .color(TEXT_MUTED),
                        text(if btn_down { "▼ PRESSED" } else { "△ released" })
                            .size(18)
                            .color(if btn_down {
                                ACCENT_EMERALD
                            } else {
                                TEXT_MUTED
                            }),
                        text("NO · D2–GND")
                            .size(10)
                            .font(Font::MONOSPACE)
                            .color(TEXT_MUTED),
                    ]
                    .spacing(4)
                    .align_x(iced::Alignment::Center),
                )
                .padding(16)
                .width(Length::Fixed(160.0))
                .style(move |_t| container::Style {
                    background: Some(surface_lowest_bg()),
                    border: iced::Border {
                        color: if btn_down { ACCENT_EMERALD } else { BORDER },
                        width: 2.0,
                        radius: 4.0.into(),
                    },
                    ..container::Style::default()
                });

                col = col.push(
                    row![
                        board_plane,
                        column![
                            live_led,
                            live_btn,
                            text("Actuate").size(11).color(TEXT_MUTED),
                            button(text("Press").size(12))
                                .padding([6, 14])
                                .style(pause_btn)
                                .on_press(Message::SimPress),
                            button(text("Release").size(12))
                                .padding([6, 14])
                                .style(chrome_btn)
                                .on_press(Message::SimRelease),
                            button(text("Step scan").size(12))
                                .padding([6, 14])
                                .style(chrome_btn)
                                .on_press(Message::SimStep),
                        ]
                        .spacing(8)
                        .padding(8),
                    ]
                    .spacing(16)
                    .align_y(iced::Alignment::Start),
                );
            }
        }

        container(scrollable(col))
            .width(Fill)
            .height(Fill)
            .style(|_t| container::Style {
                background: Some(surface_bg()),
                ..container::Style::default()
            })
            .into()
    }

    fn code_workspace(&self) -> Element<'_, Message> {
        let tree = container(
            column![
                text("EXPLORER").size(12).color(TEXT_HEADING),
                text("projects/button_state_change")
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
                Space::with_height(8),
                text("project.toml")
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(ACCENT_ORANGE),
                text("build/").size(12).font(Font::MONOSPACE).color(TEXT_MUTED),
                text("  arduino/main.cpp")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
                text("  st/main.st")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
                text("  micropython/main.py")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
                Space::with_height(12),
                text("ASL SYMBOLS").size(11).color(TEXT_MUTED),
                text("pin  btnPin  D2 · DI · pullup")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(ACCENT_CYAN),
                text("pin  ledPin  D13 · DO")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(ACCENT_SKY),
                text("var  pressed  BOOL")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MAIN),
                Space::with_height(12),
                button("ASL → views")
                    .style(chrome_btn)
                    .on_press(Message::ReloadFromAsl),
                button("Save")
                    .style(chrome_btn)
                    .on_press(Message::Save),
            ]
            .spacing(4)
            .padding(12),
        )
        .width(Length::Fixed(240.0))
        .height(Fill)
        .style(|_t| container::Style {
            background: Some(surface_lowest_bg()),
            border: iced::Border {
                color: BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });

        let editor = column![
            text("project.toml — sole source of truth")
                .size(14)
                .color(TEXT_HEADING),
            text("Hardware pull=pullup · LED integrated_resistance · ASL NOT GET_BOOL")
                .size(11)
                .font(Font::MONOSPACE)
                .color(TEXT_MUTED),
            text_editor(&self.project_toml)
                .height(Fill)
                .on_action(Message::ProjectTomlAction),
        ]
        .spacing(8)
        .padding(12)
        .width(Fill)
        .height(Fill);

        row![tree, editor].height(Fill).into()
    }

    fn ladder_workspace(&self) -> Element<'_, Message> {
        let palette = container(
            column![
                text("INSTRUCTIONS · IEC 61131-3")
                    .size(12)
                    .color(TEXT_HEADING),
                Space::with_height(8),
                button("+ NO contact")
                    .style(chrome_btn)
                    .on_press(Message::AddContactNo),
                button("+ Coil")
                    .style(chrome_btn)
                    .on_press(Message::AddCoil),
                button("+ SET_BOOL")
                    .style(chrome_btn)
                    .on_press(Message::AddActionSetBool),
                button("+ Parallel OR")
                    .style(chrome_btn)
                    .on_press(Message::AddParallelOr),
                button("+ TON")
                    .style(chrome_btn)
                    .on_press(Message::AddTon),
                button("+ R_TRIG")
                    .style(chrome_btn)
                    .on_press(Message::AddRTrig),
                button("- Cell")
                    .style(chrome_btn)
                    .on_press(Message::RemoveLastLadderCell),
            ]
            .spacing(6)
            .padding(12),
        )
        .width(Length::Fixed(220.0))
        .height(Fill)
        .style(|_t| container::Style {
            background: Some(surface_low_bg()),
            border: iced::Border {
                color: BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });

        let mut diagram = Column::new().spacing(8).padding(12);
        diagram = diagram.push(text("Ladder rungs").size(16).color(TEXT_HEADING));
        if let Some(block) = self.session.ladder_model() {
            for rung in &block.rungs {
                let mut line = String::from("|");
                for cell in &rung.cells {
                    line.push_str(&format!("-[{}]-", cell_label(cell)));
                }
                line.push('|');
                diagram = diagram.push(text(line).font(Font::MONOSPACE).color(TEXT_MAIN));
                if let Some(c) = &rung.comment {
                    diagram = diagram.push(
                        text(format!("  // {c}"))
                            .size(12)
                            .color(TEXT_MUTED),
                    );
                }
            }
        } else {
            diagram = diagram.push(text("(empty — use palette)").color(TEXT_MUTED));
        }

        let right = column![
            container(scrollable(diagram)).width(Fill).height(Fill),
            container(
                column![
                    text("Ladder TOML (live sync)").size(12).color(TEXT_MUTED),
                    text_editor(&self.toml_content)
                        .height(Length::Fixed(180.0))
                        .on_action(Message::TomlAction),
                ]
                .spacing(4)
                .padding(8),
            )
            .style(|_t| container::Style {
                background: Some(surface_lowest_bg()),
                ..container::Style::default()
            }),
        ]
        .width(Fill)
        .height(Fill);

        row![palette, right].height(Fill).into()
    }

    fn grafcet_workspace(&self) -> Element<'_, Message> {
        let palette = container(
            column![
                text("GRAFCET · IEC 60848").size(12).color(TEXT_HEADING),
                Space::with_height(8),
                button("+ Step")
                    .style(chrome_btn)
                    .on_press(Message::AddSfcStep),
                button("+ Transition")
                    .style(chrome_btn)
                    .on_press(Message::AddSfcTransition),
                Space::with_height(8),
                button("ASL → views")
                    .style(chrome_btn)
                    .on_press(Message::ReloadFromAsl),
            ]
            .spacing(6)
            .padding(12),
        )
        .width(Length::Fixed(220.0))
        .height(Fill)
        .style(|_t| container::Style {
            background: Some(surface_low_bg()),
            border: iced::Border {
                color: BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });

        let mut chart = Column::new().spacing(8).padding(12);
        chart = chart.push(
            text("C01-button-led.GRAFCET")
                .size(16)
                .color(TEXT_HEADING),
        );
        if let Some(block) = self.session.sfc_model() {
            chart = chart.push(
                text(format!(
                    "{} INITIAL STEP · {} TRANSITIONS",
                    block.steps.iter().filter(|s| s.initial).count(),
                    block.transitions.len()
                ))
                .size(11)
                .font(Font::MONOSPACE)
                .color(ACCENT_EMERALD),
            );
            for step in &block.steps {
                let mark = if step.initial { "[*]" } else { "[ ]" };
                chart = chart.push(text(format!(
                    "{mark} {}  ({:.0},{:.0})  actions={}",
                    step.id,
                    step.x,
                    step.y,
                    step.actions.len()
                )));
            }
            for tr in &block.transitions {
                chart = chart.push(text(format!(
                    "  -- {} -> {:?} -> {:?}",
                    tr.id, tr.from, tr.to
                )));
            }
        } else {
            chart = chart.push(text("(empty chart — use palette)").color(TEXT_MUTED));
        }

        let right = column![
            container(scrollable(chart)).width(Fill).height(Fill),
            container(
                column![
                    text("SFC TOML (live sync)").size(12).color(TEXT_MUTED),
                    text_editor(&self.toml_content)
                        .height(Length::Fixed(180.0))
                        .on_action(Message::TomlAction),
                ]
                .spacing(4)
                .padding(8),
            )
            .style(|_t| container::Style {
                background: Some(surface_lowest_bg()),
                ..container::Style::default()
            }),
        ]
        .width(Fill)
        .height(Fill);

        row![palette, right].height(Fill).into()
    }

    fn boot_canvas_from_session(&mut self) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        match SimCanvas::from_project(&self.session.project, &root, &SimConfig::default()) {
            Ok(mut c) => match c.boot() {
                Ok(frame) => {
                    self.canvas_note = frame.to_snapshot().trim().replace('\n', " | ");
                    self.canvas = Some(c);
                }
                Err(e) => {
                    self.canvas = None;
                    self.canvas_note = format!("boot failed: {e}");
                }
            },
            Err(e) => {
                self.canvas = None;
                self.canvas_note = format!("canvas load failed: {e}");
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

    fn refresh_project_toml(&mut self) {
        let text = self
            .session
            .emit_project_toml()
            .unwrap_or_else(|_| String::new());
        self.project_toml = text_editor::Content::with_text(&text);
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
        self.mode = Mode::Ladder;
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
        self.mode = Mode::Grafcet;
        self.refresh_toml_from_session();
        Ok(())
    }
}

fn mode_tab(label: &'static str, mode: Mode, current: Mode) -> Element<'static, Message> {
    let active = mode == current;
    button(text(label).size(12).font(Font::MONOSPACE))
        .padding([4, 10])
        .style(if active { tab_active } else { tab_idle })
        .on_press(Message::SwitchMode(mode))
        .into()
}

fn chip(label: &'static str, active: bool) -> Element<'static, Message> {
    let color = if active { ACCENT_ORANGE } else { TEXT_MUTED };
    container(text(label).size(11).font(Font::MONOSPACE).color(color))
        .padding([2, 8])
        .style(move |_t| container::Style {
            background: Some(if active {
                surface_low_bg()
            } else {
                iced::Background::Color(Color::TRANSPARENT)
            }),
            border: iced::Border {
                color: BORDER,
                width: if active { 1.0 } else { 0.0 },
                radius: 4.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Simulation => "Simulation",
        Mode::Code => "Code",
        Mode::Ladder => "Ladder",
        Mode::Grafcet => "GRAFCET",
    }
}

fn format_elapsed(ms: u64) -> String {
    let total_s = ms / 1000;
    let m = total_s / 60;
    let s = total_s % 60;
    let frac = ms % 1000;
    format!("{m:02}:{s:02}.{frac:03}s")
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
        LadderCell::FbEdge {
            rising, instance, ..
        } => format!(
            "[{}:{}]",
            if *rising { "R_TRIG" } else { "F_TRIG" },
            instance
        ),
        LadderCell::FbCompare { op, .. } => format!("[{op}]"),
        LadderCell::Parallel { branches, .. } => {
            format!("[||*{}]", branches.len())
        }
    }
}

fn default_project_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/fixtures/projects/P01-uno-led-blink.toml")
}

fn canvas_demo_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/fixtures/canvas/C01-button-led.toml")
}

fn export_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/export-project.toml")
}

fn load_session(path: &std::path::Path) -> Result<SyncSession, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    SyncSession::from_project_toml(&text).map_err(|e| e.to_string())
}

fn empty_project() -> dendri_ast::SimulationProject {
    dendri_ast::SimulationProject {
        metadata: dendri_ast::Metadata {
            asl_version: Some(dendri_ast::ASL_VERSION.into()),
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
                asl_version: Some(dendri_ast::ASL_VERSION.into()),
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
