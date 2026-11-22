//! Phase B1 / D0b scene graph: boards/components from the project with layout poses.
//!
//! Bodies use SVG sizes (mm → grid) when a profile store is available; curated
//! MVP sizes remain for Uno / LED / pushbutton. Terminals sit on full-module
//! intersections after transform (snapped). Unknown profiles are no longer
//! skipped (D-comp-11). Drag snaps origins to the half-module.

use std::fs;
use std::path::Path;

use dendri_ast::{LayoutPose, SimulationProject};
use dendri_profiles::{ProfileDocument, ProfileStore};

use crate::camera::{WorldPoint, WorldRect, WorldSize};
use crate::grid::{is_terminal_intersection, mm_to_grid, snap_half_module, snap_terminal};
use crate::sim_role::{sim_role_from_doc, SimRole};

/// Electrical role for pin coloring (mirrors editor A11Y theme roles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalRole {
    Digital,
    Analog,
    Power,
    Gnd,
    Reset,
    Nc,
}

/// One connection terminal (borne) in local part coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalTerminal {
    pub port: String,
    pub label: String,
    pub local: WorldPoint,
    pub role: TerminalRole,
}

/// Kind of placeable part in the Simulation scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    Board,
    Component,
}

/// Visual / curated kind for known MVP assets; [`Generic`] for any other profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MvpProfile {
    UnoR3,
    Led,
    Pushbutton,
    /// Unknown / non-MVP profile — size + terminals from profile/SVG (D-comp-11).
    Generic,
}

impl MvpProfile {
    pub fn from_profile_id(profile: &str) -> Self {
        match profile {
            "arduino-uno-r3" | "uno" | "arduino-uno" => Self::UnoR3,
            "led" => Self::Led,
            "cmd-pushbutton-no-start" => Self::Pushbutton,
            _ => Self::Generic,
        }
    }

    /// Outer SVG size in millimetres (contract: multiples of 2 mm).
    pub fn size_mm(self) -> (f64, f64) {
        match self {
            Self::UnoR3 => (76.0, 54.0),
            Self::Led => (10.0, 10.0),
            Self::Pushbutton => (24.0, 24.0),
            Self::Generic => (20.0, 20.0),
        }
    }

    pub fn size_grid(self) -> WorldSize {
        let (w, h) = self.size_mm();
        WorldSize::new(mm_to_grid(w), mm_to_grid(h))
    }

    /// Local terminal offsets in grid units (pre-transform).
    ///
    /// Uno includes **A0** (scene-only; SVG tags unchanged — D-comp-12 / D-C5).
    pub fn local_terminals(self) -> Vec<LocalTerminal> {
        match self {
            Self::UnoR3 => {
                // Right-edge digital header + left power / A0 (module-aligned).
                vec![
                    LocalTerminal {
                        port: "D13".into(),
                        label: "D13".into(),
                        local: WorldPoint::new(19.0, 2.0),
                        role: TerminalRole::Digital,
                    },
                    LocalTerminal {
                        port: "D2".into(),
                        label: "D2".into(),
                        local: WorldPoint::new(19.0, 5.0),
                        role: TerminalRole::Digital,
                    },
                    LocalTerminal {
                        port: "GND".into(),
                        label: "GND".into(),
                        local: WorldPoint::new(19.0, 8.0),
                        role: TerminalRole::Gnd,
                    },
                    LocalTerminal {
                        port: "5V".into(),
                        label: "5V".into(),
                        local: WorldPoint::new(0.0, 8.0),
                        role: TerminalRole::Power,
                    },
                    LocalTerminal {
                        port: "A0".into(),
                        label: "A0".into(),
                        local: WorldPoint::new(0.0, 5.0),
                        role: TerminalRole::Analog,
                    },
                ]
            }
            Self::Led => vec![
                LocalTerminal {
                    port: "A".into(),
                    label: "A".into(),
                    local: WorldPoint::new(0.0, 1.0),
                    role: TerminalRole::Digital,
                },
                LocalTerminal {
                    port: "K".into(),
                    label: "K".into(),
                    local: WorldPoint::new(0.0, 2.0),
                    role: TerminalRole::Gnd,
                },
            ],
            Self::Pushbutton => vec![
                LocalTerminal {
                    port: "NO".into(),
                    label: "NO".into(),
                    local: WorldPoint::new(0.0, 2.0),
                    role: TerminalRole::Digital,
                },
                LocalTerminal {
                    port: "COM".into(),
                    label: "COM".into(),
                    local: WorldPoint::new(0.0, 4.0),
                    role: TerminalRole::Gnd,
                },
            ],
            Self::Generic => Vec::new(),
        }
    }
}

