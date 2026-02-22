//! PoP 负载与时间窗校验。

use axum::http::StatusCode;

use crate::{
    api::{error::ApiError, types::POP_MAX_SKEW_SEC},
    auth::store::unix_now,
};

/// 解析秒级时间戳。
pub(crate) fn parse_ts(
    raw: &str,
    code: &'static str,
    message: &'static str,
) -> Result<u64, ApiError> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, code, message, "请刷新后重试"))
}

/// 校验时间窗。
pub(crate) fn verify_ts_window(
    ts: u64,
    code: &'static str,
    message: &'static str,
) -> Result<(), ApiError> {
    let now = unix_now();
    if ts.saturating_add(POP_MAX_SKEW_SEC) < now || ts > now.saturating_add(POP_MAX_SKEW_SEC) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            code,
            message,
            "请重新发起请求",
        ));
    }
    Ok(())
}

/// 组装 WS PoP 签名 payload。
pub(crate) fn ws_pop_payload(
    system_id: &str,
    device_id: &str,
    key_id: &str,
    ts: u64,
    nonce: &str,
) -> String {
    format!("ws\n{system_id}\n{device_id}\n{key_id}\n{ts}\n{nonce}")
}

/// 组装 exchange proof payload。
pub(crate) fn pair_exchange_payload(system_id: &str, device_id: &str, key_id: &str) -> String {
    format!("pair-exchange\n{system_id}\n{device_id}\n{key_id}")
}

/// 组装 refresh 签名 payload。
pub(crate) fn auth_refresh_payload(
    system_id: &str,
    device_id: &str,
    key_id: &str,
    ts: u64,
    nonce: &str,
) -> String {
    format!("auth-refresh\n{system_id}\n{device_id}\n{key_id}\n{ts}\n{nonce}")
}

/// 组装 revoke 签名 payload。
pub(crate) fn auth_revoke_payload(
    system_id: &str,
    device_id: &str,
    target_device_id: &str,
    key_id: &str,
    ts: u64,
    nonce: &str,
) -> String {
    format!("auth-revoke\n{system_id}\n{device_id}\n{target_device_id}\n{key_id}\n{ts}\n{nonce}")
}

/// 组装 list-devices 签名 payload。
pub(crate) fn auth_list_payload(
    system_id: &str,
    device_id: &str,
    key_id: &str,
    ts: u64,
    nonce: &str,
) -> String {
    format!("auth-list-devices\n{system_id}\n{device_id}\n{key_id}\n{ts}\n{nonce}")
}

#[cfg(test)]
mod tests {
    use super::{
        auth_list_payload, auth_refresh_payload, auth_revoke_payload, pair_exchange_payload,
        ws_pop_payload,
    };

    #[test]
    fn pop_payloads_use_real_newline_separator() {
        let ws = ws_pop_payload("sid", "did", "kid", 123, "nonce");
        let exchange = pair_exchange_payload("sid", "did", "kid");
        let refresh = auth_refresh_payload("sid", "did", "kid", 123, "nonce");
        let revoke = auth_revoke_payload("sid", "did", "target", "kid", 123, "nonce");
        let list = auth_list_payload("sid", "did", "kid", 123, "nonce");

        for payload in [ws, exchange, refresh, revoke, list] {
            assert!(payload.contains('\n'));
            assert!(!payload.contains("\\n"));
        }
    }
}
