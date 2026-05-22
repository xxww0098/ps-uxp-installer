#!/bin/bash
set -e

cd "$(dirname "$0")"

echo "==> Installing dependencies..."
npm install

echo "==> Building Tauri app..."
npx tauri build "$@"

echo "==> Done. Output in src-tauri/target/release/bundle/"