/// One placed part in the scene (board or component).
#[derive(Debug, Clone, PartialEq)]
pub struct ScenePart {
    pub id: String,
    pub kind: PartKind,
    pub profile: MvpProfile,
    pub profile_id: String,
    /// From `[dendri].sim_role` when the profile declares it (D-comp-14).
    pub sim_role: Option<SimRole>,
    pub pose: LayoutPose,
    pub size: WorldSize,
    pub terminals: Vec<LocalTerminal>,
}

impl ScenePart {
    /// Axis-aligned world bounds of the **unrotated** body AABB (pose origin).
    pub fn unrotated_rect(&self) -> WorldRect {
        WorldRect {
            min: WorldPoint::new(self.pose.x, self.pose.y),
            max: WorldPoint::new(self.pose.x + self.size.w, self.pose.y + self.size.h),
        }
    }

    /// AABB after rotation about body center (mirrors applied in local space first).
    pub fn world_aabb(&self) -> WorldRect {
        let corners = [
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(self.size.w, 0.0),
            WorldPoint::new(self.size.w, self.size.h),
            WorldPoint::new(0.0, self.size.h),
        ];
        let mut xs = [0.0; 4];
        let mut ys = [0.0; 4];
        for (i, c) in corners.iter().enumerate() {
            let w = self.local_to_world(*c);
            xs[i] = w.x;
            ys[i] = w.y;
        }
        WorldRect {
            min: WorldPoint::new(
                xs.iter().cloned().fold(f64::INFINITY, f64::min),
                ys.iter().cloned().fold(f64::INFINITY, f64::min),
            ),
            max: WorldPoint::new(
                xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            ),
        }
    }

    pub fn contains_world(&self, p: WorldPoint) -> bool {
        let r = self.world_aabb();
        p.x >= r.min.x && p.x <= r.max.x && p.y >= r.min.y && p.y <= r.max.y
    }

    /// Local → world with mirror, then rotation about body center; terminals snapped.
    pub fn local_to_world(&self, local: WorldPoint) -> WorldPoint {
        transform_local(self.pose, self.size, local)
    }

    pub fn terminal_world(&self, t: &LocalTerminal) -> WorldPoint {
        let w = self.local_to_world(t.local);
        WorldPoint::new(snap_terminal(w.x), snap_terminal(w.y))
    }

    pub fn endpoint_id(&self, port: &str) -> String {
        format!("{}.{}", self.id, port)
    }
}

fn transform_local(pose: LayoutPose, size: WorldSize, local: WorldPoint) -> WorldPoint {
    let mut lx = local.x;
    let mut ly = local.y;
    if pose.mirror_x {
        lx = size.w - lx;
    }
    if pose.mirror_y {
        ly = size.h - ly;
    }
    let cx = size.w * 0.5;
    let cy = size.h * 0.5;
    let dx = lx - cx;
    let dy = ly - cy;
    let (rx, ry) = match pose.rotation % 360 {
        90 => (-dy, dx),
        180 => (-dx, -dy),
        270 => (dy, -dx),
        _ => (dx, dy),
    };
    WorldPoint::new(pose.x + cx + rx, pose.y + cy + ry)
}

/// Full Simulation scene built from a [`SimulationProject`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scene {
    pub parts: Vec<ScenePart>,
}

