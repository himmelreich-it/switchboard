#!/usr/bin/env bash
# Linux counterpart of `install-local`: build the unpacked app (no installer) and install it for this user,
# with a desktop entry and icons so it shows up in the app grid and the dock. No sudo.
set -euo pipefail
cd "$(dirname "$0")/.."

DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
DEST="$DATA/switchboard"
BIN="$HOME/.local/bin"

# Quit the installed copy first, which stops its running agents and terminals. The oldest process is Electron's main
# one; its helpers share the name. Matching the install path leaves other copies (dev, tests) alone.
if pid=$(pgrep -o -f "^$DEST/switchboard( |$)"); then
  kill -TERM "$pid"
  # An open error dialog keeps it from quitting; the server still stops once its stdin closes.
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
  kill -KILL "$pid" 2>/dev/null || true
fi

bun run package:linux
rm -rf "$DEST" && mkdir -p "$DATA"
cp -a dist/linux*-unpacked "$DEST"
rm -rf dist


for n in 16 24 32 48 64 128 256 512; do
  install -Dm644 "images/icons/flat/switchboard-flat-$n.png" "$DATA/icons/hicolor/${n}x${n}/apps/switchboard.png"
done
install -Dm644 images/icons/flat/switchboard-flat.svg "$DATA/icons/hicolor/scalable/apps/switchboard.svg"

# --no-sandbox: Ubuntu 24.04+ blocks the unprivileged user namespaces Chromium's sandbox needs, and the SUID fallback
# wants a root-owned chrome-sandbox. The window only loads our own 127.0.0.1 page; links open in the real browser.
mkdir -p "$BIN" "$DATA/applications"
printf '#!/bin/sh\nexec "%s/switchboard" --no-sandbox "$@"\n' "$DEST" > "$BIN/switchboard"
chmod +x "$BIN/switchboard"
cat > "$DATA/applications/switchboard.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Switchboard
Comment=Run TODOs as Claude Code sessions
Exec="$DEST/switchboard" --no-sandbox
Icon=switchboard
StartupWMClass=switchboard
Categories=Development;
Terminal=false
DESKTOP

update-desktop-database "$DATA/applications" 2>/dev/null || true
echo "Installed $DEST (launcher: Switchboard, terminal: $BIN/switchboard)"
