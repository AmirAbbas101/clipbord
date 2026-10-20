#!/usr/bin/env bash
# Clipbord — one-shot installer for Fedora Workstation (GNOME / Wayland).
#
# Usage:
#   git clone <url> clipbord && cd clipbord
#   chmod +x install.sh && ./install.sh
#
# Installs packages, Rust (if needed), builds, enables the background daemon,
# configures auto-paste (ydotool), and prints the one manual shortcut step.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
SYSTEMD_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
BASHRC="${HOME}/.bashrc"
UID_NUM="$(id -u)"
GID_NUM="$(id -g)"

echo "==> Clipbord installer"
echo "    prefix: $PREFIX"
echo

# ── helpers ──────────────────────────────────────────────────────────────────

run_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  elif command -v pkexec >/dev/null 2>&1; then
    pkexec "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    echo "Need root to install packages (pkexec or sudo)." >&2
    exit 1
  fi
}

have_cmd() { command -v "$1" >/dev/null 2>&1; }

pkg_installed() {
  rpm -q "$1" >/dev/null 2>&1
}

# ── 1) Fedora packages (runtime + build) ─────────────────────────────────────

# Runtime:
#   gtk4 / libadwaita  — history popup
#   xclip              — clipboard capture via Xwayland (no dock blink)
#   xdotool / ydotool  — auto-paste into focused fields (GNOME rejects wtype)
#   wtype              — optional fallback on non-Mutter compositors
#   wl-clipboard       — optional image / Wayland helpers
# Build:
#   *-devel, gcc, pkgconf, openssl, libX11 — compile GTK UI + top-right placement
PACKAGES=(
  gtk4
  libadwaita
  xclip
  xdotool
  ydotool
  wtype
  wl-clipboard
  gtk4-devel
  libadwaita-devel
  gcc
  pkgconf-pkg-config
  openssl-devel
  libX11-devel
  curl
  ca-certificates
)

NEED_INSTALL=()
for pkg in "${PACKAGES[@]}"; do
  if ! pkg_installed "$pkg"; then
    NEED_INSTALL+=("$pkg")
  fi
done