impl Scene {
    /// Build from project using curated MVP sizes/terminals when known.
    ///
    /// Prefer [`Self::from_project_with_store`] so unknown profiles get size +
    /// terminals from profile/SVG (D-comp-11). Without a store, Generic parts
    /// still appear with default size and empty terminals.
    pub fn from_project(project: &SimulationProject) -> Self {
        Self::from_project_opt(project, None)
    }

    /// Build scene with profile/SVG size and port terminals for non-MVP parts.
    pub fn from_project_with_store(project: &SimulationProject, store: &ProfileStore) -> Self {
        Self::from_project_opt(project, Some(store))
    }

    fn from_project_opt(project: &SimulationProject, store: Option<&ProfileStore>) -> Self {
        let mut parts = Vec::new();
        parts.push(build_board_part(project, store));
        for c in &project.components {
            parts.push(build_component_part(
                &c.instance_id,
                &c.profile,
                c.layout,
                store,
            ));
        }
        Self { parts }
    }

    /// C01-like default layout when poses are all zero (legacy fixtures).
    pub fn with_default_layout_if_empty(mut self) -> Self {
        let all_zero = self.parts.iter().all(|p| {
            p.pose.x == 0.0
                && p.pose.y == 0.0
                && p.pose.rotation == 0
                && !p.pose.mirror_x
                && !p.pose.mirror_y
        });
        if !all_zero || self.parts.is_empty() {
            return self;
        }
        let mut generic_x = 13.0;
        for p in &mut self.parts {
            match p.profile {
                MvpProfile::UnoR3 => {
                    p.pose = LayoutPose::with_xy(1.0, 2.0);
                }
                MvpProfile::Led => {
                    // Center near former Phase A (14.5, 3.5) with 2.5×2.5 body.
                    p.pose = LayoutPose::with_xy(13.25, 2.25);
                }
                MvpProfile::Pushbutton => {
                    p.pose = LayoutPose::with_xy(13.0, 6.0);
                }
                MvpProfile::Generic => {
                    p.pose = LayoutPose::with_xy(generic_x, 10.0);
                    generic_x += p.size.w + 1.0;
                }
            }
        }
        self
    }

    /// Top-most part under a world point (later parts paint above earlier).
    pub fn pick(&self, world: WorldPoint) -> Option<&ScenePart> {
        self.parts.iter().rev().find(|p| p.contains_world(world))
    }

    pub fn part_mut(&mut self, id: &str) -> Option<&mut ScenePart> {
        self.parts.iter_mut().find(|p| p.id == id)
    }

    pub fn part(&self, id: &str) -> Option<&ScenePart> {
        self.parts.iter().find(|p| p.id == id)
    }

    /// Snap a drag origin to the half-module lattice.
    pub fn snap_origin(x: f64, y: f64) -> (f64, f64) {
        (snap_half_module(x), snap_half_module(y))
    }

    pub fn assert_terminal_contract(&self) {
        for part in &self.parts {
            for t in &part.terminals {
                let w = part.terminal_world(t);
                assert!(
                    is_terminal_intersection(w.x) && is_terminal_intersection(w.y),
                    "{} ({},{}) not on full-module after transform",
                    part.endpoint_id(&t.port),
                    w.x,
                    w.y
                );
            }
        }
    }
}

fn build_board_part(project: &SimulationProject, store: Option<&ProfileStore>) -> ScenePart {
    let profile_id = project.board.profile.clone();
    let kind = MvpProfile::from_profile_id(&profile_id);
    let (doc, family) = load_profile_doc(store, &profile_id, true);
    let sim_role = doc.as_ref().and_then(sim_role_from_doc);
    let (size, terminals) = match kind {
        MvpProfile::UnoR3 | MvpProfile::Led | MvpProfile::Pushbutton => {
            (kind.size_grid(), kind.local_terminals())
        }
        MvpProfile::Generic => resolve_size_and_terminals(
            kind,
            doc.as_ref(),
            store,
            family,
            true,
        ),
    };
    ScenePart {
        id: project.board.instance_id.clone(),
        kind: PartKind::Board,
        profile: kind,
        profile_id,
        sim_role,
        pose: project.board.layout,
        size,
        terminals,
    }
}

