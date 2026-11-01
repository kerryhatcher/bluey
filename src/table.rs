//! Table of the devices heard so far, so the log can show one line per device.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use embassy_time::{Duration, Instant};

use crate::adv::{AddrClass, Kind, Summary};

/// Most devices tracked at once. When full, the one heard longest ago is dropped.
const MAX_DEVICES: usize = 64;

#[derive(Clone)]
pub struct Device {
    pub addr: [u8; 6],
    pub class: AddrClass,
    pub kind: Kind,
    pub name: Option<String>,
    pub rssi: i8,
    pub rssi_min: i8,
    pub rssi_max: i8,
    pub count: u32,
    pub first_seen: Instant,
    pub last_seen: Instant,
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

    /// Forgets devices not heard from for `max_age`. Rotating addresses would otherwise pile up.
    pub fn prune(&mut self, now: Instant, max_age: Duration) {
        self.devices.retain(|d| now - d.last_seen < max_age);
    }

    /// A copy of the table, strongest signal first, so it can be printed outside a lock.
    pub fn snapshot(&self) -> Vec<Device> {
        let mut devices = self.devices.clone();
        devices.sort_unstable_by_key(|d| core::cmp::Reverse(d.rssi));
        devices
    }
}
