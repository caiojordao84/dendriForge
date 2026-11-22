//! Live SVG simulation canvas for the visual MVP set.
//!
//! Binds `dendri-sim` pin/scan state to SVG class hooks on:
//! - board `arduino-uno-r3` (`#led-pin13`, digital pin pads)
//! - component `led` (`#led-body`)
//! - component `cmd-pushbutton-no-start` (`#actuator`, `#contact`)
//!
//! Headless CI uses deterministic [`VisualFrame`] text snapshots; the iced
//! editor consumes the same binding and rendered SVG strings.
//!
//! Simulation view uses a **world + camera** ([`camera`], [`world`]) with an
//! infinite line grid under iced: 1 u = 4 mm → 48 px @ 100%, terminals on
//! full-module intersections. Headless [`scene`] SVG remains for CI snapshots.
//!
//! ## Phase B0 foundations
//! - [`svg_dyn`] — **usvg** parse + filter by stable `id`
//! - [`interpolate`] — **lilt** Instant/springs wrapper
//! - [`routing`] — **arqlet-routing** orthogonal channel smoke
//! - [`svg_contract`] — 2 mm / 24 px size rule + dynamic-layer ids
//!
//! Lyon path conversion uses iced’s re-export
//! `iced::widget::canvas::path::lyon_path` in `dendri-editor` (canvas feature).

pub mod camera;
pub mod endpoint;
pub mod grid;
pub mod interpolate;
pub mod led_model;
pub mod routing;
pub mod scene;
pub mod sim_bridge;
pub mod sim_role;
pub mod svg_contract;
pub mod svg_dyn;
pub mod wire_activity;
pub mod wires;
pub mod world;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use dendri_ast::{SimulationProject, Wire};
use dendri_profiles::{ProfileStore, SvgHook, VisualMvp};
use dendri_sim::{SimConfig, Simulator};

pub use camera::{Camera, WorldPoint, WorldRect, WorldSize, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};
pub use grid::{
    default_scene_px, grid_to_mm, grid_to_px, mm_to_grid, px_per_grid, px_to_grid,
    snap_half_module, snap_terminal, GRID_UNIT_MM, HALF_MODULE, PX_PER_GRID_AT_100,
    PX_PER_HALF_AT_100,
};
pub use scene::{render_live_scene_svg, C01Layout, LiveSceneState};
pub use routing::{manhattan_l, manhattan_via, route_pin_to_pin, rubber_band};
pub use wire_activity::{
    activity_animation_enabled, endpoint_logical_level, wire_logical_activity, WireLogicalActivity,
};
pub use wires::{
    pick_wire, route_wires, waypoint_from_click, wire_color_kind, RoutedWire, TerminalRef,
    WireColorKind, PIN_SNAP_SCREEN_PX, WIRE_BASE_WIDTH_PX, WIRE_COLOR_WIDTH_PX,
    WIRE_FILLET_SCREEN_PX, WIRE_HIT_SCREEN_PX,
};
pub use endpoint::split_endpoint;
pub use led_model::{
    amperes_to_ua, defaults_from_profile_params, evaluate_led, ua_to_amperes, LedElectricalParams,
    LedElectricalState,
};
pub use sim_bridge::{
    apply_sim_command, build_runtime_snapshot, build_runtime_snapshot_ex,
    resolve_terminal_bindings, resolve_terminal_bindings_with_hints, update_led_burn_state,
    update_led_burn_state_with_hints, ComponentRuntimeState, ControlOutcome, RuntimeAvailability,
    RuntimeSnapshot, SimCommand, SimRunState, SnapshotExtras, TerminalBinding, TerminalKind,
};
pub use sim_role::{
    board_logic_high_v, load_project_sim_hints, load_project_sim_hints_from_store,
    sim_role_from_doc, ProjectSimHints, SimRole,
};
pub use svg_dyn::{
    parse_svg_str, prepare_hook, HookCircle, HookSeg, PreparedHook,
};
pub use world::{
    LedPlaceholder, LocalTerminal, MvpProfile, PartKind, PhaseAScene, Scene, ScenePart,
    Terminal, TerminalRole, UnoPlaceholder,
};

