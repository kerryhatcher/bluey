//! Putting messages on the serial port and getting them back off, in both directions:
//! [`Message`](crate::proto::Message)s from the device, and [`Command`](crate::proto::Command)s
//! and [`Feed`](crate::proto::Feed)s to it.
//!
//! The device shares the port with its bootloader and panic handler, which print plain text.
//! Each message is therefore a postcard encoding wrapped in COBS, which contains no zero
//! bytes, with a zero byte before and after. A reader splits the stream on zeros, decodes the
//! chunks that are messages and treats the rest as text.

use alloc::string::String;
use alloc::vec::Vec;

use core::marker::PhantomData;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Chunks longer than this are given up on and handed over as text.
const MAX_CHUNK: usize = 64 * 1024;

/// Encodes a message as a frame, ready to be written to the port.
pub fn encode<T: Serialize>(message: &T) -> Vec<u8> {
    // The leading zero ends any text that came before the frame.
    let mut frame = Vec::from([0]);
    // This can only fail if the message can't be serialized, which none of ours can.
    frame.extend(postcard::to_allocvec_cobs(message).unwrap_or_default());
    frame
}

/// What came out of the byte stream.
#[derive(Debug, PartialEq)]
pub enum Item<T> {
    Message(T),
    /// Bytes that weren't a message: bootloader output, panics, or a corrupted frame.
    Text(String),
}

/// Reassembles messages from bytes read from the port in arbitrary pieces.
pub struct Decoder<T> {
    chunk: Vec<u8>,
    message: PhantomData<T>,
}

impl<T: DeserializeOwned> Default for Decoder<T> {
    fn default() -> Self {
        Self { chunk: Vec::new(), message: PhantomData }
    }
}

impl<T: DeserializeOwned> Decoder<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds in bytes and returns everything that is now complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Item<T>> {
        let mut items = Vec::new();
        for &byte in bytes {
            if byte != 0 {
                self.chunk.push(byte);
                if self.chunk.len() > MAX_CHUNK {
                    items.extend(self.finish_chunk());
                }
            } else {
                items.extend(self.finish_chunk());
            }
        }
        items
    }

    fn finish_chunk(&mut self) -> Option<Item<T>> {
        if self.chunk.is_empty() {
            return None;
        }
        let mut chunk = core::mem::take(&mut self.chunk);
        // Decoding works in place, so keep a copy to fall back on.
        let text = chunk.clone();
        match postcard::from_bytes_cobs::<T>(&mut chunk) {
            Ok(message) => Some(Item::Message(message)),
            Err(_) => Some(Item::Text(String::from_utf8_lossy(&text).into_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;
    use crate::proto::{Command, LogLevel, LogLine, Message, Mode};

    fn log(text: &str) -> Message {
        Message::Log(LogLine { level: LogLevel::Info, text: text.to_string() })
    }

    #[test]
    fn round_trip() {
        let message = log("hello");
        let items = Decoder::<Message>::new().push(&encode(&message));
        assert_eq!(items, vec![Item::Message(message)]);
    }

    #[test]
    fn frames_split_across_reads() {
        let (a, b) = (log("one"), log("two"));
        let mut bytes = encode(&a);
        bytes.extend(encode(&b));

        let mut decoder = Decoder::<Message>::new();
        let mut items = Vec::new();
        for byte in bytes {
            items.extend(decoder.push(&[byte]));
        }
        assert_eq!(items, vec![Item::Message(a), Item::Message(b)]);
    }

    #[test]
    fn text_before_a_frame_is_kept_apart() {
        let message = log("hello");
        let mut bytes = b"ESP-ROM:esp32c6 boot\r\n".to_vec();
        bytes.extend(encode(&message));

        let items = Decoder::<Message>::new().push(&bytes);
        assert_eq!(
            items,
            vec![Item::Text("ESP-ROM:esp32c6 boot\r\n".to_string()), Item::Message(message)]
        );
    }

    #[test]
    fn feeds_round_trip() {
        use crate::proto::{Feed, Position};
        let feed = Feed::Position(Some(Position {
            fix: true,
            satellites: Some(6),
            latitude: Some(32.5),
            longitude: Some(-83.25),
            altitude_m: Some(140.0),
            hdop: Some(1.4),
            speed_mps: None,
            course_deg: None,
        }));
        // Feeds travel inside commands, so a device needs only one decoder.
        let command = Command::Feed(feed);
        let items = Decoder::<Command>::new().push(&encode(&command));
        assert_eq!(items, vec![Item::Message(command)]);
    }

    #[test]
    fn hello_round_trip() {
        use crate::proto::{Hello, PROTOCOL_VERSION, Role};
        let hello = Message::Hello(Hello { protocol: PROTOCOL_VERSION, role: Role::Dashboard, device: [1, 2, 3, 4, 5, 6] });
        let items = Decoder::<Message>::new().push(&encode(&hello));
        assert_eq!(items, vec![Item::Message(hello)]);
    }

    #[test]
    fn commands_go_the_other_way() {
        let command = Command::SetMode(Mode::Wifi);
        let items = Decoder::<Command>::new().push(&encode(&command));
        assert_eq!(items, vec![Item::Message(command)]);
    }
}
