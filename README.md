# CipherWhisper

## 项目简介

**Cipher + Whisper**：加密的私语。CipherWhisper 是基于 Rust 的端到端加密 P2P 聊天应用，以独立话题组织交流。

两个 Personal Trust Domain 的中心计算机直接 P2P 通信，使用经过身份认证的 Olm 3DH / Double Ratchet 加密，不需要 Relay。中心保存稳定身份与 SQLite 历史；授权设备通过独立的 HTTPS 协议同步自己的中心，设备不获得中心身份私钥或外部 ratchet。

```text
Alice devices -- authenticated TLS 1.3 --> Alice center
                                             ↕ direct authenticated E2EE
Bob devices   -- authenticated TLS 1.3 --> Bob center
```

双击 macOS 的 **CipherWhisper.app** 或 Windows 的 **cipherwhisper.exe**，即可在应用窗口中创建身份、设置口令、自动生成证书、导入联系人和配对设备。关闭窗口后继续后台运行，可通过 macOS 菜单栏或 Windows 系统托盘重新打开或退出；再次启动会唤起已有窗口。

跨电脑聊天需要双方地址可达，例如 LAN / Tailscale；首次握手时对方需在线。项目目前为 MVP，尚未实现群聊、Federation 和账号恢复，整体尚未经过独立安全审计。

使用指南：[界面配置](docs/ui-setup.md)、[直接 P2P](docs/p2p-testing.md)、[设备同步](docs/device-sync.md)、[Windows](docs/windows.md)。技术文档：[架构](docs/architecture.md)、[协议](docs/protocol.md)、[安全模型](docs/security.md)、[API](docs/api.md)。

## Features

- **身份与端到端加密**：Ed25519 稳定身份、X25519 密钥交换、固定 Peer 身份验证；使用 vodozemac 的 Olm 3DH / Double Ratchet。
- **可靠投递**：签名 one-time / fallback prekey、请求与响应验签、防重放、密文队列、重试和去重；消息解密并持久化后返回签名送达确认，断线时可本地排队。
- **本地密钥与存储**：Argon2id + XChaCha20-Poly1305 密钥保险库；SQLite WAL / FULL，ratchet、outbox 与历史原子保存，进程锁防止并发打开同一目录。
- **话题与消息管理**：独立 Topic、回复、编辑、撤回、置顶、标签、状态和本地 FTS5 搜索。
- **阅读与文件传输**：加密草稿自动保存、持久化未读位置、未读分割线、历史分页和新消息导航；文件经接收方确认后加密分块传输。
- **桌面窗口与后台运行**：应用管理独立窗口，关闭后保留后台；macOS 菜单栏、Windows 系统托盘提供打开、隐藏和退出菜单。macOS 使用系统 WebKit，Windows 使用系统已安装的 WebView2 Runtime。
- **内嵌 UI 与安全渲染**：Svelte、KaTeX、Shiki、DOMPurify 和字体全部内嵌，无 CDN；禁止原始 HTML 执行和远程图片加载，支持 Markdown、公式和代码高亮。
- **设备同步**：`serve` 中心与 `connect` 设备客户端使用独立设备密钥、授权 / 撤销、TLS 1.3、分页日志、每设备游标 / ACK、幂等操作和双向历史同步。
- **兼容与命令行**：保留原数据目录、口令和加密会话；支持 `CIPHERWHISPER_PASSPHRASE`，兼容旧 `TOPICAIRN_PASSPHRASE`。`serve` 默认直接 P2P，仅显式指定 `--relay <URL>` 时使用旧 Relay 适配器。

更多功能见 [聊天功能](docs/chat-features.md) 和 [本机 UI](docs/local-ui.md)；旧 Relay 模式见 [可选 Relay 指南](docs/macos-testing.md)。

## Build

### 环境准备

在项目根目录执行以下命令。构建需要通过 rustup 安装的 **Rust 1.99.0** 和 **Node.js 26.10.0**（含 npm）；版本分别由 [rust-toolchain.toml](rust-toolchain.toml) 和 [.node-version](.node-version) 指定，Rust 与前端依赖由各自的 lockfile 固定。

打包脚本会自动安装前端依赖、检查 Svelte 类型、构建内嵌 UI，再编译 Rust 并生成发行包，无需提前手动构建前端。