/// Canonical asset roots (prefer staged `content/assets/svg/`, fall back to `core/`).
pub const ASSET_BOARD: &str = "content/assets/svg/boards/arduino-uno-r3.svg";
pub const ASSET_LED: &str = "content/assets/svg/components/led.svg";
pub const ASSET_BUTTON: &str = "content/assets/svg/components/cmd-pushbutton-no-start.svg";
pub const CORE_BOARD: &str = "core/boards/arduino-uno-r3.svg";
pub const CORE_LED: &str = "core/components/led.svg";
pub const CORE_BUTTON: &str = "core/components/cmd-pushbutton-no-start.svg";

/// Default MVP pin map from scenario-1 / P01 wiring.
pub const DEFAULT_BUTTON_PIN: u32 = 2;
pub const DEFAULT_LED_PIN: u32 = 13;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinBinding {
    pub button_pin: u32,
    pub led_pin: u32,
    /// True when the button input uses MCU pull-up (pressed reads LOW).
    pub button_active_low: bool,
}

impl Default for PinBinding {
    fn default() -> Self {
        Self {
            button_pin: DEFAULT_BUTTON_PIN,
            led_pin: DEFAULT_LED_PIN,
            button_active_low: false,
        }
    }
}

impl PinBinding {
    /// Infer GPIO pins from project wiring (`board.D13` → LED, `board.D2` → button).
    pub fn from_project(project: &SimulationProject) -> Self {
        let mut binding = Self::default();
        for w in &project.wiring {
            if let Some(pin) = parse_board_digital_pin(&w.from).or_else(|| parse_board_digital_pin(&w.to))
            {
                if wiring_touches(w, "led") {
                    binding.led_pin = pin;
                }
                if wiring_touches(w, "btn") || wiring_touches(w, "button") || wiring_touches(w, "push")
                {
                    binding.button_pin = pin;
                }
            }
        }
        // Also honour hardware IO roles when wiring is sparse.
        for hw in &project.asl.hardware {
            let role = hw.target_role.to_ascii_lowercase();
            let label = hw.label.to_ascii_lowercase();
            if role.contains("out") || label.contains("led") {
                binding.led_pin = hw.pin;
            }
            if role.contains("in") || label.contains("btn") || label.contains("button") {
                binding.button_pin = hw.pin;
                binding.button_active_low = matches!(hw.pull, dendri_ast::PullMode::Pullup);
            }
        }
        binding
    }

    /// Electrical level driven onto the DI pin for a logical press/release.
    /// Pull-up buttons to GND are active-low (pressed → LOW).
    pub fn button_pin_level(&self, pressed: bool) -> bool {
        if self.button_active_low {
            !pressed
        } else {
            pressed
        }
    }
}

fn wiring_touches(w: &Wire, needle: &str) -> bool {
    let n = needle.to_ascii_lowercase();
    w.from.to_ascii_lowercase().contains(&n) || w.to.to_ascii_lowercase().contains(&n)
}

fn parse_board_digital_pin(endpoint: &str) -> Option<u32> {
    let lower = endpoint.to_ascii_lowercase();
    // board.D13 / board.d2
    if let Some(rest) = lower.strip_prefix("board.d") {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        return digits.parse().ok();
    }
    None
}

/// One deterministic visual/sim frame for snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualFrame {
    pub cycle: u64,
    pub time_ms: u64,
    pub button_pressed: bool,
    pub led_on: bool,
    pub pins: BTreeMap<u32, bool>,
}

impl VisualFrame {
    pub fn from_sim(sim: &Simulator, binding: &PinBinding, button_pressed: bool) -> Self {
        let mut pins = BTreeMap::new();
        for (k, v) in sim.pins() {
            pins.insert(*k, *v);
        }
        Self {
            cycle: sim.cycle(),
            time_ms: sim.time_ms(),
            button_pressed,
            led_on: sim.pin(binding.led_pin),
            pins,
        }
    }

