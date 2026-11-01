//! Parsing and rough classification of BLE advertising data.

use core::fmt;

const AD_SERVICE_UUIDS_16_INCOMPLETE: u8 = 0x02;
const AD_SERVICE_UUIDS_16_COMPLETE: u8 = 0x03;
const AD_SHORT_NAME: u8 = 0x08;
const AD_COMPLETE_NAME: u8 = 0x09;
const AD_SERVICE_DATA_16: u8 = 0x16;
const AD_MANUFACTURER: u8 = 0xFF;

const COMPANY_APPLE: u16 = 0x004C;
const COMPANY_MICROSOFT: u16 = 0x0006;

const UUID_EXPOSURE_NOTIFICATION: u16 = 0xFD6F;
const UUID_SAMSUNG_FIND: u16 = 0xFD5A;
const UUID_FAST_PAIR: u16 = 0xFE2C;
const UUID_EDDYSTONE: u16 = 0xFEAA;
const UUID_TILE: u16 = 0xFEED;

/// A device address, printed the usual way (most significant byte first).
pub struct Mac<'a>(pub &'a [u8]);

impl fmt::Display for Mac<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().rev().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{b:02X}")?;
        }
        Ok(())
    }
}

/// Address type, derived from the two top bits of a random address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrClass {
    Public,
    RandomStatic,
    /// Rotates periodically (phones, AirTags, ...), can be resolved with an IRK.
    ResolvablePrivate,
    /// Rotates periodically, cannot be resolved.
    NonResolvablePrivate,
}

impl AddrClass {
    pub fn new(random: bool, addr: &[u8]) -> Self {
        if !random {
            return Self::Public;
        }
        match addr.get(5).map_or(0, |msb| msb >> 6) {
            0b11 => Self::RandomStatic,
            0b01 => Self::ResolvablePrivate,
            _ => Self::NonResolvablePrivate,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::RandomStatic => "static",
            Self::ResolvablePrivate => "rpa",
            Self::NonResolvablePrivate => "nrpa",
        }
    }
}

/// What kind of device or protocol an advertisement most likely comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Apple Find My network: AirTags and third-party Find My accessories.
    AppleFindMy,
    AppleNearbyInfo,
    AppleNearbyAction,
    AppleAirPods,
    AppleHandoff,
    AppleAirPlay,
    AppleAirDrop,
    AppleIBeacon,
    AppleOther(u8),
    FastPair,
    SamsungFind,
    Tile,
    Eddystone,
    ExposureNotification,
    Microsoft,
    OtherManufacturer(u16),
    Unknown,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::AppleFindMy => "Apple Find My (AirTag/accessory)",
            Self::AppleNearbyInfo => "Apple Nearby Info (iPhone/iPad/Mac/Watch)",
            Self::AppleNearbyAction => "Apple Nearby Action",
            Self::AppleAirPods => "Apple AirPods/Beats",
            Self::AppleHandoff => "Apple Handoff",
            Self::AppleAirPlay => "Apple AirPlay",
            Self::AppleAirDrop => "Apple AirDrop",
            Self::AppleIBeacon => "iBeacon",
            Self::AppleOther(_) => "Apple (other)",
            Self::FastPair => "Google Fast Pair",
            Self::SamsungFind => "Samsung SmartTag/Find",
            Self::Tile => "Tile",
            Self::Eddystone => "Eddystone",
            Self::ExposureNotification => "Exposure Notification",
            Self::Microsoft => "Microsoft (Windows/Surface/Xbox)",
            Self::OtherManufacturer(_) => "other",
            Self::Unknown => "unknown",
        }
    }
}

/// What we could pull out of one advertisement.
pub struct Summary<'a> {
    pub kind: Kind,
    pub name: Option<&'a str>,
    pub company: Option<u16>,
}

/// Iterates over the (type, payload) AD structures in advertising data.
fn structures(mut data: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    core::iter::from_fn(move || {
        let (&len, rest) = data.split_first()?;
        let len = usize::from(len);
        if len == 0 || len > rest.len() {
            return None;
        }
        let (body, rest) = rest.split_at(len);
        data = rest;
        Some((body[0], &body[1..]))
    })
}

fn classify_apple(payload: &[u8]) -> Kind {
    // Payload is a list of type/length/value records.
    let mut rest = payload;
    let mut first = None;
    while let [ty, len, tail @ ..] = rest {
        let len = usize::from(*len).min(tail.len());
        let kind = match ty {
            0x02 => Kind::AppleIBeacon,
            0x05 => Kind::AppleAirDrop,
            0x07 => Kind::AppleAirPods,
            0x09 => Kind::AppleAirPlay,
            0x0C => Kind::AppleHandoff,
            0x0F => Kind::AppleNearbyAction,
            0x10 => Kind::AppleNearbyInfo,
            0x12 => Kind::AppleFindMy,
            other => Kind::AppleOther(*other),
        };
        // Find My is the one we care most about, even if it isn't the first record.
        if kind == Kind::AppleFindMy {
            return kind;
        }
        first.get_or_insert(kind);
        rest = &tail[len..];
    }
    first.unwrap_or(Kind::AppleOther(0))
}

fn classify_uuid(uuid: u16) -> Option<Kind> {
    match uuid {
        UUID_FAST_PAIR => Some(Kind::FastPair),
        UUID_SAMSUNG_FIND => Some(Kind::SamsungFind),
        UUID_TILE => Some(Kind::Tile),
        UUID_EDDYSTONE => Some(Kind::Eddystone),
        UUID_EXPOSURE_NOTIFICATION => Some(Kind::ExposureNotification),
        _ => None,
    }
}

pub fn summarize(data: &[u8]) -> Summary<'_> {
    let mut summary = Summary { kind: Kind::Unknown, name: None, company: None };

    for (ty, payload) in structures(data) {
        match ty {
            AD_SHORT_NAME | AD_COMPLETE_NAME => {
                summary.name = core::str::from_utf8(payload).ok();
            }
            AD_MANUFACTURER if payload.len() >= 2 => {
                let company = u16::from_le_bytes([payload[0], payload[1]]);
                summary.company = Some(company);
                summary.kind = match company {
                    COMPANY_APPLE => classify_apple(&payload[2..]),
                    COMPANY_MICROSOFT => Kind::Microsoft,
                    other if summary.kind == Kind::Unknown => Kind::OtherManufacturer(other),
                    _ => summary.kind,
                };
            }
            AD_SERVICE_DATA_16 if payload.len() >= 2 => {
                let uuid = u16::from_le_bytes([payload[0], payload[1]]);
                if let Some(kind) = classify_uuid(uuid) {
                    summary.kind = kind;
                }
            }
            AD_SERVICE_UUIDS_16_INCOMPLETE | AD_SERVICE_UUIDS_16_COMPLETE => {
                for &[lo, hi] in payload.as_chunks::<2>().0 {
                    if let Some(kind) = classify_uuid(u16::from_le_bytes([lo, hi]))
                        && summary.kind == Kind::Unknown
                    {
                        summary.kind = kind;
                    }
                }
            }
            _ => {}
        }
    }

    summary
}
