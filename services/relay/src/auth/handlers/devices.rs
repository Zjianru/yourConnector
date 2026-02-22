//! 设备列表查询逻辑。

use axum::http::StatusCode;

use crate::{
    api::{
        error::ApiError,
        types::{AuthDevicesData, AuthDevicesQuery, DeviceEntry},
    },
    auth::pop::{auth_list_payload, parse_ts, verify_ts_window},
    state::AppState,
};

impl AppState {
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
            .map(|item| DeviceEntry {
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
}
