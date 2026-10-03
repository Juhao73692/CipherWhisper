# Topicairn

**Topic + Cairn**：用独立话题组织消息，用可信节点连接个人可信域。

这是一个无界面的 Rust 服务端 MVP。协议端点是两个 Personal Trust Domain 的**中心计算机**，不是域内设备。一个中心端点拥有一个稳定身份，理解 Topic 和 Markdown，保存明文历史；中转 Relay 只保存公钥、路由元数据和密文。

```text
Alice Trust Domain center  ── authenticated E2EE ──  Bob Trust Domain center
                 \             opaque Relay             /
                  └────────── offline queue ────────────┘
```

当前没有桌面客户端、浏览器客户端、消息渲染、域内设备同步、多设备、群聊或 Federation。管理 API 仅供本机管理，不是设备通信协议。

## 已实现

- 本地身份：Ed25519 签名身份、X25519 密钥交换公钥、签名 Contact Card、指纹固定；地址与身份分离。
- 离线首次发送：签名 fallback prekey + Relay 原子领取 one-time prekey；使用 vodozemac Olm 3DH 和 Double Ratchet。
- 私钥/ratchet 状态：Argon2id 口令派生密钥 + 随机 nonce 的 XChaCha20-Poly1305 加密，绑定状态记录。
- SQLite：peers、topics、messages、identity、sessions、outbox、接收去重、待 ACK，FTS5 仅本地检索。
- Markdown/LaTeX/code 的源文完整传输及存储，不解析、不执行、不渲染 HTML。
- Topic 创建、独立历史、回复引用、标题/归档的加密事件；解密后校验 Peer 与 Topic 所属关系。
- 持久化离线队列、游标分页、幂等发送、接收去重、持久化后 ACK、送达状态、指数退避重试。
- 无界面的中心端点服务、密文 Relay、管理 CLI；两个中心端点共用一个简单 Relay。

## 单文件 macOS 版本

运行 `./scripts/package-macos.sh` 构建 `dist/topicairn`：Universal arm64 + x86_64，最低 macOS 13，只需要 macOS 系统库。一个可执行文件包含 `relay`、`serve`、`admin`、`tls-init`，无需用户安装 Rust、Node 或反向代理。打包步骤会生成本地 ad-hoc 签名、SHA-256 校验和及附带使用指南的 tar.gz。

两台电脑的完整操作步骤见 [macOS 测试指南](docs/macos-testing.md)。Relay 可使用内置 HTTPS，`tls-init` 生成测试 CA 和服务器证书；中心端点使用 `--relay-ca ca.pem` 信任该 CA，继续验证证书和主机名。测试 TLS 私钥不会进入 Git。

## 工具版本

2026-10-03 核对并安装的最新稳定工具：Rust/Cargo **1.99.0**（Rust 2024 edition），rustup **1.29.1**，Node **26.10.0 Current**，npm **12.2.0**，pnpm **12.8.1**。

当前服务端只需要 Rust。`rust-toolchain.toml` 固定工具链，`Cargo.lock` 固定完整依赖。核心依赖：vodozemac 0.11.1、Axum 0.8.9、Tokio 1.53.1、reqwest 0.13.5、rusqlite 0.40.2、Argon2 0.6.0、chacha20poly1305 0.11.0。Svelte/Tauri/Vite 不属于当前交付范围。

