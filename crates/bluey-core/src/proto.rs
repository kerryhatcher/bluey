//! Messages sent by the device.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::adv::{AddrClass, Kind};

/// Bumped whenever the encoding of [`Message`] changes incompatibly.
pub const PROTOCOL_VERSION: u16 = 2;

/// What the device is scanning for. It has one radio, so it can only do one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Bluetooth LE devices.
    Ble,
    /// WiFi access points.
    Wifi,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ble => "ble",
            Self::Wifi => "wifi",
        }
    }
}

/// What the host can ask of the device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Switch what the device scans for. It answers with a snapshot once it has switched.
    SetMode(Mode),
    /// Send a snapshot now instead of waiting for the next one.
    Snapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// Everything the device has heard recently. Sent periodically.
    Snapshot(Snapshot),
    /// A line from the device's log.
    Log(LogLine),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub protocol: u16,
    /// Identifies the device: the chip's base MAC address. Use [`Bssid`] to print it.
    pub device: [u8; 6],
    pub mode: Mode,
    /// Time since the device booted. All `*_ms` fields in the snapshot use this clock.
    pub uptime_ms: u64,
    /// Bluetooth LE devices, ordered by address. Empty unless the device is in BLE mode.
    pub ble: Vec<BleDevice>,
    /// WiFi access points, ordered by BSSID. Empty unless the device is in WiFi mode.
    pub wifi: Vec<AccessPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BleDevice {
    /// Least significant byte first, as on the air. Use [`crate::adv::Mac`] to print it.
    pub addr: [u8; 6],
    pub class: AddrClass,
    pub kind: Kind,
    pub name: Option<String>,
    pub rssi: i8,
    pub rssi_min: i8,
    pub rssi_max: i8,
    /// Advertisements heard.
    pub count: u32,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccessPoint {
    /// Most significant byte first. Use [`Bssid`] to print it.
    pub bssid: [u8; 6],
    /// Empty for hidden networks.
    pub ssid: String,
    pub channel: u8,
    pub auth: String,
    pub rssi: i8,
    pub rssi_min: i8,
    pub rssi_max: i8,
    /// Scans that found the access point.
    pub count: u32,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
}

/// A six byte address printed most significant byte first, as BSSIDs and chip MACs are.
pub struct Bssid<'a>(pub &'a [u8; 6]);

impl fmt::Display for Bssid<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{b:02X}")?;
        }
        Ok(())
    }
}
