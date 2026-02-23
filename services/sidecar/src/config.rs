//! 配置模块职责：
//! 1. 读取 sidecar 运行所需的环境变量与持久化配置文件，并提供默认值。
//! 2. 管理宿主机 `systemId` 与 `pairToken` 的本地持久化身份。
//! 3. 提供 relay URL 校验、配置落盘、布尔/时长/CSV 解析等通用能力。

use std::{
    fs,
    net::{Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::tooling::core::scheduler::{
    DEFAULT_DETAILS_COMMAND_TIMEOUT_MS, DEFAULT_DETAILS_DEBOUNCE_SEC, DEFAULT_DETAILS_INTERVAL_SEC,
    DEFAULT_DETAILS_MAX_PARALLEL,
};

/// sidecar 默认 relay 地址（开发态默认本机）。
pub(crate) const DEFAULT_RELAY_WS_URL: &str = "ws://127.0.0.1:18080/v1/ws";
/// 允许不安全 ws 的环境变量开关。
const ALLOW_INSECURE_WS_ENV: &str = "YC_ALLOW_INSECURE_WS";
/// 标记 research 构建渠道的环境变量。
const BUILD_CHANNEL_ENV: &str = "YC_BUILD_CHANNEL";
/// 持久化配置版本。
const SIDECAR_CONFIG_VERSION: u8 = 1;

/// sidecar 持久化配置（仅存可覆盖项，不存敏感令牌）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarPersistedConfig {
    /// 配置结构版本。
    #[serde(default = "default_config_version")]
    pub(crate) version: u8,
    /// 持久化 relay WS 地址。
    #[serde(default)]
    pub(crate) relay_ws_url: Option<String>,
    /// 持久化宿主机显示名。
    #[serde(default)]
    pub(crate) host_name: Option<String>,
    /// 持久化 sidecar 设备 ID。
    #[serde(default)]
    pub(crate) device_id: Option<String>,
    /// 持久化“首个控制端自动绑定”开关。
    #[serde(default)]
    pub(crate) allow_first_controller_bind: Option<bool>,
    /// 持久化控制端白名单。
    #[serde(default)]
    pub(crate) controller_device_ids: Option<Vec<String>>,
}

impl Default for SidecarPersistedConfig {
    /// 返回 sidecar 持久化配置默认值。
    fn default() -> Self {
        Self {
            version: SIDECAR_CONFIG_VERSION,
            relay_ws_url: None,
            host_name: None,
            device_id: None,
            allow_first_controller_bind: None,
            controller_device_ids: None,
        }
    }
}

/// 返回持久化配置版本默认值。
fn default_config_version() -> u8 {
    SIDECAR_CONFIG_VERSION
}

/// Sidecar 运行时配置。
#[derive(Debug, Clone)]
pub(crate) struct Config {
    /// Relay WebSocket 地址。
    pub(crate) relay_ws_url: String,
    /// 宿主系统标识。
    pub(crate) system_id: String,
    /// 当前 sidecar 设备标识。
    pub(crate) device_id: String,
    /// Relay 握手鉴权 token。
    pub(crate) pair_token: String,
    /// 宿主机展示名称。
    pub(crate) host_name: String,
    /// 预授权控制端设备 ID 列表。
    pub(crate) controller_device_ids: Vec<String>,
    /// 当未配置控制端白名单时，是否允许首个 app 自动绑定。
    pub(crate) allow_first_controller_bind: bool,
    /// Sidecar 健康检查监听地址。
    pub(crate) health_addr: String,
    /// 心跳推送周期。
    pub(crate) heartbeat_interval: Duration,
    /// 指标快照推送周期。
    pub(crate) metrics_interval: Duration,
    /// 工具详情补采周期。
    pub(crate) details_interval: Duration,
    /// 工具详情按需刷新去抖窗口。
    pub(crate) details_refresh_debounce: Duration,
    /// 工具详情 CLI 命令执行超时。
    pub(crate) details_command_timeout: Duration,
    /// 工具详情采集并发上限。
    pub(crate) details_max_parallel: usize,
    /// 是否启用 fallback 工具占位。
    pub(crate) fallback_tool: bool,
}

impl Config {
    /// 从环境变量与配置文件构建配置，并做 relay URL 安全校验。
    pub(crate) fn from_env() -> anyhow::Result<Self> {
        let persisted = load_sidecar_persisted_config().unwrap_or_default();

        let raw_relay = std::env::var("RELAY_WS_URL")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| persisted.relay_ws_url.clone())
            .unwrap_or_else(|| DEFAULT_RELAY_WS_URL.to_string());

        let allow_insecure_ws = bool_from_env(ALLOW_INSECURE_WS_ENV, false);
        let relay_ws_url = validate_relay_ws_url_with_mode(&raw_relay, allow_insecure_ws)
            .with_context(|| format!("invalid relay ws url: {raw_relay}"))?;

        let system_id = std::env::var("SYSTEM_ID")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(load_or_create_system_id);
        let pair_token = std::env::var("PAIR_TOKEN")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(load_or_create_pair_token);

        let host_name = std::env::var("HOST_NAME")
            .ok()
            .map(|raw| normalize_host_name(&raw))
            .filter(|value| !value.is_empty())
            .or_else(|| {
                persisted
                    .host_name
                    .as_ref()
                    .map(|raw| normalize_host_name(raw))
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(detect_host_name);

        let device_id = std::env::var("DEVICE_ID")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                persisted
                    .device_id
                    .as_ref()
                    .map(|raw| raw.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| "sidecar_local".to_string());

        let controller_device_ids = csv_list_from_env_optional("CONTROLLER_DEVICE_IDS")
            .or_else(|| persisted.controller_device_ids.clone())
            .unwrap_or_default();

        let allow_first_controller_bind = bool_from_env_optional("ALLOW_FIRST_CONTROLLER_BIND")
            .or(persisted.allow_first_controller_bind)
            .unwrap_or_else(|| relay_is_local(&relay_ws_url));

        Ok(Self {
            relay_ws_url,
            system_id,
            device_id,
            pair_token,
            host_name,
            controller_device_ids,
            allow_first_controller_bind,
            health_addr: env_or_default("SIDECAR_ADDR", "0.0.0.0:18081"),
            heartbeat_interval: duration_from_env("HEARTBEAT_INTERVAL_SEC", 5),
            metrics_interval: duration_from_env("METRICS_INTERVAL_SEC", 10),
            details_interval: duration_from_env(
                "DETAILS_INTERVAL_SEC",
                DEFAULT_DETAILS_INTERVAL_SEC,
            ),
            details_refresh_debounce: duration_from_env(
                "DETAILS_REFRESH_DEBOUNCE_SEC",
                DEFAULT_DETAILS_DEBOUNCE_SEC,
            ),
            details_command_timeout: duration_from_env_millis(
                "DETAILS_COMMAND_TIMEOUT_MS",
                DEFAULT_DETAILS_COMMAND_TIMEOUT_MS,
            ),
            details_max_parallel: usize_from_env(
                "DETAILS_MAX_PARALLEL",
                DEFAULT_DETAILS_MAX_PARALLEL,
            ),
            fallback_tool: bool_from_env("FALLBACK_TOOL_ENABLED", false),
        })
    }

    /// 返回可直接粘贴到 mobile 的配对码。
    pub(crate) fn pairing_code(&self) -> String {
        format!("{}.{}", self.system_id, self.pair_token)
    }
}

/// 读取 sidecar 持久化配置；文件不存在时返回默认值。
pub(crate) fn load_sidecar_persisted_config() -> anyhow::Result<SidecarPersistedConfig> {
    let Some(path) = sidecar_config_file_path() else {
        return Ok(SidecarPersistedConfig::default());
    };
    if !path.exists() {
        return Ok(SidecarPersistedConfig::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read sidecar config failed: {}", path.display()))?;
    let mut parsed: SidecarPersistedConfig = serde_json::from_str(&raw)
        .with_context(|| format!("decode sidecar config failed: {}", path.display()))?;
    if parsed.version == 0 {
        parsed.version = SIDECAR_CONFIG_VERSION;
    }
    Ok(parsed)
}

/// 持久化 sidecar 配置到 `~/.config/yourconnector/sidecar/config.json`。
pub(crate) fn save_sidecar_persisted_config(config: &SidecarPersistedConfig) -> anyhow::Result<()> {
    let Some(path) = sidecar_config_file_path() else {
        return Err(anyhow!("HOME not set, cannot persist sidecar config"));
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "create sidecar config directory failed: {}",
                parent.display()
            )
        })?;
    }
    let payload = serde_json::to_string_pretty(config).context("encode sidecar config failed")?;
    fs::write(&path, format!("{payload}\n"))
        .with_context(|| format!("write sidecar config failed: {}", path.display()))?;
    Ok(())
}

/// 返回 sidecar 配置文件路径。
pub(crate) fn sidecar_config_file_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        Path::new(&home)
            .join(".config")
            .join("yourconnector")
            .join("sidecar")
            .join("config.json"),
    )
}

