#!/bin/sh
# Build a universal app; use --debug for development builds.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
package_profile=release
if [ "${1:-}" = "--debug" ]; then
  package_profile=debug
elif [ "$#" -ne 0 ]; then
  echo "Usage: $0 [--debug]" >&2
  exit 2
fi
npm --prefix apps/local-ui ci
npm --prefix apps/local-ui run check
npm --prefix apps/local-ui run build
export MACOSX_DEPLOYMENT_TARGET=13.0
if [ "$package_profile" = debug ]; then
  cargo build --locked -p cipherwhisper --bin cipherwhisper --target aarch64-apple-darwin
  cargo build --locked -p cipherwhisper --bin cipherwhisper --target x86_64-apple-darwin
else
  cargo build --release --locked -p cipherwhisper --bin cipherwhisper --target aarch64-apple-darwin
  cargo build --release --locked -p cipherwhisper --bin cipherwhisper --target x86_64-apple-darwin
fi
mkdir -p dist
# Replace executables with fresh files. Rewriting a previously executed inode can
# leave macOS's code-signature cache rejecting the new program on its first run.
lipo -create "target/aarch64-apple-darwin/$package_profile/cipherwhisper" "target/x86_64-apple-darwin/$package_profile/cipherwhisper" -output dist/cipherwhisper.new
chmod 755 dist/cipherwhisper.new
codesign --force --sign - --identifier org.cipherwhisper.server dist/cipherwhisper.new
codesign --verify --strict dist/cipherwhisper.new
mv -f dist/cipherwhisper.new dist/cipherwhisper
mkdir -p dist/CipherWhisper.app/Contents/MacOS
cp dist/cipherwhisper dist/CipherWhisper.app/Contents/MacOS/CipherWhisper.new
mv -f dist/CipherWhisper.app/Contents/MacOS/CipherWhisper.new dist/CipherWhisper.app/Contents/MacOS/CipherWhisper
rm -f dist/CipherWhisper.app/Contents/MacOS/cipherwhisper-runtime
cp apps/cipherwhisper/Info.plist dist/CipherWhisper.app/Contents/Info.plist
codesign --force --sign - --identifier org.cipherwhisper.app dist/CipherWhisper.app
codesign --verify --strict dist/CipherWhisper.app
package_guide() {
  sed -e 's|(device-sync.md)|(DEVICES.zh-CN.md)|g' \
      -e 's|(local-ui.md)|(UI.zh-CN.md)|g' \
      -e 's|(p2p-testing.md)|(README.zh-CN.md)|g' \
      -e 's|(ui-setup.md)|(SETUP.zh-CN.md)|g' \
      -e 's|(macos-testing.md)|(RELAY.zh-CN.md)|g' "$1" > "$2"
}
package_guide docs/p2p-testing.md dist/README.zh-CN.md
package_guide docs/macos-testing.md dist/RELAY.zh-CN.md
package_guide docs/local-ui.md dist/UI.zh-CN.md
package_guide docs/device-sync.md dist/DEVICES.zh-CN.md
package_guide docs/ui-setup.md dist/SETUP.zh-CN.md
cp server/domain/ui/third-party-ui.txt dist/THIRD-PARTY-UI.txt
(cd dist && shasum -a 256 cipherwhisper > SHA256SUMS)
tar -czf dist/cipherwhisper-macos-universal.tar.gz -C dist CipherWhisper.app cipherwhisper README.zh-CN.md RELAY.zh-CN.md UI.zh-CN.md DEVICES.zh-CN.md SETUP.zh-CN.md THIRD-PARTY-UI.txt SHA256SUMS
file dist/cipherwhisper
ls -lh dist/cipherwhisper dist/cipherwhisper-macos-universal.tar.gz
