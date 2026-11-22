//! Phase C0/C1 — runtime bridge contract shared by WorldCanvas and headless consumers.
//!
//! ```text
//! Runtime / snapshot
//!        ├── WorldCanvas live
//!        └── Headless / CI snapshots
//! ```
//!
//! Control semantics follow plan D-C2. Per-instance / terminal bindings avoid
//! sole `button_pin` / `led_pin` coupling (GPIO high ≠ correctly wired).
//! C1: LED DC current + burned (D-C3) live in the snapshot; canvas only presents.

use std::collections::BTreeMap;

use dendri_ast::{SimulationProject, Wire};
use dendri_sim::Simulator;

use crate::endpoint::split_endpoint;
use crate::led_model::{
    amperes_to_ua, evaluate_led, ua_to_amperes, LedElectricalParams,
};
use crate::sim_role::{ProjectSimHints, SimRole};
use crate::PinBinding;

/// Editor / bridge run mode (D-C2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimRunState {
    /// Stopped; pin/var values may still be preserved from the last session.
    #[default]
    Idle,
    /// Continuously advancing scans on the runtime clock.
    Running,
    /// Suspended; state preserved (including Shift+LMB latches when present).
    Paused,
}

impl SimRunState {
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Layout / wire / component mutations are blocked while Running (D-C1).
    pub fn edits_allowed(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// Whether a live runtime session is usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeAvailability {
    Available,
    /// No runtime session (project not loaded / boot skipped).
    Unavailable,
    /// Runtime failed; message is user-visible. No fabricated cycles.
    Error(String),
}

impl RuntimeAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Error(m) => Some(m.as_str()),
            _ => None,
        }
    }
}

/// Shared Play / Pause / Stop / Reset / Step commands (toolbar + context menus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimCommand {
    Play,
    Pause,
    Stop,
    Reset,
    Step,
}

/// Pure control-transition result (testable without iced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlOutcome {
    pub run_state: SimRunState,
    /// Cancel momentary button press (Pause / Stop / Reset).
    pub clear_momentary: bool,
    /// Clear all Shift+LMB interaction latches (Stop / Reset).
    pub clear_latches: bool,
    /// Reboot runtime from project (Reset only).
    pub reboot_runtime: bool,
    /// Advance exactly one scan + one presentation update (Step).
    pub step_once: bool,
}

/// Apply D-C2 control semantics. Does not touch the simulator itself.
pub fn apply_sim_command(current: SimRunState, cmd: SimCommand) -> ControlOutcome {
    match cmd {
        SimCommand::Play => ControlOutcome {
            run_state: SimRunState::Running,
            clear_momentary: false,
            clear_latches: false,
            reboot_runtime: false,
            step_once: false,
        },
        SimCommand::Pause => ControlOutcome {
            run_state: SimRunState::Paused,
            clear_momentary: true,
            clear_latches: false,
            reboot_runtime: false,
            step_once: false,
        },
        SimCommand::Stop => ControlOutcome {
            run_state: SimRunState::Idle,
            clear_momentary: true,
            clear_latches: true,
            reboot_runtime: false,
            step_once: false,
        },
        SimCommand::Reset => ControlOutcome {
            // Reset restores initial project state; keep run mode so Play-after-Reset
            // is not implied, but continuous Running resumes from a clean boot.
            run_state: current,
            clear_momentary: true,
            clear_latches: true,
            reboot_runtime: true,
            step_once: false,
        },
        SimCommand::Step => ControlOutcome {
            // Step suspends continuous run (same as Pause) then advances once.
            run_state: SimRunState::Paused,
            clear_momentary: false,
            clear_latches: false,
            reboot_runtime: false,
            step_once: true,
        },
    }
}

/// Role of a terminal in the runtime snapshot (presentation hooks for C1+).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    BoardDigital,
    LedAnode,
    LedCathode,
    ButtonContact,
    Other,
}

/// One instance terminal → optional board GPIO binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalBinding {
    pub instance_id: String,
    pub terminal: String,
    pub kind: TerminalKind,
    /// Board digital pin when wired to `board.D*`; `None` if unwired / non-GPIO.
    pub board_pin: Option<u32>,
}

