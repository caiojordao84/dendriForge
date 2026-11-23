//! Shared wiring endpoint parsing (`board.D13`, `board.Ia.0`, `led1.A`).

/// Split `instance.port` where `port` may itself contain dots (`Ia.0`).
///
/// Uses the first `.` only — identical in `sim_bridge` and `wire_activity`
/// so Siemens-style ids stay consistent (D-comp-13 / D0b).
pub fn split_endpoint(endpoint: &str) -> Option<(&str, &str)> {
    let (instance, port) = endpoint.split_once('.')?;
    if instance.is_empty() || port.is_empty() {
        return None;
    }
    Some((instance, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_port_keeps_suffix() {
        assert_eq!(split_endpoint("board.Ia.0"), Some(("board", "Ia.0")));
        assert_eq!(split_endpoint("board.D13"), Some(("board", "D13")));
        assert_eq!(split_endpoint("led1.A"), Some(("led1", "A")));
        assert_eq!(split_endpoint("board."), None);
        assert_eq!(split_endpoint(".D13"), None);
        assert_eq!(split_endpoint("nodot"), None);
    }
}
