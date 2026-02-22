//! WebSocket 连接鉴权逻辑。

use axum::http::StatusCode;

use crate::{
    api::{error::ApiError, types::WsQuery},
    auth::{
        pop::{parse_ts, verify_ts_window, ws_pop_payload},
        token::{authorize_pair_token, verify_access_token, verify_pop_signature},
    },
    state::{AppState, SystemRoom},
};

impl AppState {
    /// 连接鉴权入口：sidecar 走 pairToken；app 仅允许 accessToken + PoP。
    pub(crate) async fn authorize_connection(&self, q: &WsQuery) -> Result<(), ApiError> {
        if q.client_type == "sidecar" {
            if q.pair_token.trim().is_empty() {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "sidecar 缺少 pairToken",
                    "请重启 sidecar 并检查配置",
                ));
            }
            return self.authorize_sidecar(q).await;
        }

        if let Some(access_token) = q.access_token.as_deref().map(str::trim)
            && !access_token.is_empty()
        {
            return self.authorize_app_with_access(q).await;
        }

        let has_legacy_pair = !q.pair_token.trim().is_empty()
            || q.pair_ticket
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .is_some();
        // App 链路禁止继续使用临时票据/配对码直接连 WS，必须先完成凭证换发。
        if has_legacy_pair {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "PAIR_TOKEN_NOT_SUPPORTED",
                "App 连接已不支持 pairToken/pairTicket，请先完成设备凭证换发",
                "请重新扫码配对后再连接",
            ));
        }

        Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "MISSING_CREDENTIALS",
            "缺少 accessToken",
            "请重新扫码配对后再连接",
        ))
    }

    /// sidecar 鉴权并建房。
    async fn authorize_sidecar(&self, q: &WsQuery) -> Result<(), ApiError> {
        let incoming_pair_token = q.pair_token.trim();
        let mut guard = self.systems.write().await;
        let Some(room) = guard.get_mut(&q.system_id) else {
            guard.insert(
                q.system_id.clone(),
                SystemRoom {
                    pair_token: incoming_pair_token.to_string(),
                    ticket_nonces: std::collections::HashMap::new(),
                    app_nonces: std::collections::HashMap::new(),
                    clients: std::collections::HashMap::new(),
                },
            );
            self.persist_pair_token_meta(&q.system_id, incoming_pair_token)
                .await;
            return Ok(());
        };

        let sidecar_clients = room
            .clients
            .values()
            .filter(|client| client.client_type == "sidecar")
            .count();
        match authorize_pair_token(
            Some(room.pair_token.as_str()),
            sidecar_clients,
            &q.client_type,
            incoming_pair_token,
        )
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_TOKEN_MISMATCH",
                "pairToken 不匹配",
                "请重新生成配对信息后再试",
            )
        })? {
            crate::api::types::PairTokenAuthDecision::Allow => Ok(()),
            crate::api::types::PairTokenAuthDecision::Rotate => {
                room.pair_token = incoming_pair_token.to_string();
                drop(guard);
                self.persist_pair_token_meta(&q.system_id, incoming_pair_token)
                    .await;
                Ok(())
            }
            crate::api::types::PairTokenAuthDecision::Initialize => Ok(()),
        }
    }

    /// app 使用 access token + PoP 的生产鉴权。
    async fn authorize_app_with_access(&self, q: &WsQuery) -> Result<(), ApiError> {
        let access_token = q
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "缺少 accessToken",
                    "请重新配对并连接",
                )
            })?;

        let key_id = q
            .key_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "缺少 keyId",
                    "请重新配对并连接",
                )
            })?;

        let ts = parse_ts(
            q.ts.as_deref().unwrap_or_default(),
            "ACCESS_SIGNATURE_EXPIRED",
            "签名时间戳无效",
        )?;
        verify_ts_window(ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间窗已过期")?;

        let nonce = q
            .nonce
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "缺少 nonce",
                    "请重新连接",
                )
            })?;
        let sig = q
            .sig
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "缺少签名",
                    "请重新连接",
                )
            })?;

        let payload = ws_pop_payload(&q.system_id, &q.device_id, key_id, ts, nonce);
        let device = {
            let guard = self.auth_store.read().await;
            verify_access_token(
                access_token,
                &guard.signing_key,
                &q.system_id,
                &q.device_id,
                key_id,
            )?;

            let Some(system) = guard.system_ref(&q.system_id) else {
                return Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "SYSTEM_NOT_REGISTERED",
                    "system 未注册，请先启动 sidecar",
                    "先启动宿主机 sidecar 再配对",
                ));
            };
            let Some(device) = system.devices.get(&q.device_id) else {
                return Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "DEVICE_REVOKED",
                    "设备未绑定或已被移除",
                    "请重新扫码配对",
                ));
            };
            if device.status != "ACTIVE" {
                return Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "DEVICE_REVOKED",
                    "设备已被吊销",
                    "请重新扫码配对",
                ));
            }
            verify_pop_signature(&device.public_key, &payload, sig)?;
            device.clone()
        };

        let mut guard = self.systems.write().await;
        let Some(room) = guard.get_mut(&q.system_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "宿主机未在线",
                "请先启动 sidecar",
            ));
        };
        if !room.has_online_sidecar() {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "宿主机 sidecar 未在线",
                "请先启动 sidecar",
            ));
        }

        let now = crate::auth::store::unix_now();
        // nonce 以短窗口缓存，过期即清理，既防重放又避免内存常驻增长。
        room.app_nonces.retain(|_, exp| exp.saturating_add(5) > now);
        if let Some(exp) = room.app_nonces.get(nonce)
            && *exp > now
        {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "ACCESS_SIGNATURE_REPLAYED",
                "签名请求已使用，请重试",
                "请重新发起连接",
            ));
        }
        room.app_nonces.insert(
            nonce.to_string(),
            now.saturating_add(crate::api::types::POP_MAX_SKEW_SEC),
        );

        drop(guard);
        self.touch_device_last_seen(&q.system_id, &device.device_id)
            .await;
        Ok(())
    }
}
