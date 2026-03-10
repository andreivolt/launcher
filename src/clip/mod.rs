/// Clipboard backend — database layer, MIME detection, and Wayland watcher.
///
/// Used by both `clipd` (daemon, read-write) and `clipboard` (UI, read-only via WAL).

pub mod db;
pub mod mime;
pub mod watcher;

// Re-exports for convenience
pub use db::{ClipboardDb, Entry};
