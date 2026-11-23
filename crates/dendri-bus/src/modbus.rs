//! Virtual Ethernet / Modbus TCP LAN fabric (`02-simulation-and-electrical-engine.md` §4.4).
//!
//! Supports:
//! - Multi-node virtual LAN binding boards/servers by `Ipv4Addr` + `unit_id: u8`, with optional
//!   physical Ethernet topology verification across [`BusTopology`].
//! - Full Modbus TCP MBAP header (7 bytes) + PDU binary framing (`encode` / `decode`).
//! - Complete standard function codes: `FC1` (Read Coils), `FC2` (Read Discrete Inputs),
//!   `FC3` (Read Holding Registers), `FC4` (Read Input Registers), `FC5` (Write Single Coil),
//!   `FC6` (Write Single Register), `FC15` (Write Multiple Coils), `FC16` (Write Multiple Registers).
//! - Standard Modbus exception codes `0x01..=0x04` and `OB_COMM_EVENT` (`OB50`) write notifications.

use crate::topology::BusTopology;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use thiserror::Error;

/// Standard Modbus TCP function codes supported by [`ModbusNetwork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ModbusFunction {
    ReadCoils = 0x01,
    ReadDiscreteInputs = 0x02,
    ReadHoldingRegisters = 0x03,
    ReadInputRegisters = 0x04,
    WriteSingleCoil = 0x05,
    WriteSingleRegister = 0x06,
    WriteMultipleCoils = 0x0F,
    WriteMultipleRegisters = 0x10,
}

impl ModbusFunction {
    #[must_use]
    pub const fn from_u8(code: u8) -> Option<Self> {
        match code {
            0x01 => Some(Self::ReadCoils),
            0x02 => Some(Self::ReadDiscreteInputs),
            0x03 => Some(Self::ReadHoldingRegisters),
            0x04 => Some(Self::ReadInputRegisters),
            0x05 => Some(Self::WriteSingleCoil),
            0x06 => Some(Self::WriteSingleRegister),
            0x0F => Some(Self::WriteMultipleCoils),
            0x10 => Some(Self::WriteMultipleRegisters),
            _ => None,
        }
    }
}

/// Normative Modbus exception codes (`01`..=`04`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ModbusException {
    IllegalFunction = 0x01,
    IllegalDataAddress = 0x02,
    IllegalDataValue = 0x03,
    ServerDeviceFailure = 0x04,
}

impl ModbusException {
    #[must_use]
    pub const fn from_u8(code: u8) -> Option<Self> {
        match code {
            0x01 => Some(Self::IllegalFunction),
            0x02 => Some(Self::IllegalDataAddress),
            0x03 => Some(Self::IllegalDataValue),
            0x04 => Some(Self::ServerDeviceFailure),
            _ => None,
        }
    }
}

/// Transport-layer Modbus TCP errors (distinct from protocol-level [`ModbusException`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModbusError {
    #[error("no Modbus TCP node listening at {ip} (unit_id={unit_id})")]
    UnreachableHost { ip: Ipv4Addr, unit_id: u8 },
    #[error("Ethernet topology mismatch: `{client}` and `{server}` are not on the same LAN net")]
    EthernetDisconnected { client: String, server: String },
    #[error("malformed MBAP/PDU frame: {0}")]
    MalformedFrame(String),
}

/// Raw Modbus TCP MBAP + PDU wire frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbapFrame {
    pub transaction_id: u16,
    pub protocol_id: u16,
    pub unit_id: u8,
    pub pdu: Vec<u8>,
}

