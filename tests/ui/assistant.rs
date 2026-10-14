//! 通过真实工作区验证流式展示、审批、取消、主机隔离和自动摘要。
use crate::support::*;
use serde_json::{Value, json};
use shellrs::{
    app::ApproveAssistantCommand,
    assistant::{AssistantProvider, ModelEvent, ModelRequest, ModelTurn, PermissionMode, ToolCall},
};
use std::sync::atomic::AtomicBool;

struct FakeModel {
    release: AtomicBool,
    calls: Mutex<Vec<(bool, Vec<Value>, String)>>,
    long_answer: bool,
}

impl FakeModel {
    fn new(release: bool, long_answer: bool) -> Self {
        Self {
            release: AtomicBool::new(release),
            calls: Mutex::new(Vec::new()),
            long_answer,
        }
    }
}

impl AssistantProvider for FakeModel {
    fn complete(
        &self,
        request: ModelRequest,
        event: &mut dyn FnMut(ModelEvent),
    ) -> Result<ModelTurn, String> {
        let count = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((request.compacting, request.history.clone(), request.context));
            calls.len()
        };
        let mut turn = ModelTurn::default();
        if request.compacting {
            turn.text = "用户要检查磁盘，尚未授权修改；保留拒绝记录。".into();
            return Ok(turn);
        }
        event(ModelEvent::Reasoning("先核对磁盘使用情况".into()));
        event(ModelEvent::Text("准备检查".into()));
        while !self.release.load(Ordering::Acquire) {
            if request.cancelled.load(Ordering::Acquire) {
                return Err("已取消".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        turn.reasoning = "先核对磁盘使用情况".into();
        if self.long_answer && count == 1 {
            turn.text = "检查结果".repeat(1200);
        } else if !self.long_answer
            && count == 1
            && request.settings.permission == PermissionMode::Approve
        {
            turn.text = "需要读取磁盘使用量".into();
            turn.call = Some(ToolCall::new(
                "disk-1",
                "df -h",
                "查询磁盘使用量，不修改文件",
            ));
            turn.items = vec![
                json!({"type":"function_call","call_id":"disk-1","name":"execute_ssh_command","arguments":"{\"command\":\"df -h\",\"explanation\":\"查询磁盘\"}"}),
            ];
            return Ok(turn);
        } else {
            turn.text = "检查完成\n\n| 文件系统 | 状态 |\n| --- | --- |\n| **根目录** | 正常 |\n\n```sh\ndf -h\n```".into();
        }
        turn.items = vec![json!({"role":"assistant","content":turn.text})];
        Ok(turn)
    }
}

fn open_assistant(
    cx: &mut TestAppContext,
    model: Arc<FakeModel>,
    permission: PermissionMode,
    context_tokens: u32,
) -> (
    WindowHandle<Root>,
    Entity<Workspace>,
    Arc<FakeTerminalFactory>,
) {
    let factory = Arc::new(FakeTerminalFactory::answering(&["disk output"]));
    let (handle, workspace) =
        open_workspace_with_remote_factory(cx, HostStore::seed(), factory.clone());
    workspace.update(cx, |workspace, cx| {
        workspace.settings().clone().update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    settings.assistant.model = "fixture-model".into();
                    settings.assistant.permission = permission;
                    settings.assistant.context_tokens = context_tokens;
                    settings.assistant.max_output_tokens = 256;
                },
                cx,
            )
        });
        workspace
            .assistant(cx)
            .update(cx, |assistant, cx| assistant.set_provider(model, cx));
    });
    in_frame(cx, handle, |window, cx| {
        window.activate_window();
        window.click(("terminal-tab", INITIAL_WEB_TERMINAL), cx);
    });
    in_frame(cx, handle, |window, cx| window.click("tool-assistant", cx));
    (handle, workspace, factory)
}

fn send(cx: &mut TestAppContext, handle: WindowHandle<Root>) {
    in_frame(cx, handle, |window, cx| {
        window.click("assistant-input", cx);
        window.input("检查磁盘", cx);
        window.press("enter", cx);
    });
}

async fn wait_status(cx: &mut TestAppContext, handle: WindowHandle<Root>, status: &str) {
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, _| {
        window
            .try_find("assistant-status")
            .is_some_and(|entry| entry.label() == Some(status))
    })
    .await;
}

