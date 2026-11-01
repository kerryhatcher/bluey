//! The JSON a dashboard's web page reads. It's written by hand rather than with serde so the
//! device doesn't need a JSON library, so it lives here where it can be tested on a computer.

use alloc::string::String;
use core::fmt::{self, Display, Write as _};

use crate::adv::Mac;
use crate::proto::{Bssid, Position, Snapshot};

/// A scanner's snapshot and how long ago it arrived.
pub struct SourceView<'a> {
    pub name: &'a str,
    pub snapshot: &'a Snapshot,
    /// Seconds since the snapshot arrived at the dashboard.
    pub age_s: u64,
}

/// The position and how long ago it arrived.
pub struct PositionView<'a> {
    pub position: &'a Position,
    pub age_s: u64,
}

/// A string in JSON: quoted, with the characters that need it escaped.
struct Quoted<'a>(&'a str);

impl Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for c in self.0.chars() {
            match c {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
                c => f.write_char(c)?,
            }
        }
        f.write_char('"')
    }
}

/// An optional number in JSON.
struct Num<T>(Option<T>);

impl<T: Display> Display for Num<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(n) => write!(f, "{n}"),
            None => f.write_str("null"),
        }
    }
}

/// An optional string in JSON.
struct Text<'a>(Option<&'a str>);

impl Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(s) => write!(f, "{}", Quoted(s)),
            None => f.write_str("null"),
        }
    }
}

/// Seconds since a scanner last heard something, counting the time since its snapshot arrived.
fn heard_s(source: &SourceView<'_>, last_seen_ms: u64) -> u64 {
    source.snapshot.uptime_ms.saturating_sub(last_seen_ms) / 1000 + source.age_s
}

fn write_source(out: &mut String, source: &SourceView<'_>) -> fmt::Result {
    let s = source.snapshot;
    write!(
        out,
        r#"{{"name":{},"device":"{}","mode":"{}","age_s":{},"uptime_s":{},"ble":["#,
        Quoted(source.name),
        Bssid(&s.device),
        s.mode.label(),
        source.age_s,
        s.uptime_ms / 1000,
    )?;
    for (i, d) in s.ble.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(
            out,
            r#"{{"addr":"{}","type":"{}","kind":{},"name":{},"rssi":{},"min":{},"max":{},"count":{},"age_s":{}}}"#,
            Mac(&d.addr),
            d.class.label(),
            Quoted(d.kind.label()),
            Text(d.name.as_deref()),
            d.rssi,
            d.rssi_min,
            d.rssi_max,
            d.count,
            heard_s(source, d.last_seen_ms),
        )?;
    }
    out.push_str(r#"],"wifi":["#);
    for (i, ap) in s.wifi.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(
            out,
            r#"{{"bssid":"{}","ssid":{},"channel":{},"auth":{},"rssi":{},"min":{},"max":{},"count":{},"age_s":{}}}"#,
            Bssid(&ap.bssid),
            Quoted(&ap.ssid),
            ap.channel,
            Quoted(&ap.auth),
            ap.rssi,
            ap.rssi_min,
            ap.rssi_max,
            ap.count,
            heard_s(source, ap.last_seen_ms),
        )?;
    }
    out.push_str(r#"],"ieee802154":["#);
    for (i, n) in s.ieee802154.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let pan = n.pan_id.map(|p| alloc::format!("{p:04X}"));
        write!(
            out,
            r#"{{"pan":{},"addr":"{}","protocol":"{}","network":{},"channel":{},"rssi":{},"min":{},"max":{},"count":{},"age_s":{}}}"#,
            Text(pan.as_deref()),
            n.addr,
            n.protocol.label(),
            Text(n.network_name.as_deref()),
            n.channel,
            n.rssi,
            n.rssi_min,
            n.rssi_max,
            n.count,
            heard_s(source, n.last_seen_ms),
        )?;
    }
    out.push_str("]}");
    Ok(())
}

