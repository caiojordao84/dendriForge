//! Phase C0 editor runtime session — single control path + snapshot for WorldCanvas.
//!
//! Toolbar and context menus share [`SimSession::apply_command`]. No fabricated
//! scan when the runtime is missing (D-C0 / plan §3).

use std::collections::BTreeMap;
use std::path::Path;

use dendri_ast::SimulationProject;
use dendri_canvas::{
    apply_sim_command, build_runtime_snapshot, RuntimeAvailability, RuntimeSnapshot, SimCanvas,
    SimCommand, SimRunState,
};
use dendri_sim::SimConfig;

/// Live simulation session bound to the open project.
#[derive(Debug)]
pub struct SimSession {
    pub run_state: SimRunState,
    pub canvas: Option<SimCanvas>,
    pub snapshot: RuntimeSnapshot,
    /// Per-instance button actuation (momentary ∪ latches). Prepared for C1.
    pub button_pressed: BTreeMap<String, bool>,
    /// Ctrl+LMB latches (cleared on Stop/Reset; kept on Pause). C1 UX may polish.
    pub latched: BTreeMap<String, bool>,
    /// Instance currently held by momentary LMB (cleared on Pause/Stop/Reset/focus loss).
    pub momentary: Option<String>,
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
            present_gen: 0,
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
        match SimCanvas::from_project(project, repo_root, &SimConfig::default()) {
            Ok(mut canvas) => match canvas.boot() {
                Ok(_) => {
                    // Live path must not retain capture history.
                    canvas.clear_frame_history();
                    self.canvas = Some(canvas);
                    self.sync_primary_button_from_map(Some(project));
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

        if outcome.clear_momentary {
            self.momentary = None;
            self.recompute_button_map();
            self.sync_primary_button_from_map(project);
        }
        if outcome.clear_latches {
            self.latched.clear();
            self.recompute_button_map();
            self.sync_primary_button_from_map(project);
        }
        if outcome.reboot_runtime {
            if let Some(project) = project {
                match self.boot_from_project(project, repo_root) {
                    Ok(()) => {
                        // Preserve the run mode Reset was issued under (D-C2).
                        self.run_state = outcome.run_state;
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
    pub fn present_frame(&mut self, project: Option<&SimulationProject>) {
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
        self.snapshot = build_runtime_snapshot(
            self.run_state,
            availability,
            self.canvas.as_ref().map(|c| &c.sim),
            project,
            &self.button_pressed,
            self.present_gen,
        );
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
        let held = pressed || self.any_primary_held(project);
        if let Some(canvas) = self.canvas.as_mut() {
            canvas.set_button(held);
        }
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

    fn any_primary_held(&self, project: Option<&SimulationProject>) -> bool {
        let Some(project) = project else {
            return self.button_pressed.values().any(|v| *v);
        };
        primary_button_id(project)
            .and_then(|id| self.button_pressed.get(&id).copied())
            .unwrap_or(false)
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

    fn sync_primary_button_from_map(&mut self, project: Option<&SimulationProject>) {
        let pressed = self.any_primary_held(project);
        if let Some(canvas) = self.canvas.as_mut() {
            canvas.set_button(pressed);
        }
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
            RuntimeAvailability::Available => format!(
                "sim {} · scan {} · led={} · present_gen={}",
                run_label(self.run_state),
                self.snapshot.cycle,
                if self.snapshot.any_led_on() { 1 } else { 0 },
                self.snapshot.present_gen
            ),
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
}
