#!/bin/bash
# Xcode post-build phase: copy the Next.js static export into the app bundle (loaded as warptalk://app/).
#   Source: $WEB_OUT_DIR, default <repo>/apps/web/out  (produced by `npm run build` with next.config `output: 'export'`)
#   Dest:   <app>/Contents/Resources/web
# Without an export the app still builds (it then loads the dev server URL from Settings), unless REQUIRE_WEB_BUNDLE=YES.
set -euo pipefail

SRC="${WEB_OUT_DIR:-${SRCROOT}/../web/out}"
DEST="${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH}/web"

if [ ! -f "${SRC}/index.html" ]; then
  if [ "${REQUIRE_WEB_BUNDLE:-NO}" = "YES" ]; then
    echo "error: no web export at ${SRC} (expected index.html). Run: cd apps/web && npm run build  (next.config: output: 'export')"
    exit 1
  fi
  echo "note: no web export at ${SRC}; app will load the URL from Settings (dev server). Use 'make bundle' to embed the web app."
  rm -rf "${DEST}"
  exit 0
fi

mkdir -p "${DEST}"
rsync -a --delete "${SRC}/" "${DEST}/"
echo "note: bundled web export from ${SRC} ($(find "${DEST}" -type f | wc -l | tr -d ' ') files)"
