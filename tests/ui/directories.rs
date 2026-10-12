//! 目录联动固定操作来源，并通过 SSH exec 启动目录，避免写入交互历史。

use crate::support::*;
use shellrs::app::{EnableDirectoryReporting, OpenTerminalDirectory};
use shellrs::terminal::{directory_reporting_terminal_command, directory_terminal_command};

#[gpui_kit::test]
async fn terminal_directory_opens_the_source_host_after_sftp_connects(cx: &mut TestAppContext) {
    let factory = Arc::new(FakeTerminalFactory::printing(
        "\x1b]7;file://server/srv/a%20b/%E4%B8%AD%E6%96%87\x07",
    ));
    let (handle, workspace) = open_workspace_with_remote_factory(cx, HostStore::seed(), factory);
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
        let panel = workspace
            .read(cx)
            .remote_terminal(RemoteTerminalId(INITIAL_WEB_TERMINAL))
            .unwrap();
        panel
            .read(cx)
            .terminal()
            .read(cx)
            .current_directory(cx)
            .is_some()
    })
    .await;
    in_frame(cx, handle, |window, cx| {
        window.click(("terminal-tab", INITIAL_STAGING_TERMINAL), cx);
    });
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(OpenTerminalDirectory(RemoteTerminalId(
                INITIAL_WEB_TERMINAL,
            ))),
            cx,
        );
    });
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        window
            .try_find("remote-path")
            .is_some_and(|p| p.value() == Some("/srv/a b/中文"))
    })
    .await;
    cx.update(|cx| {
        let explorer = workspace.read(cx).explorer(ExplorerId(SFTP_TAB)).unwrap();
        assert_eq!(explorer.read(cx).host_id(), HostId(WEB_01));
    });
}

#[gpui_kit::test]
async fn sftp_directory_uses_exec_and_reconnect_keeps_the_directory(cx: &mut TestAppContext) {
    let provider = Arc::new(RecordingRemoteProvider::default());
    let (handle, workspace) =
        open_workspace_with_remote_provider(cx, HostStore::seed(), provider.clone());
    open_test_explorer(cx, handle).await;
    in_frame(cx, handle, |window, cx| {
        window
            .within(("remote-pane", SFTP_TAB))
            .click("terminal-here", cx);
    });
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
        workspace
            .read(cx)
            .remote_terminal(RemoteTerminalId(FIRST_NEW_TERMINAL))
            .is_some()
    })
    .await;
    let expected = directory_terminal_command("/home/tester").unwrap();
    assert_eq!(
        provider.logins().last().unwrap().terminal_command(),
        Some(expected.as_str())
    );
    assert!(provider.factory.written_text().is_empty());
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(ReconnectTerminal(RemoteTerminalId(FIRST_NEW_TERMINAL))),
            cx,
        );
    });
    assert_eq!(
        provider.logins().last().unwrap().terminal_command(),
        Some(expected.as_str())
    );
    assert!(provider.factory.written_text().is_empty());
}

#[gpui_kit::test]
async fn container_paths_are_not_opened_on_the_host(cx: &mut TestAppContext) {
    let factory = Arc::new(FakeTerminalFactory::printing(
        "\x1b]7;file://server/app\x07",
    ));
    let (handle, workspace) = open_workspace_with_remote_factory(cx, HostStore::seed(), factory);
    in_frame(cx, handle, |window, cx| {
        let panel = workspace
            .read(cx)
            .remote_terminal(RemoteTerminalId(INITIAL_WEB_TERMINAL))
            .unwrap()
            .clone();
        panel.update(cx, |panel, cx| panel.set_host_filesystem(false, cx));
        window.dispatch_action(
            Box::new(EnableDirectoryReporting(RemoteTerminalId(
                INITIAL_WEB_TERMINAL,
            ))),
            cx,
        );
        window.dispatch_action(
            Box::new(OpenTerminalDirectory(RemoteTerminalId(
                INITIAL_WEB_TERMINAL,
            ))),
            cx,
        );
    });
    cx.update(|cx| {
        assert!(workspace.read(cx).explorer(ExplorerId(SFTP_TAB)).is_none());
        assert!(
            workspace
                .read(cx)
                .remote_terminal(RemoteTerminalId(FIRST_NEW_TERMINAL))
                .is_none()
        );
    });
    in_frame(cx, handle, |window, cx| {
        window.click(("terminal-tab", INITIAL_WEB_TERMINAL), cx);
    });
    in_frame(cx, handle, |window, _| {
        assert!(
            window
                .try_find(("terminal-directory", INITIAL_WEB_TERMINAL))
                .is_none()
        )
    });
}

