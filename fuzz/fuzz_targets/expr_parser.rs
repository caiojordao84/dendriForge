#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(expr) = dendri_expr::parse_expr(text) {
            let printed = dendri_expr::print_expr(&expr);
            let _ = dendri_expr::parse_expr(&printed);
        }
        let _ = dendri_expr::parse_lvalue(text);
    }
});
