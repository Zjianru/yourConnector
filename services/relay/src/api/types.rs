//! API 请求/响应类型与内部鉴权类型。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// WS 握手 query 参数。
#[derive(Debug, Deserialize)]
pub(crate) struct WsQuery {
    #[serde(rename = "systemId")]
    pub(crate) system_id: String,
    #[serde(rename = "clientType")]
    pub(crate) client_type: String,
    #[serde(rename = "deviceId")]
    pub(crate) device_id: String,
    #[serde(rename = "pairToken", default)]
    pub(crate) pair_token: String,
    #[serde(rename = "pairTicket", default)]
    pub(crate) pair_ticket: Option<String>,
    #[serde(rename = "hostName", default)]
    pub(crate) host_name: Option<String>,
    /// 设备 access token（生产链路）。
    #[serde(rename = "accessToken", default)]
    pub(crate) access_token: Option<String>,
    /// 设备 key id。
    #[serde(rename = "keyId", default)]
    pub(crate) key_id: Option<String>,
    /// 签名时间戳（秒）。
    #[serde(rename = "ts", default)]
    pub(crate) ts: Option<String>,
    /// 请求 nonce。
    #[serde(default)]
    pub(crate) nonce: Option<String>,
    /// PoP 签名。
    #[serde(rename = "sig", default)]
    pub(crate) sig: Option<String>,
}

/// 配对鉴权方式。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PairAuthMode {
    PairTicket,
}

/// 配对预检请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairPreflightRequest {
    pub(crate) system_id: String,
    pub(crate) device_id: String,
    /// 兼容字段：旧客户端可能仍上传 pairToken，新版将显式拒绝。
    #[serde(default)]
    pub(crate) pair_token: Option<String>,
    #[serde(default)]
    pub(crate) pair_ticket: Option<String>,
}

/// 配对预检返回数据。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairPreflightData {
    pub(crate) auth_mode: PairAuthMode,
}

/// 配对换发请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairExchangeRequest {
    pub(crate) system_id: String,
    pub(crate) device_id: String,
    #[serde(default)]
    pub(crate) device_name: String,
    /// 兼容字段：旧客户端可能仍上传 pairToken，新版将显式拒绝。
    #[serde(default)]
    pub(crate) pair_token: Option<String>,
    #[serde(default)]
    pub(crate) pair_ticket: Option<String>,
    pub(crate) device_pub_key: String,
    pub(crate) key_id: String,
    pub(crate) proof: String,
}

/// 配对换发数据。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairExchangeData {
    pub(crate) auth_mode: PairAuthMode,
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) key_id: String,
    pub(crate) credential_id: String,
    pub(crate) access_expires_in_sec: u64,
    pub(crate) refresh_expires_in_sec: u64,
}

/// 配对签发请求（供 sidecar/脚本统一拿链接）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairBootstrapRequest {
    pub(crate) system_id: String,
    pub(crate) pair_token: String,
    #[serde(default)]
    pub(crate) host_name: Option<String>,
    #[serde(default)]
    pub(crate) relay_ws_url: Option<String>,
    #[serde(default)]
    pub(crate) include_code: Option<bool>,
    #[serde(default)]
    pub(crate) ttl_sec: Option<u64>,
}

/// 配对签发响应。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairBootstrapData {
    pub(crate) pair_link: String,
    pub(crate) pair_ticket: String,
    pub(crate) relay_ws_url: String,
    pub(crate) system_id: String,
    pub(crate) host_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pair_code: Option<String>,
    pub(crate) simctl_command: String,
}

/// 刷新请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthRefreshRequest {
    pub(crate) system_id: String,
    pub(crate) device_id: String,
    pub(crate) refresh_token: String,
    pub(crate) key_id: String,
    pub(crate) ts: String,
    pub(crate) nonce: String,
    pub(crate) sig: String,
}

/// 刷新返回。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthRefreshData {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) key_id: String,
    pub(crate) credential_id: String,
    pub(crate) access_expires_in_sec: u64,
    pub(crate) refresh_expires_in_sec: u64,
}

/// 吊销请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthRevokeDeviceRequest {
    pub(crate) system_id: String,
    pub(crate) device_id: String,
    pub(crate) target_device_id: String,
    pub(crate) access_token: String,
    pub(crate) key_id: String,
    pub(crate) ts: String,
    pub(crate) nonce: String,
    pub(crate) sig: String,
}

/// 吊销结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthRevokeDeviceData {
    pub(crate) target_device_id: String,
}

