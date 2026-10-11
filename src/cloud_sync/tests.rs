use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use zeroize::Zeroizing;

use super::{
    CloudSyncSettings, crypto,
    service::SyncServices,
    storage::{ObjectVersion, RemoteObject, S3Storage, VaultStorage},
    vault::Vault,
};
use crate::{
    host::{
        AuthKind, CredentialDraft, CredentialKind, GeneratedKey, GroupDraft, HostDatabase,
        HostDraft, HostInventory, HostStore, KeyAlgorithm, ProxyKind, ProxySettings, Route,
    },
    secrets::{
        InMemorySecretStore, SecretRef, SecretStore, SharedSecretStore, TemporarySecretStore,
    },
};

pub(crate) const PASSWORD: &str = "测试保险库主密码-12345";

#[derive(Default)]
pub(crate) struct MemoryStorage {
    contents: Mutex<(Option<Vec<u8>>, u64)>,
    pub(crate) force_conflict: std::sync::atomic::AtomicBool,
}

impl MemoryStorage {
    pub(crate) fn service(self: &Arc<Self>, directory: &std::path::Path) -> SyncServices {
        SyncServices::with_storage(directory.into(), self.clone())
    }
}

impl VaultStorage for MemoryStorage {
    fn get(&self) -> Result<Option<RemoteObject>> {
        let stored = self.contents.lock().unwrap();
        Ok(stored.0.as_ref().map(|bytes| RemoteObject {
            bytes: bytes.clone(),
            etag: format!("\"{}\"", stored.1),
            version: None,
        }))
    }
    fn put(&self, bytes: Vec<u8>, expected: Option<&ObjectVersion>) -> Result<ObjectVersion> {
        let mut stored = self.contents.lock().unwrap();
        anyhow::ensure!(!self.force_conflict.load(Ordering::SeqCst), "测试并发冲突");
        match expected {
            Some(expected) => anyhow::ensure!(
                expected.etag == format!("\"{}\"", stored.1) && stored.0.is_some(),
                "测试版本冲突"
            ),
            None => anyhow::ensure!(stored.0.is_none(), "测试对象已存在"),
        }
        stored.1 += 1;
        stored.0 = Some(bytes);
        Ok(ObjectVersion {
            etag: format!("\"{}\"", stored.1),
            version: None,
        })
    }
}

pub(crate) fn settings() -> CloudSyncSettings {
    CloudSyncSettings {
        bucket: "test-bucket".into(),
        access_key_id: "test-access".into(),
        ..Default::default()
    }
}

pub(crate) fn test_open_cloud(storage: &MemoryStorage) -> HostInventory {
    crypto::open(&storage.get().unwrap().unwrap().bytes, PASSWORD, "")
        .unwrap()
        .vault
        .inventory(&HostStore::empty().inventory(), &Default::default())
}

fn inventory() -> HostInventory {
    let mut store = HostStore::empty();
    let root = store.insert_group_unnotified(GroupDraft::new("生产", None));
    let child = store.insert_group_unnotified(GroupDraft::new("数据库", Some(root)));
    let jump = store.insert_unnotified(HostDraft::new(
        "跳板",
        "jump.example",
        22,
        "root",
        AuthKind::Password,
        Some(root),
    ));
    let credential = store.insert_credential_unnotified(CredentialDraft::new(
        "共享凭据",
        CredentialKind::Password,
        "admin",
    ));
    let host = store.insert_unnotified(
        HostDraft::new(
            "数据库",
            "db.example",
            2222,
            "admin",
            AuthKind::Password,
            Some(child),
        )
        .with_credential(credential),
    );
    let mut draft = store.host(host).unwrap().draft();
    draft.route = Route::Jump(vec![Some(jump), None]);
    store.update_unnotified(host, draft);
    let proxy_host = store.insert_unnotified(HostDraft::new(
        "代理主机",
        "proxy-target.example",
        22,
        "ops",
        AuthKind::Password,
        None,
    ));
    let mut draft = store.host(proxy_host).unwrap().draft();
    draft.route = Route::Proxy(
        ProxySettings::new(ProxyKind::Socks5, "proxy.example", 1080).with_user("proxy-user"),
    );
    store.update_unnotified(proxy_host, draft);
    store.inventory()
}

