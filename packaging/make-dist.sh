#!/usr/bin/env bash
# Build a tarball other Fedora users can unpack (binary + installer).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(grep '^version' "$ROOT/Cargo.toml" | head -1 | cut -d'"' -f2)"
NAME="clipbord-${VERSION}-fedora"
OUT="$ROOT/dist"
STAGE="$OUT/$NAME"

source "$HOME/.cargo/env" 2>/dev/null || true
cd "$ROOT"
cargo build --release

rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/packaging"
install -m 755 "$ROOT/target/release/clipbord" "$STAGE/bin/clipbord"
cp -a "$ROOT/install.sh" "$ROOT/README.md" "$ROOT/LICENSE" "$ROOT/Cargo.toml" "$STAGE/"
cp -a "$ROOT/packaging/clipbord.service" "$ROOT/packaging/clipbord.system.service" \
  "$ROOT/packaging/clipbord.spec" "$STAGE/packaging/" 2>/dev/null || true
cp -a "$ROOT/packaging/make-dist.sh" "$STAGE/packaging/" 2>/dev/null || true
cp -a "$ROOT/src" "$STAGE/src"

cat >"$STAGE/install-binary.sh" <<'EOF'
#!/usr/bin/env bash
# Install prebuilt clipbord for the current user (no dock icon).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
SYSTEMD_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

echo "Runtime packages (Fedora):"
echo "  sudo dnf install gtk4 libadwaita xclip"
echo


mkdir -p "$BIN_DIR" "$SYSTEMD_DIR"
install -m 755 "$ROOT/bin/clipbord" "$BIN_DIR/clipbord"

# Never install a .desktop file (that puts an icon in the GNOME dock).
rm -f "$PREFIX/share/applications/io.github.clipbord.Clipbord.desktop" \
      "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/io.github.clipbord.Clipbord.desktop"

sed "s|ExecStart=.*|ExecStart=$BIN_DIR/clipbord daemon|" \
  "$ROOT/packaging/clipbord.service" >"$SYSTEMD_DIR/clipbord.service"

systemctl --user import-environment WAYLAND_DISPLAY DISPLAY XDG_RUNTIME_DIR XDG_CURRENT_DESKTOP 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable --now clipbord.service

echo
echo "Installed to $BIN_DIR/clipbord (background daemon, no dock icon)."
echo
echo "Bind a keyboard shortcut:"
echo "  Settings → Keyboard → Keyboard Shortcuts → Custom Shortcuts → +"
echo "  Name:    Clipbord"
echo "  Command: $BIN_DIR/clipbord toggle"
echo "  Shortcut: Super+V"
echo
echo "Check: clipbord status"
EOF
chmod +x "$STAGE/install-binary.sh" "$STAGE/install.sh"

mkdir -p "$OUT"
tar -C "$OUT" -czvf "$OUT/${NAME}.tar.gz" "$NAME"
echo "Created $OUT/${NAME}.tar.gz"
ls -lh "$OUT/${NAME}.tar.gz"
