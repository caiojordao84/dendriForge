//! Phase B1 scene graph: boards/components from the project with layout poses.
//!
//! Bodies use SVG sizes (mm → grid). Terminals sit on full-module intersections
//! after transform (snapped). Drag snaps origins to the half-module.

use dendri_ast::{LayoutPose, SimulationProject};

use crate::camera::{WorldPoint, WorldRect, WorldSize};
use crate::grid::{is_terminal_intersection, mm_to_grid, snap_half_module, snap_terminal};

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

/// MVP profiles supported in Phase B1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MvpProfile {
    UnoR3,
    Led,
    Pushbutton,
}

impl MvpProfile {
    pub fn from_profile_id(profile: &str) -> Option<Self> {
        match profile {
            "arduino-uno-r3" | "uno" => Some(Self::UnoR3),
            "led" => Some(Self::Led),
            "cmd-pushbutton-no-start" => Some(Self::Pushbutton),
            _ => None,
        }
    }

    /// Outer SVG size in millimetres (contract: multiples of 2 mm).
    pub fn size_mm(self) -> (f64, f64) {
        match self {
            Self::UnoR3 => (76.0, 54.0),
            Self::Led => (10.0, 10.0),
            Self::Pushbutton => (24.0, 24.0),
        }
    }

    pub fn size_grid(self) -> WorldSize {
        let (w, h) = self.size_mm();
        WorldSize::new(mm_to_grid(w), mm_to_grid(h))
    }

    /// Local terminal offsets in grid units (pre-transform).
    pub fn local_terminals(self) -> Vec<LocalTerminal> {
        match self {
            Self::UnoR3 => {
                // Right-edge digital header, module-aligned.
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
    /// Build from project; unknown profiles are skipped (B1 = Uno + LED + button).
    pub fn from_project(project: &SimulationProject) -> Self {
        let mut parts = Vec::new();
        if let Some(profile) = MvpProfile::from_profile_id(&project.board.profile) {
            parts.push(ScenePart {
                id: project.board.instance_id.clone(),
                kind: PartKind::Board,
                profile,
                profile_id: project.board.profile.clone(),
                pose: project.board.layout,
                size: profile.size_grid(),
                terminals: profile.local_terminals(),
            });
        }
        for c in &project.components {
            let Some(profile) = MvpProfile::from_profile_id(&c.profile) else {
                continue;
            };
            parts.push(ScenePart {
                id: c.instance_id.clone(),
                kind: PartKind::Component,
                profile,
                profile_id: c.profile.clone(),
                pose: c.layout,
                size: profile.size_grid(),
                terminals: profile.local_terminals(),
            });
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
}