if ((${#NEED_INSTALL[@]} > 0)); then
  echo "==> Installing system packages (may ask for password):"
  printf '    %s\n' "${NEED_INSTALL[@]}"
  run_root dnf install -y "${NEED_INSTALL[@]}"
else
  echo "==> System packages already installed"
fi

# ── 2) Rust toolchain ────────────────────────────────────────────────────────

if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi

if ! have_cmd cargo || ! have_cmd rustc; then
  echo "==> Installing Rust via rustup (non-interactive)…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi

if ! have_cmd cargo; then
  echo "cargo still not on PATH after rustup. Open a new terminal and re-run ./install.sh" >&2
  exit 1
fi

echo "==> Rust: $(rustc --version) / $(cargo --version)"

if ! pkg-config --exists gtk4 libadwaita-1 x11; then
  echo "pkg-config cannot find gtk4 / libadwaita-1 / x11 after package install." >&2
  exit 1
fi

if ! have_cmd xclip; then
  echo "xclip is required (clipboard capture without dock blink)." >&2
  exit 1
fi

# ── 3) Build ─────────────────────────────────────────────────────────────────

echo "==> Building release binary…"
(
  cd "$ROOT"
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env" 2>/dev/null || true
  cargo build --release
)

# ── 4) Install binary ────────────────────────────────────────────────────────

echo "==> Installing $BIN_DIR/clipbord"
mkdir -p "$BIN_DIR" "$SYSTEMD_DIR"
install -m 755 "$ROOT/target/release/clipbord" "$BIN_DIR/clipbord"

# Never install a .desktop launcher — that puts an icon in the GNOME dock.
rm -f "$APP_DIR/io.github.clipbord.Clipbord.desktop" \
  "$AUTOSTART_DIR/io.github.clipbord.Clipbord.desktop" \
  "$HOME/.local/share/applications/io.github.clipbord.Clipbord.desktop"

# ── 5) PATH ──────────────────────────────────────────────────────────────────

if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
  export PATH="$BIN_DIR:$PATH"
fi

if [[ -f "$BASHRC" ]] || [[ ! -e "$BASHRC" ]]; then
  touch "$BASHRC"
  if ! grep -qF 'export PATH="$HOME/.local/bin:$PATH"' "$BASHRC" 2>/dev/null \
    && ! grep -qF "export PATH=\"\$HOME/.local/bin:\$PATH\"" "$BASHRC" 2>/dev/null; then
    {
      echo ""
      echo "# Clipbord / user binaries"
      echo 'export PATH="$HOME/.local/bin:$PATH"'
    } >>"$BASHRC"
    echo "==> Added ~/.local/bin to PATH in ~/.bashrc"
  fi
fi

# ── 6) Auto-paste helper (ydotool) ───────────────────────────────────────────
# GNOME Mutter does not support wtype's virtual-keyboard protocol.
# ydotoold must expose a user-accessible socket for Ctrl+V injection.

if have_cmd systemctl && pkg_installed ydotool; then
  DROP_IN_DIR=/etc/systemd/system/ydotool.service.d
  DROP_IN="$DROP_IN_DIR/clipbord.conf"
  EXPECTED="ExecStart=/usr/bin/ydotoold -p /tmp/.ydotool_socket -P 0660 -o ${UID_NUM}:${GID_NUM}"
  if [[ -f "$DROP_IN" ]] && grep -qF "$EXPECTED" "$DROP_IN" 2>/dev/null; then
    echo "==> ydotool service drop-in already configured"
  else
    echo "==> Enabling ydotool (auto-paste into focused inputs)"
    run_root bash -c "mkdir -p '$DROP_IN_DIR' && cat > '$DROP_IN' <<EOF
[Service]
ExecStart=
$EXPECTED
EOF
systemctl daemon-reload
systemctl enable --now ydotool.service
systemctl restart ydotool.service" || {
      echo "Warning: could not configure ydotool.service (auto-paste may fall back to xdotool)." >&2
    }
  fi
fi

# ── 7) systemd user service (background daemon) ──────────────────────────────

echo "==> Enabling background daemon (no dock icon)"
sed "s|ExecStart=.*|ExecStart=$BIN_DIR/clipbord daemon|" \
  "$ROOT/packaging/clipbord.service" >"$SYSTEMD_DIR/clipbord.service"

# Critical after reboot: DISPLAY + XAUTHORITY (mutter Xwayland cookie).
# The daemon also discovers .mutter-Xwaylandauth.* at runtime if missing.
systemctl --user import-environment \
  WAYLAND_DISPLAY DISPLAY XDG_RUNTIME_DIR XDG_CURRENT_DESKTOP XAUTHORITY 2>/dev/null || true

systemctl --user daemon-reload
systemctl --user disable clipbord.service 2>/dev/null || true
systemctl --user enable --now clipbord.service

# ── 8) Default config (auto_paste on) ────────────────────────────────────────

CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/clipbord"
CONFIG_FILE="$CONFIG_DIR/config.toml"
mkdir -p "$CONFIG_DIR"
if [[ ! -f "$CONFIG_FILE" ]]; then
  cat >"$CONFIG_FILE" <<'EOF'
max_items = 50
max_text_bytes = 524288
max_image_bytes = 4194304
poll_interval_ms = 2000
dedupe = true
persist = true
auto_paste = true
ignore_sensitive = true
window_width = 480
window_height = 560
EOF
  echo "==> Wrote default config: $CONFIG_FILE"
elif grep -qE '^\s*auto_paste\s*=' "$CONFIG_FILE"; then
  sed -i 's/^\s*auto_paste\s*=.*/auto_paste = true/' "$CONFIG_FILE"
else
  echo 'auto_paste = true' >>"$CONFIG_FILE"
fi

# ── 8.5) Fix any GNOME custom shortcuts with unexpanded $USER ────────────────

if have_cmd gsettings; then
  CUSTOM_LIST=$(gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings 2>/dev/null || true)
  if [[ "$CUSTOM_LIST" =~ /org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/ ]]; then
    # Extract path entries
    for path in $(echo "$CUSTOM_LIST" | tr -d "[]'," | tr ' ' '\n'); do
      if [[ -n "$path" ]]; then
        cmd=$(gsettings get "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path" command 2>/dev/null || true)
        if [[ "$cmd" == *"'/home/\$USER/"* ]] || [[ "$cmd" == *"\"/home/\$USER/"* ]] || [[ "$cmd" == *"/home/\$USER/"* ]]; then
          fixed_cmd=$(echo "$cmd" | sed "s|/home/\\\$USER|$HOME|g" | tr -d "'\"")
          echo "==> Fixed GNOME shortcut command with literal \$USER: $path -> $fixed_cmd"
          gsettings set "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path" command "$fixed_cmd" 2>/dev/null || true
        fi
      fi
    done
  fi
fi

# ── 9) Verify ────────────────────────────────────────────────────────────────

sleep 0.6
if ! "$BIN_DIR/clipbord" status >/dev/null 2>&1; then
  echo "Warning: daemon may still be starting. Check: systemctl --user status clipbord.service" >&2
else
  echo "==> Daemon is running"
  "$BIN_DIR/clipbord" status
fi

echo
echo "════════════════════════════════════════════════════════════"
echo "  Clipbord installed successfully."
echo
echo "  • Background daemon: enabled (starts on login, no dock icon)"
echo "  • Capture: xclip / Xwayland (survives reboot via XAUTHORITY)"
echo "  • Images: clipboard image data + local image file paths"
echo "  • Auto-paste: ydotool (+ xdotool fallback) into focused fields"
echo
echo "  Manual step — bind keyboard shortcuts:"
echo
echo "  Settings → Keyboard → Keyboard Shortcuts → Custom Shortcuts → +"
echo "    Name:     Clipbord"
echo "    Command:  $BIN_DIR/clipbord toggle"
echo "    Shortcut: Super+V   (or Ctrl+Alt+V)"
echo
echo "  Optional — touchpad on/off while typing:"
echo "    Name:     Clipbord Touchpad"
echo "    Command:  $BIN_DIR/clipbord touchpad"
echo "    Shortcut: Super+Shift+T   (or any free combo)"
echo
echo "  Then: click an input → Super+V → Enter (pastes into that field)."
echo
echo "  Useful:"
echo "    clipbord status"
echo "    clipbord toggle"
echo "    clipbord touchpad"
echo "    systemctl --user status clipbord.service"
echo "    systemctl --user restart clipbord.service"
echo "    journalctl --user -u clipbord.service -f"
echo "════════════════════════════════════════════════════════════"
