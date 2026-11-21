//! Orthogonal wire routing via **arqlet-routing** (Phase B0 + B2).
//!
//! B2 calls [`arqlet_routing::channel::route_channels`] with scene AABBs and
//! pin stubs. When the channel router returns an empty / unusable polyline,
//! a thin **Manhattan L-shape fallback** is used (documented below) so wiring
//! always produces a drawable orthogonal path.
//!
//! ## Fallback policy
//! `route_channels` expects port stubs just outside node bodies. If stubs or
//! graph construction cannot produce ≥2 points (e.g. coincident pins), we fall
//! back to [`manhattan_l`] — a single L (or straight) between endpoints. This
//! is intentionally thin: arqlet remains the primary router for multi-wire
//! congestion; the fallback only guarantees a path.

use arqlet_model::{
    DirectionConstraint, Edge, Graph, Node, Orientation, Point, Rect, Route,
};
use arqlet_routing::channel::{route_channels, ChannelEnd};
use arqlet_routing::Params;
use std::collections::BTreeMap;

use crate::camera::WorldPoint;
use crate::world::Scene;

/// Default arqlet routing parameters (prototype defaults: track 6, rail 12).
pub fn default_params() -> Params {
    Params::default()
}

/// Route a single orthogonal channel between two stubs (B0 API smoke / B2 seed).
pub fn route_one_channel(
    graph: &Graph,
    rects: &BTreeMap<String, Rect>,
    end: ChannelEnd,
    margin: f64,
    track: f64,
) -> Vec<Route> {
    route_channels(graph, rects, &[end], margin, track)
}

/// Tiny two-node graph used by the compile-proof unit test.
pub fn smoke_two_node_graph() -> (Graph, BTreeMap<String, Rect>, ChannelEnd) {
    let node = |id: &str| Node {
        id: id.into(),
        label: id.into(),
        kind: "symbol".into(),
        parent_id: None,
        shape: Default::default(),
    };
    let graph = Graph {
        default_direction: DirectionConstraint::Fixed(Orientation::Lr),
        container_directions: BTreeMap::new(),
        default_strategy: Default::default(),
        container_strategies: Default::default(),
        default_routing: Default::default(),
        container_routing: Default::default(),
        ports: vec![],
        edge_ports: vec![],
        nodes: vec![node("a"), node("b")],
        edges: vec![Edge {
            id: "e0".into(),
            kind: "wire".into(),
            source: "a".into(),
            target: Some("b".into()),
            weight: None,
        }],
        buses: Vec::new(),
    };
    let mut rects = BTreeMap::new();
    rects.insert(
        "a".into(),
        Rect {
            x: 0.,
            y: 0.,
            width: 40.,
            height: 20.,
        },
    );
    rects.insert(
        "b".into(),
        Rect {
            x: 160.,
            y: 0.,
            width: 40.,
            height: 20.,
        },
    );
    let end = ChannelEnd {
        edge_id: "e0".into(),
        source: "a".into(),
        target: "b".into(),
        start: Point { x: 40., y: 10. },
        depart: Point { x: 50., y: 10. },
        arrive: Point { x: 150., y: 10. },
        end: Point { x: 160., y: 10. },
    };
    (graph, rects, end)
}

/// Straight or single-bend Manhattan between two points.
pub fn manhattan_l(a: WorldPoint, b: WorldPoint) -> Vec<WorldPoint> {
    if (a.x - b.x).abs() < 1e-9 || (a.y - b.y).abs() < 1e-9 {
        return vec![a, b];
    }
    // Prefer horizontal-then-vertical (schematic convention).
    vec![a, WorldPoint::new(b.x, a.y), b]
}

/// Chain Manhattan L-segments through a list of anchors (endpoints + waypoints).
pub fn manhattan_via(anchors: &[WorldPoint]) -> Vec<WorldPoint> {
    if anchors.is_empty() {
        return Vec::new();
    }
    if anchors.len() == 1 {
        return vec![anchors[0]];
    }
    let mut out = Vec::new();
    for win in anchors.windows(2) {
        let seg = manhattan_l(win[0], win[1]);
        if out.is_empty() {
            out.extend(seg);
        } else {
            // Skip duplicate joint.
            out.extend(seg.into_iter().skip(1));
        }
    }
    simplify_orthogonal(&mut out);
    out
}

