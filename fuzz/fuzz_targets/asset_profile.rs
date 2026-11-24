#![no_main]

use dendri_assets::{
    BoardProfile, CANONICAL_GRID_PX, ComponentProfile, ReviewSignOff,
    canonicalize_and_sanitize_svg, parse_and_validate_svg,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = toml::from_str::<BoardProfile>(text);
        let _ = toml::from_str::<ComponentProfile>(text);
        let _ = toml::from_str::<ReviewSignOff>(text);
        let _ = parse_and_validate_svg(text, CANONICAL_GRID_PX);
        let _ = canonicalize_and_sanitize_svg(text, CANONICAL_GRID_PX);
    }
});
