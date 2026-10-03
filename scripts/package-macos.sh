#!/bin/sh
# Build a universal, self-contained command-line executable; macOS system libraries only.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
npm --prefix apps/local-ui ci
npm --prefix apps/local-ui run check
npm --prefix apps/local-ui run build
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --release --locked -p topicairn --target aarch64-apple-darwin
cargo build --release --locked -p topicairn --target x86_64-apple-darwin
mkdir -p dist
lipo -create target/aarch64-apple-darwin/release/topicairn target/x86_64-apple-darwin/release/topicairn -output dist/topicairn
chmod 755 dist/topicairn
codesign --force --sign - --identifier org.topicairn.server dist/topicairn
codesign --verify --strict dist/topicairn
package_guide() {
  sed -e 's|(device-sync.md)|(DEVICES.zh-CN.md)|g' \
      -e 's|(local-ui.md)|(UI.zh-CN.md)|g' \
      -e 's|(macos-testing.md)|(README.zh-CN.md)|g' "$1" > "$2"
}
package_guide docs/macos-testing.md dist/README.zh-CN.md
package_guide docs/local-ui.md dist/UI.zh-CN.md
package_guide docs/device-sync.md dist/DEVICES.zh-CN.md
cp server/domain/ui/third-party-ui.txt dist/THIRD-PARTY-UI.txt
(cd dist && shasum -a 256 topicairn > SHA256SUMS)
tar -czf dist/topicairn-macos-universal.tar.gz -C dist topicairn README.zh-CN.md UI.zh-CN.md DEVICES.zh-CN.md THIRD-PARTY-UI.txt SHA256SUMS
file dist/topicairn
ls -lh dist/topicairn dist/topicairn-macos-universal.tar.gz
