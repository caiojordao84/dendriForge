//! Logical wire activity for Phase C2 (not calculated electrical current).
//!
//! **“Hot” / logical activity:** a wire is active when at least one endpoint
//! resolves to a **known digital level** (`Some(true)` / `Some(false)`) from the
//! runtime snapshot (`pins` via bindings, or bound component `terminals`).
//!
//! - LOW is still activity (do **not** treat LOW as “no activity”).
//! - Unbound / unknown endpoints are **not** fabricated into levels.
//! - Power/Gnd role wires without a digital GPIO binding are not digital activity.
//! - No current direction or intensity is invented — callers present undirected
//!   dash/pulse overlays only while activity animation is enabled (Running /
//!   frozen on Pause; cleared on Stop).

use crate::endpoint::split_endpoint;
use crate::sim_bridge::RuntimeSnapshot;
use crate::wires::{wire_color_kind, RoutedWire, WireColorKind};

/// Logical activity classification for one routed wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireLogicalActivity {
    /// No known digital level on either endpoint (or not a digital signal net).
    Inactive,
    /// Known digital level — HIGH or LOW both count as activity.
    Active { level: bool },
}

impl WireLogicalActivity {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active { .. })
    }

    pub fn level(self) -> Option<bool> {
        match self {
            Self::Active { level } => Some(level),
            Self::Inactive => None,
        }
    }
}

/// Resolve a project endpoint (`board.D13`, `led1.A`, …) to a known digital level.
pub fn endpoint_logical_level(endpoint: &str, snapshot: &RuntimeSnapshot) -> Option<bool> {
    let (instance, terminal) = split_endpoint(endpoint)?;

    // Prefer explicit GPIO binding → live pin map.
    if let Some(binding) = snapshot.bindings.iter().find(|b| {
        b.instance_id.eq_ignore_ascii_case(instance) && b.terminal.eq_ignore_ascii_case(terminal)
    }) {
        if let Some(pin) = binding.board_pin {
            if let Some(level) = snapshot.pins.get(&pin).copied() {
                return Some(level);
            }
        }
    }

    // Fall back to per-component terminal slice (already bound levels).
    snapshot
        .components
        .get(instance)
        .and_then(|c| {
            c.terminals
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(terminal))
                .and_then(|(_, v)| *v)
        })
        .or_else(|| {
            // Case-insensitive instance lookup.
            snapshot.components.iter().find_map(|(id, c)| {
                if id.eq_ignore_ascii_case(instance) {
                    c.terminals
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(terminal))
                        .and_then(|(_, v)| *v)
                } else {
                    None
                }
            })
        })
}

