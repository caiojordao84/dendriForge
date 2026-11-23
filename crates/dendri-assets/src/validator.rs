//! Complete 13-rule Asset Validator Gate (`dendri asset-check`), explicit human sign-off helper
//! (`dendri asset-approve`), and live [`AssetCatalog`] bridging certified `assets/` into
//! `dendri-validate::ProfileCatalog` (`03-asset-and-profile-contract.md` §4.3, §7).

use crate::ref_des::is_valid_ref_designator_prefix;
use crate::schema::{
    BoardProfile, ComponentProfile, PortCapability, Provenance, ProvenanceSource, ReviewChecklist,
    ReviewSignOff, is_registered_primitive_model,
};
use crate::svg::{
    CANONICAL_GRID_PX, CANONICAL_PIN_PX, ParsedSvg, compute_render_sha256, parse_and_validate_svg,
};
use dendri_ast::{Codec, is_valid_profile_id};
use dendri_stdlib::StandardsRegistry;
use dendri_validate::{
    BoardProfileMeta, ComponentProfileMeta, DisplayCharset, PortMeta, ProfileCatalog,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Single violation emitted by [`validate_assets_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetViolation {
    pub asset_path: String,
    pub rule: String,
    pub message: String,
}

impl fmt::Display for AssetViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.rule, self.asset_path, self.message)
    }
}

/// Summary report returned on a 100% green `dendri asset-check` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetCheckReport {
    pub boards_checked: usize,
    pub components_checked: usize,
    pub standards_loaded: usize,
    pub ml_restricted_assets: Vec<String>,
    pub third_party_assets: Vec<String>,
}

/// Failure returned by [`validate_assets_dir`] or [`approve_asset_dir`].
#[derive(Debug, Error)]
pub enum AssetCheckError {
    #[error("I/O error at `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to load standards registry `{path}`: {message}")]
    Standards { path: PathBuf, message: String },
    #[error("asset-check failed with {} violation(s)", .0.len())]
    Violations(Vec<AssetViolation>),
    #[error("asset approval error at `{path}`: {message}")]
    Approve { path: PathBuf, message: String },
}

/// Loaded, certified live asset catalog (`assets/boards/*`, `assets/components/*`).
#[derive(Debug, Clone, Default)]
pub struct AssetCatalog {
    pub boards: BTreeMap<String, BoardProfile>,
    pub components: BTreeMap<String, ComponentProfile>,
    /// Pre-converted validation summaries for `dendri-validate::ProfileCatalog`.
    board_meta: BTreeMap<String, BoardProfileMeta>,
    component_meta: BTreeMap<String, ComponentProfileMeta>,
}

impl AssetCatalog {
    /// Loads and validates all boards & components under `assets_dir` (must pass all 13 checks!).
    pub fn load_validated(
        workspace_root: &Path,
        assets_dir: &Path,
    ) -> Result<Self, AssetCheckError> {
        let (_, catalog) = validate_and_load_assets(workspace_root, assets_dir)?;
        Ok(catalog)
    }

    /// Returns all asset IDs marked `ml_restricted = true` so training/RAG pipelines exclude them (`03` §4.1).
    #[must_use]
    pub fn ml_restricted_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (id, b) in &self.boards {
            if b.provenance.ml_restricted {
                out.push(format!("boards/{id}"));
            }
        }
        for (id, c) in &self.components {
            if c.provenance.ml_restricted {
                out.push(format!("components/{id}"));
            }
        }
        out
    }
}

impl ProfileCatalog for AssetCatalog {
    fn board_profile(&self, id: &str) -> Option<&BoardProfileMeta> {
        self.board_meta.get(id)
    }

    fn component_profile(&self, id: &str) -> Option<&ComponentProfileMeta> {
        self.component_meta.get(id)
    }
}

/// Runs ALL 13 normative checks of `03-asset-and-profile-contract.md` §7 across `assets_dir`.
pub fn validate_assets_dir(
    workspace_root: &Path,
    assets_dir: &Path,
) -> Result<AssetCheckReport, AssetCheckError> {
    let (report, _) = validate_and_load_assets(workspace_root, assets_dir)?;
    Ok(report)
}

