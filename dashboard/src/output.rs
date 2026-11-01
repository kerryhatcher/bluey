//! Sending messages to the host over the USB serial port.
//!
//! Messages are queued and written by a task in small pieces. Writing takes a critical section,
//! and a whole snapshot in one go would hold off the Bluetooth and WiFi interrupts for too long.

use alloc::vec::Vec;

use bluey_core::framing;
use bluey_core::proto::Message;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use esp_println::Printer;

/// Bytes written per critical section.
const CHUNK: usize = 64;

static QUEUE: Channel<CriticalSectionRawMutex, Vec<u8>, 8> = Channel::new();

/// Queues a message for sending. If the queue is full the message is dropped.
pub fn send(message: &Message) {
    let _ = QUEUE.try_send(framing::encode(message));
}

/// Writes queued messages to the port. Never returns.
pub async fn run() -> ! {
    loop {
        let frame = QUEUE.receive().await;
        for chunk in frame.chunks(CHUNK) {
            Printer::write_bytes(chunk);
            embassy_futures::yield_now().await;
        }
    }
}

/// [`run`], as a task.
#[embassy_executor::task]
pub async fn task() {
    run().await
}
