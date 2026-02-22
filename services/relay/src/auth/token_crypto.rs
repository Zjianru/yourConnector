//! Token 加解密与 PoP 签名辅助函数。

use axum::http::StatusCode;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::api::error::ApiError;

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
