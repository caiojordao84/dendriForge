//! Orthogonal wire routing via **arqlet-routing** (Phase B0 foundation).
//!
//! Phase B2 will call [`arqlet_routing::channel::route_channels`] with real
//! component AABBs and pin stubs. This module exposes default [`Params`] and a
//! minimal channel-route smoke so the crate dependency is not dead.

use arqlet_model::{
    DirectionConstraint, Edge, Graph, Node, Orientation, Point, Rect, Route,
};
use arqlet_routing::channel::{route_channels, ChannelEnd};
use arqlet_routing::Params;
use std::collections::BTreeMap;

/// Default arqlet routing parameters (prototype defaults: track 6, rail 12).
pub fn default_params() -> Params {
    Params::default()
}

/// Route a single orthogonal channel between two stubs (B0 API smoke / B2 seed).
///
/// `source` / `target` node ids must exist in `graph` with matching `rects`.
/// `start`→`depart` and `arrive`→`end` are port → stub pairs (axis-aligned).
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
