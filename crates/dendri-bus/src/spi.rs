//! Physical-topology SPI bus fabric (`02-simulation-and-electrical-engine.md` §4.3).
//!
//! Enforces:
//! - Shared `MOSI`, `MISO`, and `SCK` physical nets in [`BusTopology`].
//! - Per-device active-low Chip Select (`CS`) voltage verification (`V_CS < v_il_v`).
//! - Honest multi-device coexistence: 0 selected devices returns idle `0xFF`;
//!   >1 simultaneously asserted `CS` lines raises [`SpiError::MultipleCsActive`] contention!

use crate::topology::BusTopology;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Standard SPI clock polarity / phase modes (`CPOL`, `CPHA`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SpiMode {
    #[default]
    Mode0,
    Mode1,
    Mode2,
    Mode3,
}

/// Physical SPI controller pin binding (`MOSI`, `MISO`, `SCK`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiMasterPins {
    pub instance_id: String,
    pub mosi_port: String,
    pub miso_port: String,
    pub sck_port: String,
    pub mode: SpiMode,
}

/// Physical SPI peripheral pin binding (`MOSI`, `MISO`, `SCK`, `CS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiSlavePins {
    pub instance_id: String,
    pub mosi_port: String,
    pub miso_port: String,
    pub sck_port: String,
    pub cs_port: String,
}

/// Errors emitted by [`SpiBus`] transfers.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SpiError {
    #[error("SPI controller `{0}` is missing wired MOSI/MISO/SCK nets")]
    ControllerNotWired(String),
    #[error("SPI bus contention: multiple devices have active-low CS simultaneously ({devices:?})")]
    MultipleCsActive { devices: Vec<String> },
    #[error(
        "SPI mode mismatch on selected device `{device}` (controller={master:?}, device={slave:?})"
    )]
    ModeMismatch {
        device: String,
        master: SpiMode,
        slave: SpiMode,
    },
}

/// Pluggable SPI peripheral trait (`02` §4.3).
pub trait SpiDevice: Send {
    /// Supported SPI clock mode.
    fn mode(&self) -> SpiMode {
        SpiMode::Mode0
    }

    /// Full-duplex byte exchange while active-low `CS` is asserted.
    fn transfer(&mut self, mosi: &[u8]) -> Vec<u8>;

    /// Notifies the peripheral that `CS` returned high (end of transaction).
    fn deselect(&mut self) {}
}

struct AttachedSpiSlave {
    pins: SpiSlavePins,
    device: Box<dyn SpiDevice>,
}

/// Physical-topology SPI bus supporting multi-device active-low `CS` selection.
#[derive(Default)]
pub struct SpiBus {
    slaves: BTreeMap<String, AttachedSpiSlave>,
    /// Logic-low threshold (V) below which an active-low `CS` line counts as asserted (default `0.8 V`).
    cs_v_il_v: f64,
}

