//! Sidecar 主程序职责：
//! 1. 维护与 relay 的长连接，双向收发事件。
//! 2. 周期采集宿主机与工具指标，并推送给移动端。
//! 3. 处理工具接入/断开控制命令，维护本地白名单与控制权限。

use anyhow::Result;
use axum::{Router, routing::get};
use tracing::{error, info};

mod cli;
mod config;
mod control;
mod logging;
mod pairing;
mod runtime;
mod session;
mod stores;
mod tooling;

use config::Config;

pub(crate) use runtime::{ProcInfo, fallback_tools_or_empty};
pub(crate) use tooling::{
    build_openclaw_tool_id, build_opencode_tool_id, bytes_to_gb, bytes_to_mb,
    collect_opencode_session_state, detect_openclaw_mode, detect_opencode_mode,
    evaluate_openclaw_connection, evaluate_opencode_connection, first_non_empty,
    is_openclaw_candidate_command, is_opencode_candidate_command, is_opencode_wrapper_command,
    normalize_path, normalize_probe_host, option_non_empty, parse_cli_flag_value,
    parse_serve_address, pick_runtime_pid, round2,
};

/// Sidecar 入口：初始化日志、启动 health server、进入 relay 会话循环。
#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<String>>();
    match cli::dispatch(&args).await? {
        cli::CliDispatch::Run => {}
        cli::CliDispatch::Exit => return Ok(()),
    }

    let _log_runtime = logging::init("sidecar")?;

    let cfg = Config::from_env()?;
    info!(
        "sidecar identity ready system_id={} device_id={} host_name={} pairing_code={}",
        cfg.system_id,
        cfg.device_id,
        cfg.host_name,
        cfg.pairing_code()
    );

    let health_addr = cfg.health_addr.clone();
    tokio::spawn(async move {
        if let Err(err) = run_health_server(&health_addr).await {
            error!("health server exited: {err}");
        }
    });

    session::r#loop::run(cfg).await
}

/// 对外暴露 `/healthz`，用于本机探活与调试。
async fn run_health_server(addr: &str) -> Result<()> {
    let app = Router::new().route("/healthz", get(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("sidecar-rs listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_openclaw_tool_id, build_opencode_tool_id};
    use crate::config::{derive_system_id, normalize_relay_for_system_id, relay_is_local};

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
    fn opencode_tool_id_uses_workspace_and_instance() {
        let a = build_opencode_tool_id("/workspace/work-a", 1001);
        let b = build_opencode_tool_id("/workspace/work-a", 2002);
        let c = build_opencode_tool_id("/workspace/work-b", 1001);

        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("opencode_"));
        assert!(a.ends_with("_p1001"));
    }

    #[test]
    fn openclaw_tool_id_uses_workspace_and_instance() {
        let a = build_openclaw_tool_id("/workspace/work-a", "openclaw", 1001);
        let b = build_openclaw_tool_id("/workspace/work-a", "openclaw --model gpt-5", 2002);
        let c = build_openclaw_tool_id("/workspace/work-b", "openclaw", 1001);

        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("openclaw_"));
        assert!(a.ends_with("_p1001"));
    }

    #[test]
    fn openclaw_tool_id_falls_back_to_command_hash_without_workspace() {
        let a = build_openclaw_tool_id("", "openclaw --model gpt-5", 1001);
        let b = build_openclaw_tool_id("", "openclaw --model gpt-5", 2002);
        let c = build_openclaw_tool_id("", "openclaw --model claude", 1001);

        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn relay_local_detection_supports_loopback_only() {
        assert!(relay_is_local("ws://127.0.0.1:18080/v1/ws"));
        assert!(relay_is_local("ws://localhost:18080/v1/ws"));
        assert!(relay_is_local("ws://[::1]:18080/v1/ws"));
        assert!(!relay_is_local("wss://relay.example.com/v1/ws"));
    }
}
