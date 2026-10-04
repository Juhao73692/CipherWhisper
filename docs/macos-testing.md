# 可选旧 Relay 模式：CipherWhisper：两台 Mac 测试指南

当前默认是无 Relay 的直接 P2P，使用 [直接 P2P 指南](p2p-testing.md)。本文仅适用于显式指定 `--relay` 的旧兼容模式。

`cipherwhisper` 是一个 Universal macOS 命令行可执行文件，包含 Apple Silicon arm64 和 Intel x86_64 两个架构，最低 macOS 13。运行时不需要安装 Rust、Node、Homebrew、SQLite、Caddy 或 OpenSSL；它只链接 macOS 系统库。

这份指南测试两个用户的中心端点。若两台 Mac 分别作为同一个可信域的中心和客户端，使用同一个程序的 `serve` / `connect` 模式，按 [域内设备同步指南](device-sync.md) 操作；压缩包内也附带 `DEVICES.zh-CN.md`。

```text
电脑 A：Alice 中心端点 + HTTPS 密文 Relay
电脑 B：Bob 中心端点
Alice ── E2EE ── Relay（A:8787） ── E2EE ── Bob
```

Relay 与 Alice 端点是独立角色，拥有不同存储：Relay 不获得 Bob/Alice 的消息解密密钥。第一轮使用短命的 `admin` 命令操作各自中心端点，Relay 持续运行；这使离线首次发送容易验证。`serve --open` 连续同步并打开内置本机 UI，可渲染 Markdown、LaTeX 和代码。

## 1. 两台电脑准备

把相同的 `cipherwhisper` 文件分别复制到两台电脑各自的工作目录，例如 `~/CipherWhisperTest/`。在终端进入该目录：

```sh
cd ~/CipherWhisperTest
chmod +x cipherwhisper
./cipherwhisper --version
./cipherwhisper --help
```

打包文件 `cipherwhisper-macos-universal.tar.gz` 内有这个可执行文件、这份指南和 SHA256SUMS。可以在解压目录运行 `shasum -a 256 -c SHA256SUMS` 验证文件完整性。可执行文件使用本地 ad-hoc 签名，未做 Apple Developer ID 签名/公证；若下载或 AirDrop 后 macOS 拦截，请在系统“隐私与安全性”中对你已核对来源的程序允许运行。

两台电脑先在同一局域网。确认 A 的局域网 IP，例如 **192.168.1.10**；下文所有这个示例地址都要替换成实际地址。可以通过系统设置查看，或尝试 `ipconfig getifaddr en0`；有线网卡可能是其他接口。Mac 防火墙询问时允许 `cipherwhisper` 接收连接。

## 2. 电脑 A：生成测试 TLS 证书并启动 Relay

在 A 的终端 1：

```sh
cd ~/CipherWhisperTest
./cipherwhisper tls-init --host 192.168.1.10 --out relay-tls
cp relay-tls/ca.pem relay-ca.pem
shasum -a 256 relay-ca.pem
./cipherwhisper relay --bind 0.0.0.0:8787 --database relay.sqlite \
  --tls-cert relay-tls/server.pem --tls-key relay-tls/server-key.pem
```

保持这个终端运行。证书自动包含你给的 IP，以及 localhost、127.0.0.1、::1；有效期一年。`--host` 可重复提供多个 IP/DNS 名。它不会覆盖已有 TLS 目录。`server-key.pem` 只留在 A，权限为 0600；CA 的签名私钥只用于本次签发，不落盘。

把 **relay-ca.pem** 复制到 B 的工作目录，用可信渠道核对 SHA-256。CA 是公钥证书，可以分享，不用导入系统钥匙串。客户端每次通过 `--relay-ca` 明确信任它，同时继续验证主机名/IP 和证书有效期。

B 可以先用系统 curl 检查连通性：

```sh
curl --noproxy '*' --cacert relay-ca.pem https://192.168.1.10:8787/health
```

预期：`{"status":"ok","protocol":1}`。不要用 `curl -k` 绕过验证。

## 3. 两端各自创建身份

A 的终端 2：

```sh
cd ~/CipherWhisperTest
read -rs 'CIPHERWHISPER_PASSPHRASE?Alice 的口令（至少 12 bytes）: '; echo
export CIPHERWHISPER_PASSPHRASE
function ta() {
  ./cipherwhisper admin --data alice --relay https://127.0.0.1:8787 --relay-ca relay-ca.pem "$@"
}
ta init --name Alice > alice.contact.json
cat alice.contact.json
```

B 的终端：

```sh
cd ~/CipherWhisperTest
read -rs 'CIPHERWHISPER_PASSPHRASE?Bob 的口令（至少 12 bytes）: '; echo
export CIPHERWHISPER_PASSPHRASE
function tb() {
  ./cipherwhisper admin --data bob --relay https://192.168.1.10:8787 --relay-ca relay-ca.pem "$@"
}
tb init --name Bob > bob.contact.json
cat bob.contact.json
```

Alice 和 Bob 各自保管独立的口令和数据目录。公开 Contact Card 包含 `user_id` 指纹和公钥；交换 **alice.contact.json** 和 **bob.contact.json**，核对完整 `user_id` 后导入。不要交换或复制 `alice/`、`bob/` 或 `server-key.pem`。

A：

```sh
ta add-peer bob.contact.json
ta publish
```

B：

```sh
tb add-peer alice.contact.json
tb publish
```

预期 publish 输出 `{"published":true}`。Bob 发布 prekeys 后可以退出命令，甚至关机。A 的 Relay 必须继续运行。双方若重新开终端，需要再次设置各自口令和 ta/tb 函数；`init` 不会替换已有身份。

