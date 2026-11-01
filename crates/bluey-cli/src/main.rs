mod gps;
mod port;
mod record;
mod store;
mod render;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bluey_core::framing::Item;
use bluey_core::proto::{Bssid, Command as DeviceCommand, Feed, Hello, Message, Mode, Position, Role, Snapshot, Source};
use clap::{Args, Parser, Subcommand};

use crate::port::{Event, Target};

/// Reads what bluey devices hear: Bluetooth LE devices or WiFi access points.
///
/// A device scans for one thing at a time. With several devices plugged in, every
/// command works on all of them, so one can be in each mode.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Serial port of a device, optionally followed by the mode to put it in: /dev/ttyACM0:wifi.
    /// Can be given more than once. Defaults to every ESP32 that is plugged in.
    #[arg(short, long = "port", global = true, value_name = "PORT[:MODE]", value_parser = port::parse_target)]
    ports: Vec<Target>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct GpsArgs {
    /// USB GPS to take the position from, optionally with its speed: /dev/ttyUSB0 or
    /// /dev/ttyUSB0:4800. Without this, the USB serial ports are searched for one.
    #[arg(long, value_name = "PORT[:BAUD]", conflicts_with = "no_gps")]
    gps: Option<String>,
    /// Don't look for a GPS.
    #[arg(long)]
    no_gps: bool,
}

impl GpsArgs {
    /// The GPS to use, if there is one.
    fn open(&self) -> Result<Option<gps::Gps>> {
        if self.no_gps {
            return Ok(None);
        }
        let found = match &self.gps {
            Some(spec) => Some(gps::open(spec)?),
            None => gps::detect()?,
        };
        match &found {
            Some(g) => eprintln!("position from the GPS on {} ({} baud)", g.port, g.baud),
            None => eprintln!("no GPS found, so the log won't have positions (`--gps` names one)"),
        }
        Ok(found)
    }
}

#[derive(Args)]
struct StoreArgs {
    /// GeoPackage to add to, instead of the one in the XDG data folder.
    #[arg(long, value_name = "FILE", conflicts_with = "no_gpkg")]
    gpkg: Option<PathBuf>,
    /// Don't record to a GeoPackage.
    #[arg(long)]
    no_gpkg: bool,
    /// Also append each snapshot to this file, one JSON object per line.
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
    /// Replace the JSON file instead of adding to it. The GeoPackage is never replaced.
    #[arg(long, requires = "output")]
    truncate: bool,
}

impl StoreArgs {
    /// Opens what was asked for. `by_default` says whether the GeoPackage is used when not named.
    fn open(&self, by_default: bool) -> Result<Sinks> {
        let jsonl = match &self.output {
            Some(path) => Some((open_log(path, self.truncate)?, path.clone())),
            None => None,
        };
        let gpkg_path = match (&self.gpkg, self.no_gpkg, by_default) {
            (_, true, _) => None,
            (Some(path), false, _) => Some(path.clone()),
            (None, false, true) => Some(store::default_path()?),
            (None, false, false) => None,
        };
        let gpkg = gpkg_path.map(|path| store::GeoPackage::open(&path).map(|g| (g, path))).transpose()?;

        for path in gpkg.iter().map(|(_, p)| p).chain(jsonl.iter().map(|(_, p)| p)) {
            eprintln!("recording to {}", path.display());
        }
        Ok(Sinks { jsonl, gpkg, written: 0 })
    }
}

/// Where `log` and `dashboard` put what they hear.
struct Sinks {
    jsonl: Option<(std::fs::File, PathBuf)>,
    gpkg: Option<(store::GeoPackage, PathBuf)>,
    written: u64,
}

impl Sinks {
    fn is_empty(&self) -> bool {
        self.jsonl.is_none() && self.gpkg.is_none()
    }

    /// Records one snapshot everywhere, with the position if there is a GPS, and says so.
    fn record(&mut self, port: &str, s: &Snapshot, gps: Option<&gps::Gps>) -> Result<()> {
        let reading = gps.map(gps::Gps::reading);
        let record = record::Record::new(port, s, reading.clone())?;
        if let Some((file, _)) = &mut self.jsonl {
            writeln!(file, "{}", serde_json::to_string(&record)?)?;
            file.flush()?;
        }
        let rows = match &mut self.gpkg {
            Some((gpkg, _)) => Some(gpkg.record(&record)?),
            None => None,
        };
        self.written += 1;
        let position = reading.map(|r| format!("  [{}]", gps::describe(&r))).unwrap_or_default();
        let rows = rows.map(|n| format!(" ({n} rows)")).unwrap_or_default();
        eprintln!("{} snapshot {}: {}{rows}{position}", render::label(port, Some(s)), self.written, render::summary(s));
        Ok(())
    }
}

