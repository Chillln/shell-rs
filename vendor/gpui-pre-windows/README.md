# Windows 拖放补丁

此目录基于 `gpui-pre-windows` 0.3.2，保留原项目的 Apache-2.0 许可证。

本仓库仅在 `src/window.rs` 中修改外部文件拖放处理：放下文件时再次向来源程序请求 `CF_HDROP` 数据，并把最新路径交给 GPUI。部分压缩软件在鼠标进入窗口时只提供预定的临时路径，直到真正放下文件才准备内容。

升级 `gpui-pre-windows` 时需要重新核对这处补丁。
