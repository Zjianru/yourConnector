//! 配对换发设备凭证逻辑。

use axum::http::StatusCode;

use crate::{
    api::{
        error::ApiError,
        types::{PairExchangeData, PairExchangeRequest},
    },
    auth::{
        pop::pair_exchange_payload,
        store::persist_auth_store,
        token::{
            issue_access_token, issue_refresh_session, key_id_for_public_key, verify_pop_signature,
        },
    },
    state::AppState,
};

impl AppState {
    /// 配对换发设备凭证（消费票据）。
    pub(crate) async fn exchange_device_credential(
        &self,
        req: &PairExchangeRequest,
    ) -> Result<PairExchangeData, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        let key_id = req.key_id.trim();
        let pubkey = req.device_pub_key.trim();
        let proof = req.proof.trim();

        if system_id.is_empty()
            || device_id.is_empty()
            || key_id.is_empty()
            || pubkey.is_empty()
            || proof.is_empty()
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "换发参数不完整",
                "请重新扫码或重新粘贴配对链接",
            ));
        }

        let pair_token = req.pair_token.as_deref().unwrap_or_default().trim();
        if !pair_token.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "PAIR_TOKEN_NOT_SUPPORTED",
                "App 配对接口已不支持 pairToken",
                "请改用 sid + pairTicket（扫码或配对链接）",
            ));
        }
        let pair_ticket = req.pair_ticket.as_deref().unwrap_or_default().trim();
        // 票据在这里做一次性校验并消费，阻断重复使用。
        let auth_mode = self
            .verify_pair_ticket(system_id, pair_ticket, true)
            .await?;

        let expected_payload = pair_exchange_payload(system_id, device_id, key_id);
        verify_pop_signature(pubkey, &expected_payload, proof)?;

        let expected_key_id = key_id_for_public_key(pubkey)?;
        // keyId 必须与公钥可推导结果一致，防止伪造 keyId 越权换发。
        if expected_key_id != key_id {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "keyId 与设备公钥不匹配",
                "请重新生成设备绑定信息后重试",
            ));
        }

        let mut store = self.auth_store.write().await;
        let signing_key = store.signing_key.clone();
        let system = store.system_mut(system_id);

        let now_text = yc_shared_protocol::now_rfc3339_nanos();
        let device_name = normalize_device_name(&req.device_name, device_id);
        let credential_id = format!("crd_{}", uuid::Uuid::new_v4().simple());

        system.devices.insert(
            device_id.to_string(),
            crate::api::types::DeviceCredential {
                device_id: device_id.to_string(),
                device_name,
                key_id: key_id.to_string(),
                public_key: pubkey.to_string(),
                status: "ACTIVE".to_string(),
                created_at: now_text.clone(),
                last_seen_at: now_text,
                revoked_at: None,
            },
        );

        let access_token = issue_access_token(
            &signing_key,
            system_id,
            device_id,
            key_id,
            crate::api::types::ACCESS_TOKEN_TTL_SEC,
        )?;
        let (refresh_token, refresh_session) =
            issue_refresh_session(system_id, device_id, key_id, &credential_id);
        system
            .refresh_sessions
            .insert(refresh_session.session_id.clone(), refresh_session);

        persist_auth_store(&self.auth_store_path, &store).map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                err,
                "请稍后重试",
            )
        })?;

        Ok(PairExchangeData {
            auth_mode,
            access_token,
            refresh_token,
            key_id: key_id.to_string(),
            credential_id,
            access_expires_in_sec: crate::api::types::ACCESS_TOKEN_TTL_SEC,
            refresh_expires_in_sec: crate::api::types::REFRESH_TOKEN_TTL_SEC,
        })
    }
}

/// 归一化设备名称。
fn normalize_device_name(raw: &str, fallback: &str) -> String {
    let normalized = raw.trim();
    if normalized.is_empty() {
        return fallback.to_string();
    }
    normalized.chars().take(64).collect()
}
