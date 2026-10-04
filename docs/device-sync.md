# 可信域内设备同步：同一个软件，两种模式

`cipherwhisper serve` 是可信域中心，`cipherwhisper connect` 是它的设备客户端。两者使用**同一个 macOS Universal 可执行文件、同一个本机聊天界面**；无需安装 Rust、Node 或另一套客户端。

```text
Alice Laptop -- authenticated TLS 1.3 --> Alice center
Alice Desktop -- authenticated TLS 1.3 --> Alice center
                                           |
                              Olm 3DH / Double Ratchet
                                           |
                                        Bob center
```

设备独立持有 Ed25519 私钥。中心持有稳定用户身份和外部 Peer 的加密会话；设备不复制中心私钥或 ratchet。外部联系人仍只看到 Alice 的中心身份，不知道她内部有哪些设备。设备经授权可以读取完整联系人、话题和来往历史，以及代中心添加联系人、创建/修改话题、发送消息；它不能授权或撤销别的设备。

## 在两台 Mac 上测试一个可信域

也可以全程通过界面创建、授权和配对设备，无需生成证书或填写启动参数。见 [UI 配置指南](ui-setup.md)。下面保留命令行流程供开发与自动化使用。

假设中心 Mac 的地址是 `192.168.1.10`，客户端 Mac 能连接该地址的 TCP 8792 端口。下载同一个 `cipherwhisper` 到两台机器；最低 macOS 13，支持 Apple Silicon 和 Intel。中心默认直接 P2P，外部通信与域内设备同步使用独立监听。

### 1. 中心开启设备 HTTPS 端口

在中心 Mac，为实际连接的 IP/DNS 生成证书（地址必须与证书一致）：

```sh
./cipherwhisper tls-init --host 192.168.1.10 --out device-tls
```

已有中心使用**原数据目录和原口令**。退出原 `serve` 后重新启动，添加以下设备参数，并按需要保留原 P2P 地址/证书参数：

```sh
read -rs 'CIPHERWHISPER_PASSPHRASE?中心原口令: '; echo
export CIPHERWHISPER_PASSPHRASE
./cipherwhisper serve --data alice --name Alice \
  --peer-bind 0.0.0.0:8800 --peer-url https://192.168.1.10:8800 \
  --peer-tls-cert device-tls/server.pem --peer-tls-key device-tls/server-key.pem \
  --peer-ca device-tls/ca.pem \
  --device-bind 0.0.0.0:8792 \
  --device-tls-cert device-tls/server.pem \
  --device-tls-key device-tls/server-key.pem \
  --device-ca device-tls/ca.pem \
  --device-url https://192.168.1.10:8792 --open
```

示例在 P2P 和设备两个独立 TLS 监听复用同一中心测试证书，稳定身份签名仍分别验证。另一用户中心通过 `.peer.json` 导入在中心 UI 配置；设备添加的裸联系人卡不包含 P2P 路由，需要中心补充连接卡。中心间直连设置见 [P2P 指南](p2p-testing.md)。旧 Relay 模式也可继续添加设备监听，不会改变设备协议。

本机 UI/Admin 仍只监听 `127.0.0.1:8790`；设备连接的是独立的 HTTPS 8792。不要把管理端口映射到网络。允许 macOS 防火墙上的设备监听端口。

### 2. 客户端创建独立设备身份

在客户端 Mac 设置自己的口令，可以与中心不同：

```sh
read -rs 'CIPHERWHISPER_PASSPHRASE?客户端口令（至少 12 bytes）: '; echo
export CIPHERWHISPER_PASSPHRASE
./cipherwhisper device-init --data laptop --name 'Alice Laptop' > laptop.device.json
```

通过可信渠道将 `laptop.device.json` 交给中心。这只是公开设备卡，包含完整 `dev_…` 指纹和自签名。不要传输 `laptop/` 数据目录、口令、token 或私钥。

### 3. 中心授权并导出配对文件

在中心 UI 点击 **域内设备管理**，选取或粘贴设备卡，核对完整 `dev_…` 指纹，点击 **授权设备并下载配对文件**。下载的 `.pair.json` 绑定设备公钥、中心公开身份、HTTPS 地址、公开 CA 和同步日志 epoch，由中心签名。

把配对文件交给客户端；从中心自己的身份详情取得完整 `td_…` 指纹，通过可信渠道核对。配对文件是公开配置；单独持有它无法冒充设备，设备私钥从不进入文件。

### 4. 客户端连接并打开同一界面

在客户端 Mac：

```sh
./cipherwhisper connect --data laptop \
  --pairing '<下载的 dev_….pair.json>' \
  --trust-domain '<核对后的完整 td_… 指纹>' --open
```

