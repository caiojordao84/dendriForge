//! Phase C0–C2 editor runtime session — single control path + snapshot for WorldCanvas.
//!
//! Toolbar and context menus share [`SimSession::apply_command`]. No fabricated
//! scan when the runtime is missing (D-C0 / plan §3).
//! C1: LED DC current/burned in snapshot; canvas button actuation (D-C4).
//! C2: wire logical-activity phase via Instant deltas (D-C2 / D-C6).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use dendri_ast::SimulationProject;
use dendri_canvas::{
    activity_animation_enabled, apply_sim_command, build_runtime_snapshot_ex,
    resolve_terminal_bindings, update_led_burn_state, LedElectricalParams, RuntimeAvailability,
    RuntimeSnapshot, SimCanvas, SimCommand, SimRunState, SnapshotExtras, TerminalKind,
};
use dendri_profiles::ProfileStore;
use dendri_sim::SimConfig;

/// Live simulation session bound to the open project.
#[derive(Debug)]
pub struct SimSession {
    pub run_state: SimRunState,
    pub canvas: Option<SimCanvas>,
    pub snapshot: RuntimeSnapshot,
    /// Per-instance button actuation (momentary ∪ latches).
    pub button_pressed: BTreeMap<String, bool>,
    /// Shift+LMB latches (cleared on Stop/Reset; kept on Pause). UI-only.
    pub latched: BTreeMap<String, bool>,
    /// Instance currently held by momentary LMB (cleared on Pause/Stop/Reset/focus loss).
    pub momentary: Option<String>,
    /// LED burned flags — persist across Stop; cleared on Reset.
    pub led_burned: BTreeMap<String, bool>,
    /// Profile defaults + board V_high for LED DC model.
    pub led_params_base: LedElectricalParams,
    /// Wire activity animation phase (radians-ish seconds accumulator).
    /// Advanced only while Running via Instant deltas; frozen on Pause; cleared on Stop.
    pub wire_anim_phase: f32,
    /// Last visual-clock Instant used for phase deltas (`None` when not advancing).
    wire_anim_last: Option<Instant>,
    present_gen: u64,
}

impl Default for SimSession {
    fn default() -> Self {
        Self::new()
    }
}

impl SimSession {
    pub fn new() -> Self {
        Self {
            run_state: SimRunState::Idle,
            canvas: None,
            snapshot: RuntimeSnapshot::unavailable(SimRunState::Idle),
            button_pressed: BTreeMap::new(),
            latched: BTreeMap::new(),
            momentary: None,
            led_burned: BTreeMap::new(),
            led_params_base: LedElectricalParams::PROFILE_DEFAULTS,
            wire_anim_phase: 0.0,
            wire_anim_last: None,
            present_gen: 0,
        }
    }

    /// Activity overlay phase while Running/Paused; `None` after Stop/Idle (D-C2).
    pub fn wire_activity_phase(&self) -> Option<f32> {
        if activity_animation_enabled(self.run_state) {
            Some(self.wire_anim_phase)
        } else {
            None
        }
    }

    pub fn edits_allowed(&self) -> bool {
        self.run_state.edits_allowed()
    }

    pub fn is_running(&self) -> bool {
        self.run_state.is_running()
    }

    pub fn bump_present(&mut self) {
        self.present_gen = self.present_gen.saturating_add(1);
    }

    /// Boot or rebuild runtime from the project. On failure → explicit error, no fake cycles.
    pub fn boot_from_project(
        &mut self,
        project: &SimulationProject,
        repo_root: &Path,
    ) -> Result<(), String> {
        self.button_pressed.clear();
        self.latched.clear();
        self.momentary = None;
        self.led_burned.clear();
        self.led_params_base = load_led_params_base(repo_root, project);
        match SimCanvas::from_project(project, repo_root, &SimConfig::default()) {
            Ok(mut canvas) => match canvas.boot() {
                Ok(_) => {
                    // Live path must not retain capture history.
                    canvas.clear_frame_history();
                    self.canvas = Some(canvas);
                    self.sync_buttons_to_sim(Some(project));
                    self.bump_present();
                    self.refresh_snapshot(Some(project));
                    Ok(())
                }
                Err(e) => {
                    self.canvas = None;
                    self.bump_present();
                    self.snapshot =
                        RuntimeSnapshot::error(self.run_state, format!("sim boot failed: {e}"));
                    Err(e)
                }
            },
            Err(e) => {
                self.canvas = None;
                self.bump_present();
                self.snapshot =
                    RuntimeSnapshot::error(self.run_state, format!("canvas load failed: {e}"));
                Err(e)
            }
        }
    }

