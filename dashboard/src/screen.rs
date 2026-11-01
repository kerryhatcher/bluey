//! What is drawn on the 1.14" display, which is 135 wide and 240 tall.

use core::fmt::Write;

use bluey_core::proto::Mode;
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_10X20};
use embedded_graphics::mono_font::{MonoTextStyle, MonoTextStyleBuilder};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use heapless::String;

use crate::state::Dashboard;
use crate::wifi::{AP_IP, SSID};

/// Characters of the small font that fit across the display, with a margin.
const COLUMNS: usize = 21;
const MARGIN: i32 = 3;

/// A scanner that hasn't reported for this long is shown in red.
const STALE_SECS: u64 = 30;

/// Most scanners listed. They fit between the position and the bottom of the display.
const MAX_ROWS: usize = 7;

const GRAY: Rgb565 = Rgb565::new(16, 32, 16);

/// Draws one line of small text, padded so it covers what was there before.
fn line<D: DrawTarget<Color = Rgb565>>(display: &mut D, y: i32, color: Rgb565, text: &str) -> Result<(), D::Error> {
    let style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(color)
        .background_color(Rgb565::BLACK)
        .build();
    let mut padded: String<32> = String::new();
    let _ = write!(padded, "{text:<COLUMNS$.COLUMNS$}");
    Text::new(&padded, Point::new(MARGIN, y), style).draw(display)?;
    Ok(())
}

fn rule<D: DrawTarget<Color = Rgb565>>(display: &mut D, y: i32) -> Result<(), D::Error> {
    Rectangle::new(Point::new(0, y), Size::new(135, 1))
        .into_styled(PrimitiveStyle::with_fill(GRAY))
        .draw(display)
}

fn mode_color(mode: Mode) -> Rgb565 {
    match mode {
        Mode::Ble => Rgb565::new(10, 30, 31),
        Mode::Wifi => Rgb565::GREEN,
        Mode::Ieee802154 => Rgb565::MAGENTA,
    }
}

pub fn draw<D: DrawTarget<Color = Rgb565>>(display: &mut D, dashboard: &Dashboard) -> Result<(), D::Error> {
    let title = MonoTextStyle::new(&FONT_10X20, Rgb565::CYAN);
    Text::new("BLUEY", Point::new(MARGIN, 22), title).draw(display)?;

    let mut text: String<32> = String::new();
    let _ = write!(text, "wifi {SSID}");
    line(display, 40, Rgb565::WHITE, &text)?;
    text.clear();
    let _ = write!(text, "http://{AP_IP}");
    line(display, 52, Rgb565::WHITE, &text)?;
    text.clear();
    let _ = write!(text, "connected: {}", dashboard.clients);
    line(display, 64, if dashboard.clients > 0 { Rgb565::GREEN } else { GRAY }, &text)?;
    rule(display, 72)?;

    // Where we are. The GPS is on the computer, so this is stale if the CLI stops.
    match &dashboard.position {
        Some(entry) if entry.received.elapsed().as_secs() < STALE_SECS => {
            let p = &entry.position;
            text.clear();
            let sats = p.satellites.unwrap_or(0);
            let _ = write!(text, "GPS {} {sats} sat", if p.fix { "fix" } else { "no fix" });
            line(display, 88, if p.fix { Rgb565::GREEN } else { Rgb565::YELLOW }, &text)?;
            text.clear();
            if let (true, Some(lat), Some(lon)) = (p.fix, p.latitude, p.longitude) {
                let _ = write!(text, "{lat:.5} {lon:.5}");
            }
            line(display, 100, Rgb565::WHITE, &text)?;
            text.clear();
            if let (true, Some(alt)) = (p.fix, p.altitude_m) {
                let _ = write!(text, "alt {alt:.0}m");
                if let Some(hdop) = p.hdop {
                    let _ = write!(text, " hdop {hdop:.1}");
                }
            }
            line(display, 112, GRAY, &text)?;
        }
        _ => {
            line(display, 88, GRAY, "GPS none")?;
            line(display, 100, GRAY, "")?;
            line(display, 112, GRAY, "")?;
        }
    }
    rule(display, 120)?;

    text.clear();
    let _ = write!(text, "SCANNERS ({})", dashboard.sources.len());
    line(display, 134, Rgb565::CYAN, &text)?;

    for row in 0..MAX_ROWS {
        let y = 148 + 12 * row as i32;
        match dashboard.sources.get(row) {
            Some(entry) => {
                let s = &entry.source.snapshot;
                let heard = match s.mode {
                    Mode::Ble => s.ble.len(),
                    Mode::Wifi => s.wifi.len(),
                    Mode::Ieee802154 => s.ieee802154.len(),
                };
                // "ttyACM0" is mostly "tty", which is no use here.
                let name = entry.source.name.strip_prefix("tty").unwrap_or(&entry.source.name);
                let age = entry.received.elapsed().as_secs();
                text.clear();
                let _ = write!(text, "{name:<6.6}{:<7}{heard:>3}{age:>4}s", s.mode.label());
                let color = if age > STALE_SECS { Rgb565::RED } else { mode_color(s.mode) };
                line(display, y, color, &text)?;
            }
            None => line(display, y, GRAY, if row == 0 { "waiting for the CLI" } else { "" })?,
        }
    }
    rule(display, 232)?;
    Ok(())
}
