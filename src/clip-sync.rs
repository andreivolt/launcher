/// clip-sync — clipboard-history sync daemon.
///
/// Mirrors clipd's clipboard history between two Hyprland machines over
/// Tailscale. It is orthogonal to clipd: it opens the same shared SQLite DB,
/// reconciles entries with a peer keyed on `content_hash`, and never runs its
/// own Wayland clipboard watcher.
///
/// The peer hostname comes from `$CLIP_SYNC_PEER`, or the first CLI argument.

use launcher::clipsync::transport::{self, Config};

fn main() -> ! {
    let peer_host = std::env::var("CLIP_SYNC_PEER")
        .ok()
        .or_else(|| std::env::args().nth(1))
        .unwrap_or_default();

    if peer_host.trim().is_empty() {
        eprintln!("[clip-sync] no peer configured");
        eprintln!("[clip-sync] set CLIP_SYNC_PEER=<tailscale-hostname> or pass it as an argument");
        std::process::exit(1);
    }

    let local_host = transport::hostname();
    let peer_host = peer_host.trim().to_ascii_lowercase();

    if peer_host == local_host {
        eprintln!("[clip-sync] peer '{peer_host}' is this host; nothing to sync");
        std::process::exit(1);
    }

    eprintln!("[clip-sync] starting: host '{local_host}', peer '{peer_host}'");

    transport::run(Config {
        local_host,
        peer_host,
    })
}
