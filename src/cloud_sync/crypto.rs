use anyhow::{Result, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::vault::Vault;
use crate::i18n::t;

pub(super) const MAX_ENCRYPTED_BYTES: usize = 48 * 1024 * 1024;
const MAX_PLAIN_BYTES: usize = 32 * 1024 * 1024;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    vault_id: String,
    salt: [u8; 16],
    key_nonce: [u8; 24],
    wrapped_key: String,
    recovery_nonce: [u8; 24],
    recovery_wrapped_key: String,
    data_nonce: [u8; 24],
    ciphertext: String,
}

pub(super) struct OpenVault {
    envelope: Envelope,
    key: Zeroizing<[u8; 32]>,
    pub vault: Vault,
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

fn password_key(password: &str, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>> {
    // 格式版本固定参数，远端文件不能要求任意大的内存或迭代次数。
    let parameters = Params::new(64 * 1024, 3, 4, Some(32))
        .map_err(|_| anyhow::anyhow!(t!("sync.error.crypto")))?;
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, parameters)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|_| anyhow::anyhow!(t!("sync.error.crypto")))?;
    Ok(key)
}

fn aad(version: u32, id: &str, purpose: &str) -> Vec<u8> {
    format!("shellrs-vault:{version}:{id}:{purpose}").into_bytes()
}

/// 内容认证覆盖整个密钥封套，任一解锁方式的封套被改动都会被发现。
fn data_aad(envelope: &Envelope) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&(
        envelope.version,
        &envelope.vault_id,
        &envelope.salt,
        &envelope.key_nonce,
        &envelope.wrapped_key,
        &envelope.recovery_nonce,
        &envelope.recovery_wrapped_key,
        &envelope.data_nonce,
    ))?)
}

fn encrypt(key: &[u8; 32], nonce: &[u8; 24], plain: &[u8], aad: &[u8]) -> Result<String> {
    let encrypted = XChaCha20Poly1305::new(key.into())
        .encrypt(XNonce::from_slice(nonce), Payload { msg: plain, aad })
        .map_err(|_| anyhow::anyhow!(t!("sync.error.crypto")))?;
    Ok(STANDARD.encode(encrypted))
}

fn decrypt(
    key: &[u8; 32],
    nonce: &[u8; 24],
    ciphertext: &str,
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let bytes = STANDARD
        .decode(ciphertext)
        .map_err(|_| anyhow::anyhow!(t!("sync.error.format")))?;
    let plain = XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), Payload { msg: &bytes, aad })
        .map_err(|_| anyhow::anyhow!(t!("sync.error.unlock")))?;
    Ok(Zeroizing::new(plain))
}

pub(super) fn create(vault: &Vault, password: &str) -> Result<(Vec<u8>, Zeroizing<String>)> {
    ensure!(
        password.chars().count() >= 12,
        t!("sync.error.password_length")
    );
    let id = uuid::Uuid::new_v4().to_string();
    let key = Zeroizing::new(random::<32>());
    let recovery = Zeroizing::new(random::<32>());
    let salt = random();
    let key_nonce = random();
    let recovery_nonce = random();
    let wrapping_key = password_key(password, &salt)?;
    let envelope = Envelope {
        version: FORMAT_VERSION,
        vault_id: id.clone(),
        salt,
        key_nonce,
        wrapped_key: encrypt(
            &wrapping_key,
            &key_nonce,
            key.as_ref(),
            &aad(FORMAT_VERSION, &id, "password"),
        )?,
        recovery_nonce,
        recovery_wrapped_key: encrypt(
            &recovery,
            &recovery_nonce,
            key.as_ref(),
            &aad(FORMAT_VERSION, &id, "recovery"),
        )?,
        data_nonce: [0; 24],
        ciphertext: String::new(),
    };
    let bytes = seal(envelope, &key, vault)?;
    Ok((
        bytes,
        Zeroizing::new(format!(
            "shellrs-recovery-{}",
            URL_SAFE_NO_PAD.encode(recovery.as_ref())
        )),
    ))
}

