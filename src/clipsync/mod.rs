//! clip-sync backend — mirrors clipboard history between two Hyprland machines
//! over Tailscale.
//!
//! This is a peer in the shared-DB architecture, not a fork of clipd: it opens
//! the same `ClipboardDb` that clipd writes and the `clipboard` UI reads, and
//! syncs purely by reconciling `content_hash` sets with a remote peer. It never
//! runs its own Wayland clipboard watcher — clipd owns that.
//!
//! Modules are split by concern:
//! - [`db_observer`] — detects rows clipd appended locally.
//! - [`protocol`] — the length-prefixed wire format.
//! - [`reconcile`] — set-reconciliation and the merge into the local DB.
//! - [`transport`] — TCP, connection lifecycle, reconnect, session driver.

pub mod db_observer;
pub mod protocol;
pub mod reconcile;
pub mod transport;
