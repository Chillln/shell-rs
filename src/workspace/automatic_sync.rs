//! 启动、数据变更和退出触发自动同步，网络与钥匙串操作全部在后台。

use std::{rc::Rc, time::Duration};

use anyhow::{Result, ensure};
use gpui_kit::{
    component::{WindowExt as _, notification::Notification},
    *,
};

use super::Workspace;
use crate::{
    app::ConfirmQuit,
    cloud_sync::{AutomaticSync, SyncPhase, inventory_stamp},
    i18n::t,
    shared::confirm_danger,
};

impl Workspace {
    pub(super) fn on_sync_inventory_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stamp = inventory_stamp(&self.store.read(cx).inventory());
        if stamp != self.auto_sync_stamp {
            self.auto_sync_stamp = stamp;
            self.request_automatic_sync(window, cx);
        }
    }

    fn automatic_enabled(&self, cx: &App) -> bool {
        let settings = self.settings().read(cx).settings().cloud_sync;
        self.cloud_sync_services.available() && settings.automatic && settings.validated().is_ok()
    }

    pub(super) fn request_automatic_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.automatic_enabled(cx) {
            if self.exit_requested {
                cx.quit();
            }
            return;
        }
        self.auto_sync_requested = true;
        if self.auto_sync_running {
            return;
        }
        // 连续编辑合并为一次上传，等待密码写入完成后才取同步快照。
        self.auto_sync_delay = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                let waiting = this
                    .update_in(cx, |this, window, cx| {
                        if !this.automatic_enabled(cx) {
                            this.auto_sync_requested = false;
                            if this.exit_requested {
                                cx.quit();
                            }
                            return false;
                        }
                        if this.store.read(cx).has_pending_secrets()
                            || this.cloud_sync.read(cx).phase() == SyncPhase::Busy
                        {
                            return true;
                        }
                        if this.cloud_sync.read(cx).phase() == SyncPhase::Preview {
                            if this.exit_requested {
                                this.ask_exit_sync_failed(
                                    t!("sync.error.pending_preview"),
                                    window,
                                    cx,
                                );
                            }
                            return false;
                        }
                        this.start_automatic_sync(window, cx);
                        false
                    })
                    .unwrap_or(false);
                if !waiting {
                    break;
                }
            }
        }));
    }

    fn start_automatic_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.auto_sync_running || !self.auto_sync_requested {
            return;
        }
        self.auto_sync_requested = false;
        self.auto_sync_running = true;
        self.cloud_sync.update(cx, |panel, cx| {
            panel.set_status(SyncPhase::Busy, t!("sync.status.automatic"), false, cx)
        });
        let settings = self.settings().read(cx).settings().cloud_sync;
        let local = self.store.read(cx).inventory();
        let stamp = inventory_stamp(&local);
        let services = self.cloud_sync_services.clone();
        let secrets = self.store.read(cx).secrets();
        let key_dir = self.store.read(cx).key_dir().map(ToOwned::to_owned);
        self.auto_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
            let worker_settings = settings.clone();
            let worker_services = services.clone();
            let worker_local = local.clone();
            let worker_secrets = secrets.clone();
            let result = cx
                .background_spawn(async move {
                    worker_services.automatic(&worker_settings, &worker_local, worker_secrets)
                })
                .await;
            let result = match result {
                Ok(AutomaticSync::Download(download)) => {
                    // 正在连接时保留远端数据，稍后重试；不能替换仍在使用的凭据。
                    let ready = this
                        .update_in(cx, |this, _, cx| -> Result<()> {
                            ensure!(
                                this.terminals.is_empty()
                                    && this.explorers.is_empty()
                                    && this.forwards.read(cx).active_count() == 0,
                                t!("sync.error.connections")
                            );
                            ensure!(
                                inventory_stamp(&this.store.read(cx).inventory()) == stamp
                                    && !this.store.read(cx).has_pending_secrets(),
                                t!("sync.error.local_changed")
                            );
                            Ok(())
                        })
                        .and_then(|result| result);
                    match ready {
                        Err(error) => Err(error),
                        Ok(()) => {
                            let prepared = cx
                                .background_spawn(async move {
                                    services.stage_import(
                                        *download,
                                        &local,
                                        key_dir.as_deref(),
                                        secrets,
                                    )
                                })
                                .await;
                            match prepared {
                                Err(error) => Err(error),
                                Ok(stage) => {
                                    let imported = this
                                        .update_in(cx, |this, _, cx| -> Result<()> {
                                            ensure!(
                                                this.settings().read(cx).settings().cloud_sync
                                                    == settings
                                                    && this.terminals.is_empty()
                                                    && this.explorers.is_empty()
                                                    && this.forwards.read(cx).active_count() == 0
                                                    && !this.store.read(cx).has_pending_secrets()
                                                    && inventory_stamp(
                                                        &this.store.read(cx).inventory()
                                                    ) == stamp,
                                                t!("sync.error.local_changed")
                                            );
                                            this.store.update(cx, |store, cx| {
                                                store.replace_inventory(stage.inventory.clone(), cx)
                                            })?;
                                            this.auto_sync_stamp =
                                                inventory_stamp(&this.store.read(cx).inventory());
                                            Ok(())
                                        })
                                        .and_then(|result| result);
                                    cx.background_spawn(async move {
                                        match imported {
                                            Ok(()) => stage.commit(),
                                            Err(error) => {
                                                stage.rollback()?;
                                                Err(error)
                                            }
                                        }
                                    })
                                    .await
                                }
                            }
                        }
                    }
                }
                Ok(_) => Ok(()),
                Err(error) => Err(error),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_automatic_sync(result, window, cx)
            });
        }));
    }

    fn finish_automatic_sync(
        &mut self,
        result: Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.auto_sync_running = false;
        if self.auto_start_forwards {
            self.auto_start_forwards = false;
            self.forwards
                .update(cx, |forwards, cx| forwards.start_automatic(cx));
        }
        self.cloud_sync.update(cx, |panel, cx| {
            panel.set_status(
                SyncPhase::Idle,
                match &result {
                    Ok(()) => t!("sync.status.automatic_done"),
                    Err(error) => error.to_string().into(),
                },
                result.is_err(),
                cx,
            )
        });
        if let Err(error) = result {
            if self.exit_requested {
                self.ask_exit_sync_failed(error, window, cx);
            } else {
                window.push_notification(
                    Notification::error(error.to_string()).title(t!("sync.title")),
                    cx,
                );
                // 运行期间收到的新修改再尝试一次；没有新修改时不自动循环重试。
                if self.auto_sync_requested {
                    let entity = cx.weak_entity();
                    window.defer(cx, move |window, cx| {
                        let _ =
                            entity.update(cx, |this, cx| this.request_automatic_sync(window, cx));
                    });
                }
            }
            // 冲突与网络故障等待下次启动、退出或用户修改重试，不循环覆盖或刷通知。
            return;
        }
        if self.auto_sync_requested || self.store.read(cx).has_pending_secrets() {
            let entity = cx.weak_entity();
            window.defer(cx, move |window, cx| {
                let _ = entity.update(cx, |this, cx| this.request_automatic_sync(window, cx));
            });
        } else if self.exit_requested {
            cx.quit();
        }
    }

    pub(super) fn on_confirm_quit(
        &mut self,
        _: &ConfirmQuit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.exit_requested = true;
        if self.automatic_enabled(cx) {
            window.push_notification(
                Notification::info(t!("sync.status.exiting")).title(t!("sync.title")),
                cx,
            );
        }
        self.request_automatic_sync(window, cx);
    }

    fn ask_exit_sync_failed(
        &mut self,
        error: impl std::fmt::Display,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.exit_requested = false;
        confirm_danger(
            t!("sync.exit_failed.title"),
            Some(t!("sync.exit_failed.body", error = error)),
            t!("sync.exit_failed.quit"),
            Rc::new(|_, cx| cx.quit()),
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        cloud_sync::{
            SyncServices, test_open_cloud,
            tests::{MemoryStorage, PASSWORD, settings},
        },
        connection::{ConnectionTester, LoginTest, TrustCallback},
        forward::{ForwardTransport, ForwardTransportProvider},
        host::{
            AuthKind, CredentialDraft, CredentialKind, ForwardRule, GroupDraft, HostDraft,
            HostLogin, HostStore,
        },
        secrets::InMemorySecretStore,
        settings::{InterfaceLanguage, SettingsStore},
        sftp::{
            DirectoryListing, FileStamp, LocalDirectoryProvider, PermissionEdit, SftpTransport,
            SftpTransportProvider,
        },
        terminal::{
            FixedRemoteTerminalTransportProvider, TerminalTransport, TerminalTransportFactory,
        },
        workspace::Workspace,
    };
    use gpui_kit::{
        AppContext as _, Entity, TestAppContext, WindowHandle, component::Root, px, size,
        test::TestWindowExt,
    };
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, atomic::Ordering},
        time::Duration,
    };

    // 所有传输与本地目录均注入禁用实现，误触真实服务会立即使测试失败。
    struct DisabledServices;
    impl TerminalTransportFactory for DisabledServices {
        fn create(&self) -> Box<dyn TerminalTransport> {
            panic!("测试不允许启动终端")
        }
    }
    impl SftpTransportProvider for DisabledServices {
        fn create(&self, _: &HostLogin) -> Box<dyn SftpTransport> {
            panic!("测试不允许连接 SFTP")
        }
    }
    impl ForwardTransportProvider for DisabledServices {
        fn create(&self, _: &ForwardRule, _: &HostLogin) -> Box<dyn ForwardTransport> {
            panic!("测试不允许连接转发")
        }
    }
    impl ConnectionTester for DisabledServices {
        fn test(&self, _: LoginTest, _: TrustCallback) -> Result<(), String> {
            panic!("测试不允许探测服务器")
        }
    }
    impl LocalDirectoryProvider for DisabledServices {
        fn home(&self) -> PathBuf {
            panic!("测试不允许读取主目录")
        }
        fn list(&self, _: &Path) -> anyhow::Result<DirectoryListing> {
            panic!("测试不允许读目录")
        }
        fn trash(&self, _: &[PathBuf]) -> anyhow::Result<()> {
            panic!("测试不允许删除文件")
        }
        fn rename(&self, _: &Path, _: &Path) -> anyhow::Result<()> {
            panic!("测试不允许重命名文件")
        }
        fn create_dir(&self, _: &Path) -> anyhow::Result<()> {
            panic!("测试不允许创建目录")
        }
        fn create_file(&self, _: &Path) -> anyhow::Result<()> {
            panic!("测试不允许创建文件")
        }
        fn set_permissions(
            &self,
            _: &[PathBuf],
            _: PermissionEdit,
            _: bool,
            _: bool,
        ) -> anyhow::Result<()> {
            panic!("测试不允许修改权限")
        }
        fn read_file(&self, _: &Path, _: u64) -> anyhow::Result<(Vec<u8>, FileStamp)> {
            panic!("测试不允许读取文件")
        }
        fn write_file(
            &self,
            _: &Path,
            _: &[u8],
            _: Option<FileStamp>,
        ) -> anyhow::Result<FileStamp> {
            panic!("测试不允许写入文件")
        }
    }

    fn open(
        cx: &mut TestAppContext,
        service: SyncServices,
        secrets: Arc<InMemorySecretStore>,
    ) -> (WindowHandle<Root>, Entity<Workspace>) {
        crate::i18n::isolate_thread();
        crate::i18n::set_locale("zh-CN");
        cx.update(crate::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let mut workspace = None;
        let handle = cx.open_window(size(px(1280.), px(800.)), |window, cx| {
            let store = cx.new(|_| HostStore::empty().with_secrets(secrets));
            let preferences = cx.new(|_| {
                let mut store = SettingsStore::in_memory();
                store.update_unnotified(|prefs| {
                    prefs.cloud_sync = settings();
                    prefs.language = InterfaceLanguage::SimplifiedChinese;
                });
                store
            });
            let view = cx.new(|cx| {
                let mut view = Workspace::new_with_services(
                    store,
                    preferences,
                    Arc::new(FixedRemoteTerminalTransportProvider::new(Arc::new(
                        DisabledServices,
                    ))),
                    Arc::new(DisabledServices),
                    Arc::new(DisabledServices),
                    Arc::new(DisabledServices),
                    Arc::new(DisabledServices),
                    Arc::new(DisabledServices),
                    window,
                    cx,
                );
                view.cloud_sync_services = service;
                view
            });
            workspace = Some(view.clone());
            Root::new(view, window, cx)
        });
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.run_until_parked();
        (handle, workspace.unwrap())
    }

    fn settle(cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn automatic_sync_observes_crud_and_waits_for_saved_passwords(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let service = storage.service(dir.path());
        let secrets = Arc::new(InMemorySecretStore::default());
        service
            .upload(
                &settings(),
                &HostStore::empty().inventory(),
                secrets.clone(),
                PASSWORD,
                "",
            )
            .unwrap();
        let (_, workspace) = open(cx, service, secrets.clone());
        let store = cx.update(|cx| workspace.read(cx).store.clone());
        let (host, credential) = store.update(cx, |store, cx| {
            let group = store.insert_group(GroupDraft::new("自动分组", None), cx);
            let credential = store.insert_credential(
                CredentialDraft::new("自动凭据", CredentialKind::Password, "root"),
                cx,
            );
            let host = store.insert(
                HostDraft::new(
                    "自动主机",
                    "auto.example",
                    22,
                    "root",
                    AuthKind::Password,
                    Some(group),
                )
                .with_credential(credential),
                cx,
            );
            let secret = store.credential(credential).unwrap().password_secret();
            store.save_secret(secret, Some("自动保存的密码".into()), cx);
            assert!(store.has_pending_secrets());
            (host, credential)
        });
        settle(cx);
        let remote = test_open_cloud(&storage);
        assert_eq!(remote.hosts.len(), 1);
        assert_eq!(remote.groups.len(), 1);
        assert_eq!(remote.credentials.len(), 1);
        assert!(!cx.update(|cx| workspace.read(cx).auto_sync_running));
        store.update(cx, |store, cx| {
            let mut draft = store.host(host).unwrap().draft();
            draft.name = "已自动修改".into();
            store.update(host, draft, cx);
            store.save_secret(
                store.credential(credential).unwrap().password_secret(),
                Some("修改后的密码".into()),
                cx,
            );
        });
        settle(cx);
        assert_eq!(test_open_cloud(&storage).hosts[0].name, "已自动修改");
        store.update(cx, |store, cx| {
            store.remove(host, cx);
            store.remove_credential(credential, cx);
        });
        settle(cx);
        assert!(test_open_cloud(&storage).hosts.is_empty());
        assert!(test_open_cloud(&storage).credentials.is_empty());
    }

    #[gpui_kit::test]
    fn startup_sync_applies_remote_only_changes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let service = storage.service(dir.path());
        let secrets = Arc::new(InMemorySecretStore::default());
        service
            .upload(
                &settings(),
                &HostStore::empty().inventory(),
                secrets.clone(),
                PASSWORD,
                "",
            )
            .unwrap();
        let cursor_path = dir
            .path()
            .join(format!("cursor-{}.json", settings().scope()));
        let baseline = std::fs::read(&cursor_path).unwrap();
        let mut changed = HostStore::empty();
        changed.insert_unnotified(HostDraft::new(
            "另一台设备新增",
            "remote.example",
            22,
            "root",
            AuthKind::Password,
            None,
        ));
        service
            .upload(
                &settings(),
                &changed.inventory(),
                secrets.clone(),
                PASSWORD,
                "",
            )
            .unwrap();
        std::fs::write(cursor_path, baseline).unwrap();
        let (handle, workspace) = open(cx, service, secrets);
        cx.update_window(handle.into(), |_, window, cx| {
            workspace.update(cx, |this, cx| this.request_automatic_sync(window, cx));
        })
        .unwrap();
        settle(cx);
        assert_eq!(
            cx.update(|cx| workspace.read(cx).store.read(cx).hosts()[0].name.clone()),
            "另一台设备新增"
        );
    }

    #[gpui_kit::test]
    fn exit_waits_for_sync_and_preserves_local_changes_on_failure(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let service = storage.service(dir.path());
        let secrets = Arc::new(InMemorySecretStore::default());
        service
            .upload(
                &settings(),
                &HostStore::empty().inventory(),
                secrets.clone(),
                PASSWORD,
                "",
            )
            .unwrap();
        let (handle, workspace) = open(cx, service, secrets.clone());
        let store = cx.update(|cx| workspace.read(cx).store.clone());
        store.update(cx, |store, cx| {
            store.insert(
                HostDraft::new(
                    "退出前新增",
                    "exit.example",
                    22,
                    "root",
                    AuthKind::Password,
                    None,
                ),
                cx,
            );
        });
        storage.force_conflict.store(true, Ordering::SeqCst);
        cx.update_window(handle.into(), |_, window, cx| {
            workspace.update(cx, |this, cx| {
                this.on_confirm_quit(&crate::app::ConfirmQuit, window, cx)
            });
        })
        .unwrap();
        assert!(cx.update(|cx| workspace.read(cx).exit_requested));
        settle(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("ok").visible());
            assert!(!workspace.read(cx).exit_requested);
            window.click("cancel", cx);
        })
        .unwrap();
        assert_eq!(cx.update(|cx| store.read(cx).hosts().len()), 1);
        assert!(test_open_cloud(&storage).hosts.is_empty());
        storage.force_conflict.store(false, Ordering::SeqCst);
        cx.update_window(handle.into(), |_, window, cx| {
            workspace.update(cx, |this, cx| {
                this.on_confirm_quit(&crate::app::ConfirmQuit, window, cx)
            });
        })
        .unwrap();
        settle(cx);
        assert_eq!(test_open_cloud(&storage).hosts[0].name, "退出前新增");
    }
}