#[gpui_kit::test]
async fn assistant_streams_before_completion_and_executes_only_after_approval(
    cx: &mut TestAppContext,
) {
    let model = Arc::new(FakeModel::new(false, false));
    let (handle, _, factory) = open_assistant(cx, model.clone(), PermissionMode::Approve, 16384);
    send(cx, handle);
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, _| {
        window
            .try_find(("assistant-message", 2u64))
            .is_some_and(|entry| {
                entry
                    .label()
                    .is_some_and(|label| label.contains("准备检查"))
            })
    })
    .await;
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("assistant-approval").is_none())
    });
    assert_eq!(factory.exec_count(), 0);
    model.release.store(true, Ordering::Release);
    wait_status(cx, handle, "等待你批准此命令").await;
    assert_eq!(factory.exec_count(), 0);
    in_frame(cx, handle, |window, cx| {
        window.click("assistant-approve", cx);
        window.dispatch_action(Box::new(ApproveAssistantCommand), cx);
    });
    wait_status(cx, handle, "响应已完成").await;
    assert_eq!(factory.exec_commands(), ["df -h"]);
    // 原始结果默认收起；展开与复制保留真实换行，不显示协议 JSON。
    in_frame(cx, handle, |window, cx| {
        assert!(window.try_find(("assistant-output", 3u64)).is_none());
        window.click(("assistant-details", 3u64), cx);
    });
    in_frame(cx, handle, |window, cx| {
        assert_eq!(
            window.find(("assistant-output", 3u64)).label(),
            Some("disk output\n")
        );
        window.click(("assistant-copy", 3u64), cx);
    });
    cx.update(|cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("df -h\n\nstdout:\ndisk output\n\nstderr:\n".into())
        )
    });
    let calls = model.calls.lock().unwrap();
    assert!(calls[0].2.contains("root@"));
    assert!(calls[0].2.contains("remote OS:"));
    assert!(calls[0].2.contains("never inherit"));
    assert!(
        calls[1]
            .1
            .iter()
            .any(|item| item["type"] == "function_call_output"
                && item["output"].as_str().unwrap().contains("disk output"))
    );
}

#[gpui_kit::test]
async fn assistant_refusal_and_host_switch_cannot_execute_a_pending_command(
    cx: &mut TestAppContext,
) {
    let model = Arc::new(FakeModel::new(true, false));
    let (handle, workspace, factory) = open_assistant(cx, model, PermissionMode::Approve, 16384);
    send(cx, handle);
    wait_status(cx, handle, "等待你批准此命令").await;
    in_frame(cx, handle, |window, cx| {
        window.click("assistant-reject", cx)
    });
    wait_status(cx, handle, "已拒绝，命令未执行。").await;
    assert_eq!(factory.exec_count(), 0);
    workspace.update(cx, |workspace, cx| {
        workspace.assistant(cx).update(cx, |assistant, cx| {
            assistant.set_provider(Arc::new(FakeModel::new(true, false)), cx)
        })
    });
    send(cx, handle);
    wait_status(cx, handle, "等待你批准此命令").await;
    in_frame(cx, handle, |window, cx| {
        window.click(("terminal-tab", INITIAL_STAGING_TERMINAL), cx)
    });
    in_frame(cx, handle, |window, cx| {
        assert!(window.try_find("assistant-approval").is_none());
        window.dispatch_action(Box::new(ApproveAssistantCommand), cx);
    });
    assert_eq!(factory.exec_count(), 0);
}

#[gpui_kit::test]
async fn assistant_cancels_streaming_and_invalidates_approval_on_reconnect(
    cx: &mut TestAppContext,
) {
    let model = Arc::new(FakeModel::new(false, false));
    let (handle, workspace, factory) =
        open_assistant(cx, model.clone(), PermissionMode::Approve, 16384);
    send(cx, handle);
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, _| {
        !model.calls.lock().unwrap().is_empty()
    })
    .await;
    in_frame(cx, handle, |window, cx| window.click("assistant-stop", cx));
    model.release.store(true, Ordering::Release);
    in_frame(cx, handle, |window, _| {
        assert!(window.try_find("assistant-approval").is_none())
    });
    workspace.update(cx, |workspace, cx| {
        workspace.assistant(cx).update(cx, |assistant, cx| {
            assistant.set_provider(Arc::new(FakeModel::new(true, false)), cx)
        })
    });
    send(cx, handle);
    wait_status(cx, handle, "等待你批准此命令").await;
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(
            Box::new(ReconnectTerminal(shellrs::terminal::RemoteTerminalId(
                INITIAL_WEB_TERMINAL,
            ))),
            cx,
        )
    });
    in_frame(cx, handle, |window, cx| {
        window.dispatch_action(Box::new(ApproveAssistantCommand), cx);
        assert!(window.try_find("assistant-approval").is_none());
    });
    assert_eq!(factory.exec_count(), 0);
}

