//! Printing what the devices send.

use bluey_core::adv::Mac;
use bluey_core::proto::{Bssid, LogLevel, LogLine, Mode, Snapshot};

use crate::port;

/// What to call a device in output: its port and, once known, what it's scanning for.
pub fn label(port: &str, snapshot: Option<&Snapshot>) -> String {
    let name = port::short_name(port);
    match snapshot {
        Some(s) => format!("{name} [{}]", s.mode.label()),
        None => name.to_string(),
    }
}

pub fn snapshot(port: &str, snapshot: &Snapshot) {
    let age = |last_seen_ms: u64| snapshot.uptime_ms.saturating_sub(last_seen_ms) / 1000;

    // The device only scans for one thing at a time.
    match snapshot.mode {
        Mode::Ble => {
            println!("---- {}: {} devices ----", label(port, Some(snapshot)), snapshot.ble.len());
            for d in &snapshot.ble {
                let name = d.name.as_ref().map(|n| format!(" name={n:?}")).unwrap_or_default();
                println!(
                    "{} ({:>6}) rssi={:>4} [{:>4}..{:>4}] n={:<4} seen {:>3}s ago: {}{}",
                    Mac(&d.addr),
                    d.class.label(),
                    d.rssi,
                    d.rssi_min,
                    d.rssi_max,
                    d.count,
                    age(d.last_seen_ms),
                    d.kind.label(),
                    name,
                );
            }
        }
        Mode::Wifi => {
            println!("---- {}: {} wifi access points ----", label(port, Some(snapshot)), snapshot.wifi.len());
            for ap in &snapshot.wifi {
                println!(
                    "{} ch{:<2} rssi={:>4} [{:>4}..{:>4}] n={:<4} seen {:>3}s ago: {} {}",
                    Bssid(&ap.bssid),
                    ap.channel,
                    ap.rssi,
                    ap.rssi_min,
                    ap.rssi_max,
                    ap.count,
                    age(ap.last_seen_ms),
                    if ap.ssid.is_empty() { "<hidden>" } else { &ap.ssid },
                    ap.auth,
                );
            }
        }
    }
}

pub fn log(port: &str, line: &LogLine) {
    let level = match line.level {
        LogLevel::Error => "ERROR",
        LogLevel::Warn => "WARN",
        LogLevel::Info => "INFO",
        LogLevel::Debug => "DEBUG",
        LogLevel::Trace => "TRACE",
    };
    eprintln!("{} {level}: {}", port::short_name(port), line.text);
}

/// Output that isn't a message: the bootloader at reset, or a panic.
pub fn text(port: &str, text: &str) {
    for line in text.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        eprintln!("{}: {line}", port::short_name(port));
    }
}
