#![no_main]

use dendri_frontend_core::{BoardContext, LanguageFrontend};
use dendri_lang_cpp::CppFrontend;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let fe = CppFrontend::new();
        let ctx = BoardContext::arduino_uno_r3("fuzz_uno");
        if let Ok(parsed) = fe.parse(text, &ctx) {
            let _ = fe.emit(&parsed.value, &ctx);
        }
    }
});
