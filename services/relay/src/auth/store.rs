//! 认证存储读写。

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::api::types::AuthStore;

/// 当前 unix 秒。
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 认证存储路径。
pub(crate) fn auth_store_path() -> PathBuf {
    if let Ok(path) = std::env::var("RELAY_AUTH_STORE_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("yourconnector")
        .join("relay")
        .join("auth-store.json")
}

/// 加载认证元数据。
pub(crate) fn load_auth_store(path: &Path) -> Result<AuthStore, String> {
    if !path.exists() {
        return Ok(AuthStore::new(generate_signing_key_seed()));
    }
    let raw = fs::read(path).map_err(|err| format!("read auth store failed: {err}"))?;
    let mut parsed: AuthStore =
        serde_json::from_slice(&raw).map_err(|err| format!("decode auth store failed: {err}"))?;
    if parsed.signing_key.trim().is_empty() {
        parsed.signing_key = generate_signing_key_seed();
    }
    Ok(parsed)
}

/// 持久化认证元数据。
pub(crate) fn persist_auth_store(path: &Path, store: &AuthStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create auth store dir failed: {err}"))?;
    }
    let encoded = serde_json::to_vec_pretty(store)
        .map_err(|err| format!("encode auth store failed: {err}"))?;
    fs::write(path, encoded).map_err(|err| format!("write auth store failed: {err}"))
}

/// 生成 relay 自身 token 签名种子。
pub(crate) fn generate_signing_key_seed() -> String {
    format!(
        "relay_sk_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
