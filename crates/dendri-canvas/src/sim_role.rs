//! Declarative `[dendri].sim_role` for bridge / session (D-comp-14).
//!
//! Values: `led-load` | `momentary-contact` | `adc-source`.
//! Bridge and session must not key off profile id substrings.

use std::collections::BTreeMap;
use std::path::Path;

use dendri_ast::SimulationProject;
use dendri_profiles::{ProfileDocument, ProfileStore};

use crate::led_model::{defaults_from_profile_params, LedElectricalParams};

/// Runtime role declared on a component profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimRole {
    /// DC load with vf / R / if_max (LED, pilot lamp).
    LedLoad,
    /// Momentary contact actuator (industrial NO/NC, tactile).
    MomentaryContact,
    /// Analog source (pot, 4–20 mA TX). Recognition only in D0b; path in D2.
    AdcSource,
}

impl SimRole {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "led-load" => Some(Self::LedLoad),
            "momentary-contact" => Some(Self::MomentaryContact),
            "adc-source" => Some(Self::AdcSource),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::LedLoad => "led-load",
            Self::MomentaryContact => "momentary-contact",
            Self::AdcSource => "adc-source",
        }
    }
}

/// Per-project hints loaded from profiles for scene / bridge / session.
#[derive(Debug, Clone)]
pub struct ProjectSimHints {
    /// `profile_id` → role (components that declare `[dendri].sim_role`).
    pub roles: BTreeMap<String, SimRole>,
    /// `profile_id` → LED electrical defaults for every `led-load` in the project
    /// (and `"led"` fallback). Includes board `logic_high` as `v_high`.
    pub led_params_by_profile: BTreeMap<String, LedElectricalParams>,
    /// Board logic-high rail (Uno 5 V, S7 24 V). Fallback when a profile map miss.
    pub led_params_base: LedElectricalParams,
}

impl Default for ProjectSimHints {
    fn default() -> Self {
        Self {
            roles: BTreeMap::new(),
            led_params_by_profile: BTreeMap::new(),
            led_params_base: LedElectricalParams::PROFILE_DEFAULTS,
        }
    }
}

impl ProjectSimHints {
    pub fn role_of(&self, profile_id: &str) -> Option<SimRole> {
        self.roles.get(profile_id).copied()
    }

    pub fn is_led_load(&self, profile_id: &str) -> bool {
        self.role_of(profile_id) == Some(SimRole::LedLoad)
    }

    pub fn is_momentary_contact(&self, profile_id: &str) -> bool {
        self.role_of(profile_id) == Some(SimRole::MomentaryContact)
    }

    pub fn is_adc_source(&self, profile_id: &str) -> bool {
        self.role_of(profile_id) == Some(SimRole::AdcSource)
    }

    /// Electrical base for a load instance: **component’s own** profile, not only `"led"`.
    pub fn led_params_for_profile(&self, profile_id: &str) -> LedElectricalParams {
        self.led_params_by_profile
            .get(profile_id)
            .copied()
            .unwrap_or(self.led_params_base)
    }
}

/// Read `sim_role` from a loaded profile document.
pub fn sim_role_from_doc(doc: &ProfileDocument) -> Option<SimRole> {
    doc.dendri
        .as_ref()
        .and_then(|d| d.sim_role.as_deref())
        .and_then(SimRole::parse)
}

