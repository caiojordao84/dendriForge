//! Physical-topology UART byte-stream bus (`02-simulation-and-electrical-engine.md` §4.1).
//!
//! Enforces:
//! - Strict physical `TX -> RX` wiring through [`BusTopology`] (unwired or `TX -> TX` links deliver nothing).
//! - Baud rate tolerance (`<= 3%`) and framing parity/data/stop-bit compatibility.
//! - Exact microsecond bit-time propagation scheduling (`frame_us = ceil(bits * 1_000_000 / baud)`).
//! - Bounded per-endpoint RX/TX FIFOs with explicit overflow counters and `OB_COMM_EVENT` notifications.

use crate::topology::BusTopology;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use thiserror::Error;

/// UART parity configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UartParity {
    #[default]
    None,
    Even,
    Odd,
}

/// UART stop-bit configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UartStopBits {
    #[default]
    One,
    Two,
}

/// Physical UART endpoint attached to a board or peripheral (`(instance, tx_port, rx_port)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UartEndpointConfig {
    /// Logical channel/node key (e.g., `"uno:UART0"` or `"uno"`).
    pub node_id: String,
    /// Board or component instance ID in [`BusTopology`].
    pub instance_id: String,
    /// Physical transmit pin ID (e.g., `"D1"`).
    pub tx_port: String,
    /// Physical receive pin ID (e.g., `"D0"`).
    pub rx_port: String,
    /// Symbol rate in bits/second (e.g., `9600`, `115_200`).
    pub baud_rate: u32,
    /// Data payload width (`5..=8`, default `8`).
    pub data_bits: u8,
    pub parity: UartParity,
    pub stop_bits: UartStopBits,
    /// Bounded RX FIFO capacity in bytes (default `256`).
    pub rx_fifo_cap: usize,
    /// Bounded TX FIFO capacity in bytes (default `256`).
    pub tx_fifo_cap: usize,
}

impl UartEndpointConfig {
    /// Standard `8N1` UART endpoint configuration.
    #[must_use]
    pub fn standard_8n1(
        node_id: impl Into<String>,
        instance_id: impl Into<String>,
        tx_port: impl Into<String>,
        rx_port: impl Into<String>,
        baud_rate: u32,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            instance_id: instance_id.into(),
            tx_port: tx_port.into(),
            rx_port: rx_port.into(),
            baud_rate,
            data_bits: 8,
            parity: UartParity::None,
            stop_bits: UartStopBits::One,
            rx_fifo_cap: 256,
            tx_fifo_cap: 256,
        }
    }

    /// Total wire bits per UART character frame (`start + data + parity + stop`).
    #[must_use]
    pub const fn bits_per_frame(&self) -> u64 {
        let parity_bits = match self.parity {
            UartParity::None => 0,
            UartParity::Even | UartParity::Odd => 1,
        };
        let stop_bits = match self.stop_bits {
            UartStopBits::One => 1,
            UartStopBits::Two => 2,
        };
        1 + (self.data_bits as u64) + parity_bits + stop_bits
    }

    /// Exact duration of one UART character frame in microseconds (`ceil`).
    #[must_use]
    pub fn frame_duration_us(&self) -> u64 {
        let baud = u64::from(self.baud_rate.max(1));
        let num = self.bits_per_frame().saturating_mul(1_000_000);
        num.div_ceil(baud).max(1)
    }

    /// Returns `true` iff two endpoints agree on framing and baud rate within 3% tolerance.
    #[must_use]
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        if self.data_bits != other.data_bits
            || self.parity != other.parity
            || self.stop_bits != other.stop_bits
            || self.baud_rate == 0
            || other.baud_rate == 0
        {
            return false;
        }
        let hi = u64::from(self.baud_rate.max(other.baud_rate));
        let lo = u64::from(self.baud_rate.min(other.baud_rate));
        // |a - b| / max(a, b) <= 0.03  <=>  (hi - lo) * 100 <= hi * 3
        hi.saturating_sub(lo).saturating_mul(100) <= hi.saturating_mul(3)
    }
}

/// Telemetry & error counters for one UART endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UartNodeStats {
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub rx_overflow_drops: u64,
    pub framing_errors: u64,
}

/// Errors emitted by [`UartBus`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UartError {
    #[error("unknown UART endpoint `{0}`")]
    UnknownEndpoint(String),
    #[error("invalid UART baud rate 0 on endpoint `{0}`")]
    InvalidBaudRate(String),
    #[error("invalid UART data bits {bits} on endpoint `{node_id}` (expected 5..=8)")]
    InvalidDataBits { node_id: String, bits: u8 },
    #[error("TX FIFO full on UART endpoint `{node_id}` (capacity {cap})")]
    TxFifoFull { node_id: String, cap: usize },
}

#[derive(Debug, Clone)]
struct InFlightByte {
    arrive_us: u64,
    dest_node: String,
    byte: u8,
    framing_error: bool,
}

#[derive(Debug, Clone)]
struct UartNodeState {
    config: UartEndpointConfig,
    rx_fifo: VecDeque<u8>,
    tx_busy_until_us: u64,
    stats: UartNodeStats,
}

