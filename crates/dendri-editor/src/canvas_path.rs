//! Lyon path access via iced canvas (Phase B0).
//!
//! Dynamic SVG geometry will convert usvg segments through
//! [`iced::widget::canvas::path::lyon_path`] into `canvas::Path` (plan §1 Camada B).
//! This module only proves the iced re-export is on the compile path.

/// Touch iced’s documented lyon path re-export so the dep path stays live.
#[allow(dead_code)] // exercised by unit test; used by B1 Path conversion
pub fn lyon_path_available() -> bool {
    let _ = std::any::type_name::<iced::widget::canvas::path::lyon_path::Path>();
    let _ = std::any::type_name::<iced::widget::canvas::path::Builder>();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iced_exposes_lyon_path() {
        assert!(lyon_path_available());
    }
}
