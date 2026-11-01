//! Printing what the device sends.

use bluey_core::adv::Mac;
use bluey_core::proto::{Bssid, LogLevel, LogLine, Snapshot};

pub fn snapshot(snapshot: &Snapshot) {
    let age = |last_seen_ms: u64| snapshot.uptime_ms.saturating_sub(last_seen_ms) / 1000;

    println!("---- {} devices ----", snapshot.ble.len());
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

    println!("---- {} wifi access points ----", snapshot.wifi.len());
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

pub fn log(line: &LogLine) {
    let level = match line.level {
        LogLevel::Error => "ERROR",
        LogLevel::Warn => "WARN",
        LogLevel::Info => "INFO",
        LogLevel::Debug => "DEBUG",
        LogLevel::Trace => "TRACE",
    };
    eprintln!("device {level}: {}", line.text);
}

/// Output that isn't a message: the bootloader at reset, or a panic.
pub fn text(text: &str) {
    for line in text.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        eprintln!("device: {line}");
    }
}