    pub fn clear_runtime(&mut self) {
        self.canvas = None;
        self.button_pressed.clear();
        self.latched.clear();
        self.momentary = None;
        self.led_burned.clear();
        self.bump_present();
        self.snapshot = RuntimeSnapshot::unavailable(self.run_state);
    }

    /// Shared Play/Pause/Stop/Reset/Step path (toolbar + RMB menus).
    pub fn apply_command(
        &mut self,
        cmd: SimCommand,
        project: Option<&SimulationProject>,
        repo_root: &Path,
    ) -> String {
        let outcome = apply_sim_command(self.run_state, cmd);
        self.run_state = outcome.run_state;

        // Wire activity animation (D-C2 / D-C6): Pause freezes phase; Stop/Reset clear.
        match cmd {
            SimCommand::Play => {
                self.wire_anim_last = None; // next SimFrame starts a fresh delta
            }
            SimCommand::Pause => {
                self.wire_anim_last = None; // freeze phase; keep wire_anim_phase
            }
            SimCommand::Stop | SimCommand::Reset => {
                self.wire_anim_phase = 0.0;
                self.wire_anim_last = None;
            }
            SimCommand::Step => {
                // Step lands in Paused — freeze phase (no continuous anim loop).
                self.wire_anim_last = None;
            }
        }

        if outcome.clear_momentary {
            self.momentary = None;
            self.recompute_button_map();
            self.sync_buttons_to_sim(project);
        }
        if outcome.clear_latches {
            self.latched.clear();
            self.recompute_button_map();
            self.sync_buttons_to_sim(project);
        }
        if outcome.reboot_runtime {
            if let Some(project) = project {
                match self.boot_from_project(project, repo_root) {
                    Ok(()) => {
                        // Preserve the run mode Reset was issued under (D-C2).
                        self.run_state = outcome.run_state;
                        self.wire_anim_phase = 0.0;
                        self.wire_anim_last = None;
                    }
                    Err(e) => {
                        self.run_state = outcome.run_state;
                        return format!("reset failed: {e}");
                    }
                }
            } else {
                self.clear_runtime();
                self.run_state = outcome.run_state;
                return "reset failed: no project".into();
            }
        }
        if outcome.step_once {
            if let Err(e) = self.scan_once(project) {
                self.refresh_snapshot(project);
                return e;
            }
        }

        self.bump_present();
        self.refresh_snapshot(project);
        self.status_line()
    }

    /// Toolbar Run toggle: Pause if Running, else Play.
    pub fn toggle_run(
        &mut self,
        project: Option<&SimulationProject>,
        repo_root: &Path,
    ) -> String {
        let cmd = if self.is_running() {
            SimCommand::Pause
        } else {
            SimCommand::Play
        };
        self.apply_command(cmd, project, repo_root)
    }

    /// One runtime scan + presentation update. Never fabricates state without a canvas.
    pub fn scan_once(&mut self, project: Option<&SimulationProject>) -> Result<(), String> {
        let Some(canvas) = self.canvas.as_mut() else {
            self.bump_present();
            self.snapshot = match &self.snapshot.availability {
                RuntimeAvailability::Error(m) => {
                    RuntimeSnapshot::error(self.run_state, m.clone())
                }
                _ => RuntimeSnapshot::unavailable(self.run_state),
            };
            return Err(match &self.snapshot.availability {
                RuntimeAvailability::Error(m) => {
                    format!("scan unavailable: {m}")
                }
                _ => "scan unavailable: runtime not loaded".into(),
            });
        };
        match canvas.step() {
            Ok(_) => {
                // Never accumulate live frames.
                canvas.clear_frame_history();
                self.bump_present();
                self.refresh_snapshot(project);
                Ok(())
            }
            Err(e) => {
                self.bump_present();
                self.snapshot = RuntimeSnapshot::error(self.run_state, format!("scan error: {e}"));
                Err(format!("scan error: {e}"))
            }
        }
    }

