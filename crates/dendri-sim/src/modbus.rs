//! Virtual Modbus TCP register map for deterministic sim (DEN-25 / P2).
//!
//! This is **not** a wire-level MBAP/TCP stack. ASL ops map 1:1 onto
//! holding/coil/input/discrete tables keyed by `(unit_id, address)`, matching
//! the existing I2C/SPI virtual-device pattern (`BusCfg` / `BusXfer` traces).

use std::collections::HashMap;

/// Seedable Modbus TCP register image.
#[derive(Debug, Clone, Default)]
pub struct ModbusMap {
    /// Holding registers (FC3/FC6) — read/write.
    pub holding: HashMap<(u64, u64), u16>,
    /// Coils (FC1/FC5) — read/write.
    pub coils: HashMap<(u64, u64), bool>,
    /// Input registers (FC4) — read-only from program; seedable.
    pub inputs: HashMap<(u64, u64), u16>,
    /// Discrete inputs (FC2) — read-only from program; seedable.
    pub discretes: HashMap<(u64, u64), bool>,
    /// Last `CFG_MODBUS_TCP` detail (unit, role, port), if configured.
    pub cfg: Option<ModbusCfg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModbusCfg {
    pub unit_id: u64,
    pub role: String,
    pub port: u64,
}

impl ModbusMap {
    pub fn from_seeds(
        holding: HashMap<(u64, u64), u16>,
        coils: HashMap<(u64, u64), bool>,
        inputs: HashMap<(u64, u64), u16>,
        discretes: HashMap<(u64, u64), bool>,
    ) -> Self {
        Self {
            holding,
            coils,
            inputs,
            discretes,
            cfg: None,
        }
    }

    pub fn configure(&mut self, unit_id: u64, role: String, port: u64) -> String {
        let role_norm = normalize_role(&role);
        self.cfg = Some(ModbusCfg {
            unit_id,
            role: role_norm.clone(),
            port,
        });
        format!("unit={unit_id} role={role_norm} port={port}")
    }

    pub fn write_holding(&mut self, unit: u64, addr: u64, value: u16) -> String {
        self.holding.insert((unit, addr), value);
        format!("WRITE_HOLDING unit={unit} addr={addr} value=0x{value:04X}")
    }

    pub fn read_holding(&self, unit: u64, addr: u64) -> (u16, String) {
        let value = *self.holding.get(&(unit, addr)).unwrap_or(&0);
        (
            value,
            format!("READ_HOLDING unit={unit} addr={addr} value=0x{value:04X}"),
        )
    }

    pub fn write_coil(&mut self, unit: u64, addr: u64, value: bool) -> String {
        self.coils.insert((unit, addr), value);
        format!("WRITE_COIL unit={unit} addr={addr} value={value}")
    }

    pub fn read_coil(&self, unit: u64, addr: u64) -> (bool, String) {
        let value = *self.coils.get(&(unit, addr)).unwrap_or(&false);
        (
            value,
            format!("READ_COIL unit={unit} addr={addr} value={value}"),
        )
    }

    pub fn read_input(&self, unit: u64, addr: u64) -> (u16, String) {
        let value = *self.inputs.get(&(unit, addr)).unwrap_or(&0);
        (
            value,
            format!("READ_INPUT unit={unit} addr={addr} value=0x{value:04X}"),
        )
    }

    pub fn read_discrete(&self, unit: u64, addr: u64) -> (bool, String) {
        let value = *self.discretes.get(&(unit, addr)).unwrap_or(&false);
        (
            value,
            format!("READ_DISCRETE unit={unit} addr={addr} value={value}"),
        )
    }
}

fn normalize_role(role: &str) -> String {
    match role.trim().to_ascii_lowercase().as_str() {
        "server" | "slave" => "server".into(),
        "client" | "master" => "client".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holding_round_trip() {
        let mut m = ModbusMap::default();
        m.write_holding(1, 100, 0x1234);
        assert_eq!(m.read_holding(1, 100).0, 0x1234);
        assert_eq!(m.read_holding(1, 101).0, 0);
    }

    #[test]
    fn coil_and_role_normalize() {
        let mut m = ModbusMap::default();
        let detail = m.configure(1, "Slave".into(), 502);
        assert!(detail.contains("role=server"));
        m.write_coil(1, 0, true);
        assert!(m.read_coil(1, 0).0);
    }
}
