#!/bin/bash
# Installs WarpTalk.app into /Applications so it opens with no Gatekeeper prompt.
#   Usage: bash install-macos.sh <zip-or-dmg URL | local path>
#   e.g.   curl -fsSL <host>/install-macos.sh | bash -s -- <host>/WarpTalk-0.2.5-macos.zip
# Why: the app is ad-hoc signed (not notarized), so a copy downloaded by a browser is quarantined
# and blocked. curl does not set the quarantine flag, and we clear it anyway for local files.
set -euo pipefail

SRC="${1:-}"
if [ -z "$SRC" ]; then echo "usage: $0 <WarpTalk zip/dmg URL or path>" >&2; exit 2; fi

TMP="$(mktemp -d)"
trap 'hdiutil detach -quiet "$TMP/mnt" 2>/dev/null || true; rm -rf "$TMP"' EXIT

case "$SRC" in
  http://*|https://*) echo "Downloading $SRC"; curl -fL --progress-bar -o "$TMP/pkg" "$SRC" ;;
  *) cp "$SRC" "$TMP/pkg" ;;
esac

if [[ "$SRC" == *.dmg ]]; then
  mkdir "$TMP/mnt"
  hdiutil attach -quiet -nobrowse -readonly -mountpoint "$TMP/mnt" "$TMP/pkg"
  APP_SRC="$TMP/mnt/WarpTalk.app"
else
  ditto -x -k "$TMP/pkg" "$TMP/x"
  APP_SRC="$TMP/x/WarpTalk.app"
fi
[ -d "$APP_SRC" ] || { echo "WarpTalk.app not found in package" >&2; exit 1; }

pkill -x WarpTalk 2>/dev/null || true
DEST="/Applications/WarpTalk.app"
SUDO=""; [ -w /Applications ] || SUDO="sudo"
$SUDO rm -rf "$DEST"
$SUDO ditto "$APP_SRC" "$DEST"
$SUDO xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true

echo "Installed $(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$DEST/Contents/Info.plist") to $DEST"
open "$DEST"
