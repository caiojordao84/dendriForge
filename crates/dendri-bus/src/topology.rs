//! Deterministic physical wiring topology (`Union-Find` over `[[wires]]`, ADR-011).
//!
//! Bus fabrics (`UartBus`, `I2cBus`, `SpiBus`, `ModbusNetwork`) query [`BusTopology`] so
//! frames only flow across genuinely wired physical nets.

use dendri_ast::{Endpoint, Project};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Canonical `(instance_id, port_id)` electrical/communication endpoint key.
pub type EndpointKey = (String, String);

/// Canonical deterministic electrical net identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NetId(pub usize);

/// Errors emitted while extracting wiring topology from a [`Project`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TopologyError {
    #[error("malformed wire endpoint `{endpoint}` on wire `{wire_id}`: {reason}")]
    MalformedEndpoint {
        wire_id: String,
        endpoint: String,
        reason: String,
    },
}

/// Physical netlist extracted from canvas `[[wires]]` (or assembled programmatically).
#[derive(Debug, Clone, Default)]
pub struct BusTopology {
    endpoint_ids: BTreeMap<EndpointKey, usize>,
    edges: Vec<(usize, usize)>,
    endpoint_to_net: BTreeMap<EndpointKey, NetId>,
    net_members: BTreeMap<NetId, BTreeSet<EndpointKey>>,
    dirty: bool,
}

impl BusTopology {
    /// Creates an empty wiring topology.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a deterministic [`BusTopology`] directly from all `[[wires]]` of a [`Project`].
    pub fn from_project(project: &Project) -> Result<Self, TopologyError> {
        let mut topo = Self::new();
        for wire in &project.wires {
            let from_ep =
                Endpoint::parse(&wire.from).map_err(|e| TopologyError::MalformedEndpoint {
                    wire_id: wire.id.clone(),
                    endpoint: wire.from.clone(),
                    reason: e.to_string(),
                })?;
            let to_ep =
                Endpoint::parse(&wire.to).map_err(|e| TopologyError::MalformedEndpoint {
                    wire_id: wire.id.clone(),
                    endpoint: wire.to.clone(),
                    reason: e.to_string(),
                })?;
            topo.connect(
                from_ep.instance_id,
                from_ep.port_id,
                to_ep.instance_id,
                to_ep.port_id,
            );
        }
        topo.rebuild();
        Ok(topo)
    }

    /// Connects physical terminal `(inst_a, port_a)` to `(inst_b, port_b)`.
    pub fn connect(
        &mut self,
        inst_a: impl Into<String>,
        port_a: impl Into<String>,
        inst_b: impl Into<String>,
        port_b: impl Into<String>,
    ) {
        let ka = (inst_a.into(), port_a.into());
        let kb = (inst_b.into(), port_b.into());
        let next_a = self.endpoint_ids.len();
        let ia = *self.endpoint_ids.entry(ka).or_insert(next_a);
        let next_b = self.endpoint_ids.len();
        let ib = *self.endpoint_ids.entry(kb).or_insert(next_b);
        self.edges.push((ia, ib));
        self.dirty = true;
    }

    /// Recomputes canonical Union-Find net IDs deterministically.
    pub fn rebuild(&mut self) {
        if !self.dirty {
            return;
        }
        let mut uf = UnionFind::new(self.endpoint_ids.len());
        for &(a, b) in &self.edges {
            uf.union(a, b);
        }
        self.endpoint_to_net.clear();
        self.net_members.clear();
        for (key, &idx) in &self.endpoint_ids {
            let net = NetId(uf.find(idx));
            self.endpoint_to_net.insert(key.clone(), net);
            self.net_members.entry(net).or_default().insert(key.clone());
        }
        self.dirty = false;
    }

    /// Ensures nets are finalized before read-only queries.
    fn ensure_built(&mut self) {
        if self.dirty {
            self.rebuild();
        }
    }

    /// Returns the canonical [`NetId`] for `(inst, port)` if connected to at least one wire.
    #[must_use]
    pub fn net_of(&self, inst: &str, port: &str) -> Option<NetId> {
        self.endpoint_to_net
            .get(&(inst.to_owned(), port.to_owned()))
            .copied()
    }

    /// Rebuilds if dirty and returns the canonical [`NetId`] for `(inst, port)`.
    pub fn net_of_mut(&mut self, inst: &str, port: &str) -> Option<NetId> {
        self.ensure_built();
        self.net_of(inst, port)
    }

    /// Returns `true` iff `(inst_a, port_a)` and `(inst_b, port_b)` share the exact same wired net.
    #[must_use]
    pub fn are_connected(&self, inst_a: &str, port_a: &str, inst_b: &str, port_b: &str) -> bool {
        match (self.net_of(inst_a, port_a), self.net_of(inst_b, port_b)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// Returns all endpoints attached to `net` in deterministic lexicographic order.
    #[must_use]
    pub fn members(&self, net: NetId) -> Option<&BTreeSet<EndpointKey>> {
        self.net_members.get(&net)
    }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        let mut root = x;
        while let Some(&p) = self.parent.get(root) {
            if p == root {
                break;
            }
            root = p;
        }
        while let Some(&p) = self.parent.get(x) {
            if p == root {
                break;
            }
            if let Some(slot) = self.parent.get_mut(x) {
                *slot = root;
            }
            x = p;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            if let Some(slot) = self.parent.get_mut(hi) {
                *slot = lo;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_find_topology_resolves_transitive_nets_deterministically() {
        let mut topo = BusTopology::new();
        topo.connect("uno", "D1", "junction", "1");
        topo.connect("junction", "1", "plc1", "RX0");
        topo.rebuild();

        assert!(topo.are_connected("uno", "D1", "plc1", "RX0"));
        assert!(!topo.are_connected("uno", "D0", "plc1", "RX0"));
    }
}
