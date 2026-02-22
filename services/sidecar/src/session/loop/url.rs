//! Relay 连接 URL 与日志开关工具。

use anyhow::Result;
use url::Url;

use crate::config::Config;

/// 原始 payload 日志开关环境变量（默认关闭）。
const RAW_PAYLOAD_LOG_ENV: &str = "YC_DEBUG_RAW_PAYLOAD";

/// 组装 sidecar 连接 relay 的 WS URL，并注入身份 query 参数。
pub(crate) fn sidecar_ws_url(cfg: &Config) -> Result<Url> {
    let mut url = Url::parse(&cfg.relay_ws_url)?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("clientType", "sidecar");
        pairs.append_pair("systemId", &cfg.system_id);
        pairs.append_pair("deviceId", &cfg.device_id);
        pairs.append_pair("pairToken", &cfg.pair_token);
        pairs.append_pair("hostName", &cfg.host_name);
    }
    Ok(url)
}

/// 是否开启原始 payload 日志（默认关闭）。
pub(crate) fn raw_payload_logging_enabled() -> bool {
    let raw = std::env::var(RAW_PAYLOAD_LOG_ENV).unwrap_or_default();
    let normalized = raw.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
}
