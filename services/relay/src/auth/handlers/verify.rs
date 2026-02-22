//! Access token + PoP 校验逻辑。

use axum::http::StatusCode;

use crate::{
    api::error::ApiError,
    auth::token::{verify_access_token, verify_pop_signature},
    state::AppState,
};

impl AppState {
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