/// Explicit human-invoked helper (`dendri asset-approve <asset-dir> --reviewer <name>`)
/// that validates `profile.toml` + SVGs, computes canonical `usvg`/`resvg` render SHA-256 hashes,
/// and atomically writes `review.toml` (`03` §4.3).
pub fn approve_asset_dir(
    asset_dir: &Path,
    reviewer: &str,
    reviewed_on: &str,
    conformity_claimed: bool,
) -> Result<ReviewSignOff, AssetCheckError> {
    if reviewer.trim().is_empty() {
        return Err(AssetCheckError::Approve {
            path: asset_dir.to_path_buf(),
            message: "reviewer name must be non-empty".to_owned(),
        });
    }
    let profile_path = asset_dir.join("profile.toml");
    let profile_toml = fs::read_to_string(&profile_path).map_err(|source| AssetCheckError::Io {
        path: profile_path.clone(),
        source,
    })?;

    // Determine physical & schematic SVG filenames + pin pitch from either BoardProfile or ComponentProfile
    let (phys_file, schem_file, pin_pitch) = match toml::from_str::<ComponentProfile>(&profile_toml)
    {
        Ok(comp) => (
            comp.svg.physical,
            Some(comp.svg.schematic),
            comp.geometry.pin_pitch_px,
        ),
        Err(comp_err) => match toml::from_str::<BoardProfile>(&profile_toml) {
            Ok(board) => (
                board.svg.physical,
                board.svg.schematic,
                board.geometry.pin_pitch_px,
            ),
            Err(board_err) => {
                return Err(AssetCheckError::Approve {
                    path: profile_path,
                    message: format!(
                        "profile.toml failed both ComponentProfile ({comp_err}) and BoardProfile ({board_err})"
                    ),
                });
            }
        },
    };

    let phys_path = asset_dir.join(&phys_file);
    let phys_svg = fs::read_to_string(&phys_path).map_err(|source| AssetCheckError::Io {
        path: phys_path.clone(),
        source,
    })?;
    parse_and_validate_svg(&phys_svg, pin_pitch).map_err(|e| AssetCheckError::Approve {
        path: phys_path,
        message: e.to_string(),
    })?;
    let physical_render_sha256 =
        compute_render_sha256(&profile_toml, &phys_svg).map_err(|e| AssetCheckError::Approve {
            path: asset_dir.to_path_buf(),
            message: e.to_string(),
        })?;

    let schematic_render_sha256 = match schem_file {
        Some(sf) => {
            let sp = asset_dir.join(&sf);
            let s_svg = fs::read_to_string(&sp).map_err(|source| AssetCheckError::Io {
                path: sp.clone(),
                source,
            })?;
            parse_and_validate_svg(&s_svg, pin_pitch).map_err(|e| AssetCheckError::Approve {
                path: sp,
                message: e.to_string(),
            })?;
            Some(compute_render_sha256(&profile_toml, &s_svg).map_err(|e| {
                AssetCheckError::Approve {
                    path: asset_dir.to_path_buf(),
                    message: e.to_string(),
                }
            })?)
        }
        None => None,
    };

    let review = ReviewSignOff {
        schema_version: "2.0".to_owned(),
        reviewer: reviewer.to_owned(),
        reviewed_on: reviewed_on.to_owned(),
        physical_render_sha256,
        schematic_render_sha256,
        conformity_claimed,
        checklist: ReviewChecklist {
            grid_and_pins_verified: true,
            symbology_style_checked: true,
            datasheet_params_cited: true,
            provenance_verified: true,
        },
    };

    let rendered = toml::to_string_pretty(&review).map_err(|e| AssetCheckError::Approve {
        path: asset_dir.to_path_buf(),
        message: format!("failed to serialize review.toml: {e}"),
    })?;
    let review_path = asset_dir.join("review.toml");
    let tmp_path = asset_dir.join(".review.toml.tmp");
    fs::write(&tmp_path, rendered.as_bytes()).map_err(|source| AssetCheckError::Io {
        path: tmp_path.clone(),
        source,
    })?;
    fs::rename(&tmp_path, &review_path).map_err(|source| AssetCheckError::Io {
        path: review_path,
        source,
    })?;

    Ok(review)
}

fn validate_and_load_assets(
    workspace_root: &Path,
    assets_dir: &Path,
) -> Result<(AssetCheckReport, AssetCatalog), AssetCheckError> {
    // Rule 10 prerequisite: load and validate `assets/standards.toml`
    let standards_path = assets_dir.join("standards.toml");
    let standards = StandardsRegistry::load_from_path(&standards_path).map_err(|e| {
        AssetCheckError::Standards {
            path: standards_path,
            message: e.to_string(),
        }
    })?;

    // Rule 11 prerequisite: load `THIRD_PARTY_NOTICES` at workspace root
    let notices_path = workspace_root.join("THIRD_PARTY_NOTICES");
    let notices_text = fs::read_to_string(&notices_path).unwrap_or_default();

    let mut violations = Vec::new();
    let mut global_ids = BTreeSet::new();
    let mut catalog = AssetCatalog::default();
    let mut ml_restricted_assets = Vec::new();
    let mut third_party_assets = Vec::new();

    // Validate `assets/boards/*`
    let boards_dir = assets_dir.join("boards");
    for dir in sorted_subdirs(&boards_dir)? {
        validate_board_dir(
            &dir,
            &standards,
            &notices_text,
            &mut global_ids,
            &mut catalog,
            &mut ml_restricted_assets,
            &mut third_party_assets,
            &mut violations,
        );
    }

    // Validate `assets/components/*`
    let comps_dir = assets_dir.join("components");
    for dir in sorted_subdirs(&comps_dir)? {
        validate_component_dir(
            &dir,
            &standards,
            &notices_text,
            &mut global_ids,
            &mut catalog,
            &mut ml_restricted_assets,
            &mut third_party_assets,
            &mut violations,
        );
    }

    if !violations.is_empty() {
        return Err(AssetCheckError::Violations(violations));
    }

    let report = AssetCheckReport {
        boards_checked: catalog.boards.len(),
        components_checked: catalog.components.len(),
        standards_loaded: standards.len(),
        ml_restricted_assets,
        third_party_assets,
    };
    Ok((report, catalog))
}

