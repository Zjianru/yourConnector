// 文件职责：
// 1. 启动 Tauri Mobile 应用并监听配对深链。
// 2. 提供前端可调用的安全凭证命令（设备密钥、签名、会话存取）。

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
use std::collections::HashMap;
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
use std::sync::{Mutex, OnceLock};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{Manager, RunEvent};

/// Keychain 服务名：设备私钥。
const KEYCHAIN_SERVICE_DEVICE_KEY: &str = "dev.yourconnector.mobile.device-key";
/// Keychain 服务名：设备会话。
const KEYCHAIN_SERVICE_DEVICE_SESSION: &str = "dev.yourconnector.mobile.device-session";

/// 设备公钥响应体。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceKeyBinding {
    key_id: String,
    public_key: String,
}

/// 签名响应体。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceSignature {
    key_id: String,
    public_key: String,
    signature: String,
}

/// 设备会话结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceSession {
    system_id: String,
    device_id: String,
    access_token: String,
    refresh_token: String,
    key_id: String,
    credential_id: String,
}

/// 非 Apple 平台下的简易内存安全存储（仅用于开发构建兜底）。
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
fn fallback_secure_store() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 从 Keychain（或兜底存储）读取字节。
fn secure_get(service: &str, account: &str) -> Option<Vec<u8>> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        security_framework::passwords::get_generic_password(service, account).ok()
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        let key = format!("{service}::{account}");
        fallback_secure_store()
            .lock()
            .ok()
            .and_then(|guard| guard.get(&key).cloned())
    }
}

/// 写入 Keychain（或兜底存储）字节。
fn secure_set(service: &str, account: &str, value: &[u8]) -> Result<(), String> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        security_framework::passwords::set_generic_password(service, account, value)
            .map_err(|err| format!("keychain set failed: {err}"))
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        let key = format!("{service}::{account}");
        let mut guard = fallback_secure_store()
            .lock()
            .map_err(|_| "secure store lock failed".to_string())?;
        guard.insert(key, value.to_vec());
        Ok(())
    }
}

/// 删除 Keychain（或兜底存储）字节。
fn secure_delete(service: &str, account: &str) -> Result<(), String> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        security_framework::passwords::delete_generic_password(service, account)
            .map_err(|err| format!("keychain delete failed: {err}"))
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        let key = format!("{service}::{account}");
        let mut guard = fallback_secure_store()
            .lock()
            .map_err(|_| "secure store lock failed".to_string())?;
        guard.remove(&key);
        Ok(())
    }
}

/// 生成设备私钥存储键。
fn device_private_key_account(device_id: &str) -> String {
    format!("device:{device_id}:ed25519")
}

/// 生成设备会话存储键。
fn device_session_account(system_id: &str, device_id: &str) -> String {
    format!("session:{system_id}:{device_id}")
}

/// 根据公钥生成稳定 keyId。
fn key_id_for_public_key(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    format!("kid_{}", URL_SAFE_NO_PAD.encode(&digest[..10]))
}

/// 读取或创建设备私钥。
fn load_or_create_signing_key(device_id: &str) -> Result<SigningKey, String> {
    let account = device_private_key_account(device_id);
    if let Some(raw) = secure_get(KEYCHAIN_SERVICE_DEVICE_KEY, &account) {
        if raw.len() != 32 {
            return Err("device key length invalid".to_string());
        }
        let mut seed = [0_u8; 32];
        seed.copy_from_slice(&raw);
        return Ok(SigningKey::from_bytes(&seed));
    }

    let mut seed = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    secure_set(KEYCHAIN_SERVICE_DEVICE_KEY, &account, &seed)?;
    Ok(SigningKey::from_bytes(&seed))
}

