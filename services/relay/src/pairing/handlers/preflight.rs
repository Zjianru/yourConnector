//! 配对预检逻辑。

use axum::http::StatusCode;

use crate::{
    api::{
        error::ApiError,
        types::{PairAuthMode, PairPreflightRequest},
    },
    state::AppState,
};

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
}
