//! Token 与签名逻辑。

use axum::http::StatusCode;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    api::{
        error::ApiError,
        types::{AccessTokenClaims, PairTokenAuthDecision, REFRESH_TOKEN_TTL_SEC, RefreshSession},
    },
    auth::store::unix_now,
};

/// pairToken 鉴权决策。
pub(crate) fn authorize_pair_token(
    existing_pair_token: Option<&str>,
    active_client_count: usize,
    client_type: &str,
    incoming_pair_token: &str,
) -> Result<PairTokenAuthDecision, String> {
    if incoming_pair_token.trim().is_empty() {
        return Err("pairToken 不能为空".to_string());
    }

    let Some(existing) = existing_pair_token else {
        if client_type == "sidecar" {
            return Ok(PairTokenAuthDecision::Initialize);
        }
        return Err("system 未注册，请先启动 sidecar 完成配对".to_string());
    };

    if existing == incoming_pair_token {
        return Ok(PairTokenAuthDecision::Allow);
    }

    if client_type == "sidecar" && active_client_count == 0 {
        return Ok(PairTokenAuthDecision::Rotate);
    }

    Err("pairToken 不匹配".to_string())
}

/// 生成 access token。
pub(crate) fn issue_access_token(
    signing_key: &str,
    system_id: &str,
    device_id: &str,
    key_id: &str,
    ttl_sec: u64,
) -> Result<String, ApiError> {
    let now = unix_now();
    let claims = AccessTokenClaims {
        sid: system_id.to_string(),
        did: device_id.to_string(),
        kid: key_id.to_string(),
        iat: now,
        exp: now.saturating_add(ttl_sec),
        jti: Uuid::new_v4().simple().to_string(),
    };
    let payload = serde_json::to_string(&claims).map_err(|err| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            format!("encode access token claims failed: {err}"),
            "请稍后重试",
        )
    })?;
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());
    let sig_b64 = hmac_b64url(signing_key, payload_b64.as_bytes())?;
    Ok(format!("yat_v1.{payload_b64}.{sig_b64}"))
}

/// 校验 access token。
pub(crate) fn verify_access_token(
    token: &str,
    signing_key: &str,
    expected_system: &str,
    expected_device: &str,
    expected_key_id: &str,
) -> Result<AccessTokenClaims, ApiError> {
    let mut parts = token.split('.');
    let version = parts.next().unwrap_or_default();
    let payload_b64 = parts.next().unwrap_or_default();
    let sig_b64 = parts.next().unwrap_or_default();
    if version != "yat_v1" || payload_b64.is_empty() || sig_b64.is_empty() || parts.next().is_some()
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken 格式无效",
            "请重新配对",
        ));
    }

    let sig = URL_SAFE_NO_PAD.decode(sig_b64.as_bytes()).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken 签名格式无效",
            "请重新配对",
        )
    })?;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(signing_key.as_bytes()).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken 签名器无效",
            "请重新配对",
        )
    })?;
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&sig).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken 签名校验失败",
            "请重新配对",
        )
    })?;

    let payload_raw = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "ACCESS_TOKEN_INVALID",
                "accessToken payload 无效",
                "请重新配对",
            )
        })?;
    let claims: AccessTokenClaims = serde_json::from_slice(&payload_raw).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken claims 无效",
            "请重新配对",
        )
    })?;

    if claims.sid != expected_system
        || claims.did != expected_device
        || claims.kid != expected_key_id
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_MISMATCH",
            "accessToken 与当前连接信息不匹配",
            "请重新配对",
        ));
    }

    let now = unix_now();
    if claims.exp <= now {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_EXPIRED",
            "accessToken 已过期",
            "请刷新凭证或重新配对",
        ));
    }

    Ok(claims)
}

/// 生成 refresh 会话。
pub(crate) fn issue_refresh_session(
    system_id: &str,
    device_id: &str,
    key_id: &str,
    credential_id: &str,
) -> (String, RefreshSession) {
    let session_id = format!("rs_{}", Uuid::new_v4().simple());
    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token = format!("yrt_v1.{session_id}.{secret}");
    let now = yc_shared_protocol::now_rfc3339_nanos();
    (
        token,
        RefreshSession {
            session_id: session_id.clone(),
            system_id: system_id.to_string(),
            device_id: device_id.to_string(),
            key_id: key_id.to_string(),
            credential_id: credential_id.to_string(),
            refresh_secret_hash: sha256_hex(&secret),
            expires_at: unix_now().saturating_add(REFRESH_TOKEN_TTL_SEC),
            created_at: now,
            revoked_at: None,
            rotated_from: None,
        },
    )
}

/// 解析 refresh token（`yrt_v1.<session>.<secret>`）。
pub(crate) fn parse_refresh_token(token: &str) -> Result<(String, String), ApiError> {
    let mut parts = token.split('.');
    let version = parts.next().unwrap_or_default();
    let session = parts.next().unwrap_or_default();
    let secret = parts.next().unwrap_or_default();
    if version != "yrt_v1" || session.is_empty() || secret.is_empty() || parts.next().is_some() {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "REFRESH_TOKEN_INVALID",
            "refreshToken 格式无效",
            "请重新配对",
        ));
    }
    Ok((session.to_string(), secret.to_string()))
}

/// 校验 Ed25519 PoP 签名。
pub(crate) fn verify_pop_signature(
    public_key_b64: &str,
    payload: &str,
    signature_b64: &str,
) -> Result<(), ApiError> {
    let pk_raw = URL_SAFE_NO_PAD
        .decode(public_key_b64.as_bytes())
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "设备公钥格式无效",
                "请重新生成设备绑定信息",
            )
        })?;
    let pk_bytes: [u8; 32] = pk_raw.try_into().map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_PROOF_INVALID",
            "设备公钥长度无效",
            "请重新生成设备绑定信息",
        )
    })?;

    let sig_raw = URL_SAFE_NO_PAD
        .decode(signature_b64.as_bytes())
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "签名格式无效",
                "请重试",
            )
        })?;
    let sig_bytes: [u8; 64] = sig_raw.try_into().map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_PROOF_INVALID",
            "签名长度无效",
            "请重试",
        )
    })?;

    let verifying_key = VerifyingKey::from_bytes(&pk_bytes).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_PROOF_INVALID",
            "设备公钥无法解析",
            "请重新生成设备绑定信息",
        )
    })?;

    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "签名校验失败",
                "请重试或重新配对",
            )
        })
}

/// 计算 keyId。
pub(crate) fn key_id_for_public_key(public_key_b64: &str) -> Result<String, ApiError> {
    let pk_raw = URL_SAFE_NO_PAD
        .decode(public_key_b64.as_bytes())
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "设备公钥格式无效",
                "请重新生成设备绑定信息",
            )
        })?;
    let digest = Sha256::digest(pk_raw);
    Ok(format!("kid_{}", URL_SAFE_NO_PAD.encode(&digest[..10])))
}

/// HMAC-SHA256 并输出 base64url。
pub(crate) fn hmac_b64url(secret: &str, payload: &[u8]) -> Result<String, ApiError> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "签名密钥无效",
            "请稍后重试",
        )
    })?;
    mac.update(payload);
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// sha256 hex。
pub(crate) fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}
