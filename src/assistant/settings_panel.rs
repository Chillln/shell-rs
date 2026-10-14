use super::{ApiProtocol, AssistantSettings, PermissionMode};
use crate::{
    app::{ClearAssistantKey, SaveAssistantSettings, SetAssistantPermission, SetAssistantProtocol},
    i18n::t,
    secrets::SharedSecretStore,
    settings::SettingsStore,
};
use gpui_kit::{
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::Button,
        h_flex,
        input::{Input, InputState},
        radio::{Radio, RadioGroup},
        v_flex,
    },
    prelude::*,
    *,
};
use std::sync::Arc;
use zeroize::Zeroizing;

/// 表单只持有待保存值；空密钥表示保留原值，删除必须走独立动作。
pub struct AssistantSettingsPanel {
    store: Entity<SettingsStore>,
    secrets: SharedSecretStore,
    inputs: Vec<Entity<InputState>>,
    key: Entity<InputState>,
    protocol: ApiProtocol,
    permission: PermissionMode,
    dispatch: FocusHandle,
    busy: bool,
    status: SharedString,
    task: Option<Task<()>>,
}

impl AssistantSettingsPanel {
    pub fn new(
        store: Entity<SettingsStore>,
        secrets: SharedSecretStore,
        dispatch: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = store.read(cx).settings().assistant;
        let values = [
            config.endpoint,
            config.model,
            config.timeout_seconds.to_string(),
            config.max_output_tokens.to_string(),
            config
                .temperature
                .map(|v| v.to_string())
                .unwrap_or_default(),
            config.reasoning_effort,
            config.instructions,
            config.context_tokens.to_string(),
        ];
        let inputs = values
            .into_iter()
            .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value)))
            .collect();
        Self {
            store,
            secrets,
            inputs,
            key: cx.new(|cx| InputState::new(window, cx).masked(true)),
            protocol: config.protocol,
            permission: config.permission,
            dispatch,
            busy: false,
            status: t!("assistant.settings_hint"),
            task: None,
        }
    }

    pub fn set_protocol(&mut self, protocol: ApiProtocol, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.protocol = protocol;
        cx.notify();
    }

    pub fn set_permission(&mut self, permission: PermissionMode, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.permission = permission;
        cx.notify();
    }

    fn configuration(&self, cx: &App) -> Result<AssistantSettings, String> {
        let values: Vec<String> = self
            .inputs
            .iter()
            .map(|input| input.read(cx).value().trim().to_string())
            .collect();
        let invalid = || t!("assistant.invalid_parameters").to_string();
        AssistantSettings {
            endpoint: values[0].clone(),
            model: values[1].clone(),
            timeout_seconds: values[2].parse().map_err(|_| invalid())?,
            max_output_tokens: values[3].parse().map_err(|_| invalid())?,
            context_tokens: values[7].parse().map_err(|_| invalid())?,
            temperature: if values[4].is_empty() {
                None
            } else {
                Some(values[4].parse().map_err(|_| invalid())?)
            },
            reasoning_effort: values[5].clone(),
            instructions: values[6].clone(),
            protocol: self.protocol,
            permission: self.permission,
        }
        .validated()
    }

    pub fn save(&mut self, clear_key: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let config = match self.configuration(cx) {
            Ok(config) => config,
            Err(error) => {
                self.status = error.into();
                cx.notify();
                return;
            }
        };
        let key = Zeroizing::new(self.key.read(cx).value().to_string());
        self.busy = true;
        self.status = t!("assistant.saving");
        let secrets = Arc::clone(&self.secrets);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_spawn(async move {
                    let result = if clear_key {
                        secrets.delete(&config.secret())
                    } else if key.is_empty() {
                        Ok(())
                    } else {
                        secrets.set(&config.secret(), &key)
                    };
                    result
                        .map(|()| config)
                        .map_err(|_| t!("assistant.key_error").to_string())
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match saved {
                    Ok(config) => {
                        this.store.update(cx, |store, cx| {
                            store.update(|settings| settings.assistant = config, cx)
                        });
                        // 替换输入实体，旧密钥不能从撤销历史中找回。
                        this.key = cx.new(|cx| InputState::new(window, cx).masked(true));
                        this.status = if clear_key {
                            t!("assistant.key_cleared")
                        } else {
                            t!("assistant.saved")
                        };
                    }
                    Err(error) => this.status = error.into(),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn command(&self, id: &'static str, label: SharedString, action: impl Action) -> Button {
        let dispatch = self.dispatch.clone();
        Button::new(id)
            .small()
            .outline()
            .label(label)
            .disabled(self.busy)
            .on_click(move |_, window, cx| {
                let dispatch = dispatch.clone();
                let action = action.boxed_clone();
                window.defer(cx, move |window, cx| {
                    dispatch.dispatch_action(action.as_ref(), window, cx)
                });
            })
    }
}

impl Render for AssistantSettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fields = [
            "endpoint",
            "model",
            "timeout",
            "tokens",
            "temperature",
            "reasoning_effort",
            "instructions",
            "context_tokens",
        ];
        let protocol_dispatch = self.dispatch.clone();
        let permission_dispatch = self.dispatch.clone();
        v_flex()
            .id("assistant-settings")
            .test_support()
            .w_full()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("assistant.settings_hint")),
            )
            .child(
                v_flex().gap_2().child(t!("assistant.protocol")).child(
                    RadioGroup::new("assistant-protocol")
                        .disabled(self.busy)
                        .selected_index(Some(usize::from(
                            self.protocol == ApiProtocol::ChatCompletions,
                        )))
                        .child(Radio::new("responses").label("Responses").small())
                        .child(
                            Radio::new("chat-completions")
                                .label("Chat Completions")
                                .small(),
                        )
                        .on_change(move |ix, window, cx| {
                            let dispatch = protocol_dispatch.clone();
                            let protocol = if *ix == 0 {
                                ApiProtocol::Responses
                            } else {
                                ApiProtocol::ChatCompletions
                            };
                            window.defer(cx, move |window, cx| {
                                dispatch.dispatch_action(
                                    &SetAssistantProtocol(protocol),
                                    window,
                                    cx,
                                )
                            });
                        }),
                ),
            )
            .child(
                v_flex().gap_2().child(t!("assistant.permission")).child(
                    RadioGroup::new("assistant-permission")
                        .disabled(self.busy)
                        .selected_index(Some(usize::from(
                            self.permission == PermissionMode::Approve,
                        )))
                        .child(Radio::new("suggest").label(t!("assistant.suggest")).small())
                        .child(
                            Radio::new("approve")
                                .label(t!("assistant.approve_mode"))
                                .small(),
                        )
                        .on_change(move |ix, window, cx| {
                            let dispatch = permission_dispatch.clone();
                            let permission = if *ix == 0 {
                                PermissionMode::Suggest
                            } else {
                                PermissionMode::Approve
                            };
                            window.defer(cx, move |window, cx| {
                                dispatch.dispatch_action(
                                    &SetAssistantPermission(permission),
                                    window,
                                    cx,
                                )
                            });
                        }),
                ),
            )
            .children(
                fields
                    .into_iter()
                    .zip(self.inputs.iter())
                    .map(|(field, input)| {
                        let label = t!(match field {
                            "endpoint" => "assistant.endpoint",
                            "model" => "assistant.model",
                            "timeout" => "assistant.timeout",
                            "tokens" => "assistant.tokens",
                            "temperature" => "assistant.temperature",
                            "reasoning_effort" => "assistant.reasoning_effort",
                            "context_tokens" => "assistant.context_tokens",
                            _ => "assistant.instructions",
                        });
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(div().text_sm().child(label.clone()))
                            .child(
                                Input::new(input)
                                    .id(format!("assistant-{field}"))
                                    .aria_label(label)
                                    .small()
                                    .w_full()
                                    .disabled(self.busy),
                            )
                    }),
            )
            .child(
                v_flex().gap_1().child(t!("assistant.api_key")).child(
                    Input::new(&self.key)
                        .id("assistant-api-key")
                        .aria_label(t!("assistant.api_key"))
                        .small()
                        .disabled(self.busy || !self.secrets.is_available()),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(self.command(
                        "assistant-save",
                        t!("assistant.save"),
                        SaveAssistantSettings,
                    ))
                    .child(self.command(
                        "assistant-clear-key",
                        t!("assistant.clear_key"),
                        ClearAssistantKey,
                    )),
            )
            .child(
                div()
                    .id("assistant-settings-status")
                    .test_support()
                    .aria_label(self.status.clone())
                    .text_sm()
                    .child(self.status.clone()),
            )
    }
}
