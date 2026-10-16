use crate::config::Config;
use crate::store::hash_bytes;
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum PendingClip {
    Text(String),
    ImagePng(Vec<u8>),
}

/// Background clipboard capture that does **not** flash the GNOME dock.
///
/// Uses **xclip / Xwayland** only when installed. Never polls `wl-paste`
/// (that flashes the dock). Images come from xclip image targets or from
/// clipboard text that points at a local image file (common for screenshots).
pub struct ClipboardWatcher {
    rx: Receiver<PendingClip>,
}

impl ClipboardWatcher {
    pub fn start(config: Config) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("clipbord-watcher".into())
            .spawn(move || watch_loop(config, tx))
            .context("spawn clipboard watcher")?;
        Ok(Self { rx })
    }

    pub fn try_recv(&self) -> Option<PendingClip> {
        self.rx.try_recv().ok()
    }
}

#[derive(Clone, Copy)]
enum Backend {
    Xclip,
    WlPaste,
}

fn watch_loop(config: Config, tx: Sender<PendingClip>) {
    ensure_session_env();
    let backend = choose_backend();
    let interval = Duration::from_millis(config.poll_interval_ms.clamp(1500, 10_000));

    match backend {
        Backend::Xclip => {
            tracing::info!("waiting for Xwayland before polling clipboard");
            wait_for_xclip();
            tracing::info!(
                "clipboard watcher using xclip/Xwayland every {}ms (text + images)",
                interval.as_millis()
            );
        }
        Backend::WlPaste => {
            tracing::warn!(
                "xclip not installed — using wl-paste (may flash the GNOME dock). Fix: sudo dnf install xclip && systemctl --user restart clipbord.service"
            );
        }
    }

    let mut last_hash = String::new();

    if let Some(pending) = sample_clipboard(&config, backend) {
        last_hash = clip_hash(&pending);
        let _ = tx.send(pending);
        tracing::info!("initial clipboard sample captured");
    }

    let mut polls_since_health = 0u32;
    loop {
        ensure_session_env();
        // Empty clipboard is normal — only re-wait when xclip itself is broken (auth/display).
        polls_since_health = polls_since_health.saturating_add(1);
        if matches!(backend, Backend::Xclip) && polls_since_health >= 30 {
            polls_since_health = 0;
            if !xclip_healthy() {
                tracing::warn!("xclip lost X11 access; re-waiting for Xwayland auth");
                wait_for_xclip();
            }
        }

        if let Some(pending) = sample_clipboard(&config, backend) {
            let hash = clip_hash(&pending);
            if hash != last_hash {
                last_hash = hash;
                if tx.send(pending).is_err() {
                    break;
                }
            }
        }
        thread::sleep(interval);
    }
}

fn choose_backend() -> Backend {
    if which("xclip") {
        Backend::Xclip
    } else if which("wl-paste") {
        Backend::WlPaste
    } else {
        tracing::error!("neither xclip nor wl-paste found; clipboard capture disabled");
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
}

/// systemd user units often start before GNOME exports DISPLAY / XAUTHORITY.
/// Without XAUTHORITY, xclip cannot open the Xwayland display after reboot.
pub fn ensure_session_env() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        if let Ok(uid) = std::fs::read_to_string("/proc/self/loginuid") {
            let uid = uid.trim();
            if !uid.is_empty() && uid != "4294967295" {
                let dir = format!("/run/user/{uid}");
                if Path::new(&dir).is_dir() {
                    std::env::set_var("XDG_RUNTIME_DIR", &dir);
                }
            }
        }
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            let dir = format!("/run/user/{}", nix_uid());
            if Path::new(&dir).is_dir() {
                std::env::set_var("XDG_RUNTIME_DIR", dir);
            }
        }
    }

    // Always refresh XAUTHORITY — GNOME creates a new .mutter-Xwaylandauth.* each login.
    if let Some(auth) = find_xauthority() {
        std::env::set_var("XAUTHORITY", auth);
    }

    // Prefer a display that xclip can actually open with current auth.
    if let Some(display) = find_working_display() {
        std::env::set_var("DISPLAY", display);
    } else if std::env::var_os("DISPLAY").is_none() {
        if Path::new("/tmp/.X11-unix/X0").exists() {
            std::env::set_var("DISPLAY", ":0");
        } else if Path::new("/tmp/.X11-unix/X1").exists() {
            std::env::set_var("DISPLAY", ":1");
        } else {
            std::env::set_var("DISPLAY", ":0");
        }
    }

    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
            for name in ["wayland-0", "wayland-1"] {
                let sock = Path::new(&runtime).join(name);
                if sock.exists() {
                    std::env::set_var("WAYLAND_DISPLAY", name);
                    break;
                }
            }
        }
    }
}

