//! Finding and talking to the devices' serial ports.

use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use bluey_core::framing::{self, Decoder, Item};
use bluey_core::proto::{Command, Message, Mode};
use serialport::{SerialPortInfo, SerialPortType};

/// USB vendor ID of Espressif, whose USB serial/JTAG controller the ESP32-C6 uses.
const ESPRESSIF_VID: u16 = 0x303a;

/// The speed doesn't matter for USB serial, but the port needs one.
const BAUD: u32 = 115_200;

/// A device to talk to, and the mode to put it in first, if any.
#[derive(Clone, Debug)]
pub struct Target {
    pub port: String,
    pub mode: Option<Mode>,
}

/// Parses `PORT` or `PORT:MODE`, as in `/dev/ttyACM0:wifi`.
pub fn parse_target(s: &str) -> Result<Target, String> {
    let Some((port, mode)) = s.rsplit_once(':') else {
        return Ok(Target { port: s.to_string(), mode: None });
    };
    let mode = match mode {
        "ble" => Mode::Ble,
        "wifi" => Mode::Wifi,
        other => return Err(format!("unknown mode {other:?}, expected ble or wifi")),
    };
    Ok(Target { port: port.to_string(), mode: Some(mode) })
}

fn is_espressif(port: &SerialPortInfo) -> bool {
    matches!(&port.port_type, SerialPortType::UsbPort(usb) if usb.vid == ESPRESSIF_VID)
}

pub fn list() -> Result<()> {
    // Plain serial ports (/dev/ttyS*) are on every PC and never the device.
    let ports: Vec<_> = serialport::available_ports()
        .context("listing serial ports")?
        .into_iter()
        .filter(|p| p.port_type != SerialPortType::Unknown)
        .collect();
    if ports.is_empty() {
        println!("no serial ports found");
    }
    for port in ports {
        let detail = match &port.port_type {
            SerialPortType::UsbPort(usb) => format!(
                "USB {:04x}:{:04x} {}",
                usb.vid,
                usb.pid,
                usb.product.as_deref().unwrap_or("")
            ),
            SerialPortType::PciPort => "PCI".to_string(),
            SerialPortType::BluetoothPort => "Bluetooth".to_string(),
            SerialPortType::Unknown => String::new(),
        };
        let marker = if is_espressif(&port) { "  <- ESP32" } else { "" };
        println!("{}  {}{}", port.port_name, detail.trim_end(), marker);
    }
    Ok(())
}

/// The targets to use: the ones asked for, or every ESP32 that is plugged in.
pub fn resolve(asked_for: &[Target]) -> Result<Vec<Target>> {
    if !asked_for.is_empty() {
        return Ok(asked_for.to_vec());
    }
    let mut targets: Vec<_> = serialport::available_ports()
        .context("listing serial ports")?
        .into_iter()
        .filter(is_espressif)
        .map(|p| Target { port: p.port_name, mode: None })
        .collect();
    targets.sort_by(|a, b| a.port.cmp(&b.port));
    if targets.is_empty() {
        bail!("no ESP32 found; is the device plugged in? (`bluey ports` lists what is, `--port` picks one)");
    }
    Ok(targets)
}

/// A short name for a port, for labelling output.
pub fn short_name(port: &str) -> &str {
    Path::new(port).file_name().and_then(|n| n.to_str()).unwrap_or(port)
}

pub enum Event {
    Item(Item<Message>),
    /// The port couldn't be opened, or the device went away. Nothing more will come from it.
    Closed(String),
}

pub struct Update {
    pub port: String,
    pub event: Event,
}

/// Starts reading from every target, each on its own thread, and returns what they send.
///
/// The receiver ends once every device has gone away or been dropped.
pub fn watch(targets: Vec<Target>) -> Receiver<Update> {
    let (tx, rx) = mpsc::channel();
    for target in targets {
        let tx = tx.clone();
        thread::spawn(move || {
            let port = target.port.clone();
            let result = read_loop(&target, |item| {
                tx.send(Update { port: port.clone(), event: Event::Item(item) }).is_ok()
            });
            if let Err(e) = result {
                let _ = tx.send(Update { port, event: Event::Closed(format!("{e:#}")) });
            }
        });
    }
    rx
}

/// Opens the port, tells the device what we want, and passes everything it sends to `on_item`
/// until that returns `false`.
fn read_loop(target: &Target, mut on_item: impl FnMut(Item<Message>) -> bool) -> Result<()> {
    let name = &target.port;
    let mut port = serialport::new(name, BAUD)
        .timeout(Duration::from_millis(250))
        .open()
        .with_context(|| format!("opening {name}"))?;

    // Switching modes makes the device answer with a snapshot. Otherwise ask for one, so we
    // don't wait for the next periodic one.
    let command = target.mode.map_or(Command::Snapshot, Command::SetMode);
    port.write_all(&framing::encode(&command))
        .and_then(|()| port.flush())
        .with_context(|| format!("writing to {name}"))?;

    let mut decoder = Decoder::<Message>::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match port.read(&mut buf) {
            Ok(0) => return Err(anyhow!("{name} closed")),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::TimedOut => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {name}")),
        };
        for item in decoder.push(&buf[..n]) {
            if !on_item(item) {
                return Ok(());
            }
        }
    }
}
