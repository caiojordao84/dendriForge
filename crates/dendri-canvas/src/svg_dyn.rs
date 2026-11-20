//! Dynamic SVG layer extraction via **usvg** (Phase B0 foundation).
//!
//! Camada B (plan §1): parse the asset once, look up nodes by stable `id`
//! (`led-body`, `actuator`, …). Conversion to iced `canvas::Path` via
//! `iced::widget::canvas::path::lyon_path` lands with the editor draw path
//! (B1+); this module only proves parse + id filter.

use usvg::{Node, Options, Tree};

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
}
