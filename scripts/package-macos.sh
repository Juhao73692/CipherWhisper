#!/bin/sh
# Build a universal, self-contained command-line executable; macOS system libraries only.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
npm --prefix apps/local-ui ci
npm --prefix apps/local-ui run check
npm --prefix apps/local-ui run build
export MACOSX_DEPLOYMENT_TARGET=13.0
cargo build --release --locked -p cipherwhisper --target aarch64-apple-darwin
cargo build --release --locked -p cipherwhisper --target x86_64-apple-darwin
mkdir -p dist
lipo -create target/aarch64-apple-darwin/release/cipherwhisper target/x86_64-apple-darwin/release/cipherwhisper -output dist/cipherwhisper
chmod 755 dist/cipherwhisper
codesign --force --sign - --identifier org.cipherwhisper.server dist/cipherwhisper
codesign --verify --strict dist/cipherwhisper
package_guide() {
  sed -e 's|(device-sync.md)|(DEVICES.zh-CN.md)|g' \
      -e 's|(local-ui.md)|(UI.zh-CN.md)|g' \
      -e 's|(p2p-testing.md)|(README.zh-CN.md)|g' \
      -e 's|(macos-testing.md)|(RELAY.zh-CN.md)|g' "$1" > "$2"
}
package_guide docs/p2p-testing.md dist/README.zh-CN.md
package_guide docs/macos-testing.md dist/RELAY.zh-CN.md
package_guide docs/local-ui.md dist/UI.zh-CN.md
package_guide docs/device-sync.md dist/DEVICES.zh-CN.md
cp server/domain/ui/third-party-ui.txt dist/THIRD-PARTY-UI.txt
(cd dist && shasum -a 256 cipherwhisper > SHA256SUMS)
tar -czf dist/cipherwhisper-macos-universal.tar.gz -C dist cipherwhisper README.zh-CN.md RELAY.zh-CN.md UI.zh-CN.md DEVICES.zh-CN.md THIRD-PARTY-UI.txt SHA256SUMS
file dist/cipherwhisper
ls -lh dist/cipherwhisper dist/cipherwhisper-macos-universal.tar.gz
