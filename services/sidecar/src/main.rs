//! Sidecar 主程序职责：
//! 1. 维护与 relay 的长连接，双向收发事件。
//! 2. 周期采集宿主机与工具指标，并推送给移动端。
//! 3. 处理工具接入/断开控制命令，维护本地白名单与控制权限。

use std::time::{Duration, Instant};

use anyhow::anyhow;
use axum::{Router, routing::get};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{Sink, SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use sysinfo::{Disks, ProcessesToUpdate, System};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use url::Url;
use uuid::Uuid;
use yc_shared_protocol::{
    EventEnvelope, MetricsSnapshotPayload, SidecarMetricsPayload, SystemMetricsPayload,
    ToolRuntimePayload, ToolsSnapshotPayload, now_rfc3339_nanos,
};

mod config;
mod control;
mod discoverers;
mod runtime;
mod stores;
mod tooling;

use config::Config;
use control::{
    CONTROLLER_BIND_UPDATED_EVENT, SidecarCommand, SidecarCommandEnvelope,
    TOOL_WHITELIST_UPDATED_EVENT, command_feedback_parts, parse_sidecar_command,
};
use stores::{ControllerDevicesStore, ToolWhitelistStore};

pub(crate) use runtime::{ProcInfo, discover_tools, fallback_tools_or_empty};
pub(crate) use tooling::{
    build_openclaw_tool_id, build_opencode_tool_id, bytes_to_gb, bytes_to_mb,
    collect_opencode_session_state, detect_openclaw_mode, detect_opencode_mode,
    evaluate_opencode_connection, first_non_empty, is_openclaw_candidate_command,
    is_opencode_candidate_command, is_opencode_wrapper_command, normalize_path,
    normalize_probe_host, option_non_empty, parse_cli_flag_value, parse_serve_address,
    pick_runtime_pid, round2,
};

/// 已接入工具快照事件。
const TOOLS_SNAPSHOT_EVENT: &str = "tools_snapshot";
/// 候选工具快照事件。
const TOOLS_CANDIDATES_EVENT: &str = "tools_candidates";
/// 系统/sidecar/工具指标快照事件。
const METRICS_SNAPSHOT_EVENT: &str = "metrics_snapshot";
/// 终端高亮样式：重置。
const ANSI_RESET: &str = "\x1b[0m";
/// 终端高亮样式：粗体。
const ANSI_BOLD: &str = "\x1b[1m";
/// 终端高亮样式：青色。
const ANSI_CYAN: &str = "\x1b[36m";
/// 终端高亮样式：亮白。
const ANSI_WHITE: &str = "\x1b[97m";
/// 配对票据默认有效期（秒）。
const DEFAULT_PAIR_TICKET_TTL_SEC: u64 = 300;

/// Sidecar 入口：初始化日志、启动 health server、进入 relay 会话循环。
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env();
    info!(
        "sidecar identity ready system_id={} device_id={} host_name={} pairing_code={}",
        cfg.system_id,
        cfg.device_id,
        cfg.host_name,
        cfg.pairing_code()
    );
    print_pairing_banner(&cfg);

    let health_addr = cfg.health_addr.clone();
    tokio::spawn(async move {
        if let Err(err) = run_health_server(&health_addr).await {
            error!("health server exited: {err}");
        }
    });

    run_relay_loop(cfg).await
}

/// 生成可供扫码/导入的统一配对链接。
fn build_pairing_link(cfg: &Config) -> String {
    let ttl_sec = pairing_ticket_ttl_sec();
    let pair_ticket = generate_pairing_ticket(&cfg.system_id, &cfg.pair_token, ttl_sec);
    let mut link = Url::parse("yc://pair").expect("pairing link base must be valid");
    {
        let mut pairs = link.query_pairs_mut();
        pairs.append_pair("relay", &cfg.relay_ws_url);
        pairs.append_pair("sid", &cfg.system_id);
        pairs.append_pair("ticket", &pair_ticket);
        pairs.append_pair("name", &cfg.host_name);
    }
    link.to_string()
}