/// Per-instance presentation slice of the runtime snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentRuntimeState {
    pub instance_id: String,
    pub profile: String,
    /// Lit when forward current > 0 and not burned (presentation helper).
    pub led_on: Option<bool>,
    /// Forward current in microamperes (µA). Canvas presents; never recomputes.
    pub led_current_ua: Option<i64>,
    /// Burned until Reset (open circuit after burn scan).
    pub led_burned: Option<bool>,
    /// Button actuation (UI press / latch), not GPIO high alone.
    pub button_pressed: Option<bool>,
    /// True when this button is Shift+LMB latched (UI-only hold indicator).
    pub button_latched: Option<bool>,
    /// Terminal port → logical pin level when bound to a board GPIO.
    pub terminals: BTreeMap<String, Option<bool>>,
}

impl ComponentRuntimeState {
    /// Forward current in amperes (LED only).
    pub fn led_current_a(&self) -> Option<f64> {
        self.led_current_ua.map(ua_to_amperes)
    }
}

/// Snapshot consumed by WorldCanvas and (optionally) headless CI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub run_state: SimRunState,
    pub availability: RuntimeAvailability,
    pub cycle: u64,
    pub time_ms: u64,
    pub pins: BTreeMap<u32, bool>,
    pub bindings: Vec<TerminalBinding>,
    pub components: BTreeMap<String, ComponentRuntimeState>,
    /// Presentation generation — bumped on scan / control / input changes.
    pub present_gen: u64,
}

impl RuntimeSnapshot {
    pub fn unavailable(run_state: SimRunState) -> Self {
        Self {
            run_state,
            availability: RuntimeAvailability::Unavailable,
            cycle: 0,
            time_ms: 0,
            pins: BTreeMap::new(),
            bindings: Vec::new(),
            components: BTreeMap::new(),
            present_gen: 0,
        }
    }

    pub fn error(run_state: SimRunState, message: impl Into<String>) -> Self {
        Self {
            run_state,
            availability: RuntimeAvailability::Error(message.into()),
            cycle: 0,
            time_ms: 0,
            pins: BTreeMap::new(),
            bindings: Vec::new(),
            components: BTreeMap::new(),
            present_gen: 0,
        }
    }

    /// Any LED with non-zero forward current and not burned.
    pub fn any_led_on(&self) -> bool {
        self.components
            .values()
            .any(|c| c.led_on == Some(true))
    }

    /// Any LED marked burned in the snapshot.
    pub fn any_led_burned(&self) -> bool {
        self.components
            .values()
            .any(|c| c.led_burned == Some(true))
    }

    /// Any button currently marked pressed in the snapshot.
    pub fn any_button_pressed(&self) -> bool {
        self.components
            .values()
            .any(|c| c.button_pressed == Some(true))
    }
}

/// Resolve per-instance / terminal bindings from project wiring (no role map).
///
/// Prefer [`resolve_terminal_bindings_with_hints`] so `sim_role` drives LED /
/// button terminal kinds (D-comp-14).
pub fn resolve_terminal_bindings(project: &SimulationProject) -> Vec<TerminalBinding> {
    resolve_terminal_bindings_with_hints(project, &ProjectSimHints::default())
}

