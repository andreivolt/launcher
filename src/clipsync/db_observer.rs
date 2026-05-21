//! Local-change detector.
//!
//! clip-sync never touches the Wayland clipboard for *reading* — clipd owns
//! that watcher. Instead this observes clipd's SQLite DB: it polls for rows
//! with an id higher than the last one seen and reports their content hashes,
//! which the transport layer then offers to the peer.
//!
//! Polling (rather than inotify) is deliberate: it is a couple of indexed
//! `SELECT`s every poll interval, trivially cheap, and immune to the WAL file
//! never changing mtime in a way `notify` reliably catches across platforms.

use crate::clip::db::ClipboardDb;
use std::time::Duration;

/// Default poll interval. Fast enough that a copy on one machine appears on the
/// other within a second or two; slow enough to be free.
pub const POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// Tracks the highest DB row id clip-sync has already accounted for, so each
/// poll yields only genuinely new entries.
pub struct DbObserver {
    last_seen_id: i64,
}

impl DbObserver {
    /// Start observing from the current end of the table.
    ///
    /// Rows that already exist at startup are handled by the initial full
    /// reconciliation, not by this observer — so the high-water mark begins at
    /// the present `MAX(id)` and only fires for strictly newer rows.
    pub fn new(db: &ClipboardDb) -> rusqlite::Result<Self> {
        Ok(Self {
            last_seen_id: db.max_id()?,
        })
    }

    /// Return the content hashes of rows appended since the last call, and
    /// advance the high-water mark past them.
    ///
    /// Returns an empty vec when nothing new has landed.
    pub fn poll(&mut self, db: &ClipboardDb) -> rusqlite::Result<Vec<i64>> {
        let current_max = db.max_id()?;
        if current_max <= self.last_seen_id {
            return Ok(Vec::new());
        }
        let hashes = db.hashes_after_id(self.last_seen_id)?;
        self.last_seen_id = current_max;
        Ok(hashes)
    }
}