fn build_component_part(
    instance_id: &str,
    profile_id: &str,
    pose: LayoutPose,
    store: Option<&ProfileStore>,
) -> ScenePart {
    let kind = MvpProfile::from_profile_id(profile_id);
    let (doc, family) = load_profile_doc(store, profile_id, false);
    let sim_role = doc.as_ref().and_then(sim_role_from_doc);
    let (size, terminals) = match kind {
        MvpProfile::UnoR3 | MvpProfile::Led | MvpProfile::Pushbutton => {
            (kind.size_grid(), kind.local_terminals())
        }
        MvpProfile::Generic => resolve_size_and_terminals(
            kind,
            doc.as_ref(),
            store,
            family,
            false,
        ),
    };
    ScenePart {
        id: instance_id.to_string(),
        kind: PartKind::Component,
        profile: kind,
        profile_id: profile_id.to_string(),
        sim_role,
        pose,
        size,
        terminals,
    }
}

fn load_profile_doc(
    store: Option<&ProfileStore>,
    profile_id: &str,
    is_board: bool,
) -> (Option<ProfileDocument>, &'static str) {
    let Some(store) = store else {
        return (None, if is_board { "boards" } else { "components" });
    };
    if is_board {
        (
            store.load_board(profile_id).ok(),
            "boards",
        )
    } else {
        (
            store.load_component(profile_id).ok(),
            "components",
        )
    }
}

fn resolve_size_and_terminals(
    kind: MvpProfile,
    doc: Option<&ProfileDocument>,
    store: Option<&ProfileStore>,
    family: &str,
    is_board: bool,
) -> (WorldSize, Vec<LocalTerminal>) {
    let size = size_from_profile(doc, store, family).unwrap_or_else(|| kind.size_grid());
    let terminals = if let Some(doc) = doc {
        terminals_from_profile(doc, size, is_board)
    } else {
        Vec::new()
    };
    (size, terminals)
}

/// Prefer authored SVG width/height (mm), then `[dims]`, else `None`.
fn size_from_profile(
    doc: Option<&ProfileDocument>,
    store: Option<&ProfileStore>,
    family: &str,
) -> Option<WorldSize> {
    let doc = doc?;
    let store = store?;
    if let Some(hook) = store.svg_hook_for(family, doc) {
        if hook.present {
            let path = store.repo_root().join(&hook.relative_path);
            if let Some((w_mm, h_mm)) = read_svg_size_mm(&path) {
                return Some(WorldSize::new(mm_to_grid(w_mm), mm_to_grid(h_mm)));
            }
        }
    }
    if let Some(dims) = doc.rest.get("dims").and_then(|v| v.as_table()) {
        let w = dims.get("w").and_then(toml_f64)?;
        let h = dims.get("h").and_then(toml_f64)?;
        // Round up to 2 mm quantum when dims are physical but not contract-aligned.
        let w_mm = (w / 2.0).ceil() * 2.0;
        let h_mm = (h / 2.0).ceil() * 2.0;
        return Some(WorldSize::new(mm_to_grid(w_mm), mm_to_grid(h_mm)));
    }
    None
}

fn read_svg_size_mm(path: &Path) -> Option<(f64, f64)> {
    let text = fs::read_to_string(path).ok()?;
    let w = attr_mm(&text, "width")?;
    let h = attr_mm(&text, "height")?;
    Some((w, h))
}

fn attr_mm(svg: &str, name: &str) -> Option<f64> {
    // Prefer early attributes on the root <svg …>.
    let head = svg.get(..2048).unwrap_or(svg);
    let key = format!("{name}=\"");
    let start = head.find(&key)? + key.len();
    let rest = &head[start..];
    let end = rest.find('"')?;
    let raw = rest[..end].trim();
    let num = raw.trim_end_matches("mm").trim();
    num.parse().ok()
}

