#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_PATH="$ROOT_DIR/src-tauri/target/release/bundle/macos/Token Notifier.app"

cd "$ROOT_DIR/src-tauri"
cargo tauri build

cd "$ROOT_DIR"
codesign --force --deep --sign - "$APP_PATH"
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

echo "Packaged and ad-hoc signed: $APP_PATH"
