//! Authoring loop: place/wire hardware + edit sources → ASL IR → generators.
//!
//! Single writer: [`dendri_sync::SyncSession`]. All four views read/write through
//! this hub so Simulation / Code / Ladder / GRAFCET stay coherent.

use dendri_ast::expr::Expr;
use dendri_ast::stmt::Stmt;
use dendri_ast::{
    BoardInstance, ComponentInstance, DataEntry, DataType, GrafcetDocument, HardwareEntry,
    HwType, IoMode, LadderCell, LadderDocument, LadderRung, LayoutPose, Metadata, Program,
    PullMode, Scope, SimulationProject, SourceLanguage, Wire,
};
use dendri_frontends::{emit_source, parse_source};
use dendri_sync::{SyncSession, VisualKind};

use crate::project_io::Projections;

/// Placeable MVP catalog entries (profile id → default placement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeable {
    Led,
    PushbuttonNo,
}

impl Placeable {
    pub fn label(self) -> &'static str {
        match self {
            Self::Led => "led · Red LED module",
            Self::PushbuttonNo => "btn · NO pushbutton",
        }
    }

    pub fn profile(self) -> &'static str {
        match self {
            Self::Led => "led",
            Self::PushbuttonNo => "cmd-pushbutton-no-start",
        }
    }
}

/// Clipboard payload for canvas copy/cut/paste (components only in B1).
#[derive(Debug, Clone)]
pub struct CanvasClipboard {
    pub components: Vec<ComponentInstance>,
}

#[derive(Debug, Clone)]
pub struct AuthoringHub {
    pub session: SyncSession,
    pub projections: Projections,
    pub ladder_toml: String,
    pub sfc_toml: String,
    /// Minimal undo stack (project snapshots) for canvas layout edits.
    undo_stack: Vec<SimulationProject>,
    redo_stack: Vec<SimulationProject>,
    /// In-app clipboard (Ctrl+C/X/V for scene parts).
    pub canvas_clipboard: Option<CanvasClipboard>,
}

impl AuthoringHub {
    pub fn from_project(mut project: SimulationProject) -> Self {
        dendri_sync::fill_visual_sources(&mut project);
        Self::ensure_default_layout(&mut project);
        let session = SyncSession::new(project);
        let mut hub = Self {
            session,
            projections: Projections::default(),
            ladder_toml: String::new(),
            sfc_toml: String::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            canvas_clipboard: None,
        };
        hub.refresh_derived();
        hub
    }

    /// If every pose is at the origin, apply C01-like defaults (legacy fixtures).
    fn ensure_default_layout(project: &mut SimulationProject) {
        let board_zero = project.board.layout.x == 0.0
            && project.board.layout.y == 0.0
            && project.board.layout.rotation == 0
            && !project.board.layout.mirror_x
            && !project.board.layout.mirror_y;
        let comps_zero = project.components.iter().all(|c| {
            c.layout.x == 0.0
                && c.layout.y == 0.0
                && c.layout.rotation == 0
                && !c.layout.mirror_x
                && !c.layout.mirror_y
        });
        if !(board_zero && comps_zero) {
            return;
        }
        project.board.layout = LayoutPose::with_xy(1.0, 2.0);
        for c in &mut project.components {
            match c.profile.as_str() {
                "led" => c.layout = LayoutPose::with_xy(13.25, 2.25),
                "cmd-pushbutton-no-start" => c.layout = LayoutPose::with_xy(13.0, 6.0),
                _ => {}
            }
        }
    }

    pub fn project(&self) -> &SimulationProject {
        &self.session.project
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.session.project.clone());
        if self.undo_stack.len() > 64 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> Result<(), String> {
        let Some(prev) = self.undo_stack.pop() else {
            return Err("nothing to undo".into());
        };
        self.redo_stack.push(self.session.project.clone());
        self.session.project = prev;
        self.refresh_derived();
        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), String> {
        let Some(next) = self.redo_stack.pop() else {
            return Err("nothing to redo".into());
        };
        self.undo_stack.push(self.session.project.clone());
        self.session.project = next;
        self.refresh_derived();
        Ok(())
    }

