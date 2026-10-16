use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// User-tunable settings. Persisted as TOML under XDG config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Maximum history entries (pinned count toward this total).
    pub max_items: usize,
    /// Truncate / reject text larger than this (bytes).
    pub max_text_bytes: usize,
    /// Reject images larger than this (bytes) to protect RAM.
    pub max_image_bytes: usize,
    /// How often to sample the clipboard when the GDK signal is quiet (ms).
    pub poll_interval_ms: u64,
    /// Deduplicate identical consecutive copies.
    pub dedupe: bool,
    /// Persist history across restarts.
    pub persist: bool,
    /// After selecting an item, try to auto-paste via `wtype` if available.
    pub auto_paste: bool,
    /// Ignore password managers / sensitive MIME hints when possible.
    pub ignore_sensitive: bool,
    /// Window width in pixels.
    pub window_width: i32,
    /// Window height in pixels.
    pub window_height: i32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_items: 50,
            max_text_bytes: 512 * 1024,
            max_image_bytes: 4 * 1024 * 1024,
            // Capture interval. Keep ≥1500ms so helpers stay quiet.
            poll_interval_ms: 2000,
            dedupe: true,
            persist: true,
            // Paste into the previously focused field after picking an item.
            auto_paste: true,
            ignore_sensitive: true,
            window_width: 480,
            window_height: 560,
        }
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("io", "github", "clipbord")
            .context("could not resolve XDG project dirs")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    pub fn data_dir() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("io", "github", "clipbord")
            .context("could not resolve XDG project dirs")?;
        let dir = dirs.data_dir().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn runtime_dir() -> Result<PathBuf> {
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
            let dir = PathBuf::from(runtime).join("clipbord");
            fs::create_dir_all(&dir)?;
            return Ok(dir);
        }
        let dir = Self::data_dir()?.join("runtime");
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            let cfg = Self::default();
            cfg.save()?;
            return Ok(cfg);
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let cfg: Self = toml::from_str(&raw).context("parse config.toml")?;
        Ok(cfg.sanitized())
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self)?;
        fs::write(&path, raw)?;
        Ok(())
    }

    fn sanitized(mut self) -> Self {
        self.max_items = self.max_items.clamp(5, 500);
        self.max_text_bytes = self.max_text_bytes.clamp(4_096, 8 * 1024 * 1024);
        self.max_image_bytes = self.max_image_bytes.clamp(64 * 1024, 16 * 1024 * 1024);
        self.poll_interval_ms = self.poll_interval_ms.clamp(100, 5_000);
        self.window_width = self.window_width.clamp(360, 900);
        self.window_height = self.window_height.clamp(320, 900);
        self
    }
}
