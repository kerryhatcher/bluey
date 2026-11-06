//! The JSON written by `bluey log` and `bluey snapshot --json`.
//!
//! The device only knows how long it has been running, so times are turned into
//! wall-clock times using the moment the snapshot arrived.

use anyhow::Result;

use crate::gps::Reading;
use bluey_core::adv::Mac;
use bluey_core::proto::{Bssid, NodeAddr, Snapshot};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
pub struct Record {
    /// When the snapshot was received.
    pub(crate) time: Timestamp,
    /// The serial port it came from.
    pub(crate) port: String,
    /// The device's chip MAC address, which stays the same when it is moved to another port.
    pub(crate) device: String,
    pub(crate) mode: &'static str,
    pub(crate) uptime_ms: u64,
    pub(crate) ble: Vec<Ble>,
    pub(crate) wifi: Vec<Wifi>,
    pub(crate) ieee802154: Vec<Node>,
    /// Where the snapshot was taken. Absent if no GPS was in use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) gps: Option<Reading>,
}

#[derive(Serialize)]
pub(crate) struct Ble {
    pub(crate) addr: String,
    pub(crate) addr_type: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) name: Option<String>,
    pub(crate) rssi: i8,
    pub(crate) rssi_min: i8,
    pub(crate) rssi_max: i8,
    pub(crate) count: u32,
    pub(crate) first_seen: Timestamp,
    pub(crate) last_seen: Timestamp,
    /// The latest advertisement's data. Written as hex.
    #[serde(serialize_with = "as_hex")]
    pub(crate) adv: Vec<u8>,
}

#[derive(Serialize)]
pub(crate) struct Wifi {
    pub(crate) bssid: String,
    pub(crate) ssid: String,
    pub(crate) channel: u8,
    pub(crate) auth: String,
    pub(crate) rssi: i8,
    pub(crate) rssi_min: i8,
    pub(crate) rssi_max: i8,
    pub(crate) count: u32,
    pub(crate) first_seen: Timestamp,
    pub(crate) last_seen: Timestamp,
}

#[derive(Serialize)]
pub(crate) struct Node {
    pub(crate) pan_id: Option<String>,
    pub(crate) addr: String,
    pub(crate) addr_type: &'static str,
    pub(crate) protocol: &'static str,
    pub(crate) network_name: Option<String>,
    pub(crate) channel: u8,
    pub(crate) rssi: i8,
    pub(crate) rssi_min: i8,
    pub(crate) rssi_max: i8,
    pub(crate) count: u32,
    pub(crate) first_seen: Timestamp,
    pub(crate) last_seen: Timestamp,
}

impl Record {
    pub fn new(port: &str, snapshot: &Snapshot, gps: Option<Reading>) -> Result<Self> {
        let now = Timestamp::now();
        // Converts a device uptime into wall-clock time.
        let at = |uptime_ms: u64| -> Result<Timestamp> {
            let ago = snapshot.uptime_ms.saturating_sub(uptime_ms);
            Ok(Timestamp::from_millisecond(now.as_millisecond() - ago as i64)?)
        };

        let ble = snapshot
            .ble
            .iter()
            .map(|d| {
                Ok(Ble {
                    addr: Mac(&d.addr).to_string(),
                    addr_type: d.class.label(),
                    kind: d.kind.label(),
                    name: d.name.clone(),
                    rssi: d.rssi,
                    rssi_min: d.rssi_min,
                    rssi_max: d.rssi_max,
                    count: d.count,
                    first_seen: at(d.first_seen_ms)?,
                    last_seen: at(d.last_seen_ms)?,
                    adv: d.adv.clone(),
                })
            })
            .collect::<Result<_>>()?;
        let wifi = snapshot
            .wifi
            .iter()
            .map(|ap| {
                Ok(Wifi {
                    bssid: Bssid(&ap.bssid).to_string(),
                    ssid: ap.ssid.clone(),
                    channel: ap.channel,
                    auth: ap.auth.clone(),
                    rssi: ap.rssi,
                    rssi_min: ap.rssi_min,
                    rssi_max: ap.rssi_max,
                    count: ap.count,
                    first_seen: at(ap.first_seen_ms)?,
                    last_seen: at(ap.last_seen_ms)?,
                })
            })
            .collect::<Result<_>>()?;

        let ieee802154 = snapshot
            .ieee802154
            .iter()
            .map(|n| {
                Ok(Node {
                    pan_id: n.pan_id.map(|p| format!("{p:04X}")),
                    addr: n.addr.to_string(),
                    addr_type: match n.addr {
                        NodeAddr::Short(_) => "short",
                        NodeAddr::Extended(_) => "extended",
                    },
                    protocol: n.protocol.label(),
                    network_name: n.network_name.clone(),
                    channel: n.channel,
                    rssi: n.rssi,
                    rssi_min: n.rssi_min,
                    rssi_max: n.rssi_max,
                    count: n.count,
                    first_seen: at(n.first_seen_ms)?,
                    last_seen: at(n.last_seen_ms)?,
                })
            })
            .collect::<Result<_>>()?;

        Ok(Self {
            time: now,
            port: port.to_string(),
            device: Bssid(&snapshot.device).to_string(),
            mode: snapshot.mode.label(),
            uptime_ms: snapshot.uptime_ms,
            ble,
            wifi,
            ieee802154,
            gps,
        })
    }
}

/// Bytes as lowercase hex, two digits each.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn as_hex<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&hex(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_lowercase_pairs() {
        assert_eq!(hex(&[0x02, 0x01, 0xff, 0x4c]), "0201ff4c");
        assert_eq!(hex(&[]), "");
    }
}