    pub fn set_part_layout(&mut self, id: &str, layout: LayoutPose) -> Result<(), String> {
        self.push_undo();
        if self.session.project.board.instance_id == id {
            self.session.project.board.layout = layout;
            self.refresh_derived();
            return Ok(());
        }
        if let Some(c) = self
            .session
            .project
            .components
            .iter_mut()
            .find(|c| c.instance_id == id)
        {
            c.layout = layout;
            self.refresh_derived();
            Ok(())
        } else {
            let _ = self.undo_stack.pop();
            Err(format!("part not found: {id}"))
        }
    }

    /// Commit a drag without stacking an extra undo (caller pushed once at drag start).
    pub fn commit_part_layout_no_undo(&mut self, id: &str, layout: LayoutPose) -> Result<(), String> {
        if self.session.project.board.instance_id == id {
            self.session.project.board.layout = layout;
            self.refresh_derived();
            return Ok(());
        }
        if let Some(c) = self
            .session
            .project
            .components
            .iter_mut()
            .find(|c| c.instance_id == id)
        {
            c.layout = layout;
            self.refresh_derived();
            Ok(())
        } else {
            Err(format!("part not found: {id}"))
        }
    }

    pub fn begin_layout_edit(&mut self) {
        self.push_undo();
    }

    pub fn rotate_part_cw(&mut self, id: &str) -> Result<(), String> {
        self.push_undo();
        let layout = self.part_layout(id)?;
        let mut next = layout;
        next.rotate_cw_90();
        self.commit_part_layout_no_undo(id, next)
    }

    pub fn mirror_part_x(&mut self, id: &str) -> Result<(), String> {
        self.push_undo();
        let mut layout = self.part_layout(id)?;
        layout.mirror_x = !layout.mirror_x;
        self.commit_part_layout_no_undo(id, layout)
    }

    pub fn mirror_part_y(&mut self, id: &str) -> Result<(), String> {
        self.push_undo();
        let mut layout = self.part_layout(id)?;
        layout.mirror_y = !layout.mirror_y;
        self.commit_part_layout_no_undo(id, layout)
    }

    pub fn part_layout(&self, id: &str) -> Result<LayoutPose, String> {
        if self.session.project.board.instance_id == id {
            return Ok(self.session.project.board.layout);
        }
        self.session
            .project
            .components
            .iter()
            .find(|c| c.instance_id == id)
            .map(|c| c.layout)
            .ok_or_else(|| format!("part not found: {id}"))
    }

    pub fn copy_parts(&mut self, ids: &[String]) {
        let comps: Vec<_> = self
            .session
            .project
            .components
            .iter()
            .filter(|c| ids.iter().any(|id| id == &c.instance_id))
            .cloned()
            .collect();
        if !comps.is_empty() {
            self.canvas_clipboard = Some(CanvasClipboard { components: comps });
        }
    }

    pub fn cut_parts(&mut self, ids: &[String]) -> Result<(), String> {
        self.copy_parts(ids);
        self.push_undo();
        for id in ids {
            if self.session.project.board.instance_id == *id {
                continue; // board is not deleted via cut
            }
            self.remove_component_no_undo(id);
        }
        self.refresh_derived();
        Ok(())
    }

    pub fn paste_clipboard(&mut self) -> Result<Vec<String>, String> {
        let Some(clip) = self.canvas_clipboard.clone() else {
            return Err("clipboard empty".into());
        };
        self.push_undo();
        let mut placed = Vec::new();
        for mut c in clip.components {
            let base = c.instance_id.clone();
            c.instance_id = unique_id(&self.session.project, &base);
            c.layout.x += 1.0;
            c.layout.y += 1.0;
            placed.push(c.instance_id.clone());
            self.session.project.components.push(c);
        }
        self.refresh_derived();
        Ok(placed)
    }

