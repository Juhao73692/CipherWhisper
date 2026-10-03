#!/bin/sh
# Build a universal, self-contained command-line executable; macOS system libraries only.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --release --locked -p topicairn --target aarch64-apple-darwin
cargo build --release --locked -p topicairn --target x86_64-apple-darwin
mkdir -p dist
lipo -create target/aarch64-apple-darwin/release/topicairn target/x86_64-apple-darwin/release/topicairn -output dist/topicairn
chmod 755 dist/topicairn
codesign --force --sign - --identifier org.topicairn.server dist/topicairn
codesign --verify --strict dist/topicairn
cp docs/macos-testing.md dist/README.zh-CN.md
(cd dist && shasum -a 256 topicairn > SHA256SUMS)
tar -czf dist/topicairn-macos-universal.tar.gz -C dist topicairn README.zh-CN.md SHA256SUMS
file dist/topicairn
ls -lh dist/topicairn dist/topicairn-macos-universal.tar.gz
