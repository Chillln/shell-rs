use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::i18n::t;
use crate::secrets::SecretRef;

/// 仅保存非秘密配置；访问密钥和令牌保存在系统钥匙串。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct CloudSyncSettings {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub object_key: String,
    pub access_key_id: String,
    pub virtual_hosted: bool,
    /// 开启后在成功手动同步时把数据密钥保存在本机钥匙串。
    pub automatic: bool,
}

impl Default for CloudSyncSettings {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            region: "us-east-1".into(),
            bucket: String::new(),
            object_key: "shellrs/vault.enc".into(),
            access_key_id: String::new(),
            virtual_hosted: false,
            automatic: true,
        }
    }
}

impl CloudSyncSettings {
    pub fn validated(mut self) -> Result<Self> {
        self.endpoint = self.endpoint.trim().trim_end_matches('/').into();
        self.region = self.region.trim().into();
        self.bucket = self.bucket.trim().into();
        self.object_key = self.object_key.trim().into();
        self.access_key_id = self.access_key_id.trim().into();
        if !self.endpoint.is_empty() {
            let endpoint = url::Url::parse(&self.endpoint)
                .map_err(|_| anyhow::anyhow!(t!("sync.error.endpoint")))?;
            ensure!(
                endpoint.scheme() == "https"
                    && endpoint.host_str().is_some()
                    && endpoint.username().is_empty()
                    && endpoint.password().is_none()
                    && endpoint.query().is_none()
                    && endpoint.fragment().is_none(),
                t!("sync.error.endpoint")
            );
        }
        ensure!(
            !self.region.is_empty() && !self.bucket.is_empty() && !self.access_key_id.is_empty(),
            t!("sync.error.config")
        );
        ensure!(
            !self.object_key.is_empty()
                && !self.object_key.starts_with('/')
                && !self
                    .object_key
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == ".."),
            t!("sync.error.object_key")
        );
        Ok(self)
    }

    /// 远端位置与签名方式共同标识配置，切换位置后不会沿用旧版本游标。
    pub(crate) fn scope(&self) -> String {
        // 自动同步偏好不改变远端身份，保留已有钥匙串条目和版本游标。
        #[derive(Serialize)]
        struct Profile<'a> {
            endpoint: &'a str,
            region: &'a str,
            bucket: &'a str,
            object_key: &'a str,
            access_key_id: &'a str,
            virtual_hosted: bool,
        }
        let encoded = serde_json::to_vec(&Profile {
            endpoint: &self.endpoint,
            region: &self.region,
            bucket: &self.bucket,
            object_key: &self.object_key,
            access_key_id: &self.access_key_id,
            virtual_hosted: self.virtual_hosted,
        })
        .expect("sync configuration is serializable");
        format!("{:x}", Sha256::digest(encoded))
    }

    pub(crate) fn secret(&self, name: &str) -> SecretRef {
        SecretRef::cloud_sync(self.scope(), name)
    }
}