    /// Stable multi-line text for CI snapshots.
    pub fn to_snapshot(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "cycle={} t_ms={} button={} led={}",
            self.cycle,
            self.time_ms,
            if self.button_pressed {
                "pressed"
            } else {
                "released"
            },
            if self.led_on { "on" } else { "off" }
        ));
        for (pin, val) in &self.pins {
            lines.push(format!("pin.{pin}={}", if *val { 1 } else { 0 }));
        }
        lines.push(format!(
            "svg.led-body={}",
            if self.led_on { "on" } else { "off" }
        ));
        lines.push(format!(
            "svg.actuator={}",
            if self.button_pressed {
                "pressed"
            } else {
                "released"
            }
        ));
        lines.push(format!(
            "svg.contact={}",
            if self.button_pressed { "closed" } else { "open" }
        ));
        lines.push(format!(
            "svg.board.led-pin13={}",
            if self.led_on { "on" } else { "off" }
        ));
        lines.join("\n") + "\n"
    }
}

#[derive(Debug, Clone)]
pub struct LoadedSvgs {
    pub board: String,
    pub led: String,
    pub button: String,
    pub board_path: PathBuf,
    pub led_path: PathBuf,
    pub button_path: PathBuf,
}

impl LoadedSvgs {
    pub fn load(repo_root: impl AsRef<Path>) -> Result<Self, String> {
        let root = repo_root.as_ref();
        let board_path = resolve_asset(root, ASSET_BOARD, CORE_BOARD)?;
        let led_path = resolve_asset(root, ASSET_LED, CORE_LED)?;
        let button_path = resolve_asset(root, ASSET_BUTTON, CORE_BUTTON)?;
        Ok(Self {
            board: fs::read_to_string(&board_path).map_err(|e| e.to_string())?,
            led: fs::read_to_string(&led_path).map_err(|e| e.to_string())?,
            button: fs::read_to_string(&button_path).map_err(|e| e.to_string())?,
            board_path,
            led_path,
            button_path,
        })
    }

    pub fn assert_required_ids(&self) -> Result<(), String> {
        require_id(&self.led, "led-body")?;
        require_id(&self.button, "actuator")?;
        require_id(&self.button, "contact")?;
        require_id(&self.board, "led-pin13")?;
        Ok(())
    }
}

fn resolve_asset(root: &Path, preferred: &str, fallback: &str) -> Result<PathBuf, String> {
    let a = root.join(preferred);
    if a.exists() {
        return Ok(a);
    }
    let b = root.join(fallback);
    if b.exists() {
        return Ok(b);
    }
    Err(format!(
        "SVG missing: tried {} and {}",
        a.display(),
        b.display()
    ))
}

fn require_id(svg: &str, id: &str) -> Result<(), String> {
    let needle = format!("id=\"{id}\"");
    if svg.contains(&needle) {
        Ok(())
    } else {
        Err(format!("SVG missing required id={id}"))
    }
}

/// Apply sim visual state as CSS classes / data-state on known SVG hooks.
pub fn render_led_svg(template: &str, led_on: bool) -> String {
    let state = if led_on { "on" } else { "off" };
    let mut out = set_element_class(template, "led-body", &["df-component-die", state]);
    out = set_css_var(&out, "--df-state", state);
    out = set_css_var(
        &out,
        "--df-state-opacity",
        if led_on { "1" } else { "0.35" },
    );
    out
}

pub fn render_button_svg(template: &str, pressed: bool) -> String {
    let mut out = if pressed {
        set_element_class(template, "actuator", &["pressed"])
    } else {
        // clear pressed: keep empty class list meaningful for snapshot checks
        set_element_class(template, "actuator", &[])
    };
    out = if pressed {
        set_element_class(&out, "contact", &["closed"])
    } else {
        set_element_class(&out, "contact", &["open"])
    };
    out
}

