//! 配对接口处理。

use axum::{Json, extract::State, http::StatusCode};

use crate::{
    api::{
        error::ApiError,
        response::{ApiEnvelope, ok_response},
        types::{
            PairAuthMode, PairBootstrapData, PairBootstrapRequest, PairExchangeData,
            PairExchangeRequest, PairPreflightData, PairPreflightRequest,
        },
    },
    auth::{
        pop::pair_exchange_payload,
        store::persist_auth_store,
        token::{
            issue_access_token, issue_refresh_session, key_id_for_public_key, verify_pop_signature,
        },
    },
    pairing::{
        bootstrap::{
            build_pair_bootstrap_data, normalize_host_name, normalize_ttl_sec, relay_public_ws_url,
        },
        ticket::{pair_ticket_error_to_api, verify_pairing_ticket},
    },
    state::AppState,
};

/// 配对预检接口：用于移动端精确映射失败弹窗。
pub(crate) async fn pair_preflight_handler(
    State(state): State<AppState>,
    Json(req): Json<PairPreflightRequest>,
) -> (StatusCode, Json<ApiEnvelope<PairPreflightData>>) {
    match state.preflight_pair_credentials(&req).await {
        Ok(mode) => ok_response(
            StatusCode::OK,
            "配对信息可用",
            "可以继续执行配对并连接",
            Some(PairPreflightData { auth_mode: mode }),
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

/// 配对换发接口：绑定设备公钥并签发 access/refresh。
pub(crate) async fn pair_exchange_handler(
    State(state): State<AppState>,
    Json(req): Json<PairExchangeRequest>,
) -> (StatusCode, Json<ApiEnvelope<PairExchangeData>>) {
    let result = state.exchange_device_credential(&req).await;
    match result {
        Ok(data) => ok_response(
            StatusCode::OK,
            "设备凭证换发成功",
            "后续连接将使用设备凭证",
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

/// 配对签发接口：统一 sidecar 与脚本的配对链接来源。
pub(crate) async fn pair_bootstrap_handler(
    State(state): State<AppState>,
    Json(req): Json<PairBootstrapRequest>,
) -> (StatusCode, Json<ApiEnvelope<PairBootstrapData>>) {
    match state.issue_pair_bootstrap(&req).await {
        Ok(data) => ok_response(
            StatusCode::OK,
            "配对信息已签发",
            "请使用配对链接或二维码完成接入",
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
    /// 配对预检（不消费票据）。
    pub(crate) async fn preflight_pair_credentials(
        &self,
        req: &PairPreflightRequest,
    ) -> Result<PairAuthMode, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        if system_id.is_empty() || device_id.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "systemId/deviceId 不能为空",
                "请检查配对信息",
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
        self.verify_pair_ticket(system_id, pair_ticket, false).await
    }

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
        let auth_mode = self
            .verify_pair_ticket(system_id, pair_ticket, true)
            .await?;

        let expected_payload = pair_exchange_payload(system_id, device_id, key_id);
        verify_pop_signature(pubkey, &expected_payload, proof)?;

        let expected_key_id = key_id_for_public_key(pubkey)?;
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

    /// pairTicket 凭证校验（仅支持短时票据）。
    pub(crate) async fn verify_pair_ticket(
        &self,
        system_id: &str,
        pair_ticket: &str,
        consume_ticket: bool,
    ) -> Result<PairAuthMode, ApiError> {
        let mut guard = self.systems.write().await;
        let Some(room) = guard.get_mut(system_id) else {
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

        if pair_ticket.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "缺少 pairTicket",
                "请重新扫码或重新导入配对链接",
            ));
        }

        match verify_pairing_ticket(
            pair_ticket,
            system_id,
            &room.pair_token,
            &mut room.ticket_nonces,
            consume_ticket,
        ) {
            Ok(_) => Ok(PairAuthMode::PairTicket),
            Err(err) => Err(pair_ticket_error_to_api(err)),
        }
    }

    /// 统一签发配对链接数据。
    pub(crate) async fn issue_pair_bootstrap(
        &self,
        req: &PairBootstrapRequest,
    ) -> Result<PairBootstrapData, ApiError> {
        let system_id = req.system_id.trim();
        let pair_token = req.pair_token.trim();
        if system_id.is_empty() || pair_token.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "systemId/pairToken 不能为空",
                "请检查输入后重试",
            ));
        }

        let guard = self.systems.read().await;
        let Some(room) = guard.get(system_id) else {
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
        if room.pair_token != pair_token {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_TOKEN_MISMATCH",
                "pairToken 不匹配",
                "请使用最新配对信息",
            ));
        }

        let relay_ws_url = req
            .relay_ws_url
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(relay_public_ws_url);
        let host_name = normalize_host_name(req.host_name.as_deref(), system_id);
        let ttl_sec = normalize_ttl_sec(req.ttl_sec);
        let include_code = req.include_code.unwrap_or(true);

        Ok(build_pair_bootstrap_data(
            &relay_ws_url,
            system_id,
            pair_token,
            &host_name,
            include_code,
            ttl_sec,
        ))
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
