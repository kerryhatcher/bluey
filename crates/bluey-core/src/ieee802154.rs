//! Parsing and rough classification of IEEE 802.15.4 frames, the radio layer under Zigbee and Thread.
//!
//! Only as much of the frame is read as is needed to say who sent it and what they are
//! probably speaking.

use alloc::string::String;

use crate::proto::{NodeAddr, Protocol};

const FRAME_TYPE_BEACON: u8 = 0;
const FRAME_TYPE_DATA: u8 = 1;

const ADDR_MODE_NONE: u8 = 0;
const ADDR_MODE_SHORT: u8 = 2;
const ADDR_MODE_EXTENDED: u8 = 3;

/// First byte of the payload of a Thread beacon.
const THREAD_BEACON_PROTOCOL_ID: u8 = 0x03;
/// A Thread beacon payload is protocol id, version and flags, a 16 byte network name
/// and the 8 byte extended PAN id.
const THREAD_NAME_LEN: usize = 16;
/// A Zigbee beacon payload starts with protocol id 0, then the stack profile and network
/// protocol version (2 for Zigbee PRO).
const ZIGBEE_BEACON_PROTOCOL_ID: u8 = 0x00;

/// A parsed frame.
#[derive(Debug, PartialEq)]
pub struct Frame<'a> {
    pub frame_type: u8,
    /// The network the frame is on: the sender's PAN, or the destination's if they share one.
    pub pan_id: Option<u16>,
    pub src: Option<NodeAddr>,
    pub dest: Option<NodeAddr>,
    /// The MAC layer is encrypted. Thread does that; Zigbee encrypts higher up.
    pub secured: bool,
    /// What follows the MAC header, if it could be located.
    pub payload: Option<&'a [u8]>,
}

/// What a frame says about its sender.
#[derive(Debug, PartialEq)]
pub struct Sender {
    pub protocol: Protocol,
    pub network_name: Option<String>,
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.bytes.split_at_checked(n)?;
        self.bytes = tail;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    fn addr(&mut self, mode: u8) -> Option<NodeAddr> {
        match mode {
            ADDR_MODE_SHORT => self.u16().map(NodeAddr::Short),
            ADDR_MODE_EXTENDED => {
                let b = self.take(8)?;
                // On the air the least significant byte is first.
                let mut addr = [0u8; 8];
                for (out, byte) in addr.iter_mut().zip(b.iter().rev()) {
                    *out = *byte;
                }
                Some(NodeAddr::Extended(addr))
            }
            _ => None,
        }
    }
}

/// Parses a frame, from the frame control field to the end of the payload. The checksum
/// at the end should already be removed.
pub fn parse(bytes: &[u8]) -> Option<Frame<'_>> {
    let mut r = Reader { bytes };
    let control = r.u16()?;
    let frame_type = (control & 0x7) as u8;
    let secured = control & (1 << 3) != 0;
    let pan_compression = control & (1 << 6) != 0;
    let seq_suppressed = control & (1 << 8) != 0;
    let ie_present = control & (1 << 9) != 0;
    let dest_mode = ((control >> 10) & 0x3) as u8;
    let version = (control >> 12) & 0x3;
    let src_mode = ((control >> 14) & 0x3) as u8;

    if !seq_suppressed {
        r.u8()?;
    }

    let has_dest = dest_mode != ADDR_MODE_NONE;
    let has_src = src_mode != ADDR_MODE_NONE;
    // Which PAN ids are present depends on the frame version.
    let (dest_pan_present, src_pan_present) = if version == 2 {
        let both_extended = dest_mode == ADDR_MODE_EXTENDED && src_mode == ADDR_MODE_EXTENDED;
        match (has_dest, has_src) {
            (false, false) => (pan_compression, false),
            (true, false) => (!pan_compression, false),
            (false, true) => (false, !pan_compression),
            (true, true) if both_extended => (pan_compression, false),
            (true, true) => (true, !pan_compression),
        }
    } else {
        (has_dest, has_src && !pan_compression)
    };

    let dest_pan = if dest_pan_present { Some(r.u16()?) } else { None };
    let dest = r.addr(dest_mode);
    let src_pan = if src_pan_present { Some(r.u16()?) } else { None };
    let src = r.addr(src_mode);

    if secured {
        // The auxiliary security header: control, frame counter, and a key identifier whose
        // size depends on the control field.
        let security_control = r.u8()?;
        r.take(4)?;
        r.take(match (security_control >> 3) & 0x3 {
            0 => 0,
            1 => 1,
            2 => 5,
            _ => 9,
        })?;
    }

    // Information elements make the payload hard to find, and nothing we want is in them.
    let payload = if ie_present { None } else { Some(r.bytes) };

    Some(Frame {
        frame_type,
        pan_id: src_pan.or(dest_pan),
        src,
        dest,
        secured,
        payload,
    })
}

/// Works out what the sender of a frame is probably speaking.
pub fn classify(frame: &Frame<'_>) -> Sender {
    let unknown = Sender { protocol: Protocol::Unknown, network_name: None };

    // Zigbee doesn't use MAC layer security, Thread always does.
    if frame.secured {
        return Sender { protocol: Protocol::Thread, network_name: None };
    }
    let Some(payload) = frame.payload else { return unknown };

    match frame.frame_type {
        FRAME_TYPE_BEACON => classify_beacon(payload).unwrap_or(unknown),
        FRAME_TYPE_DATA => classify_data(payload),
        _ => unknown,
    }
}

