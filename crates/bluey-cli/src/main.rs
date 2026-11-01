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
use clap::{Parser, Subcommand, ValueEnum};

use crate::port::{Event, Target};

/// Reads what bluey devices hear: Bluetooth LE devices or WiFi access points.
///
/// A device scans for one of the two at a time. With several devices plugged in, every
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

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    /// Bluetooth LE devices
    Ble,
    /// WiFi access points
    Wifi,
}

impl From<ModeArg> for Mode {
    fn from(mode: ModeArg) -> Self {
        match mode {
            ModeArg::Ble => Mode::Ble,
            ModeArg::Wifi => Mode::Wifi,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// List serial ports.
    Ports,
    /// Show each device and what it's doing.
    Devices,
    /// Switch devices to scan for Bluetooth LE or WiFi.
    Mode {
        mode: ModeArg,
    },
    /// Show what the devices hear, as they hear it.
    Monitor,
    /// Append each snapshot from the devices to a file, one JSON object per line.
    Log {
        /// File to write to. It is created if it doesn't exist.
        #[arg(short, long)]
        output: PathBuf,
        /// Replace the file instead of adding to it.
        #[arg(long)]
        truncate: bool,
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
        Command::Mode { mode } => set_mode(port::resolve(&cli.ports)?, mode.into()),
        Command::Monitor => monitor(port::resolve(&cli.ports)?),
        Command::Log { output, truncate } => log(port::resolve(&cli.ports)?, &output, truncate),
        Command::Snapshot { json } => snapshot(port::resolve(&cli.ports)?, json),
    }
}

/// Waits until every target has sent a snapshot that `wanted` accepts, or the time is up.
/// Returns the first such snapshot from each device, ordered by port.
fn gather(targets: Vec<Target>, wanted: impl Fn(&Target, &Snapshot) -> bool) -> Result<BTreeMap<String, Snapshot>> {
    let by_port: BTreeMap<_, _> = targets.iter().map(|t| (t.port.clone(), t.clone())).collect();
    let updates = port::watch(targets);
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    let mut found = BTreeMap::new();
    let mut failures = Vec::new();

    while found.len() + failures.len() < by_port.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let update = match updates.recv_timeout(remaining) {
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
        let heard = match s.mode {
            Mode::Ble => format!("{} BLE devices", s.ble.len()),
            Mode::Wifi => format!("{} WiFi access points", s.wifi.len()),
        };
        println!("{port}  {}  {}  up {}s  {heard}", Bssid(&s.device), s.mode.label(), s.uptime_ms / 1000);
    }
    Ok(())
}

fn set_mode(targets: Vec<Target>, mode: Mode) -> Result<()> {
    let targets = targets.into_iter().map(|t| Target { mode: Some(mode), ..t }).collect();
    // An older periodic snapshot can arrive before the switch has happened.
    for (port, s) in gather(targets, |_, s| s.mode == mode)? {
        println!("{port}  {}  now scanning for {}", Bssid(&s.device), mode.label());
    }
    Ok(())
}

fn monitor(targets: Vec<Target>) -> Result<()> {
    let mut open = targets.len();
    for update in port::watch(targets) {
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

fn log(targets: Vec<Target>, output: &PathBuf, truncate: bool) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(output)
        .with_context(|| format!("opening {}", output.display()))?;

    let mut open = targets.len();
    let mut written = 0u64;
    for update in port::watch(targets) {
        match update.event {
            Event::Item(Item::Message(Message::Snapshot(s))) => {
                let line = serde_json::to_string(&record::Record::new(&update.port, &s)?)?;
                writeln!(file, "{line}")?;
                file.flush()?;
                written += 1;
                eprintln!(
                    "{} snapshot {written}: {} -> {}",
                    render::label(&update.port, Some(&s)),
                    match s.mode {
                        Mode::Ble => format!("{} BLE devices", s.ble.len()),
                        Mode::Wifi => format!("{} WiFi access points", s.wifi.len()),
                    },
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
            println!("{}", serde_json::to_string_pretty(&record::Record::new(&port, &s)?)?);
        } else {
            render::snapshot(&port, &s);
        }
    }
    Ok(())
}
