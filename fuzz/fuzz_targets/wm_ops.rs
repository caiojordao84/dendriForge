#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (_shell, report) = dendri_wm::fuzz_operation_sequence(data);
    assert!(
        report.violations.is_empty(),
        "dendri-wm invariant violation: {:?}",
        report.violations
    );
});
