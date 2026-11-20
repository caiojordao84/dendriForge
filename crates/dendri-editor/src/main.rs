//! DendriForge iced editor — prototype chrome + authoring loop.
//!
//! Visual SoT: Caio iced 0.13 prototype (`docs/canvas-integration/`).
//! Utility: SyncSession hub — place/wire, edit code, Ladder/GRAFCET palette +
//! TOML, parse → ASL IR → generators (same pipeline as `dendri export`).

mod authoring;
mod project_io;
#[allow(dead_code)] // retained C01 reference chrome; Simulation uses world canvas
mod schematic;
mod sim_world;
mod theme;

use std::path::PathBuf;
use std::time::Duration;

use authoring::{AuthoringHub, Placeable};
use dendri_ast::SourceLanguage;
use dendri_canvas::{Camera, PhaseAScene, SimCanvas, ZOOM_MAX, ZOOM_MIN};
use dendri_sim::SimConfig;
use iced::{
    alignment, mouse,
    widget::{
        button, canvas, checkbox, column, container, row, scrollable, text, text_editor, text_input,
        Space,
    },
    Color, Element, Font, Length, Point, Rectangle, Renderer, Size, Subscription, Theme,
};
use project_io::{export_project, load_project, save_project};
use sim_world::WorldCanvas;
use theme as ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Simulation,
    Code,
    Ladder,
    Grafcet,
}
impl View {
    const ALL: [Self; 4] = [Self::Simulation, Self::Code, Self::Ladder, Self::Grafcet];
    fn label(self) -> &'static str {
        match self {
            Self::Simulation => "Simulation",
            Self::Code => "Code",
            Self::Ladder => "Ladder",
            Self::Grafcet => "GRAFCET",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Cpp,
    St,
    Python,
    Toml,
}
impl Source {
    const ALL: [Self; 4] = [Self::Cpp, Self::St, Self::Python, Self::Toml];
    fn label(self) -> &'static str {
        match self {
            Self::Cpp => "arduino/main.cpp",
            Self::St => "st/main.st",
            Self::Python => "micropython/main.py",
            Self::Toml => "project.toml",
        }
    }
    fn language(self) -> SourceLanguage {
        match self {
            Self::Cpp => SourceLanguage::Cpp,
            Self::St => SourceLanguage::St,
            Self::Python => SourceLanguage::Micropython,
            Self::Toml => SourceLanguage::Asl,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    View(View),
    Source(Source),
    Run,
    Step,
    Reset,
    Tick,
    Raw(bool),
    Physical(bool),
    ToggleRaw,
    Left,
    Right,
    Bottom,
    Zoom(f32),
    /// Scroll-wheel zoom about a view-space anchor (Simulation world canvas).
    CameraZoomAt {
        factor: f32,
        anchor_x: f32,
        anchor_y: f32,
    },
    /// Middle-mouse pan in view pixels (Simulation world canvas).
    CameraPanBy {
        dx_px: f32,
        dy_px: f32,
    },
    Copy,
    PathInput(String),
    OpenPath,
    OpenC01,
    NewBlank,
    Save,
    Export,
    Place(Placeable),
    WireFrom(String),
    WireTo(String),
    WireSignal(String),
    AddWire,
    RemoveComponent(String),
    RemoveWire(usize),
    RebuildSim,
    CodeAction(text_editor::Action),
    ApplyCode,
    GenerateFromAsl,
    VisualAction(text_editor::Action),
    ApplyVisual,
    LadderContactNo,
    LadderContactNc,
    LadderCoil,
    LadderRemoveLast,
    SfcAddStep,
    SfcAddTransition,
}

struct App {
    view: View,
    source: Source,
    running: bool,
    cycles: u64,
    raw: bool,
    pressed: bool,
    led: bool,
    left: bool,
    right: bool,
    bottom: bool,
    /// Shared zoom readout (Ladder/GRAFCET scale + Simulation camera.zoom).
    zoom: f32,
    /// Simulation world camera (pan + zoom). Infinite line grid; no page bounds.
    camera: Camera,
    /// Phase A placeholders (Uno + LED) in logical grid units.
    phase_a: PhaseAScene,
    canvas: Option<SimCanvas>,
    hub: Option<AuthoringHub>,
    project_path: PathBuf,
    path_input: String,
    project_label: String,
    status: String,
    code_editor: text_editor::Content,
    visual_editor: text_editor::Content,
    wire_from: String,
    wire_to: String,
    wire_signal: String,
    code_dirty: bool,
    visual_dirty: bool,
}

impl App {
    fn boot() -> (Self, iced::Task<Message>) {
        let project_path = startup_project_path();
        let mut app = Self {
            view: View::Simulation,
            source: Source::Cpp,
            running: true,
            cycles: 0,
            raw: true,
            pressed: false,
            led: false,
            left: true,
            right: true,
            bottom: true,
            zoom: 1.0,
            camera: Camera::default(),
            phase_a: PhaseAScene::default(),
            canvas: None,
            hub: None,
            project_path: project_path.clone(),
            path_input: project_path.display().to_string(),
            project_label: "C01-button-led".into(),
            status: String::new(),
            code_editor: text_editor::Content::new(),
            visual_editor: text_editor::Content::new(),
            wire_from: "board.D13".into(),
            wire_to: "led1.A".into(),
            wire_signal: "digital".into(),
            code_dirty: false,
            visual_dirty: false,
        };
        app.load_into_hub(project_path);
        (app, iced::Task::none())
    }

    fn load_into_hub(&mut self, path: PathBuf) {
        match load_project(&path) {
            Ok(project) => {
                let hub = AuthoringHub::from_project(project);
                self.project_label = project_io::project_display_name(hub.project(), &path);
                self.project_path = path.clone();
                self.path_input = path.display().to_string();
                self.hub = Some(hub);
                self.sync_editors_from_hub();
                self.boot_canvas_from_hub();
            }
            Err(e) => {
                self.hub = None;
                self.canvas = None;
                self.status = format!("project load failed: {e}");
            }
        }
    }

    fn sync_editors_from_hub(&mut self) {
        let Some(hub) = self.hub.as_ref() else {
            return;
        };
        let code = match self.source {
            Source::Cpp => hub.projections.cpp.as_str(),
            Source::St => hub.projections.st.as_str(),
            Source::Python => hub.projections.python.as_str(),
            Source::Toml => hub.projections.toml.as_str(),
        };
        self.code_editor = text_editor::Content::with_text(code);
        self.code_dirty = false;
        let visual = match self.view {
            View::Grafcet => hub.sfc_toml.as_str(),
            _ => hub.ladder_toml.as_str(),
        };
        self.visual_editor = text_editor::Content::with_text(visual);
        self.visual_dirty = false;
    }

    fn boot_canvas_from_hub(&mut self) {
        let Some(hub) = self.hub.as_ref() else {
            self.canvas = None;
            return;
        };
        let root = repo_root();
        match SimCanvas::from_project(hub.project(), &root, &SimConfig::default()) {
            Ok(mut canvas) => match canvas.boot() {
                Ok(_) => {
                    self.raw = canvas.sim.pin(canvas.binding.button_pin);
                    self.pressed = false;
                    self.led = false;
                    self.cycles = 0;
                    self.canvas = Some(canvas);
                    self.status = format!(
                        "loaded {} · dendri-sim · parsers/generators live · no AVR/serial",
                        self.project_path.display()
                    );
                }
                Err(e) => {
                    self.canvas = None;
                    self.status = format!("sim boot failed: {e}");
                }
            },
            Err(e) => {
                self.canvas = None;
                self.status = format!("canvas load failed: {e} (project still editable)");
            }
        }
    }

    fn refresh_after_authoring(&mut self, note: impl Into<String>) {
        self.sync_editors_from_hub();
        self.boot_canvas_from_hub();
        self.status = note.into();
    }

    fn scan(&mut self) {
        let Some(canvas) = self.canvas.as_mut() else {
            self.pressed = !self.raw;
            self.led = self.pressed;
            self.cycles = self.cycles.saturating_add(1);
            return;
        };
        match canvas.step() {
            Ok(frame) => {
                let raw = canvas.sim.pin(canvas.binding.button_pin);
                let pressed = match canvas.sim.var("pressed") {
                    Some(dendri_sim::Value::Bool(v)) => *v,
                    _ => frame.button_pressed,
                };
                self.cycles = frame.cycle;
                self.led = frame.led_on;
                self.raw = raw;
                self.pressed = pressed;
                self.status = format!(
                    "scan {} · btnPin={} pressed={} ledPin={}",
                    self.cycles,
                    bit(self.raw),
                    bit(self.pressed),
                    bit(self.led)
                );
            }
            Err(e) => self.status = format!("scan error: {e}"),
        }
    }

    fn apply_physical(&mut self, down: bool) {
        self.raw = !down;
        if let Some(canvas) = self.canvas.as_mut() {
            canvas.set_button(down);
            self.raw = canvas.sim.pin(canvas.binding.button_pin);
        }
        if self.running {
            self.scan();
        }
    }

    fn apply_raw(&mut self, raw: bool) {
        self.raw = raw;
        if let Some(canvas) = self.canvas.as_mut() {
            let pressed = if canvas.binding.button_active_low {
                !raw
            } else {
                raw
            };
            canvas.set_button(pressed);
            self.raw = canvas.sim.pin(canvas.binding.button_pin);
        }
        if self.running {
            self.scan();
        }
    }

    fn apply_code(&mut self) {
        let Some(hub) = self.hub.as_mut() else {
            self.status = "apply failed: no project".into();
            return;
        };
        let text = self.code_editor.text();
        match hub.apply_code(self.source.language(), &text) {
            Ok(()) => {
                self.code_dirty = false;
                let note = format!(
                    "parsed {} → ASL IR → regenerated C++/ST/MPY/Ladder/GRAFCET",
                    self.source.label()
                );
                self.sync_editors_from_hub();
                self.boot_canvas_from_hub();
                self.status = note;
            }
            Err(e) => self.status = format!("parse/apply failed: {e}"),
        }
    }

    fn apply_visual(&mut self) {
        let Some(hub) = self.hub.as_mut() else {
            self.status = "apply failed: no project".into();
            return;
        };
        let text = self.visual_editor.text();
        let result = if self.view == View::Grafcet {
            hub.apply_sfc_toml(&text)
        } else {
            hub.apply_ladder_toml(&text)
        };
        match result {
            Ok(()) => {
                self.visual_dirty = false;
                self.refresh_after_authoring("visual TOML → ASL IR → generators refreshed");
            }
            Err(e) => self.status = format!("visual apply failed: {e}"),
        }
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::View(view) => {
                self.view = view;
                if matches!(view, View::Ladder | View::Grafcet) {
                    self.sync_editors_from_hub();
                }
            }
            Message::Source(source) => {
                self.source = source;
                self.sync_editors_from_hub();
            }
            Message::Run => self.running = !self.running,
            Message::Step => {
                self.running = false;
                self.scan();
            }
            Message::Reset => {
                let was = self.running;
                self.boot_canvas_from_hub();
                self.running = was;
            }
            Message::Tick => {
                if self.running {
                    self.scan();
                }
            }
            Message::Raw(raw) => self.apply_raw(raw),
            Message::Physical(down) => self.apply_physical(down),
            Message::ToggleRaw => self.apply_raw(!self.raw),
            Message::Left => self.left = !self.left,
            Message::Right => self.right = !self.right,
            Message::Bottom => self.bottom = !self.bottom,
            Message::Zoom(delta) => {
                let z = Camera::clamp_zoom((self.zoom + delta) as f64) as f32;
                self.zoom = z;
                self.camera.zoom = z as f64;
            }
            Message::CameraZoomAt {
                factor,
                anchor_x,
                anchor_y,
            } => {
                self.camera
                    .zoom_by_at_view(factor as f64, anchor_x as f64, anchor_y as f64);
                self.zoom = self.camera.zoom as f32;
            }
            Message::CameraPanBy { dx_px, dy_px } => {
                self.camera
                    .pan_by_view_delta(dx_px as f64, dy_px as f64);
            }
            Message::Copy => {
                return iced::clipboard::write(self.code_editor.text());
            }
            Message::PathInput(s) => self.path_input = s,
            Message::OpenPath => {
                let path = PathBuf::from(self.path_input.trim());
                if path.as_os_str().is_empty() {
                    self.status = "open failed: empty path".into();
                } else {
                    self.load_into_hub(path);
                }
            }
            Message::OpenC01 => self.load_into_hub(c01_path()),
            Message::NewBlank => {
                let path = repo_root().join("build/untitled.toml");
                let hub = AuthoringHub::from_project(AuthoringHub::blank_uno_project());
                self.project_path = path.clone();
                self.path_input = path.display().to_string();
                self.project_label = "untitled".into();
                self.hub = Some(hub);
                self.sync_editors_from_hub();
                self.boot_canvas_from_hub();
                self.status =
                    "blank Uno project · place components, wire, write code, Apply".into();
            }
            Message::Save => {
                let Some(hub) = self.hub.as_ref() else {
                    self.status = "save failed: no project".into();
                    return iced::Task::none();
                };
                match save_project(&self.project_path, hub.project()) {
                    Ok(()) => {
                        self.status = format!(
                            "saved {} · project TOML (ASL SoT) · no firmware flash",
                            self.project_path.display()
                        );
                    }
                    Err(e) => self.status = format!("save failed: {e}"),
                }
            }
            Message::Export => {
                let Some(hub) = self.hub.as_ref() else {
                    self.status = "export failed: no project".into();
                    return iced::Task::none();
                };
                let out = project_io::default_export_dir(&repo_root());
                match export_project(hub.project(), &out) {
                    Ok(files) => {
                        self.status = format!(
                            "exported {} files → {} · same as `dendri export`",
                            files.len(),
                            out.display()
                        );
                    }
                    Err(e) => self.status = format!("export failed: {e}"),
                }
            }
            Message::Place(kind) => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.place(kind) {
                        Ok(note) => self.refresh_after_authoring(note),
                        Err(e) => self.status = format!("place failed: {e}"),
                    }
                }
            }
            Message::WireFrom(s) => self.wire_from = s,
            Message::WireTo(s) => self.wire_to = s,
            Message::WireSignal(s) => self.wire_signal = s,
            Message::AddWire => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.add_wire(&self.wire_from, &self.wire_to, Some(&self.wire_signal)) {
                        Ok(()) => self.refresh_after_authoring(format!(
                            "wired {} → {}",
                            self.wire_from, self.wire_to
                        )),
                        Err(e) => self.status = format!("wire failed: {e}"),
                    }
                }
            }
            Message::RemoveComponent(id) => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.remove_component(&id) {
                        Ok(()) => self.refresh_after_authoring(format!("removed {id}")),
                        Err(e) => self.status = format!("remove failed: {e}"),
                    }
                }
            }
            Message::RemoveWire(i) => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.remove_wire_at(i) {
                        Ok(()) => self.refresh_after_authoring(format!("removed wire #{i}")),
                        Err(e) => self.status = format!("remove wire failed: {e}"),
                    }
                }
            }
            Message::RebuildSim => {
                self.boot_canvas_from_hub();
            }
            Message::CodeAction(action) => {
                self.code_editor.perform(action);
                self.code_dirty = true;
            }
            Message::ApplyCode => self.apply_code(),
            Message::GenerateFromAsl => {
                if let Some(hub) = self.hub.as_mut() {
                    hub.refresh_derived();
                    self.sync_editors_from_hub();
                    self.status = "generated all projections from ASL IR".into();
                }
            }
            Message::VisualAction(action) => {
                self.visual_editor.perform(action);
                self.visual_dirty = true;
            }
            Message::ApplyVisual => self.apply_visual(),
            Message::LadderContactNo => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.ladder_add_contact_no("pressed") {
                        Ok(()) => self.refresh_after_authoring("ladder +NO contact → ASL"),
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::LadderContactNc => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.ladder_add_contact_nc("btnPin") {
                        Ok(()) => self.refresh_after_authoring("ladder +NC contact → ASL"),
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::LadderCoil => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.ladder_add_coil("ledPin") {
                        Ok(()) => self.refresh_after_authoring("ladder +coil → ASL"),
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::LadderRemoveLast => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.ladder_remove_last() {
                        Ok(()) => self.refresh_after_authoring("ladder removed last cell → ASL"),
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::SfcAddStep => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.sfc_add_step() {
                        Ok(()) => self.refresh_after_authoring("GRAFCET +step → ASL"),
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::SfcAddTransition => {
                if let Some(hub) = self.hub.as_mut() {
                    match hub.sfc_add_transition() {
                        Ok(()) => self.refresh_after_authoring("GRAFCET +transition → ASL"),
                        Err(e) => self.status = e,
                    }
                }
            }
        }
        iced::Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.running {
            iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let mut tabs = row![].spacing(4);
        for v in View::ALL {
            tabs = tabs.push(
                button(text(v.label()).size(12))
                    .padding([7, 10])
                    .style(if self.view == v {
                        ui::selected
                    } else {
                        ui::control
                    })
                    .on_press(Message::View(v)),
            );
        }
        let breadcrumb = format!("PROJECTS / UNO R3 / {}", self.project_label);
        let header = row![
            text("Δ").size(25).color(ui::ORANGE),
            text("DendriForge").size(21).color(ui::HEADING),
            text("v0.6.0").size(11).color(ui::MUTED),
            Space::with_width(12),
            text(breadcrumb).size(11).color(ui::SKY),
            Space::with_width(Length::Fill),
            tabs,
            action("Export", Message::Export),
        ]
        .spacing(9)
        .align_y(alignment::Vertical::Center);

        let tools = row![
            action(if self.running { "PAUSE" } else { "RUN" }, Message::Run),
            action("STEP CYCLE", Message::Step),
            action("RESET", Message::Reset),
            text(if self.running {
                "● PREVIEW RUNNING"
            } else {
                "● PREVIEW PAUSED"
            })
            .size(12)
            .color(if self.running {
                ui::EMERALD
            } else {
                ui::ORANGE
            }),
            text(format!("Cycle: {}", self.cycles))
                .size(12)
                .color(ui::CYAN),
            Space::with_width(8),
            action("Open C01", Message::OpenC01),
            action("New", Message::NewBlank),
            action("Save", Message::Save),
            Space::with_width(Length::Fill),
            action(
                if self.left {
                    "Hide left"
                } else {
                    "Show left"
                },
                Message::Left,
            ),
            action(
                if self.right {
                    "Hide right"
                } else {
                    "Show right"
                },
                Message::Right,
            ),
            action(
                if self.bottom {
                    "Hide dock"
                } else {
                    "Show dock"
                },
                Message::Bottom,
            ),
            action("−", Message::Zoom(-0.1)),
            text(format!("{:.0}%", self.zoom * 100.0)).size(12),
            action("+", Message::Zoom(0.1)),
        ]
        .spacing(8)
        .align_y(alignment::Vertical::Center);

        let mut workspace = row![].spacing(1).height(Length::Fill);
        if self.left {
            workspace = workspace.push(
                container(scrollable(self.palette()))
                    .padding(12)
                    .width(230)
                    .height(Length::Fill)
                    .style(ui::panel),
            );
        }
        let center = match self.view {
            View::Simulation => self.simulation(),
            View::Code => self.code(),
            View::Ladder | View::Grafcet => self.diagram(),
        };
        workspace = workspace.push(
            container(center)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| ui::surface(ui::SURFACE)),
        );
        if self.right {
            workspace = workspace.push(
                container(scrollable(self.inspector()))
                    .padding(12)
                    .width(310)
                    .height(Length::Fill)
                    .style(ui::panel),
            );
        }

        let mut content = column![
            container(header)
                .padding([9, 12])
                .width(Length::Fill)
                .style(ui::panel),
            container(tools)
                .padding([8, 12])
                .width(Length::Fill)
                .style(|_| ui::surface(ui::LOWEST)),
            workspace,
        ]
        .spacing(1);

        if self.bottom {
            content = content.push(
                container(
                    row![
                        column![
                            caption("LOCAL PREVIEW CONSOLE"),
                            text(format!(
                                "[{}] btnPin={}   pressed={}   ledPin={}",
                                self.cycles,
                                bit(self.raw),
                                bit(self.pressed),
                                bit(self.led)
                            ))
                            .font(Font::MONOSPACE)
                            .size(12),
                            text(if self.status.is_empty() {
                                "100 ms UI tick · dendri-sim · parse→ASL→emit live".into()
                            } else {
                                format!("100 ms UI tick · {}", self.status)
                            })
                            .size(11)
                            .color(ui::MUTED)
                        ]
                        .spacing(9)
                        .width(Length::Fill),
                        column![
                            caption("BUILD / MEMORY"),
                            text("Flash: not compiled   SRAM: not measured").size(12),
                            text("Honest stubs · Export writes build/ · no upload")
                                .size(11)
                                .color(ui::MUTED)
                        ]
                        .spacing(9)
                        .width(Length::Fill),
                    ]
                    .spacing(20),
                )
                .padding(12)
                .width(Length::Fill)
                .style(ui::panel),
            );
        }

        content = content.push(
            container(
                row![
                    text("AVR ATmega328P · 16 MHz target · LOCAL PREVIEW (not flashed)")
                        .size(11)
                        .color(ui::EMERALD),
                    Space::with_width(Length::Fill),
                    text("D2 INPUT_PULLUP · D13 OUTPUT · integrated 220 Ω")
                        .size(11)
                        .color(ui::MUTED)
                ],
            )
            .padding([5, 12])
            .width(Length::Fill)
            .style(ui::panel),
        );

        container(content)
            .height(Length::Fill)
            .style(|_| ui::surface(ui::MAIN))
            .into()
    }

    fn palette(&self) -> Element<'_, Message> {
        let title = match self.view {
            View::Simulation => "COMPONENT CATALOG",
            View::Code => "PROJECT EXPLORER",
            View::Ladder => "INSTRUCTIONS · LD",
            View::Grafcet => "PALETTE · GRAFCET",
        };
        let mut items = column![caption(title)].spacing(10);
        match self.view {
            View::Code => {
                items = items
                    .push(caption("FILE"))
                    .push(
                        text_input("path/to/project.toml", &self.path_input)
                            .size(12)
                            .on_input(Message::PathInput)
                            .on_submit(Message::OpenPath),
                    )
                    .push(
                        row![
                            action("Open", Message::OpenPath),
                            action("Open C01", Message::OpenC01),
                            action("New", Message::NewBlank),
                        ]
                        .spacing(6),
                    )
                    .push(
                        row![
                            action("Save", Message::Save),
                            action("Export", Message::Export),
                        ]
                        .spacing(6),
                    )
                    .push(
                        text(format!("loaded: {}", self.project_path.display()))
                            .size(11)
                            .color(ui::MUTED),
                    )
                    .push(Space::with_height(8))
                    .push(caption("SOURCES"));
                for source in Source::ALL {
                    items = items.push(action(source.label(), Message::Source(source)));
                }
                items = items
                    .push(Space::with_height(8))
                    .push(action("Apply → ASL", Message::ApplyCode))
                    .push(action("Generate from ASL", Message::GenerateFromAsl))
                    .push(
                        text(if self.code_dirty {
                            "editor dirty — Apply to parse"
                        } else {
                            "in sync with ASL IR"
                        })
                        .size(11)
                        .color(ui::MUTED),
                    );
            }
            View::Simulation => {
                items = items
                    .push(text("Click to place on project (updates wiring + ASL hardware):").size(11).color(ui::MUTED))
                    .push(action(Placeable::Led.label(), Message::Place(Placeable::Led)))
                    .push(action(
                        Placeable::PushbuttonNo.label(),
                        Message::Place(Placeable::PushbuttonNo),
                    ))
                    .push(Space::with_height(8))
                    .push(caption("WIRE"))
                    .push(
                        text_input("from (board.D13)", &self.wire_from)
                            .size(12)
                            .on_input(Message::WireFrom),
                    )
                    .push(
                        text_input("to (led1.A)", &self.wire_to)
                            .size(12)
                            .on_input(Message::WireTo),
                    )
                    .push(
                        text_input("signal", &self.wire_signal)
                            .size(12)
                            .on_input(Message::WireSignal),
                    )
                    .push(action("Add wire", Message::AddWire))
                    .push(action("Rebuild sim", Message::RebuildSim))
                    .push(Space::with_height(8))
                    .push(caption("PLACED"));
                if let Some(hub) = self.hub.as_ref() {
                    for c in &hub.project().components {
                        items = items.push(
                            row![
                                text(format!("{} · {}", c.instance_id, c.profile))
                                    .size(11)
                                    .width(Length::Fill),
                                action("×", Message::RemoveComponent(c.instance_id.clone())),
                            ]
                            .spacing(4),
                        );
                    }
                    items = items.push(caption("WIRES"));
                    for (i, w) in hub.project().wiring.iter().enumerate() {
                        items = items.push(
                            row![
                                text(format!("{} → {}", w.from, w.to))
                                    .size(11)
                                    .width(Length::Fill),
                                action("×", Message::RemoveWire(i)),
                            ]
                            .spacing(4),
                        );
                    }
                }
            }
            View::Ladder => {
                items = items
                    .push(action("| |  Add NO (pressed)", Message::LadderContactNo))
                    .push(action("|/|  Add NC (btnPin)", Message::LadderContactNc))
                    .push(action("( )  Add coil (ledPin)", Message::LadderCoil))
                    .push(action("Remove last cell", Message::LadderRemoveLast))
                    .push(Space::with_height(8))
                    .push(action("Apply ladder TOML → ASL", Message::ApplyVisual))
                    .push(action("Generate from ASL", Message::GenerateFromAsl))
                    .push(
                        text("Palette edits lower into ASL loop via dendri-frontend-ladder.")
                            .size(11)
                            .color(ui::MUTED),
                    );
            }
            View::Grafcet => {
                items = items
                    .push(action("Add step", Message::SfcAddStep))
                    .push(action("Add transition", Message::SfcAddTransition))
                    .push(Space::with_height(8))
                    .push(action("Apply SFC TOML → ASL", Message::ApplyVisual))
                    .push(action("Generate from ASL", Message::GenerateFromAsl))
                    .push(
                        text("Palette edits lower into ASL via dendri-frontend-sfc.")
                            .size(11)
                            .color(ui::MUTED),
                    );
            }
        }
        items = items
            .push(Space::with_height(16))
            .push(caption("ASL SYMBOLS"))
            .push(
                text("D2    btnPin · DI")
                    .font(Font::MONOSPACE)
                    .size(12)
                    .color(ui::CYAN),
            )
            .push(
                text("VAR   pressed · BOOL")
                    .font(Font::MONOSPACE)
                    .size(12)
                    .color(ui::SECONDARY),
            )
            .push(
                text("D13   ledPin · DO")
                    .font(Font::MONOSPACE)
                    .size(12)
                    .color(ui::ORANGE),
            );
        items.into()
    }

    fn simulation(&self) -> Element<'_, Message> {
        // Full-bleed world canvas in the Simulation center pane (D1: no world scrollbars).
        // Scroll = zoom, middle-button = pan (D7). Hold-button lives in the inspector.
        let _ = (ZOOM_MIN, ZOOM_MAX);
        canvas(WorldCanvas {
            camera: self.camera,
            scene: self.phase_a,
            led_on: self.led,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn code(&self) -> Element<'_, Message> {
        let mut tabs = row![].spacing(4);
        for source in Source::ALL {
            tabs = tabs.push(
                button(text(source.label()).size(11))
                    .padding(8)
                    .on_press(Message::Source(source))
                    .style(if source == self.source {
                        ui::selected
                    } else {
                        ui::control
                    }),
            );
        }
        column![
            container(
                row![
                    tabs,
                    Space::with_width(Length::Fill),
                    action("Apply → ASL", Message::ApplyCode),
                    action("Generate", Message::GenerateFromAsl),
                    action("Save", Message::Save),
                    action("Export", Message::Export),
                    action("Copy", Message::Copy),
                ]
            )
            .padding(8)
            .style(ui::panel),
            container(
                text_editor(&self.code_editor)
                    .height(Length::Fill)
                    .on_action(Message::CodeAction),
            )
            .padding(8)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(ui::editor),
            container(
                text(
                    "Editable · Apply runs dendri-frontends parse → ASL IR → emit all langs · ST is DendriForge dialect · MicroPython not an Uno flash target",
                )
                .size(11)
                .color(ui::MUTED),
            )
            .padding(10),
        ]
        .height(Length::Fill)
        .into()
    }

    fn diagram(&self) -> Element<'_, Message> {
        let is_ladder = self.view == View::Ladder;
        let title = if is_ladder {
            "LADDER · live diagram + editable TOML (parse → ASL)"
        } else {
            "GRAFCET · live diagram + editable TOML (parse → ASL)"
        };
        column![
            container(caption(title)).padding(10),
            canvas(Diagram {
                view: self.view,
                raw: self.raw,
                pressed: self.pressed,
                led: self.led,
                zoom: self.zoom,
            })
            .width(Length::Fill)
            .height(Length::FillPortion(3)),
            container(
                row![
                    if is_ladder {
                        checkbox("btnPin / D2 = HIGH (released)", self.raw).on_toggle(Message::Raw)
                    } else {
                        checkbox("Physical button pressed", !self.raw)
                            .on_toggle(Message::Physical)
                    },
                    Space::with_width(Length::Fill),
                    action("Apply TOML → ASL", Message::ApplyVisual),
                    action("Generate from ASL", Message::GenerateFromAsl),
                ]
                .spacing(8),
            )
            .padding(8),
            container(
                text_editor(&self.visual_editor)
                    .height(Length::Fill)
                    .on_action(Message::VisualAction),
            )
            .padding(8)
            .height(Length::FillPortion(2))
            .style(ui::editor),
        ]
        .height(Length::Fill)
        .into()
    }

    fn inspector(&self) -> Element<'_, Message> {
        let watch = column![
            caption("LIVE TAG WATCH"),
            tag("btnPin", "D2 / DI", self.raw, ui::CYAN),
            tag("pressed", "BOOL", self.pressed, ui::SECONDARY),
            tag("ledPin", "D13 / DO", self.led, ui::ORANGE),
        ]
        .spacing(12);

        let mut body = column![
            text(format!("INSPECTOR · {}", self.project_label))
                .size(12)
                .color(ui::SKY),
            container(watch).padding(12).style(ui::card),
            checkbox("Physical button pressed", !self.raw).on_toggle(Message::Physical),
            text("Sim canvas: scroll=zoom · middle-button=pan · line grid 1u=4mm")
                .size(11)
                .color(ui::MUTED),
        ]
        .spacing(16);

        if let Some(hub) = self.hub.as_ref() {
            let hw_lines: String = hub
                .project()
                .asl
                .hardware
                .iter()
                .map(|h| {
                    format!(
                        "D{} {} · {} ({:?})",
                        h.pin, h.label, h.target_role, h.pull
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            body = body.push(
                container(
                    column![
                        caption("ASL HARDWARE"),
                        text(if hw_lines.is_empty() {
                            "(none — place components)".into()
                        } else {
                            hw_lines
                        })
                        .font(Font::MONOSPACE)
                        .size(11),
                    ]
                    .spacing(8),
                )
                .padding(12)
                .style(ui::card),
            );
        }

        body = body
            .push(
                container(
                    column![
                        caption("PIPELINE"),
                        text("place/wire → project TOML\nedit code/LD/SFC → parse\n→ ASL IR → emit langs\nSave / Export / sim preview")
                            .font(Font::MONOSPACE)
                            .size(11),
                    ]
                    .spacing(8),
                )
                .padding(12)
                .style(ui::card),
            )
            .push(
                text("Prototype chrome + SyncSession authoring. No AVR compile/upload, serial, or electrical solver. Schematic SVG is C01 reference chrome.")
                    .size(11)
                    .color(ui::MUTED),
            );

        body.into()
    }
}

fn bit(value: bool) -> &'static str {
    if value {
        "TRUE"
    } else {
        "FALSE"
    }
}
fn caption<'a>(label: &'a str) -> iced::widget::Text<'a> {
    text(label).size(12).color(ui::SKY)
}
fn action<'a>(label: &'a str, message: Message) -> iced::widget::Button<'a, Message> {
    button(text(label).size(11))
        .padding([6, 9])
        .style(ui::control)
        .on_press(message)
}
fn tag<'a>(name: &'a str, address: &'a str, value: bool, color: Color) -> Element<'a, Message> {
    row![
        text(name)
            .font(Font::MONOSPACE)
            .size(12)
            .color(color)
            .width(85),
        text(address).size(11).color(ui::MUTED).width(75),
        text(bit(value))
            .size(12)
            .color(if value { ui::EMERALD } else { ui::MUTED })
    ]
    .into()
}

