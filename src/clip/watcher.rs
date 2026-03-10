/// Wayland clipboard watcher — event-driven via wayland-clipboard-listener.
///
/// Uses the `zwlr_data_control_manager` protocol through wayland-clipboard-listener,
/// which provides a blocking iterator over clipboard changes — no polling needed.

use crate::clip::db::ClipboardDb;
use crate::clip::mime;
use fnv::FnvHasher;
use std::hash::Hasher;
use wayland_clipboard_listener::{WlClipboardPasteStreamWlr, WlListenType};

/// Compute FNV-1a hash of content bytes.
pub fn content_hash(data: &[u8]) -> i64 {
    let mut hasher = FnvHasher::default();
    hasher.write(data);
    hasher.finish() as i64
}

/// Run the clipboard watcher loop. Blocks forever.
///
/// Listens for clipboard changes via the wlr-data-control Wayland protocol.
/// Each clipboard change is received as an event — no polling.
pub fn watch_loop(db: &ClipboardDb) {
    let mut stream = match WlClipboardPasteStreamWlr::init(WlListenType::ListenOnCopy) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[clipd] failed to connect to Wayland clipboard: {:?}", e);
            std::process::exit(1);
        }
    };

    // MIME priority: images first, then text, then URI list.
    // Ensures we prefer images over text/html (Firefox "Copy Image" sends both).
    stream.set_priority(vec![
        "image/png".into(),
        "image/jpeg".into(),
        "image/gif".into(),
        "image/webp".into(),
        "text/plain;charset=utf-8".into(),
        "UTF8_STRING".into(),
        "text/plain".into(),
        "STRING".into(),
        "text/uri-list".into(),
    ]);

    eprintln!("[clipd] watcher started (event-driven, wlr-data-control)");

    let mut last_hash: i64 = 0;

    for msg in stream.paste_stream().flatten() {
        let content = &msg.context.context;
        let received_mime = &msg.context.mime_type;

        if content.is_empty() {
            continue;
        }

        let hash = content_hash(content);
        if hash == last_hash {
            continue;
        }
        last_hash = hash;

        // Normalize MIME for storage
        let store_mime = match received_mime.as_str() {
            "text/plain;charset=utf-8" | "UTF8_STRING" | "STRING" | "text/plain" => "text/plain",
            "text/uri-list" => "text/uri-list",
            "text/html" => "text/html",
            m if m.starts_with("image/") => mime::detect_mime(content),
            other => other,
        };

        if let Err(e) = db.store(content, hash, store_mime, None) {
            eprintln!("[clipd] store error: {}", e);
            continue;
        }

        if let Err(e) = db.cleanup() {
            eprintln!("[clipd] cleanup error: {}", e);
        }
    }

    eprintln!("[clipd] clipboard stream ended unexpectedly");
}
