//! 工作区把历史模块的数据交给指定终端，异步结果按连接代次隔离。

use super::Workspace;
use crate::app::AcceptCommandPrediction;
use crate::host::HostOs;
use crate::terminal::{RemoteTerminalId, exec_answer};
use gpui_kit::*;

impl Workspace {
    pub(super) fn load_prediction_history(&mut self, id: RemoteTerminalId, cx: &mut Context<Self>) {
        let Some(panel) = self.terminals.get(&id) else {
            return;
        };
        let panel = panel.read(cx);
        let store = self.store.read(cx);
        if !panel.uses_host_filesystem()
            || store.is_external(panel.host_id())
            || store
                .host(panel.host_id())
                .is_some_and(|host| host.os == Some(HostOs::Windows))
        {
            return;
        }
        let view = panel.terminal().clone();
        if !view.read(cx).needs_prediction_history(cx) {
            return;
        }
        let generation = view.read(cx).generation(cx);
        let Some(reply) = view.read(cx).exec(crate::history::read_command(), cx) else {
            return;
        };
        let target = view.downgrade();
        let task = cx.spawn(async move |_, cx| {
            let commands = exec_answer(reply, cx)
                .await
                .and_then(Result::ok)
                .and_then(|output| crate::history::prediction_commands(&output));
            let _ = target.update(cx, |view, cx| {
                view.record_prediction_history(generation, commands, cx)
            });
        });
        view.update(cx, |view, _| view.set_prediction_reading(task));
    }

    pub(super) fn on_accept_command_prediction(
        &mut self,
        action: &AcceptCommandPrediction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = action.view.upgrade() else {
            return;
        };
        if !view.read(cx).focus_handle().is_focused(window) {
            return;
        }
        view.update(cx, |view, cx| {
            view.accept_prediction(action.generation, &action.suffix, cx)
        });
    }
}