/// Multi-endpoint physical-topology UART bus fabric.
#[derive(Debug, Clone, Default)]
pub struct UartBus {
    nodes: BTreeMap<String, UartNodeState>,
    in_flight: Vec<InFlightByte>,
    /// Nodes that received >= 1 valid byte since last drain (used to trigger `OB_COMM_EVENT`).
    rx_notified: BTreeSet<String>,
    now_us: u64,
}

impl UartBus {
    /// Creates an empty [`UartBus`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a UART endpoint.
    pub fn register_endpoint(&mut self, config: UartEndpointConfig) -> Result<(), UartError> {
        if config.baud_rate == 0 {
            return Err(UartError::InvalidBaudRate(config.node_id));
        }
        if !(5..=8).contains(&config.data_bits) {
            return Err(UartError::InvalidDataBits {
                node_id: config.node_id,
                bits: config.data_bits,
            });
        }
        let id = config.node_id.clone();
        self.nodes.insert(
            id,
            UartNodeState {
                config,
                rx_fifo: VecDeque::new(),
                tx_busy_until_us: 0,
                stats: UartNodeStats::default(),
            },
        );
        Ok(())
    }

    /// Queues `bytes` for transmission on `sender_node`'s physical `TX` pin over `topology`.
    ///
    /// Every registered endpoint whose `RX` pin shares the exact physical wire net with
    /// `sender_node.tx_port` receives each byte after its exact bit-time propagation delay.
    /// Incompatible baud/framing links increment the receiver's `framing_errors` counter
    /// rather than silently delivering clean payload bytes.
    pub fn write_bytes(
        &mut self,
        sender_node: &str,
        bytes: &[u8],
        topology: &BusTopology,
        now_us: u64,
    ) -> Result<usize, UartError> {
        self.now_us = self.now_us.max(now_us);
        let sender_cfg = self
            .nodes
            .get(sender_node)
            .ok_or_else(|| UartError::UnknownEndpoint(sender_node.to_owned()))?
            .config
            .clone();

        let frame_us = sender_cfg.frame_duration_us();
        let current_busy = self
            .nodes
            .get(sender_node)
            .map_or(now_us, |s| s.tx_busy_until_us);
        let queued_frames = current_busy.saturating_sub(now_us).div_ceil(frame_us);
        if usize::try_from(queued_frames)
            .unwrap_or(usize::MAX)
            .saturating_add(bytes.len())
            > sender_cfg.tx_fifo_cap
        {
            return Err(UartError::TxFifoFull {
                node_id: sender_node.to_owned(),
                cap: sender_cfg.tx_fifo_cap,
            });
        }

        // Discover all physically wired RX endpoints on sender's TX net (excluding self-loop unless wired).
        let mut receivers: Vec<(String, bool)> = Vec::new();
        if let Some(tx_net) = topology.net_of(&sender_cfg.instance_id, &sender_cfg.tx_port) {
            for (dst_id, dst_state) in &self.nodes {
                let rx_net =
                    topology.net_of(&dst_state.config.instance_id, &dst_state.config.rx_port);
                if rx_net == Some(tx_net) {
                    let framing_err = !sender_cfg.is_compatible_with(&dst_state.config);
                    receivers.push((dst_id.clone(), framing_err));
                }
            }
        }

        let mut cursor_us = current_busy.max(now_us);
        for &b in bytes {
            cursor_us = cursor_us.saturating_add(frame_us);
            for (dst_id, framing_err) in &receivers {
                self.in_flight.push(InFlightByte {
                    arrive_us: cursor_us,
                    dest_node: dst_id.clone(),
                    byte: b,
                    framing_error: *framing_err,
                });
            }
        }

        if let Some(state) = self.nodes.get_mut(sender_node) {
            state.tx_busy_until_us = cursor_us;
            state.stats.tx_bytes = state
                .stats
                .tx_bytes
                .saturating_add(u64::try_from(bytes.len()).unwrap_or(0));
        }

        Ok(bytes.len())
    }

    /// Convenience UTF-8 string transmit over [`Self::write_bytes`].
    pub fn write_str(
        &mut self,
        sender_node: &str,
        text: &str,
        topology: &BusTopology,
        now_us: u64,
    ) -> Result<usize, UartError> {
        self.write_bytes(sender_node, text.as_bytes(), topology, now_us)
    }

    /// Delivers all in-flight UART frames whose arrival time is `<= time_us`.
    pub fn step_to(&mut self, time_us: u64) {
        self.now_us = self.now_us.max(time_us);
        // Partition deterministically preserving arrival order.
        self.in_flight.sort_by_key(|f| f.arrive_us);
        let mut remaining = Vec::new();
        let batch = std::mem::take(&mut self.in_flight);
        for frame in batch {
            if frame.arrive_us <= time_us {
                if let Some(dst) = self.nodes.get_mut(&frame.dest_node) {
                    if frame.framing_error {
                        dst.stats.framing_errors = dst.stats.framing_errors.saturating_add(1);
                    } else if dst.rx_fifo.len() >= dst.config.rx_fifo_cap {
                        dst.stats.rx_overflow_drops = dst.stats.rx_overflow_drops.saturating_add(1);
                    } else {
                        dst.rx_fifo.push_back(frame.byte);
                        dst.stats.rx_bytes = dst.stats.rx_bytes.saturating_add(1);
                        self.rx_notified.insert(frame.dest_node);
                    }
                }
            } else {
                remaining.push(frame);
            }
        }
        self.in_flight = remaining;
    }