#[derive(Subcommand)]
enum Command {
    /// List serial ports.
    Ports,
    /// Show each device and what it's doing.
    Devices,
    /// Switch devices to scan for something else.
    ///
    /// ble: Bluetooth LE devices. wifi: WiFi access points. 802154: Zigbee and Thread devices.
    /// A device has BLE and one of the other two, depending on the firmware flashed on it.
    Mode {
        #[arg(value_parser = port::parse_mode)]
        mode: Mode,
    },
    /// Show what the devices hear, as they hear it.
    Monitor,
    /// Show what the GPS reports, once a second.
    Gps {
        #[command(flatten)]
        gps: GpsArgs,
    },
    /// Record what the devices hear, with where and when, into a GeoPackage.
    ///
    /// The GeoPackage is kept in the XDG data folder ($XDG_DATA_HOME/bluey/bluey.gpkg, usually
    /// ~/.local/share/bluey/) and only ever added to, so it builds up over time. Open it in QGIS to
    /// see it on a map. If a USB GPS is plugged in, each row gets its position.
    Log {
        #[command(flatten)]
        store: StoreArgs,
        #[command(flatten)]
        gps: GpsArgs,
    },
    /// Send what the scanners hear (and the GPS position) to a dashboard device.
    ///
    /// The dashboard is an ESP32-S3 with a screen that runs its own Wi-Fi network and web page.
    /// It is found by itself among the devices plugged in. This keeps running, like `log`, and
    /// can record at the same time.
    Dashboard {
        /// Also record to the GeoPackage, like `bluey log`.
        #[arg(long)]
        record: bool,
        #[command(flatten)]
        store: StoreArgs,
        #[command(flatten)]
        gps: GpsArgs,
    },
    /// Wait for a snapshot from each device, print them and exit.
    Snapshot {
        /// Print JSON instead of tables.
        #[arg(long)]
        json: bool,
    },
}

/// How long to wait for a device to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ports => port::list(),
        Command::Devices => devices(port::resolve(&cli.ports)?),
        Command::Mode { mode } => set_mode(port::resolve(&cli.ports)?, mode),
        Command::Monitor => monitor(port::resolve(&cli.ports)?),
        Command::Gps { gps } => match gps.open()? {
            Some(g) => gps::watch(&g),
            None => bail!("no GPS found; `--gps PORT` names one"),
        },
        Command::Log { store, gps } => log(port::resolve(&cli.ports)?, store.open(true)?, gps.open()?),
        Command::Dashboard { record, store, gps } => {
            dashboard(port::resolve(&cli.ports)?, store.open(record)?, gps.open()?)
        }
        Command::Snapshot { json } => snapshot(port::resolve(&cli.ports)?, json),
    }
}

/// What the devices answered with.
struct Gathered {
    /// The scanners' snapshots, by port.
    scanners: BTreeMap<String, Snapshot>,
    /// Devices that don't scan, by port.
    dashboards: BTreeMap<String, Hello>,
}

/// Waits until every target has answered, or the time is up. A scanner answers with the first
/// snapshot that `wanted` accepts, anything else says what it is.
fn gather(targets: Vec<Target>, wanted: impl Fn(&Target, &Snapshot) -> bool) -> Result<Gathered> {
    let by_port: BTreeMap<_, _> = targets.iter().map(|t| (t.port.clone(), t.clone())).collect();
    let watch = port::watch(targets);
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    let mut found = Gathered { scanners: BTreeMap::new(), dashboards: BTreeMap::new() };
    let mut failures = Vec::new();

    while found.scanners.len() + found.dashboards.len() + failures.len() < by_port.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let update = match watch.recv_timeout(remaining) {
            Ok(update) => update,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        };
        match update.event {
            Event::Item(Item::Message(Message::Snapshot(s))) => {
                if wanted(&by_port[&update.port], &s) {
                    found.scanners.entry(update.port).or_insert(s);
                }
            }
            Event::Item(Item::Message(Message::Hello(hello))) => {
                if hello.role == Role::Dashboard {
                    found.dashboards.entry(update.port).or_insert(hello);
                }
            }
            Event::Item(Item::Message(Message::Log(line))) => render::log(&update.port, &line),
            Event::Item(Item::Text(_)) => {}
            Event::Closed(reason) => failures.push(format!("{}: {reason}", update.port)),
        }
    }

    if !failures.is_empty() {
        bail!("{}", failures.join("\n"));
    }
    let answered = |p: &&String| found.scanners.contains_key(*p) || found.dashboards.contains_key(*p);
    if let Some(silent) = by_port.keys().find(|p| !answered(p)) {
        bail!("no answer from {silent} within {} seconds", ANSWER_TIMEOUT.as_secs());
    }
    Ok(found)
}

