#!/usr/bin/env bash
# Linux counterpart of `install-local`: build the unpacked app (no installer) and install it for this user,
# with a desktop entry and icons so it shows up in the app grid and the dock. No sudo.
# --beta installs "Switchboard Beta" next to it, from this checkout (a worktree, say): same data in ~/.switchboard, own
# name, folder, launcher and icon. Only one of the two runs at a time (electron/main.js).
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "${1:-}" = "--beta" ]; then
  ID=switchboard-beta NAME="Switchboard Beta" ICONS=images/beta-icons/flat/switchboard-beta-flat
else
  ID=switchboard NAME=Switchboard ICONS=images/icons/flat/switchboard-flat
fi
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
DEST="$DATA/$ID"
BIN="$HOME/.local/bin"

# Quit the installed copy first, which stops its running agents and terminals. The oldest process is Electron's main
# one; its helpers share the name. Matching the install path leaves other copies (dev, tests) alone.
if pid=$(pgrep -o -f "^$DEST/$ID( |$)"); then
  kill -TERM "$pid"
  # An open error dialog keeps it from quitting; the server still stops once its stdin closes.
  for _ in $(seq 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
  kill -KILL "$pid" 2>/dev/null || true
fi

# A fresh worktree has no node_modules yet; the app runs its server from the copied ones.
[ -d node_modules ] || bun install
if [ "$ID" = switchboard-beta ]; then
  # The channel goes into the packaged package.json; the app reads it at start. The package name sets the window class.
  bunx electron-builder --linux --dir -c.productName="$NAME" -c.appId=dev.himmelreich.switchboard.beta \
    -c.linux.executableName="$ID" -c.extraMetadata.name="$ID" -c.extraMetadata.productName="$NAME" -c.extraMetadata.channel=beta
else
  bun run package:linux
fi
rm -rf "$DEST" && mkdir -p "$DATA"
cp -a dist/linux*-unpacked "$DEST"
rm -rf dist


for n in 16 24 32 48 64 128 256 512; do
  install -Dm644 "$ICONS-$n.png" "$DATA/icons/hicolor/${n}x${n}/apps/$ID.png"
done
install -Dm644 "$ICONS.svg" "$DATA/icons/hicolor/scalable/apps/$ID.svg"

# --no-sandbox: Ubuntu 24.04+ blocks the unprivileged user namespaces Chromium's sandbox needs, and the SUID fallback
# wants a root-owned chrome-sandbox. The window only loads our own 127.0.0.1 page; links open in the real browser.
mkdir -p "$BIN" "$DATA/applications"
printf '#!/bin/sh\nexec "%s/%s" --no-sandbox "$@"\n' "$DEST" "$ID" > "$BIN/$ID"
chmod +x "$BIN/$ID"
cat > "$DATA/applications/$ID.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$NAME
Comment=Run TODOs as Claude Code sessions
Exec="$DEST/$ID" --no-sandbox
Icon=$ID
StartupWMClass=$ID
Categories=Development;
Terminal=false
DESKTOP

update-desktop-database "$DATA/applications" 2>/dev/null || true
echo "Installed $DEST (launcher: $NAME, terminal: $BIN/$ID)"