impl MbapFrame {
    /// Encodes the frame into big-endian Modbus TCP bytes (`7-byte MBAP + PDU`).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let len_field = u16::try_from(self.pdu.len().saturating_add(1)).unwrap_or(u16::MAX);
        let mut out = Vec::with_capacity(7 + self.pdu.len());
        out.extend_from_slice(&self.transaction_id.to_be_bytes());
        out.extend_from_slice(&self.protocol_id.to_be_bytes());
        out.extend_from_slice(&len_field.to_be_bytes());
        out.push(self.unit_id);
        out.extend_from_slice(&self.pdu);
        out
    }

    /// Decodes a raw Modbus TCP byte buffer into an [`MbapFrame`].
    pub fn decode(bytes: &[u8]) -> Result<Self, ModbusError> {
        let (
            Some(&t0),
            Some(&t1),
            Some(&p0),
            Some(&p1),
            Some(&l0),
            Some(&l1),
            Some(&unit_id),
            Some(pdu_slice),
        ) = (
            bytes.first(),
            bytes.get(1),
            bytes.get(2),
            bytes.get(3),
            bytes.get(4),
            bytes.get(5),
            bytes.get(6),
            bytes.get(7..),
        )
        else {
            return Err(ModbusError::MalformedFrame(
                "frame shorter than 7-byte MBAP header".to_owned(),
            ));
        };
        let transaction_id = u16::from_be_bytes([t0, t1]);
        let protocol_id = u16::from_be_bytes([p0, p1]);
        if protocol_id != 0 {
            return Err(ModbusError::MalformedFrame(format!(
                "non-zero MBAP protocol_id {protocol_id}"
            )));
        }
        let length = usize::from(u16::from_be_bytes([l0, l1]));
        if length < 2 || pdu_slice.len() + 1 != length {
            return Err(ModbusError::MalformedFrame(format!(
                "MBAP length field {length} mismatches payload len {}",
                pdu_slice.len() + 1
            )));
        }
        Ok(Self {
            transaction_id,
            protocol_id,
            unit_id,
            pdu: pdu_slice.to_vec(),
        })
    }
}

/// High-level typed Modbus client request (FC1–FC6, FC15, FC16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModbusRequest {
    ReadCoils { start: u16, count: u16 },
    ReadDiscreteInputs { start: u16, count: u16 },
    ReadHoldingRegisters { start: u16, count: u16 },
    ReadInputRegisters { start: u16, count: u16 },
    WriteSingleCoil { addr: u16, value: bool },
    WriteSingleRegister { addr: u16, value: u16 },
    WriteMultipleCoils { start: u16, values: Vec<bool> },
    WriteMultipleRegisters { start: u16, values: Vec<u16> },
}

impl ModbusRequest {
    /// Serializes this typed request into a standard Modbus PDU byte vector.
    #[must_use]
    pub fn to_pdu(&self) -> Vec<u8> {
        match self {
            Self::ReadCoils { start, count } => {
                let mut v = vec![ModbusFunction::ReadCoils as u8];
                v.extend_from_slice(&start.to_be_bytes());
                v.extend_from_slice(&count.to_be_bytes());
                v
            }
            Self::ReadDiscreteInputs { start, count } => {
                let mut v = vec![ModbusFunction::ReadDiscreteInputs as u8];
                v.extend_from_slice(&start.to_be_bytes());
                v.extend_from_slice(&count.to_be_bytes());
                v
            }
            Self::ReadHoldingRegisters { start, count } => {
                let mut v = vec![ModbusFunction::ReadHoldingRegisters as u8];
                v.extend_from_slice(&start.to_be_bytes());
                v.extend_from_slice(&count.to_be_bytes());
                v
            }
            Self::ReadInputRegisters { start, count } => {
                let mut v = vec![ModbusFunction::ReadInputRegisters as u8];
                v.extend_from_slice(&start.to_be_bytes());
                v.extend_from_slice(&count.to_be_bytes());
                v
            }
            Self::WriteSingleCoil { addr, value } => {
                let mut v = vec![ModbusFunction::WriteSingleCoil as u8];
                v.extend_from_slice(&addr.to_be_bytes());
                let raw: u16 = if *value { 0xFF00 } else { 0x0000 };
                v.extend_from_slice(&raw.to_be_bytes());
                v
            }
            Self::WriteSingleRegister { addr, value } => {
                let mut v = vec![ModbusFunction::WriteSingleRegister as u8];
                v.extend_from_slice(&addr.to_be_bytes());
                v.extend_from_slice(&value.to_be_bytes());
                v
            }
            Self::WriteMultipleCoils { start, values } => {
                let qty = u16::try_from(values.len()).unwrap_or(u16::MAX);
                let packed = pack_bits(values);
                let byte_cnt = u8::try_from(packed.len()).unwrap_or(u8::MAX);
                let mut v = vec![ModbusFunction::WriteMultipleCoils as u8];
                v.extend_from_slice(&start.to_be_bytes());
                v.extend_from_slice(&qty.to_be_bytes());
                v.push(byte_cnt);
                v.extend_from_slice(&packed);
                v
            }
            Self::WriteMultipleRegisters { start, values } => {
                let qty = u16::try_from(values.len()).unwrap_or(u16::MAX);
                let byte_cnt = u8::try_from(values.len().saturating_mul(2)).unwrap_or(u8::MAX);
                let mut v = vec![ModbusFunction::WriteMultipleRegisters as u8];
                v.extend_from_slice(&start.to_be_bytes());
                v.extend_from_slice(&qty.to_be_bytes());
                v.push(byte_cnt);
                for &r in values {
                    v.extend_from_slice(&r.to_be_bytes());
                }
                v
            }
        }
    }
}

