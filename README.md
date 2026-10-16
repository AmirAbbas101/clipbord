# Clipbord

Fast clipboard history for **Fedora Workstation (GNOME / Wayland)** — the Win+V / macOS-style picker Linux usually lacks.

Built in **Rust** with **GTK4 + libadwaita**. Runs as a **headless background daemon** (no dock icon); the history UI opens only when you press your shortcut.

## Features

- Background daemon watches the clipboard (**text + images**)
- Popup with search, pin, delete, drag-to-reorder, and keyboard navigation
- Opens in the **top-right** of the screen
- **Auto-paste** into the focused input when you pick an item
- Works after **reboot** (discovers GNOME `XAUTHORITY` / Xwayland automatically)
- Image clips: clipboard image data, or a copied path/`file://` to a local image
- Deduping, size caps, SQLite persistence — RAM stays bounded
- No dock icon — capture uses **`xclip`**, not `wl-paste`

---

## Install (from source)

**Fedora Workstation / GNOME only.** One script does everything except the keyboard shortcut.

```bash
git clone <YOUR_REPO_URL> clipbord
cd clipbord
chmod +x install.sh
./install.sh
```

You may be asked for your password once (`pkexec` / `sudo`) for system packages and the `ydotool` service.

### What `./install.sh` does

| Step | What |
|------|------|
| System packages | `gtk4`, `libadwaita`, `xclip`, `xdotool`, `ydotool`, `wtype`, `wl-clipboard`, `gtk4-devel`, `libadwaita-devel`, `gcc`, `pkgconf-pkg-config`, `openssl-devel`, `libX11-devel`, `curl`, `ca-certificates` |
| Rust | Installs [rustup](https://rustup.rs) if `cargo` is missing |
| Build | `cargo build --release` |
| Binary | `~/.local/bin/clipbord` |
| PATH | Ensures `~/.local/bin` is on `PATH` (updates `~/.bashrc` if needed) |
| Auto-paste | Enables `ydotool.service` with a user-accessible socket (`/tmp/.ydotool_socket`) |
| Daemon | `~/.config/systemd/user/clipbord.service` → `enable --now` |
| Session env | Imports `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `XAUTHORITY` |
| Config | Writes/updates `~/.config/clipbord/config.toml` with `auto_paste = true` |
| Dock safety | Removes any Clipbord `.desktop` / autostart entries |

After install the daemon runs in the background and starts again on login.

### Bind a keyboard shortcut (required)

Clipbord has **no app menu entry** on purpose. Bind a custom shortcut once:

1. **Settings → Keyboard → Keyboard Shortcuts → Custom Shortcuts → +**
2. **Name:** `Clipbord`
3. **Command:** `/home/$USER/.local/bin/clipbord toggle`
4. **Shortcut:** `Super+V` (or `Ctrl+Alt+V`)

### Verify

```bash
clipbord status
systemctl --user status clipbord.service
systemctl status ydotool.service
```

1. Copy some text (or an image / screenshot file path).
2. Click into a text field.
3. Press your shortcut → pick an item with **Enter** or click — it should paste into that field.

---

## Usage

| Action | How |
|--------|-----|
| Open history | Your shortcut, or `clipbord toggle` |
| Search | Type in the search box, or press `/` |
| Navigate | `↑` / `↓` |
| Reorder | Drag a row up or down |
| Paste (+ auto-paste) | `Enter` or click · keys `1`–`9` for the first nine |
| Pin / unpin | `P` or the star button |
| Delete | `Delete` / `Backspace` (when search is not focused) |
| Clear unpinned | Header clear button |
| Close | `Esc` |

```bash
clipbord status
clipbord toggle
clipbord clear
systemctl --user restart clipbord.service
systemctl --user stop clipbord.service
systemctl --user start clipbord.service
```

---

## Configuration

```bash
clipbord config-path
# typically: ~/.config/clipbord/config.toml
```

| Key | Default | Purpose |
|-----|---------|---------|
| `max_items` | `50` | History length cap |
| `max_text_bytes` | `524288` | Max text size |
| `max_image_bytes` | `4194304` | Max image size |
| `poll_interval_ms` | `2000` | Clipboard sample rate (clamped ≥1500ms) |
| `dedupe` | `true` | Identical copies bump to top |
| `persist` | `true` | SQLite under `~/.local/share/clipbord/` |
| `auto_paste` | `true` | Paste into focused field after pick |
| `ignore_sensitive` | `true` | Skip password-manager hints |
| `window_width` / `window_height` | `480` / `560` | Popup size |

```bash
systemctl --user restart clipbord.service
```

To disable auto-paste only:

```toml
auto_paste = false
```

---

## How it works (Fedora / GNOME notes)

- **No dock blink:** polling uses `xclip` over Xwayland, never `wl-paste` in a loop.
- **After reboot:** the daemon finds GNOME’s `XAUTHORITY` file under `$XDG_RUNTIME_DIR/.mutter-Xwaylandauth.*` so capture works without a manual service restart.
- **Auto-paste:** after you pick an item, focus returns to the previous window and Clipbord sends Ctrl+V via `ydotool` (preferred) or `xdotool`. Plain `wtype` does **not** work on Mutter.
- **Images:** reads `image/*` from the clipboard when available, or loads a local image if the clipboard holds a path / `file://` URI.

---

## Uninstall

```bash
systemctl --user disable --now clipbord.service
rm -f ~/.local/bin/clipbord
rm -f ~/.config/systemd/user/clipbord.service
rm -f ~/.local/share/applications/io.github.clipbord.Clipbord.desktop
rm -f ~/.config/autostart/io.github.clipbord.Clipbord.desktop
systemctl --user daemon-reload

# optional: stop auto-paste helper and wipe data
# sudo systemctl disable --now ydotool.service
# sudo rm -f /etc/systemd/system/ydotool.service.d/clipbord.conf
# rm -rf ~/.local/share/clipbord ~/.config/clipbord
```

Also remove the custom keyboard shortcut in GNOME Settings.

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `daemon is not running` | `systemctl --user enable --now clipbord.service` |
| Shortcut does nothing | Use full path: `$HOME/.local/bin/clipbord toggle` |
| Empty history / no capture after reboot | Wait a few seconds for Xwayland; if still broken: `systemctl --user restart clipbord.service`. Installer/daemon should set `XAUTHORITY` automatically. |
| Dock blinks every few seconds | Need `xclip` (installer installs it). Do not poll with `wl-paste`. |
| Picks copy but don’t paste into the field | `sudo dnf install xdotool ydotool` then re-run `./install.sh`, or: `sudo systemctl enable --now ydotool.service` and confirm `/tmp/.ydotool_socket` is writable by your user. |
| Images show only as a file path | Copy a real image or a path ending in `.png`/`.jpg`/…; reopen history after the next poll (~2s). |
| High memory | Lower `max_items` / `max_image_bytes` in `config.toml` |

```bash
journalctl --user -u clipbord.service -f
systemctl status ydotool.service
```

---

## License

MIT — see [LICENSE](LICENSE).
