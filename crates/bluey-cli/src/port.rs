//! Finding and reading the device's serial port.

use std::io::{ErrorKind, Read};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bluey_core::framing::{Decoder, Item};
use serialport::{SerialPortInfo, SerialPortType};

/// USB vendor ID of Espressif, whose USB serial/JTAG controller the ESP32-C6 uses.
const ESPRESSIF_VID: u16 = 0x303a;

/// The speed doesn't matter for USB serial, but the port needs one.
const BAUD: u32 = 115_200;

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

fn find() -> Result<String> {
    let ports: Vec<_> = serialport::available_ports()
        .context("listing serial ports")?
        .into_iter()
        .filter(is_espressif)
        .collect();
    match ports.as_slice() {
        [port] => Ok(port.port_name.clone()),
        [] => bail!("no ESP32 found; is the device plugged in? (`bluey ports` lists what is, `--port` picks one)"),
        many => {
            let names: Vec<_> = many.iter().map(|p| p.port_name.as_str()).collect();
            bail!("more than one ESP32 found ({}); pick one with --port", names.join(", "))
        }
    }
}

/// Reads from the device, calling `on_item` for everything it sends, until `on_item` returns
/// `false`, the optional `timeout` passes, or the device goes away.
pub fn stream(port: Option<&str>, timeout: Option<Duration>, mut on_item: impl FnMut(Item) -> Result<bool>) -> Result<()> {
    let name = match port {
        Some(name) => name.to_string(),
        None => find()?,
    };
    let mut port = serialport::new(&name, BAUD)
        .timeout(Duration::from_millis(250))
        .open()
        .with_context(|| format!("opening {name}"))?;
    eprintln!("reading from {name}");

    let deadline = timeout.map(|t| Instant::now() + t);
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 4096];
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(());
        }
        let n = match port.read(&mut buf) {
            Ok(0) => bail!("{name} closed"),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::TimedOut => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {name}")),
        };
        for item in decoder.push(&buf[..n]) {
            if !on_item(item)? {
                return Ok(());
            }
        }
    }
}