fn toml_f64(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Float(f) => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        toml::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn terminals_from_profile(
    doc: &ProfileDocument,
    size: WorldSize,
    is_board: bool,
) -> Vec<LocalTerminal> {
    let mut ports: Vec<(String, TerminalRole)> = Vec::new();
    if let Some(arr) = doc.rest.get("ports").and_then(|v| v.as_array()) {
        for p in arr {
            let Some(t) = p.as_table() else { continue };
            let id = t
                .get("id")
                .or_else(|| t.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let role = port_role_from_table(t, &id);
            ports.push((id, role));
        }
    }
    if ports.is_empty() && is_board {
        if let Some(arr) = doc.rest.get("power_pins").and_then(|v| v.as_array()) {
            for p in arr {
                let Some(t) = p.as_table() else { continue };
                let name = t
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let role = if name.contains("GND") || name.ends_with("_M") || name == "PE" {
                    TerminalRole::Gnd
                } else {
                    TerminalRole::Power
                };
                ports.push((name, role));
            }
        }
        // Cap board GPIO clutter: first few digital + analog labels only.
        if let Some(arr) = doc.rest.get("gpio").and_then(|v| v.as_array()) {
            let mut n = 0;
            for p in arr {
                if n >= 8 {
                    break;
                }
                let Some(t) = p.as_table() else { continue };
                let label = t
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let port = gpio_label_to_port(label);
                if port.is_empty() {
                    continue;
                }
                let ty = t.get("type").and_then(|v| v.as_str()).unwrap_or("digital");
                let role = if ty == "analog" {
                    TerminalRole::Analog
                } else {
                    TerminalRole::Digital
                };
                ports.push((port, role));
                n += 1;
            }
        }
    }
    layout_ports_on_left_edge(ports, size)
}

fn gpio_label_to_port(label: &str) -> String {
    // "Ia.0 / HSC0" → "Ia.0"; "A0" → "A0"; "D13 / LED / SCK" → "D13"
    let primary = label.split('/').next().unwrap_or(label).trim();
    primary.to_string()
}

fn port_role_from_table(t: &toml::map::Map<String, toml::Value>, id: &str) -> TerminalRole {
    let ty = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match ty {
        "ground" => TerminalRole::Gnd,
        "power" => TerminalRole::Power,
        "analog" => TerminalRole::Analog,
        "digital" => TerminalRole::Digital,
        _ => {
            let id_u = id.to_ascii_uppercase();
            if id_u.contains("GND") || id_u == "K" || id_u == "COM" || id_u == "B" {
                TerminalRole::Gnd
            } else if id_u.contains("V") && !id_u.starts_with('A') {
                TerminalRole::Power
            } else if id_u.starts_with('A') && id_u.len() <= 3 {
                TerminalRole::Analog
            } else {
                TerminalRole::Digital
            }
        }
    }
}

fn layout_ports_on_left_edge(
    ports: Vec<(String, TerminalRole)>,
    size: WorldSize,
) -> Vec<LocalTerminal> {
    if ports.is_empty() {
        return Vec::new();
    }
    let max_y = (size.h.floor() as i32).max(1);
    let mut out = Vec::with_capacity(ports.len());
    for (i, (port, role)) in ports.into_iter().enumerate() {
        let y = ((i as i32 + 1).min(max_y)) as f64;
        // Prefer canvas.port_* when present would be nicer; left-edge is stable for D0b.
        let local = WorldPoint::new(0.0, y);
        out.push(LocalTerminal {
            label: port.clone(),
            port,
            local,
            role,
        });
    }
    out
}

// --- Phase A compatibility shims (tests / gradual migration) ---------------

/// @deprecated Phase A placeholder — prefer [`Scene`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Terminal {
    pub id: &'static str,
    pub label: &'static str,
    pub pos: WorldPoint,
    pub ground: bool,
}

/// @deprecated Phase A Uno placeholder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnoPlaceholder {
    pub origin: WorldPoint,
    pub size: WorldPoint,
    pub d13: Terminal,
    pub gnd: Terminal,
}

