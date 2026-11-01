//! The web page and the JSON behind it, served on port 80.
//!
//! | Path         | What                                               |
//! |--------------|----------------------------------------------------|
//! | `/`          | The dashboard page                                 |
//! | `/api/state` | Everything the dashboard knows, as JSON            |
//!
//! The paths phones and computers use to check for internet (`/generate_204`,
//! `/hotspot-detect.html`, ...) redirect to `/`, which makes them offer to open the
//! dashboard as a "sign in to the network" page.

use alloc::string::String;
use alloc::vec::Vec;

use bluey_core::json::{self, PositionView, SourceView};
use embassy_net::Stack;
use embassy_time::Instant;
use picoserve::io::Write;
use picoserve::response::fs::File;
use picoserve::response::{Content, Redirect};
use picoserve::routing::{PathRouter, get, get_service};
use picoserve::{Config, Router};

use crate::state::Dashboard;
use crate::wifi::SSID;

const PORT: u16 = 80;

/// Number of connections served at the same time. Phones open several.
pub const WEB_TASKS: usize = 3;

/// JSON, as a response.
struct JsonText(String);

impl Content for JsonText {
    fn content_type(&self) -> &'static str {
        "application/json; charset=utf-8"
    }

    fn content_length(&self) -> usize {
        self.0.len()
    }

    async fn write_content<W: Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.0.as_bytes()).await
    }
}

/// Everything the dashboard knows, as JSON.
fn state_json(dashboard: &Dashboard) -> String {
    let sources: Vec<SourceView<'_>> = dashboard
        .sources
        .iter()
        .map(|entry| SourceView {
            name: &entry.source.name,
            snapshot: &entry.source.snapshot,
            age_s: entry.received.elapsed().as_secs(),
        })
        .collect();
    let position = dashboard.position.as_ref().map(|entry| PositionView {
        position: &entry.position,
        age_s: entry.received.elapsed().as_secs(),
    });
    json::state(SSID, Instant::now().as_secs(), dashboard.clients, esp_alloc::HEAP.free(), position, &sources)
}

async fn state() -> ([(&'static str, &'static str); 1], JsonText) {
    // The page asks again every couple of seconds, so never reuse an answer.
    ([("Cache-Control", "no-store")], JsonText(state_json(&crate::state::get())))
}

/// Sends a phone's "is there internet" check, whatever it asks for, to the dashboard.
async fn portal() -> Redirect {
    Redirect::to("http://192.168.4.1/")
}

fn app() -> Router<impl PathRouter> {
    Router::new()
        .route("/", get_service(File::html(include_str!("../index.html"))))
        .route("/api/state", get(state))
        // Android and Chrome
        .route("/generate_204", get(portal))
        .route("/gen_204", get(portal))
        // iOS and macOS
        .route("/hotspot-detect.html", get(portal))
        .route("/library/test/success.html", get(portal))
        // Windows
        .route("/connecttest.txt", get(portal))
        .route("/ncsi.txt", get(portal))
        .route("/redirect", get(portal))
        // Firefox
        .route("/canonical.html", get(portal))
        .route("/success.txt", get(portal))
}

#[embassy_executor::task(pool_size = WEB_TASKS)]
pub async fn web_task(id: usize, stack: Stack<'static>) -> ! {
    let app = app();
    let config = Config::const_default();

    let mut tcp_rx = [0u8; 1024];
    let mut tcp_tx = [0u8; 4096];
    let mut http_buf = [0u8; 2048];

    picoserve::Server::new(&app, &config, &mut http_buf)
        .listen_and_serve(id, stack, PORT, &mut tcp_rx, &mut tcp_tx)
        .await
        .into_never()
}