/// GNOME Wayland stores X11 cookies in a temp file under XDG_RUNTIME_DIR.
fn find_xauthority() -> Option<PathBuf> {
    // Prefer an already-valid env path.
    if let Ok(existing) = std::env::var("XAUTHORITY") {
        let p = PathBuf::from(&existing);
        if p.is_file() {
            return Some(p);
        }
    }

    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let runtime = Path::new(&runtime);
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(runtime) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".mutter-Xwaylandauth.") {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                match &best {
                    None => best = Some((modified, path)),
                    Some((t, _)) if modified > *t => best = Some((modified, path)),
                    _ => {}
                }
            }
        }
    }
    if let Some((_, path)) = best {
        return Some(path);
    }

    let home_auth = PathBuf::from(format!("{}/.Xauthority", std::env::var("HOME").ok()?));
    if home_auth.is_file() {
        return Some(home_auth);
    }
    None
}

fn find_working_display() -> Option<String> {
    for candidate in [":0", ":1"] {
        let num = candidate.trim_start_matches(':');
        let sock = PathBuf::from(format!("/tmp/.X11-unix/X{num}"));
        if !sock.exists() {
            continue;
        }
        if xclip_probe_on(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn nix_uid() -> u32 {
    // Avoid extra deps — parse Status from /proc/self.
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                if let Some(uid) = rest.split_whitespace().next() {
                    if let Ok(v) = uid.parse() {
                        return v;
                    }
                }
            }
        }
    }
    1000
}