### macOS 打包

在 macOS 上构建，需要 Xcode Command Line Tools（未安装时运行 `xcode-select --install`）。首次构建安装两个 Rust 目标：

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

构建 Release 发行包：

```sh
sh scripts/package-macos.sh
```

构建 Debug 开发包：

```sh
sh scripts/package-macos.sh --debug
```

两种模式都生成 **arm64 + x86_64 Universal** 产物，最低支持 macOS 13，并写入相同的 `dist/` 路径；后一次打包会覆盖前一次产物。重新打包前，先退出正在运行的 `dist/CipherWhisper.app`。

| 产物 | 用途 |
| --- | --- |
| `dist/CipherWhisper.app` | 可双击打开的桌面应用 |
| `dist/cipherwhisper` | 包含 UI 与全部功能的独立可执行文件，也支持命令行 |
| `dist/cipherwhisper-macos-universal.tar.gz` | 包含应用、可执行文件及中文指南的发行包 |
| `dist/SHA256SUMS` | 独立可执行文件的 SHA-256 校验值 |

脚本会完成 ad-hoc 签名和签名校验，不进行 Apple 公证。运行时仅依赖系统库，用户无需安装 Rust 或 Node。

### Windows 打包

脚本使用 LLVM-MinGW，在 **macOS / Linux** 上交叉构建 Windows 10 / 11 的 **amd64（x86_64）** Release 发行包。

先准备 [LLVM-MinGW](https://github.com/mstorsjo/llvm-mingw/releases) 工具链，并确保 CMake、Ninja、NASM、Python 3 和 ripgrep 在 `PATH` 中。首次构建安装 Rust 目标：

```sh
rustup target add x86_64-pc-windows-gnullvm
```

指定 LLVM-MinGW 解压目录并打包（路径指向包含 `bin/` 的目录）：

```sh
LLVM_MINGW=/path/to/llvm-mingw sh scripts/package-windows.sh
```

如果 LLVM-MinGW 工具已在 `PATH` 中，也可以直接运行：

```sh
sh scripts/package-windows.sh
```

脚本不接受额外参数，产物为 `dist/cipherwhisper-win_amd64.zip` 和 `dist/cipherwhisper-win_amd64.zip.sha256`。ZIP 包含：

- `cipherwhisper.exe`：无控制台的桌面应用。
- `cipherwhisper-cli.exe`：用于 PowerShell / CMD 的命令行入口。
- `WebView2Loader.dll`：调用 WebView2 所需的加载组件，请保留在可执行文件旁。
- 中文指南、构建信息、开源许可与 `SHA256SUMS`。

Windows 使用系统已安装的 **Microsoft Edge WebView2 Runtime**，发行包不携带浏览器运行时。脚本检查 PE32+ AMD64 架构、GUI 子系统、DLL 依赖和 ZIP 完整性；详细使用说明见 [Windows 指南](docs/windows.md)。

### 本地开发与验证

不打包时，可构建当前平台的开发版本：

```sh
npm --prefix apps/local-ui ci
npm --prefix apps/local-ui run check
npm --prefix apps/local-ui run build
cargo build --workspace --locked
```

启动应用窗口：

```sh
./target/debug/cipherwhisper
```

`cipherwhisper ui --browser` 使用默认浏览器，`cipherwhisper ui --no-open` 不创建窗口或托盘，适合受控测试。Linux 当前保留浏览器界面。

构建后可运行以下检查；集成测试需要允许 localhost TCP 监听，macOS 浏览器测试使用已安装的 Google Chrome：

```sh
npm --prefix apps/local-ui test
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/p2p-smoke.py
python3 scripts/p2p-smoke.py --tls
python3 scripts/device-smoke.py --direct
npm --prefix apps/local-ui run test:browser
```

打包后也可用一条命令启动两个临时 P2P 身份，互相导入连接卡并验证聊天：

```sh
# macOS
./dist/cipherwhisper local-test --open
```

```powershell
# Windows：在解压后的发行包目录中运行
.\cipherwhisper-cli.exe local-test --open
```

两个测试界面分别在 `http://127.0.0.1:8790/` 和 `http://127.0.0.1:8791/`。按 Ctrl+C 停止双方并清理临时数据，详见 [直接 P2P 测试指南](docs/p2p-testing.md)。