fn seal(mut envelope: Envelope, key: &[u8; 32], vault: &Vault) -> Result<Vec<u8>> {
    vault.validate()?;
    let plain = Zeroizing::new(serde_json::to_vec(vault)?);
    ensure!(plain.len() <= MAX_PLAIN_BYTES, t!("sync.error.size"));
    envelope.data_nonce = random();
    envelope.ciphertext = encrypt(key, &envelope.data_nonce, &plain, &data_aad(&envelope)?)?;
    let bytes = serde_json::to_vec(&envelope)?;
    ensure!(bytes.len() <= MAX_ENCRYPTED_BYTES, t!("sync.error.size"));
    Ok(bytes)
}

pub(super) fn open(bytes: &[u8], password: &str, recovery: &str) -> Result<OpenVault> {
    ensure!(bytes.len() <= MAX_ENCRYPTED_BYTES, t!("sync.error.size"));
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!(t!("sync.error.format")))?;
    ensure!(
        envelope.version == FORMAT_VERSION
            && uuid::Uuid::parse_str(&envelope.vault_id).is_ok()
            && envelope.wrapped_key.len() == 64
            && envelope.recovery_wrapped_key.len() == 64,
        t!("sync.error.format")
    );
    let unwrapped = if recovery.trim().is_empty() {
        ensure!(!password.is_empty(), t!("sync.error.password"));
        let wrapping = password_key(password, &envelope.salt)?;
        decrypt(
            &wrapping,
            &envelope.key_nonce,
            &envelope.wrapped_key,
            &aad(envelope.version, &envelope.vault_id, "password"),
        )?
    } else {
        let encoded = recovery
            .trim()
            .strip_prefix("shellrs-recovery-")
            .ok_or_else(|| anyhow::anyhow!(t!("sync.error.unlock")))?;
        let decoded = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| anyhow::anyhow!(t!("sync.error.unlock")))?,
        );
        let recovery_key: Zeroizing<[u8; 32]> = Zeroizing::new(
            decoded
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!(t!("sync.error.unlock")))?,
        );
        decrypt(
            &recovery_key,
            &envelope.recovery_nonce,
            &envelope.recovery_wrapped_key,
            &aad(envelope.version, &envelope.vault_id, "recovery"),
        )?
    };
    let key = Zeroizing::new(
        unwrapped
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!(t!("sync.error.format")))?,
    );
    open_key(envelope, key)
}

fn open_key(envelope: Envelope, key: Zeroizing<[u8; 32]>) -> Result<OpenVault> {
    ensure!(envelope.version == FORMAT_VERSION, t!("sync.error.format"));
    let plain = decrypt(
        &key,
        &envelope.data_nonce,
        &envelope.ciphertext,
        &data_aad(&envelope)?,
    )?;
    ensure!(plain.len() <= MAX_PLAIN_BYTES, t!("sync.error.size"));
    let vault: Vault =
        serde_json::from_slice(&plain).map_err(|_| anyhow::anyhow!(t!("sync.error.format")))?;
    vault.validate()?;
    Ok(OpenVault {
        envelope,
        key,
        vault,
    })
}

impl OpenVault {
    /// 仅保存保险库数据密钥到系统钥匙串，主密码不落盘。
    pub fn device_key(&self) -> Result<Zeroizing<String>> {
        Ok(Zeroizing::new(serde_json::to_string(&(
            self.id(),
            self.key.as_ref(),
        ))?))
    }

    pub fn open_device(bytes: &[u8], saved: &str) -> Result<Self> {
        ensure!(bytes.len() <= MAX_ENCRYPTED_BYTES, t!("sync.error.size"));
        let envelope: Envelope =
            serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!(t!("sync.error.format")))?;
        let (id, key): (String, Zeroizing<[u8; 32]>) =
            serde_json::from_str(saved).map_err(|_| anyhow::anyhow!(t!("sync.error.unlock")))?;
        ensure!(id == envelope.vault_id, t!("sync.error.rollback"));
        open_key(envelope, key)
    }

    pub fn content_digest(&self, vault: &Vault) -> Result<[u8; 32]> {
        // 摘要加密钥，游标文件不能用于离线猜测凭据。
        let mut digest = Sha256::new();
        digest.update(self.key.as_ref());
        digest.update(vault.canonical_content()?.as_slice());
        Ok(digest.finalize().into())
    }

    pub fn id(&self) -> &str {
        &self.envelope.vault_id
    }

    pub fn seal(&self, vault: &Vault) -> Result<Vec<u8>> {
        seal(self.envelope.clone(), &self.key, vault)
    }
}