/// 对外暴露：校验并规范化用户输入的 relay WS URL。
pub(crate) fn validate_user_relay_ws_url(
    raw: &str,
    allow_insecure_ws: bool,
) -> anyhow::Result<String> {
    validate_relay_ws_url_with_mode(raw, allow_insecure_ws)
}

/// 将 relay 地址映射为健康检查地址（`/healthz`）。
pub(crate) fn relay_health_url(relay_ws_url: &str) -> anyhow::Result<Url> {
    let mut parsed = Url::parse(relay_ws_url)
        .with_context(|| format!("invalid relay ws url: {relay_ws_url}"))?;
    match parsed.scheme() {
        "ws" => {
            let _ = parsed.set_scheme("http");
        }
        "wss" => {
            let _ = parsed.set_scheme("https");
        }
        other => return Err(anyhow!("unsupported relay scheme: {other}")),
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.set_path("/healthz");
    Ok(parsed)
}

/// 仅在 debug/research 且显式传入开关时允许 ws。
fn allow_insecure_ws_for_runtime(allow_insecure_ws: bool) -> bool {
    if !allow_insecure_ws {
        return false;
    }
    if cfg!(debug_assertions) {
        return true;
    }
    matches!(
        std::env::var(BUILD_CHANNEL_ENV)
            .ok()
            .map(|v| v.trim().to_ascii_lowercase()),
        Some(v) if v == "research"
    )
}

/// 校验 relay URL：默认仅允许 `wss://.../v1/ws`，`ws` 仅开发显式开启。
fn validate_relay_ws_url_with_mode(raw: &str, allow_insecure_ws: bool) -> anyhow::Result<String> {
    let normalized = normalize_relay_ws_url(raw)?;
    let parsed = Url::parse(&normalized).context("parse normalized relay url failed")?;
    match parsed.scheme() {
        "wss" => Ok(normalized),
        "ws" => {
            // 本机回环 ws 仅用于单机 relay<->sidecar 内部链路，允许直接通过。
            if relay_is_local(&normalized) {
                return Ok(normalized);
            }
            if allow_insecure_ws_for_runtime(allow_insecure_ws) {
                Ok(normalized)
            } else {
                Err(anyhow!(
                    "insecure ws is disabled; use wss://.../v1/ws or set {ALLOW_INSECURE_WS_ENV}=1 in debug/research build"
                ))
            }
        }
        other => Err(anyhow!("unsupported relay ws scheme: {other}")),
    }
}

/// 规范化 relay 地址，仅保留 scheme/host/path，且 path 必须为 `/v1/ws`。
fn normalize_relay_ws_url(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("relay ws url cannot be empty"));
    }
    let mut parsed = Url::parse(trimmed).context("invalid relay ws url")?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "ws" && scheme != "wss" {
        return Err(anyhow!("relay ws url must use ws:// or wss://"));
    }
    if parsed.host().is_none() {
        return Err(anyhow!("relay ws url missing host"));
    }

    parsed.set_query(None);
    parsed.set_fragment(None);

    let normalized_path = parsed.path().trim_end_matches('/');
    if normalized_path != "/v1/ws" {
        return Err(anyhow!("relay ws url path must be /v1/ws"));
    }
    parsed.set_path("/v1/ws");

    Ok(parsed.to_string())
}

