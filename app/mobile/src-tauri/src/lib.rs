// 文件职责：
// 1. 启动 Tauri Mobile 应用并监听配对深链。
// 2. 提供前端可调用的安全凭证命令（设备密钥、签名、会话存取）。

#[cfg(all(
    not(any(target_os = "ios", target_os = "macos")),
    not(target_os = "android")
))]
use std::collections::HashMap;
#[cfg(all(
    not(any(target_os = "ios", target_os = "macos")),
    not(target_os = "android")
))]
use std::sync::{Mutex, OnceLock};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
#[cfg(target_os = "android")]
use jni::objects::{JByteArray, JObject, JString, JValue};
#[cfg(target_os = "android")]
use jni::JavaVM;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;
#[cfg(any(target_os = "ios", target_os = "macos"))]
use tauri::RunEvent;

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

/// 聊天存储 bootstrap 返回结构。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatStoreBootstrap {
    index: serde_json::Value,
}

/// 仅非 Apple / 非 Android 平台下的简易内存安全存储（开发构建兜底）。
#[cfg(all(
    not(any(target_os = "ios", target_os = "macos")),
    not(target_os = "android")
))]
fn fallback_secure_store() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static STORE: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Android 端安全存储桥接类路径。
#[cfg(target_os = "android")]
const ANDROID_SECURE_STORE_CLASS: &str = "dev/yourconnector/mobile/SecureStoreBridge";

#[cfg(target_os = "android")]
fn with_android_context<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&mut jni::JNIEnv<'_>, &JObject<'_>) -> Result<T, String>,
{
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|err| format!("android vm init failed: {err}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|err| format!("android attach thread failed: {err}"))?;

    // ndk_context 给出的 context 是 Android runtime 持有的对象句柄，避免在 Rust drop 时误释放。
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };
    let output = f(&mut env, &context);
    std::mem::forget(context);
    output
}

#[cfg(target_os = "android")]
fn android_secure_get(service: &str, account: &str) -> Result<Option<Vec<u8>>, String> {
    with_android_context(|env, context| {
        let class = env
            .find_class(ANDROID_SECURE_STORE_CLASS)
            .map_err(|err| format!("find SecureStoreBridge failed: {err}"))?;
        let service_arg = env
            .new_string(service)
            .map_err(|err| format!("new service string failed: {err}"))?;
        let account_arg = env
            .new_string(account)
            .map_err(|err| format!("new account string failed: {err}"))?;
        let service_obj = JObject::from(service_arg);
        let account_obj = JObject::from(account_arg);

        let result = env
            .call_static_method(
                class,
                "get",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)[B",
                &[
                    JValue::Object(context),
                    JValue::Object(&service_obj),
                    JValue::Object(&account_obj),
                ],
            )
            .map_err(|err| format!("call SecureStoreBridge.get failed: {err}"))?;
        let value_obj = result
            .l()
            .map_err(|err| format!("SecureStoreBridge.get decode failed: {err}"))?;
        if value_obj.is_null() {
            return Ok(None);
        }

        let value_array = JByteArray::from(value_obj);
        let bytes = env
            .convert_byte_array(&value_array)
            .map_err(|err| format!("decode secure bytes failed: {err}"))?;
        Ok(Some(bytes))
    })
}

#[cfg(target_os = "android")]
fn android_secure_set(service: &str, account: &str, value: &[u8]) -> Result<(), String> {
    with_android_context(|env, context| {
        let class = env
            .find_class(ANDROID_SECURE_STORE_CLASS)
            .map_err(|err| format!("find SecureStoreBridge failed: {err}"))?;
        let service_arg = env
            .new_string(service)
            .map_err(|err| format!("new service string failed: {err}"))?;
        let account_arg = env
            .new_string(account)
            .map_err(|err| format!("new account string failed: {err}"))?;
        let value_arg = env
            .byte_array_from_slice(value)
            .map_err(|err| format!("new value bytes failed: {err}"))?;
        let service_obj = JObject::from(service_arg);
        let account_obj = JObject::from(account_arg);
        let value_obj = JObject::from(value_arg);

        let result = env
            .call_static_method(
                class,
                "set",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;[B)Ljava/lang/String;",
                &[
                    JValue::Object(context),
                    JValue::Object(&service_obj),
                    JValue::Object(&account_obj),
                    JValue::Object(&value_obj),
                ],
            )
            .map_err(|err| format!("call SecureStoreBridge.set failed: {err}"))?;
        let err_obj = result
            .l()
            .map_err(|err| format!("SecureStoreBridge.set decode failed: {err}"))?;
        if err_obj.is_null() {
            return Ok(());
        }
        let err_jstr = JString::from(err_obj);
        let err_msg: String = env
            .get_string(&err_jstr)
            .map_err(|err| format!("read SecureStoreBridge.set error failed: {err}"))?
            .into();
        Err(format!("android secure set failed: {err_msg}"))
    })
}