/// 读取配对票据有效期（秒）。
fn pairing_ticket_ttl_sec() -> u64 {
    std::env::var("PAIRING_TICKET_TTL_SEC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v >= 30 && *v <= 3600)
        .unwrap_or(DEFAULT_PAIR_TICKET_TTL_SEC)
}

/// 生成短时配对票据（`pct_v1.<payload_b64url>.<sig_b64url>`）。
fn generate_pairing_ticket(system_id: &str, pair_token: &str, ttl_sec: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let exp = now.saturating_add(ttl_sec);
    let nonce = Uuid::new_v4().simple().to_string();
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

/// 在 sidecar 启动时高亮输出配对信息，确保用户无需翻日志即可看到入口。
fn print_pairing_banner(cfg: &Config) {
    let pairing_code = cfg.pairing_code();
    let pairing_link = build_pairing_link(cfg);
    let simctl_cmd = format!("xcrun simctl openurl booted \"{pairing_link}\"");

    println!(
        "{cyan}{bold}\n╔══════════════════════════════════════════════════════════════╗\n\
         ║                    首次配对（宿主机）                   ║\n\
         ╚══════════════════════════════════════════════════════════════╝{reset}",
        cyan = ANSI_CYAN,
        bold = ANSI_BOLD,
        reset = ANSI_RESET
    );
    println!(
        "{white}{bold}配对码:{reset} {white}{code}{reset}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        code = pairing_code
    );
    println!(
        "{white}{bold}宿主机名:{reset} {name}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        name = cfg.host_name
    );
    println!(
        "{white}{bold}配对链接:{reset} {link}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        link = pairing_link
    );
    println!(
        "{white}{bold}模拟扫码(iOS):{reset} {cmd}\n",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        cmd = simctl_cmd
    );
}

/// 对外暴露 `/healthz`，用于本机探活与调试。
async fn run_health_server(addr: &str) -> anyhow::Result<()> {
    let app = Router::new().route("/healthz", get(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("sidecar-rs listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// 维护 relay 会话生命周期，并在断线后执行指数退避重连。
async fn run_relay_loop(cfg: Config) -> anyhow::Result<()> {
    let mut backoff = Duration::from_secs(1);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("sidecar-rs shutdown requested");
                return Ok(());
            }
            session = run_session(&cfg) => {
                match session {
                    Ok(_) => info!("relay session closed"),
                    Err(err) => warn!("relay session ended: {err}"),
                }
            }
        }

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("sidecar-rs shutdown requested");
                return Ok(());
            }
            _ = tokio::time::sleep(backoff) => {}
        }

        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
}

/// 单次 relay 会话：连接、收命令、推送心跳与快照，直到连接中断。
async fn run_session(cfg: &Config) -> anyhow::Result<()> {
    let ws_url = sidecar_ws_url(cfg)?;
    info!("connecting relay {}", ws_url);

    let (ws_stream, _) = connect_async(ws_url.as_str()).await?;
    info!("relay connected");

    let (mut ws_writer, mut ws_reader) = ws_stream.split();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SidecarCommandEnvelope>();

    // reader_task 专门读取 relay 下行消息，并抽取 sidecar 控制命令。
    let mut reader_task = tokio::spawn(async move {
        while let Some(next) = ws_reader.next().await {
            match next {
                Ok(Message::Text(text)) => {
                    if let Some(command) = parse_sidecar_command(&text) {
                        if cmd_tx.send(command).is_err() {
                            break;
                        }
                    } else {
                        info!("incoming: {text}");
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    warn!("relay read error: {err}");
                    break;
                }
            }
        }
    });

    let mut seq = 0_u64;
    let started_at = Instant::now();
    let mut sys = System::new_all();
    // 工具白名单：决定 tools_snapshot 与 tools_candidates 的划分。
    let mut whitelist = ToolWhitelistStore::load();
    // 控制端白名单：决定当前命令是否有权限执行。
    let mut controllers = ControllerDevicesStore::load();
    if let Err(err) = controllers.seed(&cfg.controller_device_ids) {
        warn!("seed controller devices failed: {err}");
    }
    let mut discovered_tools = discover_tools(&mut sys, cfg.fallback_tool);

    send_snapshots(
        &mut ws_writer,
        cfg,
        &mut seq,
        &mut sys,
        started_at,
        &discovered_tools,
        &whitelist,
    )
    .await?;

    let mut heartbeat_ticker = tokio::time::interval(cfg.heartbeat_interval);
    heartbeat_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut metrics_ticker = tokio::time::interval(cfg.metrics_interval);
    metrics_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            done = &mut reader_task => {
                match done {
                    Ok(_) => return Err(anyhow!("relay read loop closed")),
                    Err(err) => return Err(anyhow!("relay read task join error: {err}")),
                }
            }
            maybe_cmd = cmd_rx.recv() => {
                let Some(command_envelope) = maybe_cmd else {
                    return Err(anyhow!("command channel closed"));
                };

                // 重绑控制端命令由 pairToken 鉴权兜底，不依赖旧 device 白名单。
                if let SidecarCommand::RebindController { device_id } = &command_envelope.command {
                    let device = device_id.trim();
                    let (ok, changed, reason) = if command_envelope.source_client_type != "app" {
                        (
                            false,
                            false,
                            "仅接受 app 客户端发起控制端重绑。".to_string(),
                        )
                    } else if device.is_empty() {
                        (
                            false,
                            false,
                            "缺少目标设备标识，无法重绑控制端。".to_string(),
                        )
                    } else {
                        match controllers.rebind(device) {
                            Ok(changed) => (true, changed, String::new()),
                            Err(err) => (
                                false,
                                false,
                                format!("重绑控制设备失败: {err}"),
                            ),
                        }
                    };

                    send_event(
                        &mut ws_writer,
                        &cfg.system_id,
                        &mut seq,
                        CONTROLLER_BIND_UPDATED_EVENT,
                        json!({
                            "ok": ok,
                            "changed": changed,
                            "deviceId": device,
                            "reason": reason,
                        }),
                    ).await?;

                    continue;
                }

                let (allowed, allow_reason) = match controllers.authorize_or_bind(
                    &command_envelope.source_client_type,
                    &command_envelope.source_device_id,
                    cfg.allow_first_controller_bind,
                ) {
                    Ok(value) => value,
                    Err(err) => (false, format!("更新控制设备配置失败: {err}")),
                };

                // 未授权设备直接回执失败，不执行任何白名单变更。
                if !allowed {
                    let (action, tool_id) = command_feedback_parts(&command_envelope.command);
                    send_event(
                        &mut ws_writer,
                        &cfg.system_id,
                        &mut seq,
                        TOOL_WHITELIST_UPDATED_EVENT,
                        json!({
                            "action": action,
                            "toolId": tool_id,
                            "ok": false,
                            "changed": false,
                            "reason": allow_reason,
                        }),
                    ).await?;
                    continue;
                }

                match command_envelope.command {
                    SidecarCommand::Refresh => {}
                    SidecarCommand::ConnectTool { tool_id } => {
                        let candidate = discovered_tools.iter().find(|tool| tool.tool_id == tool_id);
                        let (ok, changed, reason) = if candidate.is_none() {
                            (
                                false,
                                false,
                                "工具不在当前候选列表，无法接入。".to_string(),
                            )
                        } else if candidate.map(is_fallback_tool).unwrap_or(false) {
                            (
                                false,
                                false,
                                "fallback 工具仅用于占位展示，不能接入。".to_string(),
                            )
                        } else {
                            match whitelist.add(&tool_id) {
                                Ok(changed) => {
                                    if changed {
                                        info!("tool whitelisted: {tool_id}");
                                    }
                                    (true, changed, String::new())
                                }
                                Err(err) => (
                                    false,
                                    false,
                                    format!("更新白名单失败: {err}"),
                                ),
                            }
                        };

                        send_event(
                            &mut ws_writer,
                            &cfg.system_id,
                            &mut seq,
                            TOOL_WHITELIST_UPDATED_EVENT,
                            json!({
                                "action": "connect",
                                "toolId": tool_id,
                                "ok": ok,
                                "changed": changed,
                                "reason": reason,
                            }),
                        ).await?;
                    }
                    SidecarCommand::DisconnectTool { tool_id } => {
                        let (ok, changed, reason) = match whitelist.remove(&tool_id) {
                            Ok(changed) => (true, changed, String::new()),
                            Err(err) => (
                                false,
                                false,
                                format!("更新白名单失败: {err}"),
                            ),
                        };

                        send_event(
                            &mut ws_writer,
                            &cfg.system_id,
                            &mut seq,
                            TOOL_WHITELIST_UPDATED_EVENT,
                            json!({
                                "action": "disconnect",
                                "toolId": tool_id,
                                "ok": ok,
                                "changed": changed,
                                "reason": reason,
                            }),
                        ).await?;
                    }
                    SidecarCommand::RebindController { .. } => {
                        // 已在命令入口提前处理，这里理论上不可达。
                    }
                }

                discovered_tools = discover_tools(&mut sys, cfg.fallback_tool);
                send_snapshots(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    &mut sys,
                    started_at,
                    &discovered_tools,
                    &whitelist,
                )
                .await?;
            }
            _ = heartbeat_ticker.tick() => {
                // 心跳用于移动端判断 sidecar 在线状态。
                send_event(
                    &mut ws_writer,
                    &cfg.system_id,
                    &mut seq,
                    "heartbeat",
                    json!({
                        "status": "ONLINE",
                        "latencyMs": 0,
                    }),
                ).await?;
            }
            _ = metrics_ticker.tick() => {
                // 指标定时器触发时，刷新工具探测并下发三类快照事件。
                discovered_tools = discover_tools(&mut sys, cfg.fallback_tool);
                send_snapshots(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    &mut sys,
                    started_at,
                    &discovered_tools,
                    &whitelist,
                )
                .await?;
            }
        }
    }
}

/// 发送标准 envelope 事件，并维护单连接内递增 seq。
async fn send_event<W>(
    ws_writer: &mut W,
    system_id: &str,
    seq: &mut u64,
    event_type: &str,
    payload: Value,
) -> anyhow::Result<()>
where
    W: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    *seq += 1;
    let mut env = EventEnvelope::new(event_type, system_id, payload);
    env.seq = Some(*seq);
    env.ts = now_rfc3339_nanos();

    let raw = serde_json::to_string(&env)?;
    ws_writer.send(Message::Text(raw.into())).await?;
    Ok(())
}