    /// Visual-only tick while Running (D-C6). Does **not** advance scans.
    ///
    /// Advances wire activity phase from `at` Instant deltas (never frame counts).
    pub fn present_frame(&mut self, project: Option<&SimulationProject>, at: Instant) {
        if self.run_state.is_running() {
            if let Some(prev) = self.wire_anim_last {
                let dt = at.saturating_duration_since(prev).as_secs_f32();
                // Cap runaway deltas (tab background / hitch) so phase stays smooth.
                let dt = dt.min(0.05);
                self.wire_anim_phase = self.wire_anim_phase + dt;
            }
            self.wire_anim_last = Some(at);
        } else {
            self.wire_anim_last = None;
        }
        self.bump_present();
        self.refresh_snapshot(project);
    }

    pub fn refresh_snapshot(&mut self, project: Option<&SimulationProject>) {
        let availability = if self.canvas.is_some() {
            RuntimeAvailability::Available
        } else {
            match &self.snapshot.availability {
                RuntimeAvailability::Error(m) => RuntimeAvailability::Error(m.clone()),
                _ => RuntimeAvailability::Unavailable,
            }
        };
        self.snapshot = build_runtime_snapshot_ex(
            self.run_state,
            availability,
            self.canvas.as_ref().map(|c| &c.sim),
            project,
            SnapshotExtras {
                button_pressed: &self.button_pressed,
                button_latched: &self.latched,
                led_burned: &self.led_burned,
                led_params_base: self.led_params_base,
            },
            self.present_gen,
        );
        // Persist burns after the burn-scan snapshot has been published (I≠0 that frame).
        if let Some(project) = project {
            let pins = self.snapshot.pins.clone();
            update_led_burn_state(
                project,
                &pins,
                &mut self.led_burned,
                self.led_params_base,
            );
        }
    }

    /// Inspector / primary MVP button actuation (active-low aware via SimCanvas).
    pub fn set_primary_button(
        &mut self,
        pressed: bool,
        project: Option<&SimulationProject>,
    ) {
        if let Some(project) = project {
            if let Some(id) = primary_button_id(project) {
                if pressed {
                    self.button_pressed.insert(id, true);
                } else if !self.latched.contains_key(&id) {
                    self.button_pressed.remove(&id);
                }
            }
        }
        self.sync_buttons_to_sim(project);
        self.bump_present();
        self.refresh_snapshot(project);
    }

    pub fn set_primary_raw_level(
        &mut self,
        raw_high: bool,
        project: Option<&SimulationProject>,
    ) {
        let pressed = if let Some(canvas) = self.canvas.as_ref() {
            if canvas.binding.button_active_low {
                !raw_high
            } else {
                raw_high
            }
        } else {
            !raw_high
        };
        self.set_primary_button(pressed, project);
    }

    /// Canvas LMB down on button body while Running (momentary press).
    pub fn canvas_button_press(
        &mut self,
        instance_id: &str,
        project: Option<&SimulationProject>,
    ) {
        if !self.is_running() {
            return;
        }
        self.momentary = Some(instance_id.to_string());
        self.recompute_button_map();
        self.sync_buttons_to_sim(project);
        self.bump_present();
        self.refresh_snapshot(project);
    }

    /// Canvas LMB up — release momentary only (latches stay).
    pub fn canvas_button_release(&mut self, project: Option<&SimulationProject>) {
        if self.momentary.take().is_some() {
            self.recompute_button_map();
            self.sync_buttons_to_sim(project);
            self.bump_present();
            self.refresh_snapshot(project);
        }
    }

    /// Focus loss / interaction cancel — drop momentary; latches independent.
    pub fn cancel_momentary(&mut self, project: Option<&SimulationProject>) {
        self.canvas_button_release(project);
    }

    /// Shift+LMB toggle latch while Running (D-C4). UI-only — no profile/ASL/wiring change.
    pub fn canvas_button_latch_toggle(
        &mut self,
        instance_id: &str,
        project: Option<&SimulationProject>,
    ) {
        if !self.is_running() {
            return;
        }
        if self.latched.remove(instance_id).is_none() {
            self.latched.insert(instance_id.to_string(), true);
        }
        // Latch toggle also clears momentary on that button so state is unambiguous.
        if self.momentary.as_deref() == Some(instance_id) {
            self.momentary = None;
        }
        self.recompute_button_map();
        self.sync_buttons_to_sim(project);
        self.bump_present();
        self.refresh_snapshot(project);
    }

