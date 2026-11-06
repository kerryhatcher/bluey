//! `bluey tui`: a live table of the devices heard recently, strongest signal first.
//!
//! It reads the `observations` table of the GeoPackage that `bluey log` and `bluey poll` write
//! to, and only ever reads it. Each device is listed once, with the strongest signal it had in
//! the window and when it was last heard. The table refreshes every few seconds.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use jiff::{Timestamp, ToSpan};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};
use ratatui::{DefaultTerminal, Frame};
use rusqlite::{Connection, OpenFlags};

use crate::store;

/// How often the table is read again.
const REFRESH: Duration = Duration::from_secs(3);

/// One device seen in the window.
#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub kind: String,
    pub device: String,
    pub label: Option<String>,
    pub category: String,
    /// The strongest signal in the window, in dBm.
    pub best_rssi: i64,
    /// When it was last heard, as stored in the GeoPackage.
    pub last_heard: String,
    /// How many snapshots it was in.
    pub hits: i64,
}

/// Devices heard since `since` (a time as the GeoPackage stores it), strongest signal first.
pub fn contacts(conn: &Connection, since: &str) -> rusqlite::Result<Vec<Contact>> {
    let mut stmt = conn.prepare(
        "SELECT kind, device, MAX(label), MAX(category), MAX(rssi), MAX(time), COUNT(*) \
         FROM observations WHERE time >= ?1 \
         GROUP BY kind, device \
         ORDER BY MAX(rssi) DESC, device",
    )?;
    let rows = stmt.query_map([since], |r| {
        Ok(Contact {
            kind: r.get(0)?,
            device: r.get(1)?,
            label: r.get(2)?,
            category: r.get(3)?,
            best_rssi: r.get(4)?,
            last_heard: r.get(5)?,
            hits: r.get(6)?,
        })
    })?;
    rows.collect()
}

/// Opens the GeoPackage read-only, so the TUI never creates or changes it.
fn open(path: &Path) -> Result<Connection> {
    if !path.exists() {
        anyhow::bail!("no observations yet in {}: run `bluey log` or `bluey poll` first", path.display());
    }
    Ok(Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?)
}

struct App {
    contacts: Vec<Contact>,
    /// Why the last read failed, if it did.
    error: Option<String>,
    updated: Option<Timestamp>,
    table: TableState,
}

impl App {
    fn refresh(&mut self, path: &Path, minutes: u64) {
        // Capped at a year, so the subtraction can't overflow.
        let since = store::time(Timestamp::now() - (minutes.min(525_600) as i64).minutes());
        match open(path).and_then(|conn| Ok(contacts(&conn, &since)?)) {
            Ok(contacts) => {
                self.contacts = contacts;
                self.error = None;
                self.updated = Some(Timestamp::now());
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
        if self.table.selected().is_some_and(|i| i >= self.contacts.len()) {
            self.table.select(self.contacts.len().checked_sub(1));
        }
    }

    fn move_selection(&mut self, down: bool) {
        let len = self.contacts.len();
        if len == 0 {
            return;
        }
        let next = match self.table.selected() {
            None => 0,
            Some(i) if down => (i + 1).min(len - 1),
            Some(i) => i.saturating_sub(1),
        };
        self.table.select(Some(next));
    }
}

/// Runs the table until `q` is pressed. The terminal is restored even if something fails.
pub fn run(gpkg: Option<PathBuf>, minutes: u64) -> Result<()> {
    let path = match gpkg {
        Some(path) => path,
        None => store::default_path()?,
    };
    let mut app = App { contacts: Vec::new(), error: None, updated: None, table: TableState::default() };
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &path, minutes);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, path: &Path, minutes: u64) -> Result<()> {
    let mut last_read: Option<Instant> = None;
    loop {
        if last_read.is_none_or(|t| t.elapsed() >= REFRESH) {
            app.refresh(path, minutes);
            last_read = Some(Instant::now());
        }
        terminal.draw(|frame| draw(frame, app, path, minutes))?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                KeyCode::Char('r') => last_read = None,
                KeyCode::Down | KeyCode::Char('j') => app.move_selection(true),
                KeyCode::Up | KeyCode::Char('k') => app.move_selection(false),
                _ => {}
            }
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App, path: &Path, minutes: u64) {
    let [header, body, footer] = Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)])
        .areas(frame.area());

    let updated = app.updated.map_or("never".to_string(), |t| t.strftime("%H:%M:%S").to_string());
    let title = format!(" bluey: devices heard in the last {minutes} min ({} seen)   updated {updated}", app.contacts.len());
    frame.render_widget(Paragraph::new(title).style(Style::new().add_modifier(Modifier::BOLD)), header);

    let now = Timestamp::now();
    let rows = app.contacts.iter().map(|c| {
        let name = c.label.clone().unwrap_or_else(|| "-".to_string());
        let heard = c.last_heard.parse::<Timestamp>().map_or("?".to_string(), |t| age(now, t));
        Row::new([
            Cell::from(signal_bar(c.best_rssi)).style(Style::new().fg(signal_color(c.best_rssi))),
            Cell::from(c.kind.clone()),
            Cell::from(c.device.clone()),
            Cell::from(name),
            Cell::from(c.category.clone()),
            Cell::from(heard),
            Cell::from(c.hits.to_string()),
        ])
    });
    let widths = [
        Constraint::Length(19),
        Constraint::Length(6),
        Constraint::Length(24),
        Constraint::Fill(2),
        Constraint::Fill(2),
        Constraint::Length(9),
        Constraint::Length(6),
    ];
    let header_row = Row::new(["signal", "kind", "device", "name", "what", "last heard", "hits"])
        .style(Style::new().add_modifier(Modifier::BOLD));
    let table = Table::new(rows, widths)
        .header(header_row)
        .block(Block::bordered().title(path.display().to_string()))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(table, body, &mut app.table);

    let status = match &app.error {
        Some(e) => format!(" {e}"),
        None => " q quit · r refresh · up/down select".to_string(),
    };
    let style = if app.error.is_some() { Style::new().fg(Color::Red) } else { Style::new().add_modifier(Modifier::DIM) };
    frame.render_widget(Paragraph::new(status).style(style), footer);
}