## 4. Alice 在 Bob 离线时发第一条消息

A：把 `bob.contact.json` 的 `user_id` 完整复制到 BOB_ID：

```sh
BOB_ID='td_这里替换为Bob的完整64位十六进制指纹'
TOPIC_ID=$(ta new-topic --peer "$BOB_ID" --title '数学' --id-only)
echo "$TOPIC_ID"
ta send --topic "$TOPIC_ID" --body '你好 Bob，$E=mc^2$'
ta sync
```

`send` 返回本地 `queued` 消息；`sync` 把同一密文 Envelope 发给 Relay，`errors` 应为空。Bob 此时不需要运行中心端点。CLI 输出保留 Markdown 原文；本机 UI 可以渲染。较长内容可通过 `--file message.md` 或 stdin 输入。

## 5. Bob 上线接收、回复

B：

```sh
tb sync
tb topics
```

从 topics 输出中复制“数学”的 `id`（与 A 的 TOPIC_ID 相同）：

```sh
TOPIC_ID='这里替换为数学Topic的UUID'
tb history --topic "$TOPIC_ID"
tb send --topic "$TOPIC_ID" --body '收到，离线发送和公式源码都正常。'
tb sync
```

A：

```sh
ta sync
ta history --topic "$TOPIC_ID"
```

应看到两条消息；Alice 原消息的 delivery 更新为 `delivered`。B 再执行一次 `tb sync` 可更新自己回复的 delivery。ACK 表示中心端点已持久化，由 Relay 报告，不是人已读或对端签名的送达证明。

## 6. 多 Topic、重启和断网验证

A：

```sh
NAS_ID=$(ta new-topic --peer "$BOB_ID" --title 'NAS' --id-only)
ta send --topic "$NAS_ID" --body '这是另一个独立话题。'
ta sync
```

B 执行 `tb sync`、`tb topics`；数学和 NAS 的历史分别查询，不混在一起。重复 sync 不增加重复消息。

停止 A 的 Relay（终端 1 Ctrl-C）。已有会话情况下，A 的终端 2仍可 `ta send`，`ta sync` 会报告网络错误并保留原密文，`ta outbox` 可查看重试状态。重新执行第 2 步的 **relay 命令**，不重建证书；A `ta sync`，B `tb sync`，即可补投。

重新启动电脑/终端时，保留原数据目录、口令、Relay SQLite 和证书；按上述命令再次运行即可继续会话。不要从陈旧快照覆盖 ratchet 数据库。本版没有口令遗失恢复。

## 7. 使用本机 UI 连续聊天

完成 CLI 验证后，在对应电脑带着已设置的口令启动：

A：

```sh
./cipherwhisper serve --data alice --relay https://127.0.0.1:8787 \
  --relay-ca relay-ca.pem --bind 127.0.0.1:8790 --open
```

B：

```sh
./cipherwhisper serve --data bob --relay https://192.168.1.10:8787 \
  --relay-ca relay-ca.pem --bind 127.0.0.1:8790 --open
```

`--open` 会打开本机浏览器并用 90 秒内有效的一次性链接解锁。若无法自动打开，访问 `http://127.0.0.1:8790/` 并粘贴自己数据目录的 `admin.token`；不要交换这个管理令牌。

界面有三栏：联系人、话题、对话。可以在左下角下载公开身份卡，通过联系人 `＋` 导入对方，点话题 `＋` 创建独立对话。支持编写/预览、公式与代码高亮、回复、查看原文、本地搜索、重命名、归档和恢复。按 `⌘/Ctrl + Enter` 发送。已经完成前面的 CLI 身份交换时，直接使用已有联系人和历史即可。刷新页面会锁定并清除内存草稿，历史仍保存在 SQLite。

端口 8790 是各自本机管理 API，自动同步默认每 5 秒一次，不需两台电脑互相访问这个端口。仅此旧模式需要 HTTPS Relay 的 8787 跨电脑可达。域内设备可另行启用，见 [设备同步](device-sync.md)。

同一数据目录的 serve 和 admin 不能同时打开；想用 ta/tb 就先 Ctrl-C 停止 serve，或通过 UI / 本机管理 API 发送。例如 A 的另一个终端：

```sh
curl --noproxy '*' --config - http://127.0.0.1:8790/topics <<EOF_CONFIG
header = "Authorization: Bearer $(cat alice/admin.token)"
EOF_CONFIG
```

所有管理 API 要求数据目录内的 admin.token；不要把 token 发给对方。具体接口详见源码仓库的 docs/api.md。

## 常见问题

- **连接拒绝/超时**：确认 A Relay 运行、实际 IP、8787 端口、防火墙和同一局域网；检查 health。
- **证书错误**：确认 CA 文件 SHA-256、URL 的 IP/DNS 是签发时提供的 host、两台电脑系统时间正确。IP 变化时用新 TLS 目录签发并交换新的公共 CA；聊天身份不变。
- **unknown peer**：两端都需要导入对方 Contact Card，按指纹核对；不要仅凭 label 认人。
- **no prekeys published**：先让 Bob publish，再让 Alice 建立第一次会话。
- **domain is already open**：该电脑上对应数据目录的 serve 尚在运行，先停止或改用本机 API。
- **wrong passphrase**：使用该数据目录创建时的口令；重新开终端后必须重新 export。

打包的是包含本机浏览器 UI 的工程 MVP，不含完整 Federation 或域内设备传输。ARM64 的实际进程测试和 Intel 架构的交叉构建结果会在交付说明中分别注明。
