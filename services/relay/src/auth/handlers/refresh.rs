//! 设备凭证刷新逻辑。

use axum::http::StatusCode;

use crate::{
    api::{
        error::ApiError,
        types::{AuthRefreshData, AuthRefreshRequest},
    },
    auth::{
        pop::{auth_refresh_payload, parse_ts, verify_ts_window},
        store::persist_auth_store,
        token::{
            issue_access_token, issue_refresh_session, parse_refresh_token, sha256_hex,
            verify_pop_signature,
        },
    },
    state::AppState,
};

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
}