#[gpui_kit::test]
async fn assistant_automatically_compacts_history_without_granting_execution(
    cx: &mut TestAppContext,
) {
    let model = Arc::new(FakeModel::new(true, true));
    let (handle, workspace, factory) =
        open_assistant(cx, model.clone(), PermissionMode::Suggest, 4096);
    send(cx, handle);
    wait_status(cx, handle, "响应已完成").await;
    in_frame(cx, handle, |window, _| {
        let panel = window.find("assistant").bounds();
        let input = window.find("assistant-input");
        let send = window.find("assistant-send");
        assert!(input.visible() && send.visible());
        assert!(input.bounds().bottom() <= panel.bottom());
        assert!(send.bounds().right() <= panel.right());
    });
    send(cx, handle);
    wait_status(cx, handle, "响应已完成").await;
    workspace.update(cx, |workspace, cx| {
        assert_eq!(workspace.assistant(cx).read(cx).compactions(), 1)
    });
    let calls = model.calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|call| call.0).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert_eq!(calls[2].1.len(), 2);
    assert!(
        calls[2].1[0]["content"]
            .as_str()
            .unwrap()
            .contains("grants no permissions")
    );
    assert_eq!(factory.exec_count(), 0);
}

fn reveal(window: &mut gpui_kit::Window, id: &'static str, cx: &mut App) {
    for _ in 0..14 {
        window.render_frame(cx);
        if window.try_find(id).is_some_and(|element| element.visible()) {
            return;
        }
        window.scroll(
            "settings",
            gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-200.))),
            cx,
        );
    }
    assert!(window.find(id).visible());
}

#[gpui_kit::test]
async fn assistant_composer_accepts_newlines_without_sending_early(cx: &mut TestAppContext) {
    let model = Arc::new(FakeModel::new(true, false));
    let (handle, _, factory) = open_assistant(cx, model.clone(), PermissionMode::Suggest, 16384);
    in_frame(cx, handle, |window, cx| {
        window.click("assistant-input", cx);
        window.input("检查磁盘", cx);
        window.press("shift-enter", cx);
        window.input("继续分析", cx);
    });
    assert!(model.calls.lock().unwrap().is_empty());
    in_frame(cx, handle, |window, cx| window.press("enter", cx));
    wait_status(cx, handle, "响应已完成").await;
    assert!(
        model.calls.lock().unwrap()[0]
            .1
            .iter()
            .any(|item| item["content"] == "检查磁盘\n继续分析")
    );
    assert_eq!(factory.exec_count(), 0);
}

#[gpui_kit::test]
async fn assistant_settings_save_keys_in_keychain_and_preserve_blank_key(cx: &mut TestAppContext) {
    let secrets = Arc::new(InMemorySecretStore::default());
    let (handle, workspace) =
        open_workspace_with_store(cx, HostStore::empty().with_secrets(secrets.clone()));
    in_frame(cx, handle, |window, cx| window.click("open-settings", cx));
    in_frame(cx, handle, |window, cx| {
        window.within("settings").click("0-8", cx)
    });
    in_frame(cx, handle, |window, cx| {
        window.click("assistant-model", cx);
        window.input("fixture-model", cx);
        reveal(window, "assistant-api-key", cx);
        window.click("assistant-api-key", cx);
        window.input("fixture-secret", cx);
        reveal(window, "assistant-save", cx);
        window.click("assistant-save", cx);
    });
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, cx| {
        workspace
            .read(cx)
            .settings()
            .read(cx)
            .settings()
            .assistant
            .model
            == "fixture-model"
    })
    .await;
    let config = workspace.read_with(cx, |workspace, cx| {
        workspace.settings().read(cx).settings().assistant
    });
    assert_eq!(
        secrets
            .get(&config.secret())
            .unwrap()
            .as_deref()
            .map(String::as_str),
        Some("fixture-secret")
    );
    assert!(
        !serde_json::to_string(&config)
            .unwrap()
            .contains("fixture-secret")
    );
    in_frame(cx, handle, |window, cx| {
        reveal(window, "assistant-save", cx);
        window.click("assistant-save", cx);
    });
    cx.wait_for(handle.into(), Duration::from_secs(5), |window, _| {
        window
            .try_find("assistant-settings-status")
            .is_some_and(|element| element.label() == Some("配置已保存，新的请求使用此接口。"))
    })
    .await;
    assert_eq!(secrets.len(), 1);
    in_frame(cx, handle, |window, cx| {
        window.click("assistant-clear-key", cx)
    });
    cx.wait_for(handle.into(), Duration::from_secs(5), |_, _| {
        secrets.is_empty()
    })
    .await;
}
