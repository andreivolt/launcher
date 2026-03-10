# clipd — Clipboard Backend

Native Wayland clipboard daemon and DB layer, replacing cliphist. Part of the launcher workspace — used by both `clipd` (daemon binary) and `clipboard` (UI binary).

## Module structure

```
src/clip/
  mod.rs       # Public API: ClipboardDb, Entry, re-exports
  db.rs        # SQLite schema, migrations, queries
  watcher.rs   # Wayland clipboard watcher (event-driven via wayland-clipboard-listener)
  mime.rs      # MIME detection from magic bytes
```

`clipd.rs` (binary entry point) lives in `src/clipd.rs`, uses this module.

## DB Schema (SQLite, WAL mode)

```sql
CREATE TABLE entries (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    content       BLOB NOT NULL,
    content_hash  INTEGER NOT NULL,      -- FNV-1a hash for fast dedup
    mime          TEXT NOT NULL DEFAULT 'text/plain',
    source_app    TEXT,                   -- app_id of source window (via toplevel tracking)
    created_at    INTEGER NOT NULL,       -- unix seconds, first seen
    last_used     INTEGER NOT NULL,       -- unix seconds, updated on paste
    pinned        INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX idx_hash ON entries(content_hash);
CREATE INDEX idx_last_used ON entries(last_used);
```

DB path: `$XDG_CACHE_HOME/clipd/db.sqlite` (default `~/.cache/clipd/db.sqlite`)

### Design decisions

These are based on analysis of clipvault (github.com/Rolv-Apneseth/clipvault) and stash (github.com/NotAShelf/stash):

- **WAL mode** with `synchronous=NORMAL` — safe against crashes, allows concurrent reader (UI) + writer (daemon). Stash uses `journal_mode=MEMORY, synchronous=OFF` which can corrupt on crash. Clipvault uses WAL correctly.
- **FNV-1a content hash** for dedup — fast, deterministic across runs (unlike stdlib DefaultHasher which uses random seed). Stash uses this. Clipvault uses UNIQUE on content blob which is slower for large entries.
- **Upsert on hash conflict** — if same content is copied again, bump `last_used` timestamp. Don't create duplicates.
- **MIME stored at write time** — clipvault sniffs MIME on every `list` call, which is wasteful. Store it once.
- **Separate `created_at` / `last_used`** — know when content was first copied AND when last pasted back. Neither clipvault nor stash distinguishes these.
- **`source_app`** — which app the content came from. Tracked via `zwlr_foreign_toplevel_manager_v1` protocol (same as stash's app exclusion feature, but we store it as metadata).
- **`pinned`** — favorites that survive cleanup. Simple integer flag.
- **Hard delete for cleanup** — trim old entries on insert. No soft-delete/is_expired complexity (stash's approach adds schema bloat).

### Pragmas (applied on every connection)

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA cache_size = -2000;     -- 2MB cache
PRAGMA busy_timeout = 5000;    -- 5s retry on lock
PRAGMA foreign_keys = ON;
```

## Daemon (`clipd`)

Entry point: `src/clipd.rs`

### Clipboard watching

Uses `wayland-clipboard-listener` crate with the `wlr-data-control` feature (`WlClipboardPasteStreamWlr`). Event-driven — blocks on a Wayland event stream, no polling. Direct Wayland connection, no CLI tools.

Event loop:
1. Receive clipboard change event (blocking iterator)
2. MIME priority set via `set_priority()` — images > text > uri-list (skips HTML when images available)
3. FNV-1a hash the content
4. If hash differs from last seen → upsert into DB
5. Trim excess entries (beyond max count / max age, skip pinned)

### MIME negotiation

Priority order when clipboard offers multiple types:
1. `image/png`, `image/jpeg`, `image/gif`, `image/webp` — prefer images
2. `text/plain;charset=utf-8`, `text/plain` — standard text
3. `text/uri-list` — file paths
4. Skip `text/html` when image types are available (Firefox "Copy Image" sends both HTML and image — prefer image, like stash does)

### Source app tracking (optional)

Background thread connects to Wayland and subscribes to `zwlr_foreign_toplevel_manager_v1`. Tracks which app_id is currently focused. When a clipboard change is detected, the currently focused app_id is stored as `source_app`.

### Cleanup policy

On every insert, after upsert:
```sql
DELETE FROM entries
WHERE pinned = 0
  AND id NOT IN (SELECT id FROM entries ORDER BY last_used DESC LIMIT :max_entries)
  OR (pinned = 0 AND last_used < :cutoff);
```

Defaults: max 1000 entries, max 30 days.

### CLI

- `clipd` — run the watcher daemon (foreground, no subcommands)

### Auto-migration from cliphist

On first start, if DB is empty and `cliphist` is on PATH:
1. Run `cliphist list`, parse each `id\tpreview` line
2. For each entry, run `cliphist decode` with the id piped to stdin, capture raw bytes
3. Detect MIME, hash content, insert into DB with `created_at = last_used = now()`
4. Log count of migrated entries

## UI integration

The `clipboard` binary (UI) reads the same SQLite DB directly:

- Open DB read-only (WAL allows concurrent readers)
- Query: `SELECT id, content, mime, source_app, created_at, last_used, pinned FROM entries WHERE NOT is_expired ORDER BY last_used DESC`
- On user paste: pipe content to `wl-copy`, then `UPDATE entries SET last_used = ? WHERE id = ?`
- On user delete: `DELETE FROM entries WHERE id = ?`
- Live reload: watch DB file with `notify` crate (inotify on Linux), same pattern as current cliphist watcher

### Timestamp display format

Relative time, compact:
- < 1 min: "now"
- < 1 hour: "5m"
- < 24 hours: "3h"
- < 7 days: "2d"
- < current year: "Mar 3"
- older: "2025-12-15"

## Dependencies

New crates needed (add to workspace Cargo.toml):
- `rusqlite` with `bundled` feature — SQLite with WAL support
- `wayland-clipboard-listener` with `wlr-data-control` feature — event-driven Wayland clipboard watching
- `imagesize` — lightweight MIME detection from magic bytes (much lighter than `image` crate)
- `fnv` — FNV-1a hasher
- `dirs` — XDG directory paths

## NixOS / flake.nix changes

- Add `clipd` to `postInstall` wrapProgram list
- New systemd user service `clipd`:
  ```nix
  systemd.user.services.clipd = {
    description = "Clipboard daemon";
    wantedBy = [ "hyprland-session.target" ];
    partOf = [ "hyprland-session.target" ];
    serviceConfig = {
      ExecStart = "${launcherPkg}/bin/clipd";
      Restart = "on-failure";
      RestartSec = 2;
      PassEnvironment = "HYPRLAND_INSTANCE_SIGNATURE XDG_RUNTIME_DIR WAYLAND_DISPLAY XDG_CACHE_HOME HOME";
    };
  };
  ```
- Remove `cliphist` and `wl-clipboard` from clipboard service path
- Keep `wl-clipboard` as system package (for `wl-copy` paste-back)
- Remove `wl-paste --watch cliphist store` if present anywhere