/// Classify a wire's logical activity from the runtime snapshot.
///
/// Power/Gnd conductors (by signal/role) are never treated as digital activity
/// overlays — they keep static role colours only.
pub fn wire_logical_activity(wire: &RoutedWire, snapshot: &RuntimeSnapshot) -> WireLogicalActivity {
    match wire_color_kind(
        wire.signal.as_deref(),
        wire.from_role,
        wire.to_role,
        wire.sequence,
    ) {
        WireColorKind::Gnd | WireColorKind::Vcc => return WireLogicalActivity::Inactive,
        WireColorKind::Signal { .. } => {}
    }

    // Prefer board-side endpoint when both resolve (stable presentation level).
    let from_lvl = endpoint_logical_level(&wire.from, snapshot);
    let to_lvl = endpoint_logical_level(&wire.to, snapshot);
    let level = match (from_lvl, to_lvl) {
        (Some(a), Some(_)) => {
            // Prefer board.* endpoint when present.
            if wire.from.to_ascii_lowercase().starts_with("board.") {
                Some(a)
            } else if wire.to.to_ascii_lowercase().starts_with("board.") {
                to_lvl
            } else {
                Some(a)
            }
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    match level {
        Some(level) => WireLogicalActivity::Active { level },
        None => WireLogicalActivity::Inactive,
    }
}

/// Whether activity **animation** should be painted (D-C2 / D-C6).
///
/// Running / Paused: yes (Paused freezes phase). Idle after Stop: no —
/// logical pin state may remain, but activity overlays are cleared.
pub fn activity_animation_enabled(run_state: crate::SimRunState) -> bool {
    matches!(
        run_state,
        crate::SimRunState::Running | crate::SimRunState::Paused
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_bridge::{
        RuntimeAvailability, RuntimeSnapshot, SimRunState, TerminalBinding, TerminalKind,
    };
    use crate::world::TerminalRole;
    use crate::WorldPoint;
    use std::collections::BTreeMap;

    fn signal_wire(from: &str, to: &str) -> RoutedWire {
        RoutedWire {
            index: 0,
            from: from.into(),
            to: to.into(),
            signal: Some("digital".into()),
            waypoints: vec![],
            points: vec![WorldPoint::new(0.0, 0.0), WorldPoint::new(1.0, 0.0)],
            from_role: TerminalRole::Digital,
            to_role: TerminalRole::Digital,
            sequence: 0,
        }
    }

    fn gnd_wire() -> RoutedWire {
        RoutedWire {
            index: 1,
            from: "board.GND".into(),
            to: "led1.K".into(),
            signal: Some("gnd".into()),
            waypoints: vec![],
            points: vec![WorldPoint::new(0.0, 0.0), WorldPoint::new(1.0, 0.0)],
            from_role: TerminalRole::Gnd,
            to_role: TerminalRole::Gnd,
            sequence: 0,
        }
    }

    fn snap_with_pin(pin: u32, high: bool) -> RuntimeSnapshot {
        let mut pins = BTreeMap::new();
        pins.insert(pin, high);
        RuntimeSnapshot {
            run_state: SimRunState::Running,
            availability: RuntimeAvailability::Available,
            cycle: 1,
            time_ms: 0,
            pins,
            bindings: vec![TerminalBinding {
                instance_id: "board".into(),
                terminal: format!("D{pin}"),
                kind: TerminalKind::BoardDigital,
                board_pin: Some(pin),
            }],
            components: BTreeMap::new(),
            present_gen: 1,
        }
    }

    #[test]
    fn high_and_low_are_both_hot() {
        let w = signal_wire("board.D13", "led1.A");
        let high = snap_with_pin(13, true);
        assert_eq!(
            wire_logical_activity(&w, &high),
            WireLogicalActivity::Active { level: true }
        );
        let low = snap_with_pin(13, false);
        assert_eq!(
            wire_logical_activity(&w, &low),
            WireLogicalActivity::Active { level: false }
        );
    }

    #[test]
    fn unbound_endpoint_is_inactive() {
        let w = signal_wire("board.D8", "led1.A");
        let snap = snap_with_pin(13, true);
        assert_eq!(
            wire_logical_activity(&w, &snap),
            WireLogicalActivity::Inactive
        );
    }

    #[test]
    fn power_rails_are_not_digital_activity() {
        let snap = snap_with_pin(13, true);
        assert_eq!(
            wire_logical_activity(&gnd_wire(), &snap),
            WireLogicalActivity::Inactive
        );
    }

    #[test]
    fn activity_anim_enabled_running_paused_not_idle() {
        assert!(activity_animation_enabled(SimRunState::Running));
        assert!(activity_animation_enabled(SimRunState::Paused));
        assert!(!activity_animation_enabled(SimRunState::Idle));
    }

    #[test]
    fn dotted_port_id_resolves_like_sim_bridge() {
        // Regression: wire_activity used to truncate `board.Ia.0` → terminal `Ia`.
        let mut pins = BTreeMap::new();
        pins.insert(0u32, true);
        let snap = RuntimeSnapshot {
            run_state: SimRunState::Running,
            availability: RuntimeAvailability::Available,
            cycle: 1,
            time_ms: 0,
            pins,
            bindings: vec![TerminalBinding {
                instance_id: "board".into(),
                terminal: "Ia.0".into(),
                kind: TerminalKind::BoardDigital,
                board_pin: Some(0),
            }],
            components: BTreeMap::new(),
            present_gen: 1,
        };
        assert_eq!(endpoint_logical_level("board.Ia.0", &snap), Some(true));
        let w = signal_wire("board.Ia.0", "load.A");
        assert_eq!(
            wire_logical_activity(&w, &snap),
            WireLogicalActivity::Active { level: true }
        );
    }
}
