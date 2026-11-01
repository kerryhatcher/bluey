mod port;
mod record;
mod render;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bluey_core::framing::Item;
use bluey_core::proto::{Message, Snapshot};
use clap::{Parser, Subcommand};

/// Reads what the bluey device has heard (Bluetooth LE devices and WiFi access points).
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Serial port of the device. Found automatically if there is only one ESP32.
    #[arg(short, long, global = true)]
    port: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List serial ports.
    Ports,
    /// Show what the device hears, as it hears it.
    Monitor,
    /// Append each snapshot from the device to a file, one JSON object per line.
    Log {
        /// File to write to. It is created if it doesn't exist.
        #[arg(short, long)]
        output: PathBuf,
        /// Replace the file instead of adding to it.
        #[arg(long)]
        truncate: bool,
    },
    /// Wait for the next snapshot, print it and exit.
    Snapshot {
        /// Print JSON instead of tables.
        #[arg(long)]
        json: bool,
        /// Give up after this many seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ports => port::list(),
        Command::Monitor => monitor(cli.port.as_deref()),
        Command::Log { output, truncate } => log(cli.port.as_deref(), &output, truncate),
        Command::Snapshot { json, timeout } => snapshot(cli.port.as_deref(), json, Duration::from_secs(timeout)),
    }
}

fn monitor(port: Option<&str>) -> Result<()> {
    port::stream(port, None, |item| {
        match item {
            Item::Message(Message::Snapshot(snapshot)) => render::snapshot(&snapshot),
            Item::Message(Message::Log(line)) => render::log(&line),
            Item::Text(text) => render::text(&text),
        }
        Ok(true)
    })
}

fn log(port: Option<&str>, output: &PathBuf, truncate: bool) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(output)
        .with_context(|| format!("opening {}", output.display()))?;

    let mut written = 0u64;
    port::stream(port, None, |item| {
        match item {
            Item::Message(Message::Snapshot(snapshot)) => {
                let line = serde_json::to_string(&record::Record::new(&snapshot)?)?;
                writeln!(file, "{line}")?;
                file.flush()?;
                written += 1;
                eprintln!(
                    "snapshot {written}: {} BLE devices, {} WiFi access points -> {}",
                    snapshot.ble.len(),
                    snapshot.wifi.len(),
                    output.display()
                );
            }
            Item::Message(Message::Log(line)) => render::log(&line),
            Item::Text(_) => {}
        }
        Ok(true)
    })
}

fn snapshot(port: Option<&str>, json: bool, timeout: Duration) -> Result<()> {
    let mut found: Option<Snapshot> = None;
    port::stream(port, Some(timeout), |item| {
        if let Item::Message(Message::Snapshot(snapshot)) = item {
            found = Some(snapshot);
            return Ok(false);
        }
        Ok(true)
    })?;

    let Some(snapshot) = found else {
        bail!("no snapshot within {} seconds; the device sends one every 10 seconds", timeout.as_secs());
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&record::Record::new(&snapshot)?)?);
    } else {
        render::snapshot(&snapshot);
    }
    Ok(())
}
