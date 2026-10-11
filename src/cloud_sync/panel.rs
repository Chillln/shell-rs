use gpui_kit::{
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::Button,
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputState},
        v_flex,
    },
    prelude::*,
    *,
};
use zeroize::Zeroizing;

use super::CloudSyncSettings;
use crate::{
    app::{
        ApplyCloudVault, ClearCloudSessionToken, CopyCloudRecoveryKey, DownloadCloudVault,
        ImportCloudBackup, KeepLocalCloudVault, LockCloudVault, SaveCloudSyncSettings,
        UploadCloudVault,
    },
    i18n::t,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncPhase {
    Idle,
    Busy,
    Preview,
}

pub(crate) struct SyncInput {
    pub settings: CloudSyncSettings,
    pub secret_key: Zeroizing<String>,
    pub token: Zeroizing<String>,
    pub password: Zeroizing<String>,
    pub recovery: Zeroizing<String>,
}

/// 云同步表单持有输入与可见状态；工作区处理动作，后台服务处理秘密和网络。
pub struct CloudSyncPanel {
    endpoint: Entity<InputState>,
    region: Entity<InputState>,
    bucket: Entity<InputState>,
    object_key: Entity<InputState>,
    access_key: Entity<InputState>,
    secret_key: Entity<InputState>,
    token: Entity<InputState>,
    password: Entity<InputState>,
    recovery: Entity<InputState>,
    virtual_hosted: bool,
    automatic: bool,
    dispatch: FocusHandle,
    phase: SyncPhase,
    status: SharedString,
    error: bool,
    recovery_to_copy: Option<Zeroizing<String>>,
    preview_cloud: bool,
}

fn input(
    value: impl Into<SharedString>,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<CloudSyncPanel>,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .masked(masked)
            .default_value(value)
    })
}

impl CloudSyncPanel {
    pub fn new(
        settings: CloudSyncSettings,
        dispatch: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            endpoint: input(settings.endpoint, false, window, cx),
            region: input(settings.region, false, window, cx),
            bucket: input(settings.bucket, false, window, cx),
            object_key: input(settings.object_key, false, window, cx),
            access_key: input(settings.access_key_id, false, window, cx),
            secret_key: input("", true, window, cx),
            token: input("", true, window, cx),
            password: input("", true, window, cx),
            recovery: input("", true, window, cx),
            virtual_hosted: settings.virtual_hosted,
            automatic: settings.automatic,
            dispatch,
            phase: SyncPhase::Idle,
            status: t!("sync.status.ready"),
            error: false,
            recovery_to_copy: None,
            preview_cloud: false,
        }
    }

    pub(crate) fn input(&self, cx: &App) -> SyncInput {
        SyncInput {
            settings: CloudSyncSettings {
                endpoint: self.endpoint.read(cx).value().to_string(),
                region: self.region.read(cx).value().to_string(),
                bucket: self.bucket.read(cx).value().to_string(),
                object_key: self.object_key.read(cx).value().to_string(),
                access_key_id: self.access_key.read(cx).value().to_string(),
                virtual_hosted: self.virtual_hosted,
                automatic: self.automatic,
            },
            secret_key: Zeroizing::new(self.secret_key.read(cx).value().to_string()),
            token: Zeroizing::new(self.token.read(cx).value().to_string()),
            password: Zeroizing::new(self.password.read(cx).value().to_string()),
            recovery: Zeroizing::new(self.recovery.read(cx).value().to_string()),
        }
    }

    pub(crate) fn phase(&self) -> SyncPhase {
        self.phase
    }

    pub(crate) fn set_status(
        &mut self,
        phase: SyncPhase,
        status: SharedString,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.phase = phase;
        self.status = status;
        self.error = error;
        cx.notify();
    }

    pub(crate) fn set_recovery(&mut self, key: Zeroizing<String>) {
        self.recovery_to_copy = Some(key);
    }
    pub(crate) fn reset_recovery(&mut self) {
        self.recovery_to_copy = None;
    }
    pub(crate) fn set_preview_cloud(&mut self, cloud: bool) {
        self.preview_cloud = cloud;
    }
    pub(crate) fn recovery(&self) -> Option<Zeroizing<String>> {
        self.recovery_to_copy.clone()
    }

    pub(crate) fn clear_access_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 更换实体同时丢弃输入历史，清空后无法通过撤销找回秘密。
        self.secret_key = input("", true, window, cx);
        self.token = input("", true, window, cx);
        cx.notify();
    }

    pub(crate) fn lock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_unlock_inputs(window, cx);
        self.set_status(SyncPhase::Idle, t!("sync.status.locked"), false, cx);
    }

    pub(crate) fn clear_unlock_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_access_inputs(window, cx);
        self.password = input("", true, window, cx);
        self.recovery = input("", true, window, cx);
        self.recovery_to_copy = None;
        if self.phase != SyncPhase::Busy {
            self.set_status(SyncPhase::Idle, t!("sync.status.locked"), false, cx);
        }
    }

    fn field(
        &self,
        id: &'static str,
        label: &'static str,
        state: &Entity<InputState>,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .child(div().text_sm().child(t!(label)))
            .child(
                Input::new(state)
                    .id(id)
                    .aria_label(t!(label))
                    .small()
                    .w_full()
                    .disabled(self.phase == SyncPhase::Busy || self.phase == SyncPhase::Preview),
            )
    }

    fn command(&self, id: &'static str, label: &'static str, action: impl Action) -> Button {
        let focus = self.dispatch.clone();
        Button::new(id)
            .small()
            .outline()
            .label(t!(label))
            .disabled(self.phase == SyncPhase::Busy)
            .on_click(move |_, window, cx| {
                let focus = focus.clone();
                let action = action.boxed_clone();
                window.defer(cx, move |window, cx| {
                    focus.dispatch_action(action.as_ref(), window, cx)
                });
            })
    }
}

