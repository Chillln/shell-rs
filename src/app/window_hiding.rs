//! macOS 关闭窗口只隐藏应用，连接、传输、转发与外部 CLI 继续运行。
//! 点 Dock 图标、⌘Tab 或再次启动会恢复原窗口，⌘Q 才真正退出。
//! Windows 与 Linux 关闭窗口走退出确认，并等待已启用的云同步完成。

use gpui_kit::{App, Window};

/// Make the window's close button hide ShellRS on macOS. Elsewhere closing
/// quits, after asking the quit guard.
#[cfg(target_os = "macos")]
pub fn hide_when_closed(window: &Window, cx: &App) {
    window.on_window_should_close(cx, |_, cx| {
        cx.hide();
        false
    });
}

#[cfg(not(target_os = "macos"))]
pub fn hide_when_closed(window: &Window, cx: &App) {
    window.on_window_should_close(cx, |window, cx| !super::quit::quit_held_back(window, cx));
}

/// Show the window again, in front of other apps: when the Dock icon is
/// clicked, or ShellRS is opened again while it runs.
pub fn bring_forward(window: &mut Window, cx: &mut App) {
    // Activating alone may leave a hidden app's windows hidden.
    #[cfg(target_os = "macos")]
    if let Some(main_thread) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(main_thread).unhide(None);
    }
    cx.activate(true);
    window.activate_window();
}