fn classify_beacon(payload: &[u8]) -> Option<Sender> {
    let mut r = Reader { bytes: payload };
    // The superframe specification, the guaranteed time slots and the pending addresses
    // come first. Thread and Zigbee don't use them, but they have to be stepped over.
    r.take(2)?;
    let gts = r.u8()?;
    if gts & 0x7 != 0 {
        r.take(1 + usize::from(gts & 0x7) * 3)?;
    }
    let pending = r.u8()?;
    r.take(usize::from(pending & 0x7) * 2 + usize::from((pending >> 4) & 0x7) * 8)?;

    match r.u8()? {
        THREAD_BEACON_PROTOCOL_ID => {
            r.u8()?;
            let name = r.take(THREAD_NAME_LEN)?;
            let name = name.split(|&b| b == 0).next().unwrap_or(&[]);
            Some(Sender {
                protocol: Protocol::Thread,
                network_name: core::str::from_utf8(name).ok().map(String::from).filter(|n| !n.is_empty()),
            })
        }
        ZIGBEE_BEACON_PROTOCOL_ID => Some(Sender { protocol: Protocol::Zigbee, network_name: None }),
        _ => None,
    }
}

fn classify_data(payload: &[u8]) -> Sender {
    let protocol = match payload.first() {
        // 6LoWPAN, which Thread carries IPv6 in: an uncompressed IPv6 header, or a compressed one.
        Some(0x41 | 0x60..=0x7F) => Protocol::Thread,
        // The start of a Zigbee network layer header: frame type data or command, protocol
        // version 2 (Zigbee PRO).
        Some(&b) if b & 0x3 <= 1 && (b >> 2) & 0xF == 2 => Protocol::Zigbee,
        _ => Protocol::Unknown,
    };
    Sender { protocol, network_name: None }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    /// A beacon from short address 0x1234 on PAN 0xBEEF, with the given beacon payload.
    fn beacon(payload: &[u8]) -> Vec<u8> {
        // Beacon, short source address, frame version 0.
        let mut frame = alloc::vec![0x00, 0x80, 7];
        frame.extend([0xEF, 0xBE, 0x34, 0x12]);
        // Superframe spec, no GTS, no pending addresses.
        frame.extend([0xFF, 0xCF, 0x00, 0x00]);
        frame.extend(payload);
        frame
    }

    #[test]
    fn thread_beacon_has_the_network_name() {
        let mut payload = alloc::vec![0x03, 0x91];
        payload.extend(b"MyHome\0\0\0\0\0\0\0\0\0\0");
        payload.extend([0u8; 8]);
        let bytes = beacon(&payload);

        let frame = parse(&bytes).unwrap();
        assert_eq!(frame.pan_id, Some(0xBEEF));
        assert_eq!(frame.src, Some(NodeAddr::Short(0x1234)));
        let sender = classify(&frame);
        assert_eq!(sender.protocol, Protocol::Thread);
        assert_eq!(sender.network_name.as_deref(), Some("MyHome"));
    }

    #[test]
    fn zigbee_beacon() {
        let mut payload = alloc::vec![0x00, 0x22, 0x84];
        payload.extend([0u8; 12]);
        let bytes = beacon(&payload);
        assert_eq!(classify(&parse(&bytes).unwrap()).protocol, Protocol::Zigbee);
    }

    #[test]
    fn extended_addresses_are_reversed_into_reading_order() {
        // Data frame, PAN compression, short destination, extended source, frame version 0.
        let mut bytes = alloc::vec![0x41, 0xC8, 1];
        bytes.extend([0xEF, 0xBE, 0x00, 0x00]);
        bytes.extend([8, 7, 6, 5, 4, 3, 2, 1]);
        let frame = parse(&bytes).unwrap();
        assert_eq!(frame.dest, Some(NodeAddr::Short(0)));
        assert_eq!(frame.src, Some(NodeAddr::Extended([1, 2, 3, 4, 5, 6, 7, 8])));
        assert_eq!(frame.pan_id, Some(0xBEEF));
    }

    #[test]
    fn zigbee_and_thread_data() {
        let data = |payload: &[u8]| {
            let mut bytes = alloc::vec![0x41, 0x88, 1, 0xEF, 0xBE, 0x00, 0x00, 0x34, 0x12];
            bytes.extend(payload);
            bytes
        };
        // A Zigbee network layer data frame, protocol version 2.
        assert_eq!(classify(&parse(&data(&[0x08, 0x02])).unwrap()).protocol, Protocol::Zigbee);
        // A compressed IPv6 header.
        assert_eq!(classify(&parse(&data(&[0x7A, 0x33])).unwrap()).protocol, Protocol::Thread);
        assert_eq!(classify(&parse(&data(&[0xFF])).unwrap()).protocol, Protocol::Unknown);
    }

    #[test]
    fn mac_security_means_thread() {
        // Data frame with security enabled, short addresses, key id mode 1.
        let mut bytes = alloc::vec![0x49, 0x98, 1, 0xEF, 0xBE, 0x00, 0x00, 0x34, 0x12];
        bytes.extend([0x0D, 1, 0, 0, 0, 1]);
        bytes.extend([0xDE, 0xAD]);
        let frame = parse(&bytes).unwrap();
        assert_eq!(frame.payload, Some(&[0xDE, 0xAD][..]));
        assert_eq!(classify(&frame).protocol, Protocol::Thread);
    }

    #[test]
    fn frames_without_a_sender_have_no_address() {
        // An acknowledgement: no addressing at all.
        let frame = parse(&[0x02, 0x00, 5]).unwrap();
        assert_eq!(frame.src, None);
        assert_eq!(frame.dest, None);
    }

    #[test]
    fn truncated_frames_do_not_panic() {
        let bytes = beacon(&[0x03, 0x91, b'a']);
        for len in 0..bytes.len() {
            if let Some(frame) = parse(&bytes[..len]) {
                let _ = classify(&frame);
            }
        }
        assert_eq!(parse(&[]), None);
    }
}
