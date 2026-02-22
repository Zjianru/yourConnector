//! 配对票据生成与校验。

use std::collections::HashMap;

use axum::http::StatusCode;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

use crate::{
    api::{
        error::ApiError,
        types::{PairTicketClaims, PairTicketError},
    },
    auth::store::unix_now,
};

/// 生成短时配对票据（`pct_v1.<payload_b64url>.<sig_b64url>`）。
pub(crate) fn generate_pairing_ticket(system_id: &str, pair_token: &str, ttl_sec: u64) -> String {
    let now = unix_now();
    let exp = now.saturating_add(ttl_sec);
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let payload = json!({
        "sid": system_id,
        "iat": now,
        "exp": exp,
        "nonce": nonce
    });
    let payload_raw =
        serde_json::to_string(&payload).expect("pair ticket payload must be serializable");
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload_raw.as_bytes());

    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(pair_token.as_bytes()).expect("hmac key should be valid");
    mac.update(payload_b64.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    format!("pct_v1.{payload_b64}.{sig_b64}")
}

/// 校验短时配对票据。
pub(crate) fn verify_pairing_ticket(
    ticket: &str,
    expected_system_id: &str,
    pair_token: &str,
    used_nonces: &mut HashMap<String, u64>,
    consume: bool,
) -> Result<(), PairTicketError> {
    if ticket.is_empty() {
        return Err(PairTicketError::Empty);
    }

    let mut parts = ticket.split('.');
    let version = parts.next().unwrap_or_default();
    let payload_b64 = parts.next().unwrap_or_default();
    let sig_b64 = parts.next().unwrap_or_default();
    if version != "pct_v1" || payload_b64.is_empty() || sig_b64.is_empty() || parts.next().is_some()
    {
        return Err(PairTicketError::Format);
    }

    let sig = URL_SAFE_NO_PAD
        .decode(sig_b64.as_bytes())
        .map_err(|_| PairTicketError::SignatureFormat)?;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(pair_token.as_bytes())
        .map_err(|_| PairTicketError::SignatureVerify)?;
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&sig)
        .map_err(|_| PairTicketError::SignatureVerify)?;

    let payload_raw = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|_| PairTicketError::Payload)?;
    let claims: PairTicketClaims =
        serde_json::from_slice(&payload_raw).map_err(|_| PairTicketError::Claims)?;

    if claims.sid != expected_system_id {
        return Err(PairTicketError::SystemMismatch);
    }
    if claims.nonce.trim().is_empty() {
        return Err(PairTicketError::EmptyNonce);
    }

    let now = unix_now();
    if claims.exp <= now {
        return Err(PairTicketError::Expired);
    }
    if claims.iat > now.saturating_add(30) {
        return Err(PairTicketError::IatInvalid);
    }

    used_nonces.retain(|_, exp| exp.saturating_add(30) > now);
    if let Some(exp) = used_nonces.get(&claims.nonce)
        && *exp > now
    {
        return Err(PairTicketError::Replay);
    }

    if consume {
        used_nonces.insert(claims.nonce, claims.exp);
    }

    Ok(())
}

/// pairTicket 错误映射到 API 错误。
pub(crate) fn pair_ticket_error_to_api(err: PairTicketError) -> ApiError {
    match err {
        PairTicketError::Empty => ApiError::new(
            StatusCode::BAD_REQUEST,
            "MISSING_CREDENTIALS",
            "缺少配对票据",
            "请重新扫码",
        ),
        PairTicketError::Format
        | PairTicketError::SignatureFormat
        | PairTicketError::SignatureVerify
        | PairTicketError::Payload
        | PairTicketError::Claims
        | PairTicketError::SystemMismatch
        | PairTicketError::EmptyNonce
        | PairTicketError::IatInvalid => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_TICKET_INVALID",
            "配对票据无效",
            "请重新扫码获取最新配对信息",
        ),
        PairTicketError::Expired => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_TICKET_EXPIRED",
            "配对票据已过期",
            "请重新扫码获取最新二维码",
        ),
        PairTicketError::Replay => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_TICKET_REPLAYED",
            "配对票据已使用",
            "请重新扫码获取最新二维码",
        ),
    }
}
