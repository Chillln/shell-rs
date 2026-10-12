//! 按来源身份衔接终端和 SFTP 的目录，避免活动标签切换后操作另一台主机。

use std::{cell::Cell, rc::Rc};

use gpui_kit::component::{WindowExt as _, notification::Notification, v_flex};
use gpui_kit::*;

use crate::app::{EnableDirectoryReporting, OpenTerminalDirectory, OpenTerminalHere};
use crate::host::{ConnectionState, HostOs};
use crate::i18n::t;
use crate::shared::{RenamableTab as _, commit_footer};
use crate::terminal::{
    TerminalLifecycle, directory_reporting_terminal_command, directory_terminal_command,
};

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
            let host = panel.host_id();
            if panel.terminal().read(cx).lifecycle(cx) != TerminalLifecycle::Running
                || self.store.read(cx).is_external(host)
                || self
                    .store
                    .read(cx)
                    .host(host)
                    .is_some_and(|host| host.os == Some(HostOs::Windows))
            {
                window.push_notification(
                    Notification::warning(t!("directory.reporting.unavailable")),
                    cx,
                );
                return;
            }
            let view = panel.terminal().downgrade();
            let dispatch = self.focus_handle.clone();
            let terminal = action.0;
            let confirmed = Rc::new(Cell::new(false));
            window.open_dialog(cx, move |dialog, _, _| {
                let submitted = confirmed.clone();
                let closed = confirmed.clone();
                let dispatch = dispatch.clone();
                let view = view.clone();
                dialog
                    .title(t!("directory.reporting.dialog_title"))
                    .overlay_closable(false)
                    .child(
                        v_flex()
                            .id("directory-reporting-setup")
                            .test_support()
                            .gap_3()
                            .child(t!("directory.no_report"))
                            .child(t!("directory.reporting.description")),
                    )
                    .footer(commit_footer(
                        "enable-directory-reporting",
                        t!("directory.reporting.enable"),
                    ))
                    .on_ok(move |_, window, cx| {
                        submitted.set(true);
                        let dispatch = dispatch.clone();
                        window.defer(cx, move |window, cx| {
                            dispatch.dispatch_action(
                                &EnableDirectoryReporting(terminal),
                                window,
                                cx,
                            )
                        });
                        true
                    })
                    .on_close(move |_, window, cx| {
                        if !closed.get() {
                            let view = view.clone();
                            window.defer(cx, move |window, cx| {
                                if let Some(view) = view.upgrade() {
                                    let focus = view.read(cx).focus_handle();
                                    window.focus(&focus, cx);
                                }
                            });
                        }
                    })
            });
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

    pub(super) fn on_enable_directory_reporting(
        &mut self,
        action: &EnableDirectoryReporting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.terminals.get(&action.0) else {
            return;
        };
        let source = source.read(cx);
        let host = source.host_id();
        let store = self.store.read(cx);
        if !source.uses_host_filesystem()
            || source.terminal().read(cx).lifecycle(cx) != TerminalLifecycle::Running
            || store.is_external(host)
            || store
                .host(host)
                .is_some_and(|host| host.os == Some(HostOs::Windows))
        {
            window.push_notification(
                Notification::warning(t!("directory.reporting.unavailable")),
                cx,
            );
            return;
        }
        let Some(name) = store.host(host).map(|host| host.name.clone()) else {
            return;
        };
        let Some(id) = self.connect_host_with_command(
            host,
            Some(directory_reporting_terminal_command()),
            window,
            cx,
        ) else {
            return;
        };
        if let Some(panel) = self.terminals.get(&id) {
            panel.update(cx, |panel, cx| {
                panel.set_custom_title(Some(t!("directory.reporting.title", host = name)), cx)
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