/// Typed response returned by a Modbus TCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModbusResponse {
    Coils(Vec<bool>),
    DiscreteInputs(Vec<bool>),
    HoldingRegisters(Vec<u16>),
    InputRegisters(Vec<u16>),
    WriteSingleCoilAck {
        addr: u16,
        value: bool,
    },
    WriteSingleRegisterAck {
        addr: u16,
        value: u16,
    },
    WriteMultipleAck {
        function: ModbusFunction,
        start: u16,
        count: u16,
    },
    Exception {
        function_code: u8,
        exception: ModbusException,
    },
}

/// 16-bit addressable Modbus register & coil memory bank for a virtual PLC/MCU server.
#[derive(Debug, Clone)]
pub struct ModbusRegisterBank {
    pub coils: Vec<bool>,
    pub discrete_inputs: Vec<bool>,
    pub holding_registers: Vec<u16>,
    pub input_registers: Vec<u16>,
    pub faulted: bool,
}

impl ModbusRegisterBank {
    /// Creates a zero-initialized register bank with explicit sizes for all 4 tables.
    #[must_use]
    pub fn new(coils: usize, discrete_inputs: usize, holding: usize, input: usize) -> Self {
        Self {
            coils: vec![false; coils],
            discrete_inputs: vec![false; discrete_inputs],
            holding_registers: vec![0; holding],
            input_registers: vec![0; input],
            faulted: false,
        }
    }

    /// Standard industrial bank (`256` coils, `256` discrete inputs, `256` holding, `256` input).
    #[must_use]
    pub fn standard_256() -> Self {
        Self::new(256, 256, 256, 256)
    }

