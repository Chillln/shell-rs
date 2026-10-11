use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    crypto::{self, MAX_ENCRYPTED_BYTES, OpenVault},
    model::CloudSyncSettings,
    storage::{ObjectVersion, S3Storage, VaultStorage},
    vault::Vault,
};
use crate::{
    host::HostInventory,
    i18n::t,
    secrets::{SecretRef, SharedSecretStore},
};

#[derive(Clone, Default)]
pub(crate) struct SyncServices {
    directory: Option<PathBuf>,
    storage: Option<Arc<dyn VaultStorage>>,
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    etag: String,
    vault_id: String,
    revision: u64,
    #[serde(default)]
    digest: Option<[u8; 32]>,
}

pub(crate) enum AutomaticSync {
    Unchanged,
    Uploaded,
    Download(Box<Download>),
}

pub(crate) struct Upload {
    pub backup: PathBuf,
    pub recovery: Option<Zeroizing<String>>,
}

pub(crate) struct Download {
    automatic: bool,
    open: OpenVault,
    version: Option<ObjectVersion>,
    scope: String,
    pub stamp: [u8; 32],
    digest: [u8; 32],
    pub backup: PathBuf,
}

/// 导入提交前保留补偿信息；掉落时仅在独立线程触碰钥匙串。
pub(crate) struct ImportStage {
    pub inventory: HostInventory,
    undo: Vec<SecretUndo>,
    files: Vec<PathBuf>,
    secrets: SharedSecretStore,
    cursor: Option<Cursor>,
    cursor_path: Option<PathBuf>,
    committed: bool,
    device_key: Option<(SecretRef, Zeroizing<String>)>,
}

struct SecretUndo {
    secret: SecretRef,
    before: Option<Zeroizing<String>>,
    after: Option<Zeroizing<String>>,
}

pub(crate) fn inventory_stamp(inventory: &HostInventory) -> [u8; 32] {
    // 此处只有主机元数据，摘要用于拒绝后台工作期间的过期导入。
    let groups: Vec<_> = inventory
        .groups
        .iter()
        .map(|group| {
            (
                group.id,
                &group.public_id,
                &group.name,
                group.parent,
                group.sort_order,
            )
        })
        .collect();
    let hosts: Vec<_> = inventory
        .hosts
        .iter()
        .map(|host| {
            (
                host.id,
                &host.public_id,
                &host.name,
                &host.address,
                host.port,
                &host.user,
                host.auth,
                host.credential,
                &host.route,
                host.group,
                &host.notes,
                host.sort_order,
            )
        })
        .collect();
    Sha256::digest(
        format!(
            "{:?}{:?}{:?}{:?}:{}",
            groups, hosts, inventory.credentials, inventory.next_ids, inventory.secret_revision
        )
        .as_bytes(),
    )
    .into()
}

