# Personal Trust Domain 中心端点架构

一个中心端点代表一个用户身份和一个 Personal Trust Domain。域内授权设备/同步历史属于独立 Local Trust 协议，不进入外部 Peer 协议。中心和设备客户端由同一软件运行，分别使用本机 SQLite 和 loopback API/UI；中心另外提供认证设备 HTTPS 接口。静态资产由 Rust 内嵌，运行时无需 Node；消息渲染不进入外部协议/Relay 层。

## 四层

1. **Local Trust**：本机管理 token、目录权限、数据库、服务生命周期；独立设备身份、中心授权/撤销、TLS 1.3 连接和增量同步。管理接口仅 loopback。
2. **Identity**：Ed25519 身份指纹、签名 Contact Card、固定 X25519 公钥和签名 prekeys。不存在由 Relay 替你决定 Peer 身份的隐式 TOFU。
3. **Peer Transport**：签名 Envelope、Olm 3DH/Double Ratchet、直接 P2P、本地密文队列、幂等发送、重试、去重和签名送达确认。旧 Relay 是显式可选适配器。
4. **Conversation**：Topic、Markdown 源码、reply、topic.update。全部在加密 Payload 中。

## 第一阶段拓扑

```text
Alice center <-> direct authenticated E2EE <-> Bob center
```

`serve` 默认直连，无 Relay。对端独立 P2P 监听只接受固定 Peer 发给自己的密文，不转发第三方消息。首次握手需双方在线；已有会话可在发送方本地排队。远程仅 HTTPS/TLS 1.3，loopback 测试允许 HTTP。双方通过签名 `.peer.json` 交换身份、地址和可选 CA；身份不依赖地址。所有密钥交换的数学实现、会话消息密钥派生、ratchet 和随机数生成来自成熟库。

## 事务与崩溃恢复

发送事务：加载持久化 session → 在临时 session 上 encrypt → 保存新 session + 完整不可变 Envelope + 本地消息 → SQLite commit。网络发送发生在提交之后。失败重试复用完全相同的 Envelope，不重新 encrypt。

接收事务：验证固定 Peer 签名和收件人 → 从已保存状态加载临时 account/session → decrypt → 校验 Payload 与外层路由绑定 → 校验 Topic 所属、消息 ID、reply → 保存新 account/session + 消息 + 接收去重记录 + 待 ACK → commit。之后才能签名返回 `acknowledged:true`。旧 Relay 模式则在提交后向 Relay ACK。

验证或提交失败时，临时状态被丢弃，原 ratchet 不变。直连接收接口先持久化密文，后台再解密；只入队不等于已解密持久化。提交后确认丢失，重复投递命中 Envelope ID + digest 去重并重新返回签名确认。发送方保留原密文直至验证确认，接收方删除已处理密文并保留摘要 tombstone。

入站使用同一 SQLite 的独立连接，不持有外发 Endpoint mutex、不发起网络请求、不推进 ratchet；双方同时领取 prekeys、发信或同步时可以独立回答入站请求，避免互相等待。所有 ratchet 变化仍由唯一 Endpoint writer 管理。

SQLite 使用 WAL、FULL synchronous、foreign_keys 和本地单写者文件锁。正常进程崩溃/重启保持事务一致性；从陈旧数据库备份回滚是另一种攻击/运维问题，本版不支持恢复旧 ratchet 快照。

## Session 管理

每个 Peer 可以有多个并行 session，以处理双方同时首次发送。密文容器携带 session_id；按确切 session 解密。每个 Peer 最多 32 个 session。双方掌握同一组 session 后，发送按 session_id 字典序选取，避免无休止切换。接收窗口大小和 skipped message keys 由 vodozemac 管理；不是无限乱序缓存。

## Topic

Topic 创建先发生在本地。首条 Message 的加密 Event 包含 topic_id、标题和创建时间，使对端无需先收到单独 topic.create 才能接收。消息只能落到同一 Peer 的 Topic。消息事件不覆盖已存在的 Topic 标题；显式 topic.update 负责标题和归档。

当前 Topic 更新时间是秒精度的发送时间，topic.update 使用时间比较，不是 CRDT；同时改名的完美收敛策略、编辑/删除/已读事件及其冲突语义留待后续协议版本。回复支持目标晚于回复到达；目标后来到达时仍校验同 Topic。显示顺序按 timestamp 和本地插入顺序，不承诺跨端同秒消息的总排序。

## 域内设备副本

`serve` 唯一持有外部身份和 ratchet，`connect` 持有自己的设备签名私钥。设备卡由中心本机 UI 授权；中心签名配对配置绑定自身身份、设备和 TLS CA。设备拉取采用中心事务内触发器写入的物化日志，序号独立于时间戳。设备请求和响应均有身份签名，域内数据只经 TLS 1.3 传输。

每设备游标/ACK 独立。副本在事务内应用整页和游标，完成后才 ACK；ACK 不删除共享历史。新设备可从头同步 incoming 和 SENT。命令固定 ID，中心的操作收据与数据、ratchet、outbox 原子提交；重试取原结果。实体版本防止晚到收据倒退投递状态；客户端话题修改带原标题/归档状态做冲突检查。外部 Peer 的同时修改仍沿用原 topic.update 的时间比较语义。

`Workspace` 统一 center/replica 的本机 API，使 UI 的联系人/话题/消息/搜索在两种模式下复用。设备的明文缓存和待处理操作留在自己的 SQLite；未知网络结果保留重试，明确拒绝的结果可查看并手动删除。中心同步日志和收据尚未压缩，不能恢复旧快照。详见 [设备同步](device-sync.md)。

## 未来演进

之后可替换 Peer Transport 的队列适配器为：

```text
Alice center -> Alice home relay -> Bob home relay -> Bob center
```

Device/Admin Protocol 与 Federation Protocol 分开；中转仍是 opaque ciphertext。路由地址独立于 Contact Card。身份迁移、Federation 和跨中心 SENT 密文存储仍需设计；当前设备通过域内加密连接取得自己中心的来往历史，不复制中心长期 private key。
