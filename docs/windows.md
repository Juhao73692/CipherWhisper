# CipherWhisper Windows 64 位发行包

适用于 Windows 10 / 11 的 Intel、AMD 64 位电脑（win_amd64 / x86_64）。

## 启动

1. 将整个 ZIP 解压到本机目录。
2. 双击 `cipherwhisper.exe`，应用窗口会打开本机配置界面。
3. 按照 [界面配置指南](ui-setup.md) 创建聊天中心，或者连接自己的中心。

界面、字体、Markdown、LaTeX 和代码高亮资源全部内嵌；无需安装 Rust 或 Node。需要 [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)；若启动提示缺少运行时，请安装该页面的 Evergreen Runtime。发行包随附 `WebView2Loader.dll`，请与可执行文件放在一起。

应用不显示控制台窗口。关闭聊天窗口后仍在后台运行，图标保留在右下角系统托盘（可能位于展开区）。左键点击图标重新打开，右键菜单提供打开、隐藏和退出；再次双击也会唤起同一窗口。退出请使用图标菜单中的「退出 CipherWhisper」或界面中的「退出软件」，会等待后台停止后移除图标。

默认身份、密钥、历史和配置保存到 `%LOCALAPPDATA%\CipherWhisper\`。升级时替换程序即可，保留数据目录和原口令。工作区包含私钥和聊天历史，请保存在自己的 Windows 用户目录内。

跨电脑聊天时，连接地址填写对方能访问的局域网 IP、Tailscale IP 或域名；Windows 防火墙需允许程序对应的聊天端口（默认 8800），开启设备同步时还需允许设备端口（默认 8802）。配置界面只监听本机。

## 同机试用两个身份

在解压目录打开 PowerShell：

```powershell
.\cipherwhisper-cli.exe local-test --open
```

自动启动 Alice 和 Bob 两个临时中心并建立 P2P 连接。界面分别位于 `http://127.0.0.1:8790/` 和 `http://127.0.0.1:8791/`。按 Ctrl+C 停止试用并清理临时数据。

指定其他永久工作区：

```powershell
.\cipherwhisper.exe ui --data "D:\CipherWhisperData"
```

## 校验

发行包内的 `SHA256SUMS` 包含各文件的 SHA-256，ZIP 外的 `.zip.sha256` 校验文件用于核对整个压缩包。PowerShell 可计算：

```powershell
Get-FileHash .\cipherwhisper.exe -Algorithm SHA256
```

此开发版未进行 Windows 代码签名。

## 在 macOS / Linux 构建

安装项目固定版本的 Rust / Node，以及 [LLVM-MinGW 官方工具链](https://github.com/mstorsjo/llvm-mingw/releases)、CMake、Ninja、NASM、Python 3 和 ripgrep。

```sh
rustup target add x86_64-pc-windows-gnullvm
LLVM_MINGW=/path/to/llvm-mingw sh scripts/package-windows.sh
```

产物为 `dist/cipherwhisper-win_amd64.zip`。包含无控制台的 `cipherwhisper.exe`、用于 PowerShell/CMD 的 `cipherwhisper-cli.exe` 和 `WebView2Loader.dll`。打包时检查 PE32+ AMD64 架构、GUI 子系统、系统与随包 DLL 依赖及 ZIP 完整性，并生成校验文件。
