//! Dynamic SVG layer extraction via **usvg** (Phase B0 foundation + C1 hooks).
//!
//! Camada B (plan §1 / C1): parse the asset once, look up nodes by stable `id`
//! (`led-body`, `actuator`, …), and prepare path geometry for reuse while
//! drawing. Do **not** re-parse / rebuild the full SVG per frame.

use usvg::{Node, Options, Tree};

/// One absolute path segment in SVG user units (portable; no iced dependency).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HookSeg {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Prepared hook geometry extracted once from usvg (reused every frame).
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedHook {
    pub id: String,
    /// SVG document width / height in user units (`Tree::size`).
    pub view_w: f32,
    pub view_h: f32,
    /// Absolute path segments (transformed), when the hook is a Path or Group of paths.
    pub segments: Vec<HookSeg>,
    /// Bounding circle in SVG user units (centre + radius) — used for LED glow /
    /// burned fills and button held indicator without inventing new SVG tags.
    pub circle: Option<HookCircle>,
}

/// Circle derived from a hook's absolute bounding box (inscribed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HookCircle {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
}

/// Parse SVG bytes into a usvg [`Tree`].
pub fn parse_svg(data: &[u8]) -> Result<Tree, usvg::Error> {
    Tree::from_data(data, &Options::default())
}

/// Parse an SVG string into a usvg [`Tree`].
pub fn parse_svg_str(text: &str) -> Result<Tree, usvg::Error> {
    Tree::from_str(text, &Options::default())
}

/// Look up a renderable node by element `id` (empty id → `None`).
pub fn node_by_id<'a>(tree: &'a Tree, id: &str) -> Option<&'a Node> {
    tree.node_by_id(id)
}

/// Whether the tree contains a node with the given `id`.
pub fn has_id(tree: &Tree, id: &str) -> bool {
    node_by_id(tree, id).is_some()
}

/// Extract hook geometry once for later Path drawing (C1).
pub fn prepare_hook(tree: &Tree, id: &str) -> Option<PreparedHook> {
    let node = node_by_id(tree, id)?;
    let size = tree.size();
    let view_w = size.width();
    let view_h = size.height();
    let mut segments = Vec::new();
    collect_segments(node, &mut segments);
    let circle = node.abs_bounding_box().map(|bb| {
        let cx = bb.x() + bb.width() * 0.5;
        let cy = bb.y() + bb.height() * 0.5;
        let r = bb.width().min(bb.height()) * 0.5;
        HookCircle { cx, cy, r }
    });
    Some(PreparedHook {
        id: id.to_string(),
        view_w,
        view_h,
        segments,
        circle,
    })
}

fn collect_segments(node: &Node, out: &mut Vec<HookSeg>) {
    match node {
        Node::Path(path) => {
            let data = path.data();
            // Prefer absolute path when available.
            let abs = path
                .data()
                .clone()
                .transform(path.abs_transform())
                .unwrap_or_else(|| data.clone());
            for seg in abs.segments() {
                out.push(match seg {
                    usvg::tiny_skia_path::PathSegment::MoveTo(p) => HookSeg::MoveTo(p.x, p.y),
                    usvg::tiny_skia_path::PathSegment::LineTo(p) => HookSeg::LineTo(p.x, p.y),
                    usvg::tiny_skia_path::PathSegment::QuadTo(p1, p2) => {
                        HookSeg::QuadTo(p1.x, p1.y, p2.x, p2.y)
                    }
                    usvg::tiny_skia_path::PathSegment::CubicTo(p1, p2, p3) => {
                        HookSeg::CubicTo(p1.x, p1.y, p2.x, p2.y, p3.x, p3.y)
                    }
                    usvg::tiny_skia_path::PathSegment::Close => HookSeg::Close,
                });
            }
        }
        Node::Group(group) => {
            for child in group.children() {
                collect_segments(child, out);
            }
        }
        Node::Image(_) | Node::Text(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{svg_contract, LoadedSvgs};
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn usvg_loads_mvp_svgs_and_finds_dynamic_ids() {
        let svgs = LoadedSvgs::load(repo()).expect("load MVP SVGs");
        let led = parse_svg_str(&svgs.led).expect("parse led.svg");
        for id in svg_contract::required_ids::LED {
            assert!(has_id(&led, id), "led missing id={id}");
        }
        let button = parse_svg_str(&svgs.button).expect("parse button.svg");
        for id in svg_contract::required_ids::BUTTON {
            assert!(has_id(&button, id), "button missing id={id}");
        }
        let board = parse_svg_str(&svgs.board).expect("parse uno.svg");
        for id in svg_contract::required_ids::UNO {
            assert!(has_id(&board, id), "uno missing id={id}");
        }
    }

    #[test]
    fn prepare_hook_led_body_and_actuator_once() {
        let svgs = LoadedSvgs::load(repo()).expect("load");
        let led = parse_svg_str(&svgs.led).unwrap();
        let body = prepare_hook(&led, "led-body").expect("led-body");
        assert!(body.circle.is_some());
        assert!(!body.segments.is_empty(), "led-body should yield path segments");
        assert!(body.view_w > 0.0 && body.view_h > 0.0);

        let btn = parse_svg_str(&svgs.button).unwrap();
        let act = prepare_hook(&btn, "actuator").expect("actuator");
        assert!(act.circle.is_some());
        assert!(!act.segments.is_empty());
    }
}
