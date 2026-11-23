//! Physical-topology I2C bus fabric (`02-simulation-and-electrical-engine.md` §4.2).
//!
//! Enforces:
//! - Shared `SDA` and `SCL` physical nets in [`BusTopology`] between controller and every target.
//! - Pull-up verification on both `SDA` and `SCL` lines (`I2cError::MissingPullUp`).
//! - 7-bit addressing (`0x08..=0x77`), `scan()`, multi-byte `write`/`read`/`write_read`,
//!   and multi-device coexistence via [`I2cDevice`].

use crate::topology::BusTopology;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Physical `(instance_id, sda_port, scl_port)` binding for an I2C controller or peripheral.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct I2cEndpoint {
    pub instance_id: String,
    pub sda_port: String,
    pub scl_port: String,
}

impl I2cEndpoint {
    #[must_use]
    pub fn new(
        instance_id: impl Into<String>,
        sda_port: impl Into<String>,
        scl_port: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            sda_port: sda_port.into(),
            scl_port: scl_port.into(),
        }
    }
}

/// Errors emitted by [`I2cBus`] transactions.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum I2cError {
    #[error("invalid 7-bit I2C address 0x{0:02X} (valid range 0x08..=0x77)")]
    InvalidAddress(u8),
    #[error("I2C bus missing pull-up resistor on SDA and/or SCL")]
    MissingPullUp,
    #[error("I2C controller `{0}` is not wired to any SDA/SCL net")]
    ControllerNotWired(String),
    #[error("I2C address 0x{0:02X} returned NACK (no wired device acknowledged)")]
    Nack(u8),
    #[error("I2C address collision at 0x{addr:02X} on the same physical bus")]
    AddressCollision { addr: u8 },
    #[error("I2C device error at 0x{addr:02X}: {message}")]
    DeviceFault { addr: u8, message: String },
}

/// Pluggable I2C peripheral device trait (`02` §4.2).
pub trait I2cDevice: Send {
    /// 7-bit I2C target address (`0x08..=0x77`).
    fn address_7bit(&self) -> u8;

    /// Handles a master-write transfer (`START + addr(W) + data + STOP`).
    fn write(&mut self, data: &[u8]) -> Result<(), I2cError>;

    /// Handles a master-read transfer (`START + addr(R) + len bytes + STOP`).
    fn read(&mut self, len: usize) -> Result<Vec<u8>, I2cError>;

    /// Handles a combined write-then-repeated-start-read transfer (register reads).
    fn write_read(&mut self, tx: &[u8], rx_len: usize) -> Result<Vec<u8>, I2cError> {
        self.write(tx)?;
        self.read(rx_len)
    }
}

struct AttachedI2cDevice {
    endpoint: I2cEndpoint,
    device: Box<dyn I2cDevice>,
}

/// Physical-topology I2C bus managing multiple devices across wired `SDA`/`SCL` nets.
#[derive(Default)]
pub struct I2cBus {
    devices: BTreeMap<String, AttachedI2cDevice>,
    /// Whether physical pull-ups are present on SDA and SCL (can be inferred from circuit or configured).
    sda_pullup_ok: bool,
    scl_pullup_ok: bool,
}

impl I2cBus {
    /// Creates an [`I2cBus`] with pull-ups enabled (typical pull-up resistor present on bus).
    #[must_use]
    pub fn with_pullups() -> Self {
        Self {
            devices: BTreeMap::new(),
            sda_pullup_ok: true,
            scl_pullup_ok: true,
        }
    }

    /// Creates an [`I2cBus`] with explicit `SDA`/`SCL` pull-up presence flags.
    #[must_use]
    pub fn new(sda_pullup_ok: bool, scl_pullup_ok: bool) -> Self {
        Self {
            devices: BTreeMap::new(),
            sda_pullup_ok,
            scl_pullup_ok,
        }
    }

    /// Updates observed electrical pull-up health on `SDA` and `SCL`.
    pub fn set_pullups(&mut self, sda_ok: bool, scl_ok: bool) {
        self.sda_pullup_ok = sda_ok;
        self.scl_pullup_ok = scl_ok;
    }

    /// Attaches a peripheral device at its physical `(instance_id, sda_port, scl_port)` pins.
    pub fn attach_device(
        &mut self,
        endpoint: I2cEndpoint,
        device: Box<dyn I2cDevice>,
    ) -> Result<(), I2cError> {
        let addr = device.address_7bit();
        if !(0x08..=0x77).contains(&addr) {
            return Err(I2cError::InvalidAddress(addr));
        }
        let key = endpoint.instance_id.clone();
        self.devices
            .insert(key, AttachedI2cDevice { endpoint, device });
        Ok(())
    }

    /// Borrows an attached device by instance ID for inspection/testing.
    #[must_use]
    pub fn device(&self, instance_id: &str) -> Option<&dyn I2cDevice> {
        self.devices.get(instance_id).map(|a| a.device.as_ref())
    }