struct Diagram {
    view: View,
    raw: bool,
    pressed: bool,
    led: bool,
    zoom: f32,
}
impl canvas::Program<Message> for Diagram {
    type State = ();
    fn update(
        &self,
        _: &mut (),
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if self.view == View::Ladder {
            if let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
                if let Some(p) = cursor.position_in(bounds) {
                    let scale = (bounds.width / 800.0).min(bounds.height / 470.0) * self.zoom;
                    if Rectangle::new(
                        Point::new(80.0 * scale, 60.0 * scale),
                        Size::new(120.0 * scale, 100.0 * scale),
                    )
                    .contains(p)
                    {
                        return (canvas::event::Status::Captured, Some(Message::ToggleRaw));
                    }
                }
            }
        }
        (canvas::event::Status::Ignored, None)
    }
    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut f = canvas::Frame::new(renderer, bounds.size());
        f.fill_rectangle(Point::ORIGIN, bounds.size(), ui::SURFACE);
        for x in (0..bounds.width as usize).step_by(20) {
            for y in (0..bounds.height as usize).step_by(20) {
                f.fill(
                    &canvas::Path::circle(Point::new(x as f32, y as f32), 0.7),
                    ui::HOVER,
                );
            }
        }
        f.scale((bounds.width / 800.0).min(bounds.height / 470.0) * self.zoom);
        if self.view == View::Ladder {
            line(&mut f, 24.0, 52.0, 24.0, 400.0, ui::EMERALD, 5.0);
            line(&mut f, 775.0, 52.0, 775.0, 400.0, ui::BORDER, 3.0);
            for (index, y, input, output, value) in [
                (1, 120.0, "btnPin · D2", "pressed", self.pressed),
                (2, 310.0, "pressed", "ledPin · D13", self.led),
            ] {
                let c = if value { ui::EMERALD } else { ui::BORDER };
                label(&mut f, 42.0, y - 63.0, &format!("RUNG {index:03}"), ui::SKY, 14.0);
                label(
                    &mut f,
                    103.0,
                    y - 40.0,
                    input,
                    if index == 1 {
                        ui::CYAN
                    } else {
                        ui::SECONDARY
                    },
                    14.0,
                );
                line(&mut f, 24.0, y, 120.0, y, ui::EMERALD, 3.0);
                line(&mut f, 120.0, y - 18.0, 120.0, y + 18.0, c, 3.0);
                line(&mut f, 149.0, y - 18.0, 149.0, y + 18.0, c, 3.0);
                if index == 1 {
                    line(&mut f, 113.0, y + 24.0, 156.0, y - 24.0, ui::ROSE, 3.0);
                }
                line(&mut f, 149.0, y, 676.0, y, c, 3.0);
                let coil = canvas::Path::new(|b| {
                    b.move_to(Point::new(693.0, y - 21.0));
                    b.bezier_curve_to(
                        Point::new(670.0, y - 13.0),
                        Point::new(670.0, y + 13.0),
                        Point::new(693.0, y + 21.0),
                    );
                    b.move_to(Point::new(712.0, y - 21.0));
                    b.bezier_curve_to(
                        Point::new(735.0, y - 13.0),
                        Point::new(735.0, y + 13.0),
                        Point::new(712.0, y + 21.0),
                    );
                });
                f.stroke(
                    &coil,
                    canvas::Stroke::default().with_color(c).with_width(3.0),
                );
                line(&mut f, 728.0, y, 775.0, y, c, 3.0);
                label(
                    &mut f,
                    632.0,
                    y - 40.0,
                    output,
                    if index == 1 {
                        ui::SECONDARY
                    } else {
                        ui::ORANGE
                    },
                    14.0,
                );
                label(
                    &mut f,
                    45.0,
                    y + 48.0,
                    if index == 1 {
                        "pressed := NOT GET_BOOL(btnPin)"
                    } else {
                        "SET_BOOL(ledPin, pressed)"
                    },
                    ui::MUTED,
                    13.0,
                );
            }
            label(
                &mut f,
                40.0,
                430.0,
                &format!("Raw D2 = {} · pressed = {}", bit(self.raw), bit(self.pressed)),
                ui::MUTED,
                13.0,
            );
        } else {
            label(&mut f, 50.0, 60.0, "1 INITIAL STEP · 0 TRANSITIONS", ui::SKY, 15.0);
            for (x, y, w, h) in [
                (50.0, 140.0, 98.0, 98.0),
                (57.0, 147.0, 84.0, 84.0),
            ] {
                f.stroke(
                    &canvas::Path::rectangle(Point::new(x, y), Size::new(w, h)),
                    canvas::Stroke::default()
                        .with_color(ui::EMERALD)
                        .with_width(3.0),
                );
            }
            label(&mut f, 78.0, 172.0, "S0", ui::EMERALD, 28.0);
            label(&mut f, 65.0, 260.0, "ACTIVE", ui::EMERALD, 14.0);
            line(&mut f, 148.0, 189.0, 205.0, 189.0, ui::EMERALD, 3.0);
            f.fill_rectangle(Point::new(205.0, 126.0), Size::new(550.0, 160.0), ui::CARD);
            label(
                &mut f,
                225.0,
                144.0,
                "A_LOOP · ORDERED CYCLIC ACTION",
                ui::SKY,
                16.0,
            );
            label(
                &mut f,
                225.0,
                194.0,
                "1. pressed := NOT GET_BOOL(btnPin);",
                ui::SECONDARY,
                16.0,
            );
            label(
                &mut f,
                225.0,
                239.0,
                "2. SET_BOOL(ledPin, pressed);",
                ui::ORANGE,
                16.0,
            );
            label(
                &mut f,
                50.0,
                340.0,
                "S0 remains active. Edit SFC TOML below for authoring.",
                ui::MUTED,
                15.0,
            );
        }
        vec![f.into_geometry()]
    }
}

fn line(f: &mut canvas::Frame, x1: f32, y1: f32, x2: f32, y2: f32, c: Color, width: f32) {
    f.stroke(
        &canvas::Path::line(Point::new(x1, y1), Point::new(x2, y2)),
        canvas::Stroke::default().with_color(c).with_width(width),
    );
}
fn label(f: &mut canvas::Frame, x: f32, y: f32, s: &str, color: Color, size: f32) {
    f.fill_text(canvas::Text {
        content: s.to_owned(),
        position: Point::new(x, y),
        color,
        size: size.into(),
        font: Font::MONOSPACE,
        ..Default::default()
    });
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn c01_path() -> PathBuf {
    repo_root().join("content/fixtures/canvas/C01-button-led.toml")
}

fn startup_project_path() -> PathBuf {
    std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(c01_path)
}

fn main() -> iced::Result {
    iced::application("DendriForge — canvas MVP", App::update, App::view)
        .theme(|_| ui::slate())
        .subscription(App::subscription)
        .window_size((1520.0, 960.0))
        .run_with(App::boot)
}
