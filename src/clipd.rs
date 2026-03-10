/// clipd — Clipboard daemon
///
/// Watches the Wayland clipboard and stores entries in a SQLite database.
/// Auto-migrates from cliphist on first start if the DB is empty.

use launcher::clip::db::ClipboardDb;
use launcher::clip::mime;
use launcher::clip::watcher;

use std::io::Write;
use std::process::{Command, Stdio};

fn main() {
    eprintln!("[clipd] starting clipboard daemon");

    let db = match ClipboardDb::open_default() {
        Ok(db) => db,
        Err(e) => {
            eprintln!("[clipd] failed to open database: {}", e);
            std::process::exit(1);
        }
    };

    // Auto-migrate from cliphist on first start
    if let Ok(true) = db.is_empty() {
        migrate_from_cliphist(&db);
    }

    // Run the watcher loop (blocks forever)
    watcher::watch_loop(&db);
}

/// Migrate existing entries from cliphist if the binary is available.
///
/// Runs `cliphist list`, parses each `id\tpreview` line, then `cliphist decode`
/// for each entry to get the raw bytes. Detects MIME from magic bytes and inserts.
fn migrate_from_cliphist(db: &ClipboardDb) {
    // Check if cliphist is available
    let cliphist = match which("cliphist") {
        Some(path) => path,
        None => {
            eprintln!("[clipd] cliphist not found, skipping migration");
            return;
        }
    };

    eprintln!("[clipd] migrating from cliphist...");

    let list_output = match Command::new(&cliphist).arg("list").output() {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            eprintln!("[clipd] cliphist list failed: {}", String::from_utf8_lossy(&out.stderr));
            return;
        }
        Err(e) => {
            eprintln!("[clipd] failed to run cliphist: {}", e);
            return;
        }
    };

    let list_str = String::from_utf8_lossy(&list_output.stdout);
    let lines: Vec<&str> = list_str.lines().collect();

    if lines.is_empty() {
        eprintln!("[clipd] cliphist has no entries, skipping migration");
        return;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut migrated = 0u32;
    let mut errors = 0u32;

    for line in &lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Decode the raw content
        let content = match decode_cliphist_entry(&cliphist, line) {
            Some(data) if !data.is_empty() => data,
            _ => {
                errors += 1;
                continue;
            }
        };

        // Detect MIME from content
        let detected_mime = mime::detect_mime(&content);

        // Hash content
        let hash = watcher::content_hash(&content);

        // Insert with timestamps set to now
        match db.store_with_timestamps(&content, hash, detected_mime, now, now) {
            Ok(_) => migrated += 1,
            Err(e) => {
                // Duplicate hash conflicts are expected, don't count as errors
                if e.to_string().contains("UNIQUE") {
                    continue;
                }
                errors += 1;
                if errors <= 5 {
                    eprintln!("[clipd] migration error: {}", e);
                }
            }
        }
    }

    eprintln!("[clipd] migrated {} entries from cliphist ({} errors)", migrated, errors);
}

/// Decode a single cliphist entry by piping the full `id\tpreview` line to `cliphist decode`.
fn decode_cliphist_entry(cliphist: &str, line: &str) -> Option<Vec<u8>> {
    let mut child = Command::new(cliphist)
        .arg("decode")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(line.as_bytes());
    }

    let output = child.wait_with_output().ok()?;
    if output.status.success() {
        Some(output.stdout)
    } else {
        None
    }
}

/// Find a binary on PATH, similar to `which`.
fn which(name: &str) -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        let candidate = std::path::Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}