    /// Executes a raw Modbus PDU against this register bank, returning `(response_pdu, did_write)`.
    pub fn execute_pdu(&mut self, pdu: &[u8]) -> (Vec<u8>, bool) {
        let Some(&fc_raw) = pdu.first() else {
            return (vec![0x80, ModbusException::IllegalFunction as u8], false);
        };
        if self.faulted {
            return (
                vec![fc_raw | 0x80, ModbusException::ServerDeviceFailure as u8],
                false,
            );
        }
        let Some(fc) = ModbusFunction::from_u8(fc_raw) else {
            return (
                vec![fc_raw | 0x80, ModbusException::IllegalFunction as u8],
                false,
            );
        };

        let exc =
            |code: ModbusException| -> (Vec<u8>, bool) { (vec![fc_raw | 0x80, code as u8], false) };

        match fc {
            ModbusFunction::ReadCoils => {
                let Some((start, count)) = read_u16_pair(pdu, 1) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                if count == 0 || count > 2000 {
                    return exc(ModbusException::IllegalDataValue);
                }
                let end = usize::from(start).saturating_add(usize::from(count));
                let Some(slice) = self.coils.get(usize::from(start)..end) else {
                    return exc(ModbusException::IllegalDataAddress);
                };
                let packed = pack_bits(slice);
                let mut resp = vec![fc_raw, u8::try_from(packed.len()).unwrap_or(u8::MAX)];
                resp.extend_from_slice(&packed);
                (resp, false)
            }
            ModbusFunction::ReadDiscreteInputs => {
                let Some((start, count)) = read_u16_pair(pdu, 1) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                if count == 0 || count > 2000 {
                    return exc(ModbusException::IllegalDataValue);
                }
                let end = usize::from(start).saturating_add(usize::from(count));
                let Some(slice) = self.discrete_inputs.get(usize::from(start)..end) else {
                    return exc(ModbusException::IllegalDataAddress);
                };
                let packed = pack_bits(slice);
                let mut resp = vec![fc_raw, u8::try_from(packed.len()).unwrap_or(u8::MAX)];
                resp.extend_from_slice(&packed);
                (resp, false)
            }
            ModbusFunction::ReadHoldingRegisters => {
                let Some((start, count)) = read_u16_pair(pdu, 1) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                if count == 0 || count > 125 {
                    return exc(ModbusException::IllegalDataValue);
                }
                let end = usize::from(start).saturating_add(usize::from(count));
                let Some(slice) = self.holding_registers.get(usize::from(start)..end) else {
                    return exc(ModbusException::IllegalDataAddress);
                };
                let mut resp = vec![fc_raw, u8::try_from(slice.len() * 2).unwrap_or(u8::MAX)];
                for &reg in slice {
                    resp.extend_from_slice(&reg.to_be_bytes());
                }
                (resp, false)
            }
            ModbusFunction::ReadInputRegisters => {
                let Some((start, count)) = read_u16_pair(pdu, 1) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                if count == 0 || count > 125 {
                    return exc(ModbusException::IllegalDataValue);
                }
                let end = usize::from(start).saturating_add(usize::from(count));
                let Some(slice) = self.input_registers.get(usize::from(start)..end) else {
                    return exc(ModbusException::IllegalDataAddress);
                };
                let mut resp = vec![fc_raw, u8::try_from(slice.len() * 2).unwrap_or(u8::MAX)];
                for &reg in slice {
                    resp.extend_from_slice(&reg.to_be_bytes());
                }
                (resp, false)
            }
            ModbusFunction::WriteSingleCoil => {
                let Some((addr, raw_val)) = read_u16_pair(pdu, 1) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                let val = match raw_val {
                    0xFF00 => true,
                    0x0000 => false,
                    _ => return exc(ModbusException::IllegalDataValue),
                };
                let Some(slot) = self.coils.get_mut(usize::from(addr)) else {
                    return exc(ModbusException::IllegalDataAddress);
                };
                *slot = val;
                let mut resp = vec![fc_raw];
                resp.extend_from_slice(&addr.to_be_bytes());
                resp.extend_from_slice(&raw_val.to_be_bytes());
                (resp, true)
            }
            ModbusFunction::WriteSingleRegister => {
                let Some((addr, val)) = read_u16_pair(pdu, 1) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                let Some(slot) = self.holding_registers.get_mut(usize::from(addr)) else {
                    return exc(ModbusException::IllegalDataAddress);
                };
                *slot = val;
                let mut resp = vec![fc_raw];
                resp.extend_from_slice(&addr.to_be_bytes());
                resp.extend_from_slice(&val.to_be_bytes());
                (resp, true)
            }
            ModbusFunction::WriteMultipleCoils => {
                let Some((start, count)) = read_u16_pair(pdu, 1) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                let Some(&byte_cnt) = pdu.get(5) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                let expected_bytes = usize::from(count).div_ceil(8);
                if count == 0
                    || count > 1968
                    || usize::from(byte_cnt) != expected_bytes
                    || pdu.len() < 6 + expected_bytes
                {
                    return exc(ModbusException::IllegalDataValue);
                }
                let end = usize::from(start).saturating_add(usize::from(count));
                let Some(slice) = self.coils.get_mut(usize::from(start)..end) else {
                    return exc(ModbusException::IllegalDataAddress);
                };
                for (i, slot) in slice.iter_mut().enumerate() {
                    let byte = pdu.get(6 + i / 8).copied().unwrap_or(0);
                    *slot = ((byte >> (i % 8)) & 1) != 0;
                }
                let mut resp = vec![fc_raw];
                resp.extend_from_slice(&start.to_be_bytes());
                resp.extend_from_slice(&count.to_be_bytes());
                (resp, true)
            }
            ModbusFunction::WriteMultipleRegisters => {
                let Some((start, count)) = read_u16_pair(pdu, 1) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                let Some(&byte_cnt) = pdu.get(5) else {
                    return exc(ModbusException::IllegalDataValue);
                };
                let expected_bytes = usize::from(count) * 2;
                if count == 0
                    || count > 123
                    || usize::from(byte_cnt) != expected_bytes
                    || pdu.len() < 6 + expected_bytes
                {
                    return exc(ModbusException::IllegalDataValue);
                }
                let end = usize::from(start).saturating_add(usize::from(count));
                let Some(slice) = self.holding_registers.get_mut(usize::from(start)..end) else {
                    return exc(ModbusException::IllegalDataAddress);
                };
                for (i, slot) in slice.iter_mut().enumerate() {
                    let hi = pdu.get(6 + i * 2).copied().unwrap_or(0);
                    let lo = pdu.get(6 + i * 2 + 1).copied().unwrap_or(0);
                    *slot = u16::from_be_bytes([hi, lo]);
                }
                let mut resp = vec![fc_raw];
                resp.extend_from_slice(&start.to_be_bytes());
                resp.extend_from_slice(&count.to_be_bytes());
                (resp, true)
            }
        }
    }
}