impl SpiBus {
    /// Creates an [`SpiBus`] with standard CMOS/TTL `CS` low threshold (`0.8 V`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            slaves: BTreeMap::new(),
            cs_v_il_v: 0.8,
        }
    }

    /// Configures the `CS` active-low voltage threshold.
    pub fn set_cs_threshold_v(&mut self, v_il_v: f64) {
        self.cs_v_il_v = v_il_v;
    }

    /// Attaches a peripheral device on its physical SPI + `CS` pins.
    pub fn attach_device(&mut self, pins: SpiSlavePins, device: Box<dyn SpiDevice>) {
        let key = pins.instance_id.clone();
        self.slaves.insert(key, AttachedSpiSlave { pins, device });
    }

    /// Executes a full-duplex SPI transfer driven by `master`, sampling each slave's physical `CS`
    /// terminal voltage through `cs_voltage_of(instance_id, cs_port)`.
    pub fn transfer<F>(
        &mut self,
        master: &SpiMasterPins,
        mosi: &[u8],
        topology: &BusTopology,
        cs_voltage_of: F,
    ) -> Result<Vec<u8>, SpiError>
    where
        F: Fn(&str, &str) -> f64,
    {
        let (Some(mosi_net), Some(miso_net), Some(sck_net)) = (
            topology.net_of(&master.instance_id, &master.mosi_port),
            topology.net_of(&master.instance_id, &master.miso_port),
            topology.net_of(&master.instance_id, &master.sck_port),
        ) else {
            return Err(SpiError::ControllerNotWired(master.instance_id.clone()));
        };

        let mut selected: Vec<String> = Vec::new();
        for (id, slave) in &self.slaves {
            let wired = topology.net_of(&slave.pins.instance_id, &slave.pins.mosi_port)
                == Some(mosi_net)
                && topology.net_of(&slave.pins.instance_id, &slave.pins.miso_port)
                    == Some(miso_net)
                && topology.net_of(&slave.pins.instance_id, &slave.pins.sck_port) == Some(sck_net);
            if !wired {
                continue;
            }
            let v_cs = cs_voltage_of(&slave.pins.instance_id, &slave.pins.cs_port);
            if v_cs < self.cs_v_il_v {
                selected.push(id.clone());
            }
        }

        match selected.len() {
            0 => Ok(vec![0xFF; mosi.len()]),
            1 => {
                let Some(sel_id) = selected.first() else {
                    return Ok(vec![0xFF; mosi.len()]);
                };
                let Some(slave) = self.slaves.get_mut(sel_id) else {
                    return Ok(vec![0xFF; mosi.len()]);
                };
                if slave.device.mode() != master.mode {
                    return Err(SpiError::ModeMismatch {
                        device: sel_id.clone(),
                        master: master.mode,
                        slave: slave.device.mode(),
                    });
                }
                let mut rx = slave.device.transfer(mosi);
                rx.resize(mosi.len(), 0xFF);
                Ok(rx)
            }
            _ => Err(SpiError::MultipleCsActive { devices: selected }),
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in reference SPI devices (25xx Flash & 74HC595 Shift Register)
// ---------------------------------------------------------------------------

/// Simple SPI NOR flash model supporting JEDEC ID (`0x9F`), Write Enable (`0x06`),
/// Page Program (`0x02`), and Read (`0x03`).
#[derive(Debug, Clone)]
pub struct SpiFlash25xx {
    jedec_id: [u8; 3],
    write_enabled: bool,
    memory: Vec<u8>,
}

impl SpiFlash25xx {
    #[must_use]
    pub fn new(size_bytes: usize) -> Self {
        Self {
            jedec_id: [0xEF, 0x40, 0x16],
            write_enabled: false,
            memory: vec![0xFF; size_bytes.max(256)],
        }
    }
}

impl SpiDevice for SpiFlash25xx {
    fn transfer(&mut self, mosi: &[u8]) -> Vec<u8> {
        let mut miso = vec![0xFF; mosi.len()];
        let Some(&cmd) = mosi.first() else {
            return miso;
        };
        match cmd {
            0x9F => {
                for (i, &id_b) in self.jedec_id.iter().enumerate() {
                    if let Some(slot) = miso.get_mut(i + 1) {
                        *slot = id_b;
                    }
                }
            }
            0x06 => {
                self.write_enabled = true;
            }
            0x04 => {
                self.write_enabled = false;
            }
            0x02 => {
                if self.write_enabled
                    && let (Some(&a2), Some(&a1), Some(&a0)) =
                        (mosi.get(1), mosi.get(2), mosi.get(3))
                {
                    let mut addr =
                        ((usize::from(a2) << 16) | (usize::from(a1) << 8) | usize::from(a0))
                            % self.memory.len();
                    if let Some(payload) = mosi.get(4..) {
                        for &b in payload {
                            if let Some(slot) = self.memory.get_mut(addr) {
                                *slot = b;
                            }
                            addr = (addr + 1) % self.memory.len();
                        }
                    }
                    self.write_enabled = false;
                }
            }
            0x03 => {
                if let (Some(&a2), Some(&a1), Some(&a0)) = (mosi.get(1), mosi.get(2), mosi.get(3)) {
                    let mut addr =
                        ((usize::from(a2) << 16) | (usize::from(a1) << 8) | usize::from(a0))
                            % self.memory.len();
                    for slot in miso.iter_mut().skip(4) {
                        *slot = self.memory.get(addr).copied().unwrap_or(0xFF);
                        addr = (addr + 1) % self.memory.len();
                    }
                }
            }
            _ => {}
        }
        miso
    }
}

/// 8-bit SPI shift register (`74HC595`) shifting out its previous byte on `MISO`.
#[derive(Debug, Clone, Default)]
pub struct ShiftRegister74Hc595 {
    register: u8,
}

impl ShiftRegister74Hc595 {
    #[must_use]
    pub const fn value(&self) -> u8 {
        self.register
    }
}

impl SpiDevice for ShiftRegister74Hc595 {
    fn transfer(&mut self, mosi: &[u8]) -> Vec<u8> {
        let mut miso = Vec::with_capacity(mosi.len());
        for &b in mosi {
            miso.push(self.register);
            self.register = b;
        }
        miso
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spi_coexists_multi_device_via_cs_voltage_and_detects_contention() {
        let mut topo = BusTopology::new();
        for dev in ["flash1", "sr1"] {
            topo.connect("uno", "D11", dev, "MOSI");
            topo.connect("uno", "D12", dev, "MISO");
            topo.connect("uno", "D13", dev, "SCK");
        }
        topo.rebuild();

        let master = SpiMasterPins {
            instance_id: "uno".to_owned(),
            mosi_port: "D11".to_owned(),
            miso_port: "D12".to_owned(),
            sck_port: "D13".to_owned(),
            mode: SpiMode::Mode0,
        };

        let mut bus = SpiBus::new();
        bus.attach_device(
            SpiSlavePins {
                instance_id: "flash1".to_owned(),
                mosi_port: "MOSI".to_owned(),
                miso_port: "MISO".to_owned(),
                sck_port: "SCK".to_owned(),
                cs_port: "CS".to_owned(),
            },
            Box::new(SpiFlash25xx::new(1024)),
        );
        bus.attach_device(
            SpiSlavePins {
                instance_id: "sr1".to_owned(),
                mosi_port: "MOSI".to_owned(),
                miso_port: "MISO".to_owned(),
                sck_port: "SCK".to_owned(),
                cs_port: "CS".to_owned(),
            },
            Box::new(ShiftRegister74Hc595::default()),
        );

        // No CS active -> idle 0xFF
        let idle = bus
            .transfer(&master, &[0x9F, 0, 0, 0], &topo, |_, _| 5.0)
            .unwrap();
        assert_eq!(idle, vec![0xFF, 0xFF, 0xFF, 0xFF]);

        // Select flash1 (0.0 V) while sr1 high (5.0 V) -> read JEDEC ID
        let id = bus
            .transfer(&master, &[0x9F, 0, 0, 0], &topo, |inst, _| {
                if inst == "flash1" { 0.0 } else { 5.0 }
            })
            .unwrap();
        assert_eq!(id, vec![0xFF, 0xEF, 0x40, 0x16]);

        // Select sr1 -> shift bytes
        let r1 = bus
            .transfer(&master, &[0x5A, 0xC3], &topo, |inst, _| {
                if inst == "sr1" { 0.0 } else { 5.0 }
            })
            .unwrap();
        assert_eq!(r1, vec![0x00, 0x5A]);

        // Both CS low simultaneously -> bus contention error!
        let err = bus
            .transfer(&master, &[0x9F], &topo, |_, _| 0.0)
            .unwrap_err();
        assert!(matches!(err, SpiError::MultipleCsActive { .. }));
    }
}
