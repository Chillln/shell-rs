//! 工作区统一接收助手动作，功能实体持有网络任务和审批状态。

use super::Workspace;
use crate::{
    app::*,
    assistant::{AssistantPanel, AssistantSettingsPanel},
};
use gpui_kit::*;

impl Workspace {
    pub fn assistant(&self, cx: &App) -> Entity<AssistantPanel> {
        self.tools.read(cx).assistant()
    }

    fn assistant_form(&self, cx: &App) -> Option<Entity<AssistantSettingsPanel>> {
        self.settings_tab
            .as_ref()
            .map(|panel| panel.read(cx).assistant())
    }

    pub(super) fn on_send_assistant_message(
        &mut self,
        _: &SendAssistantMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant(cx)
            .update(cx, |panel, cx| panel.send(window, cx));
    }
    pub(super) fn on_approve_assistant_command(
        &mut self,
        _: &ApproveAssistantCommand,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant(cx).update(cx, |panel, cx| panel.approve(cx));
    }
    pub(super) fn on_reject_assistant_command(
        &mut self,
        _: &RejectAssistantCommand,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant(cx).update(cx, |panel, cx| panel.reject(cx));
    }
    pub(super) fn on_stop_assistant(
        &mut self,
        _: &StopAssistant,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant(cx).update(cx, |panel, cx| panel.stop(cx));
    }
    pub(super) fn on_clear_assistant_conversation(
        &mut self,
        _: &ClearAssistantConversation,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant(cx).update(cx, |panel, cx| panel.reset(cx));
    }
    pub(super) fn on_save_assistant_settings(
        &mut self,
        _: &SaveAssistantSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.assistant_form(cx) {
            form.update(cx, |panel, cx| panel.save(false, window, cx));
        }
    }
    pub(super) fn on_clear_assistant_key(
        &mut self,
        _: &ClearAssistantKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.assistant_form(cx) {
            form.update(cx, |panel, cx| panel.save(true, window, cx));
        }
    }
    pub(super) fn on_set_assistant_protocol(
        &mut self,
        action: &SetAssistantProtocol,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.assistant_form(cx) {
            form.update(cx, |panel, cx| panel.set_protocol(action.0, cx));
        }
    }
    pub(super) fn on_set_assistant_permission(
        &mut self,
        action: &SetAssistantPermission,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.assistant_form(cx) {
            form.update(cx, |panel, cx| panel.set_permission(action.0, cx));
        }
    }
    pub(super) fn on_toggle_assistant_reasoning(
        &mut self,
        _: &ToggleAssistantReasoning,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant(cx)
            .update(cx, |panel, cx| panel.toggle_reasoning(cx));
    }

    pub(super) fn on_toggle_assistant_details(
        &mut self,
        action: &ToggleAssistantDetails,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant(cx)
            .update(cx, |panel, cx| panel.toggle_details(action.0, cx));
    }

    pub(super) fn on_copy_assistant_entry(
        &mut self,
        action: &CopyAssistantEntry,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant(cx)
            .update(cx, |panel, cx| panel.copy_entry(action.0, cx));
    }
}
