//! 鉴权 HTTP 接口处理。

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};

use crate::{
    api::{
        error::ApiError,
        response::{ApiEnvelope, ok_response},
        types::{
            AuthDevicesData, AuthDevicesQuery, AuthRefreshData, AuthRefreshRequest,
            AuthRevokeDeviceData, AuthRevokeDeviceRequest,
        },
    },
    auth::{
        pop::{
            auth_list_payload, auth_refresh_payload, auth_revoke_payload, parse_ts,
            verify_ts_window,
        },
        store::persist_auth_store,
        token::{
            issue_access_token, issue_refresh_session, parse_refresh_token, sha256_hex,
            verify_access_token, verify_pop_signature,
        },
    },
    state::AppState,
};

/// 刷新接口：轮换 refresh 并颁发新 access。
pub(crate) async fn auth_refresh_handler(
    State(state): State<AppState>,
    Json(req): Json<AuthRefreshRequest>,
) -> (StatusCode, Json<ApiEnvelope<AuthRefreshData>>) {
    match state.refresh_device_credential(&req).await {
        Ok(data) => ok_response(
            StatusCode::OK,
            "凭证刷新成功",
            "继续使用当前会话",
            Some(data),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// 吊销设备接口。
pub(crate) async fn auth_revoke_device_handler(
    State(state): State<AppState>,
    Json(req): Json<AuthRevokeDeviceRequest>,
) -> (StatusCode, Json<ApiEnvelope<AuthRevokeDeviceData>>) {
    match state.revoke_device(&req).await {
        Ok(data) => ok_response(
            StatusCode::OK,
            "设备已吊销",
            "被吊销设备需重新配对",
            Some(data),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// 设备列表接口。
pub(crate) async fn auth_devices_handler(
    State(state): State<AppState>,
    Query(query): Query<AuthDevicesQuery>,
) -> (StatusCode, Json<ApiEnvelope<AuthDevicesData>>) {
    match state.list_devices(&query).await {
        Ok(data) => ok_response(
            StatusCode::OK,
            "设备列表获取成功",
            "可以对设备进行管理",
            Some(data),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

impl AppState {
    /// 刷新设备凭证（轮换 refresh）。
    pub(crate) async fn refresh_device_credential(
        &self,
        req: &AuthRefreshRequest,
    ) -> Result<AuthRefreshData, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        let key_id = req.key_id.trim();
        if system_id.is_empty() || device_id.is_empty() || key_id.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "刷新参数不完整",
                "请重新登录设备",
            ));
        }

        let ts = parse_ts(&req.ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间戳无效")?;
        verify_ts_window(ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间窗已过期")?;
        self.consume_auth_nonce("refresh", &req.nonce, ts).await?;

        let payload = auth_refresh_payload(system_id, device_id, key_id, ts, &req.nonce);
        let (session_id, refresh_secret) = parse_refresh_token(&req.refresh_token)?;
        let mut store = self.auth_store.write().await;
        let signing_key = store.signing_key.clone();
        let Some(system) = store.systems.get_mut(system_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "system 未注册",
                "请重新配对",
            ));
        };

        let Some(old_session) = system.refresh_sessions.get_mut(&session_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_INVALID",
                "refreshToken 无效",
                "请重新配对",
            ));
        };

        if old_session.revoked_at.is_some() {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_INVALID",
                "refreshToken 已失效",
                "请重新配对",
            ));
        }
        if old_session.expires_at <= crate::auth::store::unix_now() {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_EXPIRED",
                "refreshToken 已过期",
                "请重新配对",
            ));
        }

        let hash = sha256_hex(&refresh_secret);
        if hash != old_session.refresh_secret_hash {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_INVALID",
                "refreshToken 校验失败",
                "请重新配对",
            ));
        }

        if old_session.device_id != device_id || old_session.key_id != key_id {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_INVALID",
                "refreshToken 与设备不匹配",
                "请重新配对",
            ));
        }
        let Some(device) = system.devices.get(device_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "DEVICE_REVOKED",
                "设备未绑定",
                "请重新配对",
            ));
        };
        if device.status != "ACTIVE" {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "DEVICE_REVOKED",
                "设备已吊销",
                "请重新配对",
            ));
        }
        verify_pop_signature(&device.public_key, &payload, &req.sig)?;

        old_session.revoked_at = Some(yc_shared_protocol::now_rfc3339_nanos());
        let credential_id = old_session.credential_id.clone();
        let rotated_from = Some(old_session.session_id.clone());

        let access_token = issue_access_token(
            &signing_key,
            system_id,
            device_id,
            key_id,
            crate::api::types::ACCESS_TOKEN_TTL_SEC,
        )?;
        let (refresh_token, mut new_session) =
            issue_refresh_session(system_id, device_id, key_id, &credential_id);
        new_session.rotated_from = rotated_from;
        system
            .refresh_sessions
            .insert(new_session.session_id.clone(), new_session);

        persist_auth_store(&self.auth_store_path, &store).map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                err,
                "请稍后重试",
            )
        })?;

        Ok(AuthRefreshData {
            access_token,
            refresh_token,
            key_id: key_id.to_string(),
            credential_id,
            access_expires_in_sec: crate::api::types::ACCESS_TOKEN_TTL_SEC,
            refresh_expires_in_sec: crate::api::types::REFRESH_TOKEN_TTL_SEC,
        })
    }

    /// 吊销指定设备。
    pub(crate) async fn revoke_device(
        &self,
        req: &AuthRevokeDeviceRequest,
    ) -> Result<AuthRevokeDeviceData, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        let key_id = req.key_id.trim();
        let target_device_id = req.target_device_id.trim();
        if system_id.is_empty()
            || device_id.is_empty()
            || key_id.is_empty()
            || target_device_id.is_empty()
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "吊销参数不完整",
                "请检查输入后重试",
            ));
        }

        let ts = parse_ts(&req.ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间戳无效")?;
        verify_ts_window(ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间窗已过期")?;
        self.consume_auth_nonce("revoke", &req.nonce, ts).await?;

        let payload = auth_revoke_payload(
            system_id,
            device_id,
            target_device_id,
            key_id,
            ts,
            &req.nonce,
        );
        self.verify_access_http(
            system_id,
            device_id,
            key_id,
            &req.access_token,
            &payload,
            &req.sig,
        )
        .await?;

        let mut store = self.auth_store.write().await;
        let Some(system) = store.systems.get_mut(system_id) else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "SYSTEM_NOT_REGISTERED",
                "system 不存在",
                "请先完成配对",
            ));
        };

        let Some(target) = system.devices.get_mut(target_device_id) else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "DEVICE_NOT_FOUND",
                "目标设备不存在",
                "请刷新后重试",
            ));
        };

        target.status = "REVOKED".to_string();
        target.revoked_at = Some(yc_shared_protocol::now_rfc3339_nanos());
        for session in system.refresh_sessions.values_mut() {
            if session.device_id == target_device_id {
                session.revoked_at = Some(yc_shared_protocol::now_rfc3339_nanos());
            }
        }

        persist_auth_store(&self.auth_store_path, &store).map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                err,
                "请稍后重试",
            )
        })?;

        Ok(AuthRevokeDeviceData {
            target_device_id: target_device_id.to_string(),
        })
    }

    /// 查询设备列表。
    pub(crate) async fn list_devices(
        &self,
        req: &AuthDevicesQuery,
    ) -> Result<AuthDevicesData, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        let key_id = req.key_id.trim();
        if system_id.is_empty() || device_id.is_empty() || key_id.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "设备列表参数不完整",
                "请检查后重试",
            ));
        }

        let ts = parse_ts(&req.ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间戳无效")?;
        verify_ts_window(ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间窗已过期")?;
        self.consume_auth_nonce("devices", &req.nonce, ts).await?;

        let payload = auth_list_payload(system_id, device_id, key_id, ts, &req.nonce);
        self.verify_access_http(
            system_id,
            device_id,
            key_id,
            &req.access_token,
            &payload,
            &req.sig,
        )
        .await?;

        let store = self.auth_store.read().await;
        let Some(system) = store.system_ref(system_id) else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "SYSTEM_NOT_REGISTERED",
                "system 不存在",
                "请重新配对",
            ));
        };

        let mut devices = system
            .devices
            .values()
            .cloned()
            .map(|item| crate::api::types::DeviceEntry {
                device_id: item.device_id,
                device_name: item.device_name,
                key_id: item.key_id,
                status: item.status,
                created_at: item.created_at,
                last_seen_at: item.last_seen_at,
                revoked_at: item.revoked_at,
            })
            .collect::<Vec<_>>();
        devices.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        Ok(AuthDevicesData { devices })
    }

    /// HTTP 鉴权：access token + PoP。
    pub(crate) async fn verify_access_http(
        &self,
        system_id: &str,
        device_id: &str,
        key_id: &str,
        access_token: &str,
        payload: &str,
        sig: &str,
    ) -> Result<(), ApiError> {
        let store = self.auth_store.read().await;
        verify_access_token(
            access_token,
            &store.signing_key,
            system_id,
            device_id,
            key_id,
        )?;

        let Some(system) = store.system_ref(system_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "system 未注册",
                "请先启动 sidecar",
            ));
        };
        let Some(device) = system.devices.get(device_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "DEVICE_REVOKED",
                "设备未绑定",
                "请重新配对",
            ));
        };
        if device.status != "ACTIVE" {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "DEVICE_REVOKED",
                "设备已吊销",
                "请重新配对",
            ));
        }
        verify_pop_signature(&device.public_key, payload, sig)
    }
}
