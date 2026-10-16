mod db;

use crate::config::Config;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

pub use db::db_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    Text,
    Image,
}

#[derive(Debug, Clone)]
pub struct ClipItem {
    pub id: String,
    pub kind: ClipKind,
    pub text: Option<String>,
    pub image_png: Option<Vec<u8>>,
    /// Small PNG for the history list (not persisted — rebuilt on load).
    pub thumbnail_png: Option<Vec<u8>>,
    pub preview: String,
    pub hash: String,
    pub created_at: DateTime<Utc>,
    pub pinned: bool,
    pub byte_size: usize,
}

impl ClipItem {
    pub fn from_text(text: String, max_preview: usize) -> Self {
        let preview = make_preview(&text, max_preview);
        let hash = hash_bytes(text.as_bytes());
        let byte_size = text.len();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind: ClipKind::Text,
            text: Some(text),
            image_png: None,
            thumbnail_png: None,
            preview,
            hash,
            created_at: Utc::now(),
            pinned: false,
            byte_size,
        }
    }

    pub fn from_image(png: Vec<u8>) -> Self {
        let hash = hash_bytes(&png);
        let byte_size = png.len();
        let loaded = image::load_from_memory(&png).ok();
        let dims = loaded
            .as_ref()
            .map(|i| format!("{}×{}", i.width(), i.height()));
        let thumbnail_png = make_thumbnail_png(&png);
        let preview = match dims {
            Some(d) => format!("Image · {d} · {}", human_bytes(byte_size)),
            None => format!("Image · {}", human_bytes(byte_size)),
        };
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind: ClipKind::Image,
            text: None,
            image_png: Some(png),
            thumbnail_png,
            preview,
            hash,
            created_at: Utc::now(),
            pinned: false,
            byte_size,
        }
    }
}

/// 64×64 PNG thumbnail for fast UI list rendering.
pub fn make_thumbnail_png(png: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(png).ok()?.thumbnail(64, 64);
    let mut out = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut out);
        img.write_to(&mut cursor, image::ImageFormat::Png).ok()?;
    }
    Some(out)
}

#[derive(Clone)]
pub struct HistoryStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    items: Vec<ClipItem>,
    config: Config,
    db: Option<db::Database>,
}

impl HistoryStore {
    pub fn open(config: Config) -> Result<Self> {
        let db = if config.persist {
            Some(db::Database::open()?)
        } else {
            None
        };
        let items = if let Some(ref db) = db {
            db.load_all()?
        } else {
            Vec::new()
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner { items, config, db })),
        })
    }

    pub fn list(&self) -> Vec<ClipItem> {
        self.inner.lock().unwrap().items.clone()
    }

    pub fn search(&self, query: &str) -> Vec<ClipItem> {
        let q = query.trim().to_lowercase();
        let items = self.list();
        if q.is_empty() {
            return items;
        }
        items
            .into_iter()
            .filter(|item| {
                item.preview.to_lowercase().contains(&q)
                    || item
                        .text
                        .as_ref()
                        .map(|t| t.to_lowercase().contains(&q))
                        .unwrap_or(false)
            })
            .collect()
    }

    /// Insert a new clip. Returns true if the history changed.
    pub fn push(&self, item: ClipItem) -> bool {
        let mut guard = self.inner.lock().unwrap();
        let cfg = guard.config.clone();

        if cfg.dedupe {
            if let Some(pos) = guard.items.iter().position(|x| x.hash == item.hash) {
                let mut existing = guard.items.remove(pos);
                // Refresh recency + payload; keep id / pin state.
                existing.created_at = Utc::now();
                existing.text = item.text;
                existing.image_png = item.image_png;
                existing.thumbnail_png = item.thumbnail_png;
                existing.preview = item.preview;
                existing.byte_size = item.byte_size;
                existing.kind = item.kind;
                guard.items.insert(0, existing);
                sort_items(&mut guard.items);
                let _ = persist_all(&guard);
                return true;
            }
        }

        guard.items.insert(0, item);
        evict(&mut guard.items, cfg.max_items);
        sort_items(&mut guard.items);
        let _ = persist_all(&guard);
        true
    }

    pub fn toggle_pin(&self, id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if let Some(item) = guard.items.iter_mut().find(|i| i.id == id) {
            item.pinned = !item.pinned;
            sort_items(&mut guard.items);
            let _ = persist_all(&guard);
            return true;
        }
        false
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap();
        let before = guard.items.len();
        guard.items.retain(|i| i.id != id);
        if guard.items.len() != before {
            let _ = persist_all(&guard);
            true
        } else {
            false
        }
    }

    /// Move an item up (`delta < 0`) or down (`delta > 0`) in the list.
    /// Move item to an absolute index (drag-and-drop). Keeps pin groups intact.
    pub fn move_to_index(&self, id: &str, new_index: usize) -> bool {
        let mut guard = self.inner.lock().unwrap();
        let len = guard.items.len();
        if len == 0 {
            return false;
        }
        let Some(old) = guard.items.iter().position(|i| i.id == id) else {
            return false;
        };
        let pinned = guard.items[old].pinned;
        let first = guard
            .items
            .iter()
            .position(|i| i.pinned == pinned)
            .unwrap_or(0);
        let last = guard
            .items
            .iter()
            .rposition(|i| i.pinned == pinned)
            .unwrap_or(0);
        let new_index = new_index.clamp(first, last);
        if old == new_index {
            return false;
        }
        let item = guard.items.remove(old);
        let insert_at = new_index.min(guard.items.len());
        guard.items.insert(insert_at, item);
        let _ = persist_all(&guard);
        true
    }

    pub fn clear_unpinned(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard.items.retain(|i| i.pinned);
        let _ = persist_all(&guard);
    }

    pub fn get(&self, id: &str) -> Option<ClipItem> {
        self.inner
            .lock()
            .unwrap()
            .items
            .iter()
            .find(|i| i.id == id)
            .cloned()
    }

    pub fn config(&self) -> Config {
        self.inner.lock().unwrap().config.clone()
    }

    pub fn stats(&self) -> (usize, usize) {
        let guard = self.inner.lock().unwrap();
        let count = guard.items.len();
        let bytes = guard.items.iter().map(|i| i.byte_size).sum();
        (count, bytes)
    }
}

fn persist_all(inner: &Inner) -> Result<()> {
    if let Some(ref db) = inner.db {
        db.replace_all(&inner.items)?;
    }
    Ok(())
}

fn evict(items: &mut Vec<ClipItem>, max_items: usize) {
    if items.len() <= max_items {
        return;
    }
    // Drop oldest unpinned first.
    while items.len() > max_items {
        if let Some(idx) = items
            .iter()
            .enumerate()
            .rev()
            .find(|(_, i)| !i.pinned)
            .map(|(i, _)| i)
        {
            items.remove(idx);
        } else {
            // All pinned — drop oldest anyway.
            items.pop();
        }
    }
}

fn sort_items(items: &mut [ClipItem]) {
    items.sort_by(|a, b| match (a.pinned, b.pinned) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => b.created_at.cmp(&a.created_at),
    });
}

pub fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

pub fn make_preview(text: &str, max_chars: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= max_chars {
        flat
    } else {
        let truncated: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

pub fn human_bytes(n: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let n = n as f64;
    if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.1} KB", n / KB)
    } else {
        format!("{n:.0} B")
    }
}
