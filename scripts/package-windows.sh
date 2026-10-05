#!/bin/sh
# Cross-build Windows x64 with LLVM-MinGW on macOS or Linux.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
if [ "$#" -ne 0 ]; then
  echo "Usage: LLVM_MINGW=/path/to/llvm-mingw sh $0" >&2
  exit 2
fi
if [ -n "${LLVM_MINGW:-}" ]; then
  PATH="$LLVM_MINGW/bin:$PATH"
  export PATH
fi
for tool in x86_64-w64-mingw32-clang x86_64-w64-mingw32-clang++ x86_64-w64-mingw32-ar llvm-readobj cmake ninja nasm python3; do
  command -v "$tool" >/dev/null || { echo "Missing build tool: $tool (set LLVM_MINGW to your LLVM-MinGW directory)" >&2; exit 1; }
done
if ! rustup target list --installed | rg -qx x86_64-pc-windows-gnullvm; then
  echo "Run: rustup target add x86_64-pc-windows-gnullvm" >&2
  exit 1
fi
npm --prefix apps/local-ui ci
npm --prefix apps/local-ui run check
npm --prefix apps/local-ui run build
export CC_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang
export CXX_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang++
export AR_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-ar
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang
# Statically link toolchain runtimes; require only Windows system DLLs.
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-C target-feature=+crt-static -C link-arg=-static -C link-arg=-Wl,--no-insert-timestamp"
export CMAKE_GENERATOR=Ninja
cargo build --release --locked -p cipherwhisper --target x86_64-pc-windows-gnullvm
python3 - <<'PY'
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
import zipfile

binary = Path('target/x86_64-pc-windows-gnullvm/release/cipherwhisper-desktop.exe')
console_binary = binary.with_name('cipherwhisper.exe')
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1']))
webview = next(p for p in metadata['packages'] if p['name'] == 'webview2-com-sys')
loader = Path(webview['manifest_path']).parent / 'x64/WebView2Loader.dll'
assert loader.is_file(), 'Missing the Windows x64 WebView2 loader'
data = binary.read_bytes()
offset = struct.unpack_from('<I', data, 0x3c)[0]
assert data[:2] == b'MZ' and data[offset:offset + 4] == b'PE\0\0'
assert struct.unpack_from('<H', data, offset + 4)[0] == 0x8664, 'Expected AMD64'
assert struct.unpack_from('<H', data, offset + 24)[0] == 0x20b, 'Expected PE32+'
assert struct.unpack_from('<H', data, offset + 24 + 68)[0] == 2, 'Expected Windows GUI subsystem'
imports = subprocess.check_output(['llvm-readobj', '--coff-imports', str(binary)], text=True)
dlls = sorted(set(re.findall(r'Name: (\S+\.dll)', imports, re.IGNORECASE)))
system = {'advapi32.dll', 'bcrypt.dll', 'bcryptprimitives.dll', 'crypt32.dll', 'dbghelp.dll', 'iphlpapi.dll',
          'kernel32.dll', 'msvcrt.dll', 'ncrypt.dll', 'ntdll.dll', 'ole32.dll',
          'oleaut32.dll', 'secur32.dll', 'shell32.dll', 'ucrtbase.dll', 'user32.dll',
          'userenv.dll', 'version.dll', 'winmm.dll', 'ws2_32.dll'}
assert dlls, 'Cannot verify DLL dependencies'
assert all(d.lower() in system or d.lower() == 'webview2loader.dll' or d.lower().startswith(('api-ms-win-', 'ext-ms-win-'))
           for d in dlls), f'Non-system DLLs must be bundled or linked statically: {dlls}'
guides = {'windows.md': 'README.zh-CN.md', 'ui-setup.md': 'SETUP.zh-CN.md',
          'p2p-testing.md': 'P2P.zh-CN.md', 'device-sync.md': 'DEVICES.zh-CN.md',
          'local-ui.md': 'UI.zh-CN.md', 'macos-testing.md': 'RELAY.zh-CN.md'}
dist = Path('dist')
dist.mkdir(exist_ok=True)
archive = dist / 'cipherwhisper-win_amd64.zip'
with tempfile.TemporaryDirectory(prefix='cipherwhisper-win-', dir=dist) as temp:
    stage = Path(temp) / 'cipherwhisper-win_amd64'
    stage.mkdir()
    shutil.copy2(binary, stage / 'cipherwhisper.exe')
    shutil.copy2(console_binary, stage / 'cipherwhisper-cli.exe')
    shutil.copy2(loader, stage / 'WebView2Loader.dll')
    for source, destination in guides.items():
        text = (Path('docs') / source).read_text(encoding='utf-8')
        for old, new in guides.items():
            text = text.replace(f'({old})', f'({new})')
        (stage / destination).write_text(text, encoding='utf-8')
    shutil.copy2('server/domain/ui/third-party-ui.txt', stage / 'THIRD-PARTY-UI.txt')
    metadata = json.loads(Path('.build-info-state.json').read_text())
    metadata.update(target='x86_64-pc-windows-gnullvm', architecture='amd64', systemDlls=[d for d in dlls if d.lower() != 'webview2loader.dll'], bundledDlls=['WebView2Loader.dll'], webview2RuntimeRequired=True)
    (stage / 'BUILD-INFO.json').write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
    checksums = ''.join(f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n'
                        for p in sorted(stage.iterdir()))
    (stage / 'SHA256SUMS').write_text(checksums, encoding='utf-8')
    pending = Path(temp) / archive.name
    with zipfile.ZipFile(pending, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
        for path in sorted(stage.iterdir()):
            bundle.write(path, f'{stage.name}/{path.name}')
    with zipfile.ZipFile(pending) as bundle:
        assert bundle.testzip() is None, 'ZIP verification failed'
    pending.replace(archive)
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
archive.with_suffix('.zip.sha256').write_text(f'{digest}  {archive.name}\n')
print(f'Windows AMD64 GUI; bundled WebView2Loader.dll; imports: {", ".join(dlls)}')
print(f'{archive.resolve()} ({archive.stat().st_size / 1024 / 1024:.1f} MiB)')
print(f'SHA-256: {digest}')
PY
