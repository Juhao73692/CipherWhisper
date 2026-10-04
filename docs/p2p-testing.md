# 无 Relay 的直接 P2P 测试

现在 `serve` 默认运行直接 P2P。两个用户的中心直接交换身份认证的端到端加密消息；不启动 Relay，也不在另一实例内部嵌入 Relay。每个中心有自己的身份、SQLite、ratchet 和本机 UI。

## 一条命令打开两个测试实例

日常跨电脑使用可以双击 CipherWhisper.app，在页面设置地址并自动生成证书，然后交换连接卡添加联系人。参见 [全程 UI 配置](ui-setup.md)。以下命令适合开发和自动化测试。

```sh
./cipherwhisper local-test --open
```

程序自动创建临时 Alice、Bob 身份，启动两个独立 `serve` 子进程，互相导入签名连接卡并验证双向连接，然后打开本机 UI：

- Alice：`http://127.0.0.1:8790/`，直连端口 8800。
- Bob：`http://127.0.0.1:8791/`，直连端口 8801。

在 Alice 选择 Bob → **本机 P2P 测试** → 发送。Bob 收到首条消息后会出现同一话题，可以回复。支持 Markdown、LaTeX、代码高亮、多个话题和独立历史。

命令保持运行。**Ctrl-C 停止双方并删除本次临时测试数据**；测试口令随机生成、仅注入子进程，不写文件或显示。它适合试聊；要长期保存自己的身份和历史，使用下面的独立 `serve` 命令。

端口被占用时可以换端口：

```sh
./cipherwhisper local-test --open --alice-port 8890 --bob-port 8891 \
  --alice-peer-port 8900 --bob-peer-port 8901
```

## 手动启动两个永久实例

两个终端分别设置自己的口令（可以不同）；使用不同数据目录、UI 端口、P2P 端口。

终端 A（zsh）：

```sh
read -rs 'CIPHERWHISPER_PASSPHRASE?Alice 口令（至少 12 bytes）: '; echo
export CIPHERWHISPER_PASSPHRASE
./cipherwhisper serve --data alice --name Alice \
  --bind 127.0.0.1:8790 --peer-bind 127.0.0.1:8800 --sync-seconds 1 --open
```

终端 B：

```sh
read -rs 'CIPHERWHISPER_PASSPHRASE?Bob 口令（至少 12 bytes）: '; echo
export CIPHERWHISPER_PASSPHRASE
./cipherwhisper serve --data bob --name Bob \
  --bind 127.0.0.1:8791 --peer-bind 127.0.0.1:8801 --sync-seconds 1 --open
```

1. 两边点击左下角自己的身份 → **下载身份卡**，得到 `.peer.json`。
2. 两边通过可信渠道核对完整 `td_…` 指纹，导入对方的 `.peer.json`。连接卡包含签名身份、P2P 地址和可选公开 CA，不包含私钥、token 或历史；原 `.contact.json` 没有地址，单独使用不足以配置直连。
3. 点击联系人身份详情 → **测试 P2P 连接**。双方均需导入对方，未知身份不会自动建立信任。
4. 创建话题并发信。UI 只连接本机，Rust 后台直接联系对方。

重启使用原目录和原口令。不要让两个进程打开同一个数据目录。更换地址不更换身份；重新导出/导入签名连接卡即可更新路由。

## 在两台计算机间直连

本机 HTTP 只允许 loopback，消息正文仍然使用 Olm/X25519/Double Ratchet 端到端加密。远程监听和连接必须使用 HTTPS/TLS 1.3，并继续验证签名身份。

例如 Alice 地址 `192.168.1.10`：

```sh
./cipherwhisper tls-init --host 192.168.1.10 --out peer-tls
./cipherwhisper serve --data alice --name Alice --open \
  --peer-bind 0.0.0.0:8800 --peer-url https://192.168.1.10:8800 \
  --peer-tls-cert peer-tls/server.pem --peer-tls-key peer-tls/server-key.pem \
  --peer-ca peer-tls/ca.pem
```

Bob 在自己的机器上使用自身 IP 生成证书、启动服务，再双方交换 `.peer.json`。客户端仅信任连接卡指定的 CA，并验证主机名；签名还绑定地址和 CA。管理/UI 端口始终是 loopback。跨互联网双方需要可达地址/端口，例如 LAN、Tailscale 或端口转发；当前不实现自动 NAT 穿透、发现或公网信令服务。

## 离线、重试与确认

- **首次握手需要对方在线**：直接从对方原子领取签名 prekey。没有第三方离线信箱，不能保证对方离线时建立第一个会话。
- 已建立会话后，对方离线也可发送：不可变密文、明文历史和新 ratchet 状态一起持久化到发送方，再等待重连。重试不重新加密。
- 对方 HTTP 接收接口仅保存发给自己的密文，返回“已接收密文”；它不提供第三方路由或转发。后台解密、校验并原子保存历史/ratchet 后，才产生“已持久化”的签名确认。
- 发送方验证对方身份、请求 nonce 和密文 ID 绑定的签名响应。重试同一密文只插入一次，接收后响应丢失也不会推进 ratchet 两次。送达不等于人已读。
- 外发与入站使用同一个本地 SQLite 的不同连接；入站不等待外发网络锁，双方同时握手/同步不会互锁。
- 加密消息到达接收中心之后，仍可通过原独立 HTTPS 设备协议同步到它的授权客户端。[域内设备同步](device-sync.md) 继续保留。

测试：`python3 scripts/p2p-smoke.py` 以及加 `--tls`，仅启动两个中心进程，覆盖双向并发首次握手、Markdown/回复/话题、签名 ACK、断线本地队列、双方重启和去重。浏览器测试也使用 `local-test` 自动启动两边，验证连接卡导入/导出、真实收发和渲染。