## 构建与验证

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
python3 scripts/smoke.py
python3 scripts/smoke.py --binary target/debug/topicairn --tls
```

测试需要允许 localhost TCP 监听。`smoke.py` 使用临时数据目录和动态端口，启动 Alice 中心端点、Bob 中心端点、Relay 三个真实进程，验证离线首次发送、双 Topic、ACK、三方重启、回复及网络恢复；结束时停止全部测试进程并删除测试数据。

## 运行 Relay

```sh
./target/debug/topicairn-relay --bind 127.0.0.1:8787 --database relay.sqlite
```

Relay 默认以 HTTP 只监听 loopback。提供 `--tls-cert` 和 `--tls-key` 时，可以使用内置 HTTPS 监听局域网地址；也可保留 loopback HTTP 并在前面配置 HTTPS 反向代理。中心端点对非 loopback Relay 强制 HTTPS。中心端点管理 API 只监听本机。

## 两个中心端点示例

下面的命令用于本机验证。生产时 Alice、Bob 在各自中心计算机上持有自己的数据目录和口令。公共 Contact Card 通过你信任的渠道交换，核对完整 `user_id` 指纹，再导入。

在 zsh 中以隐藏输入设置口令；每个端点可使用不同口令：

```sh
read -rs 'TOPICAIRN_PASSPHRASE?Domain passphrase: '; echo
export TOPICAIRN_PASSPHRASE
```

口令至少 12 bytes；使用足够强的独立口令。不要把真实口令写入命令行、仓库、shell history 或日志。暂不支持恢复/备份/口令轮换。

```sh
./target/debug/topicairn-cli --data alice init --name Alice > alice.contact.json
./target/debug/topicairn-cli --data bob init --name Bob > bob.contact.json
./target/debug/topicairn-cli --data alice add-peer bob.contact.json
./target/debug/topicairn-cli --data bob add-peer alice.contact.json
./target/debug/topicairn-cli --data alice publish
./target/debug/topicairn-cli --data bob publish
```

Bob 发布 prekeys 后可以完全离线，Alice 仍可建立第一次会话。新 Topic 是本地创建的；第一次消息会把必要 Topic 信息放在密文中交给 Bob。

```sh
./target/debug/topicairn-cli --data alice new-topic --peer '<Bob 的 user_id>' --title '数学'
./target/debug/topicairn-cli --data alice send --topic '<topic id>' --file message.md
./target/debug/topicairn-cli --data alice sync
# 此时可以退出 Alice，Bob 后续启动即可收取：
./target/debug/topicairn-cli --data bob sync
./target/debug/topicairn-cli --data bob history --topic '<topic id>'
./target/debug/topicairn-cli --data alice sync  # 更新送达状态
```

使用已建立的会话时，Relay 离线也能把消息加密排入本地 outbox。首次建立会话需要 Relay 可用、对方预先发布公钥。`send` 只做可靠本地入队，`sync` 或运行中的服务负责投递。

## 长期运行中心端点

分别在两个终端启动，或在各自计算机上运行：

```sh
./target/debug/topicairn-domain --data alice --bind 127.0.0.1:8790 --relay http://127.0.0.1:8787
./target/debug/topicairn-domain --data bob --bind 127.0.0.1:8791 --relay http://127.0.0.1:8787
```

首次直接运行服务可以加 `--name Alice` 创建身份。服务自动发布 prekeys、轮询队列、投递、重试和 ACK。启动后生成数据目录内权限为 0600 的 `admin.token`；所有管理 API 均要求 `Authorization: Bearer <token>`。状态错误可从 `/status` 查看，服务日志不输出消息正文和密钥。

服务运行期间使用管理 API，不要并发打开同一个目录的 CLI。文件锁防止两个进程同时修改身份或 ratchet。

```sh
curl --noproxy '*' --config - http://127.0.0.1:8790/identity <<EOF_CONFIG
header = "Authorization: Bearer $(cat alice/admin.token)"
EOF_CONFIG
```

使用 `--config -` 避免把 token 放在 curl 的进程参数中。Topic、消息、搜索接口见 [管理 API](docs/api.md)。`--sync-seconds` 可设为 1..300，默认 5 秒。Ctrl-C 会优雅关闭。

## 模块边界

| 目录 | 职责 |
|---|---|
| `crates/protocol` | 版本、Contact Card、签名 prekeys、HTTP 认证、Envelope、加密 Payload/Event types |
| `crates/core` | 中心端点的身份、E2EE、密钥保险库、SQLite、Topic、outbox/ACK、Relay client |
| `server/relay` | 只处理公钥和 opaque ciphertext 的持久化路由服务 |
| `server/domain` | 中心端点守护进程及 loopback 管理 API |
| `apps/cli` | 中心端点的无界面管理工具 |
| `apps/topicairn` | 统一单文件入口和测试 TLS 证书生成 |

更多说明：[架构](docs/architecture.md)、[协议](docs/protocol.md)、[安全模型](docs/security.md)、[接口](docs/api.md)。

这是工程 MVP，项目本身尚未经过独立安全审计。库审计不等于组合协议已被审计。对外部署前需审查协议组合、速率限制、容量和运维策略。
