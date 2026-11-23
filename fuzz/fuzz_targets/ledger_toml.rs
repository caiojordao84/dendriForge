#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = dendri_cli::ledger::parse_milestone_str(text);
        let _ = dendri_cli::ledger::parse_deferrals_str(text);
        let _ = dendri_cli::provenance::parse_provenance_str(text);
    }
});
