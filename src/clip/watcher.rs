/// Wayland clipboard watcher — event-driven via wayland-clipboard-listener.
///
/// Uses the `zwlr_data_control_manager` protocol through wayland-clipboard-listener,
/// which provides a blocking iterator over clipboard changes — no polling needed.
///
/// Beyond recording history, the watcher also *persists* each new entry: it
/// re-asserts ownership of the live CLIPBOARD selection via `wl-copy`, which
/// forks a daemon that serves the content independently of the app that copied
/// it. Without this, content copied by a non-persistent owner (e.g. mpv) is
/// lost the moment that app exits — recorded in history but unpasteable. This
/// makes clipd a true clipboard manager, like wl-clip-persist / cliphist.

use crate::clip::db::ClipboardDb;
use crate::clip::mime;
use fnv::FnvHasher;
use std::hash::Hasher;
use std::io::Write;
use std::process::{Command, Stdio};
use wayland_clipboard_listener::{WlClipboardPasteStreamWlr, WlListenType};

/// Compute FNV-1a hash of content bytes.
pub fn content_hash(data: &[u8]) -> i64 {
    let mut hasher = FnvHasher::default();
    hasher.write(data);
    hasher.finish() as i64
}

/// Persist `content` onto the live CLIPBOARD selection via `wl-copy`.
///
/// `wl-copy` forks a daemon that owns and serves the selection independently of
/// any app, so the content survives the original owning app exiting. `mime` is
/// the storage MIME ("text/plain", "image/png", …); it is passed through as
/// `--type` so images and URI lists round-trip with the right type.
fn persist_to_clipboard(content: &[u8], mime: &str) {
    let mut cmd = Command::new("wl-copy");
    // wl-copy defaults to text; only override for non-text payloads so it
    // advertises the matching target. text/plain is left to wl-copy's default.
    if mime != "text/plain" {
        cmd.arg("--type").arg(mime);
    }
    match cmd.stdin(Stdio::piped()).spawn() {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(content);
            }
            let _ = child.wait();
        }
        Err(e) => eprintln!("[clipd] wl-copy persist failed: {}", e),
    }
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
    // Hash of the content clipd itself last put on the clipboard via
    // `persist_to_clipboard`. That `wl-copy` triggers a fresh wlr-data-control
    // ownership-change event the watcher will observe; recognising our own
    // content here breaks the loop — no re-record, no re-persist, no dup row.
    let mut persisted_hash: i64 = 0;

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

        // Loop guard: this event is the echo of clipd's own persist. The DB
        // upsert would dedup it anyway, but skipping outright also avoids a
        // redundant cleanup and a pointless second wl-copy.
        if hash == persisted_hash {
            continue;
        }

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

        // Re-assert ownership of the live selection so the content outlives the
        // app that copied it. Record the hash first so the resulting echo
        // event is recognised as ours and ignored above.
        persisted_hash = hash;
        persist_to_clipboard(content, store_mime);
    }

    eprintln!("[clipd] clipboard stream ended unexpectedly");
}
