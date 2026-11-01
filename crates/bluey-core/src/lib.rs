//! Code shared by the bluey firmware and the host CLI.
//!
//! - [`adv`] parses and classifies BLE advertisements.
//! - [`proto`] defines the messages the device sends.
//! - [`framing`] turns messages into bytes on the serial port, and back.

#![no_std]

extern crate alloc;

pub mod adv;
pub mod framing;
pub mod proto;