#[allow(clippy::too_many_arguments)]
fn validate_board_dir(
    dir: &Path,
    standards: &StandardsRegistry,
    notices_text: &str,
    global_ids: &mut BTreeSet<String>,
    catalog: &mut AssetCatalog,
    ml_restricted: &mut Vec<String>,
    third_party: &mut Vec<String>,
    violations: &mut Vec<AssetViolation>,
) {
    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<non-utf8>")
        .to_owned();
    let label = format!("boards/{dir_name}");
    let profile_path = dir.join("profile.toml");

    let profile_toml = match fs::read_to_string(&profile_path) {
        Ok(s) => s,
        Err(e) => {
            push_v(
                violations,
                &label,
                "R1",
                format!("cannot read profile.toml: {e}"),
            );
            return;
        }
    };
    let board: BoardProfile = match toml::from_str(&profile_toml) {
        Ok(b) => b,
        Err(e) => {
            push_v(
                violations,
                &label,
                "R1",
                format!("strict board profile.toml parse error: {e}"),
            );
            return;
        }
    };

    if board.schema_version != "2.0" {
        push_v(
            violations,
            &label,
            "R1",
            format!(
                "expected schema_version = \"2.0\", got `{}`",
                board.schema_version
            ),
        );
    }

    // Rule 2: kebab-case profile ID == dir name & globally unique
    check_profile_id(&board.meta.id, &dir_name, &label, global_ids, violations);
    if !is_valid_ref_designator_prefix(&board.meta.ref_designator) {
        push_v(
            violations,
            &label,
            "R2",
            format!(
                "invalid IEC 81346-2 ref_designator `{}`",
                board.meta.ref_designator
            ),
        );
    }

    // Rule 10: standards resolve against assets/standards.toml
    for std_id in &board.meta.standards {
        if !standards.contains(std_id) {
            push_v(
                violations,
                &label,
                "R10",
                format!("unknown standard `{std_id}` (not in assets/standards.toml)"),
            );
        }
    }

    // Rule 11: provenance + THIRD_PARTY_NOTICES + ml_restricted
    let has_schematic = board.svg.schematic.is_some();
    check_provenance(
        &board.provenance,
        &board.meta.id,
        has_schematic,
        notices_text,
        &label,
        ml_restricted,
        third_party,
        violations,
    );

    // Rule 13: codecs (D <= M), bands (fits i32), non-empty source_refs, valid cpu/electrical
    if board.cpu.clock_hz == 0 || board.cpu.source_ref.trim().is_empty() {
        push_v(
            violations,
            &label,
            "R13",
            "cpu.clock_hz must be > 0 with non-empty source_ref".to_owned(),
        );
    }
    if board.electrical.source_ref.trim().is_empty() {
        push_v(
            violations,
            &label,
            "R13",
            "electrical.source_ref must be non-empty".to_owned(),
        );
    }
    if !matches!(
        board.execution.indeterminate_reads_as.as_str(),
        "previous" | "low" | "high"
    ) {
        push_v(
            violations,
            &label,
            "R13",
            format!(
                "invalid execution.indeterminate_reads_as `{}` (expected previous|low|high)",
                board.execution.indeterminate_reads_as
            ),
        );
    }

    let mut codec_map = BTreeMap::new();
    for c in &board.codecs {
        if c.source_ref.trim().is_empty() {
            push_v(
                violations,
                &label,
                "R13",
                format!("codec `{}` missing non-empty source_ref", c.id),
            );
        }
        match Codec::new(&c.id, c.kind, c.d, c.n_max) {
            Ok(mut valid_c) => {
                valid_c.source_ref = Some(c.source_ref.clone());
                codec_map.insert(c.id.clone(), valid_c);
            }
            Err(e) => {
                push_v(
                    violations,
                    &label,
                    "R13",
                    format!("codec `{}` invalid: {e}", c.id),
                );
            }
        }
    }

    let port_role_map: BTreeMap<String, dendri_theme::PinClass> =
        board.ports.iter().map(|p| (p.id.clone(), p.role)).collect();
    if port_role_map.len() != board.ports.len() {
        push_v(
            violations,
            &label,
            "R6",
            "duplicate port IDs in [[ports]]".to_owned(),
        );
    }

    let mut band_map = BTreeMap::new();
    for ch in &board.channels {
        if !port_role_map.contains_key(&ch.port_id) {
            push_v(
                violations,
                &label,
                "R13",
                format!("channel references undeclared port `{}`", ch.port_id),
            );
        }
        let Some(codec) = codec_map.get(&ch.codec_id) else {
            push_v(
                violations,
                &label,
                "R13",
                format!(
                    "channel `{}` references undeclared codec `{}`",
                    ch.port_id, ch.codec_id
                ),
            );
            continue;
        };
        if let Some(band) = &ch.band {
            if band.source_ref.trim().is_empty() {
                push_v(
                    violations,
                    &label,
                    "R13",
                    format!("channel `{}` band missing non-empty source_ref", ch.port_id),
                );
            }
            if let Err(e) = band.validate_against(codec) {
                push_v(
                    violations,
                    &label,
                    "R13",
                    format!("channel `{}` band invalid: {e}", ch.port_id),
                );
            }
            band_map.insert(ch.port_id.clone(), band.clone());
        }
        if let Some(pwm) = &ch.pwm
            && (pwm.period_us == 0 || pwm.source_ref.trim().is_empty())
        {
            push_v(
                violations,
                &label,
                "R13",
                format!(
                    "channel `{}` PWM spec requires period_us > 0 and source_ref",
                    ch.port_id
                ),
            );
        }
    }

    // Rules 3–8 & 12: validate SVGs, viewBox, df-pins 1:1 bijection, bindings, provenance, review.toml
    check_geometry_spec(&board.geometry, &label, violations);
    let phys_svg = validate_single_svg(
        dir,
        &board.svg.physical,
        board.geometry.physical_grid_w,
        board.geometry.physical_grid_h,
        board.geometry.pin_pitch_px,
        &port_role_map,
        &board.provenance,
        false,
        &label,
        violations,
    );
    let schem_svg = if let Some(ref sf) = board.svg.schematic {
        let sw = board
            .geometry
            .schematic_grid_w
            .unwrap_or(board.geometry.physical_grid_w);
        let sh = board
            .geometry
            .schematic_grid_h
            .unwrap_or(board.geometry.physical_grid_h);
        validate_single_svg(
            dir,
            sf,
            sw,
            sh,
            board.geometry.pin_pitch_px,
            &port_role_map,
            &board.provenance,
            true,
            &label,
            violations,
        )
    } else {
        None
    };

    check_bindings(
        &board.svg.bindings,
        phys_svg.as_ref(),
        schem_svg.as_ref(),
        &label,
        violations,
    );
    check_review_toml(
        dir,
        &profile_toml,
        &board.svg.physical,
        board.svg.schematic.as_deref(),
        &label,
        violations,
    );

    // Populate live ProfileCatalog metadata
    let mut meta_ports = BTreeMap::new();
    for p in &board.ports {
        let has = |cap: PortCapability| p.capabilities.contains(&cap);
        let nominal_v = p.nominal_v.unwrap_or_else(|| {
            if has(PortCapability::Ground) {
                0.0
            } else {
                board.electrical.logic_high_v
            }
        });
        let v_abs_max_v = p.v_abs_max_v.unwrap_or(board.electrical.v_abs_max_v);
        meta_ports.insert(
            p.id.clone(),
            PortMeta {
                digital_in: has(PortCapability::DigitalIn),
                digital_out: has(PortCapability::DigitalOut),
                pullup: has(PortCapability::Pullup),
                pulldown: has(PortCapability::Pulldown),
                analog_in: has(PortCapability::AnalogIn),
                analog_out: has(PortCapability::AnalogOut),
                pwm: has(PortCapability::Pwm),
                bus: has(PortCapability::UartTx)
                    || has(PortCapability::UartRx)
                    || has(PortCapability::I2cSda)
                    || has(PortCapability::I2cScl)
                    || has(PortCapability::SpiMosi)
                    || has(PortCapability::SpiMiso)
                    || has(PortCapability::SpiSck)
                    || has(PortCapability::SpiCs)
                    || has(PortCapability::Ethernet),
                nominal_v,
                v_abs_max_v,
            },
        );
    }

    let id = board.meta.id.clone();
    catalog.board_meta.insert(
        id.clone(),
        BoardProfileMeta {
            id: id.clone(),
            ports: meta_ports,
            codecs: codec_map,
            channel_bands: band_map,
        },
    );
    catalog.boards.insert(id, board);
}

