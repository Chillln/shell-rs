//! 按来源身份衔接终端和 SFTP 的目录，避免活动标签切换后操作另一台主机。

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;

use crate::app::{OpenTerminalDirectory, OpenTerminalHere};
use crate::host::{ConnectionState, HostOs};
use crate::i18n::t;
use crate::terminal::directory_terminal_command;

use super::Workspace;

impl Workspace {
    pub(super) fn on_open_terminal_directory(
        &mut self,
        action: &OpenTerminalDirectory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.terminals.get(&action.0) else {
            return;
        };
        let panel = panel.read(cx);
        if !panel.uses_host_filesystem() {
            return;
        }
        let Some(path) = panel.terminal().read(cx).current_directory(cx) else {
            window.push_notification(Notification::warning(t!("directory.no_report")), cx);
            return;
        };
        let host = panel.host_id();
        let Some(id) = self.open_explorer(host, window, cx) else {
            return;
        };
        if let Some(panel) = self.explorers.get(&id) {
            panel.update(cx, |panel, cx| {
                panel.open_directory_when_connected(path, window, cx)
            });
        }
    }

    pub(super) fn on_open_terminal_here(
        &mut self,
        action: &OpenTerminalHere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.explorers.get(&action.0) else {
            return;
        };
        let panel = panel.read(cx);
        if panel.connection_state() != ConnectionState::Connected {
            return;
        }
        let host = panel.host_id();
        let path = panel.remote().read(cx).path();
        let store = self.store.read(cx);
        let Some(login) = store.login(host) else {
            return;
        };
        if login.endpoint() != panel.endpoint() {
            window.push_notification(
                Notification::warning(t!("directory.connection_changed")),
                cx,
            );
            return;
        }
        if store
            .host(host)
            .is_some_and(|host| host.os == Some(HostOs::Windows))
        {
            window.push_notification(Notification::warning(t!("directory.unsupported")), cx);
            return;
        }
        let Some(command) = directory_terminal_command(&path) else {
            window.push_notification(Notification::warning(t!("directory.unsupported")), cx);
            return;
        };
        self.connect_host_with_command(host, Some(command), window, cx);
    }
}