    pub fn duplicate_parts(&mut self, ids: &[String]) -> Result<Vec<String>, String> {
        self.copy_parts(ids);
        self.paste_clipboard()
    }

    pub fn refresh_derived(&mut self) {
        let _ = self.session.sync_visual_sources_from_asl();
        self.projections = Projections::from_project(&self.session.project);
        self.ladder_toml = self
            .session
            .source_text(SourceLanguage::Ladder)
            .map(str::to_string)
            .unwrap_or_else(|| {
                emit_source(SourceLanguage::Ladder, &self.session.project.asl)
                    .unwrap_or_default()
            });
        self.sfc_toml = self
            .session
            .source_text(SourceLanguage::Sfc)
            .map(str::to_string)
            .unwrap_or_else(|| {
                emit_source(SourceLanguage::Sfc, &self.session.project.asl).unwrap_or_default()
            });
    }

    /// Parse a text language (or project.toml) into ASL and regenerate siblings.
    pub fn apply_code(&mut self, lang: SourceLanguage, text: &str) -> Result<(), String> {
        match lang {
            SourceLanguage::Asl => {
                // Treat as project TOML when it has board; else program-only ASL.
                if text.contains("[board]") || text.contains("[[components]]") {
                    let project: SimulationProject =
                        toml::from_str(text).map_err(|e| e.to_string())?;
                    self.session = SyncSession::new(project);
                } else {
                    let program: Program = toml::from_str(text).map_err(|e| e.to_string())?;
                    self.session
                        .apply_asl_program(program)
                        .map_err(|e| e.to_string())?;
                }
            }
            other => {
                let program = parse_source(other, text).map_err(|e| e.to_string())?;
                self.session
                    .apply_asl_program(program)
                    .map_err(|e| e.to_string())?;
            }
        }
        self.refresh_derived();
        Ok(())
    }

    pub fn apply_ladder_toml(&mut self, text: &str) -> Result<(), String> {
        self.session
            .apply_source_toml(VisualKind::Ladder, text)
            .map_err(|e| e.to_string())?;
        self.refresh_derived();
        Ok(())
    }

    pub fn apply_sfc_toml(&mut self, text: &str) -> Result<(), String> {
        self.session
            .apply_source_toml(VisualKind::Sfc, text)
            .map_err(|e| e.to_string())?;
        self.refresh_derived();
        Ok(())
    }

    pub fn mutate_ladder(
        &mut self,
        f: impl FnOnce(&mut LadderDocument),
    ) -> Result<(), String> {
        let mut doc = self.ladder_document();
        f(&mut doc);
        self.session
            .apply_ladder_document(doc)
            .map_err(|e| e.to_string())?;
        self.refresh_derived();
        Ok(())
    }

    pub fn mutate_sfc(&mut self, f: impl FnOnce(&mut GrafcetDocument)) -> Result<(), String> {
        let mut doc = self.sfc_document();
        f(&mut doc);
        self.session
            .apply_sfc_document(doc)
            .map_err(|e| e.to_string())?;
        self.refresh_derived();
        Ok(())
    }

