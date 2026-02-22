//! pairTicket 校验逻辑。

use axum::http::StatusCode;

use crate::{
    api::{error::ApiError, types::PairAuthMode},
    pairing::ticket::{pair_ticket_error_to_api, verify_pairing_ticket},
    state::AppState,
};

impl AppState {
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
}