pub fn render_board_svg(template: &str, led_pin_high: bool) -> String {
    let state = if led_pin_high { "on" } else { "off" };
    let mut out = set_data_attr(template, "led-pin13", "data-state", state);
    let classes = if led_pin_high {
        ["led", "led-orange", "on"]
    } else {
        ["led", "led-orange", "off"]
    };
    out = set_element_class(&out, "led-pin13", &classes);
    // Mark D13 pad group when high (id contains spaces in source).
    if led_pin_high {
        out = out.replacen(
            "id=\"D13 / LED / SCK\"",
            "id=\"D13 / LED / SCK\" data-sim=\"high\"",
            1,
        );
    } else {
        out = out.replacen(
            "id=\"D13 / LED / SCK\" data-sim=\"high\"",
            "id=\"D13 / LED / SCK\"",
            1,
        );
        if !out.contains("id=\"D13 / LED / SCK\"") {
            // leave unchanged if pad id differs
        }
    }
    out
}

fn set_css_var(svg: &str, name: &str, value: &str) -> String {
    // Replace first `--name: …;` declaration.
    let key = format!("{name}:");
    let Some(idx) = svg.find(&key) else {
        return svg.to_string();
    };
    let after = &svg[idx + key.len()..];
    let Some(semi) = after.find(';') else {
        return svg.to_string();
    };
    format!("{}{} {};{}", &svg[..idx], name, value, &after[semi + 1..])
}

fn set_data_attr(svg: &str, id: &str, attr: &str, value: &str) -> String {
    let marker = format!("id=\"{id}\"");
    let Some(start) = svg.find(&marker) else {
        return svg.to_string();
    };
    // Find end of opening tag after id.
    let after_id = start + marker.len();
    let Some(rel_gt) = svg[after_id..].find('>') else {
        return svg.to_string();
    };
    let tag_end = after_id + rel_gt;
    let open = &svg[start..tag_end];
    let attr_pat = format!("{attr}=\"");
    let rewritten = if let Some(a) = open.find(&attr_pat) {
        let val_start = a + attr_pat.len();
        let val_end = open[val_start..]
            .find('"')
            .map(|i| val_start + i)
            .unwrap_or(open.len());
        format!("{}{}{}\"{}\"", &open[..a], attr_pat, value, &open[val_end + 1..])
    } else {
        format!("{open} {attr}=\"{value}\"")
    };
    format!("{}{}{}", &svg[..start], rewritten, &svg[tag_end..])
}

fn set_element_class(svg: &str, id: &str, classes: &[&str]) -> String {
    let marker = format!("id=\"{id}\"");
    let Some(id_pos) = svg.find(&marker) else {
        return svg.to_string();
    };
    // Search backwards / forwards within the same tag for class="..."
    let tag_start = svg[..id_pos].rfind('<').unwrap_or(id_pos);
    let Some(rel_gt) = svg[id_pos..].find('>') else {
        return svg.to_string();
    };
    let tag_end = id_pos + rel_gt;
    let tag = &svg[tag_start..tag_end];
    let class_value = classes.join(" ");
    let new_tag = if let Some(c) = tag.find("class=\"") {
        let val_start = c + "class=\"".len();
        let val_end = tag[val_start..]
            .find('"')
            .map(|i| val_start + i)
            .unwrap_or(tag.len());
        format!(
            "{}class=\"{}\"{}",
            &tag[..c],
            class_value,
            &tag[val_end + 1..]
        )
    } else if class_value.is_empty() {
        tag.to_string()
    } else {
        format!("{tag} class=\"{class_value}\"")
    };
    format!("{}{}{}", &svg[..tag_start], new_tag, &svg[tag_end..])
}

/// Interactive canvas session: button → sim input, LED ← sim output.
#[derive(Debug)]
pub struct SimCanvas {
    pub sim: Simulator,
    pub binding: PinBinding,
    pub svgs: LoadedSvgs,
    pub button_pressed: bool,
    pub frames: Vec<VisualFrame>,
}