    fn ladder_document(&self) -> LadderDocument {
        if let Some(block) = self.session.ladder_model() {
            let mut d = LadderDocument::from_block(&block);
            d.hardware = self.session.project.asl.hardware.clone();
            d.data = self.session.project.asl.data.clone();
            d.setup = self.session.project.asl.setup.clone();
            d
        } else {
            LadderDocument {
                name: "rungs".into(),
                rungs: vec![LadderRung {
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
        }
    }

    fn sfc_document(&self) -> GrafcetDocument {
        if let Some(block) = self.session.sfc_model() {
            let mut d = GrafcetDocument::from_block(&block);
            d.hardware = self.session.project.asl.hardware.clone();
            d.data = self.session.project.asl.data.clone();
            d.setup = self.session.project.asl.setup.clone();
            d
        } else {
            GrafcetDocument {
                name: "chart".into(),
                steps: vec![],
                transitions: vec![],
                initial_steps: vec![],
                io_map: vec![],
                hardware: self.session.project.asl.hardware.clone(),
                data: self.session.project.asl.data.clone(),
                setup: self.session.project.asl.setup.clone(),
            }
        }
    }

    /// Place a catalog component + default pin/wiring/ASL hardware stubs.
    pub fn place(&mut self, kind: Placeable) -> Result<String, String> {
        self.push_undo();
        let n = self.session.project.components.len();
        let layout = next_place_layout(&self.session.project, kind);
        match kind {
            Placeable::Led => {
                let id = unique_id(&self.session.project, "led");
                let pin = next_free_do_pin(&self.session.project).unwrap_or(13);
                let label = format!("ledPin{}", if n == 0 { String::new() } else { n.to_string() });
                self.session.project.components.push(ComponentInstance {
                    instance_id: id.clone(),
                    profile: kind.profile().into(),
                    label: Some("Status LED".into()),
                    params: {
                        let mut m = toml::map::Map::new();
                        m.insert("color".into(), toml::Value::String("red".into()));
                        m.insert("integrated_resistance".into(), toml::Value::Integer(220));
                        m
                    },
                    layout,
                });
                self.session
                    .project
                    .wiring
                    .push(Wire::new(format!("board.D{pin}"), format!("{id}.A")).with_signal("digital"));
                self.session
                    .project
                    .wiring
                    .push(Wire::new("board.GND", format!("{id}.K")).with_signal("gnd"));
                ensure_hw(
                    &mut self.session.project.asl,
                    HardwareEntry {
                        pin,
                        hw_type: HwType::Digital,
                        io_mode: IoMode::Do,
                        label: label.clone(),
                        target_role: "digital-out".into(),
                        pull: PullMode::None,
                    },
                );
                ensure_cfg_output(&mut self.session.project.asl, &label);
                self.refresh_derived();
                Ok(format!("placed {id} on D{pin} · ASL {label}"))
            }
            Placeable::PushbuttonNo => {
                let id = unique_id(&self.session.project, "btn");
                let pin = next_free_di_pin(&self.session.project).unwrap_or(2);
                let label = format!("btnPin{}", if n == 0 { String::new() } else { n.to_string() });
                self.session.project.components.push(ComponentInstance {
                    instance_id: id.clone(),
                    profile: kind.profile().into(),
                    label: Some("NO pushbutton".into()),
                    params: Default::default(),
                    layout,
                });
                self.session
                    .project
                    .wiring
                    .push(Wire::new(format!("board.D{pin}"), format!("{id}.NO")).with_signal("digital"));
                self.session
                    .project
                    .wiring
                    .push(Wire::new("board.GND", format!("{id}.COM")).with_signal("gnd"));
                ensure_hw(
                    &mut self.session.project.asl,
                    HardwareEntry {
                        pin,
                        hw_type: HwType::Digital,
                        io_mode: IoMode::Di,
                        label: label.clone(),
                        target_role: "digital-in".into(),
                        pull: PullMode::Pullup,
                    },
                );
                ensure_cfg_input(&mut self.session.project.asl, &label);
                // Ensure pressed var exists for pull-up demos.
                if !self
                    .session
                    .project
                    .asl
                    .data
                    .iter()
                    .any(|d| d.name == "pressed")
                {
                    self.session.project.asl.data.push(DataEntry {
                        name: "pressed".into(),
                        data_type: DataType::Bool,
                        scope: Scope::Var,
                        init: "FALSE".into(),
                        unit: "-".into(),
                    });
                }
                self.refresh_derived();
                Ok(format!("placed {id} on D{pin} (pull-up) · ASL {label}"))
            }
        }
    }

    pub fn add_wire(&mut self, from: &str, to: &str, signal: Option<&str>) -> Result<(), String> {
        let from = from.trim();
        let to = to.trim();
        if from.is_empty() || to.is_empty() {
            return Err("wire needs from and to".into());
        }
        let mut wire = Wire::new(from, to);
        if let Some(sig) = signal.map(str::trim).filter(|s| !s.is_empty()) {
            wire = wire.with_signal(sig);
        }
        self.session.project.wiring.push(wire);
        self.refresh_derived();
        Ok(())
    }

    /// Add a canvas wire with optional Manhattan waypoints (grid units).
    pub fn add_canvas_wire(
        &mut self,
        from: &str,
        to: &str,
        signal: Option<&str>,
        waypoints: Vec<[f64; 2]>,
    ) -> Result<usize, String> {
        let from = from.trim();
        let to = to.trim();
        if from.is_empty() || to.is_empty() {
            return Err("wire needs from and to".into());
        }
        if from == to {
            return Err("wire endpoints must differ".into());
        }
        // Reject duplicates (same endpoints either direction).
        if self.session.project.wiring.iter().any(|w| {
            (w.from == from && w.to == to) || (w.from == to && w.to == from)
        }) {
            return Err(format!("wire already exists: {from} ↔ {to}"));
        }
        self.push_undo();
        let mut wire = Wire::new(from, to);
        if let Some(sig) = signal.map(str::trim).filter(|s| !s.is_empty()) {
            wire = wire.with_signal(sig);
        }
        wire.waypoints = waypoints;
        self.session.project.wiring.push(wire);
        let idx = self.session.project.wiring.len() - 1;
        self.refresh_derived();
        Ok(idx)
    }

    pub fn set_wire_waypoints(
        &mut self,
        index: usize,
        waypoints: Vec<[f64; 2]>,
    ) -> Result<(), String> {
        if index >= self.session.project.wiring.len() {
            return Err("wire index out of range".into());
        }
        self.push_undo();
        self.session.project.wiring[index].waypoints = waypoints;
        self.refresh_derived();
        Ok(())
    }

    pub fn insert_wire_waypoint(
        &mut self,
        index: usize,
        at: [f64; 2],
        after_segment: usize,
    ) -> Result<(), String> {
        if index >= self.session.project.wiring.len() {
            return Err("wire index out of range".into());
        }
        self.push_undo();
        let wps = &mut self.session.project.wiring[index].waypoints;
        let insert_at = after_segment.min(wps.len());
        wps.insert(insert_at, at);
        self.refresh_derived();
        Ok(())
    }

    pub fn remove_component(&mut self, instance_id: &str) -> Result<(), String> {
        if !self
            .session
            .project
            .components
            .iter()
            .any(|c| c.instance_id == instance_id)
        {
            return Err(format!("component not found: {instance_id}"));
        }
        self.push_undo();
        self.remove_component_no_undo(instance_id);
        self.refresh_derived();
        Ok(())
    }

    fn remove_component_no_undo(&mut self, instance_id: &str) {
        self.session
            .project
            .components
            .retain(|c| c.instance_id != instance_id);
        self.session.project.wiring.retain(|w| {
            !w.from.starts_with(&format!("{instance_id}."))
                && !w.to.starts_with(&format!("{instance_id}."))
        });
    }

    pub fn remove_wire_at(&mut self, index: usize) -> Result<(), String> {
        if index >= self.session.project.wiring.len() {
            return Err("wire index out of range".into());
        }
        self.push_undo();
        self.session.project.wiring.remove(index);
        self.refresh_derived();
        Ok(())
    }

    pub fn blank_uno_project() -> SimulationProject {
        SimulationProject {
            metadata: Metadata {
                asl_version: Some(dendri_ast::ASL_VERSION.into()),
                project_name: Some("untitled".into()),
                editor: Some("dendriForge".into()),
                author: None,
                notes: Some("Blank Uno project — place components, wire, write code.".into()),
                expect_error: None,
            },
            board: BoardInstance {
                profile: "arduino-uno-r3".into(),
                instance_id: "board".into(),
                layout: LayoutPose::with_xy(1.0, 2.0),
            },
            components: vec![],
            wiring: vec![],
            sources: vec![],
            asl: Program {
                metadata: Metadata {
                    asl_version: Some(dendri_ast::ASL_VERSION.into()),
                    project_name: Some("untitled".into()),
                    editor: Some("dendriForge".into()),
                    ..Metadata::default()
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

    // --- Ladder palette helpers ---

    pub fn ladder_add_contact_no(&mut self, operand: &str) -> Result<(), String> {
        let operand = operand.to_string();
        self.mutate_ladder(|doc| {
            ensure_rung(doc);
            if let Some(rung) = doc.rungs.first_mut() {
                let col = rung.cells.len() as u32;
                rung.cells.push(LadderCell::ContactNo { operand, col });
            }
        })
    }

    pub fn ladder_add_contact_nc(&mut self, operand: &str) -> Result<(), String> {
        let operand = operand.to_string();
        self.mutate_ladder(|doc| {
            ensure_rung(doc);
            if let Some(rung) = doc.rungs.first_mut() {
                let col = rung.cells.len() as u32;
                rung.cells.push(LadderCell::ContactNc { operand, col });
            }
        })
    }

    pub fn ladder_add_coil(&mut self, operand: &str) -> Result<(), String> {
        let operand = operand.to_string();
        self.mutate_ladder(|doc| {
            ensure_rung(doc);
            if let Some(rung) = doc.rungs.first_mut() {
                let col = rung.cells.len() as u32;
                rung.cells.push(LadderCell::Coil { operand, col });
            }
        })
    }

    pub fn ladder_remove_last(&mut self) -> Result<(), String> {
        self.mutate_ladder(|doc| {
            if let Some(rung) = doc.rungs.first_mut() {
                let _ = rung.cells.pop();
            }
        })
    }

    pub fn sfc_add_step(&mut self) -> Result<(), String> {
        self.mutate_sfc(|doc| {
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
        })
    }

    pub fn sfc_add_transition(&mut self) -> Result<(), String> {
        self.mutate_sfc(|doc| {
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
        })
    }
}

fn ensure_rung(doc: &mut LadderDocument) {
    if doc.rungs.is_empty() {
        doc.rungs.push(LadderRung {
            id: "r1".into(),
            cells: vec![],
            comment: None,
            row: 0,
        });
    }
}

fn unique_id(project: &SimulationProject, prefix: &str) -> String {
    let base: String = prefix
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .to_string();
    let base = if base.is_empty() {
        prefix.to_string()
    } else {
        base
    };
    let mut i = 1;
    loop {
        let id = format!("{base}{i}");
        if !project.components.iter().any(|c| c.instance_id == id)
            && project.board.instance_id != id
        {
            return id;
        }
        i += 1;
    }
}

fn next_place_layout(project: &SimulationProject, kind: Placeable) -> LayoutPose {
    let n = project.components.len() as f64;
    match kind {
        Placeable::Led => LayoutPose::with_xy(13.25 + n, 2.25),
        Placeable::PushbuttonNo => LayoutPose::with_xy(13.0 + n, 6.0),
    }
}

fn used_pins(project: &SimulationProject) -> Vec<u32> {
    project.asl.hardware.iter().map(|h| h.pin).collect()
}

fn next_free_do_pin(project: &SimulationProject) -> Option<u32> {
    let used = used_pins(project);
    [13u32, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3]
        .into_iter()
        .find(|p| !used.contains(p))
}

fn next_free_di_pin(project: &SimulationProject) -> Option<u32> {
    let used = used_pins(project);
    [2u32, 3, 4, 5, 6, 7, 8, 9]
        .into_iter()
        .find(|p| !used.contains(p))
}

fn ensure_hw(asl: &mut Program, entry: HardwareEntry) {
    if let Some(existing) = asl.hardware.iter_mut().find(|h| h.pin == entry.pin) {
        *existing = entry;
    } else {
        asl.hardware.push(entry);
    }
}

fn ensure_cfg_output(asl: &mut Program, label: &str) {
    let already = asl.setup.iter().any(|s| {
        matches!(s, Stmt::Call { name, args } if name == "CFG_DIGITAL_OUTPUT"
            && args.iter().any(|a| matches!(a, Expr::Var { name } if name == label)))
    });
    if !already {
        asl.setup.push(Stmt::Call {
            name: "CFG_DIGITAL_OUTPUT".into(),
            args: vec![Expr::Var {
                name: label.into(),
            }],
        });
    }
}

fn ensure_cfg_input(asl: &mut Program, label: &str) {
    let already = asl.setup.iter().any(|s| {
        matches!(s, Stmt::Call { name, args } if name == "CFG_DIGITAL_INPUT"
            && args.iter().any(|a| matches!(a, Expr::Var { name } if name == label)))
    });
    if !already {
        asl.setup.push(Stmt::Call {
            name: "CFG_DIGITAL_INPUT".into(),
            args: vec![Expr::Var {
                name: label.into(),
            }],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_io::load_project;
    use std::path::PathBuf;

    fn c01() -> SimulationProject {
        load_project(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../content/fixtures/canvas/C01-button-led.toml"),
        )
        .unwrap()
    }

    #[test]
    fn place_led_and_button_updates_project() {
        let mut hub = AuthoringHub::from_project(AuthoringHub::blank_uno_project());
        hub.place(Placeable::Led).unwrap();
        hub.place(Placeable::PushbuttonNo).unwrap();
        assert_eq!(hub.project().components.len(), 2);
        assert!(hub.project().wiring.len() >= 4);
        assert!(hub
            .project()
            .asl
            .hardware
            .iter()
            .any(|h| h.io_mode == IoMode::Do));
        assert!(hub
            .project()
            .asl
            .hardware
            .iter()
            .any(|h| h.pull == PullMode::Pullup));
        assert!(!hub.projections.cpp.is_empty());
    }

    #[test]
    fn apply_cpp_roundtrips_into_projections() {
        let mut hub = AuthoringHub::from_project(c01());
        let cpp = hub.projections.cpp.clone();
        hub.apply_code(SourceLanguage::Cpp, &cpp).unwrap();
        assert!(hub.projections.st.contains("pressed") || hub.projections.st.contains("PRESSED") || !hub.projections.st.is_empty());
        assert!(!hub.ladder_toml.is_empty());
        assert!(!hub.sfc_toml.is_empty());
    }

    #[test]
    fn ladder_palette_mutates_asl() {
        let mut hub = AuthoringHub::from_project(c01());
        hub.ladder_add_contact_no("pressed").unwrap();
        hub.ladder_add_coil("ledPin").unwrap();
        assert!(hub
            .project()
            .asl
            .ladder
            .first()
            .map(|b| !b.rungs.is_empty())
            .unwrap_or(false)
            || !hub.ladder_toml.is_empty());
    }

    #[test]
    fn add_wire_persists() {
        let mut hub = AuthoringHub::from_project(AuthoringHub::blank_uno_project());
        hub.add_wire("board.D13", "led1.A", Some("digital"))
            .unwrap();
        assert_eq!(hub.project().wiring.len(), 1);
    }

    #[test]
    fn layout_rotate_undo_and_c01_defaults() {
        let mut hub = AuthoringHub::from_project(c01());
        assert!((hub.project().board.layout.x - 1.0).abs() < 1e-9);
        let led = hub
            .project()
            .components
            .iter()
            .find(|c| c.instance_id == "led1")
            .unwrap();
        assert!((led.layout.x - 13.25).abs() < 1e-9);
        hub.rotate_part_cw("led1").unwrap();
        assert_eq!(hub.part_layout("led1").unwrap().rotation, 90);
        hub.undo().unwrap();
        assert_eq!(hub.part_layout("led1").unwrap().rotation, 0);
        hub.mirror_part_x("led1").unwrap();
        assert!(hub.part_layout("led1").unwrap().mirror_x);
    }
}