/// 推断宿主机名称：优先系统环境变量，其次系统命令。
fn detect_host_name() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let normalized = normalize_host_name(&value);
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }

    for scutil_key in ["ComputerName", "LocalHostName", "HostName"] {
        if let Ok(output) = Command::new("scutil").args(["--get", scutil_key]).output() {
            let value = String::from_utf8_lossy(&output.stdout);
            let normalized = normalize_host_name(&value);
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }

    if let Ok(output) = Command::new("hostname").output() {
        let value = String::from_utf8_lossy(&output.stdout);
        let normalized = normalize_host_name(&value);
        if !normalized.is_empty() {
            return normalized;
        }
    }

    "My Mac".to_string()
}

/// 规范化宿主机名称：去掉空白，长度限制到 64 字符。
fn normalize_host_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(64).collect::<String>()
}

/// 基于 relay 地址稳定推导 systemId，确保移动端与 sidecar 规则一致。
#[cfg(test)]
pub(crate) fn derive_system_id(relay_ws_url: &str) -> String {
    let normalized = normalize_relay_for_system_id(relay_ws_url);
    if normalized.is_empty() {
        return "sys_local".to_string();
    }
    let hex = format!("{:016x}", fnv1a64(normalized.as_bytes()));
    format!("sys_{}", &hex[..12])
}

