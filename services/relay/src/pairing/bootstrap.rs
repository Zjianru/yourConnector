//! 配对链接签发与 banner 输出。

use url::Url;

use crate::{
    api::types::{
        ANSI_BOLD, ANSI_CYAN, ANSI_RESET, ANSI_WHITE, DEFAULT_PAIR_TICKET_TTL_SEC,
        PairBootstrapData,
    },
    pairing::ticket::generate_pairing_ticket,
};

/// Relay 用于展示配对链接的公开 WS 地址。
pub(crate) fn relay_public_ws_url() -> String {
    let from_env = std::env::var("RELAY_PUBLIC_WS_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    from_env.unwrap_or_else(|| "ws://127.0.0.1:18080/v1/ws".to_string())
}

/// 归一化宿主机名称。
pub(crate) fn normalize_host_name(raw: Option<&str>, fallback: &str) -> String {
    let normalized = raw.unwrap_or_default().trim();
    if normalized.is_empty() {
        return fallback.chars().take(64).collect();
    }
    normalized.chars().take(64).collect()
}

/// 归一化 TTL（秒）。
pub(crate) fn normalize_ttl_sec(raw: Option<u64>) -> u64 {
    raw.unwrap_or(DEFAULT_PAIR_TICKET_TTL_SEC).clamp(30, 3600)
}

/// 生成可扫码导入的统一配对链接数据。
pub(crate) fn build_pair_bootstrap_data(
    relay_ws_url: &str,
    system_id: &str,
    pair_token: &str,
    host_name: &str,
    include_code: bool,
    ttl_sec: u64,
) -> PairBootstrapData {
    let pair_ticket = generate_pairing_ticket(system_id, pair_token, ttl_sec);
    let pair_code = format!("{system_id}.{pair_token}");

    let mut link = Url::parse("yc://pair").expect("pairing link base must be valid");
    {
        let mut pairs = link.query_pairs_mut();
        pairs.append_pair("relay", relay_ws_url);
        pairs.append_pair("sid", system_id);
        pairs.append_pair("ticket", &pair_ticket);
        if !host_name.trim().is_empty() {
            pairs.append_pair("name", host_name.trim());
        }
        if include_code {
            pairs.append_pair("code", &pair_code);
        }
    }
    let pair_link = link.to_string();

    PairBootstrapData {
        pair_link: pair_link.clone(),
        pair_ticket,
        relay_ws_url: relay_ws_url.to_string(),
        system_id: system_id.to_string(),
        host_name: host_name.to_string(),
        pair_code: include_code.then_some(pair_code),
        simctl_command: format!("xcrun simctl openurl booted \"{pair_link}\""),
    }
}

/// sidecar 接入 relay 后，高亮打印配对信息。
pub(crate) fn print_pairing_banner_from_relay(data: &PairBootstrapData) {
    println!(
        "{cyan}{bold}\n╔══════════════════════════════════════════════════════════════╗\n\
         ║                 首次配对（Relay 视角）                  ║\n\
         ╚══════════════════════════════════════════════════════════════╝{reset}",
        cyan = ANSI_CYAN,
        bold = ANSI_BOLD,
        reset = ANSI_RESET
    );
    println!(
        "{white}{bold}Relay WS:{reset} {ws}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        ws = data.relay_ws_url
    );
    if let Some(code) = data.pair_code.as_ref() {
        println!(
            "{white}{bold}配对码:{reset} {white}{code}{reset}",
            white = ANSI_WHITE,
            bold = ANSI_BOLD,
            reset = ANSI_RESET,
            code = code
        );
    }
    println!(
        "{white}{bold}宿主机名:{reset} {name}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        name = data.host_name
    );
    println!(
        "{white}{bold}配对链接:{reset} {link}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        link = data.pair_link
    );
    println!(
        "{white}{bold}提示:{reset} 链接为短时票据，过期后请重新签发最新配对信息。",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET
    );
    println!(
        "{white}{bold}模拟扫码(iOS):{reset} {cmd}\n",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        cmd = data.simctl_command
    );
}

#[cfg(test)]
mod tests {
    use super::{build_pair_bootstrap_data, normalize_ttl_sec};

    #[test]
    fn bootstrap_data_contains_ticket_and_link() {
        let data = build_pair_bootstrap_data(
            "ws://127.0.0.1:18080/v1/ws",
            "sys_demo",
            "ptk_demo",
            "My Mac",
            true,
            180,
        );

        assert_eq!(data.system_id, "sys_demo");
        assert_eq!(data.relay_ws_url, "ws://127.0.0.1:18080/v1/ws");
        assert_eq!(data.host_name, "My Mac");
        assert!(data.pair_ticket.starts_with("pct_v1."));
        assert!(data.pair_link.contains("yc://pair?"));
        assert!(data.pair_link.contains("sid=sys_demo"));
        assert!(data.pair_link.contains("ticket="));
        assert!(data.pair_link.contains("name=My+Mac"));
        assert!(data.pair_link.contains("code=sys_demo.ptk_demo"));
    }

    #[test]
    fn ttl_is_clamped_to_allowed_range() {
        assert_eq!(normalize_ttl_sec(Some(1)), 30);
        assert_eq!(normalize_ttl_sec(Some(6000)), 3600);
        assert_eq!(normalize_ttl_sec(Some(180)), 180);
    }
}
