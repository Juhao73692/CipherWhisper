# 安全模型与边界

## 身份和椭圆曲线

Ed25519 为稳定身份和严格签名验证；X25519 为椭圆曲线密钥交换。`user_id = "td_" + hex(SHA-256(Ed25519 public key bytes))`，地址、IP、Relay 或客户端实现不参与身份指纹。

从可信渠道导入 Contact Card 并核对指纹是认证的起点。自签名只证明公钥拥有者签署了该卡片，不能单凭它证明某人就是你认识的 Alice。服务器返回的 prekeys 必须通过签名校验，并与本地固定的 signing_key、curve_key 完全匹配。双方都必须导入对方 Contact Card，未知 Peer 的消息不解密、不自动信任、不 ACK。

## 会话加密

使用 [matrix-org/vodozemac 0.11.1](https://github.com/matrix-org/vodozemac) 的 Olm 3DH 和 Double Ratchet。它支持异步首次握手、每条消息密钥演进以及后续 DH ratchet。**这不是 X3DH/PQXDH 的 wire implementation，也不提供抗量子保证。**

Olm v1 使用 X25519、HKDF-SHA-256、AES-256-CBC 与 HMAC-SHA-256，内部认证 tag 为 Olm 规范规定的截断 8 bytes。我们额外要求每个外层 Envelope 通过完整 Ed25519 签名，覆盖密文和路由；修改密文或路由必须先突破固定身份签名验证。这不是把完整 HMAC 或 AEAD tag 宣称为 8 bytes；所有 cryptographic primitive 与 ratchet 都由库实现。

One-time prekey 默认由对端中心原子领取（旧模式由 Relay 领取）且不可因重复 publish 重新出现；接收端建立新会话时删除对应私钥。池为空时使用签名 fallback key（Olm 模型）。fallback 被复用时，初始握手的前向保密弱于消费 one-time key 的握手；收到后续 ratchet 回复引入新熵。当前同一 fallback key 的签名有效期被续期，不自动轮换私钥，以避免破坏长时间离线队列；部署审查时应完善保留窗口和轮换策略。

Double Ratchet 的恢复能力要求未泄露的新随机 DH 熵和后续双向通信。持续控制中心端点、同时盗取长期身份密钥并主动冒充的攻击者不会自动失去控制。vodozemac 有独立库审计；CipherWhisper 的协议组合和服务实现尚未独立审计，不能据此声称整个项目已审计。

## 私钥与本地状态

口令经 Argon2id v0x13（64 MiB、3 iterations、1 lane、32-byte 随机 salt）派生 32-byte storage key。身份和 ratchet 的 serde pickle 使用 RustCrypto XChaCha20-Poly1305：每次保存使用新的 OS CSPRNG 24-byte nonce，16-byte认证 tag；AAD 绑定 account 或 Peer/session 记录。

不使用 vodozemac legacy/libolm pickle encryption 保存复用密钥的状态，因为该兼容格式从密钥派生固定 IV。保险库不自行实现 nonce 算法、cipher、KDF 或 MAC。

中心端点目录权限为 0700、数据库和 token 为 0600（Unix）。单写者锁覆盖进程生命周期。派生密钥和临时 pickle 字节使用 Zeroizing。中心端点的明文消息历史和 FTS5 索引按用户要求保存本地；整库未加密，需要 OS 磁盘加密和适当目录保护。文件权限在 Windows 上不提供等价 ACL 保证，本次只验证 macOS。

本版没有账号恢复、密钥导出/备份、口令轮换或旧快照恢复。SQLite commit 的原子性不能防止攻击者替换整个数据库为旧快照。不要恢复陈旧 ratchet 状态，也不要在多个中心端点复制同一身份数据目录。

## 传输与重放

外层 Envelope 签名绑定 version、id、from、to、ciphertext、timestamp；解密 Payload 再次绑定全部路由字段。固定签名先于解密/去重。成功解密后持久化 Envelope digest 去重；ratchet 自身也拒绝重复 message keys。

可选旧 Relay HTTP 认证签名绑定 method、完整 path/query、精确 body SHA-256、时间和 UUID nonce。Relay 只允许 ±300 秒，持久化已用 nonce；签名、body 或路径不匹配和重复请求被拒绝。新的 HTTP 重试有新 nonce，但 Envelope 保持不变。

默认直接 P2P 的签名请求使用独立 domain separator，绑定目标中心身份、方法、完整 path/query、正文摘要、时间和 UUID nonce；仅固定 Peer 可调用。中心持久化 nonce，拒绝重放。签名响应绑定双方身份和请求 nonce，客户端还验证完整响应数据。prekey、连接测试和 ACK 均不接受未验证响应；TLS 服务证书本身不能替代中心稳定身份签名。

对端入站只能保存发给自己的密文；送达状态只允许原发送者查询。仅在解密/业务校验成功并提交历史、ratchet、去重记录之后，签名响应才包含 `acknowledged:true`。错误密文不能获取成功确认；重投递同一 ID 但不同摘要被拒绝。网络返回前崩溃仍可幂等重投；每次 HTTP 重试更换 nonce，密文不变。签名送达证明是对端中心的声明，不是人已读。

远程 P2P 仅 HTTPS/TLS 1.3，证书/主机名验证、无重定向，连接卡 CA 存在时只信任它；HTTP 只用于 loopback。签名 `.peer.json` 绑定身份、地址和公开 CA，导入仍需通过可信渠道核对完整身份指纹。首次领取 prekey 需对方在线；已建立会话在发送方本地排队，无第三方离线信箱。监听与本机管理/设备协议分离，拒绝 Origin，不暴露明文历史和管理功能。公网 NAT 穿透、发现、限流及 tombstone 长期清理未实现。

非 loopback Relay URL 强制 HTTPS，HTTP redirect 禁止。Relay 未配置 TLS 时只允许 loopback；配置 `--tls-cert`/`--tls-key` 后支持内置 HTTPS 的远程监听，也可使用 TLS 反向代理。自签发测试 CA 通过 `--relay-ca` 显式提供给客户端，不禁用证书/主机名验证，不修改系统信任库。管理 API 的随机 256-bit bearer token 仅用于本机，恒定时间比较其摘要，默认不启用 CORS。

## 可选旧 Relay 的可见信息及可作恶范围

可见：双方身份、公钥/prekeys、Envelope ID、外层时间、大小、会话密文头、网络来源、投递及 ACK 状态。不可见：Topic ID/标题、Markdown、reply、搜索、历史。需要隐藏流量关系时还需另行设计元数据保护。

Relay 可以丢弃、延迟消息、消耗公开 prekeys 或谎报 delivery 状态，并能拒绝服务；E2EE 不保证 Relay 可用性。仅旧 Relay 模式的 ACK/delivery 是 Relay 报告，不是对端签名的加密送达证明。Relay 本身不拥有解密私钥。每收件人未 ACK 队列限制 10000，单消息 Markdown 最大 64 KiB；已 ACK tombstone、nonce/prekey tables 的长期清理和公网防滥用不完整。当前适用于受控部署；公网部署需限流、存储配额、日志策略和权限隔离。

附件、编辑、删除、已读、群聊、Federation 均未实现，不能从现有 API 推导这些未来功能的安全性质。

## 域内设备同步

设备持有独立 Ed25519 私钥，不获得中心用户私钥或外部 ratchet。中心本机管理员核对设备指纹并授权；客户端用可信渠道核对的完整中心身份校验签名配对文件，绑定设备、中心、HTTPS 地址、CA 和日志 epoch。配置在客户端经带独立 AAD 的 XChaCha20-Poly1305 加密保存，防止本地配置被修改后跳转到假中心。

独立设备监听仅 TLS 1.3，通过 rustls/AWS-LC 提供成熟密钥交换和认证加密。客户端仅信任配对 CA，验证主机名，不允许重定向。每次请求用设备 Ed25519 签名绑定中心、方法、完整 path/query、精确正文摘要、时间和 nonce；中心持久化 nonce 拒绝重放。JSON 响应额外由中心稳定身份签名绑定设备及这次请求 nonce。TLS 私钥本身没有授权任意设备或伪造中心身份响应的能力。

设备 API 与本机管理 API 分离，拒绝带 Origin 的浏览器请求，不提供 CORS 或管理员功能。授权设备拥有全部聊天历史读写权限；撤销持久化并永久禁止该设备密钥重新加入，无法远程擦除已有缓存。

中心日志与数据/ratchet/幂等操作收据在同一事务提交；客户端原子保存页面和游标后 ACK。每设备独立游标，ACK 不裁剪共享历史。每实体日志版本阻止旧操作确认覆盖新状态。epoch、游标和观察高水位检查拒绝可观察的数据库回滚；不能把这视为备份恢复或完整防回滚机制。日志、操作收据、撤销 tombstone 暂不 GC；公网容量/限流不完整。详见 [域内同步指南](device-sync.md)。

## 本机 UI

本机浏览器通过 Bearer 认证读取已解密历史，不向外部 Peer 提供新的浏览器协议。全部 JS、CSS、公式字体和语法定义嵌入同一个可执行文件，无 CDN。Markdown 原始 HTML 按文本显示；KaTeX 禁用 trust、隔离宏并限制展开次数与尺寸；Shiki 使用 JavaScript regex engine；最终 HTML 经 DOMPurify 清洗。远程图片不加载，避免泄露阅读行为/IP。

`--open` 使用独立的随机 256-bit 一次性 bootstrap code，90 秒有效，以 URL fragment 传给浏览器，不进入 HTTP URL 或服务日志；换取的随机会话 Bearer 只保存在页面内存，摘要留在服务进程，服务重启失效。永久管理令牌不注入静态资产；手动解锁仍可使用 admin.token。页面锁定/刷新清除授权，不停止服务。

精确校验 loopback Host、Origin 和 Sec-Fetch-Site，防止跨站页面或 DNS rebinding 调用管理 API；所有消息路由仍要求令牌。设置 no-store、nosniff、frame-ancestors none、no-referrer 和仅本机资源的 CSP。KaTeX/Shiki 的生成样式需要 style-src unsafe-inline；script-src 仅允许本机已打包脚本，禁止原始 HTML/script 注入。UI 端口不能通过反向代理暴露到网络。
