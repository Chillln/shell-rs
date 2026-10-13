<p align="center">
  <img src="assets/logo/shellrs.svg" width="96" alt="ShellRS">
</p>

<h1 align="center">ShellRS</h1>

<p align="center">
  基于 Rust 与 GPUI 构建的高性能、GPU 渲染原生跨平台 SSH 客户端<br>
  集成 Xshell 式主机管理、WinSCP 式双栏 SFTP 与 SSH 端口转发
</p>

<p align="center">
  <a href="https://github.com/Chillln/shell-rs/releases"><img src="https://img.shields.io/github/v/release/Chillln/shell-rs?include_prereleases&label=release" alt="Release"></a>
  <a href="#许可证"><img src="https://img.shields.io/badge/license-GPL--3.0-blue" alt="License: GPL-3.0"></a>
  <img src="https://img.shields.io/badge/platform-Windows-blue" alt="Platform">
  <img src="https://img.shields.io/badge/rust-1.98-orange" alt="Rust 1.98">
</p>

> **说明**：本项目为 [Chillln/shell-rs](https://github.com/Chillln/shell-rs) 个人自用二开项目，基于原项目定制。专注于轻量纯粹的 Windows 桌面体验与安全离线能力，核心二开定制特性如下：
> 
> - 🔐 **S3 对象存储端到端加密云同步**：接入兼容 S3 协议的对象存储（AWS S3、Cloudflare R2、MinIO 等），采用 Argon2id 密钥派生与 XChaCha20-Poly1305 强加密主机保险库；支持主密码与恢复密钥、对象版本校验、条件 ETag 写入、冲突保留与离线快照恢复；
> - ⚡ **应用退出确认与自动云同步**：退出应用时提供确认对话框，并在确认后自动合并保存数据并调度加密云同步，确保数据跨设备实时一致；
> - 🖱️ **终端交互增强配置**：设置 › 终端中新增「终端交互」选项，支持鼠标选中即自动复制，鼠标右键可灵活设置为快速粘贴、显示右键菜单或无动作；
> - 📦 **Windows 安装包体验优化**：采用官方最新 Inno Setup 7.1.0 64 位编译器打包；安装向导原生纯简体中文界面；支持自定义安装路径，并自动智能检测优先推荐非 C 盘（如 `D:\Programs\ShellRS`）等可用分区；
> - 🧹 **轻量纯粹与隐私精简**：彻底剥离原项目内置的在线自动更新检查、关于页、官网入口以及匿名遥测统计上报等冗余服务；重构 CI/CD 工作流，专注于 Windows 原生构建并直接发布至 GitHub Releases。


---

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screenshots/main-dark.png">
  <img src="docs/screenshots/main-light.png" alt="ShellRS 主界面">
</picture>

## 核心特性

- **主机与凭据管理**：支持多级嵌套分组、拖拽排序与实时过滤；自动识别远端系统类型；连接方式支持直连、多级跳板机及 HTTP/SOCKS5 代理。
- **安全第一**：密码与密钥口令仅存入系统原生钥匙串（Windows 凭据管理器 / macOS Keychain / Linux Secret Service），本地 SQLite 数据库绝不保存机密。
- **GPU 渲染终端**：基于 Zed 的 GPUI 框架与 `alacritty_terminal`，支持多标签分栏、终端内查找、字体设置和连接延迟显示。`Ctrl+Shift+P`（macOS 为 `⌘⇧P`）快速搜索并切换主机与标签。
- **远端历史命令预测**：读取并去重 bash、zsh、fish 的远端历史，输入前缀时显示建议，按右方向键采纳、回车执行。只缓存当前连接的历史，不在本地保存命令；常见 Bash / Zsh 提示符无需修改配置。
- **WinSCP 风格 SFTP 与目录联动**：对称双栏文件浏览，支持递归上传 / 下载、断点续传和传输队列；可在 SFTP 中打开终端当前目录，也可从 SFTP 当前目录新建终端。未启用目录报告时，可一键新建带报告的 Bash 标签页。
- **运维侧栏**：复用当前终端连接，查看系统监控、进程、systemd 服务、Docker 和历史命令。服务与容器日志支持自动刷新、暂停、筛选和时间范围；容器可打开独立终端，自动入口不会写入主机历史。
- **端口转发**：图形化配置本地转发、远程转发与 SOCKS5 代理，附带实时连接拓扑图。
- **面向 AI Agent 的 CLI**：内置 `shellrs` 命令行工具，支持由 Claude Code、Codex 等 AI Agent 安全调度已存主机执行命令与同步文件，无需泄露密码。

## 界面预览

<p align="center">
  <img src="docs/screenshots/split.png" alt="分栏视图：上方远程终端，下方 SFTP"><br>
  <em>灵活分栏：终端与 SFTP 同屏联动</em>
</p>

<p align="center">
  <img src="docs/screenshots/sftp.png" alt="WinSCP 风格双栏 SFTP"><br>
  <em>WinSCP 风格双栏 SFTP 与传输队列</em>
</p>

<p align="center">
  <img src="docs/screenshots/tools.png" alt="右侧运维工具箱"><br>
  <em>右侧工具栏：系统监控、进程、服务、Docker</em>
</p>

## 快速上手

1. **新建主机**：按 `Ctrl+N`（macOS 为 `⌘N`）打开新建主机对话框，输入主机地址、端口与认证信息。
2. **连接终端**：在左侧树列表中双击主机即可快速打开终端标签。
3. **打开 SFTP**：右键主机选择「打开 SFTP」，或在终端标签栏右侧点击「SFTP」。
4. **侧栏切换**：标题栏左侧图标可切换「主机树 / 端口转发 / 凭据管理」，`Ctrl+Alt+B`（macOS 为 `⌘⌥B`）显示或折叠右侧运维工具箱。
5. **分栏布局**：按 `Ctrl+T`（macOS 为 `⌘T`）打开本地终端，拖动标签页边缘即可拆分为并排或上下视图。
6. **快速切换**：按 `Ctrl+Shift+P`（macOS 为 `⌘⇧P`），搜索主机、已打开的标签或常用命令。

## 命令预测与目录联动

在「设置 → 终端 → 终端交互」开启「历史命令预测」，输入远端历史已有命令的前缀即可查看建议。右方向键补入剩余文字，Tab 保留 shell 原生补全。新命令尚未写入远端历史文件时，Bash 可执行 `history -a` 后刷新右侧「历史命令」。密码提示和全屏程序中不显示预测；自定义提示符的配置见[目录报告与历史命令预测](docs/shell-integration.md)。

终端标签工具栏的目录按钮可直接在 SFTP 中打开终端当前目录。尚未报告目录时，点击「启用目录报告… → 新建并启用」，会打开同一主机的 Bash 标签页并自动报告目录，无需编辑远端配置文件。配置仅作用于新标签，重新连接仍然生效；远端须支持 POSIX shell 并安装 Bash。SFTP 远程工具栏的「在此目录打开终端」可从当前浏览目录新建终端。

## 下载与更新

从 [GitHub Releases](https://github.com/Chillln/shell-rs/releases/latest) 下载当前正式版本的 Windows x64 安装包 `ShellRS-<版本>-windows-x86_64-setup.exe`，按当前用户安装，无需管理员权限。每个版本同时提供 `SHA256SUMS` 校验文件。应用内不检查或安装更新，更新时下载并运行新安装包即可。

当前发布流程提供 Windows 安装包，macOS / Linux 可从源码构建。版本变化见[更新日志](CHANGELOG.md)，完整功能说明见[使用手册](docs/manual.md)。

## 源码构建

项目固定使用 Rust 1.98.1（由 `rust-toolchain.toml` 声明），首次编译会拉取并构建 GPUI 组件栈：

```sh
# 克隆仓库
git clone https://github.com/Chillln/shell-rs.git
cd shell-rs

# 运行开发版本
cargo run

# 构建发布二进制文件
cargo build --release
```

详细环境依赖与 Windows Inno Setup 打包流程请参考 [开发与测试文档](docs/development.md)。

## 致谢

- 原项目 [since2006/shell-rs](https://github.com/since2006/shell-rs)：出色的架构与交互设计
- [GPUI](https://github.com/zed-industries/zed) 与 [gpui-kit](https://gpui-kit.com)：高性能 GPU 界面框架
- [russh](https://github.com/Eugeny/russh) 与 [russh-sftp](https://github.com/AspectUnk/russh-sftp)：SSH 与 SFTP 协议支持
- [alacritty_terminal](https://crates.io/crates/alacritty_terminal) 与 [portable-pty](https://crates.io/crates/portable-pty)：终端模拟与 PTY
- [keyring](https://crates.io/crates/keyring)：操作系统原生钥匙串对接
- [Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme)：SFTP 彩色文件图标（MIT 许可）

## 许可证

本项目沿用原项目协议，基于 [GNU General Public License v3.0 (GPL-3.0)](LICENSE) 开放源代码。