impl SimCanvas {
    pub fn from_project(
        project: &SimulationProject,
        repo_root: impl AsRef<Path>,
        config: &SimConfig,
    ) -> Result<Self, String> {
        let binding = PinBinding::from_project(project);
        let mut cfg = config.clone();
        cfg.initial_pins
            .entry(binding.button_pin)
            .or_insert(false);
        cfg.initial_pins.entry(binding.led_pin).or_insert(false);
        let sim = Simulator::from_project(project, &cfg);
        let svgs = LoadedSvgs::load(repo_root)?;
        svgs.assert_required_ids()?;
        Ok(Self {
            sim,
            binding,
            svgs,
            button_pressed: false,
            frames: Vec::new(),
        })
    }

    pub fn boot(&mut self) -> Result<VisualFrame, String> {
        self.sim.ensure_setup()?;
        // Idle at the released electrical level (HIGH when pull-up / active-low).
        self.button_pressed = false;
        let _ = self.sim.set_pin(
            self.binding.button_pin,
            self.binding.button_pin_level(false),
        );
        // Live sessions must not accumulate unbounded frames (C0).
        // Tests that need a history call [`Self::record_frame`] / [`Self::step_and_record`].
        Ok(self.current_frame())
    }

    pub fn press_button(&mut self) {
        self.button_pressed = true;
        let _ = self.sim.set_pin(
            self.binding.button_pin,
            self.binding.button_pin_level(true),
        );
    }

    pub fn release_button(&mut self) {
        self.button_pressed = false;
        let _ = self.sim.set_pin(
            self.binding.button_pin,
            self.binding.button_pin_level(false),
        );
    }

    pub fn set_button(&mut self, pressed: bool) {
        if pressed {
            self.press_button();
        } else {
            self.release_button();
        }
    }

    /// Advance one scan cycle. Does **not** append to [`Self::frames`] (live-safe).
    pub fn step(&mut self) -> Result<VisualFrame, String> {
        self.sim.step()?;
        Ok(self.current_frame())
    }

    /// Like [`Self::step`], then append to the explicit capture history (tests / CLI).
    pub fn step_and_record(&mut self) -> Result<VisualFrame, String> {
        let frame = self.step()?;
        self.record_frame(frame.clone());
        Ok(frame)
    }

    /// Append a frame to the explicit history (tests / CLI capture only).
    pub fn record_frame(&mut self, frame: VisualFrame) {
        self.frames.push(frame);
    }

    /// Capture current state into the history buffer (explicit; not used by live editor).
    pub fn capture(&mut self) -> VisualFrame {
        let frame = self.current_frame();
        self.record_frame(frame.clone());
        frame
    }

    /// Clear the optional capture history (keeps runtime state).
    pub fn clear_frame_history(&mut self) {
        self.frames.clear();
    }

    pub fn current_frame(&self) -> VisualFrame {
        VisualFrame::from_sim(&self.sim, &self.binding, self.button_pressed)
    }

    /// Shared runtime snapshot for WorldCanvas / headless consumers (D-C7).
    pub fn runtime_snapshot(
        &self,
        project: &SimulationProject,
        run_state: SimRunState,
        button_pressed_by_instance: &BTreeMap<String, bool>,
        present_gen: u64,
    ) -> RuntimeSnapshot {
        build_runtime_snapshot(
            run_state,
            RuntimeAvailability::Available,
            Some(&self.sim),
            Some(project),
            button_pressed_by_instance,
            present_gen,
        )
    }

    /// C1 snapshot with latch / burn / LED params.
    pub fn runtime_snapshot_ex(
        &self,
        project: &SimulationProject,
        run_state: SimRunState,
        extras: SnapshotExtras<'_>,
        present_gen: u64,
    ) -> RuntimeSnapshot {
        build_runtime_snapshot_ex(
            run_state,
            RuntimeAvailability::Available,
            Some(&self.sim),
            Some(project),
            extras,
            present_gen,
        )
    }