    fn recompute_button_map(&mut self) {
        self.button_pressed.clear();
        for (id, held) in &self.latched {
            if *held {
                self.button_pressed.insert(id.clone(), true);
            }
        }
        if let Some(id) = &self.momentary {
            self.button_pressed.insert(id.clone(), true);
        }
    }

    /// Drive every button instance pin from the actuation map (multi-button).
    fn sync_buttons_to_sim(&mut self, project: Option<&SimulationProject>) {
        let Some(project) = project else {
            // Fall back to primary-only SimCanvas flag.
            let pressed = self.button_pressed.values().any(|v| *v);
            if let Some(canvas) = self.canvas.as_mut() {
                canvas.set_button(pressed);
            }
            return;
        };
        let bindings = resolve_terminal_bindings(project);
        let active_low = self
            .canvas
            .as_ref()
            .map(|c| c.binding.button_active_low)
            .unwrap_or(true);
        let Some(canvas) = self.canvas.as_mut() else {
            return;
        };
        let mut any_primary = false;
        for comp in &project.components {
            let is_btn = comp.profile.contains("button") || comp.profile.contains("push");
            if !is_btn {
                continue;
            }
            let pressed = self
                .button_pressed
                .get(&comp.instance_id)
                .copied()
                .unwrap_or(false);
            if primary_button_id(project).as_deref() == Some(comp.instance_id.as_str()) {
                any_primary = pressed;
            }
            // Prefer NO contact pin; fall back to any button contact binding.
            let pin = bindings
                .iter()
                .find(|b| {
                    b.instance_id == comp.instance_id
                        && b.kind == TerminalKind::ButtonContact
                        && b.terminal == "NO"
                })
                .or_else(|| {
                    bindings.iter().find(|b| {
                        b.instance_id == comp.instance_id
                            && b.kind == TerminalKind::ButtonContact
                            && b.board_pin.is_some()
                    })
                })
                .and_then(|b| b.board_pin);
            if let Some(pin) = pin {
                let level = if active_low { !pressed } else { pressed };
                let _ = canvas.sim.set_pin(pin, level);
            }
        }
        canvas.button_pressed = any_primary
            || self.button_pressed.values().any(|v| *v);
    }

    pub fn primary_raw_high(&self) -> bool {
        let Some(canvas) = self.canvas.as_ref() else {
            return true;
        };
        canvas.sim.pin(canvas.binding.button_pin)
    }

    pub fn primary_pressed_logic(&self, project: Option<&SimulationProject>) -> bool {
        if let Some(project) = project {
            if let Some(id) = primary_button_id(project) {
                if let Some(v) = self.button_pressed.get(&id) {
                    return *v;
                }
            }
        }
        if let Some(canvas) = self.canvas.as_ref() {
            return canvas.button_pressed;
        }
        false
    }

    pub fn primary_led_on(&self) -> bool {
        self.snapshot.any_led_on()
    }

    pub fn cycles(&self) -> u64 {
        self.snapshot.cycle
    }

    pub fn status_line(&self) -> String {
        match &self.snapshot.availability {
            RuntimeAvailability::Error(m) => format!(
                "sim {} · ERROR · {m}",
                run_label(self.run_state)
            ),
            RuntimeAvailability::Unavailable => format!(
                "sim {} · runtime unavailable (no fabricated cycles)",
                run_label(self.run_state)
            ),
            RuntimeAvailability::Available => {
                let burned = if self.snapshot.any_led_burned() {
                    " · burned"
                } else {
                    ""
                };
                format!(
                    "sim {} · scan {} · led={}{} · present_gen={}",
                    run_label(self.run_state),
                    self.snapshot.cycle,
                    if self.snapshot.any_led_on() { 1 } else { 0 },
                    burned,
                    self.snapshot.present_gen
                )
            }
        }
    }
}

fn run_label(s: SimRunState) -> &'static str {
    match s {
        SimRunState::Idle => "Idle",
        SimRunState::Running => "Running",
        SimRunState::Paused => "Paused",
    }
}

fn primary_button_id(project: &SimulationProject) -> Option<String> {
    project
        .components
        .iter()
        .find(|c| c.profile.contains("button") || c.profile.contains("push"))
        .map(|c| c.instance_id.clone())
}