    /// Returns the earliest timestamp (µs) at which the next in-flight byte arrives, if any.
    #[must_use]
    pub fn next_arrival_us(&self) -> Option<u64> {
        self.in_flight.iter().map(|f| f.arrive_us).min()
    }

    /// Returns how many bytes are waiting in `node_id`'s RX FIFO.
    #[must_use]
    pub fn available(&self, node_id: &str) -> usize {
        self.nodes.get(node_id).map_or(0, |n| n.rx_fifo.len())
    }

    /// Pops one byte from `node_id`'s RX FIFO (non-blocking).
    pub fn read_byte(&mut self, node_id: &str) -> Option<u8> {
        self.nodes.get_mut(node_id)?.rx_fifo.pop_front()
    }

    /// Drains all currently available bytes from `node_id`'s RX FIFO.
    pub fn read_available(&mut self, node_id: &str) -> Vec<u8> {
        let Some(node) = self.nodes.get_mut(node_id) else {
            return Vec::new();
        };
        node.rx_fifo.drain(..).collect()
    }

    /// Reads one `\n`-terminated UTF-8 line if complete (returns `None` if incomplete).
    pub fn read_line(&mut self, node_id: &str) -> Option<String> {
        let node = self.nodes.get_mut(node_id)?;
        let pos = node.rx_fifo.iter().position(|&b| b == b'\n')?;
        let mut raw = Vec::with_capacity(pos);
        for _ in 0..=pos {
            if let Some(b) = node.rx_fifo.pop_front()
                && b != b'\n'
                && b != b'\r'
            {
                raw.push(b);
            }
        }
        Some(String::from_utf8_lossy(&raw).into_owned())
    }

    /// Returns `true` iff `node_id` has finished shifting out all queued TX bytes at `now_us`.
    #[must_use]
    pub fn is_tx_flushed(&self, node_id: &str, now_us: u64) -> bool {
        self.nodes
            .get(node_id)
            .is_some_and(|n| n.tx_busy_until_us <= now_us)
    }

    /// Returns diagnostic counters for `node_id`.
    #[must_use]
    pub fn stats(&self, node_id: &str) -> Option<&UartNodeStats> {
        self.nodes.get(node_id).map(|n| &n.stats)
    }

    /// Drains node IDs that received clean bytes since the previous check (for `OB_COMM_EVENT`).
    pub fn drain_rx_notifications(&mut self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.rx_notified.len());
        let set = std::mem::take(&mut self.rx_notified);
        for id in set {
            out.push(id);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_delivers_only_across_wired_tx_rx_with_exact_baud_timing_and_rejects_mismatch() {
        let mut topo = BusTopology::new();
        topo.connect("uno", "D1", "plc1", "RX0");
        topo.connect("plc1", "TX0", "uno", "D0");
        topo.rebuild();

        let mut bus = UartBus::new();
        bus.register_endpoint(UartEndpointConfig::standard_8n1(
            "uno", "uno", "D1", "D0", 115_200,
        ))
        .unwrap();
        bus.register_endpoint(UartEndpointConfig::standard_8n1(
            "plc1", "plc1", "TX0", "RX0", 115_200,
        ))
        .unwrap();
        // Unwired bystander at matching baud
        bus.register_endpoint(UartEndpointConfig::standard_8n1(
            "unwired", "ghost", "TX", "RX", 115_200,
        ))
        .unwrap();

        // 1 byte @ 115200 8N1 = ceil(10 * 1_000_000 / 115_200) = 87 us; 6 bytes = 522 us
        bus.write_str("uno", "HELLO\n", &topo, 1_000).unwrap();
        bus.step_to(1_050);
        assert_eq!(bus.available("plc1"), 0);

        bus.step_to(1_600);
        assert_eq!(bus.read_line("plc1").as_deref(), Some("HELLO"));
        assert_eq!(bus.available("unwired"), 0);
        assert_eq!(bus.drain_rx_notifications(), vec!["plc1".to_owned()]);

        // Baud mismatch must record framing_errors instead of clean data
        let mut bad_bus = UartBus::new();
        bad_bus
            .register_endpoint(UartEndpointConfig::standard_8n1(
                "uno", "uno", "D1", "D0", 9_600,
            ))
            .unwrap();
        bad_bus
            .register_endpoint(UartEndpointConfig::standard_8n1(
                "plc1", "plc1", "TX0", "RX0", 115_200,
            ))
            .unwrap();
        bad_bus.write_str("uno", "AB", &topo, 0).unwrap();
        bad_bus.step_to(5_000);
        assert_eq!(bad_bus.available("plc1"), 0);
        assert_eq!(bad_bus.stats("plc1").unwrap().framing_errors, 2);
    }
}
