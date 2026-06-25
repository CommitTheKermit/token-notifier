#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_PATH="$ROOT_DIR/src-tauri/target/release/bundle/macos/Token Notifier.app"
DIST_DIR="$ROOT_DIR/dist"

cd "$ROOT_DIR/src-tauri"
cargo tauri build

cd "$ROOT_DIR"
codesign --force --deep --sign - "$APP_PATH"
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

echo "Packaged and ad-hoc signed: $APP_PATH"

# 배포용 zip 생성 (ditto 로 만들어야 서명/리소스 포크가 보존된다)
VERSION="$(grep -m1 '"version"' "$ROOT_DIR/src-tauri/tauri.conf.json" | sed -E 's/.*"version" *: *"([^"]+)".*/\1/')"
ZIP_PATH="$DIST_DIR/Token-Notifier-${VERSION}.zip"
mkdir -p "$DIST_DIR"
rm -f "$ZIP_PATH"
ditto -c -k --keepParent "$APP_PATH" "$ZIP_PATH"

echo "Distributable zip: $ZIP_PATH"
