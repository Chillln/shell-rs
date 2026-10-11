use std::{sync::Arc, time::Duration};

use anyhow::{Result, ensure};
use futures::TryStreamExt as _;
use object_store::{
    ClientOptions, ObjectStore, PutMode, PutOptions, RetryConfig, UpdateVersion,
    aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut},
    path::Path,
};

use super::{crypto::MAX_ENCRYPTED_BYTES, model::CloudSyncSettings};
use crate::i18n::t;
use crate::secrets::SecretStore;

pub(super) struct RemoteObject {
    pub bytes: Vec<u8>,
    pub etag: String,
    pub version: Option<String>,
}

pub(super) struct ObjectVersion {
    pub etag: String,
    pub version: Option<String>,
}

/// 同步协议只依赖带条件写入的密文存储，测试可以完全注入。
pub(super) trait VaultStorage: Send + Sync {
    fn get(&self) -> Result<Option<RemoteObject>>;
    fn put(&self, bytes: Vec<u8>, expected: Option<&ObjectVersion>) -> Result<ObjectVersion>;
}

pub(super) struct S3Storage {
    store: AmazonS3,
    runtime: tokio::runtime::Runtime,
    path: Path,
}

impl S3Storage {
    #[cfg(test)]
    pub(super) fn for_test_server(endpoint: &str) -> Self {
        let store = AmazonS3Builder::new()
            .with_endpoint(endpoint)
            .with_bucket_name("test-bucket")
            .with_region("us-east-1")
            .with_access_key_id("test-access")
            .with_secret_access_key("test-secret")
            .with_allow_http(true)
            .with_conditional_put(S3ConditionalPut::ETagMatch)
            .with_retry(RetryConfig {
                max_retries: 0,
                ..Default::default()
            })
            .build()
            .unwrap();
        Self {
            store,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            path: Path::from("shellrs/vault.enc"),
        }
    }
    pub fn build(
        settings: &CloudSyncSettings,
        secrets: &dyn SecretStore,
    ) -> Result<Arc<dyn VaultStorage>> {
        let settings = settings.clone().validated()?;
        let key = secrets
            .get(&settings.secret("secret-access-key"))?
            .filter(|key| !key.is_empty())
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.access_key")))?;
        let token = secrets.get(&settings.secret("session-token"))?;
        let mut builder = AmazonS3Builder::new()
            .with_region(&settings.region)
            .with_bucket_name(&settings.bucket)
            .with_access_key_id(&settings.access_key_id)
            .with_secret_access_key(key.as_str())
            .with_virtual_hosted_style_request(settings.virtual_hosted)
            .with_conditional_put(S3ConditionalPut::ETagMatch)
            .with_allow_http(false)
            .with_client_options(
                ClientOptions::new()
                    .with_timeout(Duration::from_secs(30))
                    .with_connect_timeout(Duration::from_secs(10)),
            )
            .with_retry(RetryConfig {
                max_retries: 2,
                retry_timeout: Duration::from_secs(60),
                ..Default::default()
            });
        if !settings.endpoint.is_empty() {
            builder = builder.with_endpoint(&settings.endpoint);
        }
        if let Some(token) = token.filter(|token| !token.is_empty()) {
            builder = builder.with_token(token.as_str());
        }
        let store = builder.build().map_err(storage_error)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let path = Path::parse(&settings.object_key)
            .map_err(|_| anyhow::anyhow!(t!("sync.error.object_key")))?;
        Ok(Arc::new(Self {
            store,
            runtime,
            path,
        }))
    }
}

fn storage_error(error: object_store::Error) -> anyhow::Error {
    // 服务端的响应体和请求头不进入界面或日志，避免输出认证材料。
    anyhow::anyhow!(match error {
        object_store::Error::Precondition { .. } | object_store::Error::AlreadyExists { .. } =>
            t!("sync.error.conflict"),
        object_store::Error::PermissionDenied { .. }
        | object_store::Error::Unauthenticated { .. } => t!("sync.error.permission"),
        object_store::Error::NotImplemented | object_store::Error::NotSupported { .. } =>
            t!("sync.error.conditional"),
        _ => t!("sync.error.network"),
    })
}

impl VaultStorage for S3Storage {
    fn get(&self) -> Result<Option<RemoteObject>> {
        self.runtime.block_on(async {
            let result = match self.store.get(&self.path).await {
                Ok(result) => result,
                Err(object_store::Error::NotFound { .. }) => return Ok(None),
                Err(error) => return Err(storage_error(error)),
            };
            ensure!(
                result.meta.size <= MAX_ENCRYPTED_BYTES as u64,
                t!("sync.error.size")
            );
            let etag = result
                .meta
                .e_tag
                .clone()
                .filter(|tag| !tag.is_empty())
                .ok_or_else(|| anyhow::anyhow!(t!("sync.error.etag")))?;
            let version = result.meta.version.clone();
            let mut stream = result.into_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.try_next().await.map_err(storage_error)? {
                ensure!(
                    bytes.len() + chunk.len() <= MAX_ENCRYPTED_BYTES,
                    t!("sync.error.size")
                );
                bytes.extend_from_slice(&chunk);
            }
            Ok(Some(RemoteObject {
                bytes,
                etag,
                version,
            }))
        })
    }

    fn put(&self, bytes: Vec<u8>, expected: Option<&ObjectVersion>) -> Result<ObjectVersion> {
        ensure!(bytes.len() <= MAX_ENCRYPTED_BYTES, t!("sync.error.size"));
        let mode = match expected {
            Some(version) => PutMode::Update(UpdateVersion {
                e_tag: Some(version.etag.clone()),
                version: version.version.clone(),
            }),
            None => PutMode::Create,
        };
        let result = self
            .runtime
            .block_on(self.store.put_opts(
                &self.path,
                bytes.into(),
                PutOptions {
                    mode,
                    ..Default::default()
                },
            ))
            .map_err(storage_error)?;
        let etag = result
            .e_tag
            .filter(|tag| !tag.is_empty())
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.etag")))?;
        Ok(ObjectVersion {
            etag,
            version: result.version,
        })
    }
}
