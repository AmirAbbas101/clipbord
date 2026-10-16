mod watcher;

pub use watcher::{
    ensure_session_env, sample_once as watcher_sample_once, ClipboardWatcher, PendingClip,
};

use crate::store::{ClipItem, ClipKind};
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Prefer xclip (Xwayland) so we never create a Wayland toplevel / dock icon.
pub fn set_clipboard(item: &ClipItem) -> Result<()> {
    match item.kind {
        ClipKind::Text => {
            let text = item.text.as_deref().unwrap_or("");
            set_text(text)
        }
        ClipKind::Image => {
            let png = item
                .image_png
                .as_ref()
                .context("image item missing png bytes")?;
            set_image_png(png)
        }
    }
}

fn set_text(text: &str) -> Result<()> {
    // Prefer xclip exclusively when present — never fall back to wl-copy (dock flash).
    if which("xclip") {
        let mut child = Command::new("xclip")
            .args(["-selection", "clipboard", "-i"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn xclip")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        // xclip -i forks a background owner and the parent exits 0.
        let status = child.wait()?;
        if !status.success() {
            bail!("xclip failed: {status}");
        }
        return Ok(());
    }
    if which("wl-copy") {
        tracing::warn!("xclip missing; wl-copy may flash the GNOME dock");
        let mut child = Command::new("wl-copy")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DESKTOP_STARTUP_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn wl-copy")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        let status = child.wait()?;
        if !status.success() {
            bail!("wl-copy failed: {status}");
        }
        return Ok(());
    }
    bail!("need xclip (sudo dnf install xclip)")
}

fn set_image_png(png: &[u8]) -> Result<()> {
    // Prefer xclip so we never flash the dock.
    if which("xclip") {
        let mut child = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "image/png", "-i"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn xclip image")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(png)?;
        }
        let status = child.wait()?;
        if !status.success() {
            bail!("xclip image failed: {status}");
        }
        return Ok(());
    }
    if which("wl-copy") {
        tracing::warn!("xclip missing; wl-copy may flash the GNOME dock");
        let mut child = Command::new("wl-copy")
            .args(["--type", "image/png"])
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DESKTOP_STARTUP_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn wl-copy image")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(png)?;
        }
        let status = child.wait()?;
        if !status.success() {
            bail!("wl-copy image failed: {status}");
        }
        return Ok(());
    }
    bail!("need xclip to paste images (sudo dnf install xclip)")
}

/// Run after the UI has closed so focus returns to the previous input field.
/// GNOME Mutter rejects `wtype` (no virtual-keyboard protocol), so we try
/// ydotool / xdotool as well.
pub fn try_auto_paste() {
    // Let the history popup fully release focus first.
    std::thread::sleep(Duration::from_millis(220));

    // Prefer ydotool (works for Wayland apps on GNOME). xdotool is X11-only.
    // wtype usually fails on Mutter (no virtual-keyboard protocol).
    if try_paste_ydotool() {
        return;
    }
    if try_paste_xdotool() {
        return;
    }
    if try_paste_wtype() {
        return;
    }
    tracing::warn!(
        "auto-paste unavailable — install/start paste helpers: sudo dnf install xdotool ydotool && sudo systemctl enable --now ydotool.service"
    );
}

fn try_paste_wtype() -> bool {
    if !which("wtype") {
        return false;
    }
    let ok = Command::new("wtype")
        .args(["-M", "ctrl", "-k", "v", "-m", "ctrl"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        tracing::debug!("auto-paste via wtype");
    }
    ok
}

fn try_paste_xdotool() -> bool {
    if !which("xdotool") {
        return false;
    }
    // Clear modifiers so a held Super/Alt from the hotkey doesn't block paste.
    let ok = Command::new("xdotool")
        .args(["key", "--clearmodifiers", "ctrl+v"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        tracing::debug!("auto-paste via xdotool");
    }
    ok
}

fn try_paste_ydotool() -> bool {
    if !which("ydotool") {
        return false;
    }
    // Fedora's ydotoold listens on /tmp/.ydotool_socket by default.
    let socket = std::env::var("YDOTOOL_SOCKET")
        .unwrap_or_else(|_| "/tmp/.ydotool_socket".into());
    if !std::path::Path::new(&socket).exists() {
        return false;
    }
    // ctrl down, v down, v up, ctrl up (Linux keycodes)
    let ok = Command::new("ydotool")
        .args(["key", "29:1", "47:1", "47:0", "29:0"])
        .env("YDOTOOL_SOCKET", &socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        tracing::debug!("auto-paste via ydotool");
    }
    ok
}

pub fn which(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn pending_to_item(
    pending: PendingClip,
    max_text: usize,
    max_image: usize,
) -> Option<ClipItem> {
    match pending {
        PendingClip::Text(text) => {
            if text.len() > max_text || text.trim().is_empty() {
                return None;
            }
            Some(ClipItem::from_text(text, 120))
        }
        PendingClip::ImagePng(png) => {
            if png.is_empty() || png.len() > max_image {
                return None;
            }
            Some(ClipItem::from_image(png))
        }
    }
}
