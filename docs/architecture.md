# Personal Trust Domain 中心端点架构

一个中心端点代表一个用户身份和一个 Personal Trust Domain。域内部如何授权设备、同步历史、存储或备份不属于本项目的网络协议。当前端点使用本机 SQLite 和本机管理 API；CLI 是管理工具。

## 四层

1. **Local Trust**：中心计算机的管理 token、目录权限、数据库、服务生命周期。管理接口仅 loopback。未来域内设备协议在这一层独立设计。
2. **Identity**：Ed25519 身份指纹、签名 Contact Card、固定 X25519 公钥和签名 prekeys。不存在由 Relay 替你决定 Peer 身份的隐式 TOFU。
3. **Peer Transport**：签名 Envelope、Olm 3DH/Double Ratchet、Relay Queue、游标、幂等发送、重试、去重、ACK。
4. **Conversation**：Topic、Markdown 源码、reply、topic.update。全部在加密 Payload 中。

## 第一阶段拓扑

```text
Alice center -> shared ciphertext relay <- Bob center
```

双方中心端点均主动连接 Relay，解决 NAT/离线排队。Relay 不是任一方的解密端点。所有密钥交换的数学实现、会话消息密钥派生、ratchet 和随机数生成来自成熟库。

## 事务与崩溃恢复

发送事务：加载持久化 session → 在临时 session 上 encrypt → 保存新 session + 完整不可变 Envelope + 本地消息 → SQLite commit。网络发送发生在提交之后。失败重试复用完全相同的 Envelope，不重新 encrypt。

接收事务：验证固定 Peer 签名和收件人 → 从已保存状态加载临时 account/session → decrypt → 校验 Payload 与外层路由绑定 → 校验 Topic 所属、消息 ID、reply → 保存新 account/session + 消息 + 接收去重记录 + 待 ACK → commit。然后才 ACK Relay。

验证或提交失败时，临时状态被丢弃，原 ratchet 不变。接收端提交后在 ACK 前崩溃，重投递命中去重表并重新 ACK。发送成功但响应丢失，Relay 用 Envelope ID + digest 幂等处理。Relay ACK 后删除原密文，保留指纹 tombstone，避免重试重新入队。

SQLite 使用 WAL、FULL synchronous、foreign_keys 和本地单写者文件锁。正常进程崩溃/重启保持事务一致性；从陈旧数据库备份回滚是另一种攻击/运维问题，本版不支持恢复旧 ratchet 快照。

## Session 管理

每个 Peer 可以有多个并行 session，以处理双方同时首次发送。密文容器携带 session_id；按确切 session 解密。每个 Peer 最多 32 个 session。双方掌握同一组 session 后，发送按 session_id 字典序选取，避免无休止切换。接收窗口大小和 skipped message keys 由 vodozemac 管理；不是无限乱序缓存。

## Topic

Topic 创建先发生在本地。首条 Message 的加密 Event 包含 topic_id、标题和创建时间，使对端无需先收到单独 topic.create 才能接收。消息只能落到同一 Peer 的 Topic。消息事件不覆盖已存在的 Topic 标题；显式 topic.update 负责标题和归档。

当前 Topic 更新时间是秒精度的发送时间，topic.update 使用时间比较，不是 CRDT；同时改名的完美收敛策略、编辑/删除/已读事件及其冲突语义留待后续协议版本。回复支持目标晚于回复到达；目标后来到达时仍校验同 Topic。显示顺序按 timestamp 和本地插入顺序，不承诺跨端同秒消息的总排序。

## 未来演进

之后可替换 Peer Transport 的队列适配器为：

```text
Alice center -> Alice home relay -> Bob home relay -> Bob center
```

Client/Admin Protocol 与 Federation Protocol 分开；中转仍是 opaque ciphertext。路由地址独立于 Contact Card。身份迁移、设备授权、历史同步、SENT 密文 self-copy、设备撤销都需要独立设计；当前不会复制长期 private key 给域内设备。
