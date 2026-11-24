#!/usr/bin/env bash
# Build the copilot-core Swift package inputs (macOS only):
#   apps/macos/CopilotCore/CopilotCore.xcframework   universal static lib + header + modulemap
#   apps/macos/CopilotCore/Sources/CopilotCore/CopilotCore.swift   generated UniFFI Swift bindings
# Both belong to the checked-in local Swift package apps/macos/CopilotCore/Package.swift
# (module `CopilotCore`, binary target `CopilotCoreFFI`).
#
# Prereqs: Xcode command line tools, rustup with targets
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
# Usage:  crates/pipeline/copilot-core/scripts/build-xcframework.sh   (or `make core` in apps/macos)
# Env:    MACOSX_DEPLOYMENT_TARGET (default 14.2), PROFILE (default release)
set -euo pipefail

[ "$(uname -s)" = "Darwin" ] || { echo "build-xcframework.sh runs on macOS only" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
OUT="$ROOT/apps/macos/CopilotCore"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
PROFILE="${PROFILE:-release}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.2}"
LIB=libwarptalk_copilot_core.a
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cd "$ROOT"
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build -p warptalk-copilot-core --features ffi --profile "$PROFILE" --lib --target "$t"
done
cargo build -p warptalk-copilot-core --features ffi --profile "$PROFILE" --bin uniffi-bindgen

PDIR="$PROFILE"; [ "$PROFILE" = dev ] && PDIR=debug
mkdir -p "$STAGE/lib" "$STAGE/gen" "$STAGE/headers"
lipo -create \
  "$TARGET_DIR/aarch64-apple-darwin/$PDIR/$LIB" \
  "$TARGET_DIR/x86_64-apple-darwin/$PDIR/$LIB" \
  -output "$STAGE/lib/$LIB"

# Bindings are generated from the arm64 library (metadata is identical across targets).
"$TARGET_DIR/$PDIR/uniffi-bindgen" generate \
  --library "$TARGET_DIR/aarch64-apple-darwin/$PDIR/$LIB" \
  --language swift --out-dir "$STAGE/gen"

cp "$STAGE/gen/warptalk_copilot_coreFFI.h" "$STAGE/headers/"
cp "$STAGE/gen/warptalk_copilot_coreFFI.modulemap" "$STAGE/headers/module.modulemap"

rm -rf "$OUT/CopilotCore.xcframework"
mkdir -p "$OUT" "$OUT/Sources/CopilotCore"
xcodebuild -create-xcframework -library "$STAGE/lib/$LIB" -headers "$STAGE/headers" \
  -output "$OUT/CopilotCore.xcframework"
cp "$STAGE/gen/warptalk_copilot_core.swift" "$OUT/Sources/CopilotCore/CopilotCore.swift"
echo "OK: $OUT/CopilotCore.xcframework and Sources/CopilotCore/CopilotCore.swift"
