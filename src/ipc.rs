use crate::config::Config;
use crate::store::{human_bytes, ClipItem, ClipKind, HistoryStore};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Clear,
    /// Snap the current clipboard into history (used when opening the UI).
    Ingest,
    List { query: String },
    Paste { id: String },
    Pin { id: String },
    Remove { id: String },
    /// Move item to an absolute index (drag-and-drop).
    MoveTo { id: String, index: usize },
    /// PNG bytes for an image item (UI thumbnail / preview).
    ImagePng { id: String },
    Stats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipSummary {
    pub id: String,
    pub kind: String,
    pub preview: String,
    pub pinned: bool,
    pub byte_size: usize,
    pub created_at: i64,
    /// Small PNG preview for image clips (base64), if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Err { message: String },
    List { items: Vec<ClipSummary>, count: usize, bytes: usize },
    ImagePng { png_base64: String },
    Stats { count: usize, bytes: usize },
}

pub fn socket_path() -> Result<PathBuf> {
    Ok(Config::runtime_dir()?.join("clipbord.sock"))
}

pub fn request(req: &Request) -> Result<Response> {
    let path = socket_path()?;
    if !path.exists() {
        bail!(
            "clipbord daemon is not running.\nStart it with: systemctl --user enable --now clipbord.service"
        );
    }

    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("connect to {}", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let payload = serde_json::to_string(req)?;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response: Response = serde_json::from_str(line.trim())
        .with_context(|| format!("bad ipc response: {line}"))?;
    match &response {
        Response::Err { message } => bail!("{message}"),
        _ => Ok(response),
    }
}

pub fn ping() -> Result<()> {
    match request(&Request::Ping)? {
        Response::Ok => Ok(()),
        other => bail!("unexpected ping response: {other:?}"),
    }
}

pub struct IpcServer {
    listener: UnixListener,
}

impl IpcServer {
    pub fn bind() -> Result<Self> {
        let path = socket_path()?;
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        let listener =
            UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener })
    }

    pub fn poll(&self, store: &HistoryStore, auto_paste: bool) -> Result<()> {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Err(e) = handle_client(stream, store, auto_paste) {
                        tracing::warn!("ipc client error: {e:#}");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if let Ok(path) = socket_path() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn handle_client(stream: UnixStream, store: &HistoryStore, auto_paste: bool) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(800)))?;
    stream.set_write_timeout(Some(Duration::from_millis(800)))?;
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let req: Request = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            write_response(&stream, &Response::Err { message: e.to_string() })?;
            return Ok(());
        }
    };
    let response = dispatch(store, auto_paste, req);
    write_response(&stream, &response)
}

fn dispatch(store: &HistoryStore, auto_paste: bool, req: Request) -> Response {
    match req {
        Request::Ping => Response::Ok,
        Request::Ingest => {
            let cfg = store.config();
            if let Some(pending) =
                crate::clipboard::watcher_sample_once(cfg.max_text_bytes, cfg.max_image_bytes)
            {
                if let Some(item) = crate::clipboard::pending_to_item(
                    pending,
                    cfg.max_text_bytes,
                    cfg.max_image_bytes,
                ) {
                    let _ = store.push(item);
                }
            }
            Response::Ok
        }
        Request::Clear => {
            store.clear_unpinned();
            Response::Ok
        }
        Request::List { query } => {
            let items = store.search(&query);
            let (count, bytes) = store.stats();
            Response::List {
                items: items.iter().map(summary_of).collect(),
                count,
                bytes,
            }
        }
        Request::Stats => {
            let (count, bytes) = store.stats();
            Response::Stats { count, bytes }
        }
        Request::Pin { id } => {
            if store.toggle_pin(&id) {
                Response::Ok
            } else {
                Response::Err {
                    message: "item not found".into(),
                }
            }
        }
        Request::Remove { id } => {
            if store.remove(&id) {
                Response::Ok
            } else {
                Response::Err {
                    message: "item not found".into(),
                }
            }
        }
        Request::MoveTo { id, index } => {
            if store.move_to_index(&id, index) {
                Response::Ok
            } else {
                Response::Err {
                    message: "cannot move item".into(),
                }
            }
        }
        Request::ImagePng { id } => match store.get(&id) {
            Some(item) => match item.image_png {
                Some(png) => Response::ImagePng {
                    png_base64: base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        png,
                    ),
                },
                None => Response::Err {
                    message: "not an image item".into(),
                },
            },
            None => Response::Err {
                message: "item not found".into(),
            },
        },
        Request::Paste { id } => match store.get(&id) {
            Some(item) => {
                if let Err(e) = crate::clipboard::set_clipboard(&item) {
                    return Response::Err {
                        message: format!("{e:#}"),
                    };
                }
                // Return immediately so the UI can quit and restore focus; paste
                // keystroke runs after a short delay on a background thread.
                if auto_paste {
                    std::thread::spawn(|| {
                        crate::clipboard::try_auto_paste();
                    });
                }
                Response::Ok
            }
            None => Response::Err {
                message: "item not found".into(),
            },
        },
    }
}

fn summary_of(item: &ClipItem) -> ClipSummary {
    // Prefer cached thumbnail bytes (fast UI open); fall back to encode-on-demand.
    let thumbnail_base64 = item
        .thumbnail_png
        .as_ref()
        .map(|png| {
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png)
        })
        .or_else(|| match (&item.kind, &item.image_png) {
            (ClipKind::Image, Some(png)) => make_thumbnail_base64(png),
            _ => None,
        });
    ClipSummary {
        id: item.id.clone(),
        kind: match item.kind {
            ClipKind::Text => "text".into(),
            ClipKind::Image => "image".into(),
        },
        preview: item.preview.clone(),
        pinned: item.pinned,
        byte_size: item.byte_size,
        created_at: item.created_at.timestamp(),
        thumbnail_base64,
    }
}

fn make_thumbnail_base64(png: &[u8]) -> Option<String> {
    let thumb = crate::store::make_thumbnail_png(png)?;
    Some(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        thumb,
    ))
}

fn write_response(mut stream: &UnixStream, response: &Response) -> Result<()> {
    let payload = serde_json::to_string(response)?;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

#[allow(dead_code)]
pub fn format_stats(count: usize, bytes: usize) -> String {
    format!("{count} items · {} in memory · pinned stay when clearing", human_bytes(bytes))
}
