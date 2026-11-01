//! A `log` backend that sends log lines to the host as messages.

use alloc::format;
use bluey_core::proto::{LogLevel, LogLine, Message};
use log::{LevelFilter, Log, Metadata, Record};

use crate::output;

struct Logger;

static LOGGER: Logger = Logger;

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= LevelFilter::Info
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => LogLevel::Error,
            log::Level::Warn => LogLevel::Warn,
            log::Level::Info => LogLevel::Info,
            log::Level::Debug => LogLevel::Debug,
            log::Level::Trace => LogLevel::Trace,
        };
        let text = format!("{}: {}", record.target(), record.args());
        output::send(&Message::Log(LogLine { level, text }));
    }

    fn flush(&self) {}
}

/// Sends log lines to the host. The heap must be set up first, as every line is allocated.
pub fn init() {
    // Fails only if a logger is already set, in which case there is nothing to do.
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(LevelFilter::Info);
}
