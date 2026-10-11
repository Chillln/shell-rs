# 发布

[返回 README](../README.zh-CN.md)

在 `CHANGELOG.md` 里写好 `## [x.y.z]` 一节，把 `Cargo.toml` 的版本改成 `x.y.z`，提交后推送 tag `vx.y.z`。`.github/workflows/release.yml` 依次：

1. 核对 tag 与 `Cargo.toml` 一致，并从 `CHANGELOG.md` 截出发布说明；
2. 构建 Windows 安装程序；
3. 在 `release` 环境里人工批准后：给安装包签名、上传到 R2、创建 GitHub Release，再更新旧版客户端使用的更新清单并清理 CDN 缓存，最后触发官网重新构建。当前版本的 ShellRS 不再检查或安装应用更新；继续发布清单只为兼容此前已安装的版本。

版本号带 `-` 的（如 `0.3.0-beta.1`）只发到 beta 通道；正式版同时成为 beta 通道的最新版。在 Actions 里手动运行这个 workflow 只构建打包、不发布，用来演练。

一次性准备：

- **旧版更新签名密钥**：保留此前发布所用的 minisign 私钥，在 GitHub 的 `release` 环境中配置 `MINISIGN_SECRET_KEY` 与 `MINISIGN_PASSWORD`，以便旧版客户端继续验证清单和安装包。
- **Cloudflare**：shellrs.com 的 DNS 托管在 Cloudflare；建 R2 bucket，绑定自定义域 `dl.shellrs.com`，关闭 `r2.dev` 地址；Cache Rules 让 `/releases/*` 和 `/update/*` 按源站的 Cache-Control 缓存（默认不缓存 `.json`、`.AppImage`）；一个只能写这个 bucket 的 R2 API Token，一个只能清这个 zone 缓存的 API Token；官网项目（独立的私有仓库，由 Cloudflare 连着仓库构建）在「设置 › 构建 › 部署挂钩」里建一个构建 `main` 分支的挂钩。
- **图标**：`assets/logo/` 里的 `shellrs.icns`（macOS）、`shellrs.png`（1024×1024，Linux，macOS 没有 icns 时也由它生成）和 `shellrs.ico`（Windows 安装程序）。
- **GitHub secrets**：`release` 环境（设为需要审批）里的 `MINISIGN_SECRET_KEY`、`MINISIGN_PASSWORD`、`R2_ACCOUNT_ID`、`R2_ACCESS_KEY_ID`、`R2_SECRET_ACCESS_KEY`、`R2_BUCKET`、`CF_ZONE_ID`、`CF_API_TOKEN`、`SITE_DEPLOY_HOOK`。

清单格式见 `packaging/manifest.example.json`，由 `packaging/make-manifest.sh` 生成：一个文件里是清单原文和它的 minisign 签名，签名的 trusted comment 写明通道和版本。格式只增加字段、不改已有的；已发出去的 ShellRS 一直按 `/update/v1/` 这个地址检查。
