//! Wire geometry for Phase B2: resolve endpoints, route polylines, hit-test.
//!
//! Routing uses [`crate::routing`] (arqlet channel-based). Manual waypoints
//! force Manhattan segments through intermediate grid points.

use dendri_ast::Wire;

use crate::camera::WorldPoint;
use crate::grid::snap_half_module;
use crate::routing::{manhattan_via, route_pin_to_pin};
use crate::world::{Scene, TerminalRole};

/// Screen-space snap radius for pin lock (plan §4).
pub const PIN_SNAP_SCREEN_PX: f64 = 6.0;

/// Fillet radius in **screen pixels** (plan §5).
pub const WIRE_FILLET_SCREEN_PX: f32 = 4.0;

/// Dual-stroke widths in screen pixels.
pub const WIRE_BASE_WIDTH_PX: f32 = 5.0;
pub const WIRE_COLOR_WIDTH_PX: f32 = 2.5;

/// Hit-test half-width for selecting a wire (screen px).
pub const WIRE_HIT_SCREEN_PX: f64 = 6.0;

/// A resolved terminal reference on the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalRef {
    pub part_id: String,
    pub port: String,
    pub endpoint: String,
    pub world: WorldPoint,
    pub role: TerminalRole,
}

/// One routed wire ready to draw / pick.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutedWire {
    pub index: usize,
    pub from: String,
    pub to: String,
    pub signal: Option<String>,
    pub waypoints: Vec<[f64; 2]>,
    /// Orthogonal polyline in grid units (includes endpoints).
    pub points: Vec<WorldPoint>,
    pub from_role: TerminalRole,
    pub to_role: TerminalRole,
    /// Sequential conductor index among non-power wires (for palette cycling).
    pub sequence: usize,
}

impl Scene {
    /// All terminals with world positions.
    pub fn terminals(&self) -> Vec<TerminalRef> {
        let mut out = Vec::new();
        for part in &self.parts {
            for t in &part.terminals {
                out.push(TerminalRef {
                    part_id: part.id.clone(),
                    port: t.port.clone(),
                    endpoint: part.endpoint_id(&t.port),
                    world: part.terminal_world(t),
                    role: t.role,
                });
            }
        }
        out
    }

    pub fn find_endpoint(&self, endpoint: &str) -> Option<TerminalRef> {
        self.terminals()
            .into_iter()
            .find(|t| t.endpoint.eq_ignore_ascii_case(endpoint))
    }

    /// Nearest terminal within `max_screen_px` of a view-space cursor.
    pub fn nearest_terminal_screen(
        &self,
        camera: &crate::Camera,
        view_x: f64,
        view_y: f64,
        max_screen_px: f64,
    ) -> Option<TerminalRef> {
        let mut best: Option<(f64, TerminalRef)> = None;
        for t in self.terminals() {
            let (vx, vy) = camera.world_to_view(t.world);
            let d = ((vx - view_x).powi(2) + (vy - view_y).powi(2)).sqrt();
            if d <= max_screen_px {
                if best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
                    best = Some((d, t));
                }
            }
        }
        best.map(|(_, t)| t)
    }
}

/// Infer electrical role for coloring from signal + pin roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireColorKind {
    Gnd,
    Vcc,
    /// Inherit from `from` pin; `sequence` for sequential digital conductors.
    Signal { sequence: usize, from: TerminalRole },
}

pub fn wire_color_kind(
    signal: Option<&str>,
    from_role: TerminalRole,
    to_role: TerminalRole,
    sequence: usize,
) -> WireColorKind {
    let sig = signal.unwrap_or("").to_ascii_lowercase();
    if sig.contains("gnd") || from_role == TerminalRole::Gnd || to_role == TerminalRole::Gnd {
        return WireColorKind::Gnd;
    }
    if sig.contains("vcc")
        || sig.contains("5v")
        || sig.contains("3v")
        || from_role == TerminalRole::Power
        || to_role == TerminalRole::Power
    {
        return WireColorKind::Vcc;
    }
    WireColorKind::Signal {
        sequence,
        from: from_role,
    }
}