    /// Mutably borrows an attached device by instance ID.
    pub fn device_mut(&mut self, instance_id: &str) -> Option<&mut (dyn I2cDevice + 'static)> {
        self.devices.get_mut(instance_id).map(|a| a.device.as_mut())
    }

    fn verify_bus_and_controller(
        &self,
        controller: &I2cEndpoint,
        topology: &BusTopology,
    ) -> Result<(crate::topology::NetId, crate::topology::NetId), I2cError> {
        if !(self.sda_pullup_ok && self.scl_pullup_ok) {
            return Err(I2cError::MissingPullUp);
        }
        let Some(sda_net) = topology.net_of(&controller.instance_id, &controller.sda_port) else {
            return Err(I2cError::ControllerNotWired(controller.instance_id.clone()));
        };
        let Some(scl_net) = topology.net_of(&controller.instance_id, &controller.scl_port) else {
            return Err(I2cError::ControllerNotWired(controller.instance_id.clone()));
        };
        Ok((sda_net, scl_net))
    }

    fn resolve_target_key(
        &self,
        controller: &I2cEndpoint,
        addr: u8,
        topology: &BusTopology,
    ) -> Result<String, I2cError> {
        if !(0x08..=0x77).contains(&addr) {
            return Err(I2cError::InvalidAddress(addr));
        }
        let (sda_net, scl_net) = self.verify_bus_and_controller(controller, topology)?;
        let mut matched: Vec<String> = Vec::new();
        for (key, attached) in &self.devices {
            if attached.device.address_7bit() != addr {
                continue;
            }
            let dev_sda =
                topology.net_of(&attached.endpoint.instance_id, &attached.endpoint.sda_port);
            let dev_scl =
                topology.net_of(&attached.endpoint.instance_id, &attached.endpoint.scl_port);
            if dev_sda == Some(sda_net) && dev_scl == Some(scl_net) {
                matched.push(key.clone());
            }
        }
        match matched.len() {
            0 => Err(I2cError::Nack(addr)),
            1 => matched.into_iter().next().ok_or(I2cError::Nack(addr)),
            _ => Err(I2cError::AddressCollision { addr }),
        }
    }

    /// Scans all valid 7-bit addresses (`0x08..=0x77`) visible on `controller`'s wired `SDA`/`SCL` nets.
    pub fn scan(
        &self,
        controller: &I2cEndpoint,
        topology: &BusTopology,
    ) -> Result<Vec<u8>, I2cError> {
        let (sda_net, scl_net) = self.verify_bus_and_controller(controller, topology)?;
        let mut found = Vec::new();
        for addr in 0x08..=0x77u8 {
            let any = self.devices.values().any(|a| {
                a.device.address_7bit() == addr
                    && topology.net_of(&a.endpoint.instance_id, &a.endpoint.sda_port)
                        == Some(sda_net)
                    && topology.net_of(&a.endpoint.instance_id, &a.endpoint.scl_port)
                        == Some(scl_net)
            });
            if any {
                found.push(addr);
            }
        }
        Ok(found)
    }

    /// Executes an I2C master-write transaction.
    pub fn write(
        &mut self,
        controller: &I2cEndpoint,
        addr: u8,
        data: &[u8],
        topology: &BusTopology,
    ) -> Result<(), I2cError> {
        let key = self.resolve_target_key(controller, addr, topology)?;
        let Some(target) = self.devices.get_mut(&key) else {
            return Err(I2cError::Nack(addr));
        };
        target.device.write(data)
    }

    /// Executes an I2C master-read transaction.
    pub fn read(
        &mut self,
        controller: &I2cEndpoint,
        addr: u8,
        len: usize,
        topology: &BusTopology,
    ) -> Result<Vec<u8>, I2cError> {
        let key = self.resolve_target_key(controller, addr, topology)?;
        let Some(target) = self.devices.get_mut(&key) else {
            return Err(I2cError::Nack(addr));
        };
        target.device.read(len)
    }

    /// Executes a combined I2C `write_read` register transaction.
    pub fn write_read(
        &mut self,
        controller: &I2cEndpoint,
        addr: u8,
        tx: &[u8],
        rx_len: usize,
        topology: &BusTopology,
    ) -> Result<Vec<u8>, I2cError> {
        let key = self.resolve_target_key(controller, addr, topology)?;
        let Some(target) = self.devices.get_mut(&key) else {
            return Err(I2cError::Nack(addr));
        };
        target.device.write_read(tx, rx_len)
    }
}

// ---------------------------------------------------------------------------
// Built-in reference I2C devices (EEPROM 24C32 & PCF8574 8-bit GPIO expander)
// ---------------------------------------------------------------------------

/// 4 KiB I2C serial EEPROM (`24C32`, 16-bit big-endian memory pointer, default address `0x50`).
#[derive(Debug, Clone)]
pub struct Eeprom24Cxx {
    address: u8,
    memory: Vec<u8>,
    cursor: usize,
}