/// Resolve bindings using `[dendri].sim_role` when available.
pub fn resolve_terminal_bindings_with_hints(
    project: &SimulationProject,
    hints: &ProjectSimHints,
) -> Vec<TerminalBinding> {
    let mut out = Vec::new();

    // Board digital pads from wiring endpoints + defaults.
    let mut board_ports: BTreeMap<String, Option<u32>> = BTreeMap::new();
    for w in &project.wiring {
        push_endpoint_port(&mut board_ports, &w.from);
        push_endpoint_port(&mut board_ports, &w.to);
    }
    // Ensure common MVP pads exist even if unwired.
    for (port, pin) in [("D13", Some(13u32)), ("D2", Some(2u32))] {
        board_ports.entry(port.into()).or_insert(pin);
    }
    let board_id = project.board.instance_id.clone();
    for (port, pin) in board_ports {
        if parse_board_digital_pin_label(&port).is_some() || pin.is_some() {
            let pin = pin.or_else(|| parse_board_digital_pin_label(&port));
            out.push(TerminalBinding {
                instance_id: board_id.clone(),
                terminal: port,
                kind: TerminalKind::BoardDigital,
                board_pin: pin,
            });
        }
    }

    for comp in &project.components {
        let role = hints.role_of(&comp.profile);
        let terminals: Vec<&str> = match role {
            Some(SimRole::LedLoad) => vec!["A", "K"],
            Some(SimRole::MomentaryContact) => {
                // Industrial NO/COM/NC; maker tactile uses A/B (and numbered legs).
                if comp.profile == "maker-tactile-4pin" {
                    vec!["A", "B", "1", "2", "3", "4"]
                } else {
                    vec!["NO", "COM", "NC"]
                }
            }
            Some(SimRole::AdcSource) => {
                // Stub recognition only (D2 owns analog path).
                Vec::new()
            }
            None => {
                // Legacy fallback when hints were not loaded (unit tests).
                match comp.profile.as_str() {
                    "led" => vec!["A", "K"],
                    "cmd-pushbutton-no-start" | "cmd-pushbutton-nc-stop" => {
                        vec!["NO", "COM", "NC"]
                    }
                    _ => Vec::new(),
                }
            }
        };
        let kind_for = |term: &str| -> TerminalKind {
            match role {
                Some(SimRole::LedLoad) => match term {
                    "A" => TerminalKind::LedAnode,
                    "K" => TerminalKind::LedCathode,
                    _ => TerminalKind::Other,
                },
                Some(SimRole::MomentaryContact) => TerminalKind::ButtonContact,
                _ => match (comp.profile.as_str(), term) {
                    ("led", "A") => TerminalKind::LedAnode,
                    ("led", "K") => TerminalKind::LedCathode,
                    ("cmd-pushbutton-no-start" | "cmd-pushbutton-nc-stop", _) => {
                        TerminalKind::ButtonContact
                    }
                    _ => TerminalKind::Other,
                },
            }
        };
        if terminals.is_empty() {
            // Still record instance with no terminals so snapshot lists it.
            out.push(TerminalBinding {
                instance_id: comp.instance_id.clone(),
                terminal: String::new(),
                kind: TerminalKind::Other,
                board_pin: None,
            });
            continue;
        }
        for term in terminals {
            let endpoint = format!("{}.{}", comp.instance_id, term);
            let board_pin = find_board_pin_for_endpoint(&project.wiring, &endpoint);
            out.push(TerminalBinding {
                instance_id: comp.instance_id.clone(),
                terminal: term.into(),
                kind: kind_for(term),
                board_pin,
            });
        }
    }
    out
}

fn push_endpoint_port(map: &mut BTreeMap<String, Option<u32>>, endpoint: &str) {
    let Some((inst, port)) = split_endpoint(endpoint) else {
        return;
    };
    if !inst.eq_ignore_ascii_case("board") {
        return;
    }
    let pin = parse_board_digital_pin_label(port);
    // Keep dotted port ids intact (e.g. Ia.0); only upper-case for D* labels.
    let key = if port.contains('.') {
        port.to_string()
    } else {
        port.to_ascii_uppercase()
    };
    map.entry(key)
        .and_modify(|p| {
            if p.is_none() {
                *p = pin;
            }
        })
        .or_insert(pin);
}