/// 一次性发送 tools_snapshot / tools_candidates / metrics_snapshot 三个事件。
async fn send_snapshots<W>(
    ws_writer: &mut W,
    cfg: &Config,
    seq: &mut u64,
    sys: &mut System,
    started_at: Instant,
    discovered_tools: &[ToolRuntimePayload],
    whitelist: &ToolWhitelistStore,
) -> anyhow::Result<()>
where
    W: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let (connected_tools, candidate_tools) = split_discovered_tools(discovered_tools, whitelist);

    send_event(
        ws_writer,
        &cfg.system_id,
        seq,
        TOOLS_SNAPSHOT_EVENT,
        serde_json::to_value(ToolsSnapshotPayload {
            tools: connected_tools.clone(),
        })?,
    )
    .await?;

    send_event(
        ws_writer,
        &cfg.system_id,
        seq,
        TOOLS_CANDIDATES_EVENT,
        serde_json::to_value(ToolsSnapshotPayload {
            tools: candidate_tools,
        })?,
    )
    .await?;

    send_event(
        ws_writer,
        &cfg.system_id,
        seq,
        METRICS_SNAPSHOT_EVENT,
        serde_json::to_value(collect_metrics_snapshot(sys, started_at, &connected_tools))?,
    )
    .await?;

    Ok(())
}

