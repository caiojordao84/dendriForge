//! Physical-topology virtual bus fabric (`02-simulation-and-electrical-engine.md` §4, ADR-011).
//!
//! All communication buses are strictly coupled to real canvas wiring (`[[wires]]`):
//! - [`UartBus`]: Physical `TX -> RX` net verification, baud/framing mismatch checks,
//!   exact bit-time propagation scheduling (`time_us`), and bounded RX/TX FIFOs.
//! - [`I2cBus`]: Shared `SDA`/`SCL` net & pull-up verification, 7-bit bus scan,
//!   multi-byte register transactions, and multi-device coexistence via [`I2cDevice`].
//! - [`SpiBus`]: Shared `MOSI`/`MISO`/`SCK` nets + per-device active-low `CS` voltage
//!   verification (detecting simultaneous CS bus contention) via [`SpiDevice`].
//! - [`ModbusNetwork`]: Virtual Ethernet / Modbus TCP LAN routing by IPv4 + Unit ID,
//!   supporting `FC1..=FC6`, `FC15`, `FC16`, full MBAP+PDU framing, and standard
//!   Modbus exception codes (`01`..=`04`).

pub mod i2c;
pub mod modbus;
pub mod spi;
pub mod topology;
pub mod uart;

pub use i2c::{Eeprom24Cxx, I2cBus, I2cDevice, I2cEndpoint, I2cError, Pcf8574Expander};
pub use modbus::{
    MbapFrame, ModbusError, ModbusException, ModbusFunction, ModbusNetwork, ModbusNodeConfig,
    ModbusRegisterBank, ModbusRequest, ModbusResponse,
};
pub use spi::{
    ShiftRegister74Hc595, SpiBus, SpiDevice, SpiError, SpiFlash25xx, SpiMasterPins, SpiMode,
    SpiSlavePins,
};
pub use topology::{BusTopology, EndpointKey, NetId, TopologyError};
pub use uart::{UartBus, UartEndpointConfig, UartError, UartNodeStats, UartParity, UartStopBits};