fn simplify_orthogonal(pts: &mut Vec<WorldPoint>) {
    if pts.len() < 3 {
        return;
    }
    let mut i = 1;
    while i + 1 < pts.len() {
        let a = pts[i - 1];
        let b = pts[i];
        let c = pts[i + 1];
        let colinear_h = (a.y - b.y).abs() < 1e-9 && (b.y - c.y).abs() < 1e-9;
        let colinear_v = (a.x - b.x).abs() < 1e-9 && (b.x - c.x).abs() < 1e-9;
        if colinear_h || colinear_v {
            pts.remove(i);
        } else {
            i += 1;
        }
    }
}

fn outward_stub(pin: WorldPoint, aabb_min: WorldPoint, aabb_max: WorldPoint, stub: f64) -> WorldPoint {
    let dl = (pin.x - aabb_min.x).abs();
    let dr = (aabb_max.x - pin.x).abs();
    let dt = (pin.y - aabb_min.y).abs();
    let db = (aabb_max.y - pin.y).abs();
    let m = dl.min(dr).min(dt).min(db);
    if (m - dl).abs() < 1e-9 {
        WorldPoint::new(pin.x - stub, pin.y)
    } else if (m - dr).abs() < 1e-9 {
        WorldPoint::new(pin.x + stub, pin.y)
    } else if (m - dt).abs() < 1e-9 {
        WorldPoint::new(pin.x, pin.y - stub)
    } else {
        WorldPoint::new(pin.x, pin.y + stub)
    }
}

fn scene_rects(scene: &Scene) -> BTreeMap<String, Rect> {
    let mut rects = BTreeMap::new();
    for part in &scene.parts {
        let aabb = part.world_aabb();
        rects.insert(
            part.id.clone(),
            Rect {
                x: aabb.min.x,
                y: aabb.min.y,
                width: (aabb.max.x - aabb.min.x).max(0.01),
                height: (aabb.max.y - aabb.min.y).max(0.01),
            },
        );
    }
    rects
}

fn graph_with_obstacles(scene: &Scene, source: &str, target: &str, edge_id: &str) -> Graph {
    let node = |id: &str| Node {
        id: id.into(),
        label: id.into(),
        kind: "symbol".into(),
        parent_id: None,
        shape: Default::default(),
    };
    let mut nodes: Vec<Node> = scene.parts.iter().map(|p| node(&p.id)).collect();
    // Ensure source/target exist even if ids are unusual.
    if !nodes.iter().any(|n| n.id == source) {
        nodes.push(node(source));
    }
    if !nodes.iter().any(|n| n.id == target) {
        nodes.push(node(target));
    }
    Graph {
        default_direction: DirectionConstraint::Fixed(Orientation::Lr),
        container_directions: BTreeMap::new(),
        default_strategy: Default::default(),
        container_strategies: Default::default(),
        default_routing: Default::default(),
        container_routing: Default::default(),
        ports: vec![],
        edge_ports: vec![],
        nodes,
        edges: vec![Edge {
            id: edge_id.into(),
            kind: "wire".into(),
            source: source.into(),
            target: Some(target.into()),
            weight: None,
        }],
        buses: Vec::new(),
    }
}

fn arqlet_points_to_world(route: &Route) -> Vec<WorldPoint> {
    route
        .points
        .iter()
        .map(|p| WorldPoint::new(p.x, p.y))
        .collect()
}