/// Virtual Ethernet / Modbus TCP node identity & optional physical port binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModbusNodeConfig {
    pub node_id: String,
    pub ip: Ipv4Addr,
    pub unit_id: u8,
    /// Optional physical `(instance_id, ethernet_port)` for strict canvas Ethernet net verification.
    pub ethernet_port: Option<(String, String)>,
}

#[derive(Debug, Clone)]
struct ModbusNode {
    config: ModbusNodeConfig,
    bank: ModbusRegisterBank,
}

/// Multi-node Virtual Ethernet / Modbus TCP LAN fabric (`02` §4.4).
#[derive(Debug, Clone, Default)]
pub struct ModbusNetwork {
    nodes: BTreeMap<String, ModbusNode>,
    ip_unit_index: BTreeMap<(Ipv4Addr, u8), String>,
    next_tx_id: u16,
    /// Node IDs whose registers/coils were modified by remote writes (triggers `OB_COMM_EVENT`).
    write_notifications: BTreeSet<String>,
}

impl ModbusNetwork {
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            ip_unit_index: BTreeMap::new(),
            next_tx_id: 1,
            write_notifications: BTreeSet::new(),
        }
    }

    /// Registers a Modbus TCP server/client node on the virtual LAN.
    pub fn register_node(&mut self, config: ModbusNodeConfig, bank: ModbusRegisterBank) {
        let key = config.node_id.clone();
        self.ip_unit_index
            .insert((config.ip, config.unit_id), key.clone());
        self.nodes.insert(key, ModbusNode { config, bank });
    }

    /// Borrows a node's register bank for direct board I/O mirroring or inspection.
    #[must_use]
    pub fn bank(&self, node_id: &str) -> Option<&ModbusRegisterBank> {
        self.nodes.get(node_id).map(|n| &n.bank)
    }

    /// Mutably borrows a node's register bank so a PLC scan can update input registers / read coils.
    pub fn bank_mut(&mut self, node_id: &str) -> Option<&mut ModbusRegisterBank> {
        self.nodes.get_mut(node_id).map(|n| &mut n.bank)
    }

    /// Sends a raw MBAP+PDU binary frame from `client_node_id` to `(dest_ip, unit_id)`.
    ///
    /// If `topology` is provided and both endpoints declare `ethernet_port`, verifies that both
    /// ports share the exact same physical Ethernet LAN net.
    pub fn send_raw_frame(
        &mut self,
        client_node_id: &str,
        dest_ip: Ipv4Addr,
        raw_request_bytes: &[u8],
        topology: Option<&BusTopology>,
    ) -> Result<Vec<u8>, ModbusError> {
        let req_frame = MbapFrame::decode(raw_request_bytes)?;
        let Some(server_id) = self
            .ip_unit_index
            .get(&(dest_ip, req_frame.unit_id))
            .cloned()
        else {
            return Err(ModbusError::UnreachableHost {
                ip: dest_ip,
                unit_id: req_frame.unit_id,
            });
        };

        if let Some(topo) = topology
            && let Some(client_eth) = self
                .nodes
                .get(client_node_id)
                .and_then(|n| n.config.ethernet_port.as_ref())
            && let Some(server_eth) = self
                .nodes
                .get(&server_id)
                .and_then(|n| n.config.ethernet_port.as_ref())
            && !topo.are_connected(&client_eth.0, &client_eth.1, &server_eth.0, &server_eth.1)
        {
            return Err(ModbusError::EthernetDisconnected {
                client: client_node_id.to_owned(),
                server: server_id,
            });
        }

        let Some(server) = self.nodes.get_mut(&server_id) else {
            return Err(ModbusError::UnreachableHost {
                ip: dest_ip,
                unit_id: req_frame.unit_id,
            });
        };
        let (resp_pdu, did_write) = server.bank.execute_pdu(&req_frame.pdu);
        if did_write {
            self.write_notifications.insert(server_id);
        }

        let resp_frame = MbapFrame {
            transaction_id: req_frame.transaction_id,
            protocol_id: 0,
            unit_id: req_frame.unit_id,
            pdu: resp_pdu,
        };
        Ok(resp_frame.encode())
    }

    /// Executes a typed [`ModbusRequest`] over full MBAP+PDU encoding/decoding.
    pub fn request(
        &mut self,
        client_node_id: &str,
        dest_ip: Ipv4Addr,
        unit_id: u8,
        req: &ModbusRequest,
        topology: Option<&BusTopology>,
    ) -> Result<ModbusResponse, ModbusError> {
        let tx_id = self.next_tx_id;
        self.next_tx_id = self.next_tx_id.wrapping_add(1).max(1);

        let frame = MbapFrame {
            transaction_id: tx_id,
            protocol_id: 0,
            unit_id,
            pdu: req.to_pdu(),
        };
        let raw_resp = self.send_raw_frame(client_node_id, dest_ip, &frame.encode(), topology)?;
        let decoded = MbapFrame::decode(&raw_resp)?;
        decode_response_pdu(req, &decoded.pdu)
    }

    /// Drains server node IDs that received remote writes since the last drain (triggers `OB50`).
    pub fn drain_write_notifications(&mut self) -> Vec<String> {
        let set = std::mem::take(&mut self.write_notifications);
        set.into_iter().collect()
    }
}