/// Route all project wires against the scene.
pub fn route_wires(scene: &Scene, wiring: &[Wire]) -> Vec<RoutedWire> {
    let mut signal_seq = 0usize;
    let mut out = Vec::with_capacity(wiring.len());
    for (index, w) in wiring.iter().enumerate() {
        let from_t = scene.find_endpoint(&w.from);
        let to_t = scene.find_endpoint(&w.to);
        let from_role = from_t
            .as_ref()
            .map(|t| t.role)
            .unwrap_or(TerminalRole::Digital);
        let to_role = to_t
            .as_ref()
            .map(|t| t.role)
            .unwrap_or(TerminalRole::Digital);
        let kind = wire_color_kind(w.signal.as_deref(), from_role, to_role, signal_seq);
        let sequence = match kind {
            WireColorKind::Signal { sequence, .. } => {
                signal_seq = signal_seq.saturating_add(1);
                sequence
            }
            _ => 0,
        };

        let points = match (from_t.as_ref(), to_t.as_ref()) {
            (Some(a), Some(b)) => {
                if w.waypoints.is_empty() {
                    route_pin_to_pin(scene, a.world, &a.part_id, b.world, &b.part_id)
                } else {
                    let mut anchors = Vec::with_capacity(w.waypoints.len() + 2);
                    anchors.push(a.world);
                    for wp in &w.waypoints {
                        anchors.push(WorldPoint::new(
                            snap_half_module(wp[0]),
                            snap_half_module(wp[1]),
                        ));
                    }
                    anchors.push(b.world);
                    manhattan_via(&anchors)
                }
            }
            _ => Vec::new(),
        };

        out.push(RoutedWire {
            index,
            from: w.from.clone(),
            to: w.to.clone(),
            signal: w.signal.clone(),
            waypoints: w.waypoints.clone(),
            points,
            from_role,
            to_role,
            sequence,
        });
    }
    out
}

/// Distance from point P to segment AB in world units.
fn dist_point_segment(p: WorldPoint, a: WorldPoint, b: WorldPoint) -> (f64, WorldPoint, f64) {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    let t = if len2 < 1e-18 {
        0.0
    } else {
        (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0)
    };
    let closest = WorldPoint::new(a.x + t * abx, a.y + t * aby);
    let d = ((p.x - closest.x).powi(2) + (p.y - closest.y).powi(2)).sqrt();
    (d, closest, t)
}

/// Pick nearest wire under a view-space cursor within `max_screen_px`.
pub fn pick_wire(
    camera: &crate::Camera,
    wires: &[RoutedWire],
    view_x: f64,
    view_y: f64,
    max_screen_px: f64,
) -> Option<(usize, WorldPoint, usize)> {
    let world = camera.view_to_world(view_x, view_y);
    let max_world = max_screen_px / camera.scale().max(1e-9);
    let mut best: Option<(f64, usize, WorldPoint, usize)> = None;
    for w in wires {
        if w.points.len() < 2 {
            continue;
        }
        for seg in 0..w.points.len() - 1 {
            let (d, closest, _) = dist_point_segment(world, w.points[seg], w.points[seg + 1]);
            if d <= max_world {
                if best.as_ref().map(|(bd, ..)| d < *bd).unwrap_or(true) {
                    best = Some((d, w.index, closest, seg));
                }
            }
        }
    }
    best.map(|(_, idx, closest, seg)| (idx, closest, seg))
}

/// Insert a Manhattan waypoint: snap click to half-module, keep orthogonal path.
pub fn waypoint_from_click(click: WorldPoint) -> [f64; 2] {
    [snap_half_module(click.x), snap_half_module(click.y)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Scene;
    use dendri_ast::{
        BoardInstance, ComponentInstance, LayoutPose, Metadata, Program, SimulationProject,
    };

    fn sample() -> (Scene, Vec<Wire>) {
        let project = SimulationProject {
            metadata: Metadata::default(),
            board: BoardInstance {
                profile: "arduino-uno-r3".into(),
                instance_id: "board".into(),
                layout: LayoutPose::with_xy(1.0, 2.0),
            },
            components: vec![ComponentInstance {
                instance_id: "led1".into(),
                profile: "led".into(),
                label: None,
                params: Default::default(),
                layout: LayoutPose::with_xy(13.25, 2.25),
            }],
            wiring: vec![
                Wire::new("board.D13", "led1.A").with_signal("digital"),
                Wire::new("board.GND", "led1.K").with_signal("gnd"),
            ],
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
        let scene = Scene::from_project(&project);
        (scene, project.wiring)
    }

    #[test]
    fn routes_c01_style_wires() {
        let (scene, wiring) = sample();
        let routed = route_wires(&scene, &wiring);
        assert_eq!(routed.len(), 2);
        assert!(routed[0].points.len() >= 2);
        assert!(routed[1].points.len() >= 2);
        // Orthogonal: consecutive points share x or y.
        for w in &routed {
            for win in w.points.windows(2) {
                let a = win[0];
                let b = win[1];
                assert!(
                    (a.x - b.x).abs() < 1e-9 || (a.y - b.y).abs() < 1e-9,
                    "non-orthogonal {:?} → {:?}",
                    a,
                    b
                );
            }
        }
    }

    #[test]
    fn waypoints_force_via() {
        let (scene, mut wiring) = sample();
        wiring[0].waypoints = vec![[10.0, 3.0]];
        let routed = route_wires(&scene, &wiring);
        let pts = &routed[0].points;
        assert!(
            pts.iter()
                .any(|p| (p.x - 10.0).abs() < 1e-6 && (p.y - 3.0).abs() < 1e-6),
            "expected via at (10,3), got {:?}",
            pts
        );
    }
}
