#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
target="${1:-aarch64-apple-darwin}"
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x64 ;;
  *) echo "Unsupported target: $target" >&2; exit 1 ;;
esac
npm --prefix apps/web run build
cargo build --locked --release --target "$target" -p overleaf-service --bin overleaf-service-api
cd apps/desktop
npm run tauri -- build --target "$target" --bundles app --config '{"bundle":{"active":true,"targets":["app"]}}'
cd ../..
app="target/$target/release/bundle/macos/Overleaf 账号控制台.app"
cp "target/$target/release/overleaf-service-api" "$app/Contents/MacOS/"
cp -R chrome_extension "$app/Contents/MacOS/"
codesign --force --deep --sign - "$app"
mkdir -p target/desktop-package
ditto -c -k --sequesterRsrc --keepParent "$app" "target/desktop-package/overleaf-macos-$arch-portable.zip"
