//! Relay 会话循环。

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use serde_json::json;
use sysinfo::System;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use url::Url;

use crate::{
    config::Config,
    control::{
        CONTROLLER_BIND_UPDATED_EVENT, SidecarCommand, SidecarCommandEnvelope,
        TOOL_WHITELIST_UPDATED_EVENT, command_feedback_parts, parse_sidecar_command,
    },
    discover_tools,
    pairing::{banner::print_pairing_banner, bootstrap_client::fetch_pair_bootstrap},
    session::{
        snapshots::{is_fallback_tool, send_snapshots, summarize_wire_payload},
        transport::send_event,
    },
    stores::{ControllerDevicesStore, ToolWhitelistStore},
};

/// 原始 payload 日志开关环境变量（默认关闭）。
const RAW_PAYLOAD_LOG_ENV: &str = "YC_DEBUG_RAW_PAYLOAD";

/// 维护 relay 会话生命周期，并在断线后执行指数退避重连。
pub(crate) async fn run_relay_loop(cfg: Config) -> Result<()> {
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
async fn run_session(cfg: &Config) -> Result<()> {
    let ws_url = sidecar_ws_url(cfg)?;
    info!("connecting relay {}", ws_url);

    let (ws_stream, _) = connect_async(ws_url.as_str()).await?;
    info!("relay connected");

    match fetch_pair_bootstrap(
        &cfg.relay_ws_url,
        &cfg.system_id,
        &cfg.pair_token,
        &cfg.host_name,
    )
    .await
    {
        Ok(data) => print_pairing_banner(&data),
        Err(err) => warn!("fetch pair bootstrap failed: {err}"),
    }

    let (mut ws_writer, mut ws_reader) = ws_stream.split();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SidecarCommandEnvelope>();
    let log_raw_payload = raw_payload_logging_enabled();

    // reader_task 专门读取 relay 下行消息，并抽取 sidecar 控制命令。
    let mut reader_task = tokio::spawn(async move {
        while let Some(next) = ws_reader.next().await {
            match next {
                Ok(Message::Text(text)) => {
                    if let Some(command) = parse_sidecar_command(&text) {
                        if cmd_tx.send(command).is_err() {
                            break;
                        }
                    } else if log_raw_payload {
                        info!("incoming raw: {text}");
                    } else {
                        info!("incoming event: {}", summarize_wire_payload(&text));
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
    let mut whitelist = ToolWhitelistStore::load();
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
                    SidecarCommand::RebindController { .. } => {}
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

/// 组装 sidecar 连接 relay 的 WS URL，并注入身份 query 参数。
fn sidecar_ws_url(cfg: &Config) -> Result<Url> {
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
fn raw_payload_logging_enabled() -> bool {
    let raw = std::env::var(RAW_PAYLOAD_LOG_ENV).unwrap_or_default();
    let normalized = raw.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
}

/// session 模块总入口，供 main 调用。
pub(crate) async fn run(cfg: Config) -> Result<()> {
    if let Err(err) = run_relay_loop(cfg).await {
        error!("relay loop exited: {err}");
        return Err(err);
    }
    Ok(())
}
