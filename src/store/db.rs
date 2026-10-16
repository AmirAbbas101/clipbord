use super::{ClipItem, ClipKind};
use crate::config::Config;
use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use rusqlite::{params, Connection};
use std::path::PathBuf;

pub fn db_path() -> Result<PathBuf> {
    Ok(Config::data_dir()?.join("history.db"))
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open() -> Result<Self> {
        let path = db_path()?;
        let conn = Connection::open(&path)
            .with_context(|| format!("open sqlite {}", path.display()))?;
        // WAL = better concurrent read performance, lower fsync cost.
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA temp_store = MEMORY;
            PRAGMA mmap_size = 8388608;
            CREATE TABLE IF NOT EXISTS clips (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                text TEXT,
                image_png BLOB,
                preview TEXT NOT NULL,
                hash TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                pinned INTEGER NOT NULL,
                byte_size INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_clips_created ON clips(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_clips_hash ON clips(hash);
            ",
        )?;
        Ok(Self { conn })
    }

    pub fn load_all(&self) -> Result<Vec<ClipItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, text, image_png, preview, hash, created_at, pinned, byte_size
             FROM clips
             ORDER BY pinned DESC, created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let kind_str: String = row.get(1)?;
            let kind = if kind_str == "image" {
                ClipKind::Image
            } else {
                ClipKind::Text
            };
            let ts: i64 = row.get(6)?;
            let pinned: i64 = row.get(7)?;
            let image_png: Option<Vec<u8>> = row.get(3)?;
            let thumbnail_png = image_png
                .as_ref()
                .and_then(|png| super::make_thumbnail_png(png));
            Ok(ClipItem {
                id: row.get(0)?,
                kind,
                text: row.get(2)?,
                image_png,
                thumbnail_png,
                preview: row.get(4)?,
                hash: row.get(5)?,
                created_at: Utc
                    .timestamp_opt(ts, 0)
                    .single()
                    .unwrap_or_else(Utc::now),
                pinned: pinned != 0,
                byte_size: row.get::<_, i64>(8)? as usize,
            })
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    pub fn replace_all(&self, items: &[ClipItem]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM clips", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO clips (id, kind, text, image_png, preview, hash, created_at, pinned, byte_size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for item in items {
                let kind = match item.kind {
                    ClipKind::Text => "text",
                    ClipKind::Image => "image",
                };
                stmt.execute(params![
                    item.id,
                    kind,
                    item.text,
                    item.image_png,
                    item.preview,
                    item.hash,
                    item.created_at.timestamp(),
                    if item.pinned { 1 } else { 0 },
                    item.byte_size as i64,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