fn vault_digest(vault: &Vault) -> Result<[u8; 32]> {
    let plain = Zeroizing::new(serde_json::to_vec(vault)?);
    Ok(Sha256::digest(&plain).into())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

impl SyncServices {
    pub fn available(&self) -> bool {
        self.directory.is_some()
    }

    /// 根据最近一次成功同步的基线判断方向，双方都有修改时保留冲突副本。
    pub fn automatic(
        &self,
        settings: &CloudSyncSettings,
        local: &HostInventory,
        secrets: SharedSecretStore,
    ) -> Result<AutomaticSync> {
        let scope = settings.scope();
        let key = secrets
            .get(&settings.secret("device-key"))?
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.automatic_locked")))?;
        let cursor = self
            .cursor(&scope)?
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.automatic_locked")))?;
        let baseline = cursor
            .digest
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.automatic_locked")))?;
        let remote = self
            .storage(settings, &secrets)?
            .get()?
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.remote_deleted")))?;
        let open = OpenVault::open_device(&remote.bytes, &key)?;
        self.check_remote(&scope, &open)?;
        let current = Vault::capture(local, secrets.as_ref())?;
        let local_digest = open.content_digest(&current)?;
        let remote_digest = open.content_digest(&open.vault)?;
        if local_digest == remote_digest {
            self.save_cursor(&scope, &remote.etag, &open, local_digest)?;
            return Ok(AutomaticSync::Unchanged);
        }
        if local_digest == baseline {
            return self
                .prepare_download(
                    open,
                    Some(ObjectVersion {
                        etag: remote.etag,
                        version: remote.version,
                    }),
                    scope,
                    local,
                    secrets,
                )
                .map(|download| AutomaticSync::Download(Box::new(download)));
        }
        if remote.etag == cursor.etag && remote_digest == baseline {
            self.upload(settings, local, secrets, "", "")?;
            return Ok(AutomaticSync::Uploaded);
        }
        self.backup(&remote.bytes, "cloud-conflict")?;
        let backup = self.backup(&open.seal(&current)?, "local-conflict")?;
        anyhow::bail!(t!("sync.error.conflict_backup", path = backup.display()))
    }

    fn save_cursor(
        &self,
        scope: &str,
        etag: &str,
        open: &OpenVault,
        digest: [u8; 32],
    ) -> Result<()> {
        atomic_write(
            &self.cursor_path(scope)?,
            &serde_json::to_vec(&Cursor {
                etag: etag.to_string(),
                vault_id: open.id().to_string(),
                revision: open.vault.revision,
                digest: Some(digest),
            })?,
        )
    }

    fn remember_key(
        settings: &CloudSyncSettings,
        open: &OpenVault,
        secrets: &SharedSecretStore,
    ) -> Result<()> {
        if settings.automatic {
            secrets.set(&settings.secret("device-key"), &open.device_key()?)?;
        }
        Ok(())
    }

    fn unlock(
        settings: &CloudSyncSettings,
        bytes: &[u8],
        secrets: &SharedSecretStore,
        password: &str,
        recovery: &str,
    ) -> Result<OpenVault> {
        if password.is_empty() && recovery.is_empty() {
            let saved = secrets
                .get(&settings.secret("device-key"))?
                .ok_or_else(|| anyhow::anyhow!(t!("sync.error.automatic_locked")))?;
            OpenVault::open_device(bytes, &saved)
        } else {
            crypto::open(bytes, password, recovery)
        }
    }

    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory: Some(directory),
            storage: None,
        }
    }
    #[cfg(test)]
    pub(super) fn with_storage(directory: PathBuf, storage: Arc<dyn VaultStorage>) -> Self {
        Self {
            directory: Some(directory),
            storage: Some(storage),
        }
    }

    fn directory(&self) -> Result<&Path> {
        self.directory
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.unavailable")))
    }

    fn storage(
        &self,
        settings: &CloudSyncSettings,
        secrets: &SharedSecretStore,
    ) -> Result<Arc<dyn VaultStorage>> {
        match &self.storage {
            Some(storage) => Ok(storage.clone()),
            None => S3Storage::build(settings, secrets.as_ref()),
        }
    }

    fn cursor_path(&self, scope: &str) -> Result<PathBuf> {
        Ok(self.directory()?.join(format!("cursor-{scope}.json")))
    }

    fn cursor(&self, scope: &str) -> Result<Option<Cursor>> {
        match fs::read(self.cursor_path(scope)?) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!(t!("sync.error.cursor")))?,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn backup(&self, bytes: &[u8], reason: &str) -> Result<PathBuf> {
        let path = self.directory()?.join("backups").join(format!(
            "{}-{reason}-{}.enc",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
            uuid::Uuid::new_v4()
        ));
        atomic_write(&path, bytes)?;
        Ok(path)
    }

    fn check_remote(&self, scope: &str, open: &OpenVault) -> Result<()> {
        if let Some(cursor) = self.cursor(scope)? {
            ensure!(
                cursor.vault_id == open.id() && open.vault.revision >= cursor.revision,
                t!("sync.error.rollback")
            );
        }
        Ok(())
    }

    pub fn save_credentials(
        settings: &CloudSyncSettings,
        secret_key: &str,
        session_token: &str,
        secrets: &SharedSecretStore,
    ) -> Result<()> {
        ensure!(secrets.is_available(), t!("sync.error.no_keychain"));
        if !secret_key.is_empty() {
            secrets.set(&settings.secret("secret-access-key"), secret_key)?;
        }
        if !session_token.is_empty() {
            secrets.set(&settings.secret("session-token"), session_token)?;
        }
        ensure!(
            secrets
                .get(&settings.secret("secret-access-key"))?
                .is_some_and(|key| !key.is_empty()),
            t!("sync.error.access_key")
        );
        if !settings.automatic {
            secrets.delete(&settings.secret("device-key"))?;
        }
        Ok(())
    }

    pub fn upload(
        &self,
        settings: &CloudSyncSettings,
        inventory: &HostInventory,
        secrets: SharedSecretStore,
        password: &str,
        recovery: &str,
    ) -> Result<Upload> {
        ensure!(secrets.is_available(), t!("sync.error.no_keychain"));
        let mut vault = Vault::capture(inventory, secrets.as_ref())?;
        let storage = self.storage(settings, &secrets)?;
        let scope = settings.scope();
        let remote = storage.get()?;
        let (bytes, recovery_key, expected, opened) = match remote {
            Some(remote) => {
                let opened = Self::unlock(settings, &remote.bytes, &secrets, password, recovery)?;
                self.check_remote(&scope, &opened)?;
                let cursor = self.cursor(&scope)?;
                if !cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.etag == remote.etag)
                {
                    self.backup(&remote.bytes, "cloud-conflict")?;
                    let local = opened.seal(&vault)?;
                    let backup = self.backup(&local, "local-conflict")?;
                    anyhow::bail!(t!("sync.error.conflict_backup", path = backup.display()));
                }
                vault.revision = opened
                    .vault
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!(t!("sync.error.format")))?;
                let bytes = opened.seal(&vault)?;
                (
                    bytes,
                    None,
                    Some(ObjectVersion {
                        etag: remote.etag,
                        version: remote.version,
                    }),
                    opened,
                )
            }
            None => {
                ensure!(
                    self.cursor(&scope)?.is_none(),
                    t!("sync.error.remote_deleted")
                );
                let (bytes, recovery) = crypto::create(&vault, password)?;
                let opened = crypto::open(&bytes, password, "")?;
                // 创建请求的响应可能丢失，先保存恢复密钥，保证仍可找回。
                secrets.set(&settings.secret("recovery-key"), &recovery)?;
                (bytes, Some(recovery), None, opened)
            }
        };
        let digest = opened.content_digest(&vault)?;
        let backup = self.backup(&bytes, "upload")?;
        let written = storage.put(bytes, expected.as_ref()).map_err(|error| {
            anyhow::anyhow!(t!(
                "sync.error.upload_backup",
                error = error,
                path = backup.display()
            ))
        })?;
        Self::remember_key(settings, &opened, &secrets)?;
        atomic_write(
            &self.cursor_path(&scope)?,
            &serde_json::to_vec(&Cursor {
                etag: written.etag,
                vault_id: opened.id().to_string(),
                revision: vault.revision,
                digest: Some(digest),
            })?,
        )?;
        Ok(Upload {
            backup,
            recovery: recovery_key,
        })
    }

    pub fn download(
        &self,
        settings: &CloudSyncSettings,
        local: &HostInventory,
        secrets: SharedSecretStore,
        password: &str,
        recovery: &str,
    ) -> Result<Download> {
        let storage = self.storage(settings, &secrets)?;
        let remote = storage
            .get()?
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.empty")))?;
        let open = crypto::open(&remote.bytes, password, recovery)?;
        let scope = settings.scope();
        self.check_remote(&scope, &open)?;
        self.prepare_download(
            open,
            Some(ObjectVersion {
                etag: remote.etag,
                version: remote.version,
            }),
            scope,
            local,
            secrets,
        )
        .map(|mut download| {
            download.automatic = settings.automatic;
            download
        })
    }

    /// 用户在预览中选择保留本地后，仍然按预览的远端版本执行条件上传。
    pub fn keep_local(
        &self,
        settings: &CloudSyncSettings,
        download: Download,
        local: &HostInventory,
        secrets: SharedSecretStore,
    ) -> Result<Upload> {
        ensure!(
            settings.scope() == download.scope && download.stamp == inventory_stamp(local),
            t!("sync.error.local_changed")
        );
        let mut current = Vault::capture(local, secrets.as_ref())?;
        ensure!(
            download.digest == vault_digest(&current)?,
            t!("sync.error.local_changed")
        );
        let expected = download
            .version
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.no_preview")))?;
        self.backup(
            &download.open.seal(&download.open.vault)?,
            "before-cloud-replace",
        )?;
        current.revision = download
            .open
            .vault
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.format")))?;
        let bytes = download.open.seal(&current)?;
        let backup = self.backup(&bytes, "keep-local")?;
        let digest = download.open.content_digest(&current)?;
        let written = self
            .storage(settings, &secrets)?
            .put(bytes, Some(&expected))
            .map_err(|error| {
                anyhow::anyhow!(t!(
                    "sync.error.upload_backup",
                    error = error,
                    path = backup.display()
                ))
            })?;
        Self::remember_key(settings, &download.open, &secrets)?;
        atomic_write(
            &self.cursor_path(&download.scope)?,
            &serde_json::to_vec(&Cursor {
                etag: written.etag,
                vault_id: download.open.id().to_string(),
                revision: current.revision,
                digest: Some(digest),
            })?,
        )?;
        Ok(Upload {
            backup,
            recovery: None,
        })
    }

    pub fn import_backup(
        &self,
        path: &Path,
        local: &HostInventory,
        secrets: SharedSecretStore,
        password: &str,
        recovery: &str,
    ) -> Result<Download> {
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(MAX_ENCRYPTED_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        let open = crypto::open(&bytes, password, recovery)?;
        self.prepare_download(open, None, String::new(), local, secrets)
    }

    fn prepare_download(
        &self,
        open: OpenVault,
        version: Option<ObjectVersion>,
        scope: String,
        local: &HostInventory,
        secrets: SharedSecretStore,
    ) -> Result<Download> {
        ensure!(secrets.is_available(), t!("sync.error.no_keychain"));
        let current = Vault::capture(local, secrets.as_ref())?;
        let backup = self.backup(&open.seal(&current)?, "before-download")?;
        Ok(Download {
            automatic: false,
            stamp: inventory_stamp(local),
            digest: vault_digest(&current)?,
            open,
            version,
            scope,
            backup,
        })
    }

    pub fn stage_import(
        &self,
        download: Download,
        local: &HostInventory,
        key_dir: Option<&Path>,
        secrets: SharedSecretStore,
    ) -> Result<ImportStage> {
        ensure!(
            download.stamp == inventory_stamp(local),
            t!("sync.error.local_changed")
        );
        let current = Vault::capture(local, secrets.as_ref())?;
        ensure!(
            download.digest == vault_digest(&current)?,
            t!("sync.error.local_changed")
        );
        let current_paths = local
            .credentials
            .iter()
            .filter_map(|credential| {
                credential
                    .key_path
                    .as_ref()
                    .map(|path| (credential.keychain_id.to_string(), path.to_string()))
            })
            .collect();
        let expected_values: std::collections::HashMap<_, _> = current
            .secret_changes(&current_paths, true)?
            .into_iter()
            .collect();
        let cursor_path = if download.version.is_some() {
            Some(self.cursor_path(&download.scope)?)
        } else {
            None
        };
        let digest = download.open.content_digest(&download.open.vault)?;
        let device_key = if download.automatic && download.version.is_some() {
            Some((
                SecretRef::cloud_sync(download.scope.clone(), "device-key"),
                download.open.device_key()?,
            ))
        } else {
            None
        };
        let cursor = download.version.map(|version| Cursor {
            etag: version.etag,
            vault_id: download.open.id().to_string(),
            revision: download.open.vault.revision,
            digest: Some(digest),
        });
        let mut stage = ImportStage {
            inventory: HostInventory::new(Vec::new(), Vec::new(), Vec::new()),
            undo: Vec::new(),
            files: Vec::new(),
            secrets,
            cursor,
            cursor_path,
            committed: false,
            device_key,
        };
        let outcome = (|| -> Result<()> {
            let vault = &download.open.vault;
            let paths = if vault
                .credentials
                .iter()
                .any(|credential| credential.private_key.is_some())
            {
                vault.write_keys(
                    key_dir.ok_or_else(|| anyhow::anyhow!(t!("sync.error.key_directory")))?,
                    &mut stage.files,
                )?
            } else {
                Default::default()
            };
            let changes = vault.secret_changes(&paths, true)?;
            // 先读取全部旧值，再修改第一条秘密，读取失败不会留下半次导入。
            for (secret, value) in changes {
                let before = stage.secrets.get(&secret)?;
                if let Some(expected) = expected_values.get(&secret) {
                    ensure!(&before == expected, t!("sync.error.local_changed"));
                }
                stage.undo.push(SecretUndo {
                    before,
                    secret,
                    after: value,
                });
            }
            for change in &stage.undo {
                ensure!(
                    stage.secrets.compare_and_set(
                        &change.secret,
                        change.before.as_deref().map(String::as_str),
                        change.after.as_deref().map(String::as_str)
                    )?,
                    t!("sync.error.local_changed")
                );
            }
            stage.inventory = vault.inventory(local, &paths);
            Ok(())
        })();
        if let Err(error) = outcome {
            let rollback = stage.rollback();
            return match rollback {
                Ok(()) => Err(error),
                Err(_) => Err(anyhow::anyhow!(t!(
                    "sync.error.rollback_failed",
                    path = download.backup.display()
                ))),
            };
        }
        Ok(stage)
    }
}