fn decode_response_pdu(req: &ModbusRequest, pdu: &[u8]) -> Result<ModbusResponse, ModbusError> {
    let Some(&fc_raw) = pdu.first() else {
        return Err(ModbusError::MalformedFrame("empty response PDU".to_owned()));
    };
    if (fc_raw & 0x80) != 0 {
        let code = pdu.get(1).copied().unwrap_or(0x04);
        let exception =
            ModbusException::from_u8(code).unwrap_or(ModbusException::ServerDeviceFailure);
        return Ok(ModbusResponse::Exception {
            function_code: fc_raw & 0x7F,
            exception,
        });
    }

    match req {
        ModbusRequest::ReadCoils { count, .. } => {
            let data = pdu.get(2..).unwrap_or(&[]);
            Ok(ModbusResponse::Coils(unpack_bits(
                data,
                usize::from(*count),
            )))
        }
        ModbusRequest::ReadDiscreteInputs { count, .. } => {
            let data = pdu.get(2..).unwrap_or(&[]);
            Ok(ModbusResponse::DiscreteInputs(unpack_bits(
                data,
                usize::from(*count),
            )))
        }
        ModbusRequest::ReadHoldingRegisters { count, .. } => {
            let data = pdu.get(2..).unwrap_or(&[]);
            Ok(ModbusResponse::HoldingRegisters(unpack_u16s(
                data,
                usize::from(*count),
            )))
        }
        ModbusRequest::ReadInputRegisters { count, .. } => {
            let data = pdu.get(2..).unwrap_or(&[]);
            Ok(ModbusResponse::InputRegisters(unpack_u16s(
                data,
                usize::from(*count),
            )))
        }
        ModbusRequest::WriteSingleCoil { .. } => {
            let Some((addr, raw)) = read_u16_pair(pdu, 1) else {
                return Err(ModbusError::MalformedFrame("truncated FC5 ack".to_owned()));
            };
            Ok(ModbusResponse::WriteSingleCoilAck {
                addr,
                value: raw == 0xFF00,
            })
        }
        ModbusRequest::WriteSingleRegister { .. } => {
            let Some((addr, value)) = read_u16_pair(pdu, 1) else {
                return Err(ModbusError::MalformedFrame("truncated FC6 ack".to_owned()));
            };
            Ok(ModbusResponse::WriteSingleRegisterAck { addr, value })
        }
        ModbusRequest::WriteMultipleCoils { .. } => {
            let Some((start, count)) = read_u16_pair(pdu, 1) else {
                return Err(ModbusError::MalformedFrame("truncated FC15 ack".to_owned()));
            };
            Ok(ModbusResponse::WriteMultipleAck {
                function: ModbusFunction::WriteMultipleCoils,
                start,
                count,
            })
        }
        ModbusRequest::WriteMultipleRegisters { .. } => {
            let Some((start, count)) = read_u16_pair(pdu, 1) else {
                return Err(ModbusError::MalformedFrame("truncated FC16 ack".to_owned()));
            };
            Ok(ModbusResponse::WriteMultipleAck {
                function: ModbusFunction::WriteMultipleRegisters,
                start,
                count,
            })
        }
    }
}