首次配对必须提供完整中心指纹，不自动信任文件内的身份。客户端验证中心签名和自身设备绑定，只信任配对 CA，并继续验证 TLS 主机名。配对信息加密保存在客户端 SQLite 中。

之后重启只需设置同一个客户端口令：

```sh
./cipherwhisper connect --data laptop --open
```

客户端默认每 0.5 秒主动拉取。中心自己的来信、发信、联系人、话题和投递状态都会同步。单条消息连续投递失败 10 次后暂停自动投递，不影响继续拉取来信；暂停消息在聊天区显示红色感叹号，点击会按新消息发送，使用新的消息 ID 和命令 ID。原失败记录继续保留，自动重试始终复用原命令。联系人和话题更新继续按退避间隔同步。点击 **设备同步与队列** 可以查看中心地址、本机/已确认游标、待处理操作及失败原文。

## 同步和断线语义

- 中心是唯一事实源。SQLite 事务内同时记录数据变化和单调递增的日志序号；分页同步使用序号，不用设备时间戳猜测缺失消息。
- 每台设备有独立游标；客户端原子提交整页和游标，再确认 ACK。ACK 响应丢失会重新确认；某台设备 ACK 不删除其他设备所需的历史。
- 新设备从头拉取既有来信和自己发送的历史。消息只存 Markdown 源文，公式、代码和回复保持不变。投递状态及话题修改也是版本化同步数据。
- 每次操作在客户端持久化固定 UUID。中心原子提交操作收据、消息、ratchet 状态和 outbox；客户端超时或重启后重试同一操作，中心返回原收据，不再次加密或插入消息。
- 晚到的操作确认不能覆盖更新的消息状态：每个实体按中心日志版本比较。响应还绑定这次请求 nonce、中心和设备身份，不能直接重放旧响应。
- 客户端可离线阅读已缓存历史，并在已有未归档话题内排队发送。创建话题、添加联系人和修改话题需中心确认；网络结果不确定时已持久化，界面会提示等待重试，**不要重复提交**。
- 两台客户端同时修改话题，中心比较原标题和归档状态；旧版本操作明确拒绝并返回当前版本。被拒绝的消息/修改保留在设备队列，不默默丢弃。复制需要保留的正文后，可删除明确失败的记录，再基于新状态操作；结果不确定的请求不能取消。
- 消息到达中心后使用中心时间，并按中心日志传播；同秒消息在副本中保留中心插入顺序。设备离线草稿的本地排队时间可能早于最终中心接收时间。
- 外部 Peer 离线不影响设备拉取已保存历史。已有 Peer session 时中心仍可排队外发；直接 P2P 首次会话建立需对方在线。

## 撤销、更新和边界

中心 UI 可撤销设备。撤销状态持久化，旧设备密钥永久拒绝重新授权；如需重新加入，使用新的设备数据目录重新 `device-init`。撤销阻止未来读取和发送，无法远程删除曾同步到设备上的历史。设备在被撤销期间排队的内容仍可从本地队列读取。

中心 IP/证书变化时，可对同一有效设备重新导出配对文件，再用 `connect --pairing … --trust-domain …` 导入；只有同一中心密钥和相同 epoch 被接受，保留原游标。客户端还验证新的 CA 和地址。测试证书有有效期，到期前需更新。

中心和客户端数据目录必须分开，不复制中心身份目录作为客户端。没有历史备份恢复/口令轮换；不能恢复陈旧中心或客户端快照继续写入，版本/高水位检查会拒绝可观察的日志回滚，但不能证明未观察的数据从未被回滚。

本版日志、设备操作收据和撤销 tombstone 尚不清理，会随使用增长；不要手动裁剪同步表。未来需实现带设备安全检查点的压缩/重新引导。设备 API 不支持分级权限、远程设备管理、增量删除事件、实时推送或 Federation。已授权设备拥有本可信域内聊天的完整读取/发送权限。

域内加密使用成熟的 **rustls TLS 1.3**（默认密钥交换套件、认证加密、证书校验），另外以 Ed25519 设备签名认证请求，以中心身份签名认证响应。它不是为每台设备另外创建一份外部 Double Ratchet；中心间 E2EE 的算法和边界保持独立。本地历史与索引为明文 SQLite，私钥和配对配置经 Argon2id + XChaCha20-Poly1305 保护，目录 0700 / 数据库 0600。设备自身属于用户的可信域，使用者须保护本机环境。

测试：`cargo test --workspace --locked`、`python3 scripts/device-smoke.py --direct --binary dist/cipherwhisper`、`npm --prefix apps/local-ui run test:browser`。设备 smoke 使用临时目录/动态端口在直接 P2P 模式启动两个中心和两个设备共四个进程，不启动 Relay，结束后自动清理。
