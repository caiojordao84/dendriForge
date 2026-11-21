//! Lyon / iced path access + usvg hook → `canvas::Path` (Phase B0 / C1).
//!
//! Prepared hook geometry from [`dendri_canvas::svg_dyn`] is converted once into
//! drawable iced paths and reused every frame — no per-frame SVG parse/rebuild.

use dendri_canvas::{HookSeg, PreparedHook};
use iced::{
    widget::canvas::{path::Builder, Path},
    Point,
};

/// Touch iced’s documented lyon path re-export so the dep path stays live.
#[allow(dead_code)]
pub fn lyon_path_available() -> bool {
    let _ = std::any::type_name::<iced::widget::canvas::path::lyon_path::Path>();
    let _ = std::any::type_name::<iced::widget::canvas::path::Builder>();
    true
}

/// Build an iced [`Path`] from prepared usvg hook segments (SVG user units).
///
/// Caller applies the same pose transform used for the static SVG body
/// (translate to part centre, mirror, rotate, then scale viewBox → part size).
pub fn hook_path_svg_units(hook: &PreparedHook) -> Path {
    if hook.segments.is_empty() {
        if let Some(c) = hook.circle {
            return Path::circle(Point::new(c.cx, c.cy), c.r);
        }
        return Path::new(|_| {});
    }
    Path::new(|b| {
        append_hook_segments(b, &hook.segments);
    })
}

/// Circle path from prepared hook (preferred for LED glow / burned fills).
pub fn hook_circle_path(hook: &PreparedHook) -> Option<Path> {
    let c = hook.circle?;
    Some(Path::circle(Point::new(c.cx, c.cy), c.r))
}

fn append_hook_segments(b: &mut Builder, segments: &[HookSeg]) {
    for seg in segments {
        match *seg {
            HookSeg::MoveTo(x, y) => b.move_to(Point::new(x, y)),
            HookSeg::LineTo(x, y) => b.line_to(Point::new(x, y)),
            HookSeg::QuadTo(cx, cy, x, y) => {
                b.quadratic_curve_to(Point::new(cx, cy), Point::new(x, y));
            }
            HookSeg::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                b.bezier_curve_to(
                    Point::new(c1x, c1y),
                    Point::new(c2x, c2y),
                    Point::new(x, y),
                );
            }
            HookSeg::Close => b.close(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_canvas::{parse_svg_str, prepare_hook, LoadedSvgs};
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn iced_exposes_lyon_path() {
        assert!(lyon_path_available());
    }

    #[test]
    fn hook_path_from_led_body_without_reparse() {
        let svgs = LoadedSvgs::load(repo()).unwrap();
        let tree = parse_svg_str(&svgs.led).unwrap();
        let hook = prepare_hook(&tree, "led-body").unwrap();
        let path = hook_path_svg_units(&hook);
        let _ = path;
        assert!(hook.circle.is_some());
    }
}