#[cfg(target_os = "android")]
fn android_secure_delete(service: &str, account: &str) -> Result<(), String> {
    with_android_context(|env, context| {
        let class = env
            .find_class(ANDROID_SECURE_STORE_CLASS)
            .map_err(|err| format!("find SecureStoreBridge failed: {err}"))?;
        let service_arg = env
            .new_string(service)
            .map_err(|err| format!("new service string failed: {err}"))?;
        let account_arg = env
            .new_string(account)
            .map_err(|err| format!("new account string failed: {err}"))?;
        let service_obj = JObject::from(service_arg);
        let account_obj = JObject::from(account_arg);

        let result = env
            .call_static_method(
                class,
                "delete",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                &[
                    JValue::Object(context),
                    JValue::Object(&service_obj),
                    JValue::Object(&account_obj),
                ],
            )
            .map_err(|err| format!("call SecureStoreBridge.delete failed: {err}"))?;
        let err_obj = result
            .l()
            .map_err(|err| format!("SecureStoreBridge.delete decode failed: {err}"))?;
        if err_obj.is_null() {
            return Ok(());
        }
        let err_jstr = JString::from(err_obj);
        let err_msg: String = env
            .get_string(&err_jstr)
            .map_err(|err| format!("read SecureStoreBridge.delete error failed: {err}"))?
            .into();
        Err(format!("android secure delete failed: {err_msg}"))
    })
}

/// 从 Keychain（或兜底存储）读取字节。
fn secure_get(service: &str, account: &str) -> Option<Vec<u8>> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        security_framework::passwords::get_generic_password(service, account).ok()
    }
    #[cfg(target_os = "android")]
    {
        match android_secure_get(service, account) {
            Ok(value) => value,
            Err(err) => {
                eprintln!("[secure_store][android] get failed: {err}");
                None
            }
        }
    }
    #[cfg(all(
        not(any(target_os = "ios", target_os = "macos")),
        not(target_os = "android")
    ))]
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
    #[cfg(target_os = "android")]
    {
        android_secure_set(service, account, value)
    }
    #[cfg(all(
        not(any(target_os = "ios", target_os = "macos")),
        not(target_os = "android")
    ))]
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
    #[cfg(target_os = "android")]
    {
        android_secure_delete(service, account)
    }
    #[cfg(all(
        not(any(target_os = "ios", target_os = "macos")),
        not(target_os = "android")
    ))]
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

/// 聊天存储根目录：`<appData>/chat`。
fn chat_store_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("resolve app data dir failed: {err}"))?;
    let root = app_data_dir.join("chat");
    fs::create_dir_all(root.join("conversations"))
        .map_err(|err| format!("create chat dirs failed: {err}"))?;
    Ok(root)
}

/// 聊天索引文件路径。
fn chat_index_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(chat_store_root(app)?.join("index.json"))
}

/// 会话文件名（对 key 做哈希，避免路径注入与跨平台非法字符）。
fn conversation_file_name(conversation_key: &str) -> String {
    let digest = Sha256::digest(conversation_key.as_bytes());
    format!("conv_{}.jsonl", URL_SAFE_NO_PAD.encode(&digest[..18]))
}

/// 会话 JSONL 文件路径。
fn conversation_path(app: &tauri::AppHandle, conversation_key: &str) -> Result<PathBuf, String> {
    let normalized = conversation_key.trim();
    if normalized.is_empty() {
        return Err("conversationKey 不能为空".to_string());
    }
    Ok(chat_store_root(app)?
        .join("conversations")
        .join(conversation_file_name(normalized)))
}

/// 读取索引（不存在时返回空对象）。
fn read_chat_index(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    let index_path = chat_index_path(app)?;
    let bytes = match fs::read(index_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(serde_json::json!({})),
        Err(err) => return Err(format!("read chat index failed: {err}")),
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|err| format!("decode chat index failed: {err}"))
}

/// 启动时读取聊天索引。
#[tauri::command]
fn chat_store_bootstrap(app: tauri::AppHandle) -> Result<ChatStoreBootstrap, String> {
    Ok(ChatStoreBootstrap {
        index: read_chat_index(&app)?,
    })
}