/// 归一化 relay 地址，仅保留 scheme/host/path，忽略 query 与 fragment。
#[cfg(test)]
pub(crate) fn normalize_relay_for_system_id(relay_ws_url: &str) -> String {
    let raw = relay_ws_url.trim();
    if raw.is_empty() {
        return String::new();
    }

    if let Ok(parsed) = Url::parse(raw) {
        let scheme = parsed.scheme().to_ascii_lowercase();
        let host = parsed
            .host()
            .map(|host| host.to_string().to_ascii_lowercase())
            .unwrap_or_default();
        let host_port = parsed
            .port()
            .map(|port| format!("{host}:{port}"))
            .unwrap_or(host);
        let path = parsed.path().trim_end_matches('/');
        let normalized_path = if path.is_empty() { "/" } else { path };
        return format!("{scheme}://{host_port}{normalized_path}");
    }

    raw.to_ascii_lowercase()
}

/// 判断 relay 是否是本机回环地址，用于决定是否允许首个控制端自动绑定。
pub(crate) fn relay_is_local(relay_ws_url: &str) -> bool {
    let Ok(parsed) = Url::parse(relay_ws_url) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Domain(value)) => value.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(value)) => value.is_loopback(),
        Some(url::Host::Ipv6(value)) => value.is_loopback(),
        None => false,
    }
}

/// 校验公网 IPv4（排除私网、保留地址、回环、链路本地）。
#[allow(dead_code)]
pub(crate) fn validate_public_ipv4(ip: &str) -> anyhow::Result<Ipv4Addr> {
    let parsed = Ipv4Addr::from_str(ip.trim()).context("invalid ipv4")?;
    if parsed.is_private()
        || parsed.is_loopback()
        || parsed.is_link_local()
        || parsed.is_broadcast()
        || parsed.is_documentation()
        || parsed.is_unspecified()
    {
        return Err(anyhow!("ipv4 is not public routable: {parsed}"));
    }

    // 100.64.0.0/10 (CGNAT)
    let octets = parsed.octets();
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return Err(anyhow!("ipv4 is CGNAT range: {parsed}"));
    }

    Ok(parsed)
}

/// 校验并拒绝 IPv6（v1 不支持）。
#[allow(dead_code)]
pub(crate) fn reject_ipv6(ip: &str) -> anyhow::Result<()> {
    if Ipv6Addr::from_str(ip.trim()).is_ok() {
        return Err(anyhow!("ipv6 is not supported in v1"));
    }
    Ok(())
}

/// 读取环境变量；不存在时返回默认值。
fn env_or_default(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}

/// 将逗号分隔的环境变量解析为字符串列表；未设置时返回 None。
fn csv_list_from_env_optional(key: &str) -> Option<Vec<String>> {
    std::env::var(key).ok().map(|raw| {
        raw.split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<String>>()
    })
}

/// 读取秒级时长配置，非法值回退到默认秒数。
fn duration_from_env(key: &str, fallback_sec: u64) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(fallback_sec))
}

/// 读取毫秒级时长配置，非法值回退到默认毫秒数。
fn duration_from_env_millis(key: &str, fallback_ms: u64) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(fallback_ms))
}

/// 读取 usize 配置，非法值回退到默认值。
fn usize_from_env(key: &str, fallback: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

/// 解析布尔环境变量，支持常见 true/false 文本。
fn bool_from_env(key: &str, fallback: bool) -> bool {
    match std::env::var(key) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "y" | "on" => true,
            "0" | "false" | "no" | "n" | "off" => false,
            _ => fallback,
        },
        Err(_) => fallback,
    }
}

