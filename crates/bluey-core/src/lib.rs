//! Code shared by the bluey firmware and the host CLI.
//!
//! - [`adv`] parses and classifies BLE advertisements.
//! - [`ieee802154`] parses and classifies 802.15.4 frames (Zigbee and Thread).
//! - [`json`] writes the JSON a dashboard's web page reads.
//! - [`proto`] defines the messages the device sends.
//! - [`framing`] turns messages into bytes on the serial port, and back.

#![no_std]

extern crate alloc;

pub mod adv;
pub mod framing;
pub mod ieee802154;
pub mod json;
pub mod proto;
