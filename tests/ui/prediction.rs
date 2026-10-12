//! 历史预测只在已回显的 shell 输入内采纳，并保留终端的原有按键行为。

use crate::support::*;
use shellrs::app::AcceptCommandPrediction;
use shellrs::terminal::{TerminalInteraction, TerminalView};

fn open_predicting_terminal(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Root>,
    Entity<TerminalView>,
    Arc<FakeTerminalFactory>,
) {
    let factory = Arc::new(FakeTerminalFactory {
        behavior: FakeBehavior::Prints("$ \x1b]133;B\x07"),
        exec_outputs: Arc::new(Mutex::new(
            ["@@size\n80\n@@history\nls -l\nls -lh\npassword test\n".into()].into(),
        )),
        ..Default::default()
    });
    let (handle, workspace) =
        open_workspace_with_remote_factory(cx, HostStore::seed(), factory.clone());
    in_frame(cx, handle, |_, _| {});
    in_frame(cx, handle, |window, cx| {
        window.activate_window();
        window.click(("terminal-tab", INITIAL_WEB_TERMINAL), cx);
    });
    in_frame(cx, handle, |window, cx| {
        window.click(("terminal", INITIAL_WEB_TERMINAL), cx)
    });
    let view = cx.update(|cx| {
        workspace
            .read(cx)
            .remote_terminal(RemoteTerminalId(INITIAL_WEB_TERMINAL))
            .unwrap()
            .read(cx)
            .terminal()
            .clone()
    });
    (handle, view, factory)
}

async fn wait_for_prediction(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    view: &Entity<TerminalView>,
    suffix: &str,
) {
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, cx| {
        window.render_frame(cx);
        view.read(cx).prediction(cx).as_deref() == Some(suffix)
    })
    .await;
}

#[gpui_kit::test]
async fn right_accepts_only_the_suffix_without_executing(cx: &mut TestAppContext) {
    let (handle, view, factory) = open_predicting_terminal(cx);
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, _| {
        factory.exec_count() >= 2
    })
    .await;
    in_frame(cx, handle, |window, cx| window.input("ls", cx));
    wait_for_prediction(cx, handle, &view, " -lh").await;
    in_frame(cx, handle, |window, _| {
        assert!(
            window
                .find(("terminal", INITIAL_WEB_TERMINAL))
                .label()
                .unwrap()
                .contains("按右方向键采纳")
        );
    });
    in_frame(cx, handle, |window, cx| window.press("right", cx));
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, _| {
        factory.written_text() == "ls -lh"
    })
    .await;
    assert!(!factory.written_text().contains('\r'));
}

#[gpui_kit::test]
async fn tab_keeps_native_completion_and_submit_stops_password_suggestions(
    cx: &mut TestAppContext,
) {
    let (handle, view, factory) = open_predicting_terminal(cx);
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, _| {
        factory.exec_count() >= 2
    })
    .await;
    in_frame(cx, handle, |window, cx| window.input("ls", cx));
    wait_for_prediction(cx, handle, &view, " -lh").await;
    in_frame(cx, handle, |window, cx| window.press("tab", cx));
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, _| {
        factory.written_text() == "ls\t"
    })
    .await;
    in_frame(cx, handle, |window, cx| {
        window.press("enter", cx);
        window.input("password", cx);
    });
    cx.update(|cx| assert!(view.read(cx).prediction(cx).is_none()));
    in_frame(cx, handle, |window, cx| window.press("right", cx));
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, _| {
        factory.written_text().ends_with("\x1b[C")
    })
    .await;
}

#[gpui_kit::test]
async fn setting_and_connection_generation_guard_acceptance(cx: &mut TestAppContext) {
    let (handle, view, factory) = open_predicting_terminal(cx);
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, _| {
        factory.exec_count() >= 2
    })
    .await;
    in_frame(cx, handle, |window, cx| window.input("ls", cx));
    wait_for_prediction(cx, handle, &view, " -lh").await;
    let generation = cx.update(|cx| view.read(cx).generation(cx));
    cx.update(|cx| {
        let mut settings = TerminalInteraction::current(cx);
        settings.command_prediction = false;
        cx.set_global(settings);
    });
    cx.update(|cx| assert!(view.read(cx).prediction(cx).is_none()));
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(AcceptCommandPrediction {
                view: view.downgrade(),
                generation,
                suffix: " -lh".into(),
            }),
            cx,
        )
    });
    assert_eq!(factory.written_text(), "ls");
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(ReconnectTerminal(RemoteTerminalId(INITIAL_WEB_TERMINAL))),
            cx,
        )
    });
    cx.update(|cx| {
        let mut settings = TerminalInteraction::current(cx);
        settings.command_prediction = true;
        cx.set_global(settings);
    });
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(AcceptCommandPrediction {
                view: view.downgrade(),
                generation,
                suffix: " -lh".into(),
            }),
            cx,
        )
    });
    assert_eq!(factory.written_text(), "ls");
}
