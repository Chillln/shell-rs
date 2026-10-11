# 发布

[返回 README](../README.zh-CN.md)

在 `CHANGELOG.md` 里写好 `## [x.y.z]` 一节，把 `Cargo.toml` 的版本改成 `x.y.z`，提交后推送 tag `vx.y.z`。`.github/workflows/release.yml` 依次：

1. 核对 tag 与 `Cargo.toml` 一致，并从 `CHANGELOG.md` 截出更新说明；
2. 构建 Windows 安装程序（Inno Setup）；
3. 计算安装包 SHA256 校验和，并通过 GitHub CLI 直接创建 GitHub Release 并上传安装包与校验文件。

版本号带 `-` 的（如 `0.1.8-beta.1`）在 GitHub Release 上会自动标记为 Pre-release；不带 `-` 的则为正式 Release。