fn load_led_params_base(repo_root: &Path, project: &SimulationProject) -> LedElectricalParams {
    let content = repo_root.join("content");
    let store = ProfileStore::new(&content);
    let mut base = LedElectricalParams::PROFILE_DEFAULTS;
    // Board logic-high voltage (Uno = 5 V).
    if let Ok(board) = store.load_board(&project.board.profile) {
        if let Some(v) = board_logic_high_v(&board.rest) {
            base.v_high = v;
        }
    }
    if let Ok(led) = store.load_component("led") {
        if let Some(params) = led.rest.get("params").and_then(|v| v.as_table()) {
            base = dendri_canvas::defaults_from_profile_params(params, base.v_high);
        }
    }
    base
}

fn board_logic_high_v(rest: &toml::map::Map<String, toml::Value>) -> Option<f64> {
    // Prefer an explicit simulation / power rail hint when present.
    if let Some(sim) = rest.get("simulation").and_then(|v| v.as_table()) {
        for key in ["vcc", "logic_high", "voltage"] {
            if let Some(v) = sim.get(key) {
                if let Some(f) = value_as_volts(v) {
                    return Some(f);
                }
            }
        }
    }
    // Ports often list `voltage = "5V"` on digital pins.
    if let Some(ports) = rest.get("ports").and_then(|v| v.as_array()) {
        for p in ports {
            if let Some(t) = p.as_table() {
                if let Some(v) = t.get("voltage").and_then(value_as_volts) {
                    if (v - 5.0).abs() < 0.01 || (v - 3.3).abs() < 0.01 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_parse::{parse_path, ParsedDocument};
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn load_c01() -> SimulationProject {
        let path = repo().join("content/fixtures/canvas/C01-button-led.toml");
        let doc = parse_path(&path).unwrap();
        match doc {
            ParsedDocument::Project(p) => p.project,
            _ => panic!("expected project"),
        }
    }

    #[test]
    fn no_fake_scan_when_runtime_missing() {
        let mut session = SimSession::new();
        let err = session.scan_once(None).unwrap_err();
        assert!(err.contains("unavailable"));
        assert_eq!(session.cycles(), 0);
        assert!(!session.primary_led_on());
        assert!(matches!(
            session.snapshot.availability,
            RuntimeAvailability::Unavailable
        ));
    }

    #[test]
    fn pause_stop_reset_via_single_control_path() {
        let project = load_c01();
        let mut session = SimSession::new();
        session.boot_from_project(&project, &repo()).unwrap();
        session.apply_command(SimCommand::Play, Some(&project), &repo());
        assert!(session.is_running());
        session.scan_once(Some(&project)).unwrap();
        let cycles_after_run = session.cycles();
        assert!(cycles_after_run >= 1);

        session.latched.insert("btn".into(), true);
        session.apply_command(SimCommand::Pause, Some(&project), &repo());
        assert_eq!(session.run_state, SimRunState::Paused);
        assert!(session.latched.contains_key("btn"));

        let cycles_paused = session.cycles();
        session.apply_command(SimCommand::Stop, Some(&project), &repo());
        assert_eq!(session.run_state, SimRunState::Idle);
        assert!(session.latched.is_empty());
        // Stop preserves cycle / pin state (no reboot).
        assert_eq!(session.cycles(), cycles_paused);

        session.apply_command(SimCommand::Play, Some(&project), &repo());
        assert!(session.is_running());
        assert_eq!(session.cycles(), cycles_paused);

        session.apply_command(SimCommand::Reset, Some(&project), &repo());
        assert_eq!(session.cycles(), 0);
        assert!(session.latched.is_empty());
        assert!(session.led_burned.is_empty());
    }

    #[test]
    fn step_advances_one_cycle_and_pauses() {
        let project = load_c01();
        let mut session = SimSession::new();
        session.boot_from_project(&project, &repo()).unwrap();
        session.apply_command(SimCommand::Play, Some(&project), &repo());
        session.apply_command(SimCommand::Step, Some(&project), &repo());
        assert_eq!(session.run_state, SimRunState::Paused);
        assert_eq!(session.cycles(), 1);
        session.apply_command(SimCommand::Step, Some(&project), &repo());
        assert_eq!(session.cycles(), 2);
    }

    #[test]
    fn live_steps_do_not_accumulate_frame_history() {
        let project = load_c01();
        let mut session = SimSession::new();
        session.boot_from_project(&project, &repo()).unwrap();
        for _ in 0..5 {
            session.scan_once(Some(&project)).unwrap();
        }
        let frames = session.canvas.as_ref().unwrap().frames.len();
        assert_eq!(frames, 0);
    }

    #[test]
    fn edits_blocked_while_running() {
        let mut session = SimSession::new();
        session.run_state = SimRunState::Running;
        assert!(!session.edits_allowed());
        session.run_state = SimRunState::Paused;
        assert!(session.edits_allowed());
    }

    #[test]
    fn shift_latch_only_while_running_pause_keeps_stop_clears() {
        let project = load_c01();
        let mut session = SimSession::new();
        session.boot_from_project(&project, &repo()).unwrap();
        let btn = primary_button_id(&project).unwrap();

        // Outside Running: latch toggle is a no-op.
        session.canvas_button_latch_toggle(&btn, Some(&project));
        assert!(session.latched.is_empty());

        session.apply_command(SimCommand::Play, Some(&project), &repo());
        session.canvas_button_latch_toggle(&btn, Some(&project));
        assert!(session.latched.get(&btn).copied().unwrap_or(false));
        assert_eq!(session.button_pressed.get(&btn), Some(&true));
        // Active-low: pressed → D2 low.
        assert!(!session.primary_raw_high());

        session.apply_command(SimCommand::Pause, Some(&project), &repo());
        assert!(session.latched.contains_key(&btn));
        // Pause cancels momentary only; latch remains → still pressed.
        assert_eq!(session.button_pressed.get(&btn), Some(&true));

        session.apply_command(SimCommand::Stop, Some(&project), &repo());
        assert!(session.latched.is_empty());
        assert!(session.button_pressed.is_empty());
    }

    #[test]
    fn canvas_momentary_press_release_and_c01_led_follow() {
        let project = load_c01();
        let mut session = SimSession::new();
        session.boot_from_project(&project, &repo()).unwrap();
        let btn = primary_button_id(&project).unwrap();
        session.apply_command(SimCommand::Play, Some(&project), &repo());

        // Released: pull-up → D2 high.
        assert!(session.primary_raw_high());
        session.canvas_button_press(&btn, Some(&project));
        assert!(!session.primary_raw_high());
        session.scan_once(Some(&project)).unwrap();
        assert!(session.primary_led_on(), "ASL should drive LED from pressed");

        session.canvas_button_release(Some(&project));
        assert!(session.primary_raw_high());
        session.scan_once(Some(&project)).unwrap();
        assert!(!session.primary_led_on());
    }

    #[test]
    fn led_current_in_snapshot_from_profile_r220() {
        let project = load_c01();
        let mut session = SimSession::new();
        session.boot_from_project(&project, &repo()).unwrap();
        let btn = primary_button_id(&project).unwrap();
        session.apply_command(SimCommand::Play, Some(&project), &repo());
        session.canvas_button_press(&btn, Some(&project));
        session.scan_once(Some(&project)).unwrap();
        let led = project
            .components
            .iter()
            .find(|c| c.profile == "led")
            .unwrap();
        let st = session.snapshot.components.get(&led.instance_id).unwrap();
        assert_eq!(st.led_burned, Some(false));
        // C01 uses profile vf=2.1 default + R=220 → I=(5-2.1)/220 ≈ 13.18 mA
        let i = st.led_current_a().unwrap();
        assert!(i > 0.01 && i < 0.02, "I={i}");
        assert_eq!(st.led_on, Some(true));
    }

    #[test]
    fn wire_activity_phase_advances_freezes_on_pause_clears_on_stop() {
        use std::time::Duration;
        let project = load_c01();
        let mut session = SimSession::new();
        session.boot_from_project(&project, &repo()).unwrap();
        assert!(session.wire_activity_phase().is_none());

        session.apply_command(SimCommand::Play, Some(&project), &repo());
        let t0 = Instant::now();
        session.present_frame(Some(&project), t0);
        session.present_frame(Some(&project), t0 + Duration::from_millis(20));
        let phase_running = session.wire_activity_phase().unwrap();
        assert!(phase_running > 0.0);

        session.apply_command(SimCommand::Pause, Some(&project), &repo());
        let frozen = session.wire_activity_phase().unwrap();
        // Pause keeps phase; present_frame while not Running must not advance.
        session.present_frame(Some(&project), t0 + Duration::from_millis(80));
        assert_eq!(session.wire_activity_phase(), Some(frozen));

        session.apply_command(SimCommand::Stop, Some(&project), &repo());
        assert!(session.wire_activity_phase().is_none());
        assert_eq!(session.wire_anim_phase, 0.0);
    }
}
