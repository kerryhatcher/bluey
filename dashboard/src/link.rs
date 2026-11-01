//! The USB serial link to the CLI, which sends what the scanners have heard.

use bluey_core::framing::{Decoder, Item};
use bluey_core::proto::{Command, Hello, Message, PROTOCOL_VERSION, Role};
use embassy_time::{Duration, Timer};
use embedded_io_async::Read;
use esp_hal::Async;
use esp_hal::usb::usb_serial_jtag::UsbSerialJtagRx;
use log::warn;

use crate::{output, state};

/// How often the CLI is reminded what this is, in case it started listening late.
const HELLO_PERIOD: Duration = Duration::from_secs(10);

fn hello() -> Message {
    let device = <[u8; 6]>::try_from(esp_hal::efuse::base_mac_address().as_bytes()).unwrap_or_default();
    Message::Hello(Hello { protocol: PROTOCOL_VERSION, role: Role::Dashboard, device })
}

/// Reads what the CLI sends and acts on it.
#[embassy_executor::task]
pub async fn receive(mut rx: UsbSerialJtagRx<'static, Async>) {
    let mut decoder = Decoder::<Command>::new();
    let mut buf = [0u8; 256];
    loop {
        let n = match rx.read(&mut buf).await {
            Ok(n) => n,
            Err(_) => {
                Timer::after(Duration::from_millis(100)).await;
                continue;
            }
        };
        for item in decoder.push(&buf[..n]) {
            // Bootloader output and the like isn't for us.
            let Item::Message(command) = item else { continue };
            match command {
                Command::Feed(feed) => state::apply(feed),
                // The CLI asks every device what it is. Scanners answer with a snapshot,
                // this answers who it is, so the CLI knows not to expect one.
                Command::Snapshot => output::send(&hello()),
                Command::SetMode(_) => {
                    warn!("this is a dashboard, it doesn't scan");
                    output::send(&hello());
                }
            }
        }
    }
}

/// Says what this is every so often.
#[embassy_executor::task]
pub async fn announce() {
    loop {
        output::send(&hello());
        Timer::after(HELLO_PERIOD).await;
    }
}
