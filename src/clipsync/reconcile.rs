//! Set-reconciliation and merge against the shared clipboard DB.
//!
//! Sync is keyed entirely on `content_hash`: each side advertises the set of
//! hashes it holds, and only ever sends entries the peer is missing. That makes
//! the merge idempotent and loop-free by construction — there is no echo to
//! suppress, because a hash the peer already has is never re-sent.
//!
//! All DB access goes through the shared `ClipboardDb`. SQLite WAL mode lets
//! this run concurrently with clipd (writer) and the clipboard UI (reader)
//! without coordination.

use crate::clip::db::ClipboardDb;
use crate::clipsync::protocol::WireEntry;
use std::collections::HashSet;

/// Outcome of merging a batch of received entries.
pub struct MergeResult {
    /// Number of entries that were genuinely new locally and got inserted.
    pub inserted: usize,
    /// The newest inserted entry by `created_at`, if any. The caller uses this
    /// to decide whether to push fresh peer content onto the live clipboard.
    pub newest: Option<WireEntry>,
}

/// Every `content_hash` currently in the local DB.
///
/// This is the set advertised to the peer. It is read fresh on each
/// reconciliation round so it always reflects the live DB, including rows
/// clipd added since the last round.
pub fn local_hashes(db: &ClipboardDb) -> rusqlite::Result<Vec<i64>> {
    db.all_hashes()
}

/// Given the hashes a peer advertised, return the subset this machine lacks —
/// i.e. exactly what to ask the peer to send.
pub fn missing_locally(db: &ClipboardDb, peer_hashes: &[i64]) -> rusqlite::Result<Vec<i64>> {
    let mine: HashSet<i64> = local_hashes(db)?.into_iter().collect();
    Ok(peer_hashes
        .iter()
        .copied()
        .filter(|h| !mine.contains(h))
        .collect())
}

/// Load full entries for the hashes a peer requested, ready to send on the wire.
///
/// A hash the peer asked for but that has since vanished locally (e.g. clipd's
/// cleanup pruned it) is silently skipped — the peer simply will not receive it.
pub fn entries_for(db: &ClipboardDb, wanted: &[i64]) -> rusqlite::Result<Vec<WireEntry>> {
    let mut out = Vec::with_capacity(wanted.len());
    for &hash in wanted {
        if let Some(e) = db.get_by_hash(hash)? {
            out.push(WireEntry {
                content_hash: e.content_hash,
                created_at: e.created_at,
                last_used: e.last_used,
                mime: e.mime,
                content: e.content,
            });
        }
    }
    Ok(out)
}

/// Insert received entries into the local DB.
///
/// Uses the existing `store_with_timestamps` upsert, which is
/// `ON CONFLICT(content_hash) DO NOTHING` — so an entry the local DB already
/// holds (a race with clipd, or a hash the peer re-sent) is a no-op, never a
/// duplicate and never an overwrite of local history. Insertion is therefore
/// purely additive: existing rows are untouched.
pub fn merge(db: &ClipboardDb, entries: Vec<WireEntry>) -> rusqlite::Result<MergeResult> {
    let mut inserted = 0usize;
    let mut newest: Option<WireEntry> = None;

    for e in entries {
        let existed = db.get_by_hash(e.content_hash)?.is_some();
        db.store_with_timestamps(
            &e.content,
            e.content_hash,
            &e.mime,
            e.created_at,
            e.last_used,
        )?;
        if !existed {
            inserted += 1;
            if newest
                .as_ref()
                .map(|n| e.created_at > n.created_at)
                .unwrap_or(true)
            {
                newest = Some(e);
            }
        }
    }

    if inserted > 0 {
        // Keep the shared retention policy applied after a bulk insert, exactly
        // as clipd does after each store.
        let _ = db.cleanup();
    }

    Ok(MergeResult { inserted, newest })
}