fn devices(targets: Vec<Target>) -> Result<()> {
    let found = gather(targets, |_, _| true)?;
    let mut ports: Vec<&String> = found.scanners.keys().chain(found.dashboards.keys()).collect();
    ports.sort();
    for port in ports {
        if let Some(s) = found.scanners.get(port) {
            let can: Vec<_> = s.supported.iter().map(|m| m.label()).collect();
            println!(
                "{port}  {}  {}  up {}s  {}  (can scan for: {})",
                Bssid(&s.device),
                s.mode.label(),
                s.uptime_ms / 1000,
                render::summary(s),
                can.join(", ")
            );
        } else if let Some(hello) = found.dashboards.get(port) {
            println!("{port}  {}  dashboard  (shows what `bluey dashboard` sends it)", Bssid(&hello.device));
        }
    }
    Ok(())
}

fn set_mode(targets: Vec<Target>, mode: Mode) -> Result<()> {
    // Ask what each device can do first, so a device that can't is reported as such rather
    // than just never answering. Dashboards don't scan, so they are left out.
    let found = gather(targets.clone(), |_, _| true)?;
    let unsupported: Vec<_> = found
        .scanners
        .iter()
        .filter(|(_, s)| !s.supported.contains(&mode))
        .map(|(port, s)| {
            let can: Vec<_> = s.supported.iter().map(|m| m.label()).collect();
            format!("{port} can only scan for {}; see the README for flashing the other firmware", can.join(" and "))
        })
        .collect();
    if !unsupported.is_empty() {
        bail!("{}", unsupported.join("\n"));
    }

    let targets: Vec<_> = targets
        .into_iter()
        .filter(|t| found.scanners.contains_key(&t.port))
        .map(|t| Target { mode: Some(mode), ..t })
        .collect();
    if targets.is_empty() {
        bail!("none of the devices scans");
    }
    // An older periodic snapshot can arrive before the switch has happened.
    for (port, s) in gather(targets, |_, s| s.mode == mode)?.scanners {
        println!("{port}  {}  now scanning for {}", Bssid(&s.device), mode.label());
    }
    Ok(())
}

/// Says once that a device is a dashboard, which is why it sends no snapshots.
fn note_dashboard(noted: &mut BTreeSet<String>, port: &str) {
    if noted.insert(port.to_string()) {
        eprintln!("{} is a dashboard, not a scanner (`bluey dashboard` feeds it)", port::short_name(port));
    }
}

fn monitor(targets: Vec<Target>) -> Result<()> {
    let mut open = targets.len();
    let mut dashboards = BTreeSet::new();
    for update in port::watch(targets).iter() {
        match update.event {
            Event::Item(Item::Message(Message::Snapshot(s))) => render::snapshot(&update.port, &s),
            Event::Item(Item::Message(Message::Log(line))) => render::log(&update.port, &line),
            Event::Item(Item::Message(Message::Hello(hello))) => {
                if hello.role == Role::Dashboard {
                    note_dashboard(&mut dashboards, &update.port);
                }
            }
            Event::Item(Item::Text(text)) => render::text(&update.port, &text),
            Event::Closed(reason) => {
                eprintln!("{}: {reason}", update.port);
                open -= 1;
                if open == 0 {
                    bail!("all devices are gone");
                }
            }
        }
    }
    Ok(())
}

/// Opens the log file for `log` and `dashboard`.
fn open_log(output: &std::path::Path, truncate: bool) -> Result<std::fs::File> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(output)
        .with_context(|| format!("opening {}", output.display()))
}

