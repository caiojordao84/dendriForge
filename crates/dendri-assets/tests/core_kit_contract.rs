//! Phase 4 Asset Contract Integration Tests (`03-asset-and-profile-contract.md` §1.1, §4, §7).
//!
//! Verifies:
//! 1. All 10 live certified Core Kit assets under `assets/{boards,components}` pass all 13 rules.
//! 2. `AssetCatalog::load_validated` acts as a live `dendri-validate::ProfileCatalog` for real v2 projects
//!    (including `arduino-uno-r3`, `plc-1200-open-1214`, `DEF-002` channel bands, `DEF-009` ref_designators).
//! 3. Tampering with `physical.svg`, `schematic.svg`, or `profile.toml` invalidates `review.toml` (Rule 12),
//!    and explicit `approve_asset_dir` cleanly re-stamps it.
//! 4. Unregistered physics models, broken composite sub-circuit wires, unknown standards,
//!    and missing `THIRD_PARTY_NOTICES` / `ml_restricted = true` exclusions behave deterministically.

use dendri_assets::{
    AssetCatalog, AssetCheckError, approve_asset_dir, is_valid_ref_designator_prefix,
    validate_assets_dir,
};
use dendri_project::load_project_str;
use dendri_validate::{ValidateOptions, validate_project};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn find_workspace_root() -> Option<PathBuf> {
    let mut cur = std::env::current_dir().ok()?;
    loop {
        if cur.join("assets").join("standards.toml").is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}

#[test]
fn live_phase4_core_kit_passes_all_13_rules_and_exposes_certified_catalog() {
    let root = find_workspace_root().unwrap();
    let assets_dir = root.join("assets");

    let report = validate_assets_dir(&root, &assets_dir).unwrap();
    assert_eq!(report.boards_checked, 2);
    assert_eq!(report.components_checked, 14);
    assert_eq!(report.standards_loaded, 14);
    assert!(report.third_party_assets.is_empty());
    assert!(report.ml_restricted_assets.is_empty());

    let catalog = AssetCatalog::load_validated(&root, &assets_dir).unwrap();
    assert!(catalog.boards.contains_key("arduino-uno-r3"));
    assert!(catalog.boards.contains_key("plc-1200-open-1214"));
    for comp_id in [
        "cmd-pushbutton-nc-stop",
        "cmd-pushbutton-no-start",
        "i2c-eeprom-24c32",
        "lcd-16x2",
        "led",
        "maker-tactile-4pin",
        "oled-128x64",
        "pneu-cylinder-da",
        "pneu-valve-5-2",
        "potentiometer",
        "resistor",
        "sensor-limit-switch",
        "sensor-pressure-4-20ma",
        "signal-pilot-lamp",
    ] {
        let comp = catalog.components.get(comp_id).unwrap();
        assert!(
            is_valid_ref_designator_prefix(&comp.meta.ref_designator),
            "invalid IEC 81346-2 prefix on {comp_id}"
        );
    }

    // Verify DEF-007 display charset propagated cleanly from `assets/components/lcd-16x2/profile.toml`
    let lcd_meta =
        dendri_validate::ProfileCatalog::component_profile(&catalog, "lcd-16x2").unwrap();
    let cs = lcd_meta.display_charset.unwrap();
    assert!(cs.supports_char('°'));
    assert!(cs.supports_char('Ω'));
    assert!(!cs.supports_char('🔥'));

    // Verify DEF-002 verified analog channel bands on plc-1200-open-1214
    let plc = catalog.boards.get("plc-1200-open-1214").unwrap();
    let iw64 = plc.channels.iter().find(|c| c.port_id == "IW64").unwrap();
    let band = iw64.band.as_ref().unwrap();
    assert_eq!(band.underrange_min, -4_864);
    assert_eq!(band.overrange_max, 32_511);
    assert_eq!(band.overflow_code, Some(32_767));
    assert_eq!(band.wire_break_code, Some(-32_768));

    // Verify declarative composite sub-circuit on signal-pilot-lamp (`03` §1.3)
    let lamp = catalog.components.get("signal-pilot-lamp").unwrap();
    let composite = lamp.physics.composite.as_ref().unwrap();
    assert_eq!(composite.parts.len(), 2);
    assert_eq!(composite.wires.len(), 1);
    assert_eq!(
        composite.port_map.get("X1").map(String::as_str),
        Some("r_limit.A")
    );
    assert_eq!(
        composite.port_map.get("X2").map(String::as_str),
        Some("lamp_led.K")
    );
}

#[test]
fn live_asset_catalog_validates_dual_board_core_kit_project() {
    let root = find_workspace_root().unwrap();
    let catalog = AssetCatalog::load_validated(&root, &root.join("assets")).unwrap();

    let project_toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CoreKitDualBoardValidation"

[[boards]]
id = "plc1"
profile = "plc-1200-open-1214"
authoring_language = "st"
execution = "plc_scan"
scan_period_us = 10000

[boards.program]
imports = ["std.iec.*", "std.plc.*"]

[[boards.program.hardware]]
id = "n_start"
label = "START"
pin = "Ia.0"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_stop"
label = "STOP"
pin = "Ia.1"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_lamp"
label = "PILOT"
pin = "Qa.0"
kind = "digital_out"

[[boards.program.hardware]]
id = "n_ai"
label = "PRESSURE_AI"
pin = "IW64"
kind = "analog_in"
codec = "plc_span_27648"
range = { signal = "voltage", low = 0.0, high = 10.0 }

[[boards.program.pous]]
id = "n_ob1"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "network"

[[boards.program.pous.network.rungs]]
id = "n_r0"
order = 0

[[boards.program.pous.network.rungs.cells]]
id = "n_c0"
row = 0
col = 0
kind = "contact"
contact_kind = "normally_open"
target = "START"

[[boards.program.pous.network.rungs.cells]]
id = "n_c1"
row = 0
col = 1
kind = "coil"
coil_kind = "normal"
target = "PILOT"

[[components]]
id = "pb_start"
profile = "cmd-pushbutton-no-start"

[[components]]
id = "pb_stop"
profile = "cmd-pushbutton-nc-stop"

[[components]]
id = "lamp1"
profile = "signal-pilot-lamp"

[[components]]
id = "pt1"
profile = "sensor-pressure-4-20ma"

[[components]]
id = "r_shunt"
profile = "resistor"
params = { r_ohm = "500" }

[[wires]]
id = "w1"
from = "plc1.24V"
to = "pb_start.NO"
signal = "power"

[[wires]]
id = "w2"
from = "pb_start.COM"
to = "plc1.Ia.0"
signal = "digital"

[[wires]]
id = "w3"
from = "plc1.24V"
to = "pb_stop.NC"
signal = "power"

[[wires]]
id = "w4"
from = "pb_stop.COM"
to = "plc1.Ia.1"
signal = "digital"

[[wires]]
id = "w5"
from = "plc1.Qa.0"
to = "lamp1.X1"
signal = "digital"

[[wires]]
id = "w6"
from = "lamp1.X2"
to = "plc1.GND"
signal = "ground"

[[wires]]
id = "w7"
from = "pt1.OUT"
to = "plc1.IW64"
signal = "analog"

[[wires]]
id = "w8"
from = "plc1.IW64"
to = "r_shunt.1"
signal = "analog"

[[wires]]
id = "w9"
from = "r_shunt.2"
to = "plc1.GND"
signal = "ground"

[[wires]]
id = "w10"
from = "pt1.GND"
to = "plc1.GND"
signal = "ground"
"#;

    let proj = load_project_str(project_toml, None).unwrap();
    let bag = validate_project(&proj, &catalog, &ValidateOptions::default());
    assert!(
        !bag.has_errors(),
        "expected zero validation errors with live AssetCatalog, got: {:?}",
        bag.all()
    );
}

#[test]
fn tampering_svg_invalidates_review_toml_and_reapprove_restores_green_gate() {
    let root = find_workspace_root().unwrap();
    let tmp = TempDir::new().unwrap();
    fs::copy(
        root.join("THIRD_PARTY_NOTICES"),
        tmp.path().join("THIRD_PARTY_NOTICES"),
    )
    .unwrap();
    copy_dir_recursive(&root.join("assets"), &tmp.path().join("assets")).unwrap();

    // Baseline copy passes cleanly
    assert!(validate_assets_dir(tmp.path(), &tmp.path().join("assets")).is_ok());

    // Tamper with one color byte in resistor/physical.svg -> Rule 12 MUST fail!
    let res_dir = tmp.path().join("assets/components/resistor");
    let svg_path = res_dir.join("physical.svg");
    let orig = fs::read_to_string(&svg_path).unwrap();
    let edited = orig.replace("#ef4444", "#22c55e");
    assert_ne!(orig, edited);
    fs::write(&svg_path, edited).unwrap();

    let err = validate_assets_dir(tmp.path(), &tmp.path().join("assets")).unwrap_err();
    let AssetCheckError::Violations(vs) = err else {
        panic!("expected Violations, got {err:?}");
    };
    assert!(
        vs.iter()
            .any(|v| v.rule == "R12" && v.message.contains("stale physical_render_sha256")),
        "expected R12 stale render hash violation, got {vs:?}"
    );

    // Re-approving explicitly restores 100% green asset-check!
    approve_asset_dir(&res_dir, "caio", "2026-11-23", false).unwrap();
    assert!(validate_assets_dir(tmp.path(), &tmp.path().join("assets")).is_ok());
}

#[test]
fn rejects_unregistered_physics_broken_composite_unknown_standard_and_tracks_ml_restricted() {
    let root = find_workspace_root().unwrap();
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("THIRD_PARTY_NOTICES"),
        "Open Sample Library v1.0 — CC-BY-4.0\n",
    )
    .unwrap();
    copy_dir_recursive(&root.join("assets"), &tmp.path().join("assets")).unwrap();

    // 1. Corrupt signal-pilot-lamp composite wire + unknown standard -> must fail R9 & R10
    let lamp_dir = tmp.path().join("assets/components/signal-pilot-lamp");
    let lamp_profile = fs::read_to_string(lamp_dir.join("profile.toml")).unwrap();
    let broken = lamp_profile
        .replace(r#"from = "r_limit.B""#, r#"from = "ghost_part.B""#)
        .replace(r#""IEC 81346""#, r#""IEC 99999-BOGUS""#);
    fs::write(lamp_dir.join("profile.toml"), broken).unwrap();
    approve_asset_dir(&lamp_dir, "caio", "2026-11-23", false).unwrap();

    let err = validate_assets_dir(tmp.path(), &tmp.path().join("assets")).unwrap_err();
    let AssetCheckError::Violations(vs) = err else {
        panic!("expected Violations, got {err:?}");
    };
    assert!(
        vs.iter().any(|v| v.rule == "R9"),
        "expected R9 violation: {vs:?}"
    );
    assert!(
        vs.iter().any(|v| v.rule == "R10"),
        "expected R10 violation: {vs:?}"
    );

    // Restore signal-pilot-lamp and convert resistor to a compliant third-party ml_restricted asset
    fs::write(lamp_dir.join("profile.toml"), lamp_profile).unwrap();
    approve_asset_dir(&lamp_dir, "caio", "2026-11-23", false).unwrap();

    let res_dir = tmp.path().join("assets/components/resistor");
    let res_prof = fs::read_to_string(res_dir.join("profile.toml"))
        .unwrap()
        .replace(
            r#"source = "original"
symbol_ref = "IEC 60617-S00555"
reviewed_by = "caio"
reviewed_on = "2026-11-23"
ml_restricted = false"#,
            r#"source = "third-party"
symbol_ref = "IEC 60617-S00555"
reviewed_by = "caio"
reviewed_on = "2026-11-23"
ml_restricted = true
origin = "Open Sample Library v1.0"
license = "CC-BY-4.0"
attribution = "Copyright Open Sample Contributors"
modified = true"#,
        );
    fs::write(res_dir.join("profile.toml"), res_prof).unwrap();
    for svg_name in ["physical.svg", "schematic.svg"] {
        let p = res_dir.join(svg_name);
        let s = fs::read_to_string(&p)
            .unwrap()
            .replace(r#"source="original""#, r#"source="third-party""#)
            .replace(r#"ml-restricted="false""#, r#"ml-restricted="true""#);
        fs::write(p, s).unwrap();
    }
    approve_asset_dir(&res_dir, "caio", "2026-11-23", false).unwrap();

    let report = validate_assets_dir(tmp.path(), &tmp.path().join("assets")).unwrap();
    assert_eq!(
        report.third_party_assets,
        vec!["components/resistor".to_owned()]
    );
    assert_eq!(
        report.ml_restricted_assets,
        vec!["components/resistor".to_owned()]
    );

    let cat = AssetCatalog::load_validated(tmp.path(), &tmp.path().join("assets")).unwrap();
    assert_eq!(
        cat.ml_restricted_ids(),
        vec!["components/resistor".to_owned()]
    );
}
