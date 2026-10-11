//! 可移植的保险库格式。秘密字段不实现 Debug，序列化缓冲区由调用方清零。

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::Path,
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::host::{
    AuthKind, Credential, CredentialDraft, CredentialId, CredentialKind, GroupDraft, GroupId, Host,
    HostDraft, HostGroup, HostId, HostInventory, ProxyKind, ProxySettings, PublicId, Route,
};
use crate::i18n::t;
use crate::secrets::{SecretRef, SecretStore};

const MAX_RECORDS: usize = 10_000;
const MAX_KEY_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
pub(super) struct Vault {
    pub revision: u64,
    pub groups: Vec<VaultGroup>,
    pub hosts: Vec<VaultHost>,
    pub credentials: Vec<VaultCredential>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct VaultGroup {
    id: String,
    name: String,
    parent: Option<String>,
    order: i64,
}

#[derive(Serialize, Deserialize)]
pub(super) struct VaultHost {
    id: String,
    name: String,
    address: String,
    port: u16,
    user: String,
    auth: String,
    credential: Option<String>,
    route: VaultRoute,
    group: Option<String>,
    notes: String,
    order: i64,
    password: Option<Zeroizing<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum VaultRoute {
    Direct,
    Jump {
        hosts: Vec<Option<String>>,
    },
    Proxy {
        protocol: String,
        address: String,
        port: u16,
        user: Option<String>,
        password: Option<Zeroizing<String>>,
    },
}

#[derive(Serialize, Deserialize)]
pub(super) struct VaultCredential {
    id: String,
    name: String,
    kind: String,
    user: String,
    order: i64,
    password: Option<Zeroizing<String>>,
    pub(super) private_key: Option<Zeroizing<String>>,
    passphrase: Option<Zeroizing<String>>,
}

impl Vault {
    pub fn capture(inventory: &HostInventory, secrets: &dyn SecretStore) -> Result<Self> {
        ensure!(
            inventory.groups.len() + inventory.hosts.len() + inventory.credentials.len()
                <= MAX_RECORDS,
            t!("sync.error.size")
        );
        let group_ids: HashMap<_, _> = inventory
            .groups
            .iter()
            .map(|group| (group.id, group.public_id.to_string()))
            .collect();
        let host_ids: HashMap<_, _> = inventory
            .hosts
            .iter()
            .map(|host| (host.id, host.public_id.to_string()))
            .collect();
        let credential_ids: HashMap<_, _> = inventory
            .credentials
            .iter()
            .map(|credential| (credential.id, credential.keychain_id.to_string()))
            .collect();
        let groups = inventory
            .groups
            .iter()
            .map(|group| VaultGroup {
                id: group.public_id.to_string(),
                name: group.name.to_string(),
                parent: group.parent.and_then(|id| group_ids.get(&id).cloned()),
                order: group.sort_order,
            })
            .collect();
        let mut key_bytes = 0usize;
        let credentials = inventory
            .credentials
            .iter()
            .map(|credential| -> Result<_> {
                let (private_key, passphrase) = match credential.key_path.as_deref() {
                    Some(path) => {
                        let mut text = Zeroizing::new(String::new());
                        File::open(path)?
                            .take(MAX_KEY_BYTES + 1)
                            .read_to_string(&mut text)?;
                        ensure!(
                            text.len() as u64 <= MAX_KEY_BYTES,
                            t!("sync.error.key_size")
                        );
                        key_bytes += text.len();
                        ensure!(key_bytes <= 32 * 1024 * 1024, t!("sync.error.size"));
                        (Some(text), secrets.get(&SecretRef::passphrase(path))?)
                    }
                    None => (None, None),
                };
                Ok(VaultCredential {
                    id: credential.keychain_id.to_string(),
                    name: credential.name.to_string(),
                    kind: credential.kind.as_str().into(),
                    user: credential.user.to_string(),
                    order: credential.sort_order,
                    password: if credential.kind == CredentialKind::Password {
                        secrets.get(&credential.password_secret())?
                    } else {
                        None
                    },
                    private_key,
                    passphrase,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let hosts = inventory
            .hosts
            .iter()
            .map(|host| -> Result<_> {
                let route = match &host.route {
                    Route::Direct => VaultRoute::Direct,
                    Route::Jump(hops) => VaultRoute::Jump {
                        hosts: hops
                            .iter()
                            .map(|hop| hop.and_then(|id| host_ids.get(&id).cloned()))
                            .collect(),
                    },
                    Route::Proxy(proxy) => VaultRoute::Proxy {
                        protocol: proxy.kind.as_str().into(),
                        address: proxy.host.to_string(),
                        port: proxy.port,
                        user: proxy.user.as_ref().map(ToString::to_string),
                        password: match &proxy.user {
                            Some(user) => secrets.get(&SecretRef::proxy(
                                user.as_ref(),
                                proxy.host.as_ref(),
                                proxy.port,
                            ))?,
                            None => None,
                        },
                    },
                };
                Ok(VaultHost {
                    id: host.public_id.to_string(),
                    name: host.name.to_string(),
                    address: host.address.to_string(),
                    port: host.port,
                    user: host.user.to_string(),
                    auth: host.auth.as_str().into(),
                    credential: host
                        .credential
                        .and_then(|id| credential_ids.get(&id).cloned()),
                    route,
                    group: host.group.and_then(|id| group_ids.get(&id).cloned()),
                    notes: host.notes.to_string(),
                    order: host.sort_order,
                    password: secrets.get(&host.password_secret())?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let vault = Self {
            revision: 1,
            groups,
            hosts,
            credentials,
        };
        vault.validate()?;
        Ok(vault)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.revision > 0
                && self.groups.len() + self.hosts.len() + self.credentials.len() <= MAX_RECORDS,
            t!("sync.error.format")
        );
        fn ids<'a>(records: impl Iterator<Item = &'a str>) -> Result<HashSet<&'a str>> {
            let mut ids = HashSet::new();
            for id in records {
                ensure!(
                    id.len() == 16
                        && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
                        && ids.insert(id),
                    t!("sync.error.format")
                );
            }
            Ok(ids)
        }
        let groups = ids(self.groups.iter().map(|group| group.id.as_str()))?;
        let hosts = ids(self.hosts.iter().map(|host| host.id.as_str()))?;
        let credentials = ids(self
            .credentials
            .iter()
            .map(|credential| credential.id.as_str()))?;
        let parents: HashMap<_, _> = self
            .groups
            .iter()
            .map(|group| (group.id.as_str(), group.parent.as_deref()))
            .collect();
        for group in &self.groups {
            ensure!(!group.name.trim().is_empty(), t!("sync.error.format"));
            let mut seen = HashSet::new();
            let mut current = Some(group.id.as_str());
            while let Some(id) = current {
                ensure!(
                    seen.insert(id) && groups.contains(id),
                    t!("sync.error.format")
                );
                current = parents[id];
            }
        }
        for credential in &self.credentials {
            let kind = CredentialKind::from_stored(&credential.kind)
                .ok_or_else(|| anyhow::anyhow!(t!("sync.error.format")))?;
            ensure!(
                !credential.name.trim().is_empty() && !credential.user.trim().is_empty(),
                t!("sync.error.format")
            );
            ensure!(
                (kind == CredentialKind::Key) == credential.private_key.is_some(),
                t!("sync.error.format")
            );
            ensure!(
                kind == CredentialKind::Password || credential.password.is_none(),
                t!("sync.error.format")
            );
            ensure!(
                kind == CredentialKind::Key || credential.passphrase.is_none(),
                t!("sync.error.format")
            );
            if let Some(key) = &credential.private_key {
                ensure!(
                    key.len() as u64 <= MAX_KEY_BYTES && !key.trim().is_empty(),
                    t!("sync.error.key_size")
                );
                crate::host::PastedKey::parse(key)
                    .map_err(|_| anyhow::anyhow!(t!("sync.error.key_format")))?;
            }
        }
        for host in &self.hosts {
            ensure!(
                !host.name.trim().is_empty()
                    && !host.address.trim().is_empty()
                    && !host.user.trim().is_empty()
                    && host.port > 0,
                t!("sync.error.format")
            );
            ensure!(
                matches!(host.auth.as_str(), "password" | "no-password"),
                t!("sync.error.format")
            );
            ensure!(
                host.group.as_deref().is_none_or(|id| groups.contains(id))
                    && host
                        .credential
                        .as_deref()
                        .is_none_or(|id| credentials.contains(id)),
                t!("sync.error.format")
            );
            match &host.route {
                VaultRoute::Direct => {}
                VaultRoute::Jump { hosts: hops } => ensure!(
                    hops.len() <= 64
                        && hops
                            .iter()
                            .flatten()
                            .all(|id| id != &host.id && hosts.contains(id.as_str())),
                    t!("sync.error.format")
                ),
                VaultRoute::Proxy {
                    protocol,
                    address,
                    port,
                    user,
                    password,
                } => ensure!(
                    ProxyKind::from_stored(protocol).is_some()
                        && !address.trim().is_empty()
                        && *port > 0
                        && (user.is_some() || password.is_none()),
                    t!("sync.error.format")
                ),
            }
            if let Some(id) = &host.credential {
                ensure!(
                    self.credentials
                        .iter()
                        .find(|credential| &credential.id == id)
                        .is_some_and(|credential| credential.user == host.user),
                    t!("sync.error.format")
                );
            }
        }
        // 同一个端点的密码必须一致，防止导入顺序决定实际密码。
        self.secret_changes(&HashMap::new(), false)?;
        Ok(())
    }

    /// 使用稳定标识匹配本地清单，新增项从本机计数器之后分配。
    pub fn inventory(
        &self,
        local: &HostInventory,
        key_paths: &HashMap<String, String>,
    ) -> HostInventory {
        let mut next = local.next_ids;
        let mut allocate = |slot: usize| {
            let id = next[slot];
            next[slot] += 1;
            id
        };
        let group_ids: HashMap<_, _> = self
            .groups
            .iter()
            .map(|group| {
                (
                    &group.id,
                    local
                        .groups
                        .iter()
                        .find(|item| item.public_id.as_str() == group.id)
                        .map(|item| item.id)
                        .unwrap_or_else(|| GroupId(allocate(1))),
                )
            })
            .collect();
        let credential_ids: HashMap<_, _> = self
            .credentials
            .iter()
            .map(|credential| {
                (
                    &credential.id,
                    local
                        .credentials
                        .iter()
                        .find(|item| item.keychain_id.as_str() == credential.id)
                        .map(|item| item.id)
                        .unwrap_or_else(|| CredentialId(allocate(2))),
                )
            })
            .collect();
        let host_ids: HashMap<_, _> = self
            .hosts
            .iter()
            .map(|host| {
                (
                    &host.id,
                    local
                        .hosts
                        .iter()
                        .find(|item| item.public_id.as_str() == host.id)
                        .map(|item| item.id)
                        .unwrap_or_else(|| HostId(allocate(0))),
                )
            })
            .collect();
        let groups = self
            .groups
            .iter()
            .map(|group| {
                let mut item = HostGroup::new(
                    group_ids[&group.id],
                    GroupDraft::new(
                        group.name.clone(),
                        group.parent.as_ref().map(|id| group_ids[id]),
                    ),
                );
                item.public_id = PublicId::from_stored(group.id.clone());
                item.sort_order = group.order;
                item.expanded = local
                    .groups
                    .iter()
                    .find(|old| old.public_id == item.public_id)
                    .is_none_or(|old| old.expanded);
                item
            })
            .collect();
        let credentials = self
            .credentials
            .iter()
            .map(|credential| {
                let mut draft = CredentialDraft::new(
                    credential.name.clone(),
                    CredentialKind::from_stored(&credential.kind)
                        .expect("validated credential kind"),
                    credential.user.clone(),
                );
                draft.key_path = key_paths.get(&credential.id).cloned().map(Into::into);
                let mut item = Credential::new(credential_ids[&credential.id], draft);
                item.keychain_id = PublicId::from_stored(credential.id.clone());
                item.sort_order = credential.order;
                item
            })
            .collect();
        let hosts = self
            .hosts
            .iter()
            .map(|host| {
                let mut draft = HostDraft::new(
                    host.name.clone(),
                    host.address.clone(),
                    host.port,
                    host.user.clone(),
                    AuthKind::from_stored(&host.auth),
                    host.group.as_ref().map(|id| group_ids[id]),
                )
                .with_notes(host.notes.clone());
                draft.credential = host.credential.as_ref().map(|id| credential_ids[id]);
                draft.route = match &host.route {
                    VaultRoute::Direct => Route::Direct,
                    VaultRoute::Jump { hosts } => Route::Jump(
                        hosts
                            .iter()
                            .map(|id| id.as_ref().map(|id| host_ids[id]))
                            .collect(),
                    ),
                    VaultRoute::Proxy {
                        protocol,
                        address,
                        port,
                        user,
                        ..
                    } => Route::Proxy(
                        ProxySettings::new(
                            ProxyKind::from_stored(protocol).expect("validated proxy kind"),
                            address.clone(),
                            *port,
                        )
                        .with_user(user.clone().unwrap_or_default()),
                    ),
                };
                let mut item = Host::new(host_ids[&host.id], draft);
                item.public_id = PublicId::from_stored(host.id.clone());
                item.sort_order = host.order;
                item.os = local
                    .hosts
                    .iter()
                    .find(|old| old.public_id == item.public_id)
                    .and_then(|old| old.os);
                item
            })
            .collect();
        HostInventory::new(groups, hosts, credentials)
    }

    pub fn write_keys(
        &self,
        dir: &Path,
        written: &mut Vec<std::path::PathBuf>,
    ) -> Result<HashMap<String, String>> {
        let mut paths = HashMap::new();
        for credential in &self.credentials {
            if let Some(key) = &credential.private_key {
                let path = crate::host::write_key_file(dir, None, key)?;
                paths.insert(credential.id.clone(), path.display().to_string());
                written.push(path);
            }
        }
        Ok(paths)
    }

    pub fn secret_changes(
        &self,
        paths: &HashMap<String, String>,
        include_keys: bool,
    ) -> Result<Vec<(SecretRef, Option<Zeroizing<String>>)>> {
        let mut values: HashMap<SecretRef, Option<Zeroizing<String>>> = HashMap::new();
        let mut insert = |secret, value: &Option<Zeroizing<String>>| -> Result<()> {
            if let Some(previous) = values.get(&secret) {
                ensure!(previous == value, t!("sync.error.format"));
            }
            values.insert(secret, value.clone());
            Ok(())
        };
        for credential in &self.credentials {
            if credential.kind == "password" {
                insert(SecretRef::credential(&credential.id), &credential.password)?;
            }
            if include_keys && let Some(path) = paths.get(&credential.id) {
                insert(SecretRef::passphrase(path), &credential.passphrase)?;
            }
        }
        for host in &self.hosts {
            insert(
                SecretRef::password(&host.user, &host.address, host.port),
                &host.password,
            )?;
            if let VaultRoute::Proxy {
                address,
                port,
                user: Some(user),
                password,
                ..
            } = &host.route
            {
                insert(SecretRef::proxy(user, address, *port), password)?;
            }
        }
        Ok(values.into_iter().collect())
    }
}