/// 解析可选布尔环境变量。
fn bool_from_env_optional(key: &str) -> Option<bool> {
    match std::env::var(key) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "y" | "on" => Some(true),
            "0" | "false" | "no" | "n" | "off" => Some(false),
            _ => None,
        },
        Err(_) => None,
    }
}

/// FNV-1a 64 位哈希，保证跨端生成稳定短 ID。
#[cfg(test)]
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// 读取或生成宿主机持久化 `systemId`。
fn load_or_create_system_id() -> String {
    load_or_create_identity_value("system-id", || {
        let hex = Uuid::new_v4().simple().to_string();
        format!("sys_{}", &hex[..12])
    })
}

/// 读取或生成宿主机持久化 `pairToken`。
fn load_or_create_pair_token() -> String {
    load_or_create_identity_value("pair-token", || {
        let hex = Uuid::new_v4().simple().to_string();
        format!("ptk_{hex}")
    })
}

/// 身份值通用持久化逻辑：存在则读取，不存在则生成并写盘。
fn load_or_create_identity_value<F>(file_stem: &str, new_value: F) -> String
where
    F: FnOnce() -> String,
{
    if let Some(path) = identity_file_path(file_stem)
        && let Some(value) = read_trimmed_file(&path)
    {
        return value;
    }

    let value = new_value();
    if let Some(path) = identity_file_path(file_stem) {
        let _ = write_identity_file(&path, &value);
    }
    value
}

/// sidecar 身份文件路径：`~/.config/yourconnector/sidecar/<name>.txt`。
fn identity_file_path(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        Path::new(&home)
            .join(".config")
            .join("yourconnector")
            .join("sidecar")
            .join(format!("{name}.txt")),
    )
}

/// 读取文本并去掉首尾空白；空字符串视为无效。
fn read_trimmed_file(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let value = raw.trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// 持久化身份值到本地文件。
fn write_identity_file(path: &Path, value: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{value}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_RELAY_WS_URL, derive_system_id, normalize_relay_for_system_id, relay_is_local,
        validate_public_ipv4, validate_user_relay_ws_url,
    };

    #[test]
    fn normalize_relay_keeps_scheme_host_path_only() {
        let value = normalize_relay_for_system_id("WS://Relay.EXAMPLE.com:443/v1/ws/?a=1#x");
        assert_eq!(value, "ws://relay.example.com:443/v1/ws");
    }

    #[test]
    fn derive_system_id_matches_mobile_rules() {
        assert_eq!(
            derive_system_id("ws://127.0.0.1:18080/v1/ws"),
            "sys_949014ec1ae3"
        );
        assert_eq!(
            derive_system_id("wss://relay.example.com/v1/ws"),
            "sys_7451849db6ca"
        );
        assert_eq!(
            derive_system_id("ws://[::1]:18080/v1/ws"),
            "sys_b4365eab0f5d"
        );
    }

    #[test]
    fn relay_local_detection_supports_loopback_only() {
        assert!(relay_is_local("ws://127.0.0.1:18080/v1/ws"));
        assert!(relay_is_local("ws://localhost:18080/v1/ws"));
        assert!(relay_is_local("ws://[::1]:18080/v1/ws"));
        assert!(!relay_is_local("wss://relay.example.com/v1/ws"));
    }

    #[test]
    fn relay_user_url_requires_v1_ws_path() {
        assert!(validate_user_relay_ws_url("wss://relay.example.com/v1/ws", false).is_ok());
        assert!(validate_user_relay_ws_url("wss://relay.example.com/v1/other", false).is_err());
        assert!(validate_user_relay_ws_url("https://relay.example.com/v1/ws", false).is_err());
    }

    #[test]
    fn public_ipv4_validation_rejects_private_ranges() {
        assert!(validate_public_ipv4("8.8.8.8").is_ok());
        assert!(validate_public_ipv4("10.0.0.1").is_err());
        assert!(validate_public_ipv4("127.0.0.1").is_err());
        assert!(validate_public_ipv4("100.64.1.1").is_err());
    }

    #[test]
    fn default_relay_is_local_ws_path() {
        assert_eq!(DEFAULT_RELAY_WS_URL, "ws://127.0.0.1:18080/v1/ws");
    }
}