impl Default for UnoPlaceholder {
    fn default() -> Self {
        Self {
            origin: WorldPoint::new(1.0, 2.0),
            size: WorldPoint::new(7.0, 8.0),
            d13: Terminal {
                id: "board.D13",
                label: "D13",
                pos: WorldPoint::new(8.0, 3.0),
                ground: false,
            },
            gnd: Terminal {
                id: "board.GND",
                label: "GND",
                pos: WorldPoint::new(8.0, 7.0),
                ground: true,
            },
        }
    }
}

/// @deprecated Phase A LED placeholder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedPlaceholder {
    pub center: WorldPoint,
    pub radius_u: f64,
    pub anode: Terminal,
    pub cathode: Terminal,
}

impl Default for LedPlaceholder {
    fn default() -> Self {
        Self {
            center: WorldPoint::new(14.5, 3.5),
            radius_u: 1.35,
            anode: Terminal {
                id: "led1.A",
                label: "A",
                pos: WorldPoint::new(13.0, 3.0),
                ground: false,
            },
            cathode: Terminal {
                id: "led1.K",
                label: "K",
                pos: WorldPoint::new(13.0, 4.0),
                ground: true,
            },
        }
    }
}

/// @deprecated Phase A scene — prefer [`Scene::from_project`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseAScene {
    pub uno: UnoPlaceholder,
    pub led: LedPlaceholder,
}

impl Default for PhaseAScene {
    fn default() -> Self {
        let scene = Self {
            uno: UnoPlaceholder::default(),
            led: LedPlaceholder::default(),
        };
        scene.assert_terminal_contract();
        scene
    }
}