impl Download {
    pub fn counts(&self) -> (usize, usize, usize) {
        (
            self.open.vault.hosts.len(),
            self.open.vault.groups.len(),
            self.open.vault.credentials.len(),
        )
    }
    pub fn is_cloud(&self) -> bool {
        self.version.is_some()
    }
}

fn undo_changes(secrets: &SharedSecretStore, undo: &[SecretUndo], files: &[PathBuf]) -> Result<()> {
    let mut failed = false;
    for change in undo {
        let before = change.before.as_deref().map(String::as_str);
        let after = change.after.as_deref().map(String::as_str);
        let result = secrets.get(&change.secret).and_then(|current| {
            if current.as_deref().map(String::as_str) == before {
                Ok(true)
            } else {
                secrets.compare_and_set(&change.secret, after, before)
            }
        });
        failed |= !matches!(result, Ok(true));
    }
    for path in files {
        failed |= fs::remove_file(path).is_err();
    }
    ensure!(!failed, t!("sync.error.compensation"));
    Ok(())
}

impl ImportStage {
    /// 数据库成功后才推进游标；游标保存失败时仍保留已经导入的数据。
    pub fn commit(mut self) -> Result<()> {
        self.committed = true;
        if let Some((secret, key)) = &self.device_key {
            self.secrets.set(secret, key)?;
        }
        if let (Some(cursor), Some(path)) = (&self.cursor, &self.cursor_path) {
            atomic_write(path, &serde_json::to_vec(cursor)?)?;
        }
        Ok(())
    }

    pub fn rollback(mut self) -> Result<()> {
        self.committed = true;
        undo_changes(&self.secrets, &self.undo, &self.files)
    }
}

impl Drop for ImportStage {
    fn drop(&mut self) {
        if !self.committed {
            let secrets = self.secrets.clone();
            let undo = std::mem::take(&mut self.undo);
            let files = std::mem::take(&mut self.files);
            let _ = std::thread::Builder::new()
                .name("cloud-sync-rollback".into())
                .spawn(move || {
                    let _ = undo_changes(&secrets, &undo, &files);
                });
        }
    }
}
