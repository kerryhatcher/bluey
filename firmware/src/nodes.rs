//! Table of the 802.15.4 devices (Zigbee and Thread) heard so far.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use bluey_core::proto::{Ieee802154Node, NodeAddr, Protocol};
use embassy_time::{Duration, Instant};

/// Most nodes tracked at once. When full, the one heard longest ago is dropped.
const MAX_NODES: usize = 64;

struct Node {
    pan_id: Option<u16>,
    addr: NodeAddr,
    protocol: Protocol,
    network_name: Option<String>,
    channel: u8,
    rssi: i8,
    rssi_min: i8,
    rssi_max: i8,
    count: u32,
    first_seen: Instant,
    last_seen: Instant,
}

#[derive(Default)]
pub struct NodeTable {
    nodes: Vec<Node>,
}

impl NodeTable {
    pub const fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Records one frame from a node.
    #[allow(clippy::too_many_arguments, reason = "these are the fields of one frame")]
    pub fn record(
        &mut self,
        pan_id: Option<u16>,
        addr: NodeAddr,
        protocol: Protocol,
        network_name: Option<&str>,
        channel: u8,
        rssi: i8,
        now: Instant,
    ) {
        // The same short address can be in use on two networks, so the PAN is part of the key.
        let index = match self.nodes.iter().position(|n| n.pan_id == pan_id && n.addr == addr) {
            Some(index) => index,
            None => {
                if self.nodes.len() >= MAX_NODES {
                    let oldest = (0..self.nodes.len()).min_by_key(|&i| self.nodes[i].last_seen);
                    if let Some(oldest) = oldest {
                        self.nodes.swap_remove(oldest);
                    }
                }
                self.nodes.push(Node {
                    pan_id,
                    addr,
                    protocol: Protocol::Unknown,
                    network_name: None,
                    channel,
                    rssi,
                    rssi_min: rssi,
                    rssi_max: rssi,
                    count: 0,
                    first_seen: now,
                    last_seen: now,
                });
                self.nodes.len() - 1
            }
        };

        let node = &mut self.nodes[index];
        node.channel = channel;
        node.rssi = rssi;
        node.rssi_min = node.rssi_min.min(rssi);
        node.rssi_max = node.rssi_max.max(rssi);
        node.count += 1;
        node.last_seen = now;
        // Most frames say nothing about the protocol, so don't let them erase what a beacon did.
        if protocol != Protocol::Unknown {
            node.protocol = protocol;
        }
        if let Some(name) = network_name
            && node.network_name.as_deref() != Some(name)
        {
            node.network_name = Some(name.to_string());
        }
    }

    /// Forgets everything, e.g. when the radio is switched to something else.
    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    /// Forgets nodes not heard from for `max_age`.
    pub fn prune(&mut self, now: Instant, max_age: Duration) {
        self.nodes.retain(|n| now - n.last_seen < max_age);
    }

    /// The table as it goes on the wire, ordered by PAN and address.
    pub fn snapshot(&self) -> Vec<Ieee802154Node> {
        let mut nodes: Vec<Ieee802154Node> = self
            .nodes
            .iter()
            .map(|n| Ieee802154Node {
                pan_id: n.pan_id,
                addr: n.addr,
                protocol: n.protocol,
                network_name: n.network_name.clone(),
                channel: n.channel,
                rssi: n.rssi,
                rssi_min: n.rssi_min,
                rssi_max: n.rssi_max,
                count: n.count,
                first_seen_ms: n.first_seen.as_millis(),
                last_seen_ms: n.last_seen.as_millis(),
            })
            .collect();
        nodes.sort_unstable_by_key(|n| (n.pan_id, n.addr));
        nodes
    }
}