#[allow(clippy::too_many_arguments)]
fn validate_component_dir(
    dir: &Path,
    standards: &StandardsRegistry,
    notices_text: &str,
    global_ids: &mut BTreeSet<String>,
    catalog: &mut AssetCatalog,
    ml_restricted: &mut Vec<String>,
    third_party: &mut Vec<String>,
    violations: &mut Vec<AssetViolation>,
) {
    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<non-utf8>")
        .to_owned();
    let label = format!("components/{dir_name}");
    let profile_path = dir.join("profile.toml");

    let profile_toml = match fs::read_to_string(&profile_path) {
        Ok(s) => s,
        Err(e) => {
            push_v(
                violations,
                &label,
                "R1",
                format!("cannot read profile.toml: {e}"),
            );
            return;
        }
    };
    let comp: ComponentProfile = match toml::from_str(&profile_toml) {
        Ok(c) => c,
        Err(e) => {
            push_v(
                violations,
                &label,
                "R1",
                format!("strict component profile.toml parse error: {e}"),
            );
            return;
        }
    };

    if comp.schema_version != "2.0" {
        push_v(
            violations,
            &label,
            "R1",
            format!(
                "expected schema_version = \"2.0\", got `{}`",
                comp.schema_version
            ),
        );
    }

    // Rule 2: kebab-case profile ID == dir name & globally unique
    check_profile_id(&comp.meta.id, &dir_name, &label, global_ids, violations);
    if !is_valid_ref_designator_prefix(&comp.meta.ref_designator) {
        push_v(
            violations,
            &label,
            "R2",
            format!(
                "invalid IEC 81346-2 ref_designator `{}`",
                comp.meta.ref_designator
            ),
        );
    }

    // Rule 9: physics model (primitive OR declarative composite sub-circuit) must resolve
    match (&comp.physics.model, &comp.physics.composite) {
        (Some(model), None) => {
            if !is_registered_primitive_model(model) {
                push_v(
                    violations,
                    &label,
                    "R9",
                    format!("unregistered physics primitive model `{model}`"),
                );
            }
        }
        (None, Some(sub)) => {
            if sub.parts.is_empty() {
                push_v(
                    violations,
                    &label,
                    "R9",
                    "composite sub-circuit has zero parts".to_owned(),
                );
            }
            let mut part_ids = BTreeSet::new();
            for part in &sub.parts {
                if !part_ids.insert(part.id.clone()) {
                    push_v(
                        violations,
                        &label,
                        "R9",
                        format!("duplicate composite part id `{}`", part.id),
                    );
                }
                if !is_registered_primitive_model(&part.model) {
                    push_v(
                        violations,
                        &label,
                        "R9",
                        format!(
                            "composite part `{}` uses unregistered model `{}`",
                            part.id, part.model
                        ),
                    );
                }
            }
            for wire in &sub.wires {
                for ep in [&wire.from, &wire.to] {
                    let valid = ep
                        .split_once('.')
                        .is_some_and(|(pid, pin)| part_ids.contains(pid) && !pin.is_empty());
                    if !valid {
                        push_v(
                            violations,
                            &label,
                            "R9",
                            format!(
                                "composite wire endpoint `{ep}` does not reference `<part_id>.<pin>`"
                            ),
                        );
                    }
                }
            }
            for port in &comp.ports {
                match sub.port_map.get(&port.id) {
                    None => {
                        push_v(
                            violations,
                            &label,
                            "R9",
                            format!(
                                "composite sub-circuit missing port_map entry for external port `{}`",
                                port.id
                            ),
                        );
                    }
                    Some(target) => {
                        let valid = target
                            .split_once('.')
                            .is_some_and(|(pid, pin)| part_ids.contains(pid) && !pin.is_empty());
                        if !valid {
                            push_v(
                                violations,
                                &label,
                                "R9",
                                format!(
                                    "composite port_map `{}` -> `{target}` does not reference `<part_id>.<pin>`",
                                    port.id
                                ),
                            );
                        }
                    }
                }
            }
        }
        _ => {
            push_v(
                violations,
                &label,
                "R9",
                "physics must declare exactly one of `model` or `composite`".to_owned(),
            );
        }
    }

    // Rule 10: standards resolve against assets/standards.toml
    for std_id in &comp.meta.standards {
        if !standards.contains(std_id) {
            push_v(
                violations,
                &label,
                "R10",
                format!("unknown standard `{std_id}` (not in assets/standards.toml)"),
            );
        }
    }

    // Rule 11: provenance (schematic is MANDATORY on components -> symbol_ref required!)
    check_provenance(
        &comp.provenance,
        &comp.meta.id,
        true,
        notices_text,
        &label,
        ml_restricted,
        third_party,
        violations,
    );

    let port_role_map: BTreeMap<String, dendri_theme::PinClass> =
        comp.ports.iter().map(|p| (p.id.clone(), p.role)).collect();
    if port_role_map.len() != comp.ports.len() {
        push_v(
            violations,
            &label,
            "R6",
            "duplicate port IDs in [[ports]]".to_owned(),
        );
    }

    // Rules 3–8 & 12: mandatory dual-view physical.svg + schematic.svg + bindings + review.toml
    check_geometry_spec(&comp.geometry, &label, violations);
    let phys_svg = validate_single_svg(
        dir,
        &comp.svg.physical,
        comp.geometry.physical_grid_w,
        comp.geometry.physical_grid_h,
        comp.geometry.pin_pitch_px,
        &port_role_map,
        &comp.provenance,
        false,
        &label,
        violations,
    );
    let sw = comp
        .geometry
        .schematic_grid_w
        .unwrap_or(comp.geometry.physical_grid_w);
    let sh = comp
        .geometry
        .schematic_grid_h
        .unwrap_or(comp.geometry.physical_grid_h);
    let schem_svg = validate_single_svg(
        dir,
        &comp.svg.schematic,
        sw,
        sh,
        comp.geometry.pin_pitch_px,
        &port_role_map,
        &comp.provenance,
        true,
        &label,
        violations,
    );

    check_bindings(
        &comp.svg.bindings,
        phys_svg.as_ref(),
        schem_svg.as_ref(),
        &label,
        violations,
    );
    check_review_toml(
        dir,
        &profile_toml,
        &comp.svg.physical,
        Some(&comp.svg.schematic),
        &label,
        violations,
    );

    // Validate optional `[display].charset` against authoritative DEF-007 ROM charsets
    let display_charset = match &comp.display {
        Some(disp) => match DisplayCharset::from_id(&disp.charset) {
            Some(cs) => Some(cs),
            None => {
                push_v(
                    violations,
                    &label,
                    "R9",
                    format!(
                        "unknown display.charset `{}` (expected ascii_7bit|hd44780_a00|hd44780_a02)",
                        disp.charset
                    ),
                );
                None
            }
        },
        None => None,
    };

    // Populate live ProfileCatalog metadata
    let mut meta_ports = BTreeMap::new();
    for p in &comp.ports {
        let is_comm = matches!(p.role, dendri_theme::PinClass::Comm);
        meta_ports.insert(
            p.id.clone(),
            PortMeta {
                digital_in: !is_comm,
                digital_out: !is_comm,
                pullup: false,
                pulldown: false,
                analog_in: !is_comm,
                analog_out: !is_comm,
                pwm: false,
                bus: is_comm,
                nominal_v: p.nominal_v,
                v_abs_max_v: p.v_abs_max_v,
            },
        );
    }
    let id = comp.meta.id.clone();
    catalog.component_meta.insert(
        id.clone(),
        ComponentProfileMeta {
            id: id.clone(),
            ports: meta_ports,
            display_charset,
        },
    );
    catalog.components.insert(id, comp);
}