/// 读取或创建设备密钥对并返回公开信息。
#[tauri::command]
fn auth_get_device_binding(device_id: String) -> Result<DeviceKeyBinding, String> {
    let normalized_device = device_id.trim();
    if normalized_device.is_empty() {
        return Err("deviceId 不能为空".to_string());
    }
    let signing_key = load_or_create_signing_key(normalized_device)?;
    let verifying_key = signing_key.verifying_key();
    let pub_bytes = verifying_key.to_bytes();
    Ok(DeviceKeyBinding {
        key_id: key_id_for_public_key(&pub_bytes),
        public_key: URL_SAFE_NO_PAD.encode(pub_bytes),
    })
}

/// 使用设备私钥对给定 payload 进行签名。
#[tauri::command]
fn auth_sign_payload(device_id: String, payload: String) -> Result<DeviceSignature, String> {
    let normalized_device = device_id.trim();
    if normalized_device.is_empty() {
        return Err("deviceId 不能为空".to_string());
    }
    let signing_key = load_or_create_signing_key(normalized_device)?;
    let verifying_key = signing_key.verifying_key();
    let pub_bytes = verifying_key.to_bytes();
    let signature = signing_key.sign(payload.as_bytes());
    Ok(DeviceSignature {
        key_id: key_id_for_public_key(&pub_bytes),
        public_key: URL_SAFE_NO_PAD.encode(pub_bytes),
        signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    })
}

/// 将设备会话凭证写入 Keychain。
#[tauri::command]
fn auth_store_session(session: DeviceSession) -> Result<(), String> {
    let account = device_session_account(&session.system_id, &session.device_id);
    let encoded =
        serde_json::to_vec(&session).map_err(|err| format!("encode session failed: {err}"))?;
    secure_set(KEYCHAIN_SERVICE_DEVICE_SESSION, &account, &encoded)
}

/// 从 Keychain 读取设备会话凭证。
#[tauri::command]
fn auth_load_session(
    system_id: String,
    device_id: String,
) -> Result<Option<DeviceSession>, String> {
    let account = device_session_account(system_id.trim(), device_id.trim());
    let Some(raw) = secure_get(KEYCHAIN_SERVICE_DEVICE_SESSION, &account) else {
        return Ok(None);
    };
    let parsed: DeviceSession =
        serde_json::from_slice(&raw).map_err(|err| format!("decode session failed: {err}"))?;
    Ok(Some(parsed))
}

/// 清除指定 system/device 的设备会话凭证。
#[tauri::command]
fn auth_clear_session(system_id: String, device_id: String) -> Result<(), String> {
    let account = device_session_account(system_id.trim(), device_id.trim());
    // 某些平台删除不存在条目会返回错误，这里按幂等删除处理。
    let _ = secure_delete(KEYCHAIN_SERVICE_DEVICE_SESSION, &account);
    Ok(())
}

/// 将系统深链事件透传给 WebView。前端通过 `window.__YC_HANDLE_PAIR_LINK__` 接收并解析。
fn forward_pairing_link(app: &tauri::AppHandle, raw_url: &str) {
    let encoded = match serde_json::to_string(raw_url) {
        Ok(value) => value,
        Err(_) => return,
    };
    let script =
        format!("window.__YC_HANDLE_PAIR_LINK__ && window.__YC_HANDLE_PAIR_LINK__({encoded});");
    for webview in app.webview_windows().values() {
        let _ = webview.eval(script.clone());
    }
}

/// 移动端库入口：iOS/Android 目标会从这里启动。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// 启动 Tauri runtime，注册安全凭证命令并监听深链。
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            auth_get_device_binding,
            auth_sign_payload,
            auth_store_session,
            auth_load_session,
            auth_clear_session,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build mobile tauri app")
        .run(|app, event| {
            #[cfg(any(target_os = "ios", target_os = "macos"))]
            if let RunEvent::Opened { urls } = event {
                for url in urls {
                    if url.scheme() == "yc" && url.host_str() == Some("pair") {
                        forward_pairing_link(app, url.as_str());
                    }
                }
            }
        });
}
