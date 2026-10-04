# CipherWhisper

**Cipher + Whisper**：加密的私语。端到端加密的 P2P 聊天，以独立话题组织交流。

Rust MVP：两个 Personal Trust Domain 的中心计算机**直接 P2P**，使用经过身份认证的 Olm 3DH / Double Ratchet 端到端加密，不需要 Relay。中心拥有自己的稳定身份、SQLite 历史与本机聊天 UI；授权设备通过独立 HTTPS 协议同步自己的中心。

```text
Alice devices -- authenticated TLS 1.3 --> Alice center
                                             ↕ direct authenticated E2EE
Bob devices   -- authenticated TLS 1.3 --> Bob center
```

## 一条命令测试两个实例

日常使用可以双击发行包中的 **CipherWhisper.app**，全程在 UI 创建身份、设置口令、自动生成证书、导入联系人和配对自己的设备。后续地址、端口和证书变更也在「连接与证书设置」完成；无需填写启动参数。见 [纯 UI 配置指南](docs/ui-setup.md)。程序无参数启动也会打开配置向导。

下载或构建同一个 macOS Universal 可执行文件，在它所在目录运行：

```sh
./cipherwhisper local-test --open
```

自动启动 Alice 和 Bob 两个中心，互相导入签名连接卡并验证连接。UI 分别在 `http://127.0.0.1:8790/`、`http://127.0.0.1:8791/`；P2P 端口为 8800、8801。在 Alice 选择 Bob → **本机 P2P 测试**，发送消息；Bob 收到后可以回复。没有第三个中转服务。

Ctrl-C 停止双方并删除临时测试数据。永久身份、独立启动、修改端口和两台电脑的 TLS 设置见 [直接 P2P 测试指南](docs/p2p-testing.md)。跨电脑需双方可达地址，例如 LAN/Tailscale；首次握手需对方在线。已有会话可在本地排队，重连后自动投递。

## 已实现

- Ed25519 稳定身份、X25519 椭圆曲线交换、固定 Peer 身份验证；Olm 3DH / Double Ratchet 来自 vodozemac。
- 直接领取签名 one-time/fallback prekey；签名请求与响应、防重放、密文队列、重试、去重、解密持久化后的签名送达确认。
- 密钥保险库：Argon2id + XChaCha20-Poly1305；SQLite WAL/FULL，ratchet、不可变 outbox 和历史原子保存，进程锁防止并发打开同一目录。
- 多个独立 Topic、Markdown 源码、回复、标题/归档、本地 FTS5 搜索。
- 内嵌 Svelte UI，KaTeX、Shiki、DOMPurify；无 CDN，禁止原始 HTML 执行和远程图片加载。
- 同一软件的 `serve` 中心与 `connect` 设备客户端：独立设备密钥、授权/撤销、TLS 1.3、分页日志、每设备游标/ACK、幂等操作、incoming/SENT 历史同步。

设备客户端不获得中心身份私钥或外部 ratchet。外部 Peer 只看到中心身份。详见 [设备同步](docs/device-sync.md) 和 [本机 UI](docs/local-ui.md)。

## 改名与兼容

项目名为 **CipherWhisper**，程序命令为 `cipherwhisper`。继续使用原数据目录和口令即可保留身份、历史、Peer 连接卡及加密会话。新环境变量为 `CIPHERWHISPER_PASSPHRASE`，也接受原 `TOPICAIRN_PASSPHRASE`；两者都设置时优先使用新变量。签名 domain separators 与本地密钥保险库 AAD 保留原 v1 字节，不随产品名变化；旧版端点和设备协议继续兼容。

## macOS 单文件

```sh
./scripts/package-macos.sh
./dist/cipherwhisper local-test --open
```

产物 `dist/cipherwhisper` 包含全部 UI 与功能，Universal arm64 + x86_64，最低 macOS 13，仅依赖系统库；用户不需安装 Rust/Node。打包生成 ad-hoc 签名、SHA-256 与附指南的 `dist/cipherwhisper-macos-universal.tar.gz`，没有 Apple 公证。

同一发行包包含可双击打开的 `dist/CipherWhisper.app`。配置默认保存到用户应用数据目录；原命令行功能继续保留。

`serve` 默认直接 P2P。旧 Relay 适配器保留为显式可选兼容模式：只有指定 `serve --relay <URL>` 才启用；不会默认启动或自动回退。旧方式见 [可选 Relay 指南](docs/macos-testing.md)。Federation、群聊、附件、账号恢复仍未实现。

## 构建和验证

工具版本在 `rust-toolchain.toml`、`.node-version` 和两个 lockfile 中固定。

```sh
npm --prefix apps/local-ui ci
npm --prefix apps/local-ui run check
npm --prefix apps/local-ui test
npm --prefix apps/local-ui run build
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/p2p-smoke.py
python3 scripts/p2p-smoke.py --tls
python3 scripts/device-smoke.py --direct
npm --prefix apps/local-ui run test:browser
```

测试需要 localhost TCP 监听。P2P smoke 只启动两个中心，覆盖双向并发首次握手、Topic/Markdown/回复、签名 ACK、断线排队、双方重启和去重。设备 smoke 在直连模式启动两个中心和两个设备，验证独立游标、幂等发送、断线恢复和永久撤销。浏览器测试验证真实加密聊天、连接卡、渲染和设备管理；进程及临时数据自动清理。旧 Relay 回归用例也保留。

## 模块

| 目录 | 职责 |
|---|---|
| `crates/protocol` | 身份、prekeys、签名 Envelope、Peer HTTP / Device 协议、加密 Event |
| `crates/core` | 加密会话、密钥保险库、SQLite、直连传输、本地队列、同步日志和设备副本 |
| `server/domain` | 中心/客户端模式、loopback 管理 API/UI、独立 P2P 与设备监听 |
| `apps/local-ui` | Svelte UI、安全 Markdown/LaTeX/代码渲染与浏览器测试 |
| `apps/cipherwhisper` | 统一单文件入口、两个实例测试、TLS 证书生成 |
| `apps/cli` | 中心端点无界面管理工具 |
| `server/relay` | 显式可选的旧密文中转适配器 |

更多：[架构](docs/architecture.md)、[协议](docs/protocol.md)、[安全模型](docs/security.md)、[API](docs/api.md)。项目整体尚未经过独立安全审计；库审计不等于组合协议已被审计。