/// Block until Xwayland auth exists and xclip can open the display.
fn wait_for_xclip() {
    let mut logged = false;
    loop {
        ensure_session_env();
        let auth = std::env::var("XAUTHORITY").unwrap_or_default();
        let display_name = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());

        if !auth.is_empty() && Path::new(&auth).is_file() && xclip_healthy() {
            if logged {
                tracing::info!(
                    "Xwayland ready (DISPLAY={display_name}, XAUTHORITY={auth})"
                );
            }
            return;
        }
        if !logged {
            tracing::info!(
                "waiting for mutter Xwayland auth + working xclip (DISPLAY={display_name})…"
            );
            logged = true;
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// True when xclip can open the display (auth OK). Empty clipboard still counts as healthy.
fn xclip_healthy() -> bool {
    xclip_probe_on(&std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()))
}

fn xclip_probe_on(display: &str) -> bool {
    let mut cmd = Command::new("timeout");
    cmd.args([
        "--signal=KILL",
        "0.5",
        "xclip",
        "-selection",
        "clipboard",
        "-t",
        "TARGETS",
        "-o",
    ])
    .env("DISPLAY", display)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    if let Ok(auth) = std::env::var("XAUTHORITY") {
        cmd.env("XAUTHORITY", auth);
    }
    let output = cmd.output();
    match output {
        Ok(o) => {
            if o.status.code() == Some(124) {
                return false;
            }
            let err = String::from_utf8_lossy(&o.stderr);
            // Auth failure looks like success (exit 1) unless we check stderr.
            if err.contains("Authorization required") || err.contains("Can't open display") {
                return false;
            }
            // TARGETS output or empty clipboard with reachable display.
            true
        }
        Err(_) => false,
    }
}

fn clip_hash(pending: &PendingClip) -> String {
    match pending {
        PendingClip::Text(t) => hash_bytes(t.as_bytes()),
        PendingClip::ImagePng(b) => hash_bytes(b),
    }
}

fn sample_clipboard(config: &Config, backend: Backend) -> Option<PendingClip> {
    match backend {
        Backend::Xclip => sample_xclip(config),
        Backend::WlPaste => sample_text_only(config, Backend::WlPaste),
    }
}

fn sample_xclip(config: &Config) -> Option<PendingClip> {
    // Prefer real image data from the clipboard.
    if let Some(png) = read_image_xclip(config.max_image_bytes) {
        return Some(PendingClip::ImagePng(png));
    }

    let max = config.max_text_bytes.min(256 * 1024);
    let text = read_text_xclip(max)?;
    if text.trim().is_empty() {
        return None;
    }

    // Screenshot tools often put a file path / file:// URI on the clipboard.
    if let Some(png) = image_from_path_text(&text, config.max_image_bytes) {
        return Some(PendingClip::ImagePng(png));
    }

    let _ = config.ignore_sensitive;
    Some(PendingClip::Text(text))
}

fn sample_text_only(config: &Config, backend: Backend) -> Option<PendingClip> {
    let max = config.max_text_bytes.min(256 * 1024);
    let text = match backend {
        Backend::Xclip => read_text_xclip(max)?,
        Backend::WlPaste => read_text_wlpaste(max)?,
    };
    if text.trim().is_empty() {
        return None;
    }
    if let Some(png) = image_from_path_text(&text, config.max_image_bytes) {
        return Some(PendingClip::ImagePng(png));
    }
    Some(PendingClip::Text(text))
}

/// One-shot sample for IPC ingest.
pub fn sample_once(max_text: usize, max_image: usize) -> Option<PendingClip> {
    ensure_session_env();
    let config = Config {
        max_text_bytes: max_text,
        max_image_bytes: max_image,
        ..Config::default()
    };
    let backend = if which("xclip") {
        Backend::Xclip
    } else if which("wl-paste") {
        Backend::WlPaste
    } else {
        return None;
    };
    sample_clipboard(&config, backend)
}

fn read_image_xclip(max_bytes: usize) -> Option<Vec<u8>> {
    if !which("xclip") {
        return None;
    }
    let targets = xclip_targets()?;
    let targets_l = targets.to_ascii_lowercase();

    let type_name = if targets_l.contains("image/png") {
        "image/png"
    } else if targets_l.contains("image/jpeg") || targets_l.contains("image/jpg") {
        "image/jpeg"
    } else if targets_l.contains("image/bmp") {
        "image/bmp"
    } else if targets_l.contains("image/tiff") {
        "image/tiff"
    } else {
        return None;
    };

    let output = Command::new("timeout")
        .args([
            "--signal=KILL",
            "2",
            "xclip",
            "-selection",
            "clipboard",
            "-t",
            type_name,
            "-o",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if output.status.code() == Some(124) || output.stdout.is_empty() {
        return None;
    }
    if output.stdout.len() > max_bytes {
        tracing::warn!(
            "clipboard image too large ({} > {}), skipped",
            output.stdout.len(),
            max_bytes
        );
        return None;
    }
    // Normalize to PNG for storage / UI.
    normalize_to_png(&output.stdout)
}

fn xclip_targets() -> Option<String> {
    let output = Command::new("timeout")
        .args([
            "--signal=KILL",
            "0.4",
            "xclip",
            "-selection",
            "clipboard",
            "-t",
            "TARGETS",
            "-o",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if output.status.code() == Some(124) {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn image_from_path_text(text: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let path = clipboard_text_to_image_path(text)?;
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() as usize > max_bytes || meta.len() == 0 {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    normalize_to_png(&bytes)
}

fn clipboard_text_to_image_path(text: &str) -> Option<PathBuf> {
    // Prefer first non-empty line (text/uri-list).
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = uri_or_path_to_path(line)?;
        if is_image_path(&path) {
            return Some(path);
        }
    }
    let trimmed = text.trim();
    if trimmed.lines().count() == 1 {
        let path = uri_or_path_to_path(trimmed)?;
        if is_image_path(&path) {
            return Some(path);
        }
    }
    None
}

fn uri_or_path_to_path(s: &str) -> Option<PathBuf> {
    let s = s.trim().trim_matches('"').trim_matches('\'');
    if let Some(rest) = s.strip_prefix("file://") {
        // file:///home/... or file://localhost/home/...
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        let decoded = percent_decode(rest);
        return Some(PathBuf::from(decoded));
    }
    if s.starts_with('/') {
        return Some(PathBuf::from(s));
    }
    None
}

fn percent_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push(char::from(h * 16 + l));
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn is_image_path(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "tif" | "tiff") => true,
        _ => false,
    }
}

fn normalize_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    // Already PNG?
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        return Some(bytes.to_vec());
    }
    let img = image::load_from_memory(bytes).ok()?;
    let mut out = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut out);
    img.write_to(&mut cursor, image::ImageFormat::Png).ok()?;
    Some(out)
}

fn read_text_xclip(max_bytes: usize) -> Option<String> {
    if !which("xclip") {
        return None;
    }
    let output = Command::new("timeout")
        .args([
            "--signal=KILL",
            "0.5",
            "xclip",
            "-selection",
            "clipboard",
            "-o",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if output.status.code() == Some(124) {
        return None;
    }
    if output.stdout.is_empty() {
        return None;
    }
    truncate_utf8(output.stdout, max_bytes)
}

fn read_text_wlpaste(max_bytes: usize) -> Option<String> {
    if !which("wl-paste") {
        return None;
    }
    let output = Command::new("timeout")
        .args([
            "--signal=KILL",
            "0.5",
            "wl-paste",
            "--no-newline",
            "--type",
            "text",
        ])
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("DESKTOP_STARTUP_ID")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if output.status.code() == Some(124) || !output.status.success() {
        return None;
    }
    truncate_utf8(output.stdout, max_bytes)
}

fn truncate_utf8(bytes: Vec<u8>, max_bytes: usize) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let bytes = if bytes.len() > max_bytes {
        let mut end = max_bytes;
        while end > 0 && !bytes[end].is_ascii() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end -= 1;
        }
        bytes[..end].to_vec()
    } else {
        bytes
    };
    String::from_utf8(bytes).ok()
}

fn which(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
