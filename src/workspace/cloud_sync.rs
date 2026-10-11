//! 将云同步命令路由到功能服务，保持窗口壳只负责实体与确认流程。

use std::{path::PathBuf, rc::Rc};

use anyhow::{Result, ensure};
use gpui_kit::{
    component::{WindowExt as _, notification::Notification},
    *,
};

use super::Workspace;
use crate::{
    app::{
        ApplyCloudVault, ClearCloudSessionToken, CommitCloudVault, CommitLocalCloudVault,
        CopyCloudRecoveryKey, DownloadCloudVault, ImportCloudBackup, KeepLocalCloudVault,
        LockCloudVault, SaveCloudSyncSettings, UploadCloudVault,
    },
    cloud_sync::{Download, SyncInput, SyncPhase, SyncServices, Upload, inventory_stamp},
    i18n::t,
    settings::SettingsStoreEvent,
    shared::confirm_danger,
};

enum Transfer {
    Upload,
    Download,
    Backup(PathBuf),
}
enum TransferResult {
    Upload(Upload),
    Download(Box<Download>),
}

impl Workspace {
    fn cloud_sync_status(
        &self,
        phase: SyncPhase,
        message: SharedString,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.cloud_sync
            .update(cx, |panel, cx| panel.set_status(phase, message, error, cx));
    }

    fn cloud_sync_error(&self, error: impl std::fmt::Display, cx: &mut Context<Self>) {
        let phase = if self.cloud_sync_pending.is_some() {
            SyncPhase::Preview
        } else {
            SyncPhase::Idle
        };
        self.cloud_sync_status(phase, error.to_string().into(), true, cx);
    }

    fn cloud_sync_input(&self, needs_settings: bool, cx: &App) -> Result<SyncInput> {
        let mut input = self.cloud_sync.read(cx).input(cx);
        if needs_settings {
            input.settings = input.settings.validated()?;
            ensure!(
                input.settings == self.settings().read(cx).settings().cloud_sync
                    && input.secret_key.is_empty()
                    && input.token.is_empty(),
                t!("sync.error.save_first")
            );
        }
        ensure!(
            !input.password.is_empty() || !input.recovery.trim().is_empty(),
            t!("sync.error.password")
        );
        Ok(input)
    }

    fn saved_cloud_settings(&self, cx: &App) -> Result<crate::cloud_sync::CloudSyncSettings> {
        let input = self.cloud_sync.read(cx).input(cx);
        let settings = input.settings.validated()?;
        ensure!(
            settings == self.settings().read(cx).settings().cloud_sync
                && input.secret_key.is_empty()
                && input.token.is_empty(),
            t!("sync.error.save_first")
        );
        Ok(settings)
    }