/// Everything the dashboard knows, as JSON.
pub fn state(
    ssid: &str,
    uptime_s: u64,
    clients: u8,
    heap_free: usize,
    position: Option<PositionView<'_>>,
    sources: &[SourceView<'_>],
) -> String {
    let mut out = String::with_capacity(8 * 1024);
    let _ = write!(
        out,
        r#"{{"ssid":{},"uptime_s":{uptime_s},"clients":{clients},"heap_free":{heap_free},"position":"#,
        Quoted(ssid),
    );
    match position {
        Some(PositionView { position: p, age_s }) => {
            let _ = write!(
                out,
                r#"{{"fix":{},"satellites":{},"latitude":{},"longitude":{},"altitude_m":{},"hdop":{},"speed_mps":{},"course_deg":{},"age_s":{age_s}}}"#,
                p.fix,
                Num(p.satellites),
                Num(p.latitude),
                Num(p.longitude),
                Num(p.altitude_m),
                Num(p.hdop),
                Num(p.speed_mps),
                Num(p.course_deg),
            );
        }
        None => out.push_str("null"),
    }
    out.push_str(r#","sources":["#);
    for (i, source) in sources.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write_source(&mut out, source);
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    extern crate std;

    use alloc::string::ToString;
    use alloc::vec;

    use super::*;
    use crate::adv::{AddrClass, Kind};
    use crate::proto::{AccessPoint, BleDevice, Ieee802154Node, Mode, NodeAddr, PROTOCOL_VERSION, Protocol};

    fn snapshot() -> Snapshot {
        Snapshot {
            protocol: PROTOCOL_VERSION,
            device: [0x90, 0x64, 0x9B, 0xAA, 0x29, 0x00],
            mode: Mode::Ble,
            supported: vec![Mode::Ble, Mode::Wifi],
            uptime_ms: 100_000,
            ble: vec![BleDevice {
                addr: [1, 2, 3, 4, 5, 0xAB],
                class: AddrClass::ResolvablePrivate,
                kind: Kind::AppleFindMy,
                // Anything can be advertised as a name.
                name: Some("a\"quote\\ and <script>alert(1)</script>\n\tnewline".to_string()),
                rssi: -61,
                rssi_min: -70,
                rssi_max: -55,
                count: 12,
                first_seen_ms: 10_000,
                last_seen_ms: 97_000,
            }],
            wifi: vec![AccessPoint {
                bssid: [0xF4, 0x92, 0xBF, 0x92, 0x56, 0xC9],
                ssid: "Cafe \u{2615} \"free\"".to_string(),
                channel: 6,
                auth: "Wpa2Personal".to_string(),
                rssi: -50,
                rssi_min: -52,
                rssi_max: -48,
                count: 3,
                first_seen_ms: 1,
                last_seen_ms: 100_000,
            }],
            ieee802154: vec![Ieee802154Node {
                pan_id: Some(0x9297),
                addr: NodeAddr::Short(0x3148),
                protocol: Protocol::Zigbee,
                network_name: None,
                channel: 25,
                rssi: -76,
                rssi_min: -78,
                rssi_max: -73,
                count: 17,
                first_seen_ms: 1,
                last_seen_ms: 99_000,
            }],
        }
    }

    fn parse(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("not JSON ({e}): {json}"))
    }

    #[test]
    fn hostile_names_stay_valid_json() {
        let snapshot = snapshot();
        let json = state("my \"net\"", 5, 2, 1000, None, &[SourceView { name: "tty\"ACM0", snapshot: &snapshot, age_s: 4 }]);
        let v = parse(&json);
        assert_eq!(v["ssid"], "my \"net\"");
        assert_eq!(v["sources"][0]["name"], "tty\"ACM0");
        // Comes back exactly as advertised, so the page is what has to escape it.
        assert_eq!(v["sources"][0]["ble"][0]["name"], "a\"quote\\ and <script>alert(1)</script>\n\tnewline");
        assert_eq!(v["sources"][0]["wifi"][0]["ssid"], "Cafe \u{2615} \"free\"");
    }

    #[test]
    fn fields_and_ages() {
        let snapshot = snapshot();
        let json = state("bluey", 5, 1, 2000, None, &[SourceView { name: "ttyACM0", snapshot: &snapshot, age_s: 4 }]);
        let v = parse(&json);
        let source = &v["sources"][0];
        assert_eq!(source["device"], "90:64:9B:AA:29:00");
        assert_eq!(source["mode"], "ble");
        assert_eq!(source["age_s"], 4);
        let ble = &source["ble"][0];
        assert_eq!(ble["addr"], "AB:05:04:03:02:01");
        assert_eq!(ble["kind"], "Apple Find My (AirTag/accessory)");
        assert_eq!(ble["type"], "rpa");
        assert_eq!(ble["rssi"], -61);
        // Last heard 3 s before the snapshot, which arrived 4 s ago.
        assert_eq!(ble["age_s"], 7);
        let node = &source["ieee802154"][0];
        assert_eq!(node["pan"], "9297");
        assert_eq!(node["addr"], "3148");
        assert_eq!(node["protocol"], "Zigbee");
        assert!(node["network"].is_null());
        assert_eq!(source["wifi"][0]["bssid"], "F4:92:BF:92:56:C9");
    }

    #[test]
    fn position_with_and_without_a_fix() {
        let fix = Position {
            fix: true,
            satellites: Some(6),
            latitude: Some(32.5),
            longitude: Some(-83.25),
            altitude_m: Some(140.0),
            hdop: Some(1.4),
            speed_mps: None,
            course_deg: None,
        };
        let json = state("bluey", 0, 0, 0, Some(PositionView { position: &fix, age_s: 2 }), &[]);
        let v = parse(&json);
        assert_eq!(v["position"]["fix"], true);
        assert_eq!(v["position"]["latitude"], 32.5);
        assert_eq!(v["position"]["longitude"], -83.25);
        assert!(v["position"]["speed_mps"].is_null());
        assert_eq!(v["position"]["age_s"], 2);
        assert_eq!(v["sources"].as_array().map(|a| a.len()), Some(0));

        let none = Position { fix: false, satellites: Some(0), latitude: None, longitude: None, altitude_m: None, hdop: None, speed_mps: None, course_deg: None };
        let v = parse(&state("bluey", 0, 0, 0, Some(PositionView { position: &none, age_s: 0 }), &[]));
        assert_eq!(v["position"]["fix"], false);
        assert!(v["position"]["latitude"].is_null());

        assert!(parse(&state("bluey", 0, 0, 0, None, &[]))["position"].is_null());
    }

    #[test]
    fn empty_tables_are_empty_arrays() {
        let mut s = snapshot();
        s.ble.clear();
        s.wifi.clear();
        s.ieee802154.clear();
        let v = parse(&state("bluey", 0, 0, 0, None, &[SourceView { name: "x", snapshot: &s, age_s: 0 }]));
        assert_eq!(v["sources"][0]["ble"].as_array().map(|a| a.len()), Some(0));
        assert_eq!(v["sources"][0]["wifi"].as_array().map(|a| a.len()), Some(0));
        assert_eq!(v["sources"][0]["ieee802154"].as_array().map(|a| a.len()), Some(0));
    }

    #[test]
    fn many_sources_are_comma_separated() {
        let s = snapshot();
        let sources: alloc::vec::Vec<_> = (0..3).map(|_| SourceView { name: "x", snapshot: &s, age_s: 0 }).collect();
        let v = parse(&state("bluey", 0, 0, 0, None, &sources));
        assert_eq!(v["sources"].as_array().map(|a| a.len()), Some(3));
    }
}
