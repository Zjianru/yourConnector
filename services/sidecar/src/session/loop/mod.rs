//! Relay 会话循环。

mod command;
mod url;

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use serde_json::json;
use sysinfo::System;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use self::{
    command::handle_sidecar_command,
    url::{raw_payload_logging_enabled, sidecar_ws_url},
};
use crate::{
    config::Config,
    control::{SidecarCommandEnvelope, parse_sidecar_command},
    pairing::{banner::print_pairing_banner, bootstrap_client::fetch_pair_bootstrap},
    session::{
        snapshots::{send_snapshots, send_tool_details_snapshot, summarize_wire_payload},
        transport::send_event,
    },
    stores::{ControllerDevicesStore, ToolWhitelistStore},
    tooling::core::{ToolAdapterCore, types::ToolDetailsCollectRequest},
};
use yc_shared_protocol::ToolRuntimePayload;

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
        None,
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
                        debug!(
                            "incoming command type={} event_id={} trace_id={} source_type={} source_device={}",
                            command.event_type,
                            command.event_id,
                            command.trace_id,
                            command.source_client_type,
                            command.source_device_id
                        );
                        if cmd_tx.send(command).is_err() {
                            break;
                        }
                    } else if log_raw_payload {
                        debug!("incoming raw: {text}");
                    } else {
                        debug!("incoming event: {}", summarize_wire_payload(&text));
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
    let mut tool_core = ToolAdapterCore::new(
        cfg.fallback_tool,
        cfg.details_interval,
        cfg.details_command_timeout,
        cfg.details_max_parallel,
        cfg.details_refresh_debounce,
    );
    let mut whitelist = ToolWhitelistStore::load();
    let mut controllers = ControllerDevicesStore::load();
    if let Err(err) = controllers.seed(&cfg.controller_device_ids) {
        warn!("seed controller devices failed: {err}");
    }
    let mut discovered_tools = tool_core.discover_tools(&mut sys);

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
    refresh_and_send_details(
        &mut ws_writer,
        cfg,
        &mut seq,
        &mut tool_core,
        build_details_collect_request(&discovered_tools, &whitelist, None, true),
    )
    .await?;

    let mut heartbeat_ticker = tokio::time::interval(cfg.heartbeat_interval);
    heartbeat_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut metrics_ticker = tokio::time::interval(cfg.metrics_interval);
    metrics_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut details_ticker = tokio::time::interval(cfg.details_interval);
    details_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

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

                let outcome = handle_sidecar_command(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    command_envelope,
                    &discovered_tools,
                    &mut whitelist,
                    &mut controllers,
                ).await?;

                if outcome.refresh_snapshots {
                    discovered_tools = tool_core.discover_tools(&mut sys);
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
                if outcome.refresh_details {
                    refresh_and_send_details(
                        &mut ws_writer,
                        cfg,
                        &mut seq,
                        &mut tool_core,
                        build_details_collect_request(
                            &discovered_tools,
                            &whitelist,
                            outcome.detail_tool_id,
                            outcome.force_detail_refresh,
                        ),
                    )
                    .await?;
                }
            }
            _ = heartbeat_ticker.tick() => {
                send_event(
                    &mut ws_writer,
                    &cfg.system_id,
                    &mut seq,
                    "heartbeat",
                    None,
                    json!({
                        "status": "ONLINE",
                        "latencyMs": 0,
                    }),
                ).await?;
            }
            _ = metrics_ticker.tick() => {
                discovered_tools = tool_core.discover_tools(&mut sys);
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
            _ = details_ticker.tick() => {
                refresh_and_send_details(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    &mut tool_core,
                    build_details_collect_request(&discovered_tools, &whitelist, None, false),
                )
                .await?;
            }
        }
    }
}

/// 基于当前发现结果和白名单，组装一次详情采集请求。
fn build_details_collect_request(
    discovered_tools: &[ToolRuntimePayload],
    whitelist: &ToolWhitelistStore,
    target_tool_id: Option<String>,
    force: bool,
) -> ToolDetailsCollectRequest {
    let tools = discovered_tools
        .iter()
        .filter(|tool| whitelist.contains_compatible(&tool.tool_id))
        .cloned()
        .collect::<Vec<ToolRuntimePayload>>();

    ToolDetailsCollectRequest {
        tools,
        target_tool_id,
        force,
    }
}

/// 刷新工具详情并发送 `tool_details_snapshot`。
async fn refresh_and_send_details(
    ws_writer: &mut command::RelayWriter,
    cfg: &Config,
    seq: &mut u64,
    tool_core: &mut ToolAdapterCore,
    request: ToolDetailsCollectRequest,
) -> Result<()> {
    let connected_tools = request.tools.clone();

    let details = tool_core.collect_details_snapshot(request).await;

    if details.is_empty() && connected_tools.is_empty() {
        return Ok(());
    }

    send_tool_details_snapshot(ws_writer, &cfg.system_id, seq, &details).await?;
    Ok(())
}

/// session 模块总入口，供 main 调用。
pub(crate) async fn run(cfg: Config) -> Result<()> {
    if let Err(err) = run_relay_loop(cfg).await {
        error!("relay loop exited: {err}");
        return Err(err);
    }
    Ok(())
}
