#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(proj) = dendri_project::load_project_str(text, None) {
            let _ = dendri_project::to_toml_string(&proj);
        }
    }
});