#[gpui_kit::test]
async fn missing_report_has_a_setup_entry_and_opens_an_independent_source_terminal(
    cx: &mut TestAppContext,
) {
    let provider = Arc::new(RecordingRemoteProvider::default());
    let (handle, workspace) =
        open_workspace_with_remote_provider(cx, HostStore::seed(), provider.clone());
    in_frame(cx, handle, |_, _| {});
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
        remote_lifecycle(&workspace, RemoteTerminalId(INITIAL_WEB_TERMINAL), cx)
            == TerminalLifecycle::Running
    })
    .await;
    in_frame(cx, handle, |window, cx| {
        window.activate_window();
        window.click(("terminal-tab", INITIAL_WEB_TERMINAL), cx);
    });
    in_frame(cx, handle, |window, cx| {
        assert_eq!(
            window
                .find(("terminal-directory", INITIAL_WEB_TERMINAL))
                .label(),
            Some("启用目录报告…")
        );
        window.click(("terminal-directory", INITIAL_WEB_TERMINAL), cx);
    });
    in_frame(cx, handle, |window, cx| {
        assert!(window.find("directory-reporting-setup").visible());
        window.press("escape", cx);
    });
    in_frame(cx, handle, |window, cx| {
        assert!(window.try_find("directory-reporting-setup").is_none());
        let view = workspace
            .read(cx)
            .remote_terminal(RemoteTerminalId(INITIAL_WEB_TERMINAL))
            .unwrap()
            .read(cx)
            .terminal();
        assert!(view.read(cx).focus_handle().is_focused(window));
        window.click(("terminal-tab", INITIAL_STAGING_TERMINAL), cx);
    });
    // 确认必须绑定原始终端；焦点在另一台主机也不能改变连接来源。
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(OpenTerminalDirectory(RemoteTerminalId(
                INITIAL_WEB_TERMINAL,
            ))),
            cx,
        );
    });
    in_frame(cx, handle, |window, cx| {
        window.click("enable-directory-reporting", cx)
    });
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
        workspace
            .read(cx)
            .remote_terminal(RemoteTerminalId(FIRST_NEW_TERMINAL))
            .is_some()
    })
    .await;
    let expected = directory_reporting_terminal_command();
    assert_eq!(
        provider.logins().last().unwrap().terminal_command(),
        Some(expected.as_str())
    );
    assert!(provider.factory.written_text().is_empty());
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(
            workspace
                .remote_terminal(RemoteTerminalId(FIRST_NEW_TERMINAL))
                .unwrap()
                .read(cx)
                .host_id(),
            HostId(WEB_01)
        );
        assert!(
            workspace
                .remote_terminal(RemoteTerminalId(INITIAL_WEB_TERMINAL))
                .is_some()
        );
    });
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(ReconnectTerminal(RemoteTerminalId(FIRST_NEW_TERMINAL))),
            cx,
        )
    });
    assert_eq!(
        provider.logins().last().unwrap().terminal_command(),
        Some(expected.as_str())
    );
    assert!(provider.factory.written_text().is_empty());
}

#[gpui_kit::test]
async fn reporting_setup_is_unavailable_for_windows_hosts(cx: &mut TestAppContext) {
    let (handle, workspace) = open_workspace_with_remote_factory(
        cx,
        HostStore::seed(),
        Arc::new(FakeTerminalFactory::default()),
    );
    in_frame(cx, handle, |_, _| {});
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
        remote_lifecycle(&workspace, RemoteTerminalId(INITIAL_WEB_TERMINAL), cx)
            == TerminalLifecycle::Running
    })
    .await;
    in_frame(cx, handle, |window, cx| {
        workspace.read(cx).store().clone().update(cx, |store, cx| {
            store.set_host_os(HostId(WEB_01), Some(HostOs::Windows), cx)
        });
        window.dispatch_action(
            Box::new(OpenTerminalDirectory(RemoteTerminalId(
                INITIAL_WEB_TERMINAL,
            ))),
            cx,
        );
        window.dispatch_action(
            Box::new(EnableDirectoryReporting(RemoteTerminalId(
                INITIAL_WEB_TERMINAL,
            ))),
            cx,
        );
    });
    in_frame(cx, handle, |window, cx| {
        assert!(window.try_find("directory-reporting-setup").is_none());
        assert!(
            workspace
                .read(cx)
                .remote_terminal(RemoteTerminalId(FIRST_NEW_TERMINAL))
                .is_none()
        );
    });
}
