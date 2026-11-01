//! Table of the WiFi access points heard so far.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bluey_core::proto::AccessPoint as WireAccessPoint;
use embassy_time::{Duration, Instant};

/// Most access points tracked at once. When full, the one heard longest ago is dropped.
const MAX_ACCESS_POINTS: usize = 64;

struct AccessPoint {
    bssid: [u8; 6],
    /// Empty for hidden networks.
    ssid: String,
    channel: u8,
    auth: String,
    rssi: i8,
    rssi_min: i8,
    rssi_max: i8,
    count: u32,
    first_seen: Instant,
    last_seen: Instant,
}

#[derive(Default)]
pub struct AccessPointTable {
    access_points: Vec<AccessPoint>,
}

impl AccessPointTable {
    pub const fn new() -> Self {
        Self { access_points: Vec::new() }
    }

    /// Records one sighting of an access point from a scan.
    pub fn record(&mut self, bssid: [u8; 6], ssid: &str, channel: u8, auth: &str, rssi: i8, now: Instant) {
        let index = match self.access_points.iter().position(|ap| ap.bssid == bssid) {
            Some(index) => index,
            None => {
                if self.access_points.len() >= MAX_ACCESS_POINTS {
                    let oldest = (0..self.access_points.len()).min_by_key(|&i| self.access_points[i].last_seen);
                    if let Some(oldest) = oldest {
                        self.access_points.swap_remove(oldest);
                    }
                }
                self.access_points.push(AccessPoint {
                    bssid,
                    ssid: String::new(),
                    channel,
                    auth: String::new(),
                    rssi,
                    rssi_min: rssi,
                    rssi_max: rssi,
                    count: 0,
                    first_seen: now,
                    last_seen: now,
                });
                self.access_points.len() - 1
            }
        };

        let ap = &mut self.access_points[index];
        ap.rssi = rssi;
        ap.rssi_min = ap.rssi_min.min(rssi);
        ap.rssi_max = ap.rssi_max.max(rssi);
        ap.count += 1;
        ap.last_seen = now;
        ap.channel = channel;
        // Hidden networks report an empty SSID, so don't let that overwrite a name we learned.
        if !ssid.is_empty() && ap.ssid != ssid {
            ap.ssid = ssid.to_string();
        }
        if ap.auth != auth {
            ap.auth = auth.to_string();
        }
    }

    /// Forgets access points not heard from for `max_age`.
    pub fn prune(&mut self, now: Instant, max_age: Duration) {
        self.access_points.retain(|ap| now - ap.last_seen < max_age);
    }

    /// The table as it goes on the wire, ordered by BSSID.
    pub fn snapshot(&self) -> Vec<WireAccessPoint> {
        let mut access_points: Vec<WireAccessPoint> = self
            .access_points
            .iter()
            .map(|ap| WireAccessPoint {
                bssid: ap.bssid,
                ssid: ap.ssid.clone(),
                channel: ap.channel,
                auth: ap.auth.clone(),
                rssi: ap.rssi,
                rssi_min: ap.rssi_min,
                rssi_max: ap.rssi_max,
                count: ap.count,
                first_seen_ms: ap.first_seen.as_millis(),
                last_seen_ms: ap.last_seen.as_millis(),
            })
            .collect();
        access_points.sort_unstable_by_key(|ap| ap.bssid);
        access_points
    }
}