/// Route pin→pin with arqlet channel routing; Manhattan L fallback if needed.
///
/// Coordinates are **grid units** (same as the scene). Stub / margin / track
/// are expressed in those units (defaults: stub 0.5 u, margin 0.35 u, track 0.25 u).
pub fn route_pin_to_pin(
    scene: &Scene,
    from: WorldPoint,
    from_part: &str,
    to: WorldPoint,
    to_part: &str,
) -> Vec<WorldPoint> {
    const STUB: f64 = 0.5;
    const MARGIN: f64 = 0.35;
    const TRACK: f64 = 0.25;

    if (from.x - to.x).abs() < 1e-9 && (from.y - to.y).abs() < 1e-9 {
        return vec![from, to];
    }

    let rects = scene_rects(scene);
    let from_aabb = scene
        .part(from_part)
        .map(|p| p.world_aabb())
        .unwrap_or_else(|| crate::camera::WorldRect {
            min: WorldPoint::new(from.x - 0.5, from.y - 0.5),
            max: WorldPoint::new(from.x + 0.5, from.y + 0.5),
        });
    let to_aabb = scene
        .part(to_part)
        .map(|p| p.world_aabb())
        .unwrap_or_else(|| crate::camera::WorldRect {
            min: WorldPoint::new(to.x - 0.5, to.y - 0.5),
            max: WorldPoint::new(to.x + 0.5, to.y + 0.5),
        });

    let depart = outward_stub(from, from_aabb.min, from_aabb.max, STUB);
    let arrive = outward_stub(to, to_aabb.min, to_aabb.max, STUB);

    let edge_id = format!("{from_part}->{to_part}");
    let graph = graph_with_obstacles(scene, from_part, to_part, &edge_id);
    let end = ChannelEnd {
        edge_id: edge_id.clone(),
        source: from_part.into(),
        target: to_part.into(),
        start: Point { x: from.x, y: from.y },
        depart: Point {
            x: depart.x,
            y: depart.y,
        },
        arrive: Point {
            x: arrive.x,
            y: arrive.y,
        },
        end: Point { x: to.x, y: to.y },
    };

    let routes = route_channels(&graph, &rects, &[end], MARGIN, TRACK);
    if let Some(route) = routes.first() {
        let mut pts = arqlet_points_to_world(route);
        // Ensure true pin endpoints bookend the route.
        if pts.first().map(|p| (p.x - from.x).abs() > 1e-6 || (p.y - from.y).abs() > 1e-6).unwrap_or(true)
        {
            pts.insert(0, from);
        }
        if pts.last().map(|p| (p.x - to.x).abs() > 1e-6 || (p.y - to.y).abs() > 1e-6).unwrap_or(true)
        {
            pts.push(to);
        }
        simplify_orthogonal(&mut pts);
        if pts.len() >= 2 {
            return pts;
        }
    }

    // Documented thin fallback — see module docs.
    manhattan_l(from, to)
}

/// Rubber-band preview from a pinned start to a free (or snapped) end.
pub fn rubber_band(from: WorldPoint, to: WorldPoint) -> Vec<WorldPoint> {
    manhattan_l(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Scene;
    use dendri_ast::{
        BoardInstance, ComponentInstance, LayoutPose, Metadata, Program, SimulationProject,
    };

    #[test]
    fn arqlet_default_params_and_channel_route() {
        let p = default_params();
        assert_eq!(p.track, 6.0);
        assert_eq!(p.rail_offset, 12.0);

        let (graph, rects, end) = smoke_two_node_graph();
        let routes = route_one_channel(&graph, &rects, end, 8.0, p.track);
        assert_eq!(routes.len(), 1);
        assert!(
            routes[0].points.len() >= 2,
            "expected a polyline, got {:?}",
            routes[0].points
        );
    }

    #[test]
    fn pin_to_pin_is_orthogonal() {
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
        let scene = Scene::from_project(&project);
        let a = scene.find_endpoint("board.D13").unwrap();
        let b = scene.find_endpoint("led1.A").unwrap();
        let pts = route_pin_to_pin(&scene, a.world, "board", b.world, "led1");
        assert!(pts.len() >= 2);
        for win in pts.windows(2) {
            assert!(
                (win[0].x - win[1].x).abs() < 1e-9 || (win[0].y - win[1].y).abs() < 1e-9
            );
        }
    }

    #[test]
    fn manhattan_via_visits_waypoint() {
        let pts = manhattan_via(&[
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(4.0, 2.0),
            WorldPoint::new(8.0, 2.0),
        ]);
        assert!(pts.iter().any(|p| (p.x - 4.0).abs() < 1e-9 && (p.y - 2.0).abs() < 1e-9));
    }
}