impl Eeprom24Cxx {
    /// Creates an erased (`0xFF`) EEPROM of `size_bytes` (e.g., `4096` for 24C32).
    #[must_use]
    pub fn new(address: u8, size_bytes: usize) -> Self {
        Self {
            address,
            memory: vec![0xFF; size_bytes.max(16)],
            cursor: 0,
        }
    }

    /// Inspects raw EEPROM memory bytes.
    #[must_use]
    pub fn memory(&self) -> &[u8] {
        &self.memory
    }
}

impl I2cDevice for Eeprom24Cxx {
    fn address_7bit(&self) -> u8 {
        self.address
    }

    fn write(&mut self, data: &[u8]) -> Result<(), I2cError> {
        let (Some(&hi), Some(&lo)) = (data.first(), data.get(1)) else {
            return Err(I2cError::DeviceFault {
                addr: self.address,
                message: "24Cxx requires 2-byte memory address".to_owned(),
            });
        };
        let start = usize::from(u16::from_be_bytes([hi, lo]));
        self.cursor = start % self.memory.len();
        if let Some(payload) = data.get(2..) {
            for &byte in payload {
                if let Some(slot) = self.memory.get_mut(self.cursor) {
                    *slot = byte;
                }
                self.cursor = (self.cursor + 1) % self.memory.len();
            }
        }
        Ok(())
    }

    fn read(&mut self, len: usize) -> Result<Vec<u8>, I2cError> {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            let b = self.memory.get(self.cursor).copied().unwrap_or(0xFF);
            out.push(b);
            self.cursor = (self.cursor + 1) % self.memory.len();
        }
        Ok(out)
    }
}

/// PCF8574 8-bit I2C GPIO expander (default address `0x27`, widely used on HD44780 LCD backpacks).
#[derive(Debug, Clone)]
pub struct Pcf8574Expander {
    address: u8,
    latch: u8,
    input_pins: u8,
}

impl Pcf8574Expander {
    #[must_use]
    pub const fn new(address: u8) -> Self {
        Self {
            address,
            latch: 0xFF,
            input_pins: 0xFF,
        }
    }

    /// Sets external pin input mask (open-drain quasi-bidirectional readback `latch & input_pins`).
    pub fn set_external_inputs(&mut self, pins: u8) {
        self.input_pins = pins;
    }

    /// Returns the last written 8-bit output latch.
    #[must_use]
    pub const fn output_latch(&self) -> u8 {
        self.latch
    }
}

impl I2cDevice for Pcf8574Expander {
    fn address_7bit(&self) -> u8 {
        self.address
    }

    fn write(&mut self, data: &[u8]) -> Result<(), I2cError> {
        if let Some(&last) = data.last() {
            self.latch = last;
        }
        Ok(())
    }

    fn read(&mut self, len: usize) -> Result<Vec<u8>, I2cError> {
        Ok(vec![self.latch & self.input_pins; len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i2c_enforces_pullups_wiring_scan_and_multi_device_transactions() {
        let mut topo = BusTopology::new();
        topo.connect("uno", "A4", "eeprom1", "SDA");
        topo.connect("uno", "A5", "eeprom1", "SCL");
        topo.connect("uno", "A4", "gpio1", "SDA");
        topo.connect("uno", "A5", "gpio1", "SCL");
        topo.rebuild();

        let ctrl = I2cEndpoint::new("uno", "A4", "A5");
        let mut bus = I2cBus::new(false, false);
        bus.attach_device(
            I2cEndpoint::new("eeprom1", "SDA", "SCL"),
            Box::new(Eeprom24Cxx::new(0x50, 4096)),
        )
        .unwrap();
        bus.attach_device(
            I2cEndpoint::new("gpio1", "SDA", "SCL"),
            Box::new(Pcf8574Expander::new(0x27)),
        )
        .unwrap();

        // Missing pull-ups MUST fail cleanly
        assert_eq!(bus.scan(&ctrl, &topo), Err(I2cError::MissingPullUp));

        // Enabling pull-ups makes both devices visible on bus scan
        bus.set_pullups(true, true);
        assert_eq!(bus.scan(&ctrl, &topo).unwrap(), vec![0x27, 0x50]);

        // Write & read back EEPROM bytes across shared bus
        bus.write(&ctrl, 0x50, &[0x00, 0x10, 0xDE, 0xAD, 0xBE, 0xEF], &topo)
            .unwrap();
        let got = bus
            .write_read(&ctrl, 0x50, &[0x00, 0x10], 4, &topo)
            .unwrap();
        assert_eq!(got, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        // Write & read PCF8574 without interfering with EEPROM
        bus.write(&ctrl, 0x27, &[0xA5], &topo).unwrap();
        assert_eq!(bus.read(&ctrl, 0x27, 1, &topo).unwrap(), vec![0xA5]);

        // Unwired address returns NACK
        assert_eq!(bus.read(&ctrl, 0x3C, 1, &topo), Err(I2cError::Nack(0x3C)));
    }
}