impl Render for CloudSyncPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.phase == SyncPhase::Busy;
        let preview = self.phase == SyncPhase::Preview;
        v_flex()
            .id("cloud-sync")
            .test_support()
            .w_full()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("sync.description")),
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(self.field("sync-endpoint", "sync.field.endpoint", &self.endpoint))
                    .child(
                        h_flex()
                            .items_start()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child(self.field(
                                "sync-region",
                                "sync.field.region",
                                &self.region,
                            )))
                            .child(div().flex_1().min_w_0().child(self.field(
                                "sync-bucket",
                                "sync.field.bucket",
                                &self.bucket,
                            ))),
                    )
                    .child(self.field("sync-object-key", "sync.field.object_key", &self.object_key))
                    .child(self.field("sync-access-key", "sync.field.access_key", &self.access_key))
                    .child(self.field("sync-secret-key", "sync.field.secret_key", &self.secret_key))
                    .child(self.field("sync-token", "sync.field.token", &self.token))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("sync.credentials_help")),
                    )
                    .child(
                        Checkbox::new("sync-virtual-hosted")
                            .small()
                            .label(t!("sync.field.virtual_hosted"))
                            .checked(self.virtual_hosted)
                            .disabled(busy || preview)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.virtual_hosted = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("sync-automatic")
                            .small()
                            .label(t!("sync.automatic"))
                            .checked(self.automatic)
                            .disabled(busy || preview)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                this.automatic = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("sync.automatic_help")),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                self.command("sync-save", "sync.save", SaveCloudSyncSettings)
                                    .disabled(busy || preview),
                            )
                            .child(
                                self.command(
                                    "sync-clear-token",
                                    "sync.clear_token",
                                    ClearCloudSessionToken,
                                )
                                .disabled(busy || preview),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(self.field("sync-password", "sync.field.password", &self.password))
                    .child(self.field("sync-recovery", "sync.field.recovery", &self.recovery))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("sync.unlock_help")),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                self.command("sync-upload", "sync.upload", UploadCloudVault)
                                    .disabled(busy || preview),
                            )
                            .child(
                                self.command("sync-download", "sync.download", DownloadCloudVault)
                                    .disabled(busy || preview),
                            )
                            .child(
                                self.command(
                                    "sync-import-backup",
                                    "sync.import_backup",
                                    ImportCloudBackup,
                                )
                                .disabled(busy || preview),
                            )
                            .child(
                                self.command(
                                    "sync-copy-recovery",
                                    "sync.copy_recovery",
                                    CopyCloudRecoveryKey,
                                )
                                .disabled(busy || preview),
                            )
                            .child(self.command("sync-lock", "sync.lock", LockCloudVault)),
                    ),
            )
            .child(
                div()
                    .id("sync-status")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(self.status.clone())
                    .text_sm()
                    .text_color(if self.error {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(self.status.clone()),
            )
            .when(preview, |view| {
                view.child(self.command("sync-apply", "sync.apply", ApplyCloudVault))
            })
            .when(preview && self.preview_cloud, |view| {
                view.child(self.command("sync-keep-local", "sync.keep_local", KeepLocalCloudVault))
            })
    }
}