fn secrets(inventory: &HostInventory) -> Arc<InMemorySecretStore> {
    let secrets = Arc::new(InMemorySecretStore::default());
    for host in &inventory.hosts {
        secrets
            .set(&host.password_secret(), "主机秘密-不要泄露")
            .unwrap();
    }
    for credential in &inventory.credentials {
        if credential.kind == CredentialKind::Password {
            secrets
                .set(&credential.password_secret(), "凭据秘密-不要泄露")
                .unwrap();
        }
    }
    secrets
        .set(
            &SecretRef::proxy("proxy-user", "proxy.example", 1080),
            "代理秘密-不要泄露",
        )
        .unwrap();
    secrets
}

#[test]
fn automatic_sync_uploads_edits_and_credentials_without_saving_the_master_password() {
    use super::service::AutomaticSync;
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(MemoryStorage::default());
    let service = SyncServices::with_storage(dir.path().into(), storage.clone());
    let mut local = inventory();
    let secrets = secrets(&local);
    let settings = settings();
    service
        .upload(&settings, &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    let saved = secrets
        .get(&settings.secret("device-key"))
        .unwrap()
        .unwrap();
    assert!(!saved.contains(PASSWORD));
    assert!(matches!(
        service
            .automatic(&settings, &local, secrets.clone())
            .unwrap(),
        AutomaticSync::Unchanged
    ));
    let version = storage.get().unwrap().unwrap().etag;
    local.hosts[0].name = "已修改主机".into();
    secrets
        .set(&local.credentials[0].password_secret(), "新凭据密码")
        .unwrap();
    assert!(matches!(
        service
            .automatic(&settings, &local, secrets.clone())
            .unwrap(),
        AutomaticSync::Uploaded
    ));
    let remote = storage.get().unwrap().unwrap();
    assert_ne!(version, remote.etag);
    let opened = crypto::open(&remote.bytes, PASSWORD, "").unwrap();
    assert_eq!(
        opened.content_digest(&opened.vault).unwrap(),
        opened
            .content_digest(&Vault::capture(&local, secrets.as_ref()).unwrap())
            .unwrap()
    );
    assert!(matches!(
        service
            .automatic(&settings, &local, secrets.clone())
            .unwrap(),
        AutomaticSync::Unchanged
    ));
    local.hosts.remove(0);
    local
        .hosts
        .iter_mut()
        .for_each(|host| host.route = Route::Direct);
    assert!(matches!(
        service.automatic(&settings, &local, secrets).unwrap(),
        AutomaticSync::Uploaded
    ));
}

#[test]
fn automatic_sync_pulls_remote_changes_and_preserves_concurrent_edits() {
    use super::service::AutomaticSync;
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(MemoryStorage::default());
    let a = SyncServices::with_storage(a_dir.path().into(), storage.clone());
    let b = SyncServices::with_storage(b_dir.path().into(), storage.clone());
    let settings = settings();
    let mut a_local = inventory();
    let a_secrets = secrets(&a_local);
    a.upload(&settings, &a_local, a_secrets.clone(), PASSWORD, "")
        .unwrap();
    let b_empty = HostStore::empty().inventory();
    let b_secrets = Arc::new(InMemorySecretStore::default());
    let download = b
        .download(&settings, &b_empty, b_secrets.clone(), PASSWORD, "")
        .unwrap();
    let stage = b
        .stage_import(download, &b_empty, None, b_secrets.clone())
        .unwrap();
    let mut b_local = stage.inventory.clone();
    stage.commit().unwrap();
    a_local.groups[0].name = "云端分组".into();
    a.automatic(&settings, &a_local, a_secrets.clone()).unwrap();
    let download = match b.automatic(&settings, &b_local, b_secrets.clone()).unwrap() {
        AutomaticSync::Download(download) => download,
        _ => panic!("必须下载云端修改"),
    };
    let stage = b
        .stage_import(*download, &b_local, None, b_secrets.clone())
        .unwrap();
    b_local = stage.inventory.clone();
    stage.commit().unwrap();
    assert!(b_local.groups.iter().any(|group| group.name == "云端分组"));
    assert!(matches!(
        b.automatic(&settings, &b_local, b_secrets.clone()).unwrap(),
        AutomaticSync::Unchanged
    ));
    a_local.hosts[0].notes = "设备甲修改".into();
    b_local.hosts[0].notes = "设备乙修改".into();
    a.automatic(&settings, &a_local, a_secrets).unwrap();
    let remote_before = storage.get().unwrap().unwrap().bytes;
    let error = b.automatic(&settings, &b_local, b_secrets).err().unwrap();
    assert!(error.to_string().contains("冲突"));
    assert_eq!(remote_before, storage.get().unwrap().unwrap().bytes);
    assert!(
        std::fs::read_dir(b_dir.path().join("backups"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .contains("local-conflict"))
    );
}

#[test]
fn automatic_sync_does_not_advance_the_baseline_on_a_failed_upload() {
    use super::service::AutomaticSync;
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(MemoryStorage::default());
    let service = SyncServices::with_storage(dir.path().into(), storage.clone());
    let settings = settings();
    let mut local = inventory();
    let secrets = secrets(&local);
    service
        .upload(&settings, &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    let path = dir.path().join(format!("cursor-{}.json", settings.scope()));
    let before = std::fs::read(&path).unwrap();
    local.hosts[0].notes = "等待重试".into();
    storage.force_conflict.store(true, Ordering::SeqCst);
    assert!(
        service
            .automatic(&settings, &local, secrets.clone())
            .is_err()
    );
    assert_eq!(before, std::fs::read(&path).unwrap());
    storage.force_conflict.store(false, Ordering::SeqCst);
    assert!(matches!(
        service
            .automatic(&settings, &local, secrets.clone())
            .unwrap(),
        AutomaticSync::Uploaded
    ));
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("digest");
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(
        service
            .automatic(&settings, &local, secrets)
            .err()
            .unwrap()
            .to_string()
            .contains("尚未解锁")
    );
}

#[test]
fn automatic_preference_keeps_the_existing_profile_and_can_forget_the_device_key() {
    let mut settings = settings();
    let encoded = r#"{"endpoint":"","region":"us-east-1","bucket":"test-bucket","object_key":"shellrs/vault.enc","access_key_id":"test-access","virtual_hosted":false}"#;
    use sha2::Digest;
    assert_eq!(
        settings.scope(),
        format!("{:x}", sha2::Sha256::digest(encoded.as_bytes()))
    );
    let original = settings.scope();
    let secrets: SharedSecretStore = Arc::new(InMemorySecretStore::default());
    secrets
        .set(&settings.secret("device-key"), "已保存的数据密钥")
        .unwrap();
    secrets
        .set(&settings.secret("secret-access-key"), "访问密钥")
        .unwrap();
    settings.automatic = false;
    assert_eq!(original, settings.scope());
    SyncServices::save_credentials(&settings, "", "", &secrets).unwrap();
    assert!(
        secrets
            .get(&settings.secret("device-key"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn encryption_authenticates_contents_password_and_recovery() {
    let inventory = inventory();
    let secrets = secrets(&inventory);
    let vault = Vault::capture(&inventory, secrets.as_ref()).unwrap();
    let (bytes, recovery) = crypto::create(&vault, PASSWORD).unwrap();
    let cloud_text = String::from_utf8(bytes.clone()).unwrap();
    for text in [
        "db.example",
        "生产",
        "主机秘密",
        "凭据秘密",
        "代理秘密",
        PASSWORD,
        recovery.as_str(),
    ] {
        assert!(!cloud_text.contains(text));
    }
    assert_eq!(
        crypto::open(&bytes, PASSWORD, "")
            .unwrap()
            .vault
            .hosts
            .len(),
        3
    );
    assert_eq!(
        crypto::open(&bytes, "", &recovery)
            .unwrap()
            .vault
            .credentials
            .len(),
        1
    );
    assert!(crypto::open(&bytes, "错误密码", "").is_err());
    let mut damaged_recovery: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    damaged_recovery["recovery_wrapped_key"] = damaged_recovery["wrapped_key"].clone();
    assert!(
        crypto::open(
            &serde_json::to_vec(&damaged_recovery).unwrap(),
            PASSWORD,
            ""
        )
        .is_err()
    );
    let mut corrupted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let ciphertext = corrupted["ciphertext"].as_str().unwrap();
    let mut replacement = ciphertext.as_bytes().to_vec();
    replacement[0] = if replacement[0] == b'A' { b'B' } else { b'A' };
    corrupted["ciphertext"] = String::from_utf8(replacement).unwrap().into();
    assert!(crypto::open(&serde_json::to_vec(&corrupted).unwrap(), PASSWORD, "").is_err());
    corrupted["version"] = 999.into();
    assert!(crypto::open(&serde_json::to_vec(&corrupted).unwrap(), PASSWORD, "").is_err());
    assert!(crypto::create(&vault, "短密码").is_err());
}

#[test]
fn vault_rejects_duplicate_ids_cycles_and_missing_references() {
    let inventory = inventory();
    let vault = Vault::capture(&inventory, secrets(&inventory).as_ref()).unwrap();
    let value = serde_json::to_value(&vault).unwrap();
    let mut cycle = value.clone();
    cycle["groups"][0]["parent"] = cycle["groups"][1]["id"].clone();
    assert!(
        serde_json::from_value::<Vault>(cycle)
            .unwrap()
            .validate()
            .is_err()
    );
    let mut duplicate = value.clone();
    duplicate["hosts"][1]["id"] = duplicate["hosts"][0]["id"].clone();
    assert!(
        serde_json::from_value::<Vault>(duplicate)
            .unwrap()
            .validate()
            .is_err()
    );
    let mut missing = value;
    missing["hosts"][0]["group"] = "missing-group-id".into();
    assert!(
        serde_json::from_value::<Vault>(missing)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn s3_configuration_excludes_secrets_and_rejects_insecure_endpoints() {
    let mut settings = settings();
    settings.endpoint = "http://example.test".into();
    assert!(settings.clone().validated().is_err());
    settings.endpoint = "https://user:secret@example.test".into();
    assert!(settings.clone().validated().is_err());
    settings.endpoint = "https://example.test".into();
    settings.object_key = "../vault".into();
    assert!(settings.clone().validated().is_err());
    settings.object_key = "shellrs/vault.enc".into();
    assert!(settings.clone().validated().is_ok());
    let json = serde_json::to_string(&settings).unwrap();
    assert!(
        !json.contains("secret_access_key")
            && !json.contains("password")
            && !json.contains("session_token")
    );
    let profile = settings.scope();
    settings.bucket = "another-bucket".into();
    assert_ne!(profile, settings.scope());
}

#[test]
fn preview_stamp_ignores_connection_state_and_local_tree_presentation() {
    let mut inventory = inventory();
    let before = super::service::inventory_stamp(&inventory);
    inventory.hosts[0].state = crate::host::ConnectionState::Connected;
    inventory.hosts[0].os = Some(crate::host::HostOs::Windows);
    inventory.groups[0].expanded = false;
    assert_eq!(before, super::service::inventory_stamp(&inventory));
    inventory.secret_revision += 1;
    assert_ne!(before, super::service::inventory_stamp(&inventory));
}

#[test]
fn concurrent_cloud_changes_are_backed_up_and_never_silently_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(MemoryStorage::default());
    let services = SyncServices::with_storage(directory.path().into(), storage.clone());
    let inventory = inventory();
    let secrets = secrets(&inventory);
    let created = services
        .upload(&settings(), &inventory, secrets.clone(), PASSWORD, "")
        .unwrap();
    assert!(created.recovery.is_some() && created.backup.exists());
    services
        .upload(&settings(), &inventory, secrets.clone(), PASSWORD, "")
        .unwrap();
    storage.contents.lock().unwrap().1 += 1;
    let before = storage.get().unwrap().unwrap().bytes;
    assert!(
        services
            .upload(&settings(), &inventory, secrets.clone(), PASSWORD, "")
            .is_err()
    );
    assert_eq!(storage.get().unwrap().unwrap().bytes, before);
    assert!(
        std::fs::read_dir(directory.path().join("backups"))
            .unwrap()
            .count()
            >= 4
    );
    let preview = services
        .download(&settings(), &inventory, secrets.clone(), PASSWORD, "")
        .unwrap();
    services
        .keep_local(&settings(), preview, &inventory, secrets.clone())
        .unwrap();
    let preview = services
        .download(&settings(), &inventory, secrets.clone(), PASSWORD, "")
        .unwrap();
    storage.force_conflict.store(true, Ordering::SeqCst);
    let before = storage.get().unwrap().unwrap().bytes;
    assert!(
        services
            .keep_local(&settings(), preview, &inventory, secrets)
            .is_err()
    );
    assert_eq!(storage.get().unwrap().unwrap().bytes, before);
}

#[test]
fn a_new_device_must_review_an_existing_cloud_vault() {
    let storage = Arc::new(MemoryStorage::default());
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let first = SyncServices::with_storage(first_dir.path().into(), storage.clone());
    let second = SyncServices::with_storage(second_dir.path().into(), storage.clone());
    let inventory = inventory();
    let secrets = secrets(&inventory);
    first
        .upload(&settings(), &inventory, secrets.clone(), PASSWORD, "")
        .unwrap();
    assert!(
        second
            .upload(
                &settings(),
                &HostStore::empty().inventory(),
                secrets,
                PASSWORD,
                ""
            )
            .is_err()
    );
    assert_eq!(
        crypto::open(&storage.get().unwrap().unwrap().bytes, PASSWORD, "")
            .unwrap()
            .vault
            .hosts
            .len(),
        3
    );
}

#[test]
fn download_maps_ids_and_materializes_keys_and_keychain_values() {
    let source_dir = tempfile::tempdir().unwrap();
    let destination_dir = tempfile::tempdir().unwrap();
    let mut source = inventory();
    let text = GeneratedKey::generate(KeyAlgorithm::Ed25519)
        .unwrap()
        .encode("同步测试", "私钥口令")
        .unwrap();
    let path = crate::host::write_key_file(source_dir.path(), None, &text).unwrap();
    let key = crate::host::Credential::new(
        crate::host::CredentialId(99),
        CredentialDraft::new("私钥", CredentialKind::Key, "deploy")
            .with_key_path(path.display().to_string()),
    );
    source.credentials.push(key.clone());
    let source_secrets = secrets(&source);
    source_secrets
        .set(&SecretRef::passphrase(&path), "私钥口令")
        .unwrap();
    let storage = Arc::new(MemoryStorage::default());
    let first = SyncServices::with_storage(source_dir.path().join("sync"), storage.clone());
    let second = SyncServices::with_storage(destination_dir.path().join("sync"), storage);
    first
        .upload(&settings(), &source, source_secrets, PASSWORD, "")
        .unwrap();
    let mut local = source.clone();
    for group in &mut local.groups {
        group.id.0 += 100;
        group.parent = group.parent.map(|mut id| {
            id.0 += 100;
            id
        });
    }
    for credential in &mut local.credentials {
        credential.id.0 += 200;
    }
    for host in &mut local.hosts {
        host.id.0 += 300;
        host.group = host.group.map(|mut id| {
            id.0 += 100;
            id
        });
        host.credential = host.credential.map(|mut id| {
            id.0 += 200;
            id
        });
    }
    local.next_ids = [1000, 1000, 1000];
    let target_secrets: SharedSecretStore = Arc::new(InMemorySecretStore::default());
    let preview = second
        .download(&settings(), &local, target_secrets.clone(), PASSWORD, "")
        .unwrap();
    let stage = second
        .stage_import(
            preview,
            &local,
            Some(&destination_dir.path().join("keys")),
            target_secrets.clone(),
        )
        .unwrap();
    for host in &stage.inventory.hosts {
        assert!(host.id.0 >= 300 && host.id.0 < 1000);
    }
    assert!(
        matches!(&stage.inventory.hosts[1].route, Route::Jump(hops) if hops[0] == Some(stage.inventory.hosts[0].id) && hops[1].is_none())
    );
    let restored = stage
        .inventory
        .credentials
        .iter()
        .find(|credential| credential.kind == CredentialKind::Key)
        .unwrap();
    let restored_path = restored.key_path.as_deref().unwrap();
    assert_ne!(restored_path, path.display().to_string());
    assert_eq!(std::fs::read_to_string(restored_path).unwrap(), *text);
    assert_eq!(
        target_secrets
            .get(&SecretRef::passphrase(restored_path))
            .unwrap()
            .unwrap()
            .as_str(),
        "私钥口令"
    );
    let database = HostDatabase::in_memory().unwrap();
    database.replace_inventory(&stage.inventory).unwrap();
    stage.commit().unwrap();
    let loaded = HostStore::load(database).unwrap();
    assert_eq!(loaded.hosts().len(), 3);
    assert_eq!(loaded.groups()[0].public_id, source.groups[0].public_id);
    assert_eq!(loaded.credentials().len(), 2);
}

struct FailOnceStore {
    inner: Arc<InMemorySecretStore>,
    writes: AtomicUsize,
}
impl SecretStore for FailOnceStore {
    fn get(&self, secret: &SecretRef) -> Result<Option<Zeroizing<String>>> {
        self.inner.get(secret)
    }
    fn set(&self, secret: &SecretRef, value: &str) -> Result<()> {
        if self.writes.fetch_add(1, Ordering::SeqCst) == 1 {
            anyhow::bail!("模拟钥匙串写入失败");
        }
        self.inner.set(secret, value)
    }
    fn delete(&self, secret: &SecretRef) -> Result<()> {
        self.inner.delete(secret)
    }
    fn is_available(&self) -> bool {
        true
    }
}

#[test]
fn failed_keychain_import_restores_preexisting_values() {
    let directory = tempfile::tempdir().unwrap();
    let inventory = inventory();
    let source = secrets(&inventory);
    let storage = Arc::new(MemoryStorage::default());
    let services = SyncServices::with_storage(directory.path().into(), storage);
    services
        .upload(&settings(), &inventory, source, PASSWORD, "")
        .unwrap();
    let old = Arc::new(InMemorySecretStore::default());
    for host in &inventory.hosts {
        old.set(&host.password_secret(), "旧密码").unwrap();
    }
    for credential in &inventory.credentials {
        old.set(&credential.password_secret(), "旧密码").unwrap();
    }
    old.set(
        &SecretRef::proxy("proxy-user", "proxy.example", 1080),
        "旧密码",
    )
    .unwrap();
    let failing: SharedSecretStore = Arc::new(TemporarySecretStore::new(Arc::new(FailOnceStore {
        inner: old.clone(),
        writes: AtomicUsize::new(0),
    })));
    let preview = services
        .download(&settings(), &inventory, failing.clone(), PASSWORD, "")
        .unwrap();
    assert!(
        services
            .stage_import(preview, &inventory, None, failing)
            .is_err()
    );
    for host in &inventory.hosts {
        assert_eq!(
            old.get(&host.password_secret()).unwrap().unwrap().as_str(),
            "旧密码"
        );
    }
    for credential in &inventory.credentials {
        assert_eq!(
            old.get(&credential.password_secret())
                .unwrap()
                .unwrap()
                .as_str(),
            "旧密码"
        );
    }
    assert_eq!(
        old.get(&SecretRef::proxy("proxy-user", "proxy.example", 1080))
            .unwrap()
            .unwrap()
            .as_str(),
        "旧密码"
    );
}

#[test]
fn stale_previews_and_database_failures_do_not_lose_local_data() {
    let directory = tempfile::tempdir().unwrap();
    let local = inventory();
    let secrets = secrets(&local);
    let services =
        SyncServices::with_storage(directory.path().into(), Arc::new(MemoryStorage::default()));
    services
        .upload(&settings(), &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    let preview = services
        .download(&settings(), &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    secrets
        .set(&local.hosts[0].password_secret(), "预览之后修改的密码")
        .unwrap();
    assert!(
        services
            .stage_import(preview, &local, None, secrets.clone())
            .is_err()
    );
    let preview = services
        .download(&settings(), &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    let stage = services
        .stage_import(preview, &local, None, secrets.clone())
        .unwrap();
    let database = HostDatabase::in_memory().unwrap();
    database.replace_inventory(&local).unwrap();
    let mut invalid = stage.inventory.clone();
    invalid.hosts[0].port = 0;
    assert!(database.replace_inventory(&invalid).is_err());
    assert_eq!(database.load().unwrap().hosts[0].port, local.hosts[0].port);
    stage.rollback().unwrap();
    assert_eq!(
        secrets
            .get(&local.hosts[0].password_secret())
            .unwrap()
            .unwrap()
            .as_str(),
        "预览之后修改的密码"
    );
}

#[test]
fn rollback_preserves_a_password_changed_after_staging() {
    let directory = tempfile::tempdir().unwrap();
    let local = inventory();
    let secrets = secrets(&local);
    let services =
        SyncServices::with_storage(directory.path().into(), Arc::new(MemoryStorage::default()));
    services
        .upload(&settings(), &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    secrets
        .set(&local.hosts[0].password_secret(), "导入前密码")
        .unwrap();
    let preview = services
        .download(&settings(), &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    let stage = services
        .stage_import(preview, &local, None, secrets.clone())
        .unwrap();
    secrets
        .set(&local.hosts[0].password_secret(), "导入期间用户的新密码")
        .unwrap();
    assert!(stage.rollback().is_err());
    assert_eq!(
        secrets
            .get(&local.hosts[0].password_secret())
            .unwrap()
            .unwrap()
            .as_str(),
        "导入期间用户的新密码"
    );
}

#[test]
fn deleting_or_rolling_back_cloud_history_stops_upload() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(MemoryStorage::default());
    let services = SyncServices::with_storage(directory.path().into(), storage.clone());
    let local = inventory();
    let secrets = secrets(&local);
    services
        .upload(&settings(), &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    let old = storage.get().unwrap().unwrap().bytes;
    services
        .upload(&settings(), &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    storage.contents.lock().unwrap().0 = Some(old);
    assert!(
        services
            .download(&settings(), &local, secrets.clone(), PASSWORD, "")
            .is_err()
    );
    storage.contents.lock().unwrap().0 = None;
    assert!(
        services
            .upload(&settings(), &local, secrets, PASSWORD, "")
            .is_err()
    );
}

#[test]
fn encrypted_backups_can_be_recovered_without_cloud_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let local = inventory();
    let secrets = secrets(&local);
    let services =
        SyncServices::with_storage(directory.path().into(), Arc::new(MemoryStorage::default()));
    let uploaded = services
        .upload(&settings(), &local, secrets.clone(), PASSWORD, "")
        .unwrap();
    let preview = services
        .import_backup(
            &uploaded.backup,
            &local,
            secrets,
            "",
            uploaded.recovery.as_ref().unwrap(),
        )
        .unwrap();
    assert!(!preview.is_cloud());
    assert_eq!(preview.counts(), (3, 2, 1));
}

#[test]
fn s3_adapter_signs_requests_and_sends_conditional_put_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut stored = Vec::new();
        for step in 0..5 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8(request[..header_end].to_vec())
                .unwrap()
                .to_lowercase();
            assert!(headers.contains("/test-bucket/shellrs/vault.enc"));
            assert!(headers.contains("authorization: aws4-hmac-sha256"));
            assert!(headers.contains("x-amz-content-sha256:"));
            let length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while request.len() < header_end + length {
                let mut chunk = [0; 4096];
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
            }
            let (status, body, etag) = match step {
                0 => ("404 Not Found", Vec::new(), ""),
                1 => {
                    assert!(headers.contains("if-none-match: *"));
                    stored = request[header_end..].to_vec();
                    ("200 OK", Vec::new(), "ETag: \"one\"\r\n")
                }
                2 => ("200 OK", stored.clone(), "ETag: \"one\"\r\n"),
                3 => {
                    assert!(headers.contains("if-match: \"one\""));
                    ("200 OK", Vec::new(), "ETag: \"two\"\r\n")
                }
                _ => {
                    assert!(headers.contains("if-match: \"one\""));
                    ("412 Precondition Failed", Vec::new(), "")
                }
            };
            write!(stream, "HTTP/1.1 {status}\r\n{etag}Content-Length: {}\r\nLast-Modified: Sun, 11 Oct 2026 00:00:00 GMT\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let storage = S3Storage::for_test_server(&endpoint);
    assert!(storage.get().unwrap().is_none());
    let local = inventory();
    let vault = Vault::capture(&local, secrets(&local).as_ref()).unwrap();
    let (encrypted, _) = crypto::create(&vault, PASSWORD).unwrap();
    let first = storage.put(encrypted.clone(), None).unwrap();
    assert_eq!(first.etag, "\"one\"");
    let remote = storage.get().unwrap().unwrap();
    assert_eq!(remote.bytes, encrypted);
    let updated = storage.put(encrypted.clone(), Some(&first)).unwrap();
    assert_eq!(updated.etag, "\"two\"");
    assert!(storage.put(encrypted, Some(&first)).is_err());
    server.join().unwrap();
}