fn read_u16_pair(bytes: &[u8], offset: usize) -> Option<(u16, u16)> {
    let a = u16::from_be_bytes([*bytes.get(offset)?, *bytes.get(offset + 1)?]);
    let b = u16::from_be_bytes([*bytes.get(offset + 2)?, *bytes.get(offset + 3)?]);
    Some((a, b))
}

fn pack_bits(bits: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len().div_ceil(8)];
    for (i, &b) in bits.iter().enumerate() {
        if b && let Some(slot) = out.get_mut(i / 8) {
            *slot |= 1u8 << (i % 8);
        }
    }
    out
}

fn unpack_bits(bytes: &[u8], count: usize) -> Vec<bool> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let byte = bytes.get(i / 8).copied().unwrap_or(0);
        out.push(((byte >> (i % 8)) & 1) != 0);
    }
    out
}

fn unpack_u16s(bytes: &[u8], count: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let hi = bytes.get(i * 2).copied().unwrap_or(0);
        let lo = bytes.get(i * 2 + 1).copied().unwrap_or(0);
        out.push(u16::from_be_bytes([hi, lo]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modbus_tcp_supports_fc1_through_fc6_fc15_fc16_and_all_standard_exceptions() {
        let mut topo = BusTopology::new();
        topo.connect("uno", "ETH0", "plc1", "ETH0");
        topo.rebuild();

        let mut net = ModbusNetwork::new();
        let plc_ip = Ipv4Addr::new(192, 168, 1, 10);
        let uno_ip = Ipv4Addr::new(192, 168, 1, 20);
        let mut plc_bank = ModbusRegisterBank::new(16, 16, 16, 16);
        plc_bank.discrete_inputs[2] = true;
        plc_bank.input_registers[4] = 27_648;

        net.register_node(
            ModbusNodeConfig {
                node_id: "plc1".to_owned(),
                ip: plc_ip,
                unit_id: 1,
                ethernet_port: Some(("plc1".to_owned(), "ETH0".to_owned())),
            },
            plc_bank,
        );
        net.register_node(
            ModbusNodeConfig {
                node_id: "uno".to_owned(),
                ip: uno_ip,
                unit_id: 2,
                ethernet_port: Some(("uno".to_owned(), "ETH0".to_owned())),
            },
            ModbusRegisterBank::standard_256(),
        );

        // FC5 + FC1
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::WriteSingleCoil {
                    addr: 3,
                    value: true
                },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::WriteSingleCoilAck {
                addr: 3,
                value: true
            }
        );
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::ReadCoils { start: 0, count: 5 },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::Coils(vec![false, false, false, true, false])
        );

        // FC2 & FC4
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::ReadDiscreteInputs { start: 1, count: 3 },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::DiscreteInputs(vec![false, true, false])
        );
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::ReadInputRegisters { start: 4, count: 1 },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::InputRegisters(vec![27_648])
        );

        // FC6 + FC16 + FC3
        net.request(
            "uno",
            plc_ip,
            1,
            &ModbusRequest::WriteSingleRegister {
                addr: 0,
                value: 1234,
            },
            Some(&topo),
        )
        .unwrap();
        net.request(
            "uno",
            plc_ip,
            1,
            &ModbusRequest::WriteMultipleRegisters {
                start: 1,
                values: vec![5678, 9012],
            },
            Some(&topo),
        )
        .unwrap();
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::ReadHoldingRegisters { start: 0, count: 3 },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::HoldingRegisters(vec![1234, 5678, 9012])
        );

        // FC15
        net.request(
            "uno",
            plc_ip,
            1,
            &ModbusRequest::WriteMultipleCoils {
                start: 8,
                values: vec![true, false, true, true],
            },
            Some(&topo),
        )
        .unwrap();
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::ReadCoils { start: 8, count: 4 },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::Coils(vec![true, false, true, true])
        );

        // Write notifications recorded for OB50
        assert_eq!(net.drain_write_notifications(), vec!["plc1".to_owned()]);

        // Standard Exceptions: 0x02 Illegal Data Address & 0x03 Illegal Data Value
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::ReadHoldingRegisters {
                    start: 100,
                    count: 1
                },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::Exception {
                function_code: 0x03,
                exception: ModbusException::IllegalDataAddress
            }
        );
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::ReadHoldingRegisters { start: 0, count: 0 },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::Exception {
                function_code: 0x03,
                exception: ModbusException::IllegalDataValue
            }
        );

        // Exception 0x01 Illegal Function (raw unsupported FC 0x2B)
        let bad_fc = MbapFrame {
            transaction_id: 99,
            protocol_id: 0,
            unit_id: 1,
            pdu: vec![0x2B, 0x00, 0x00],
        };
        let raw_exc = net
            .send_raw_frame("uno", plc_ip, &bad_fc.encode(), Some(&topo))
            .unwrap();
        let decoded_exc = MbapFrame::decode(&raw_exc).unwrap();
        assert_eq!(decoded_exc.pdu, vec![0xAB, 0x01]);

        // Exception 0x04 Server Device Failure
        net.bank_mut("plc1").unwrap().faulted = true;
        assert_eq!(
            net.request(
                "uno",
                plc_ip,
                1,
                &ModbusRequest::ReadCoils { start: 0, count: 1 },
                Some(&topo)
            )
            .unwrap(),
            ModbusResponse::Exception {
                function_code: 0x01,
                exception: ModbusException::ServerDeviceFailure
            }
        );
    }
}
