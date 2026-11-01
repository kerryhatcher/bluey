//! The JSON written by `bluey log` and `bluey snapshot --json`.
//!
//! The device only knows how long it has been running, so times are turned into
//! wall-clock times using the moment the snapshot arrived.

use anyhow::Result;
use bluey_core::adv::Mac;
use bluey_core::proto::{Bssid, Snapshot};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
pub struct Record {
    /// When the snapshot was received.
    time: Timestamp,
    uptime_ms: u64,
    ble: Vec<Ble>,
    wifi: Vec<Wifi>,
}

#[derive(Serialize)]
struct Ble {
    addr: String,
    addr_type: &'static str,
    kind: &'static str,
    name: Option<String>,
    rssi: i8,
    rssi_min: i8,
    rssi_max: i8,
    count: u32,
    first_seen: Timestamp,
    last_seen: Timestamp,
}

#[derive(Serialize)]
struct Wifi {
    bssid: String,
    ssid: String,
    channel: u8,
    auth: String,
    rssi: i8,
    rssi_min: i8,
    rssi_max: i8,
    count: u32,
    first_seen: Timestamp,
    last_seen: Timestamp,
}

impl Record {
    pub fn new(snapshot: &Snapshot) -> Result<Self> {
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

        Ok(Self { time: now, uptime_ms: snapshot.uptime_ms, ble, wifi })
    }
}
