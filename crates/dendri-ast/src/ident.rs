//! Identifier grammar and wire endpoint parsing (`01-asl-ir-and-project-schema.md` §2.4).
//!
//! - Profile IDs:  `^[a-z0-9]+(-[a-z0-9]+)*$` (kebab-case, matches directory names 1:1)
//! - Instance IDs: `^[a-z][a-z0-9_]*$` (snake_case, no dots, globally unique)
//! - Port IDs:     non-empty, MAY contain dots (e.g. `Ia.0`, `Qa.0`, `D13`, `IW64`)
//! - Endpoints:    `<instance_id>.<port_id>`, strictly split at the FIRST dot.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Stable node identifier (`n_0001`, …) anchoring comments, assumptions, and presentation metadata.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    /// Creates a new [`NodeId`].
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrows the underlying identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Returns `true` iff `s` matches `^[a-z0-9]+(-[a-z0-9]+)*$` (profile IDs).
#[must_use]
pub fn is_valid_profile_id(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut prev_hyphen = true; // forbids leading hyphen
    for b in s.bytes() {
        if b == b'-' {
            if prev_hyphen {
                return false;
            }
            prev_hyphen = true;
        } else if b.is_ascii_lowercase() || b.is_ascii_digit() {
            prev_hyphen = false;
        } else {
            return false;
        }
    }
    !prev_hyphen // forbids trailing hyphen
}

/// Returns `true` iff `s` matches `^[a-z][a-z0-9_]*$` (project instance IDs: boards & components).
#[must_use]
pub fn is_valid_instance_id(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Returns `true` iff `s` is a valid non-empty port identifier (MAY contain dots, no whitespace).
#[must_use]
pub fn is_valid_port_id(s: &str) -> bool {
    if s.is_empty() || s.starts_with('.') || s.ends_with('.') || s.contains("..") {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'+' | b'/'))
}

/// Error produced when parsing a malformed wire endpoint (`ERR_BAD_ENDPOINT`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EndpointError {
    #[error("endpoint `{0}` must follow `<instance_id>.<port_id>`")]
    MissingDot(String),
    #[error(
        "invalid instance_id `{instance_id}` in endpoint `{raw}` (expected `^[a-z][a-z0-9_]*$`)"
    )]
    InvalidInstanceId { raw: String, instance_id: String },
    #[error("invalid port_id `{port_id}` in endpoint `{raw}`")]
    InvalidPortId { raw: String, port_id: String },
}

/// Parsed wire endpoint `<instance_id>.<port_id>` split strictly at the FIRST dot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Endpoint {
    pub instance_id: String,
    pub port_id: String,
}

impl Endpoint {
    /// Parses `<instance_id>.<port_id>` strictly splitting at the first `.`.
    pub fn parse(raw: &str) -> Result<Self, EndpointError> {
        let Some((instance, port)) = raw.split_once('.') else {
            return Err(EndpointError::MissingDot(raw.to_owned()));
        };
        if !is_valid_instance_id(instance) {
            return Err(EndpointError::InvalidInstanceId {
                raw: raw.to_owned(),
                instance_id: instance.to_owned(),
            });
        }
        if !is_valid_port_id(port) {
            return Err(EndpointError::InvalidPortId {
                raw: raw.to_owned(),
                port_id: port.to_owned(),
            });
        }
        Ok(Self {
            instance_id: instance.to_owned(),
            port_id: port.to_owned(),
        })
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.instance_id, self.port_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_and_instance_id_validation() {
        assert!(is_valid_profile_id("arduino-uno-r3"));
        assert!(is_valid_profile_id("plc-1200-open-1214"));
        assert!(is_valid_profile_id("led"));
        assert!(!is_valid_profile_id(""));
        assert!(!is_valid_profile_id("../escape"));
        assert!(!is_valid_profile_id("Arduino-Uno"));
        assert!(!is_valid_profile_id("led--red"));
        assert!(!is_valid_profile_id("-led"));
        assert!(!is_valid_profile_id("led-"));

        assert!(is_valid_instance_id("plc_main"));
        assert!(is_valid_instance_id("mcu1"));
        assert!(!is_valid_instance_id(""));
        assert!(!is_valid_instance_id("1plc"));
        assert!(!is_valid_instance_id("plc-main"));
        assert!(!is_valid_instance_id("plc.main"));
        assert!(!is_valid_instance_id("PlcMain"));
    }

    #[test]
    fn endpoint_splits_at_first_dot_and_preserves_dotted_plc_ports() {
        let ep = Endpoint::parse("plc_main.Ia.0").unwrap();
        assert_eq!(ep.instance_id, "plc_main");
        assert_eq!(ep.port_id, "Ia.0");
        assert_eq!(ep.to_string(), "plc_main.Ia.0");

        let ep2 = Endpoint::parse("mcu_uno.D13").unwrap();
        assert_eq!(ep2.instance_id, "mcu_uno");
        assert_eq!(ep2.port_id, "D13");

        assert!(matches!(
            Endpoint::parse("no_dot"),
            Err(EndpointError::MissingDot(_))
        ));
        assert!(matches!(
            Endpoint::parse("Bad-Id.D13"),
            Err(EndpointError::InvalidInstanceId { .. })
        ));
        assert!(matches!(
            Endpoint::parse("plc_main."),
            Err(EndpointError::InvalidPortId { .. })
        ));
        assert!(matches!(
            Endpoint::parse("plc_main.Ia..0"),
            Err(EndpointError::InvalidPortId { .. })
        ));
    }
}