fn parse_board_digital_pin_label(port: &str) -> Option<u32> {
    let lower = port.to_ascii_lowercase();
    let rest = lower.strip_prefix('d')?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn find_board_pin_for_endpoint(wiring: &[Wire], endpoint: &str) -> Option<u32> {
    let target = endpoint.to_ascii_lowercase();
    for w in wiring {
        let from = w.from.to_ascii_lowercase();
        let to = w.to.to_ascii_lowercase();
        if from == target {
            if let Some(pin) = parse_board_digital_from_endpoint(&w.to) {
                return Some(pin);
            }
        }
        if to == target {
            if let Some(pin) = parse_board_digital_from_endpoint(&w.from) {
                return Some(pin);
            }
        }
    }
    None
}

fn parse_board_digital_from_endpoint(endpoint: &str) -> Option<u32> {
    let lower = endpoint.to_ascii_lowercase();
    let rest = lower.strip_prefix("board.d")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Optional inputs that refine the snapshot beyond raw pin levels.
#[derive(Debug, Clone)]
pub struct SnapshotExtras<'a> {
    /// Pushbutton instance id → user actuation (momentary ∪ latches).
    pub button_pressed: &'a BTreeMap<String, bool>,
    /// Pushbutton instance id → Shift+LMB latch (UI-only hold).
    pub button_latched: &'a BTreeMap<String, bool>,
    /// LED instance id → burned (persists until Reset).
    pub led_burned: &'a BTreeMap<String, bool>,
    /// Fallback electrical params (board V_high + `"led"` defaults).
    pub led_params_base: LedElectricalParams,
    /// Per-profile LED params + `sim_role` map (D-comp-14). Empty = legacy id checks.
    pub sim_hints: &'a ProjectSimHints,
}

/// Build a runtime snapshot from simulator + project bindings + actuation map.
///
/// LED electrical state (current / burned) is computed here from profile params —
/// never by the canvas. Unbound LEDs stay off (GPIO high alone does not light them).
pub fn build_runtime_snapshot(
    run_state: SimRunState,
    availability: RuntimeAvailability,
    sim: Option<&Simulator>,
    project: Option<&SimulationProject>,
    button_pressed_by_instance: &BTreeMap<String, bool>,
    present_gen: u64,
) -> RuntimeSnapshot {
    let empty_latched = BTreeMap::new();
    let empty_burned = BTreeMap::new();
    let empty_hints = ProjectSimHints::default();
    build_runtime_snapshot_ex(
        run_state,
        availability,
        sim,
        project,
        SnapshotExtras {
            button_pressed: button_pressed_by_instance,
            button_latched: &empty_latched,
            led_burned: &empty_burned,
            led_params_base: LedElectricalParams::PROFILE_DEFAULTS,
            sim_hints: &empty_hints,
        },
        present_gen,
    )
}

/// Full snapshot builder with latch / burn / LED params (C1).
pub fn build_runtime_snapshot_ex(
    run_state: SimRunState,
    availability: RuntimeAvailability,
    sim: Option<&Simulator>,
    project: Option<&SimulationProject>,
    extras: SnapshotExtras<'_>,
    present_gen: u64,
) -> RuntimeSnapshot {
    let Some(project) = project else {
        return match &availability {
            RuntimeAvailability::Error(m) => RuntimeSnapshot::error(run_state, m.clone()),
            _ => RuntimeSnapshot::unavailable(run_state),
        };
    };
    let bindings = resolve_terminal_bindings_with_hints(project, extras.sim_hints);
    let mut pins = BTreeMap::new();
    let (cycle, time_ms) = if let Some(sim) = sim {
        for (k, v) in sim.pins() {
            pins.insert(*k, *v);
        }
        (sim.cycle(), sim.time_ms())
    } else {
        (0, 0)
    };

    let mut components = BTreeMap::new();
    for comp in &project.components {
        let mut terminals = BTreeMap::new();
        let mut led_on = None;
        let mut led_current_ua = None;
        let mut led_burned = None;
        let mut button_pressed = None;
        let mut button_latched = None;
        let role = extras.sim_hints.role_of(&comp.profile);
        let is_led = role == Some(SimRole::LedLoad)
            || (role.is_none() && comp.profile == "led");
        let is_btn = role == Some(SimRole::MomentaryContact)
            || (role.is_none()
                && (comp.profile.contains("button") || comp.profile.contains("push")));
        if is_btn {
            button_pressed = Some(
                extras
                    .button_pressed
                    .get(&comp.instance_id)
                    .copied()
                    .unwrap_or(false),
            );
            button_latched = Some(
                extras
                    .button_latched
                    .get(&comp.instance_id)
                    .copied()
                    .unwrap_or(false),
            );
        }
        if is_led {
            led_on = Some(false);
            led_current_ua = Some(0);
            led_burned = Some(
                extras
                    .led_burned
                    .get(&comp.instance_id)
                    .copied()
                    .unwrap_or(false),
            );
        }
        let mut anode_high: Option<bool> = None;
        let mut anode_wired = false;
        for b in bindings
            .iter()
            .filter(|b| b.instance_id == comp.instance_id && !b.terminal.is_empty())
        {
            let level = b.board_pin.and_then(|p| pins.get(&p).copied());
            terminals.insert(b.terminal.clone(), level);
            if b.kind == TerminalKind::LedAnode {
                anode_wired = b.board_pin.is_some();
                anode_high = level;
            }
        }
        if is_led {
            let already = extras
                .led_burned
                .get(&comp.instance_id)
                .copied()
                .unwrap_or(false);
            let base = extras.sim_hints.led_params_for_profile(&comp.profile);
            let params = LedElectricalParams::resolve(comp, base);
            // Unwired LED: no anode drive → V_pin = 0 (GPIO high alone does not light).
            let v_pin = if anode_wired {
                params.v_pin(anode_high == Some(true))
            } else {
                0.0
            };
            let elec = evaluate_led(&params, v_pin, already);
            // Snapshot of the burn scan reports burned=true with the non-zero I;
            // later scans (already burned) report I=0.
            let report_burned = already || elec.burned;
            led_burned = Some(report_burned);
            led_current_ua = Some(amperes_to_ua(elec.current));
            // Presentation: lit only when current > 0 and not showing burned-dark.
            // Burn scan still has I>0 but must present burned (not overexposed).
            led_on = Some(elec.current > 0.0 && !report_burned);
            if report_burned && elec.current > 0.0 && !already {
                // Burn scan: force presentation to burned (dark), keep current for diagnostics.
                led_on = Some(false);
            }
        }
        components.insert(
            comp.instance_id.clone(),
            ComponentRuntimeState {
                instance_id: comp.instance_id.clone(),
                profile: comp.profile.clone(),
                led_on,
                led_current_ua,
                led_burned,
                button_pressed,
                button_latched,
                terminals,
            },
        );
    }

    // Board as a component-like entry for pin inspection.
    components.insert(
        project.board.instance_id.clone(),
        ComponentRuntimeState {
            instance_id: project.board.instance_id.clone(),
            profile: project.board.profile.clone(),
            led_on: None,
            led_current_ua: None,
            led_burned: None,
            button_pressed: None,
            button_latched: None,
            terminals: bindings
                .iter()
                .filter(|b| b.instance_id == project.board.instance_id)
                .map(|b| {
                    let level = b.board_pin.and_then(|p| pins.get(&p).copied());
                    (b.terminal.clone(), level)
                })
                .collect(),
        },
    );

    RuntimeSnapshot {
        run_state,
        availability,
        cycle,
        time_ms,
        pins,
        bindings,
        components,
        present_gen,
    }
}

/// Advance LED burn state from the current pin levels (call after each scan).
///
/// Returns the updated burned map. Burn scan keeps non-zero I in the snapshot;
/// subsequent calls see `already_burned` and force I=0.
pub fn update_led_burn_state(
    project: &SimulationProject,
    pins: &BTreeMap<u32, bool>,
    burned: &mut BTreeMap<String, bool>,
    params_base: LedElectricalParams,
) {
    update_led_burn_state_with_hints(
        project,
        pins,
        burned,
        params_base,
        &ProjectSimHints::default(),
    );
}

/// Like [`update_led_burn_state`] but uses `sim_role` + per-profile LED params.
pub fn update_led_burn_state_with_hints(
    project: &SimulationProject,
    pins: &BTreeMap<u32, bool>,
    burned: &mut BTreeMap<String, bool>,
    params_base: LedElectricalParams,
    hints: &ProjectSimHints,
) {
    let bindings = resolve_terminal_bindings_with_hints(project, hints);
    for comp in &project.components {
        let role = hints.role_of(&comp.profile);
        let is_led = role == Some(SimRole::LedLoad)
            || (role.is_none() && comp.profile == "led");
        if !is_led {
            continue;
        }
        let already = burned.get(&comp.instance_id).copied().unwrap_or(false);
        if already {
            continue;
        }
        let anode = bindings.iter().find(|b| {
            b.instance_id == comp.instance_id && b.kind == TerminalKind::LedAnode
        });
        let wired = anode.and_then(|b| b.board_pin).is_some();
        let high = anode
            .and_then(|b| b.board_pin)
            .and_then(|p| pins.get(&p).copied())
            .unwrap_or(false);
        let base = if hints.led_params_by_profile.contains_key(&comp.profile) {
            hints.led_params_for_profile(&comp.profile)
        } else {
            params_base
        };
        let params = LedElectricalParams::resolve(comp, base);
        let v_pin = if wired { params.v_pin(high) } else { 0.0 };
        let elec = evaluate_led(&params, v_pin, false);
        if elec.burned {
            burned.insert(comp.instance_id.clone(), true);
        }
    }
}

/// Legacy single-pin helpers still used by headless SVG hooks.
pub fn legacy_led_pin_high(sim: &Simulator, binding: &PinBinding) -> bool {
    sim.pin(binding.led_pin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::led_model::LedElectricalParams;
    use dendri_ast::{ComponentInstance, SimulationProject, Wire};
    use dendri_parse::{parse_path, ParsedDocument};
    use dendri_sim::SimConfig;
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn load_c01() -> SimulationProject {
        let path = repo().join("content/fixtures/canvas/C01-button-led.toml");
        let doc = parse_path(&path).expect("parse");
        match doc {
            ParsedDocument::Project(p) => p.project,
            _ => panic!("expected project"),
        }
    }

    #[test]
    fn control_play_pause_stop_reset_step_semantics() {
        let o = apply_sim_command(SimRunState::Idle, SimCommand::Play);
        assert_eq!(o.run_state, SimRunState::Running);
        assert!(!o.step_once);

        let o = apply_sim_command(SimRunState::Running, SimCommand::Pause);
        assert_eq!(o.run_state, SimRunState::Paused);
        assert!(o.clear_momentary);
        assert!(!o.clear_latches);

        let o = apply_sim_command(SimRunState::Paused, SimCommand::Stop);
        assert_eq!(o.run_state, SimRunState::Idle);
        assert!(o.clear_momentary);
        assert!(o.clear_latches);
        assert!(!o.reboot_runtime);

        let o = apply_sim_command(SimRunState::Idle, SimCommand::Reset);
        assert!(o.reboot_runtime);
        assert!(o.clear_latches);
        assert_eq!(o.run_state, SimRunState::Idle);

        let o = apply_sim_command(SimRunState::Running, SimCommand::Step);
        assert_eq!(o.run_state, SimRunState::Paused);
        assert!(o.step_once);
        assert!(!o.clear_latches);
    }

    #[test]
    fn edits_blocked_only_while_running() {
        assert!(!SimRunState::Running.edits_allowed());
        assert!(SimRunState::Idle.edits_allowed());
        assert!(SimRunState::Paused.edits_allowed());
    }

    #[test]
    fn bindings_are_per_instance_not_sole_globals() {
        let project = load_c01();
        let bindings = resolve_terminal_bindings(&project);
        let led_a = bindings
            .iter()
            .find(|b| b.instance_id.contains("led") && b.terminal == "A")
            .expect("led anode");
        assert_eq!(led_a.board_pin, Some(13));
        let btn = bindings
            .iter()
            .find(|b| {
                (b.instance_id.contains("btn") || b.instance_id.contains("button"))
                    && b.terminal == "NO"
            })
            .expect("button NO");
        assert_eq!(btn.board_pin, Some(2));
        // Unwired synthetic LED must not inherit D13.
        let mut orphan = project.clone();
        orphan.components.push(ComponentInstance {
            instance_id: "led_orphan".into(),
            profile: "led".into(),
            label: None,
            params: Default::default(),
            layout: Default::default(),
        });
        let bindings = resolve_terminal_bindings(&orphan);
        let orphan_a = bindings
            .iter()
            .find(|b| b.instance_id == "led_orphan" && b.terminal == "A")
            .unwrap();
        assert_eq!(orphan_a.board_pin, None);
    }

    #[test]
    fn unwired_led_stays_off_even_if_gpio_high() {
        let mut project = load_c01();
        project.components.push(ComponentInstance {
            instance_id: "led_orphan".into(),
            profile: "led".into(),
            label: None,
            params: Default::default(),
            layout: Default::default(),
        });
        let mut canvas =
            crate::SimCanvas::from_project(&project, repo(), &SimConfig::default()).expect("canvas");
        canvas.boot().unwrap();
        let _ = canvas.sim.set_pin(13, true);
        let snap = build_runtime_snapshot(
            SimRunState::Idle,
            RuntimeAvailability::Available,
            Some(&canvas.sim),
            Some(&project),
            &BTreeMap::new(),
            1,
        );
        // Wired C01 LED follows D13; orphan stays off (not coupled via sole led_pin).
        assert_eq!(
            snap.components.get("led_orphan").unwrap().led_on,
            Some(false)
        );
        let wired_id = project
            .components
            .iter()
            .find(|c| c.profile == "led" && c.instance_id != "led_orphan")
            .unwrap()
            .instance_id
            .clone();
        assert_eq!(snap.components.get(&wired_id).unwrap().led_on, Some(true));
    }

    #[test]
    fn snapshot_follows_wired_led_and_button_actuation() {
        let project = load_c01();
        let mut canvas = crate::SimCanvas::from_project(&project, repo(), &SimConfig::default())
            .expect("canvas");
        canvas.boot().unwrap();
        canvas.press_button();
        canvas.step().unwrap();
        let mut pressed = BTreeMap::new();
        // C01 button instance id from fixture.
        let btn_id = project
            .components
            .iter()
            .find(|c| c.profile.contains("button") || c.profile.contains("push"))
            .map(|c| c.instance_id.clone())
            .unwrap();
        pressed.insert(btn_id.clone(), true);
        let snap = build_runtime_snapshot(
            SimRunState::Paused,
            RuntimeAvailability::Available,
            Some(&canvas.sim),
            Some(&project),
            &pressed,
            2,
        );
        assert!(snap.availability.is_available());
        assert!(snap.any_led_on());
        assert!(snap.any_button_pressed());
        assert_eq!(snap.components.get(&btn_id).unwrap().button_pressed, Some(true));
    }

    #[test]
    fn unavailable_snapshot_has_no_fabricated_cycles() {
        let snap = RuntimeSnapshot::unavailable(SimRunState::Idle);
        assert_eq!(snap.cycle, 0);
        assert!(!snap.any_led_on());
        assert!(matches!(
            snap.availability,
            RuntimeAvailability::Unavailable
        ));
    }

    #[test]
    fn play_after_stop_preserves_run_state_machine() {
        // Stop → Idle; Play → Running (state preservation of pins is simulator-side).
        let stop = apply_sim_command(SimRunState::Running, SimCommand::Stop);
        assert_eq!(stop.run_state, SimRunState::Idle);
        assert!(!stop.reboot_runtime);
        let play = apply_sim_command(stop.run_state, SimCommand::Play);
        assert_eq!(play.run_state, SimRunState::Running);
        assert!(!play.reboot_runtime);
    }

    #[test]
    fn wire_endpoint_helpers() {
        let wires = vec![Wire::new("board.D13", "led1.A")];
        assert_eq!(find_board_pin_for_endpoint(&wires, "led1.A"), Some(13));
        assert_eq!(find_board_pin_for_endpoint(&wires, "led1.K"), None);
    }

    #[test]
    fn led_dc_current_from_profile_params_c01() {
        // R=220, Vf=2.0 override, Vpin=5 → ≈13.6 mA, not burned.
        let mut project = load_c01();
        let led = project
            .components
            .iter_mut()
            .find(|c| c.profile == "led")
            .unwrap();
        led.params
            .insert("vf".into(), toml::Value::Float(2.0));
        let mut canvas =
            crate::SimCanvas::from_project(&project, repo(), &SimConfig::default()).unwrap();
        canvas.boot().unwrap();
        let _ = canvas.sim.set_pin(13, true);
        let empty = BTreeMap::new();
        let snap = build_runtime_snapshot_ex(
            SimRunState::Idle,
            RuntimeAvailability::Available,
            Some(&canvas.sim),
            Some(&project),
            SnapshotExtras {
                button_pressed: &empty,
                button_latched: &empty,
                led_burned: &empty,
                led_params_base: LedElectricalParams::PROFILE_DEFAULTS,
                sim_hints: &ProjectSimHints::default(),
            },
            1,
        );
        let led_id = project
            .components
            .iter()
            .find(|c| c.profile == "led")
            .unwrap()
            .instance_id
            .clone();
        let st = snap.components.get(&led_id).unwrap();
        assert_eq!(st.led_burned, Some(false));
        assert_eq!(st.led_on, Some(true));
        let i = st.led_current_a().unwrap();
        assert!((i - 0.013636363636).abs() < 1e-6, "I={i}");
    }

    #[test]
    fn led_burn_persists_and_opens_circuit_until_reset() {
        let mut project = load_c01();
        let led = project
            .components
            .iter_mut()
            .find(|c| c.profile == "led")
            .unwrap();
        // Low R → I > if_max (0.03 A).
        led.params
            .insert("integrated_resistance".into(), toml::Value::Integer(10));
        led.params
            .insert("vf".into(), toml::Value::Float(2.0));
        let mut canvas =
            crate::SimCanvas::from_project(&project, repo(), &SimConfig::default()).unwrap();
        canvas.boot().unwrap();
        let _ = canvas.sim.set_pin(13, true);
        let led_id = project
            .components
            .iter()
            .find(|c| c.profile == "led")
            .unwrap()
            .instance_id
            .clone();
        let empty = BTreeMap::new();
        let mut burned = BTreeMap::new();

        // Burn scan: I > 0, burned=true, presentation not "on"/overexposed.
        let snap = build_runtime_snapshot_ex(
            SimRunState::Running,
            RuntimeAvailability::Available,
            Some(&canvas.sim),
            Some(&project),
            SnapshotExtras {
                button_pressed: &empty,
                button_latched: &empty,
                led_burned: &burned,
                led_params_base: LedElectricalParams::PROFILE_DEFAULTS,
                sim_hints: &ProjectSimHints::default(),
            },
            1,
        );
        let st = snap.components.get(&led_id).unwrap();
        assert_eq!(st.led_burned, Some(true));
        assert!(st.led_current_a().unwrap() > 0.0);
        assert_eq!(st.led_on, Some(false));
        burned.insert(led_id.clone(), true);

        // Later scans: I=0, still burned; pin high does not re-light.
        let snap2 = build_runtime_snapshot_ex(
            SimRunState::Running,
            RuntimeAvailability::Available,
            Some(&canvas.sim),
            Some(&project),
            SnapshotExtras {
                button_pressed: &empty,
                button_latched: &empty,
                led_burned: &burned,
                led_params_base: LedElectricalParams::PROFILE_DEFAULTS,
                sim_hints: &ProjectSimHints::default(),
            },
            2,
        );
        let st2 = snap2.components.get(&led_id).unwrap();
        assert_eq!(st2.led_burned, Some(true));
        assert_eq!(st2.led_current_ua, Some(0));
        assert_eq!(st2.led_on, Some(false));
    }

    #[test]
    fn pilot_led_load_uses_own_profile_params_and_board_v_high() {
        use crate::sim_role::load_project_sim_hints;

        let mut project = load_c01();
        // Swap LED for pilot; keep wiring to A/K. Clear C01 instance R override
        // so the load uses signal-pilot-lamp profile defaults (1100 Ω).
        let led = project
            .components
            .iter_mut()
            .find(|c| c.profile == "led")
            .unwrap();
        led.profile = "signal-pilot-lamp".into();
        led.instance_id = "pilot1".into();
        led.params.clear();
        for w in &mut project.wiring {
            w.from = w.from.replace("led1", "pilot1");
            w.to = w.to.replace("led1", "pilot1");
        }
        // Use S7 board rail so pilot sees 24 V (D0a logic_high).
        project.board.profile = "siemens-s7-1200-cpu-1214c".into();

        let hints = load_project_sim_hints(&repo(), &project);
        assert_eq!(hints.role_of("signal-pilot-lamp"), Some(SimRole::LedLoad));
        let base = hints.led_params_for_profile("signal-pilot-lamp");
        assert!((base.v_high - 24.0).abs() < 1e-9);
        assert!((base.r_series - 1100.0).abs() < 1e-9);

        let mut canvas =
            crate::SimCanvas::from_project(&project, repo(), &SimConfig::default()).unwrap();
        canvas.boot().unwrap();
        let _ = canvas.sim.set_pin(13, true);
        let empty = BTreeMap::new();
        let snap = build_runtime_snapshot_ex(
            SimRunState::Idle,
            RuntimeAvailability::Available,
            Some(&canvas.sim),
            Some(&project),
            SnapshotExtras {
                button_pressed: &empty,
                button_latched: &empty,
                led_burned: &empty,
                led_params_base: hints.led_params_base,
                sim_hints: &hints,
            },
            1,
        );
        let st = snap.components.get("pilot1").unwrap();
        assert_eq!(st.led_on, Some(true));
        assert_eq!(st.led_burned, Some(false));
        let i = st.led_current_a().unwrap();
        assert!((i - 0.02).abs() < 1e-6, "pilot I={i} (expect ~20 mA @ 24 V)");
    }

    #[test]
    fn split_endpoint_keeps_dotted_port() {
        assert_eq!(
            crate::endpoint::split_endpoint("board.Ia.0"),
            Some(("board", "Ia.0"))
        );
    }
}