/// 根据白名单把“发现到的工具”分成已接入与候选两组。
fn split_discovered_tools(
    discovered_tools: &[ToolRuntimePayload],
    whitelist: &ToolWhitelistStore,
) -> (Vec<ToolRuntimePayload>, Vec<ToolRuntimePayload>) {
    let mut connected = Vec::new();
    let mut candidates = Vec::new();

    for tool in discovered_tools.iter().cloned() {
        if whitelist.contains(&tool.tool_id) {
            connected.push(tool);
        } else {
            candidates.push(tool);
        }
    }

    (connected, candidates)
}

/// 判定是否为 fallback 占位工具（不可接入）。
fn is_fallback_tool(tool: &ToolRuntimePayload) -> bool {
    if tool.tool_id == "tool_local" {
        return true;
    }
    matches!(tool.source.as_deref(), Some("fallback"))
}

/// 组装 sidecar 连接 relay 的 WS URL，并注入身份 query 参数。
fn sidecar_ws_url(cfg: &Config) -> anyhow::Result<Url> {
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

/// 采集系统/sidecar/工具指标，生成统一的 metrics payload。
fn collect_metrics_snapshot(
    sys: &mut System,
    started_at: Instant,
    tools: &[ToolRuntimePayload],
) -> MetricsSnapshotPayload {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    // 系统 CPU/内存使用率。
    let cpu_percent = round2(sys.global_cpu_usage() as f64);
    let memory_total_mb = round2(bytes_to_mb(sys.total_memory()));
    let memory_used_mb = round2(bytes_to_mb(sys.used_memory()));
    let memory_used_percent = if memory_total_mb <= 0.0 {
        0.0
    } else {
        round2(memory_used_mb / memory_total_mb * 100.0)
    };

    // 磁盘总量和已用量（聚合全部挂载盘）。
    let disks = Disks::new_with_refreshed_list();
    let disk_total = disks.list().iter().map(|d| d.total_space()).sum::<u64>();
    let disk_available = disks
        .list()
        .iter()
        .map(|d| d.available_space())
        .sum::<u64>();
    let disk_used = disk_total.saturating_sub(disk_available);

    let disk_total_gb = round2(bytes_to_gb(disk_total));
    let disk_used_gb = round2(bytes_to_gb(disk_used));
    let disk_used_percent = if disk_total_gb <= 0.0 {
        0.0
    } else {
        round2(disk_used_gb / disk_total_gb * 100.0)
    };

    // sidecar 自身进程资源占用。
    let mut sidecar_cpu = 0.0;
    let mut sidecar_mem_mb = 0.0;
    if let Ok(pid) = sysinfo::get_current_pid()
        && let Some(proc_info) = sys.process(pid)
    {
        sidecar_cpu = round2(proc_info.cpu_usage() as f64);
        sidecar_mem_mb = round2(bytes_to_mb(proc_info.memory()));
    }

    let tool_value = tools
        .first()
        .and_then(|tool| serde_json::to_value(tool).ok())
        .unwrap_or_else(|| json!({}));

    MetricsSnapshotPayload {
        system: SystemMetricsPayload {
            cpu_percent,
            memory_total_mb,
            memory_used_mb,
            memory_used_percent,
            disk_total_gb,
            disk_used_gb,
            disk_used_percent,
            uptime_sec: started_at.elapsed().as_secs(),
        },
        sidecar: SidecarMetricsPayload {
            cpu_percent: sidecar_cpu,
            memory_mb: sidecar_mem_mb,
            goroutines: 0,
        },
        tool: tool_value,
        tools: tools
            .iter()
            .cloned()
            .map(|mut tool| {
                tool.collected_at = Some(now_rfc3339_nanos());
                tool
            })
            .collect(),
    }
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
    fn opencode_tool_id_is_stable_for_same_workspace() {
        let a = build_opencode_tool_id("/Users/codez/dev/work-a", 1001);
        let b = build_opencode_tool_id("/Users/codez/dev/work-a", 2002);
        let c = build_opencode_tool_id("/Users/codez/dev/work-b", 1001);

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn openclaw_tool_id_is_stable_for_same_workspace() {
        let a = build_openclaw_tool_id("/Users/codez/dev/work-a", "openclaw", 1001);
        let b = build_openclaw_tool_id("/Users/codez/dev/work-a", "openclaw --model gpt-5", 2002);
        let c = build_openclaw_tool_id("/Users/codez/dev/work-b", "openclaw", 1001);

        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn openclaw_tool_id_falls_back_to_command_hash_without_workspace() {
        let a = build_openclaw_tool_id("", "openclaw --model gpt-5", 1001);
        let b = build_openclaw_tool_id("", "openclaw --model gpt-5", 2002);
        let c = build_openclaw_tool_id("", "openclaw --model claude", 1001);

        assert_eq!(a, b);
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
