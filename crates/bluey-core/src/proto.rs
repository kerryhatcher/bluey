//! Messages sent by the device.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::adv::{AddrClass, Kind};

/// Bumped whenever the encoding of [`Message`] changes incompatibly.
pub const PROTOCOL_VERSION: u16 = 3;

/// What the device is scanning for. It has one radio, so it can only do one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Bluetooth LE devices.
    Ble,
    /// WiFi access points.
    Wifi,
    /// IEEE 802.15.4 devices: Zigbee and Thread.
    Ieee802154,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Ble, Mode::Wifi, Mode::Ieee802154];

    /// The name used on the command line and in logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ble => "ble",
            Self::Wifi => "wifi",
            Self::Ieee802154 => "802154",
        }
    }

    /// The mode with this name. Also accepts a few friendlier names for 802.15.4.
    pub fn from_label(label: &str) -> Option<Mode> {
        match label {
            "ble" => Some(Self::Ble),
            "wifi" => Some(Self::Wifi),
            "802154" | "zigbee" | "thread" => Some(Self::Ieee802154),
            _ => None,
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
    /// The modes this firmware can switch to. BLE is always one; WiFi and 802.15.4 can't both be
    /// built into one firmware, so a device has at most one of them.
    pub supported: Vec<Mode>,
    /// Time since the device booted. All `*_ms` fields in the snapshot use this clock.
    pub uptime_ms: u64,
    /// Bluetooth LE devices, ordered by address. Empty unless the device is in BLE mode.
    pub ble: Vec<BleDevice>,
    /// WiFi access points, ordered by BSSID. Empty unless the device is in WiFi mode.
    pub wifi: Vec<AccessPoint>,
    /// 802.15.4 devices, ordered by PAN and address. Empty unless the device is in 802.15.4 mode.
    pub ieee802154: Vec<Ieee802154Node>,
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

/// An address on an 802.15.4 network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeAddr {
    /// Assigned by the network, only meaningful within one PAN.
    Short(u16),
    /// The device's EUI-64, most significant byte first.
    Extended([u8; 8]),
}

impl fmt::Display for NodeAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Short(addr) => write!(f, "{addr:04X}"),
            Self::Extended(addr) => {
                for (i, b) in addr.iter().enumerate() {
                    if i > 0 {
                        f.write_str(":")?;
                    }
                    write!(f, "{b:02X}")?;
                }
                Ok(())
            }
        }
    }
}

/// What a device on an 802.15.4 network appears to be speaking. This is a guess from the
/// frames: Thread and Zigbee are both built on 802.15.4 and share its channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Zigbee,
    Thread,
    /// Not enough seen to tell.
    Unknown,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Zigbee => "Zigbee",
            Self::Thread => "Thread",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ieee802154Node {
    /// The network the device belongs to, if the frames said.
    pub pan_id: Option<u16>,
    pub addr: NodeAddr,
    pub protocol: Protocol,
    /// The Thread network name, from a beacon.
    pub network_name: Option<String>,
    /// The channel (11 to 26) it was last heard on.
    pub channel: u8,
    pub rssi: i8,
    pub rssi_min: i8,
    pub rssi_max: i8,
    /// Frames heard from it.
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
