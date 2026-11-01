//! What the dashboard has been told, shared between the screen, the web server and the link
//! to the CLI.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::RefCell;

use bluey_core::proto::{Feed, Position, Source};
use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Instant;

/// Most scanners tracked at once. A new one beyond this replaces the one heard longest ago.
const MAX_SOURCES: usize = 8;

/// A scanner's latest snapshot and when it arrived.
#[derive(Clone)]
pub struct SourceEntry {
    pub source: Arc<Source>,
    pub received: Instant,
}

#[derive(Clone)]
pub struct PositionEntry {
    pub position: Position,
    pub received: Instant,
}

/// Everything shown. Cloning is cheap, the snapshots are shared.
#[derive(Clone, Default)]
pub struct Dashboard {
    pub sources: Vec<SourceEntry>,
    pub position: Option<PositionEntry>,
    /// Phones and other stations connected to the access point.
    pub clients: u8,
}

static STATE: Mutex<RefCell<Dashboard>> = Mutex::new(RefCell::new(Dashboard { sources: Vec::new(), position: None, clients: 0 }));

/// Raised whenever the state changes, so the screen redraws at once.
pub static CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// A copy of the state, to read without holding the lock.
pub fn get() -> Dashboard {
    critical_section::with(|cs| STATE.borrow_ref(cs).clone())
}

/// Takes in something the CLI sent.
pub fn apply(feed: Feed) {
    let now = Instant::now();
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        match feed {
            Feed::Source(source) => {
                let entry = SourceEntry { source: Arc::new(source), received: now };
                // Scanners are told apart by chip, not by name: a port can change.
                let same = |e: &SourceEntry| e.source.snapshot.device == entry.source.snapshot.device;
                if let Some(existing) = state.sources.iter_mut().find(|e| same(e)) {
                    *existing = entry;
                } else {
                    if state.sources.len() >= MAX_SOURCES {
                        let oldest = (0..state.sources.len()).min_by_key(|&i| state.sources[i].received);
                        if let Some(oldest) = oldest {
                            state.sources.swap_remove(oldest);
                        }
                    }
                    state.sources.push(entry);
                    state.sources.sort_by(|a, b| a.source.name.cmp(&b.source.name));
                }
            }
            Feed::Position(position) => {
                state.position = position.map(|position| PositionEntry { position, received: now });
            }
        }
    });
    CHANGED.signal(());
}

pub fn set_clients(clients: u8) {
    critical_section::with(|cs| STATE.borrow_ref_mut(cs).clients = clients);
    CHANGED.signal(());
}