    pub fn rendered_led(&self) -> String {
        render_led_svg(&self.svgs.led, self.sim.pin(self.binding.led_pin))
    }

    pub fn rendered_button(&self) -> String {
        render_button_svg(&self.svgs.button, self.button_pressed)
    }

    pub fn rendered_board(&self) -> String {
        render_board_svg(&self.svgs.board, self.sim.pin(self.binding.led_pin))
    }

    /// Live logical-grid scene (Simulation center). Zoom is applied at render.
    pub fn live_scene_state(&self) -> LiveSceneState {
        LiveSceneState {
            led_on: self.sim.pin(self.binding.led_pin),
            button_pressed: self.button_pressed,
        }
    }

    pub fn rendered_live_scene(&self, zoom: f64) -> String {
        render_live_scene_svg(&self.live_scene_state(), zoom)
    }

    /// Concatenate frames for multi-step snapshot files.
    pub fn frames_snapshot(&self) -> String {
        let mut out = String::new();
        for (i, f) in self.frames.iter().enumerate() {
            out.push_str(&format!("# frame {i}\n"));
            out.push_str(&f.to_snapshot());
            out.push('\n');
        }
        out
    }
}

/// Profile + SVG presence report for CLI / DEN-14 migration notes.
#[derive(Debug, Clone)]
pub struct MvpAssetReport {
    pub mvp: VisualMvp,
    pub resolved_board: PathBuf,
    pub resolved_led: PathBuf,
    pub resolved_button: PathBuf,
    pub preferred_paths_present: bool,
}

pub fn report_mvp_assets(repo_root: impl AsRef<Path>) -> Result<MvpAssetReport, String> {
    let root = repo_root.as_ref();
    let store = ProfileStore::from_repo_root(root);
    let mvp = store.load_visual_mvp().map_err(|e| e.to_string())?;
    let svgs = LoadedSvgs::load(root)?;
    svgs.assert_required_ids()?;
    let preferred = root.join(ASSET_BOARD).exists()
        && root.join(ASSET_LED).exists()
        && root.join(ASSET_BUTTON).exists();
    Ok(MvpAssetReport {
        mvp,
        resolved_board: svgs.board_path,
        resolved_led: svgs.led_path,
        resolved_button: svgs.button_path,
        preferred_paths_present: preferred,
    })
}

pub fn hook_summary(hook: &Option<SvgHook>) -> String {
    match hook {
        Some(h) => format!(
            "path={} present={} ids={:?}",
            h.relative_path, h.present, h.required_ids
        ),
        None => "missing hook".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_parse::parse_path;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn loads_mvp_svgs_with_required_ids() {
        let svgs = LoadedSvgs::load(repo()).unwrap();
        svgs.assert_required_ids().unwrap();
        assert!(svgs.led.contains("id=\"led-body\""));
        assert!(svgs.button.contains("id=\"actuator\""));
    }

    #[test]
    fn render_toggles_led_and_button_classes() {
        let svgs = LoadedSvgs::load(repo()).unwrap();
        let on = render_led_svg(&svgs.led, true);
        assert!(on.contains("id=\"led-body\""));
        assert!(on.contains("class=\"df-component-die on\"") || on.contains(" on\""));
        let pressed = render_button_svg(&svgs.button, true);
        assert!(pressed.contains("pressed"));
        assert!(pressed.contains("closed"));
    }

    #[test]
    fn binding_from_p01_wiring() {
        let path = repo().join("content/fixtures/projects/P01-uno-led-blink.toml");
        let doc = parse_path(&path).unwrap();
        let project = match doc {
            dendri_parse::ParsedDocument::Project(p) => p.project,
            _ => panic!("expected project"),
        };
        let b = PinBinding::from_project(&project);
        assert_eq!(b.led_pin, 13);
        assert_eq!(b.button_pin, 2);
    }
}
