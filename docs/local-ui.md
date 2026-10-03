# 本机 UI 与消息渲染

本机 UI 与管理 API 一起嵌入 `topicairn`，在浏览器中运行。只允许 loopback 监听；浏览器从自己这台中心计算机读取解密后的历史，不是新的外部 Web 客户端或域内设备协议。两个中心计算机之间继续使用现有身份认证、Olm 3DH / Double Ratchet 和密文 Relay。

## 开始使用

保持 Relay 运行，在两台中心计算机各自启动：

```sh
read -rs 'TOPICAIRN_PASSPHRASE?本机可信域口令（至少 12 bytes）: '; echo
export TOPICAIRN_PASSPHRASE
./topicairn serve --data alice --name Alice \
  --relay https://192.168.1.10:8787 --relay-ca relay-ca.pem --open
```

另一台将 `--data alice --name Alice` 改为 `--data bob --name Bob`，Relay 地址和 CA 使用之前两台 Mac 的配置。已有身份时不要更换数据目录或口令；`--name` 不会重建已有身份。完整 Relay 和 TLS 准备步骤见 [两台 Mac 测试指南](macos-testing.md)。

`--open` 打开默认浏览器，使用 90 秒有效、一次性的 URL fragment 完成自动解锁。永久 `admin.token` 不会出现在 URL、HTML 或服务日志中。自动打开失败时，访问终端打印的本机地址（默认 `http://127.0.0.1:8790/`），输入该数据目录中的 `admin.token`。它是本机管理凭证，不是可信域口令，不要与联系人交换。

1. 点击左下角自己的身份，下载公开身份卡。
2. 两台电脑通过可信渠道交换 `.contact.json`，核对完整 `user_id`。
3. 点击联系人列表的 `＋`，选文件或粘贴 JSON，验证并导入对方。双方均需导入。
4. 选联系人，点击话题列的 `＋` 创建话题。
5. 编写消息，可先预览，点击发送或按 `⌘/Ctrl + Enter`。
6. 支持回复、查看原文、本地搜索、重命名、归档和恢复话题。联系人名称可打开完整身份详情。

服务自动同步，界面每 5 秒刷新；`↻` 立即强制同步并重试。等待发送、已到 Relay、对方已接收分别表示本地入队、Relay 接受、对方端点确认接收。**不表示人已阅读**。首次会话需 Relay 可用且对方已上传 prekeys；之后可离线入队等待恢复网络。首次运行服务会自动发布 prekeys。

草稿按话题保存在当前页面内存中，切换话题不丢失。刷新或锁定界面会清除授权和草稿；重新解锁后消息历史从 SQLite 加载。锁定只关闭页面访问，不停止后台服务。Ctrl-C 退出服务。服务运行时不要对同一个数据目录运行管理 CLI。

## 渲染示例

网络与 SQLite 保存下面的 Markdown 源文，浏览器负责本机渲染：

````markdown
# 数学讨论

行内公式：$x^2 + y^2 = z^2$

$$
E = mc^2
$$

> 一个想法，一段独立对话。

- 证明
- 代码

| 项目 | 状态 |
| --- | --- |
| 推导 | 进行中 |

```rust
fn main() {
    println!("hello");
}
```

[普通链接](https://example.com)
````

Markdown 的原始 HTML 显示为文本，禁止执行。数学使用 KaTeX `trust: false`、独立宏上下文和展开/尺寸限制。代码使用 Shiki 的 JavaScript regex engine，支持 Rust、JavaScript、TypeScript、Python、JSON、Bash、SQL、HTML、CSS、YAML、Go、C++；未知语言按转义文本显示。最终 DOM 经 DOMPurify 清洗。远程图片仅显示文字占位，不发起请求。链接在新标签页打开并设 `noopener noreferrer`。公式字体、语法定义、脚本和样式全部内置，不依赖 CDN。

所有管理接口继续要求 Bearer 授权；界面令牌只存在当前页面内存，不放 cookie、localStorage 或 sessionStorage。服务同时检查 Host、Origin、Sec-Fetch-Site，拒绝跨站与 DNS rebinding 请求，不开放 CORS。页面设置 CSP、禁止 iframe 嵌入与缓存。静态资源可以在未解锁时读取，不能包含本机身份、令牌或消息。请勿通过反向代理把管理/UI 端口暴露到网络。

## 开发与验证

版本锁定在 `apps/local-ui/package.json` 与 `package-lock.json`。当前 Svelte 检查器支持 TypeScript 5/6，使用最新兼容 6.0.3；其余主依赖为 Svelte 5.57.1、Vite 8.3.2、markdown-it 15.0.2、KaTeX 0.19.0、Shiki 4.5.0、DOMPurify 3.4.16。

```sh
cd apps/local-ui
npm ci
npm run check
npm test
npm run build
cd ../..
cargo build --workspace --locked
cargo test --workspace --locked
npm --prefix apps/local-ui run test:browser
```

macOS 浏览器测试使用已安装的 Chrome；Linux CI 先执行 `npx playwright install --with-deps chromium`。测试启动临时 Alice、Bob、Relay 三个进程和两个浏览器上下文，验证真实 E2EE 收发、一次性自动解锁、数学/代码/表格、恶意输入、回复、本地搜索、隔离话题、草稿、重命名、归档、窄屏和锁定。进程与临时数据在结束时清理。截图写入忽略的 `artifacts/`。

生产资产生成到 `server/domain/ui` 并提交 Git，由 Rust build script 嵌入二进制；只构建 Rust 可以直接使用这些资产，不需要 Node。修改 UI 后必须重新 `npm run build`。CI 检查重建资产无差异；macOS 打包脚本会重建 UI，再构建两种架构，产物仍是单个可执行文件。
