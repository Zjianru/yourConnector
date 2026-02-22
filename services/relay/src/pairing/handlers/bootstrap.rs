//! 配对链接签发逻辑。

use axum::http::StatusCode;

use crate::{
    api::{
        error::ApiError,
        types::{PairBootstrapData, PairBootstrapRequest},
    },
    pairing::bootstrap::{
        build_pair_bootstrap_data, normalize_host_name, normalize_ttl_sec, relay_public_ws_url,
    },
    state::AppState,
};

impl AppState {
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
