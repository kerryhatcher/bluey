//! Table of the devices heard so far, so the log can show one line per device.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use embassy_time::{Duration, Instant};

use bluey_core::adv::{AddrClass, Kind, Summary};
use bluey_core::proto::BleDevice;

/// Most devices tracked at once. When full, the one heard longest ago is dropped.
const MAX_DEVICES: usize = 64;

struct Device {
    addr: [u8; 6],
    class: AddrClass,
    kind: Kind,
    name: Option<String>,
    rssi: i8,
    rssi_min: i8,
    rssi_max: i8,
    count: u32,
    first_seen: Instant,
    last_seen: Instant,
}

#[derive(Default)]
pub struct DeviceTable {
    devices: Vec<Device>,
}

impl DeviceTable {
    pub const fn new() -> Self {
        Self { devices: Vec::new() }
    }

    /// Records one advertisement or scan response.
    ///
    /// Scan responses carry no manufacturer data, so they only ever fill in details
    /// (name, kind) that the device's advertisements haven't provided yet.
    pub fn record(&mut self, addr: [u8; 6], class: AddrClass, rssi: i8, summary: &Summary<'_>, now: Instant) {
        let index = match self.devices.iter().position(|d| d.addr == addr) {
            Some(index) => index,
            None => {
                if self.devices.len() >= MAX_DEVICES {
                    let oldest = (0..self.devices.len()).min_by_key(|&i| self.devices[i].last_seen);
                    if let Some(oldest) = oldest {
                        self.devices.swap_remove(oldest);
                    }
                }
                self.devices.push(Device {
                    addr,
                    class,
                    kind: Kind::Unknown,
                    name: None,
                    rssi,
                    rssi_min: rssi,
                    rssi_max: rssi,
                    count: 0,
                    first_seen: now,
                    last_seen: now,
                });
                self.devices.len() - 1
            }
        };

        let device = &mut self.devices[index];
        device.rssi = rssi;
        device.rssi_min = device.rssi_min.min(rssi);
        device.rssi_max = device.rssi_max.max(rssi);
        device.count += 1;
        device.last_seen = now;
        if matches!(device.kind, Kind::Unknown | Kind::OtherManufacturer(_)) && summary.kind != Kind::Unknown {
            device.kind = summary.kind;
        }
        if let Some(name) = summary.name
            && device.name.as_deref() != Some(name)
        {
            device.name = Some(name.to_string());
        }
    }

    /// Forgets everything, e.g. when the radio is switched to something else.
    pub fn clear(&mut self) {
        self.devices.clear();
    }

    /// Forgets devices not heard from for `max_age`. Rotating addresses would otherwise pile up.
    pub fn prune(&mut self, now: Instant, max_age: Duration) {
        self.devices.retain(|d| now - d.last_seen < max_age);
    }

    /// The table as it goes on the wire, ordered by address.
    pub fn snapshot(&self) -> Vec<BleDevice> {
        let mut devices: Vec<BleDevice> = self
            .devices
            .iter()
            .map(|d| BleDevice {
                addr: d.addr,
                class: d.class,
                kind: d.kind,
                name: d.name.clone(),
                rssi: d.rssi,
                rssi_min: d.rssi_min,
                rssi_max: d.rssi_max,
                count: d.count,
                first_seen_ms: d.first_seen.as_millis(),
                last_seen_ms: d.last_seen.as_millis(),
            })
            .collect();
        // Addresses are stored least significant byte first, so compare them reversed.
        devices.sort_unstable_by_key(|d| core::array::from_fn::<u8, 6, _>(|i| d.addr[5 - i]));
        devices
    }
}
