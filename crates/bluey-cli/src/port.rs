//! Finding and talking to the devices' serial ports.

use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
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

/// Parses a mode name.
pub fn parse_mode(s: &str) -> Result<Mode, String> {
    Mode::from_label(s).ok_or_else(|| {
        let names: Vec<_> = Mode::ALL.iter().map(|m| m.label()).collect();
        format!("unknown mode {s:?}, expected one of: {}", names.join(", "))
    })
}

/// Parses `PORT` or `PORT:MODE`, as in `/dev/ttyACM0:wifi`.
pub fn parse_target(s: &str) -> Result<Target, String> {
    let Some((port, mode)) = s.rsplit_once(':') else {
        return Ok(Target { port: s.to_string(), mode: None });
    };
    Ok(Target { port: port.to_string(), mode: Some(parse_mode(mode)?) })
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

/// Readers for a set of devices, each on its own thread. Dropping it stops them and
/// closes the ports, so the devices can be opened again.
pub struct Watch {
    updates: Receiver<Update>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Watch {
    /// Waits for the next update. `Disconnected` means every device has gone away.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Update, RecvTimeoutError> {
        self.updates.recv_timeout(timeout)
    }

    /// Everything the devices send, until they have all gone away.
    pub fn iter(&self) -> impl Iterator<Item = Update> + '_ {
        self.updates.iter()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Starts reading from every target.
pub fn watch(targets: Vec<Target>) -> Watch {
    let (tx, updates) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let threads = targets
        .into_iter()
        .map(|target| {
            let tx = tx.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                let port = target.port.clone();
                let result = read_loop(&target, &stop, |item| {
                    tx.send(Update { port: port.clone(), event: Event::Item(item) }).is_ok()
                });
                if let Err(e) = result {
                    let _ = tx.send(Update { port, event: Event::Closed(format!("{e:#}")) });
                }
            })
        })
        .collect();
    Watch { updates, stop, threads }
}

/// Opens the port, tells the device what we want, and passes everything it sends to `on_item`
/// until that returns `false`.
fn read_loop(target: &Target, stop: &AtomicBool, mut on_item: impl FnMut(Item<Message>) -> bool) -> Result<()> {
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
    while !stop.load(Ordering::Relaxed) {
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
    Ok(())
}
