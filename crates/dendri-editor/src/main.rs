//! DendriForge iced editor — prototype chrome + authoring loop.
//!
//! Visual SoT: Caio iced 0.13 prototype (`docs/canvas-integration/`).
//! Utility: SyncSession hub — place/wire, edit code, Ladder/GRAFCET palette +
//! TOML, parse → ASL IR → generators (same pipeline as `dendri export`).

mod authoring;
mod canvas_path;
mod project_io;
#[allow(dead_code)] // retained C01 reference chrome; Simulation uses world canvas
mod schematic;
mod sim_session;
mod sim_world;
mod theme;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use authoring::{AuthoringHub, Placeable};
use dendri_ast::SourceLanguage;
use dendri_canvas::{Camera, Scene, SimCommand, SimRunState};
use iced::{
    alignment, event, keyboard, mouse, window,
    widget::{
        button, canvas, checkbox, column, container, row, scrollable, stack, text, text_editor,
        text_input, Space,
    },
    Color, Element, Event, Font, Length, Point, Rectangle, Renderer, Size, Subscription, Theme,
};
use project_io::{export_project, load_project, save_project};
use sim_session::SimSession;
use sim_world::{CanvasSelection, SceneSvgHandles, WorldCanvas};
use theme as ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextMenuKind {
    Object,
    Wire,
    Empty,
}

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
    Stop,
    /// Runtime clock while Running — advances exactly one scan (not a visual frame).
    SimScan,
    /// Visual clock while Running (`window::frames`) — presentation only (D-C6).
    SimFrame(Instant),
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
    /// Canvas / code clipboard (context-dependent).
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
    // --- Phase B1/B2 scene ---
    SelectPart {
        id: Option<String>,
    },
    /// Canvas LMB down on button while Running (momentary).
    CanvasButtonPress {
        id: String,
    },
    /// Canvas LMB up while Running — release momentary.
    CanvasButtonRelease,
    /// Shift+LMB toggle latch while Running (D-C4).
    CanvasButtonLatchToggle {
        id: String,
    },
    /// Focus / cursor loss — cancel momentary only.
    CanvasButtonCancelMomentary,
    /// Track Shift for Running latch vs outside multi-select stub.
    ShiftHeld(bool),
    SelectWire {
        index: Option<usize>,
    },
    CreateWire {
        from: String,
        to: String,
    },
    InsertWireWaypoint {
        index: usize,
        at: [f64; 2],
        after_segment: usize,
    },
    DragPartTo {
        id: String,
        x: f64,
        y: f64,
        begin_undo: bool,
    },
    DragPartEnd {
        id: String,
    },
    RotateSelection,
    MirrorSelectionX,
    MirrorSelectionY,
    DeleteSelection,
    CanvasCopy,
    CanvasCut,
    CanvasPaste,
    CanvasDuplicate,
    CanvasUndo,
    CanvasRedo,
    OpenProperties {
        id: String,
    },
    ShowObjectContextMenu {
        id: String,
        screen_x: f32,
        screen_y: f32,
    },
    ShowWireContextMenu {
        index: usize,
        screen_x: f32,
        screen_y: f32,
    },
    ShowEmptyContextMenu {
        screen_x: f32,
        screen_y: f32,
    },
    DismissContextMenu,
    /// Empty-canvas RMB menu — same handlers as toolbar (C0 single control path).
    ContextPlay,
    ContextPause,
    ContextStop,
    ContextReset,
    ContextStep,
    /// Global keyboard (Simulation shortcuts).
    KeyChord(KeyChord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyChord {
    Rotate,
    Delete,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
}

struct App {
    view: View,
    source: Source,
    /// C0 runtime bridge (Idle | Running | Paused) + snapshot for WorldCanvas.
    sim: SimSession,
    left: bool,
    right: bool,
    bottom: bool,
    /// Shared zoom readout (Ladder/GRAFCET scale + Simulation camera.zoom).
    zoom: f32,
    /// Simulation world camera (pan + zoom). Infinite line grid; no page bounds.
    camera: Camera,
    /// Scene graph built from the open project (B1).
    scene: Scene,
    /// Selected part or wire (B1/B2).
    selection: CanvasSelection,
    /// Bumped on layout / wiring commits so the static canvas cache redraws.
    static_gen: u64,
    /// MVP SVG handles + prepared hook geometry (Uno / LED / button).
    scene_svgs: SceneSvgHandles,
    /// Shift modifier — Running latch vs outside multi-select stub (D-C4).
    shift_held: bool,
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
    /// Floating context menu (Simulation).
    context_menu: Option<(ContextMenuKind, String, f32, f32)>,
}

impl App {
    fn boot() -> (Self, iced::Task<Message>) {
        let project_path = startup_project_path();
        let mut app = Self {
            view: View::Simulation,
            source: Source::Cpp,
            sim: SimSession::new(),
            left: true,
            right: true,
            bottom: true,
            zoom: 1.0,
            camera: Camera::default(),
            scene: Scene::default(),
            selection: CanvasSelection::None,
            static_gen: 0,
            scene_svgs: SceneSvgHandles::load(repo_root()),
            shift_held: false,
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
            context_menu: None,
        };
        app.load_into_hub(project_path);
        (app, iced::Task::none())
    }

    fn rebuild_scene_from_hub(&mut self) {
        if let Some(hub) = self.hub.as_ref() {
            self.scene = Scene::from_project(hub.project()).with_default_layout_if_empty();
        } else {
            self.scene = Scene::default();
        }
        self.static_gen = self.static_gen.wrapping_add(1);
        match &self.selection {
            CanvasSelection::Part(id) if self.scene.part(id).is_none() => {
                self.selection = CanvasSelection::None;
            }
            CanvasSelection::Wire(i) => {
                let n = self
                    .hub
                    .as_ref()
                    .map(|h| h.project().wiring.len())
                    .unwrap_or(0);
                if *i >= n {
                    self.selection = CanvasSelection::None;
                }
            }
            _ => {}
        }
    }

    fn load_into_hub(&mut self, path: PathBuf) {
        match load_project(&path) {
            Ok(project) => {
                let hub = AuthoringHub::from_project(project);
                self.project_label = project_io::project_display_name(hub.project(), &path);
                self.project_path = path.clone();
                self.path_input = path.display().to_string();
                self.hub = Some(hub);
                self.selection = CanvasSelection::None;
                self.context_menu = None;
                self.sync_editors_from_hub();
                self.rebuild_scene_from_hub();
                self.boot_canvas_from_hub();
            }
            Err(e) => {
                self.hub = None;
                self.sim.clear_runtime();
                self.scene = Scene::default();
                self.status = format!("project load failed: {e}");
            }
        }
    }

    fn project_ref(&self) -> Option<&dendri_ast::SimulationProject> {
        self.hub.as_ref().map(|h| h.project())
    }

    /// D-C1: block layout/component/wire mutations while Running.
    fn reject_if_running(&mut self, action: &str) -> bool {
        if self.sim.edits_allowed() {
            return false;
        }
        self.status = format!("{action} blocked while Running — Pause to edit");
        true
    }

    fn ui_raw(&self) -> bool {
        self.sim.primary_raw_high()
    }

    fn ui_pressed(&self) -> bool {
        // Prefer ASL `pressed` var when present; else actuation / snapshot.
        if let Some(canvas) = self.sim.canvas.as_ref() {
            if let Some(dendri_sim::Value::Bool(v)) = canvas.sim.var("pressed") {
                return *v;
            }
        }
        self.sim.primary_pressed_logic(self.project_ref())
    }

    fn ui_led(&self) -> bool {
        self.sim.primary_led_on()
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
            self.sim.clear_runtime();
            return;
        };
        let project = hub.project().clone();
        let root = repo_root();
        match self.sim.boot_from_project(&project, &root) {
            Ok(()) => {
                // Preview starts Running after a successful boot (prior UX).
                self.sim.run_state = SimRunState::Running;
                self.sim.refresh_snapshot(Some(&project));
                self.status = format!(
                    "loaded {} · dendri-sim · {}",
                    self.project_path.display(),
                    self.sim.status_line()
                );
            }
            Err(e) => {
                self.status = format!("{e} (project still editable; no fabricated sim)");
            }
        }
    }

    fn refresh_after_authoring(&mut self, note: impl Into<String>) {
        self.sync_editors_from_hub();
        self.rebuild_scene_from_hub();
        self.boot_canvas_from_hub();
        self.status = note.into();
    }

    fn refresh_scene_only(&mut self, note: impl Into<String>) {
        self.rebuild_scene_from_hub();
        self.status = note.into();
    }

    fn apply_sim(&mut self, cmd: SimCommand) {
        let root = repo_root();
        let project = self.hub.as_ref().map(|h| h.project().clone());
        self.status = self
            .sim
            .apply_command(cmd, project.as_ref(), &root);
    }

    fn apply_physical(&mut self, down: bool) {
        let project = self.hub.as_ref().map(|h| h.project().clone());
        self.sim.set_primary_button(down, project.as_ref());
        if self.sim.is_running() {
            let _ = self.sim.scan_once(project.as_ref());
        }
        self.status = self.sim.status_line();
    }

    fn apply_raw(&mut self, raw: bool) {
        let project = self.hub.as_ref().map(|h| h.project().clone());
        self.sim
            .set_primary_raw_level(raw, project.as_ref());
        if self.sim.is_running() {
            let _ = self.sim.scan_once(project.as_ref());
        }
        self.status = self.sim.status_line();
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
            Message::Run => {
                let root = repo_root();
                let project = self.hub.as_ref().map(|h| h.project().clone());
                self.status = self.sim.toggle_run(project.as_ref(), &root);
            }
            Message::Step => self.apply_sim(SimCommand::Step),
            Message::Reset => self.apply_sim(SimCommand::Reset),
            Message::Stop => self.apply_sim(SimCommand::Stop),
            Message::SimScan => {
                if self.sim.is_running() {
                    let project = self.hub.as_ref().map(|h| h.project().clone());
                    match self.sim.scan_once(project.as_ref()) {
                        Ok(()) => self.status = self.sim.status_line(),
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::SimFrame(at) => {
                // Visual clock only — never auto-scan per presented frame (D-C6).
                // Instant delta advances wire activity phase (not frame counts).
                if self.sim.is_running() {
                    let project = self.hub.as_ref().map(|h| h.project().clone());
                    self.sim.present_frame(project.as_ref(), at);
                }
            }
            Message::Raw(raw) => self.apply_raw(raw),
            Message::Physical(down) => self.apply_physical(down),
            Message::ToggleRaw => self.apply_raw(!self.sim.primary_raw_high()),
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
                self.selection = CanvasSelection::None;
                self.context_menu = None;
                self.sync_editors_from_hub();
                self.rebuild_scene_from_hub();
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
                if self.reject_if_running("place") {
                    return iced::Task::none();
                }
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
                if self.reject_if_running("add wire") {
                    return iced::Task::none();
                }
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
                if self.reject_if_running("remove component") {
                    return iced::Task::none();
                }
                if let Some(hub) = self.hub.as_mut() {
                    match hub.remove_component(&id) {
                        Ok(()) => self.refresh_after_authoring(format!("removed {id}")),
                        Err(e) => self.status = format!("remove failed: {e}"),
                    }
                }
            }
            Message::RemoveWire(i) => {
                if self.reject_if_running("remove wire") {
                    return iced::Task::none();
                }
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
            Message::SelectPart { id } => {
                self.selection = match id {
                    Some(id) => CanvasSelection::Part(id),
                    None => CanvasSelection::None,
                };
                self.context_menu = None;
            }
            Message::CanvasButtonPress { id } => {
                if let Some(hub) = self.hub.as_ref() {
                    let project = hub.project().clone();
                    self.sim.canvas_button_press(&id, Some(&project));
                    self.status = self.sim.status_line();
                }
            }
            Message::CanvasButtonRelease => {
                if let Some(hub) = self.hub.as_ref() {
                    let project = hub.project().clone();
                    self.sim.canvas_button_release(Some(&project));
                    self.status = self.sim.status_line();
                }
            }
            Message::CanvasButtonLatchToggle { id } => {
                if let Some(hub) = self.hub.as_ref() {
                    let project = hub.project().clone();
                    self.sim.canvas_button_latch_toggle(&id, Some(&project));
                    self.status = self.sim.status_line();
                }
            }
            Message::CanvasButtonCancelMomentary => {
                if let Some(hub) = self.hub.as_ref() {
                    let project = hub.project().clone();
                    self.sim.cancel_momentary(Some(&project));
                    self.status = self.sim.status_line();
                }
            }
            Message::ShiftHeld(held) => {
                self.shift_held = held;
            }
            Message::SelectWire { index } => {
                self.selection = match index {
                    Some(i) => CanvasSelection::Wire(i),
                    None => CanvasSelection::None,
                };
                self.context_menu = None;
                if let Some(i) = index {
                    self.status = format!("selected wire #{i}");
                }
            }
            Message::CreateWire { from, to } => {
                if self.reject_if_running("create wire") {
                    return iced::Task::none();
                }
                if let Some(hub) = self.hub.as_mut() {
                    let signal = if from.to_ascii_lowercase().contains("gnd")
                        || to.to_ascii_lowercase().contains("gnd")
                    {
                        Some("gnd")
                    } else if from.to_ascii_lowercase().contains("5v")
                        || to.to_ascii_lowercase().contains("5v")
                    {
                        Some("vcc")
                    } else {
                        Some("digital")
                    };
                    match hub.add_canvas_wire(&from, &to, signal, Vec::new()) {
                        Ok(idx) => {
                            self.selection = CanvasSelection::Wire(idx);
                            self.refresh_after_authoring(format!("wired {from} → {to}"));
                        }
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::InsertWireWaypoint {
                index,
                at,
                after_segment,
            } => {
                if self.reject_if_running("insert waypoint") {
                    return iced::Task::none();
                }
                if let Some(hub) = self.hub.as_mut() {
                    match hub.insert_wire_waypoint(index, at, after_segment) {
                        Ok(()) => {
                            self.selection = CanvasSelection::Wire(index);
                            self.refresh_after_authoring(format!(
                                "waypoint ({:.1},{:.1}) on wire #{index}",
                                at[0], at[1]
                            ));
                        }
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::DragPartTo {
                id,
                x,
                y,
                begin_undo,
            } => {
                if self.reject_if_running("drag part") {
                    return iced::Task::none();
                }
                if let Some(hub) = self.hub.as_mut() {
                    if begin_undo {
                        hub.begin_layout_edit();
                    }
                    match hub.part_layout(&id) {
                        Ok(mut layout) => {
                            layout.x = x;
                            layout.y = y;
                            let _ = hub.commit_part_layout_no_undo(&id, layout);
                            self.rebuild_scene_from_hub();
                        }
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::DragPartEnd { id } => {
                if self.reject_if_running("drag part") {
                    return iced::Task::none();
                }
                // C2: refresh code projections once at gesture end (not every drag move).
                if let Some(hub) = self.hub.as_mut() {
                    hub.refresh_derived();
                    self.sync_editors_from_hub();
                }
                self.status = format!("moved {id} (snapped)");
            }
            Message::RotateSelection => {
                if self.reject_if_running("rotate") {
                    return iced::Task::none();
                }
                if let Some(id) = self.selection.part_id().map(str::to_string) {
                    if let Some(hub) = self.hub.as_mut() {
                        match hub.rotate_part_cw(&id) {
                            Ok(()) => self.refresh_scene_only(format!("rotated {id} +90°")),
                            Err(e) => self.status = e,
                        }
                    }
                }
            }
            Message::MirrorSelectionX => {
                if self.reject_if_running("mirror") {
                    self.context_menu = None;
                    return iced::Task::none();
                }
                if let Some(id) = self.selection.part_id().map(str::to_string) {
                    if let Some(hub) = self.hub.as_mut() {
                        match hub.mirror_part_x(&id) {
                            Ok(()) => self.refresh_scene_only(format!("mirror X {id}")),
                            Err(e) => self.status = e,
                        }
                    }
                }
                self.context_menu = None;
            }
            Message::MirrorSelectionY => {
                if self.reject_if_running("mirror") {
                    self.context_menu = None;
                    return iced::Task::none();
                }
                if let Some(id) = self.selection.part_id().map(str::to_string) {
                    if let Some(hub) = self.hub.as_mut() {
                        match hub.mirror_part_y(&id) {
                            Ok(()) => self.refresh_scene_only(format!("mirror Y {id}")),
                            Err(e) => self.status = e,
                        }
                    }
                }
                self.context_menu = None;
            }
            Message::DeleteSelection => {
                if self.reject_if_running("delete") {
                    self.context_menu = None;
                    return iced::Task::none();
                }
                match self.selection.clone() {
                    CanvasSelection::Part(id) => {
                        if let Some(hub) = self.hub.as_mut() {
                            if hub.project().board.instance_id == id {
                                self.status = "cannot delete board".into();
                            } else {
                                match hub.remove_component(&id) {
                                    Ok(()) => {
                                        self.selection = CanvasSelection::None;
                                        self.refresh_after_authoring(format!("deleted {id}"));
                                    }
                                    Err(e) => self.status = e,
                                }
                            }
                        }
                    }
                    CanvasSelection::Wire(i) => {
                        if let Some(hub) = self.hub.as_mut() {
                            match hub.remove_wire_at(i) {
                                Ok(()) => {
                                    self.selection = CanvasSelection::None;
                                    self.refresh_after_authoring(format!("deleted wire #{i}"));
                                }
                                Err(e) => self.status = e,
                            }
                        }
                    }
                    CanvasSelection::None => {}
                }
                self.context_menu = None;
            }
            Message::CanvasCopy => {
                // Copy is non-destructive — allowed while Running.
                if let Some(id) = self.selection.part_id().map(str::to_string) {
                    if let Some(hub) = self.hub.as_mut() {
                        hub.copy_parts(&[id.clone()]);
                        self.status = format!("copied {id}");
                    }
                }
                self.context_menu = None;
            }
            Message::CanvasCut => {
                if self.reject_if_running("cut") {
                    self.context_menu = None;
                    return iced::Task::none();
                }
                if let Some(id) = self.selection.part_id().map(str::to_string) {
                    if let Some(hub) = self.hub.as_mut() {
                        match hub.cut_parts(&[id.clone()]) {
                            Ok(()) => {
                                self.selection = CanvasSelection::None;
                                self.refresh_after_authoring(format!("cut {id}"));
                            }
                            Err(e) => self.status = e,
                        }
                    }
                }
                self.context_menu = None;
            }
            Message::CanvasPaste => {
                if self.reject_if_running("paste") {
                    self.context_menu = None;
                    return iced::Task::none();
                }
                if let Some(hub) = self.hub.as_mut() {
                    match hub.paste_clipboard() {
                        Ok(ids) => {
                            self.selection = ids
                                .last()
                                .cloned()
                                .map(CanvasSelection::Part)
                                .unwrap_or(CanvasSelection::None);
                            self.refresh_after_authoring(format!("pasted {}", ids.join(", ")));
                        }
                        Err(e) => self.status = e,
                    }
                }
                self.context_menu = None;
            }
            Message::CanvasDuplicate => {
                if self.reject_if_running("duplicate") {
                    self.context_menu = None;
                    return iced::Task::none();
                }
                if let Some(id) = self.selection.part_id().map(str::to_string) {
                    if let Some(hub) = self.hub.as_mut() {
                        match hub.duplicate_parts(&[id]) {
                            Ok(ids) => {
                                self.selection = ids
                                    .last()
                                    .cloned()
                                    .map(CanvasSelection::Part)
                                    .unwrap_or(CanvasSelection::None);
                                self.refresh_after_authoring(format!(
                                    "duplicated → {}",
                                    ids.join(", ")
                                ));
                            }
                            Err(e) => self.status = e,
                        }
                    }
                }
                self.context_menu = None;
            }
            Message::CanvasUndo => {
                if self.reject_if_running("undo") {
                    return iced::Task::none();
                }
                if let Some(hub) = self.hub.as_mut() {
                    match hub.undo() {
                        Ok(()) => self.refresh_after_authoring("undo"),
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::CanvasRedo => {
                if self.reject_if_running("redo") {
                    return iced::Task::none();
                }
                if let Some(hub) = self.hub.as_mut() {
                    match hub.redo() {
                        Ok(()) => self.refresh_after_authoring("redo"),
                        Err(e) => self.status = e,
                    }
                }
            }
            Message::OpenProperties { id } => {
                // Sidebar out of scope — emit hook only.
                self.status = format!("open properties: {id} (hook; sidebar = later phase)");
                self.selection = CanvasSelection::Part(id);
            }
            Message::ShowObjectContextMenu {
                id,
                screen_x,
                screen_y,
            } => {
                self.selection = CanvasSelection::Part(id.clone());
                self.context_menu = Some((ContextMenuKind::Object, id, screen_x, screen_y));
            }
            Message::ShowWireContextMenu {
                index,
                screen_x,
                screen_y,
            } => {
                self.selection = CanvasSelection::Wire(index);
                self.context_menu = Some((
                    ContextMenuKind::Wire,
                    format!("wire#{index}"),
                    screen_x,
                    screen_y,
                ));
            }
            Message::ShowEmptyContextMenu {
                screen_x,
                screen_y,
            } => {
                self.context_menu = Some((
                    ContextMenuKind::Empty,
                    String::new(),
                    screen_x,
                    screen_y,
                ));
            }
            Message::DismissContextMenu => self.context_menu = None,
            Message::ContextPlay => {
                self.context_menu = None;
                self.apply_sim(SimCommand::Play);
            }
            Message::ContextPause => {
                self.context_menu = None;
                self.apply_sim(SimCommand::Pause);
            }
            Message::ContextStop => {
                self.context_menu = None;
                self.apply_sim(SimCommand::Stop);
            }
            Message::ContextReset => {
                self.context_menu = None;
                self.apply_sim(SimCommand::Reset);
            }
            Message::ContextStep => {
                self.context_menu = None;
                self.apply_sim(SimCommand::Step);
            }
            Message::KeyChord(chord) => {
                if self.view != View::Simulation {
                    return iced::Task::none();
                }
                // Don't steal keys while editing code in other views; Simulation has no text focus.
                match chord {
                    KeyChord::Rotate => {
                        return self.update(Message::RotateSelection);
                    }
                    KeyChord::Delete => {
                        return self.update(Message::DeleteSelection);
                    }
                    KeyChord::Copy => {
                        return self.update(Message::CanvasCopy);
                    }
                    KeyChord::Cut => {
                        return self.update(Message::CanvasCut);
                    }
                    KeyChord::Paste => {
                        return self.update(Message::CanvasPaste);
                    }
                    KeyChord::Undo => {
                        return self.update(Message::CanvasUndo);
                    }
                    KeyChord::Redo => {
                        return self.update(Message::CanvasRedo);
                    }
                }
            }
        }
        iced::Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        // Runtime clock ≠ visual clock (D-C6). Scans on a timer; frames only present.
        let scan = if self.sim.is_running() {
            iced::time::every(Duration::from_millis(100)).map(|_| Message::SimScan)
        } else {
            Subscription::none()
        };
        let frames = if self.sim.is_running() {
            window::frames().map(Message::SimFrame)
        } else {
            Subscription::none()
        };
        let keys = event::listen_with(|event, status, _id| {
            use keyboard::key::Named;
            use keyboard::Key;
            match event {
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    // Always track Shift for canvas latch / multi-select stub.
                    if matches!(key, Key::Named(Named::Shift)) {
                        return Some(Message::ShiftHeld(true));
                    }
                    if status == event::Status::Captured {
                        return None;
                    }
                    match key {
                        Key::Character(c) if c.eq_ignore_ascii_case("r") && !modifiers.control() => {
                            Some(Message::KeyChord(KeyChord::Rotate))
                        }
                        Key::Named(Named::Delete) | Key::Named(Named::Backspace) => {
                            Some(Message::KeyChord(KeyChord::Delete))
                        }
                        Key::Character(c) if modifiers.control() && c.eq_ignore_ascii_case("c") => {
                            Some(Message::KeyChord(KeyChord::Copy))
                        }
                        Key::Character(c) if modifiers.control() && c.eq_ignore_ascii_case("x") => {
                            Some(Message::KeyChord(KeyChord::Cut))
                        }
                        Key::Character(c) if modifiers.control() && c.eq_ignore_ascii_case("v") => {
                            Some(Message::KeyChord(KeyChord::Paste))
                        }
                        Key::Character(c) if modifiers.control() && c.eq_ignore_ascii_case("z") => {
                            Some(Message::KeyChord(KeyChord::Undo))
                        }
                        Key::Character(c) if modifiers.control() && c.eq_ignore_ascii_case("y") => {
                            Some(Message::KeyChord(KeyChord::Redo))
                        }
                        _ => None,
                    }
                }
                Event::Keyboard(keyboard::Event::KeyReleased { key, .. }) => {
                    if matches!(key, Key::Named(Named::Shift)) {
                        Some(Message::ShiftHeld(false))
                    } else {
                        None
                    }
                }
                Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                    // Keep Shift in sync even when chorded with other keys.
                    Some(Message::ShiftHeld(modifiers.shift()))
                }
                _ => None,
            }
        });
        Subscription::batch([scan, frames, keys])
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

        let run_label = match self.sim.run_state {
            SimRunState::Running => "● RUNNING",
            SimRunState::Paused => "● PAUSED",
            SimRunState::Idle => "● IDLE",
        };
        let run_color = match self.sim.run_state {
            SimRunState::Running => ui::EMERALD,
            SimRunState::Paused => ui::ORANGE,
            SimRunState::Idle => ui::MUTED,
        };
        let tools = row![
            action(
                if self.sim.is_running() {
                    "PAUSE"
                } else {
                    "PLAY"
                },
                Message::Run
            ),
            action("STOP", Message::Stop),
            action("STEP", Message::Step),
            action("RESET", Message::Reset),
            text(run_label).size(12).color(run_color),
            text(format!("Cycle: {}", self.sim.cycles()))
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

        // Simulation: world canvas is the full-bleed base layer; shell panels
        // (palette / inspector / dock) are overlays on top (plan z-order).
        // Hiding a panel reveals canvas that was already underneath — panels
        // must not be row/column siblings that shrink the canvas layout.
        // Other views keep a classic sibling workspace (editors need the gap).
        let workspace: Element<'_, Message> = match self.view {
            View::Simulation => self.simulation_workspace(),
            View::Code | View::Ladder | View::Grafcet => self.editor_workspace(),
        };

        let content = column![
            container(header)
                .padding([9, 12])
                .width(Length::Fill)
                .style(ui::panel),
            container(tools)
                .padding([8, 12])
                .width(Length::Fill)
                .style(|_| ui::surface(ui::LOWEST)),
            workspace,
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
        ]
        .spacing(1);

        container(content)
            .height(Length::Fill)
            .style(|_| ui::surface(ui::MAIN))
            .into()
    }

    /// Full-bleed Simulation stack: canvas underneath, chrome panels overlaid.
    fn simulation_workspace(&self) -> Element<'_, Message> {
        let empty: &[dendri_ast::Wire] = &[];
        let wiring = self
            .hub
            .as_ref()
            .map(|h| h.project().wiring.as_slice())
            .unwrap_or(empty);
        let canvas_layer = canvas(WorldCanvas {
            camera: self.camera,
            scene: &self.scene,
            wiring,
            selection: &self.selection,
            svgs: &self.scene_svgs,
            snapshot: &self.sim.snapshot,
            edits_allowed: self.sim.edits_allowed(),
            shift_held: self.shift_held,
            static_gen: self.static_gen,
            wire_activity_phase: self.sim.wire_activity_phase(),
        })
        .width(Length::Fill)
        .height(Length::Fill);

        let mut sides = row![].height(Length::Fill);
        if self.left {
            sides = sides.push(
                container(scrollable(self.palette()))
                    .padding(12)
                    .width(230)
                    .height(Length::Fill)
                    .style(ui::panel),
            );
        }
        sides = sides.push(Space::with_width(Length::Fill));
        if self.right {
            sides = sides.push(
                container(scrollable(self.inspector()))
                    .padding(12)
                    .width(310)
                    .height(Length::Fill)
                    .style(ui::panel),
            );
        }

        let mut chrome = column![sides].width(Length::Fill).height(Length::Fill);
        if self.bottom {
            chrome = chrome.push(self.bottom_dock());
        }

        let mut layers = stack![canvas_layer, chrome]
            .width(Length::Fill)
            .height(Length::Fill);

        if let Some((kind, id, x, y)) = self.context_menu.as_ref() {
            let menu = self.context_menu_view(*kind, id, *x, *y);
            layers = layers.push(menu);
        }

        layers.into()
    }

    fn context_menu_view(
        &self,
        kind: ContextMenuKind,
        id: &str,
        x: f32,
        y: f32,
    ) -> Element<'_, Message> {
        let paste_ok = self
            .hub
            .as_ref()
            .and_then(|h| h.canvas_clipboard.as_ref())
            .is_some();
        let menu_button = |label: String, msg: Message, enabled: bool| -> Element<'static, Message> {
            let mut b = button(text(label).size(12))
                .padding([6, 12])
                .width(Length::Fill)
                .style(ui::control);
            if enabled {
                b = b.on_press(msg);
            }
            b.into()
        };
        let entries: Element<'_, Message> = match kind {
            ContextMenuKind::Object => column![
                text(format!("· {id}")).size(11).color(ui::MUTED),
                menu_button("Copy".into(), Message::CanvasCopy, true),
                menu_button("Cut".into(), Message::CanvasCut, true),
                menu_button("Paste".into(), Message::CanvasPaste, paste_ok),
                menu_button("Duplicate".into(), Message::CanvasDuplicate, true),
                menu_button("Mirror H".into(), Message::MirrorSelectionX, true),
                menu_button("Mirror V".into(), Message::MirrorSelectionY, true),
                menu_button("Delete".into(), Message::DeleteSelection, true),
                menu_button("Close".into(), Message::DismissContextMenu, true),
            ]
            .spacing(2)
            .into(),
            ContextMenuKind::Wire => column![
                text(format!("· {id}")).size(11).color(ui::MUTED),
                menu_button("Delete wire".into(), Message::DeleteSelection, true),
                menu_button("Close".into(), Message::DismissContextMenu, true),
            ]
            .spacing(2)
            .into(),
            ContextMenuKind::Empty => column![
                text("Simulation").size(11).color(ui::MUTED),
                menu_button("Play".into(), Message::ContextPlay, true),
                menu_button("Pause".into(), Message::ContextPause, true),
                menu_button("Stop".into(), Message::ContextStop, true),
                menu_button("Reset".into(), Message::ContextReset, true),
                menu_button("Step".into(), Message::ContextStep, true),
                menu_button("Paste".into(), Message::CanvasPaste, paste_ok),
                menu_button("Close".into(), Message::DismissContextMenu, true),
            ]
            .spacing(2)
            .into(),
        };
        // Position via padding from top-left of the stack (approximate screen coords).
        container(
            container(entries)
                .padding(6)
                .width(160)
                .style(ui::card),
        )
        .padding(iced::Padding {
            top: y.max(0.0),
            right: 0.0,
            bottom: 0.0,
            left: x.max(0.0),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// Code / Ladder / GRAFCET: sibling panes (content between docks).
    fn editor_workspace(&self) -> Element<'_, Message> {
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
            View::Code => self.code(),
            View::Ladder | View::Grafcet => self.diagram(),
            View::Simulation => unreachable!("Simulation uses simulation_workspace"),
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

        let mut body = column![workspace].spacing(1).height(Length::Fill);
        if self.bottom {
            body = body.push(self.bottom_dock());
        }
        body.into()
    }

    fn bottom_dock(&self) -> Element<'_, Message> {
        container(
            row![
                column![
                    caption("LOCAL PREVIEW CONSOLE"),
                    text(format!(
                        "[{}] btnPin={}   pressed={}   ledPin={}   {}",
                        self.sim.cycles(),
                        bit(self.ui_raw()),
                        bit(self.ui_pressed()),
                        bit(self.ui_led()),
                        match self.sim.run_state {
                            SimRunState::Running => "Running",
                            SimRunState::Paused => "Paused",
                            SimRunState::Idle => "Idle",
                        }
                    ))
                    .font(Font::MONOSPACE)
                    .size(12),
                    text(if self.status.is_empty() {
                        "scan≠frame · dendri-sim · parse→ASL→emit live".into()
                    } else {
                        self.status.clone()
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
        .style(ui::panel)
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
                raw: self.ui_raw(),
                pressed: self.ui_pressed(),
                led: self.ui_led(),
                zoom: self.zoom,
            })
            .width(Length::Fill)
            .height(Length::FillPortion(3)),
            container(
                row![
                    if is_ladder {
                        checkbox("btnPin / D2 = HIGH (released)", self.ui_raw())
                            .on_toggle(Message::Raw)
                    } else {
                        checkbox("Physical button pressed", self.ui_pressed())
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
            tag("btnPin", "D2 / DI", self.ui_raw(), ui::CYAN),
            tag("pressed", "BOOL", self.ui_pressed(), ui::SECONDARY),
            tag("ledPin", "D13 / DO", self.ui_led(), ui::ORANGE),
        ]
        .spacing(12);

        let mut body = column![
            text(format!("INSPECTOR · {}", self.project_label))
                .size(12)
                .color(ui::SKY),
            container(watch).padding(12).style(ui::card),
            checkbox("Physical button pressed", self.ui_pressed())
                .on_toggle(Message::Physical),
            text("Sim canvas: scroll=zoom · middle-button=pan · line grid 1u=4mm")
                .size(11)
                .color(ui::MUTED),
            text(format!(
                "Bridge: {} · present_gen={}",
                match self.sim.run_state {
                    SimRunState::Running => "Running",
                    SimRunState::Paused => "Paused",
                    SimRunState::Idle => "Idle",
                },
                self.sim.snapshot.present_gen
            ))
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