/// 设备列表查询参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthDevicesQuery {
    pub(crate) system_id: String,
    pub(crate) device_id: String,
    pub(crate) access_token: String,
    pub(crate) key_id: String,
    pub(crate) ts: String,
    pub(crate) nonce: String,
    pub(crate) sig: String,
}

/// 设备列表项。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceEntry {
    pub(crate) device_id: String,
    pub(crate) device_name: String,
    pub(crate) key_id: String,
    pub(crate) status: String,
    pub(crate) created_at: String,
    pub(crate) last_seen_at: String,
    pub(crate) revoked_at: Option<String>,
}

/// 设备列表返回。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthDevicesData {
    pub(crate) devices: Vec<DeviceEntry>,
}

/// 持久化认证元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthStore {
    pub(crate) version: u32,
    pub(crate) signing_key: String,
    pub(crate) systems: HashMap<String, SystemAuthState>,
}

impl AuthStore {
    pub(crate) fn new(signing_key: String) -> Self {
        Self {
            version: 1,
            signing_key,
            systems: HashMap::new(),
        }
    }

    pub(crate) fn system_mut(&mut self, system_id: &str) -> &mut SystemAuthState {
        self.systems.entry(system_id.to_string()).or_default()
    }

    pub(crate) fn system_ref(&self, system_id: &str) -> Option<&SystemAuthState> {
        self.systems.get(system_id)
    }
}

/// 单 system 认证元数据。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemAuthState {
    pub(crate) pair_token_hash: Option<String>,
    pub(crate) pair_token_updated_at: Option<String>,
    pub(crate) devices: HashMap<String, DeviceCredential>,
    pub(crate) refresh_sessions: HashMap<String, RefreshSession>,
}

/// 设备凭证记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceCredential {
    pub(crate) device_id: String,
    pub(crate) device_name: String,
    pub(crate) key_id: String,
    pub(crate) public_key: String,
    pub(crate) status: String,
    pub(crate) created_at: String,
    pub(crate) last_seen_at: String,
    pub(crate) revoked_at: Option<String>,
}

/// refresh 会话记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RefreshSession {
    pub(crate) session_id: String,
    pub(crate) system_id: String,
    pub(crate) device_id: String,
    pub(crate) key_id: String,
    pub(crate) credential_id: String,
    pub(crate) refresh_secret_hash: String,
    pub(crate) expires_at: u64,
    pub(crate) created_at: String,
    pub(crate) revoked_at: Option<String>,
    pub(crate) rotated_from: Option<String>,
}

/// access token claims。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccessTokenClaims {
    pub(crate) sid: String,
    pub(crate) did: String,
    pub(crate) kid: String,
    pub(crate) iat: u64,
    pub(crate) exp: u64,
    pub(crate) jti: String,
}

/// 短时票据 claims。
#[derive(Debug, Deserialize)]
pub(crate) struct PairTicketClaims {
    pub(crate) sid: String,
    pub(crate) iat: u64,
    pub(crate) exp: u64,
    pub(crate) nonce: String,
}

/// 配对票据校验错误。
#[derive(Debug)]
pub(crate) enum PairTicketError {
    Empty,
    Format,
    SignatureFormat,
    SignatureVerify,
    Payload,
    Claims,
    SystemMismatch,
    EmptyNonce,
    Expired,
    IatInvalid,
    Replay,
}

/// pairToken 鉴权决策。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum PairTokenAuthDecision {
    Allow,
    Rotate,
    Initialize,
}

/// 终端高亮样式：重置。
pub(crate) const ANSI_RESET: &str = "\x1b[0m";
/// 终端高亮样式：粗体。
pub(crate) const ANSI_BOLD: &str = "\x1b[1m";
/// 终端高亮样式：青色。
pub(crate) const ANSI_CYAN: &str = "\x1b[36m";
/// 终端高亮样式：亮白。
pub(crate) const ANSI_WHITE: &str = "\x1b[97m";

/// 预检与换发接口默认使用的 access token TTL（秒）。
pub(crate) const ACCESS_TOKEN_TTL_SEC: u64 = 600;
/// refresh token 有效期（秒）。
pub(crate) const REFRESH_TOKEN_TTL_SEC: u64 = 30 * 24 * 3600;
/// PoP 签名请求时间窗（秒）。
pub(crate) const POP_MAX_SKEW_SEC: u64 = 120;
/// 配对票据默认有效期（秒）。
pub(crate) const DEFAULT_PAIR_TICKET_TTL_SEC: u64 = 300;
