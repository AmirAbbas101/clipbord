use crate::clipboard::{ensure_session_env, pending_to_item, ClipboardWatcher};
use crate::config::Config;
use crate::ipc::{self, IpcServer};
use crate::store::HistoryStore;
use anyhow::Result;
use std::time::Duration;

/// Fully headless daemon: no GTK Application, no windows, no dock entry.
/// Clipboard capture uses xclip/Xwayland (never wl-paste when xclip is installed).
pub fn run(config: Config) -> Result<()> {
    // Before anything else: DISPLAY is often missing when systemd starts at login.
    ensure_session_env();

    if ipc::socket_path()?.exists() {
        match ipc::ping() {
            Ok(()) => {
                tracing::info!("clipbord daemon already running; exiting quietly");
                return Ok(());
            }
            Err(_) => {
                let _ = std::fs::remove_file(ipc::socket_path()?);
            }
        }
    }

    let store = HistoryStore::open(config.clone())?;
    let watcher = ClipboardWatcher::start(config.clone())?;
    let ipc = IpcServer::bind()?;
    let auto_paste = config.auto_paste;

    tracing::info!(
        "clipbord daemon ready (headless, poll={}ms, max_items={})",
        config.poll_interval_ms.clamp(1500, 10_000),
        config.max_items
    );

    loop {
        if let Err(e) = ipc.poll(&store, auto_paste) {
            tracing::warn!("ipc: {e:#}");
        }

        while let Some(pending) = watcher.try_recv() {
            if let Some(item) =
                pending_to_item(pending, config.max_text_bytes, config.max_image_bytes)
            {
                let preview = item.preview.clone();
                if store.push(item) {
                    tracing::info!("stored clip: {preview}");
                }
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}