/// 追加写入会话事件（JSONL）。
#[tauri::command]
fn chat_store_append_events(
    app: tauri::AppHandle,
    conversation_key: String,
    events: Vec<serde_json::Value>,
) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }

    let conv_path = conversation_path(&app, &conversation_key)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(conv_path)
        .map_err(|err| format!("open chat conversation failed: {err}"))?;

    for item in events {
        let line =
            serde_json::to_string(&item).map_err(|err| format!("encode chat event failed: {err}"))?;
        file.write_all(line.as_bytes())
            .map_err(|err| format!("write chat event failed: {err}"))?;
        file.write_all(b"\n")
            .map_err(|err| format!("write chat newline failed: {err}"))?;
    }
    Ok(())
}

/// 读取指定会话事件（支持可选倒序截断）。
#[tauri::command]
fn chat_store_load_conversation(
    app: tauri::AppHandle,
    conversation_key: String,
    limit: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let conv_path = conversation_path(&app, &conversation_key)?;
    let file = match fs::File::open(conv_path) {
        Ok(handle) => handle,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("open chat conversation failed: {err}")),
    };
    let reader = BufReader::new(file);
    let mut rows = Vec::new();
    for line in reader.lines() {
        let raw = line.map_err(|err| format!("read chat line failed: {err}"))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        rows.push(value);
    }

    if let Some(max_rows) = limit {
        if rows.len() > max_rows {
            let split = rows.len() - max_rows;
            rows = rows.split_off(split);
        }
    }
    Ok(rows)
}

/// 幂等覆盖聊天索引文件。
#[tauri::command]
fn chat_store_upsert_index(app: tauri::AppHandle, index: serde_json::Value) -> Result<(), String> {
    let index_path = chat_index_path(&app)?;
    let bytes =
        serde_json::to_vec_pretty(&index).map_err(|err| format!("encode chat index failed: {err}"))?;
    fs::write(index_path, bytes).map_err(|err| format!("write chat index failed: {err}"))
}

/// 删除指定会话的本地存储（索引 + JSONL）。
#[tauri::command]
fn chat_store_delete_conversation(
    app: tauri::AppHandle,
    conversation_key: String,
) -> Result<(), String> {
    let normalized_key = conversation_key.trim();
    if normalized_key.is_empty() {
        return Err("conversationKey 不能为空".to_string());
    }

    let mut index = read_chat_index(&app)?;
    let Some(index_obj) = index.as_object_mut() else {
        index = serde_json::json!({});
        chat_store_upsert_index(app.clone(), index)?;
        let conv_path = conversation_path(&app, normalized_key)?;
        return match fs::remove_file(conv_path) {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("delete chat conversation failed: {err}")),
        };
    };

    if let Some(by_key) = index_obj
        .get_mut("conversationsByKey")
        .and_then(|value| value.as_object_mut())
    {
        by_key.remove(normalized_key);
    }

    if let Some(order) = index_obj
        .get_mut("conversationOrder")
        .and_then(|value| value.as_array_mut())
    {
        order.retain(|item| item.as_str() != Some(normalized_key));
    }

    let active_key = index_obj
        .get("activeConversationKey")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if active_key == normalized_key {
        let next_active = index_obj
            .get("conversationOrder")
            .and_then(|value| value.as_array())
            .and_then(|order| order.first())
            .and_then(|item| item.as_str())
            .unwrap_or("");
        index_obj.insert(
            "activeConversationKey".to_string(),
            serde_json::Value::String(next_active.to_string()),
        );
    }

    chat_store_upsert_index(app.clone(), index)?;
    let conv_path = conversation_path(&app, normalized_key)?;
    match fs::remove_file(conv_path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("delete chat conversation failed: {err}")),
    }
}

/// 将系统深链事件透传给 WebView。前端通过 `window.__YC_HANDLE_PAIR_LINK__` 接收并解析。
#[cfg(any(target_os = "ios", target_os = "macos"))]
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
            chat_store_bootstrap,
            chat_store_append_events,
            chat_store_load_conversation,
            chat_store_upsert_index,
            chat_store_delete_conversation,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build mobile tauri app")
        .run(|_app, _event| {
            #[cfg(any(target_os = "ios", target_os = "macos"))]
            if let RunEvent::Opened { urls } = _event {
                for url in urls {
                    if url.scheme() == "yc" && url.host_str() == Some("pair") {
                        forward_pairing_link(_app, url.as_str());
                    }
                }
            }
        });
}
