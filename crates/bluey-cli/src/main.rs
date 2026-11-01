mod gps;
mod port;
mod record;
mod render;

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bluey_core::framing::Item;
use bluey_core::proto::{Bssid, Message, Mode, Snapshot};
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
    /// Append each snapshot from the devices to a file, one JSON object per line.
    ///
    /// If a USB GPS is plugged in, each snapshot also gets its position.
    Log {
        /// File to write to. It is created if it doesn't exist.
        #[arg(short, long)]
        output: PathBuf,
        /// Replace the file instead of adding to it.
        #[arg(long)]
        truncate: bool,
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
        Command::Log { output, truncate, gps } => log(port::resolve(&cli.ports)?, &output, truncate, gps.open()?),
        Command::Snapshot { json } => snapshot(port::resolve(&cli.ports)?, json),
    }
}

/// Waits until every target has sent a snapshot that `wanted` accepts, or the time is up.
/// Returns the first such snapshot from each device, ordered by port.
fn gather(targets: Vec<Target>, wanted: impl Fn(&Target, &Snapshot) -> bool) -> Result<BTreeMap<String, Snapshot>> {
    let by_port: BTreeMap<_, _> = targets.iter().map(|t| (t.port.clone(), t.clone())).collect();
    let watch = port::watch(targets);
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    let mut found = BTreeMap::new();
    let mut failures = Vec::new();

    while found.len() + failures.len() < by_port.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let update = match watch.recv_timeout(remaining) {
            Ok(update) => update,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        };
        match update.event {
            Event::Item(Item::Message(Message::Snapshot(s))) => {
                if wanted(&by_port[&update.port], &s) {
                    found.entry(update.port).or_insert(s);
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
    if let Some(silent) = by_port.keys().find(|p| !found.contains_key(*p)) {
        bail!("no answer from {silent} within {} seconds", ANSWER_TIMEOUT.as_secs());
    }
    Ok(found)
}

fn devices(targets: Vec<Target>) -> Result<()> {
    for (port, s) in gather(targets, |_, _| true)? {
        let can: Vec<_> = s.supported.iter().map(|m| m.label()).collect();
        println!(
            "{port}  {}  {}  up {}s  {}  (can scan for: {})",
            Bssid(&s.device),
            s.mode.label(),
            s.uptime_ms / 1000,
            render::summary(&s),
            can.join(", ")
        );
    }
    Ok(())
}

fn set_mode(targets: Vec<Target>, mode: Mode) -> Result<()> {
    // Ask what each device can do first, so a device that can't is reported as such rather
    // than just never answering.
    let unsupported: Vec<_> = gather(targets.clone(), |_, _| true)?
        .into_iter()
        .filter(|(_, s)| !s.supported.contains(&mode))
        .map(|(port, s)| {
            let can: Vec<_> = s.supported.iter().map(|m| m.label()).collect();
            format!("{port} can only scan for {}; see the README for flashing the other firmware", can.join(" and "))
        })
        .collect();
    if !unsupported.is_empty() {
        bail!("{}", unsupported.join("\n"));
    }

    let targets = targets.into_iter().map(|t| Target { mode: Some(mode), ..t }).collect();
    // An older periodic snapshot can arrive before the switch has happened.
    for (port, s) in gather(targets, |_, s| s.mode == mode)? {
        println!("{port}  {}  now scanning for {}", Bssid(&s.device), mode.label());
    }
    Ok(())
}

fn monitor(targets: Vec<Target>) -> Result<()> {
    let mut open = targets.len();
    for update in port::watch(targets).iter() {
        match update.event {
            Event::Item(Item::Message(Message::Snapshot(s))) => render::snapshot(&update.port, &s),
            Event::Item(Item::Message(Message::Log(line))) => render::log(&update.port, &line),
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

fn log(targets: Vec<Target>, output: &PathBuf, truncate: bool, gps: Option<gps::Gps>) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(output)
        .with_context(|| format!("opening {}", output.display()))?;

    let mut open = targets.len();
    let mut written = 0u64;
    for update in port::watch(targets).iter() {
        match update.event {
            Event::Item(Item::Message(Message::Snapshot(s))) => {
                let reading = gps.as_ref().map(gps::Gps::reading);
                let line = serde_json::to_string(&record::Record::new(&update.port, &s, reading.clone())?)?;
                writeln!(file, "{line}")?;
                file.flush()?;
                written += 1;
                let position = reading.map(|r| format!("  [{}]", gps::describe(&r))).unwrap_or_default();
                eprintln!(
                    "{} snapshot {written}: {} -> {}{position}",
                    render::label(&update.port, Some(&s)),
                    render::summary(&s),
                    output.display()
                );
            }
            Event::Item(Item::Message(Message::Log(line))) => render::log(&update.port, &line),
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

fn snapshot(targets: Vec<Target>, json: bool) -> Result<()> {
    for (port, s) in gather(targets, |_, _| true)? {
        if json {
            println!("{}", serde_json::to_string_pretty(&record::Record::new(&port, &s, None)?)?);
        } else {
            render::snapshot(&port, &s);
        }
    }
    Ok(())
}
