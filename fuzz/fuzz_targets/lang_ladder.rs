#![no_main]

use dendri_frontend_core::{BoardContext, LanguageFrontend};
use dendri_lang_ladder::LadderFrontend;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let fe = LadderFrontend::new();
        let ctx = BoardContext::plc_1200_open_1214("fuzz_ladder");
        if let Ok(parsed) = fe.parse(text, &ctx) {
            let _ = fe.emit(&parsed.value, &ctx);
        }
    }
});
