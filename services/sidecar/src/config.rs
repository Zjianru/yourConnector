//! 配置模块职责：
//! 1. 读取 sidecar 运行所需的环境变量并提供默认值。
//! 2. 管理宿主机 `systemId` 与 `pairToken` 的本地持久化身份。
//! 3. 提供布尔、时长、CSV 等基础配置解析函数，避免主流程散落解析逻辑。

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use url::Url;
use uuid::Uuid;

use crate::tooling::core::scheduler::{
    DEFAULT_DETAILS_COMMAND_TIMEOUT_MS, DEFAULT_DETAILS_DEBOUNCE_SEC, DEFAULT_DETAILS_INTERVAL_SEC,
    DEFAULT_DETAILS_MAX_PARALLEL,
};

/// Sidecar 运行时配置。
#[derive(Debug, Clone)]
pub(crate) struct Config {
    /// Relay WebSocket 地址。
    pub(crate) relay_ws_url: String,
    /// 宿主系统标识，默认基于 relay 地址推导。
    pub(crate) system_id: String,
    /// 当前 sidecar 设备标识。
    pub(crate) device_id: String,
    /// Relay 握手鉴权 token。
    pub(crate) pair_token: String,
    /// 宿主机展示名称（用于配对链接展示与移动端本地标识）。
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
    /// 从环境变量构建配置，并补齐默认值与推导值。
    pub(crate) fn from_env() -> Self {
        let relay_ws_url = env_or_default("RELAY_WS_URL", "ws://127.0.0.1:18080/v1/ws");
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
        let allow_first_controller_bind =
            bool_from_env("ALLOW_FIRST_CONTROLLER_BIND", relay_is_local(&relay_ws_url));

        Self {
            relay_ws_url,
            system_id,
            device_id: env_or_default("DEVICE_ID", "sidecar_local"),
            pair_token,
            host_name: env_or_default("HOST_NAME", &detect_host_name()),
            controller_device_ids: csv_list_from_env("CONTROLLER_DEVICE_IDS"),
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
        }
    }

    /// 返回可直接粘贴到 mobile 的配对码。
    pub(crate) fn pairing_code(&self) -> String {
        format!("{}.{}", self.system_id, self.pair_token)
    }
}

/// 推断宿主机名称：优先 macOS 系统名，其次环境变量和 hostname 命令。
fn detect_host_name() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let normalized = normalize_host_name(&value);
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }

    // macOS 常见主机名来源。
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

/// 规范化宿主机名称：去掉空白，长度限制到 64 字符，避免链接过长。
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

/// 读取环境变量；不存在时返回默认值。
fn env_or_default(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}

/// 将逗号分隔的环境变量解析为字符串列表。
fn csv_list_from_env(key: &str) -> Vec<String> {
    std::env::var(key)
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
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