    pub(super) fn on_save_cloud_sync_settings(
        &mut self,
        _: &SaveCloudSyncSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Idle {
            return;
        }
        let input = self.cloud_sync.read(cx).input(cx);
        let settings = match input.settings.validated() {
            Ok(settings) => settings,
            Err(error) => {
                self.cloud_sync_error(error, cx);
                return;
            }
        };
        let secrets = self.store.read(cx).secrets();
        self.cloud_sync_status(SyncPhase::Busy, t!("sync.status.saving"), false, cx);
        self.cloud_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            let saved_settings = settings.clone();
            let result = cx
                .background_spawn(async move {
                    SyncServices::save_credentials(
                        &settings,
                        &input.secret_key,
                        &input.token,
                        &secrets,
                    )
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    let saved = this.settings().update(cx, |store, cx| {
                        let result = store
                            .update_unnotified(|settings| settings.cloud_sync = saved_settings);
                        cx.notify();
                        match result {
                            Some(Err(error)) => {
                                let message = t!("settings.store.save_failed", error = error);
                                cx.emit(SettingsStoreEvent::PersistFailed(message.clone()));
                                Err(message)
                            }
                            _ => Ok(()),
                        }
                    });
                    this.cloud_sync.update(cx, |panel, cx| {
                        panel.clear_access_inputs(window, cx);
                        panel.reset_recovery();
                    });
                    match saved {
                        Ok(()) => {
                            this.cloud_sync_status(
                                SyncPhase::Idle,
                                t!("sync.status.saved"),
                                false,
                                cx,
                            );
                            this.request_automatic_sync(window, cx);
                        }
                        Err(error) => this.cloud_sync_error(error, cx),
                    }
                }
                Err(error) => this.cloud_sync_error(error, cx),
            });
        }));
    }

    pub(super) fn on_upload_cloud_vault(
        &mut self,
        _: &UploadCloudVault,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_cloud_transfer(Transfer::Upload, window, cx);
    }

    pub(super) fn on_download_cloud_vault(
        &mut self,
        _: &DownloadCloudVault,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_cloud_transfer(Transfer::Download, window, cx);
    }

    fn start_cloud_transfer(
        &mut self,
        transfer: Transfer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Idle {
            return;
        }
        let input = match self.cloud_sync_input(!matches!(transfer, Transfer::Backup(_)), cx) {
            Ok(input) => input,
            Err(error) => {
                self.cloud_sync_error(error, cx);
                return;
            }
        };
        let inventory = self.store.read(cx).inventory();
        let local_counts = (
            inventory.hosts.len(),
            inventory.groups.len(),
            inventory.credentials.len(),
        );
        let services = self.cloud_sync_services.clone();
        let secrets = self.store.read(cx).secrets();
        self.cloud_sync_status(SyncPhase::Busy, t!("sync.status.working"), false, cx);
        self.cloud_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match transfer {
                        Transfer::Upload => services
                            .upload(
                                &input.settings,
                                &inventory,
                                secrets,
                                &input.password,
                                &input.recovery,
                            )
                            .map(TransferResult::Upload),
                        Transfer::Download => services
                            .download(
                                &input.settings,
                                &inventory,
                                secrets,
                                &input.password,
                                &input.recovery,
                            )
                            .map(|download| TransferResult::Download(Box::new(download))),
                        Transfer::Backup(path) => services
                            .import_backup(
                                &path,
                                &inventory,
                                secrets,
                                &input.password,
                                &input.recovery,
                            )
                            .map(|download| TransferResult::Download(Box::new(download))),
                    }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.settings_tab().is_none() {
                    if let Err(error) = result {
                        window.push_notification(
                            Notification::error(error.to_string()).title(t!("sync.title")),
                            cx,
                        );
                    }
                    this.cloud_sync
                        .update(cx, |panel, cx| panel.lock(window, cx));
                    return;
                }
                match result {
                    Ok(TransferResult::Upload(upload)) => {
                        let message = if let Some(recovery) = upload.recovery {
                            this.cloud_sync
                                .update(cx, |panel, _| panel.set_recovery(recovery));
                            t!("sync.status.created", path = upload.backup.display())
                        } else {
                            t!("sync.status.uploaded", path = upload.backup.display())
                        };
                        this.cloud_sync_status(SyncPhase::Idle, message, false, cx);
                    }
                    Ok(TransferResult::Download(download)) => {
                        this.cloud_sync
                            .update(cx, |panel, _| panel.set_preview_cloud(download.is_cloud()));
                        let (hosts, groups, credentials) = download.counts();
                        let message = t!(
                            "sync.status.preview",
                            hosts = hosts,
                            groups = groups,
                            credentials = credentials,
                            local_hosts = local_counts.0,
                            local_groups = local_counts.1,
                            local_credentials = local_counts.2,
                            path = download.backup.display()
                        );
                        this.cloud_sync_pending = Some(*download);
                        this.cloud_sync_status(SyncPhase::Preview, message, false, cx);
                    }
                    Err(error) => this.cloud_sync_error(error, cx),
                }
            });
        }));
    }

    pub(super) fn on_import_cloud_backup(
        &mut self,
        _: &ImportCloudBackup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Idle {
            return;
        }
        if let Err(error) = self.cloud_sync_input(false, cx) {
            self.cloud_sync_error(error, cx);
            return;
        }
        let choice = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("sync.import_backup")),
        });
        self.cloud_sync_status(SyncPhase::Busy, t!("sync.status.choosing"), false, cx);
        self.cloud_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            let path = choice.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.cloud_sync_status(SyncPhase::Idle, t!("sync.status.ready"), false, cx);
                match path {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.start_cloud_transfer(Transfer::Backup(path), window, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => this.cloud_sync_error(t!("sync.error.file_picker"), cx),
                }
            });
        }));
    }

    fn cloud_import_available(&self, cx: &App) -> Result<()> {
        ensure!(
            self.terminals.is_empty()
                && self.explorers.is_empty()
                && self.forwards.read(cx).active_count() == 0,
            t!("sync.error.connections")
        );
        let download = self
            .cloud_sync_pending
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.no_preview")))?;
        ensure!(
            inventory_stamp(&self.store.read(cx).inventory()) == download.stamp,
            t!("sync.error.local_changed")
        );
        Ok(())
    }

    pub(super) fn on_apply_cloud_vault(
        &mut self,
        _: &ApplyCloudVault,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Preview {
            return;
        }
        if let Err(error) = self.cloud_import_available(cx) {
            self.cloud_sync_error(error, cx);
            return;
        }
        let (hosts, groups, credentials) = self
            .cloud_sync_pending
            .as_ref()
            .expect("vault preview exists")
            .counts();
        let focus = self.focus_handle.clone();
        confirm_danger(
            t!("sync.confirm.title"),
            Some(t!(
                "sync.confirm.body",
                hosts = hosts,
                groups = groups,
                credentials = credentials
            )),
            t!("sync.confirm.apply"),
            Rc::new(move |window, cx| {
                let focus = focus.clone();
                window.defer(cx, move |window, cx| {
                    focus.dispatch_action(&CommitCloudVault, window, cx)
                });
            }),
            window,
            cx,
        );
    }

    pub(super) fn on_commit_cloud_vault(
        &mut self,
        _: &CommitCloudVault,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Preview {
            return;
        }
        if let Err(error) = self.cloud_import_available(cx) {
            self.cloud_sync_error(error, cx);
            return;
        }
        let download = self
            .cloud_sync_pending
            .take()
            .expect("vault preview exists");
        let backup = download.backup.clone();
        let stamp = download.stamp;
        let local = self.store.read(cx).inventory();
        let key_dir = self.store.read(cx).key_dir().map(ToOwned::to_owned);
        let secrets = self.store.read(cx).secrets();
        let services = self.cloud_sync_services.clone();
        self.cloud_sync_status(SyncPhase::Busy, t!("sync.status.applying"), false, cx);
        self.cloud_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            let prepared = cx
                .background_spawn(async move {
                    services.stage_import(download, &local, key_dir.as_deref(), secrets)
                })
                .await;
            let stage = match prepared {
                Ok(stage) => stage,
                Err(error) => {
                    let _ = this.update_in(cx, |this, _, cx| this.cloud_sync_error(error, cx));
                    return;
                }
            };
            let imported = this.update_in(cx, |this, _, cx| -> Result<()> {
                ensure!(
                    this.terminals.is_empty()
                        && this.explorers.is_empty()
                        && this.forwards.read(cx).active_count() == 0
                        && inventory_stamp(&this.store.read(cx).inventory()) == stamp,
                    t!("sync.error.local_changed")
                );
                this.store.update(cx, |store, cx| {
                    store.replace_inventory(stage.inventory.clone(), cx)
                })?;
                Ok(())
            });
            let imported = imported.and_then(|result| result);
            let result = cx
                .background_spawn(async move {
                    match imported {
                        Ok(()) => stage.commit(),
                        Err(error) => match stage.rollback() {
                            Ok(()) => Err(error),
                            Err(_) => Err(anyhow::anyhow!(t!(
                                "sync.error.rollback_failed",
                                path = backup.display()
                            ))),
                        },
                    }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => this.cloud_sync_status(
                        SyncPhase::Idle,
                        t!("sync.status.applied"),
                        false,
                        cx,
                    ),
                    Err(error) => this.cloud_sync_error(error, cx),
                }
                if this.settings_tab().is_none() {
                    this.cloud_sync
                        .update(cx, |panel, cx| panel.lock(window, cx));
                }
            });
        }));
    }

    pub(super) fn on_lock_cloud_vault(
        &mut self,
        _: &LockCloudVault,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() == SyncPhase::Busy {
            return;
        }
        self.cloud_sync_pending = None;
        self.cloud_sync
            .update(cx, |panel, cx| panel.lock(window, cx));
        if self.auto_sync_requested {
            self.request_automatic_sync(window, cx);
        }
    }

    pub(super) fn on_keep_local_cloud_vault(
        &mut self,
        _: &KeepLocalCloudVault,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Preview
            || !self
                .cloud_sync_pending
                .as_ref()
                .is_some_and(Download::is_cloud)
        {
            return;
        }
        let local = self.store.read(cx).inventory();
        let focus = self.focus_handle.clone();
        confirm_danger(
            t!("sync.keep_local.title"),
            Some(t!(
                "sync.keep_local.body",
                hosts = local.hosts.len(),
                groups = local.groups.len(),
                credentials = local.credentials.len()
            )),
            t!("sync.keep_local.confirm"),
            Rc::new(move |window, cx| {
                let focus = focus.clone();
                window.defer(cx, move |window, cx| {
                    focus.dispatch_action(&CommitLocalCloudVault, window, cx)
                });
            }),
            window,
            cx,
        );
    }

    pub(super) fn on_commit_local_cloud_vault(
        &mut self,
        _: &CommitLocalCloudVault,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Preview
            || !self
                .cloud_sync_pending
                .as_ref()
                .is_some_and(Download::is_cloud)
        {
            return;
        }
        let download = self
            .cloud_sync_pending
            .take()
            .expect("cloud preview exists");
        let local = self.store.read(cx).inventory();
        let settings = self.settings().read(cx).settings().cloud_sync;
        let secrets = self.store.read(cx).secrets();
        let services = self.cloud_sync_services.clone();
        self.cloud_sync_status(SyncPhase::Busy, t!("sync.status.working"), false, cx);
        self.cloud_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    services.keep_local(&settings, download, &local, secrets)
                })
                .await;
            let _ = this.update_in(cx, |this, _, cx| match result {
                Ok(upload) => this.cloud_sync_status(
                    SyncPhase::Idle,
                    t!("sync.status.uploaded", path = upload.backup.display()),
                    false,
                    cx,
                ),
                Err(error) => this.cloud_sync_error(error, cx),
            });
        }));
    }

    pub(super) fn on_copy_cloud_recovery_key(
        &mut self,
        _: &CopyCloudRecoveryKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Idle {
            return;
        }
        let settings = match self.saved_cloud_settings(cx) {
            Ok(settings) => settings,
            Err(error) => {
                self.cloud_sync_error(error, cx);
                return;
            }
        };
        if let Some(key) = self.cloud_sync.read(cx).recovery() {
            cx.write_to_clipboard(ClipboardItem::new_string(key.to_string()));
            self.cloud_sync_status(
                SyncPhase::Idle,
                t!("sync.status.recovery_copied"),
                false,
                cx,
            );
            return;
        }
        let secrets = self.store.read(cx).secrets();
        self.cloud_sync_status(SyncPhase::Busy, t!("sync.status.working"), false, cx);
        self.cloud_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { secrets.get(&settings.secret("recovery-key")) })
                .await;
            let _ = this.update_in(cx, |this, _, cx| match result {
                Ok(Some(key)) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(key.to_string()));
                    this.cloud_sync_status(
                        SyncPhase::Idle,
                        t!("sync.status.recovery_copied"),
                        false,
                        cx,
                    );
                }
                Ok(None) => this.cloud_sync_error(t!("sync.error.no_recovery"), cx),
                Err(error) => this.cloud_sync_error(error, cx),
            });
        }));
    }

    pub(super) fn on_clear_cloud_session_token(
        &mut self,
        _: &ClearCloudSessionToken,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_sync.read(cx).phase() != SyncPhase::Idle {
            return;
        }
        let settings = match self.saved_cloud_settings(cx) {
            Ok(settings) => settings,
            Err(error) => {
                self.cloud_sync_error(error, cx);
                return;
            }
        };
        let secrets = self.store.read(cx).secrets();
        self.cloud_sync_status(SyncPhase::Busy, t!("sync.status.working"), false, cx);
        self.cloud_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { secrets.delete(&settings.secret("session-token")) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.cloud_sync
                        .update(cx, |panel, cx| panel.clear_access_inputs(window, cx));
                    this.cloud_sync_status(
                        SyncPhase::Idle,
                        t!("sync.status.token_cleared"),
                        false,
                        cx,
                    );
                }
                Err(error) => this.cloud_sync_error(error, cx),
            });
        }));
    }
}