/// A bar for the signal, from -100 dBm (empty) to -40 dBm (full), with the number after it.
fn signal_bar(rssi: i64) -> String {
    const WIDTH: i64 = 10;
    let filled = ((rssi + 100).clamp(0, 60) * WIDTH / 60) as usize;
    format!("{}{} {rssi:>4}", "█".repeat(filled), "░".repeat(WIDTH as usize - filled))
}

fn signal_color(rssi: i64) -> Color {
    match rssi {
        r if r >= -60 => Color::Green,
        r if r >= -80 => Color::Yellow,
        _ => Color::Red,
    }
}

/// How long ago something was heard, in the largest sensible unit.
fn age(now: Timestamp, then: Timestamp) -> String {
    let secs = (now.as_second() - then.as_second()).max(0);
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE observations (time TEXT NOT NULL, kind TEXT NOT NULL, device TEXT NOT NULL, \
             label TEXT, category TEXT NOT NULL, rssi INTEGER NOT NULL);",
        )
        .unwrap();
        conn
    }

    fn row(conn: &Connection, time: &str, kind: &str, device: &str, label: Option<&str>, rssi: i64) {
        conn.execute(
            "INSERT INTO observations VALUES (?1, ?2, ?3, ?4, 'other', ?5)",
            rusqlite::params![time, kind, device, label, rssi],
        )
        .unwrap();
    }

    #[test]
    fn strongest_first_and_only_the_window() {
        let conn = db();
        row(&conn, "2026-11-06T17:00:00.000Z", "ble", "AA", Some("Old"), -30); // outside the window
        row(&conn, "2026-11-06T17:20:00.000Z", "ble", "BB", None, -80);
        row(&conn, "2026-11-06T17:21:00.000Z", "ble", "BB", Some("Tag"), -70);
        row(&conn, "2026-11-06T17:22:00.000Z", "ble", "CC", None, -55);
        let got = contacts(&conn, "2026-11-06T17:10:00.000Z").unwrap();
        let devices: Vec<_> = got.iter().map(|c| (c.device.as_str(), c.best_rssi, c.hits)).collect();
        assert_eq!(devices, [("CC", -55, 1), ("BB", -70, 2)]);
        assert_eq!(got[1].label.as_deref(), Some("Tag"));
    }

    #[test]
    fn the_same_address_on_two_kinds_is_two_rows() {
        let conn = db();
        row(&conn, "2026-11-06T17:20:00.000Z", "ble", "AA", None, -50);
        row(&conn, "2026-11-06T17:20:00.000Z", "wifi", "AA", None, -40);
        assert_eq!(contacts(&conn, "2026-11-06T17:10:00.000Z").unwrap().len(), 2);
    }

    #[test]
    fn bar_and_age() {
        assert_eq!(signal_bar(-40), "██████████  -40");
        assert_eq!(signal_bar(-100), "░░░░░░░░░░ -100");
        let now: Timestamp = "2026-11-06T17:20:00Z".parse().unwrap();
        assert_eq!(age(now, "2026-11-06T17:19:30Z".parse().unwrap()), "30s ago");
        assert_eq!(age(now, "2026-11-06T17:05:00Z".parse().unwrap()), "15m ago");
    }
}