impl PhaseAScene {
    pub fn assert_terminal_contract(&self) {
        for t in [
            self.uno.d13,
            self.uno.gnd,
            self.led.anode,
            self.led.cathode,
        ] {
            assert!(
                is_terminal_intersection(t.pos.x) && is_terminal_intersection(t.pos.y),
                "{} ({},{}) not on full-module intersection",
                t.id,
                t.pos.x,
                t.pos.y
            );
        }
        let c = self.led.center;
        let sx = snap_half_module(c.x);
        let sy = snap_half_module(c.y);
        assert!(
            (sx - c.x).abs() < 1e-9 && (sy - c.y).abs() < 1e-9,
            "led center ({},{}) not on half-module",
            c.x,
            c.y
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{BoardInstance, ComponentInstance, Metadata, Program};

    fn sample_project() -> SimulationProject {
        SimulationProject {
            metadata: Metadata {
                asl_version: Some("0.0.1".into()),
                project_name: Some("b1".into()),
                editor: Some("test".into()),
                author: None,
                notes: None,
                expect_error: None,
            },
            board: BoardInstance {
                profile: "arduino-uno-r3".into(),
                instance_id: "board".into(),
                layout: LayoutPose::with_xy(1.0, 2.0),
            },
            components: vec![
                ComponentInstance {
                    instance_id: "led1".into(),
                    profile: "led".into(),
                    label: None,
                    params: Default::default(),
                    layout: LayoutPose::with_xy(13.25, 2.25),
                },
                ComponentInstance {
                    instance_id: "btn_start".into(),
                    profile: "cmd-pushbutton-no-start".into(),
                    label: None,
                    params: Default::default(),
                    layout: LayoutPose::with_xy(13.0, 6.0),
                },
            ],
            wiring: vec![],
            sources: vec![],
            asl: Program {
                metadata: Metadata::default(),
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

    #[test]
    fn scene_from_project_has_three_mvp_parts() {
        let scene = Scene::from_project(&sample_project());
        assert_eq!(scene.parts.len(), 3);
        scene.assert_terminal_contract();
        let uno = scene.part("board").unwrap();
        assert!(
            uno.terminals.iter().any(|t| t.port == "A0"),
            "Uno scene must expose A0 (D-comp-12)"
        );
    }

    #[test]
    fn pick_hits_led() {
        let scene = Scene::from_project(&sample_project());
        let hit = scene.pick(WorldPoint::new(14.0, 3.0));
        assert_eq!(hit.map(|p| p.id.as_str()), Some("led1"));
    }

    #[test]
    fn rotate_keeps_terminals_on_module() {
        let mut scene = Scene::from_project(&sample_project());
        let led = scene.part_mut("led1").unwrap();
        led.pose.rotate_cw_90();
        scene.assert_terminal_contract();
    }

    #[test]
    fn snap_origin_half_module() {
        let (x, y) = Scene::snap_origin(3.2, 4.7);
        assert_eq!(x, 3.0);
        assert_eq!(y, 4.5);
    }

    #[test]
    fn phase_a_placeholders_snap_contract() {
        PhaseAScene::default().assert_terminal_contract();
    }

    #[test]
    fn uno_size_matches_svg_contract() {
        let s = MvpProfile::UnoR3.size_grid();
        assert!((s.w - 19.0).abs() < 1e-9);
        assert!((s.h - 13.5).abs() < 1e-9);
    }

    #[test]
    fn unknown_profile_gets_size_and_terminals_from_store() {
        use dendri_ast::{
            BoardInstance, ComponentInstance, Metadata, Program, SimulationProject,
        };
        use dendri_profiles::ProfileStore;
        use std::path::PathBuf;

        let store = ProfileStore::from_repo_root(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        );
        let project = SimulationProject {
            metadata: Metadata {
                asl_version: Some("0.0.1".into()),
                project_name: Some("d0b".into()),
                editor: Some("test".into()),
                author: None,
                notes: None,
                expect_error: None,
            },
            board: BoardInstance {
                profile: "siemens-s7-1200-cpu-1214c".into(),
                instance_id: "board".into(),
                layout: LayoutPose::with_xy(1.0, 1.0),
            },
            components: vec![ComponentInstance {
                instance_id: "pilot1".into(),
                profile: "signal-pilot-lamp".into(),
                label: None,
                params: Default::default(),
                layout: LayoutPose::with_xy(30.0, 2.0),
            }],
            wiring: vec![],
            sources: vec![],
            asl: Program {
                metadata: Metadata::default(),
                hardware: vec![],
                data: vec![],
                setup: vec![],
                loop_body: vec![],
                functions: vec![],
                ladder: vec![],
                grafcet: vec![],
            },
        };
        let scene = Scene::from_project_with_store(&project, &store);
        assert_eq!(scene.parts.len(), 2);
        let s7 = scene.part("board").unwrap();
        assert_eq!(s7.profile, MvpProfile::Generic);
        // SVG 110×100 mm → 27.5 × 25 grid.
        assert!((s7.size.w - 27.5).abs() < 1e-9, "w={}", s7.size.w);
        assert!((s7.size.h - 25.0).abs() < 1e-9, "h={}", s7.size.h);
        assert!(!s7.terminals.is_empty(), "S7 should expose ports");
        let pilot = scene.part("pilot1").unwrap();
        assert_eq!(pilot.profile, MvpProfile::Generic);
        assert_eq!(pilot.sim_role, Some(SimRole::LedLoad));
        assert!(pilot.terminals.iter().any(|t| t.port == "A"));
        assert!(pilot.terminals.iter().any(|t| t.port == "K"));
        // D1 authored pilot SVG is 24×24 mm → 6×6 grid.
        assert!((pilot.size.w - 6.0).abs() < 1e-9, "pilot w={}", pilot.size.w);
        assert!((pilot.size.h - 6.0).abs() < 1e-9, "pilot h={}", pilot.size.h);
        scene.assert_terminal_contract();
    }
}
