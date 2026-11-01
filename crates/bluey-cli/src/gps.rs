//! Reading position from a USB GPS, which sends NMEA sentences over a serial port.
//!
//! `bluey log` uses it to say where each snapshot was taken.

use std::io::{ErrorKind, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serialport::SerialPortType;

/// USB vendor ID of Espressif. Those ports are the bluey devices, not GPS receivers.
const ESPRESSIF_VID: u16 = 0x303a;

/// Speeds GPS receivers commonly use, most common first.
const BAUDS: [u32; 4] = [4800, 9600, 38400, 115_200];

/// How long to listen to a port at one speed before deciding it isn't a GPS.
const PROBE_TIME: Duration = Duration::from_millis(1500);

/// A reading older than this is no longer a fix. Receivers send one every second.
const STALE_AFTER: Duration = Duration::from_secs(5);

/// Where and how well the GPS can see, as put in the log.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reading {
    /// Whether the receiver has a position. If not, only `satellites` is meaningful.
    pub fix: bool,
    pub satellites: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altitude_m: Option<f64>,
    /// Horizontal dilution of precision: lower is better, under 2 is good.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hdop: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_mps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub course_deg: Option<f64>,
    /// How long ago the receiver last reported, in milliseconds. Absent if it never has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Gga {
    fix: bool,
    satellites: Option<u8>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    altitude_m: Option<f64>,
    hdop: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Rmc {
    valid: bool,
    speed_mps: Option<f64>,
    course_deg: Option<f64>,
}

#[derive(Debug, PartialEq)]
enum Sentence {
    Gga(Gga),
    Rmc(Rmc),
    /// Valid NMEA, but not one we use.
    Other,
}

/// The checksum is the XOR of everything between `$` and `*`, as two hex digits.
fn checksum_ok(line: &str) -> Option<&str> {
    let body = line.strip_prefix('$')?;
    let (body, sum) = body.rsplit_once('*')?;
    let sum = u8::from_str_radix(sum.trim(), 16).ok()?;
    (body.bytes().fold(0, |acc, b| acc ^ b) == sum).then_some(body)
}

/// Converts NMEA's `ddmm.mmmm` (or `dddmm.mmmm`) and a hemisphere to signed degrees.
fn coordinate(value: &str, hemisphere: &str) -> Option<f64> {
    let dot = value.find('.')?;
    let split = dot.checked_sub(2)?;
    let degrees: f64 = value[..split].parse().ok()?;
    let minutes: f64 = value[split..].parse().ok()?;
    let degrees = degrees + minutes / 60.0;
    match hemisphere {
        "N" | "E" => Some(degrees),
        "S" | "W" => Some(-degrees),
        _ => None,
    }
}

fn number(field: Option<&&str>) -> Option<f64> {
    field?.parse().ok()
}

/// Parses one line from the receiver. `None` if it isn't valid NMEA.
fn parse(line: &str) -> Option<Sentence> {
    let body = checksum_ok(line.trim())?;
    let fields: Vec<&str> = body.split(',').collect();
    // The first field is the talker (GP, GN, GL, ...) then the sentence type.
    let kind = fields.first()?.get(2..)?;
    Some(match kind {
        "GGA" => Sentence::Gga(Gga {
            fix: fields.get(6).is_some_and(|q| !q.is_empty() && *q != "0"),
            satellites: fields.get(7).and_then(|s| s.parse().ok()),
            latitude: coordinate(fields.get(2)?, fields.get(3)?),
            longitude: coordinate(fields.get(4)?, fields.get(5)?),
            hdop: number(fields.get(8)),
            altitude_m: number(fields.get(9)),
        }),
        "RMC" => Sentence::Rmc(Rmc {
            valid: fields.get(2) == Some(&"A"),
            speed_mps: number(fields.get(7)).map(|knots| knots * 0.514_444),
            course_deg: number(fields.get(8)),
        }),
        _ => Sentence::Other,
    })
}

#[derive(Default)]
struct State {
    gga: Option<(Gga, Instant)>,
    rmc: Option<(Rmc, Instant)>,
    /// Why the reader stopped, if it did.
    error: Option<String>,
}

/// A GPS being read in the background.
pub struct Gps {
    pub port: String,
    pub baud: u32,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Gps {
    /// Starts reading from a port at a known speed.
    pub fn start(port: &str, baud: u32) -> Result<Gps> {
        let mut serial = serialport::new(port, baud)
            .timeout(Duration::from_millis(250))
            .open()
            .with_context(|| format!("opening {port}"))?;
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let thread = {
            let (state, stop) = (state.clone(), stop.clone());
            thread::spawn(move || {
                let mut lines = Lines::default();
                let mut buf = [0u8; 512];
                while !stop.load(Ordering::Relaxed) {
                    match serial.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            for line in lines.push(&buf[..n]) {
                                record(&state, &line);
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::TimedOut => {}
                        Err(e) => {
                            state.lock().unwrap().error = Some(e.to_string());
                            break;
                        }
                    }
                }
            })
        };
        Ok(Gps { port: port.to_string(), baud, state, stop, thread: Some(thread) })
    }

    /// Waits for the receiver to report something, so the first reading isn't empty just
    /// because it has only just been opened. Receivers send every second.
    fn wait_for_first_report(&self) {
        let end = Instant::now() + Duration::from_millis(2500);
        while Instant::now() < end && self.state.lock().unwrap().gga.is_none() {
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Where the GPS is now, or that it has no fix.
    pub fn reading(&self) -> Reading {
        let state = self.state.lock().unwrap();
        let now = Instant::now();
        let fresh = |t: &Instant| now.duration_since(*t) < STALE_AFTER;

        let gga = state.gga.as_ref().filter(|(_, t)| fresh(t));
        let rmc = state.rmc.as_ref().filter(|(r, t)| r.valid && fresh(t));
        let age = state.gga.as_ref().map(|(_, t)| now.duration_since(*t));
        let age_ms = age.map(|a| u64::try_from(a.as_millis()).unwrap_or(u64::MAX));

        match gga {
            Some((g, _)) if g.fix && g.latitude.is_some() && g.longitude.is_some() => Reading {
                fix: true,
                satellites: g.satellites,
                latitude: g.latitude,
                longitude: g.longitude,
                altitude_m: g.altitude_m,
                hdop: g.hdop,
                speed_mps: rmc.and_then(|(r, _)| r.speed_mps),
                course_deg: rmc.and_then(|(r, _)| r.course_deg),
                age_ms,
            },
            _ => Reading {
                fix: false,
                satellites: gga.and_then(|(g, _)| g.satellites),
                latitude: None,
                longitude: None,
                altitude_m: None,
                hdop: None,
                speed_mps: None,
                course_deg: None,
                age_ms,
            },
        }
    }

    /// Why the GPS stopped being read, e.g. it was unplugged.
    pub fn error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }
}

impl From<&Reading> for bluey_core::proto::Position {
    fn from(r: &Reading) -> Self {
        Self {
            fix: r.fix,
            satellites: r.satellites,
            latitude: r.latitude,
            longitude: r.longitude,
            altitude_m: r.altitude_m,
            hdop: r.hdop,
            speed_mps: r.speed_mps,
            course_deg: r.course_deg,
        }
    }
}

impl Drop for Gps {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn record(state: &Mutex<State>, line: &str) {
    let now = Instant::now();
    match parse(line) {
        Some(Sentence::Gga(gga)) => state.lock().unwrap().gga = Some((gga, now)),
        Some(Sentence::Rmc(rmc)) => state.lock().unwrap().rmc = Some((rmc, now)),
        Some(Sentence::Other) | None => {}
    }
}

/// Splits a byte stream into lines.
#[derive(Default)]
struct Lines {
    partial: Vec<u8>,
}

impl Lines {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                lines.push(String::from_utf8_lossy(&self.partial).into_owned());
                self.partial.clear();
            } else if self.partial.len() < 256 {
                self.partial.push(b);
            } else {
                // Not NMEA, which has a maximum line length of 82.
                self.partial.clear();
            }
        }
        lines
    }
}

/// Whether the port sends NMEA at this speed.
fn sends_nmea(port: &str, baud: u32) -> bool {
    let Ok(mut serial) = serialport::new(port, baud).timeout(Duration::from_millis(200)).open() else {
        return false;
    };
    let mut lines = Lines::default();
    let mut buf = [0u8; 256];
    let end = Instant::now() + PROBE_TIME;
    while Instant::now() < end {
        match serial.read(&mut buf) {
            Ok(n) if n > 0 => {
                if lines.push(&buf[..n]).iter().any(|l| parse(l).is_some()) {
                    return true;
                }
            }
            Ok(_) => return false,
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(_) => return false,
        }
    }
    false
}

/// Works out the speed a GPS on this port uses.
fn find_baud(port: &str) -> Option<u32> {
    BAUDS.into_iter().find(|&baud| sends_nmea(port, baud))
}

/// What the user said to use for a GPS: a port, and optionally its speed.
pub fn parse_spec(spec: &str) -> (String, Option<u32>) {
    match spec.rsplit_once(':') {
        Some((port, baud)) => match baud.parse() {
            Ok(baud) => (port.to_string(), Some(baud)),
            Err(_) => (spec.to_string(), None),
        },
        None => (spec.to_string(), None),
    }
}

/// Starts the GPS the user named.
pub fn open(spec: &str) -> Result<Gps> {
    let (port, baud) = parse_spec(spec);
    let baud = match baud {
        Some(baud) => baud,
        None => find_baud(&port).with_context(|| format!("no NMEA data from {port}; is it a GPS?"))?,
    };
    let gps = Gps::start(&port, baud)?;
    gps.wait_for_first_report();
    Ok(gps)
}

/// Looks for a GPS among the USB serial ports that aren't bluey devices.
///
/// This opens each one and listens, which is harmless for a GPS but can reset boards like an
/// Arduino that restart when a port is opened.
pub fn detect() -> Result<Option<Gps>> {
    let mut candidates: Vec<_> = serialport::available_ports()
        .context("listing serial ports")?
        .into_iter()
        .filter(|p| matches!(&p.port_type, SerialPortType::UsbPort(usb) if usb.vid != ESPRESSIF_VID))
        .map(|p| p.port_name)
        .collect();
    candidates.sort();
    for port in candidates {
        if let Some(baud) = find_baud(&port) {
            let gps = Gps::start(&port, baud)?;
            gps.wait_for_first_report();
            return Ok(Some(gps));
        }
    }
    Ok(None)
}

/// Prints the GPS status once a second, until interrupted.
pub fn watch(gps: &Gps) -> Result<()> {
    println!("reading {} at {} baud", gps.port, gps.baud);
    loop {
        thread::sleep(Duration::from_secs(1));
        if let Some(error) = gps.error() {
            bail!("{} stopped: {error}", gps.port);
        }
        println!("{}", describe(&gps.reading()));
    }
}

/// A one line description of a reading.
pub fn describe(r: &Reading) -> String {
    let sats = r.satellites.map_or("no".to_string(), |s| s.to_string());
    match (r.fix, r.latitude, r.longitude) {
        (true, Some(lat), Some(lon)) => {
            let mut s = format!("fix: {lat:.6}, {lon:.6}  {sats} satellites");
            if let Some(alt) = r.altitude_m {
                s += &format!("  {alt:.0} m");
            }
            if let Some(hdop) = r.hdop {
                s += &format!("  hdop {hdop:.1}");
            }
            s
        }
        _ => format!("no fix  {sats} satellites in use"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The examples from the NMEA documentation.
    const GGA: &str = "$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47";
    const RMC: &str = "$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A";
    // What the receiver sent with no satellites in view.
    const GGA_NO_FIX: &str = "$GPGGA,043349.431,,,,,0,00,,,M,0.0,M,,0000*59";
    const RMC_NO_FIX: &str = "$GPRMC,043349.431,V,,,,,,,011126,,,N*47";

    #[test]
    fn gga_with_a_fix() {
        let Some(Sentence::Gga(gga)) = parse(GGA) else { panic!("not a GGA") };
        assert!(gga.fix);
        assert_eq!(gga.satellites, Some(8));
        assert!((gga.latitude.unwrap() - 48.1173).abs() < 1e-4);
        assert!((gga.longitude.unwrap() - 11.516_667).abs() < 1e-4);
        assert_eq!(gga.altitude_m, Some(545.4));
        assert_eq!(gga.hdop, Some(0.9));
    }

    #[test]
    fn southern_and_western_hemispheres_are_negative() {
        assert!((coordinate("3345.000", "S").unwrap() + 33.75).abs() < 1e-9);
        assert!((coordinate("08423.000", "W").unwrap() + 84.383_333).abs() < 1e-5);
    }

    #[test]
    fn rmc_speed_is_converted_to_metres_per_second() {
        let Some(Sentence::Rmc(rmc)) = parse(RMC) else { panic!("not an RMC") };
        assert!(rmc.valid);
        assert!((rmc.speed_mps.unwrap() - 22.4 * 0.514_444).abs() < 1e-6);
        assert_eq!(rmc.course_deg, Some(84.4));
    }

    #[test]
    fn no_fix() {
        let Some(Sentence::Gga(gga)) = parse(GGA_NO_FIX) else { panic!("not a GGA") };
        assert!(!gga.fix);
        assert_eq!(gga.satellites, Some(0));
        assert_eq!(gga.latitude, None);
        let Some(Sentence::Rmc(rmc)) = parse(RMC_NO_FIX) else { panic!("not an RMC") };
        assert!(!rmc.valid);
    }

    #[test]
    fn bad_checksums_and_garbage_are_rejected() {
        assert_eq!(parse("$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*00"), None);
        assert_eq!(parse("`x`�~�`�xxxfxx��`xxx"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("$GPGGA"), None);
    }

    #[test]
    fn other_sentences_are_valid_but_ignored() {
        assert_eq!(parse("$GPGSA,M,1,,,,,,,,,,,,,,,*12"), Some(Sentence::Other));
    }

    #[test]
    fn lines_split_across_reads() {
        let mut lines = Lines::default();
        assert!(lines.push(b"$GPGSA,M,").is_empty());
        assert_eq!(lines.push(b"1,,,\r\n$GP"), vec!["$GPGSA,M,1,,,\r".to_string()]);
    }

    #[test]
    fn spec_with_and_without_a_speed() {
        assert_eq!(parse_spec("/dev/ttyUSB0:4800"), ("/dev/ttyUSB0".to_string(), Some(4800)));
        assert_eq!(parse_spec("/dev/ttyUSB0"), ("/dev/ttyUSB0".to_string(), None));
    }
}