/// Board logic-high from `[simulation]` (typed) then raw rest; ignores `[specs].voltage`.
pub fn board_logic_high_v(board: &ProfileDocument) -> Option<f64> {
    if let Some(sim) = board.simulation.as_ref() {
        for opt in [&sim.logic_high, &sim.vcc, &sim.voltage] {
            if let Some(v) = opt.as_ref().and_then(value_as_volts) {
                return Some(v);
            }
        }
    }
    if let Some(sim) = board.rest.get("simulation").and_then(|v| v.as_table()) {
        for key in ["logic_high", "vcc", "voltage"] {
            if let Some(v) = sim.get(key).and_then(value_as_volts) {
                return Some(v);
            }
        }
    }
    if let Some(ports) = board.rest.get("ports").and_then(|v| v.as_array()) {
        for p in ports {
            if let Some(t) = p.as_table() {
                if let Some(v) = t.get("voltage").and_then(value_as_volts) {
                    if (v - 5.0).abs() < 0.01 || (v - 3.3).abs() < 0.01 || (v - 24.0).abs() < 0.01 {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

fn value_as_volts(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Float(f) => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        toml::Value::String(s) => {
            let s = s.trim().trim_end_matches('V').trim_end_matches('v');
            s.parse().ok()
        }
        _ => None,
    }
}

/// Load roles + per-profile LED params for every component in the project.
pub fn load_project_sim_hints(repo_root: &Path, project: &SimulationProject) -> ProjectSimHints {
    let store = ProfileStore::from_repo_root(repo_root);
    load_project_sim_hints_from_store(&store, project)
}

pub fn load_project_sim_hints_from_store(
    store: &ProfileStore,
    project: &SimulationProject,
) -> ProjectSimHints {
    let mut hints = ProjectSimHints::default();
    let mut v_high = hints.led_params_base.v_high;

    if let Ok(board) = store.load_board(&project.board.profile) {
        if let Some(v) = board_logic_high_v(&board) {
            v_high = v;
            hints.led_params_base.v_high = v;
        }
    }

    // Always seed `"led"` defaults (C01 / fallback).
    if let Ok(led) = store.load_component("led") {
        if let Some(role) = sim_role_from_doc(&led) {
            hints.roles.insert("led".into(), role);
        }
        if let Some(params) = led.rest.get("params").and_then(|v| v.as_table()) {
            hints.led_params_base = defaults_from_profile_params(params, v_high);
            hints
                .led_params_by_profile
                .insert("led".into(), hints.led_params_base);
        }
    }

    for comp in &project.components {
        let Ok(doc) = store.load_component(&comp.profile) else {
            continue;
        };
        if let Some(role) = sim_role_from_doc(&doc) {
            hints.roles.insert(comp.profile.clone(), role);
            if role == SimRole::LedLoad {
                let params = doc
                    .rest
                    .get("params")
                    .and_then(|v| v.as_table())
                    .map(|t| defaults_from_profile_params(t, v_high))
                    .unwrap_or(LedElectricalParams {
                        v_high,
                        ..LedElectricalParams::PROFILE_DEFAULTS
                    });
                hints
                    .led_params_by_profile
                    .insert(comp.profile.clone(), params);
            }
        }
    }

    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_parse::{parse_path, ParsedDocument};
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn c01_hints_use_sim_role_not_substrings() {
        let path = repo().join("content/fixtures/canvas/C01-button-led.toml");
        let doc = parse_path(&path).unwrap();
        let project = match doc {
            ParsedDocument::Project(p) => p.project,
            _ => panic!("expected project"),
        };
        let hints = load_project_sim_hints(&repo(), &project);
        assert_eq!(hints.role_of("led"), Some(SimRole::LedLoad));
        assert_eq!(
            hints.role_of("cmd-pushbutton-no-start"),
            Some(SimRole::MomentaryContact)
        );
        assert!((hints.led_params_base.v_high - 5.0).abs() < 1e-9);
    }

    #[test]
    fn pilot_params_come_from_own_profile_at_board_v_high() {
        let store = ProfileStore::from_repo_root(repo());
        let mut project = match parse_path(
            &repo().join("content/fixtures/canvas/C01-button-led.toml"),
        )
        .unwrap()
        {
            ParsedDocument::Project(p) => p.project,
            _ => panic!("expected project"),
        };
        // Pretend S7 + pilot so v_high=24 and load from signal-pilot-lamp.
        project.board.profile = "siemens-s7-1200-cpu-1214c".into();
        project.components[0].profile = "signal-pilot-lamp".into();
        let hints = load_project_sim_hints_from_store(&store, &project);
        assert_eq!(hints.role_of("signal-pilot-lamp"), Some(SimRole::LedLoad));
        let p = hints.led_params_for_profile("signal-pilot-lamp");
        assert!((p.v_high - 24.0).abs() < 1e-9, "v_high={}", p.v_high);
        assert!((p.vf - 2.0).abs() < 1e-9);
        assert!((p.r_series - 1100.0).abs() < 1e-9);
        assert!((p.i_max - 0.03).abs() < 1e-9);
    }
}