fn log(targets: Vec<Target>, mut sinks: Sinks, gps: Option<gps::Gps>) -> Result<()> {
    if sinks.is_empty() {
        bail!("nothing to record to: drop --no-gpkg, or give --output FILE");
    }

    let mut open = targets.len();
    let mut dashboards = BTreeSet::new();
    for update in port::watch(targets).iter() {
        match update.event {
            Event::Item(Item::Message(Message::Snapshot(s))) => sinks.record(&update.port, &s, gps.as_ref())?,
            Event::Item(Item::Message(Message::Log(line))) => render::log(&update.port, &line),
            Event::Item(Item::Message(Message::Hello(hello))) => {
                if hello.role == Role::Dashboard {
                    note_dashboard(&mut dashboards, &update.port);
                }
            }
            Event::Item(Item::Text(_)) => {}
            Event::Closed(reason) => {
                eprintln!("{}: {reason}", update.port);
                open -= 1;
                if open == 0 {
                    bail!("all devices are gone");
                }
            }
        }
    }
    Ok(())
}

/// How often the GPS position is sent to a dashboard.
const POSITION_PERIOD: Duration = Duration::from_secs(1);

fn dashboard(targets: Vec<Target>, mut sinks: Sinks, gps: Option<gps::Gps>) -> Result<()> {
    let mut open = targets.len();
    // Dashboards announce themselves, so which ports they are is learned as they answer.
    let mut dashboards: BTreeSet<String> = BTreeSet::new();
    let mut latest: BTreeMap<String, Snapshot> = BTreeMap::new();
    let mut last_position: Option<Instant> = None;

    let watch = port::watch(targets);
    let source = |port: &str, s: &Snapshot| {
        DeviceCommand::Feed(Feed::Source(Source { name: port::short_name(port).to_string(), snapshot: s.clone() }))
    };
    let position = || DeviceCommand::Feed(Feed::Position(gps.as_ref().map(|g| Position::from(&g.reading()))));

    loop {
        // The position is sent on a timer, so the dashboard shows it even when nothing else is happening.
        if last_position.is_none_or(|t| t.elapsed() >= POSITION_PERIOD) {
            for dashboard in &dashboards {
                watch.send(dashboard, &position());
            }
            last_position = Some(Instant::now());
        }

        let update = match watch.recv_timeout(Duration::from_millis(500)) {
            Ok(update) => update,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => bail!("all devices are gone"),
        };
        match update.event {
            Event::Item(Item::Message(Message::Snapshot(s))) => {
                for dashboard in &dashboards {
                    watch.send(dashboard, &source(&update.port, &s));
                }
                if !sinks.is_empty() {
                    sinks.record(&update.port, &s, gps.as_ref())?;
                } else {
                    eprintln!(
                        "{} {} -> {} dashboard{}",
                        render::label(&update.port, Some(&s)),
                        render::summary(&s),
                        dashboards.len(),
                        if dashboards.len() == 1 { "" } else { "s" }
                    );
                }
                latest.insert(update.port, s);
            }
            Event::Item(Item::Message(Message::Hello(hello))) => {
                // A dashboard that has just been seen knows nothing yet, so catch it up.
                if hello.role == Role::Dashboard && dashboards.insert(update.port.clone()) {
                    eprintln!("{} is a dashboard: sending it what the scanners hear", port::short_name(&update.port));
                    for (port, s) in &latest {
                        watch.send(&update.port, &source(port, s));
                    }
                    watch.send(&update.port, &position());
                }
            }
            Event::Item(Item::Message(Message::Log(line))) => render::log(&update.port, &line),
            Event::Item(Item::Text(_)) => {}
            Event::Closed(reason) => {
                eprintln!("{}: {reason}", update.port);
                dashboards.remove(&update.port);
                open -= 1;
                if open == 0 {
                    bail!("all devices are gone");
                }
            }
        }
    }
}

fn snapshot(targets: Vec<Target>, json: bool) -> Result<()> {
    for (port, s) in gather(targets, |_, _| true)?.scanners {
        if json {
            println!("{}", serde_json::to_string_pretty(&record::Record::new(&port, &s, None)?)?);
        } else {
            render::snapshot(&port, &s);
        }
    }
    Ok(())
}
