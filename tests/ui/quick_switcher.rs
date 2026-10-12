//! 快速切换的键盘输入、目标身份和关闭后的焦点。

use crate::support::*;
use shellrs::app::{CenterTab, Shortcut};

fn open_switcher(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    // 先完成初始 Dock 的延后激活，再模拟用户打开面板。
    in_frame(cx, handle, |_, _| {});
    in_frame(cx, handle, |window, cx| {
        window.press(Shortcut::find("quick-switcher").unwrap().default_keys(), cx);
    });
    in_frame(cx, handle, |window, _| {
        assert!(window.find("quick-switcher").visible())
    });
}

#[gpui_kit::test]
fn typing_immediately_filters_tabs_and_enter_switches_without_a_new_connection(
    cx: &mut TestAppContext,
) {
    let factory = Arc::new(FakeTerminalFactory::default());
    let (handle, workspace) =
        open_workspace_with_remote_factory(cx, HostStore::seed(), factory.clone());
    in_frame(cx, handle, |window, cx| {
        window.click(("terminal-tab", INITIAL_WEB_TERMINAL), cx)
    });
    let before = factory.starts();
    open_switcher(cx, handle);
    in_frame(cx, handle, |window, cx| window.input("staging-api", cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("switch-terminal:1").is_none());
        assert!(window.find("switch-terminal:2").visible());
    });
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    cx.update(|cx| {
        assert_eq!(
            workspace.read(cx).active_tab(),
            Some(CenterTab::Terminal(RemoteTerminalId(
                INITIAL_STAGING_TERMINAL
            )))
        )
    });
    assert_eq!(factory.starts(), before);
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("quick-switcher").is_none())
    });
}

#[gpui_kit::test]
fn hosts_are_found_by_group_and_connected_by_their_identity(cx: &mut TestAppContext) {
    let factory = Arc::new(FakeTerminalFactory::default());
    let (handle, workspace) =
        open_workspace_with_remote_factory(cx, HostStore::seed(), factory.clone());
    open_switcher(cx, handle);
    in_frame(cx, handle, |window, cx| window.input("开发", cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("switch-host:1").is_none());
        assert!(window.find("switch-host:6").visible())
    });
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    cx.update(|cx| {
        assert_eq!(
            workspace
                .read(cx)
                .remote_terminal(RemoteTerminalId(FIRST_NEW_TERMINAL))
                .unwrap()
                .read(cx)
                .host_id(),
            HostId(DEV_BOX)
        )
    });
    assert_eq!(factory.starts(), 3);
}

#[gpui_kit::test]
fn escape_restores_focus_and_empty_results_do_not_execute(cx: &mut TestAppContext) {
    let (handle, _) = open_workspace(cx);
    in_frame(cx, handle, |window, cx| window.click("host-search", cx));
    let before = in_frame(cx, handle, |window, cx| window.focused(cx));
    open_switcher(cx, handle);
    in_frame(cx, handle, |window, cx| {
        window.input("no-such-host-987654321", cx)
    });
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.find("quick-switcher").visible())
    });
    in_frame(cx, handle, |window, cx| window.press("escape", cx));
    in_frame(cx, handle, |window, cx| window.press("escape", cx));
    in_frame(cx, handle, |window, cx| {
        assert!(window.try_find("quick-switcher").is_none());
        assert_eq!(window.focused(cx), before);
    });
}

#[gpui_kit::test]
fn a_common_command_closes_the_switcher_and_opens_the_requested_surface(cx: &mut TestAppContext) {
    let (handle, _) = open_workspace(cx);
    in_frame(cx, handle, |_, _| {});
    in_frame(cx, handle, |window, cx| {
        window.click("quick-switcher-open", cx)
    });
    in_frame(cx, handle, |window, cx| window.input("新建主机", cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.find("switch-new-host").visible());
        assert!(window.try_find("switch-terminal:1").is_none());
    });
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("quick-switcher").is_none());
        assert!(window.find("host-name").visible());
    });
}

#[gpui_kit::test]
fn clicking_a_host_found_by_address_reuses_its_connected_terminal(cx: &mut TestAppContext) {
    let factory = Arc::new(FakeTerminalFactory::default());
    let (handle, workspace) =
        open_workspace_with_remote_factory(cx, HostStore::seed(), factory.clone());
    open_switcher(cx, handle);
    in_frame(cx, handle, |window, cx| window.input("10.0.9.20", cx));
    in_frame(cx, handle, |window, cx| {
        assert!(window.try_find("switch-host:1").is_none());
        window.click("switch-host:4", cx);
    });
    in_frame(cx, handle, |window, cx| {
        assert!(window.try_find("quick-switcher").is_none());
        assert_eq!(
            workspace.read(cx).active_tab(),
            Some(CenterTab::Terminal(RemoteTerminalId(
                INITIAL_STAGING_TERMINAL
            )))
        );
        let focus = workspace
            .read(cx)
            .remote_terminal(RemoteTerminalId(INITIAL_STAGING_TERMINAL))
            .unwrap()
            .read(cx)
            .terminal()
            .read(cx)
            .focus_handle();
        assert!(focus.is_focused(window));
    });
    assert_eq!(factory.starts(), 2);
}

#[gpui_kit::test]
async fn sftp_and_local_tabs_keep_their_distinct_identities(cx: &mut TestAppContext) {
    let (handle, workspace) = open_workspace_with_sftp(cx, Arc::new(FakeSftpProvider::default()));
    open_test_explorer(cx, handle).await;
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(Box::new(NewLocalTerminal), cx);
    });
    open_switcher(cx, handle);
    in_frame(cx, handle, |window, cx| window.input("SFTP", cx));
    in_frame(cx, handle, |window, cx| {
        assert!(window.find("switch-explorer:1").visible());
        window.press("enter", cx);
    });
    cx.update(|cx| {
        assert_eq!(
            workspace.read(cx).active_tab(),
            Some(CenterTab::Explorer(ExplorerId(1)))
        );
        assert!(workspace.read(cx).explorer(ExplorerId(2)).is_none());
    });
    open_switcher(cx, handle);
    in_frame(cx, handle, |window, cx| window.input("本地终端", cx));
    in_frame(cx, handle, |window, cx| {
        assert!(window.find("switch-local:1").visible());
        window.press("enter", cx);
    });
    cx.update(|cx| {
        assert_eq!(
            workspace.read(cx).active_tab(),
            Some(CenterTab::LocalTerminal(LocalTerminalId(1)))
        );
        assert!(
            workspace
                .read(cx)
                .local_terminal(LocalTerminalId(2))
                .is_none()
        );
    });
}

#[gpui_kit::test]
fn single_line_commands_align_the_icon_and_title_without_an_empty_description(
    cx: &mut TestAppContext,
) {
    let (handle, _) = open_workspace(cx);
    open_switcher(cx, handle);
    in_frame(cx, handle, |window, cx| window.input("新建本地", cx));
    in_frame(cx, handle, |window, _| {
        let icon = window.find("switch-new-local:icon").bounds();
        let title = window.find("switch-new-local:title").bounds();
        assert!((icon.center().y - title.center().y).as_f32().abs() <= 1.);
        assert!(window.try_find("switch-new-local:detail").is_none());
    });
}
