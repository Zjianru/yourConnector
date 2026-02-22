//! Relay 状态：在线连接房间与认证存储句柄。

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use axum::extract::ws::Message;
use tokio::sync::{RwLock, mpsc};
use tracing::warn;
use uuid::Uuid;

use crate::{
    api::{error::ApiError, types::AuthStore},
    auth::store::{auth_store_path, load_auth_store, persist_auth_store, unix_now},
};

/// Relay 共享状态。
#[derive(Clone)]
pub(crate) struct AppState {
    /// 在线 system 房间（内存）。
    pub(crate) systems: Arc<RwLock<HashMap<String, SystemRoom>>>,
    /// 认证元数据（持久化）。
    pub(crate) auth_store: Arc<RwLock<AuthStore>>,
    /// 认证元数据文件路径。
    pub(crate) auth_store_path: Arc<PathBuf>,
    /// HTTP 鉴权接口 nonce（内存防重放）。
    pub(crate) auth_nonces: Arc<RwLock<HashMap<String, u64>>>,
}

impl Default for AppState {
    fn default() -> Self {
        let path = auth_store_path();
        let store = load_auth_store(&path).unwrap_or_else(|err| {
            warn!("load auth store failed: {err}");
            AuthStore::new(crate::auth::store::generate_signing_key_seed())
        });
        Self {
            systems: Arc::new(RwLock::new(HashMap::new())),
            auth_store: Arc::new(RwLock::new(store)),
            auth_store_path: Arc::new(path),
            auth_nonces: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

/// 单个 system 房间状态。
pub(crate) struct SystemRoom {
    /// 当前 system 配对令牌（sidecar 注册）。
    pub(crate) pair_token: String,
    /// 已使用短时票据 nonce。
    pub(crate) ticket_nonces: HashMap<String, u64>,
    /// WS PoP nonce（app）防重放。
    pub(crate) app_nonces: HashMap<String, u64>,
    /// 当前连接客户端集合。
    pub(crate) clients: HashMap<Uuid, ClientHandle>,
}

impl SystemRoom {
    /// 判断当前房间是否存在在线 sidecar 会话。
    pub(crate) fn has_online_sidecar(&self) -> bool {
        self.clients
            .values()
            .any(|client| client.client_type == "sidecar")
    }
}

/// 单个连接发送句柄。
#[derive(Clone)]
pub(crate) struct ClientHandle {
    /// 连接端类型（`app` / `sidecar`），用于在线 sidecar 判定。
    pub(crate) client_type: String,
    pub(crate) sender: mpsc::UnboundedSender<Message>,
}

impl AppState {
    /// 注册 system 房间连接。
    pub(crate) async fn insert(
        &self,
        system_id: String,
        pair_token: String,
        client_id: Uuid,
        handle: ClientHandle,
    ) {
        let mut guard = self.systems.write().await;
        let room = guard.entry(system_id).or_insert_with(|| SystemRoom {
            pair_token,
            ticket_nonces: HashMap::new(),
            app_nonces: HashMap::new(),
            clients: HashMap::new(),
        });
        room.clients.insert(client_id, handle);
    }

    /// 移除 system 房间连接。
    pub(crate) async fn remove(&self, system_id: &str, client_id: Uuid) {
        let mut guard = self.systems.write().await;
        let mut should_drop_room = false;
        let mut close_senders = Vec::new();
        if let Some(room) = guard.get_mut(system_id) {
            room.clients.remove(&client_id);
            should_drop_room = room.clients.is_empty() || !room.has_online_sidecar();
            if should_drop_room {
                close_senders.extend(room.clients.values().map(|handle| handle.sender.clone()));
            }
        }
        for sender in close_senders {
            let _ = sender.send(Message::Close(None));
        }
        if should_drop_room {
            guard.remove(system_id);
        }
    }

    /// 广播到同 system 其他连接。
    pub(crate) async fn broadcast(&self, system_id: &str, origin_id: Uuid, msg: String) {
        let mut stale = Vec::new();

        {
            let guard = self.systems.read().await;
            if let Some(room) = guard.get(system_id) {
                for (client_id, handle) in &room.clients {
                    if *client_id == origin_id {
                        continue;
                    }
                    if handle
                        .sender
                        .send(Message::Text(msg.clone().into()))
                        .is_err()
                    {
                        stale.push(*client_id);
                    }
                }
            }
        }

        if stale.is_empty() {
            return;
        }

        let mut guard = self.systems.write().await;
        let mut should_drop_room = false;
        let mut close_senders = Vec::new();
        if let Some(room) = guard.get_mut(system_id) {
            for client_id in stale {
                room.clients.remove(&client_id);
            }
            should_drop_room = room.clients.is_empty() || !room.has_online_sidecar();
            if should_drop_room {
                close_senders.extend(room.clients.values().map(|handle| handle.sender.clone()));
            }
        }
        for sender in close_senders {
            let _ = sender.send(Message::Close(None));
        }
        if should_drop_room {
            guard.remove(system_id);
        }
    }

    /// system 连接数快照。
    pub(crate) async fn snapshot(&self) -> HashMap<String, usize> {
        let guard = self.systems.read().await;
        guard
            .iter()
            .map(|(system_id, room)| (system_id.clone(), room.clients.len()))
            .collect()
    }

    /// 记录 pair token 元数据（仅 hash，不存明文）。
    pub(crate) async fn persist_pair_token_meta(&self, system_id: &str, pair_token: &str) {
        let mut store = self.auth_store.write().await;
        let system = store.system_mut(system_id);
        system.pair_token_hash = Some(crate::auth::token::sha256_hex(pair_token));
        system.pair_token_updated_at = Some(yc_shared_protocol::now_rfc3339_nanos());
        if let Err(err) = persist_auth_store(&self.auth_store_path, &store) {
            warn!("persist pair token meta failed: {err}");
        }
    }

    /// 更新设备最后活跃时间。
    pub(crate) async fn touch_device_last_seen(&self, system_id: &str, device_id: &str) {
        let mut store = self.auth_store.write().await;
        let Some(system) = store.systems.get_mut(system_id) else {
            return;
        };
        let Some(device) = system.devices.get_mut(device_id) else {
            return;
        };
        device.last_seen_at = yc_shared_protocol::now_rfc3339_nanos();
        if let Err(err) = persist_auth_store(&self.auth_store_path, &store) {
            warn!("persist device last_seen failed: {err}");
        }
    }

    /// 消费 HTTP nonce（防重放）。
    pub(crate) async fn consume_auth_nonce(
        &self,
        scope: &str,
        nonce: &str,
        ts: u64,
    ) -> Result<(), ApiError> {
        let normalized = nonce.trim();
        if normalized.is_empty() {
            return Err(ApiError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "缺少 nonce",
                "请重试",
            ));
        }
        let now = unix_now();
        if ts > now.saturating_add(crate::api::types::POP_MAX_SKEW_SEC) {
            return Err(ApiError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "ACCESS_SIGNATURE_EXPIRED",
                "签名时间无效",
                "请重新发起请求",
            ));
        }

        let key = format!("{scope}:{normalized}");
        let mut guard = self.auth_nonces.write().await;
        guard.retain(|_, exp| exp.saturating_add(5) > now);
        if let Some(exp) = guard.get(&key)
            && *exp > now
        {
            return Err(ApiError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "ACCESS_SIGNATURE_REPLAYED",
                "签名请求重复",
                "请重新发起请求",
            ));
        }
        guard.insert(key, now.saturating_add(crate::api::types::POP_MAX_SKEW_SEC));
        Ok(())
    }
}
