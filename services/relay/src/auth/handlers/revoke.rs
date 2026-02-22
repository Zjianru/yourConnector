//! 设备吊销逻辑。

use axum::http::StatusCode;

use crate::{
    api::{
        error::ApiError,
        types::{AuthRevokeDeviceData, AuthRevokeDeviceRequest},
    },
    auth::{
        pop::{auth_revoke_payload, parse_ts, verify_ts_window},
        store::persist_auth_store,
    },
    state::AppState,
};

impl AppState {
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
}