fn check_profile_id(
    id: &str,
    dir_name: &str,
    label: &str,
    global_ids: &mut BTreeSet<String>,
    violations: &mut Vec<AssetViolation>,
) {
    if !is_valid_profile_id(id) {
        push_v(
            violations,
            label,
            "R2",
            format!("profile id `{id}` violates kebab-case grammar `^[a-z0-9]+(-[a-z0-9]+)*$`"),
        );
    }
    if id != dir_name {
        push_v(
            violations,
            label,
            "R2",
            format!("profile id `{id}` does not match folder name `{dir_name}`"),
        );
    }
    if !global_ids.insert(id.to_owned()) {
        push_v(
            violations,
            label,
            "R2",
            format!("duplicate profile id `{id}` across catalog"),
        );
    }
}

fn check_geometry_spec(
    geom: &crate::schema::GeometrySpec,
    label: &str,
    violations: &mut Vec<AssetViolation>,
) {
    if geom.grid_px != CANONICAL_GRID_PX {
        push_v(
            violations,
            label,
            "R5",
            format!(
                "geometry.grid_px must be {CANONICAL_GRID_PX}, found {}",
                geom.grid_px
            ),
        );
    }
    if geom.pin_px != CANONICAL_PIN_PX {
        push_v(
            violations,
            label,
            "R7",
            format!(
                "geometry.pin_px must be {CANONICAL_PIN_PX}, found {}",
                geom.pin_px
            ),
        );
    }
    if !matches!(geom.pin_pitch_px, 48 | 24 | 12) {
        push_v(
            violations,
            label,
            "R7",
            format!(
                "geometry.pin_pitch_px must be 48, 24, or 12 px, found {}",
                geom.pin_pitch_px
            ),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_single_svg(
    dir: &Path,
    file_name: &str,
    expected_grid_w: f64,
    expected_grid_h: f64,
    pin_pitch_px: u32,
    expected_ports: &BTreeMap<String, dendri_theme::PinClass>,
    profile_prov: &Provenance,
    is_schematic: bool,
    label: &str,
    violations: &mut Vec<AssetViolation>,
) -> Option<ParsedSvg> {
    let svg_path = dir.join(file_name);
    let svg_src = match fs::read_to_string(&svg_path) {
        Ok(s) => s,
        Err(e) => {
            push_v(
                violations,
                label,
                "R3",
                format!("missing or unreadable SVG `{file_name}`: {e}"),
            );
            return None;
        }
    };

    let parsed = match parse_and_validate_svg(&svg_src, pin_pitch_px) {
        Ok(p) => p,
        Err(e) => {
            push_v(
                violations,
                label,
                "R4",
                format!("`{file_name}` failed SVG gate: {e}"),
            );
            return None;
        }
    };

    // Rule 5: exact viewBox == grid_w * 48 x grid_h * 48 (tol <= 1e-3)
    let exp_w = expected_grid_w * f64::from(CANONICAL_GRID_PX);
    let exp_h = expected_grid_h * f64::from(CANONICAL_GRID_PX);
    if libm::fabs(parsed.viewbox_w - exp_w) > 1e-3 || libm::fabs(parsed.viewbox_h - exp_h) > 1e-3 {
        push_v(
            violations,
            label,
            "R5",
            format!(
                "`{file_name}` viewBox ({}x{}) does not match geometry ({}x{} px)",
                parsed.viewbox_w, parsed.viewbox_h, exp_w, exp_h
            ),
        );
    }

    // Rule 6 & 7: 1:1 port bijection & exact df-pin-* role parity
    for (port_id, &expected_role) in expected_ports {
        match parsed.pins.get(port_id) {
            None => {
                push_v(
                    violations,
                    label,
                    "R6",
                    format!("`{file_name}` missing <g id=\"df-pins\"> marker `pin:{port_id}`"),
                );
            }
            Some(pin) => {
                if pin.pin_class != expected_role {
                    push_v(
                        violations,
                        label,
                        "R7",
                        format!(
                            "`{file_name}` pin `{port_id}` has class `{}` but profile.toml role requires `{}`",
                            pin.pin_class.css_class(),
                            expected_role.css_class()
                        ),
                    );
                }
            }
        }
    }
    for svg_port in parsed.pins.keys() {
        if !expected_ports.contains_key(svg_port) {
            push_v(
                violations,
                label,
                "R6",
                format!(
                    "`{file_name}` has extra pin marker `pin:{svg_port}` not declared in profile.toml"
                ),
            );
        }
    }

    // Rule 11: SVG `<metadata><df:provenance .../></metadata>` matches profile.toml
    match &parsed.provenance {
        None => {
            push_v(
                violations,
                label,
                "R11",
                format!("`{file_name}` missing <metadata><df:provenance .../></metadata>"),
            );
        }
        Some(svg_prov) => {
            let expected_src = match profile_prov.source {
                ProvenanceSource::Original => "original",
                ProvenanceSource::ThirdParty => "third-party",
            };
            if svg_prov.source != expected_src
                || svg_prov.reviewed_by != profile_prov.reviewed_by
                || svg_prov.reviewed_on != profile_prov.reviewed_on
                || svg_prov.ml_restricted != profile_prov.ml_restricted
            {
                push_v(
                    violations,
                    label,
                    "R11",
                    format!("`{file_name}` <df:provenance> metadata diverges from profile.toml"),
                );
            }
            if is_schematic && svg_prov.symbol_ref.is_none() {
                push_v(
                    violations,
                    label,
                    "R11",
                    format!(
                        "schematic `{file_name}` missing required `symbol-ref` in <df:provenance>"
                    ),
                );
            }
        }
    }

    Some(parsed)
}

fn check_bindings(
    bindings: &[crate::schema::SvgBinding],
    phys: Option<&ParsedSvg>,
    schem: Option<&ParsedSvg>,
    label: &str,
    violations: &mut Vec<AssetViolation>,
) {
    for b in bindings {
        let in_phys = phys.is_some_and(|p| p.element_ids.contains(&b.target_id));
        let in_schem = schem.is_some_and(|s| s.element_ids.contains(&b.target_id));
        if !in_phys && !in_schem {
            push_v(
                violations,
                label,
                "R8",
                format!(
                    "svg.bindings target_id `{}` not found in physical or schematic SVG",
                    b.target_id
                ),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_provenance(
    prov: &Provenance,
    id: &str,
    requires_symbol_ref: bool,
    notices_text: &str,
    label: &str,
    ml_restricted: &mut Vec<String>,
    third_party: &mut Vec<String>,
    violations: &mut Vec<AssetViolation>,
) {
    if prov.reviewed_by.trim().is_empty() || prov.reviewed_on.trim().is_empty() {
        push_v(
            violations,
            label,
            "R11",
            "provenance.reviewed_by and reviewed_on must be non-empty".to_owned(),
        );
    }
    if requires_symbol_ref
        && prov
            .symbol_ref
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
    {
        push_v(
            violations,
            label,
            "R11",
            "provenance.symbol_ref is required for assets with schematic.svg".to_owned(),
        );
    }
    if prov.ml_restricted {
        ml_restricted.push(label.to_owned());
    }
    match prov.source {
        ProvenanceSource::Original => {
            if prov.origin.is_some()
                || prov.license.is_some()
                || prov.attribution.is_some()
                || prov.modified.is_some()
            {
                push_v(
                    violations,
                    label,
                    "R11",
                    "third-party provenance fields (origin/license/attribution/modified) are forbidden when source = \"original\"".to_owned(),
                );
            }
        }
        ProvenanceSource::ThirdParty => {
            third_party.push(label.to_owned());
            if prov.origin.as_deref().is_none_or(|s| s.trim().is_empty())
                || prov.license.as_deref().is_none_or(|s| s.trim().is_empty())
                || prov
                    .attribution
                    .as_deref()
                    .is_none_or(|s| s.trim().is_empty())
                || prov.modified.is_none()
            {
                push_v(
                    violations,
                    label,
                    "R11",
                    "source = \"third-party\" requires origin, license, attribution, and modified"
                        .to_owned(),
                );
            }
            let origin = prov.origin.as_deref().unwrap_or("").trim();
            if !origin.is_empty() && !notices_text.contains(origin) && !notices_text.contains(id) {
                push_v(
                    violations,
                    label,
                    "R11",
                    format!(
                        "third-party origin `{origin}` (asset `{id}`) missing matching entry in THIRD_PARTY_NOTICES"
                    ),
                );
            }
        }
    }
}

fn check_review_toml(
    dir: &Path,
    profile_toml: &str,
    phys_file: &str,
    schem_file: Option<&str>,
    label: &str,
    violations: &mut Vec<AssetViolation>,
) {
    let review_path = dir.join("review.toml");
    let raw = match fs::read_to_string(&review_path) {
        Ok(s) => s,
        Err(e) => {
            push_v(
                violations,
                label,
                "R12",
                format!("missing review.toml: {e}"),
            );
            return;
        }
    };
    let review: ReviewSignOff = match toml::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            push_v(
                violations,
                label,
                "R12",
                format!("invalid review.toml schema: {e}"),
            );
            return;
        }
    };
    if review.schema_version != "2.0"
        || review.reviewer.trim().is_empty()
        || review.reviewed_on.trim().is_empty()
    {
        push_v(
            violations,
            label,
            "R12",
            "review.toml requires schema_version = \"2.0\", non-empty reviewer and reviewed_on"
                .to_owned(),
        );
    }
    if !(review.checklist.grid_and_pins_verified
        && review.checklist.symbology_style_checked
        && review.checklist.datasheet_params_cited
        && review.checklist.provenance_verified)
    {
        push_v(
            violations,
            label,
            "R12",
            "all review.toml [checklist] booleans must be true".to_owned(),
        );
    }

    if let Ok(phys_svg) = fs::read_to_string(dir.join(phys_file))
        && let Ok(live_hash) = compute_render_sha256(profile_toml, &phys_svg)
        && live_hash != review.physical_render_sha256
    {
        push_v(
            violations,
            label,
            "R12",
            format!(
                "stale physical_render_sha256 in review.toml (recorded `{}`, live `{live_hash}`); re-review and run `dendri asset-approve`",
                review.physical_render_sha256
            ),
        );
    }

    if let Some(sf) = schem_file {
        let Some(ref recorded) = review.schematic_render_sha256 else {
            push_v(
                violations,
                label,
                "R12",
                "review.toml missing schematic_render_sha256 for asset with schematic.svg"
                    .to_owned(),
            );
            return;
        };
        if let Ok(schem_svg) = fs::read_to_string(dir.join(sf))
            && let Ok(live_hash) = compute_render_sha256(profile_toml, &schem_svg)
            && &live_hash != recorded
        {
            push_v(
                violations,
                label,
                "R12",
                format!(
                    "stale schematic_render_sha256 in review.toml (recorded `{recorded}`, live `{live_hash}`); re-review and run `dendri asset-approve`"
                ),
            );
        }
    }
}

fn sorted_subdirs(parent: &Path) -> Result<Vec<PathBuf>, AssetCheckError> {
    if !parent.exists() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    let entries = fs::read_dir(parent).map_err(|source| AssetCheckError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    for entry_res in entries {
        let entry = entry_res.map_err(|source| AssetCheckError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let p = entry.path();
        if p.is_dir() {
            dirs.push(p);
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn push_v(violations: &mut Vec<AssetViolation>, asset_path: &str, rule: &str, message: String) {
    violations.push(AssetViolation {
        asset_path: asset_path.to_owned(),
        rule: rule.to_owned(),
        message,
    });
}
