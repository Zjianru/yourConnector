//! Relay 会话循环。

mod chat;
mod command;
mod report;
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
    chat::{ChatEventSender, ChatRuntime},
    command::{SidecarCommandContext, handle_sidecar_command},
    report::{ReportEventSender, ReportRuntime},
    url::{raw_payload_logging_enabled, sidecar_ws_url},
};
use crate::{
    config::Config,
    control::{SidecarCommand, SidecarCommandEnvelope, parse_sidecar_command},
    pairing::{banner::print_pairing_banner, bootstrap_client::fetch_pair_bootstrap},
    session::{
        snapshots::{send_snapshots, send_tool_details_snapshot, summarize_wire_payload},
        transport::send_event,
    },
    stores::{ControllerDevicesStore, ToolWhitelistStore},
    tooling::core::{ToolAdapterCore, types::ToolDetailsCollectRequest},
};
use yc_shared_protocol::ToolRuntimePayload;

#[derive(Debug, Clone, Default)]
struct PendingDetailsRefresh {
    target_tool_id: Option<String>,
    force: bool,
    dirty: bool,
}

fn is_priority_command(command: &SidecarCommandEnvelope) -> bool {
    matches!(
        command.command,
        SidecarCommand::ToolChatRequest { .. }
            | SidecarCommand::ToolChatCancel { .. }
            | SidecarCommand::ToolReportFetchRequest { .. }
    )
}

fn merge_pending_details_refresh(
    pending: &mut PendingDetailsRefresh,
    target_tool_id: Option<String>,
    force: bool,
) {
    if !pending.dirty {
        pending.target_tool_id = target_tool_id;
        pending.force = force;
        pending.dirty = true;
        return;
    }

    pending.force = pending.force || force;
    match (&pending.target_tool_id, &target_tool_id) {
        (_, None) => pending.target_tool_id = None,
        (None, Some(_)) => {}
        (Some(existing), Some(next)) => {
            if existing != next {
                pending.target_tool_id = None;
            }
        }
    }
    pending.dirty = true;
}

async fn handle_command_envelope(
    ws_writer: &mut command::RelayWriter,
    cfg: &Config,
    seq: &mut u64,
    sys: &mut System,
    started_at: Instant,
    tool_core: &mut ToolAdapterCore,
    discovered_tools: &mut Vec<ToolRuntimePayload>,
    whitelist: &mut ToolWhitelistStore,
    controllers: &mut ControllerDevicesStore,
    chat_runtime: &mut ChatRuntime,
    chat_event_tx: &ChatEventSender,
    report_runtime: &mut ReportRuntime,
    report_event_tx: &ReportEventSender,
    command_envelope: SidecarCommandEnvelope,
    pending_details_refresh: &mut PendingDetailsRefresh,
) -> Result<()> {
    let outcome = handle_sidecar_command(
        SidecarCommandContext {
            ws_writer,
            cfg,
            seq,
            discovered_tools,
            whitelist,
            controllers,
            chat_runtime,
            chat_event_tx,
            report_runtime,
            report_event_tx,
        },
        command_envelope,
    )
    .await?;

    if outcome.refresh_snapshots {
        *discovered_tools = tool_core.discover_tools(sys);
        send_snapshots(
            ws_writer,
            cfg,
            seq,
            sys,
            started_at,
            discovered_tools,
            whitelist,
        )
        .await?;
    }
    if outcome.refresh_details {
        merge_pending_details_refresh(
            pending_details_refresh,
            outcome.detail_tool_id,
            outcome.force_detail_refresh,
        );
    }

    Ok(())
}

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

/// 拉取并打印最新配对 banner（短时票据 + 深链）。
async fn refresh_pairing_banner(cfg: &Config) {
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
}

/// 单次 relay 会话：连接、收命令、推送心跳与快照，直到连接中断。
async fn run_session(cfg: &Config) -> Result<()> {
    let ws_url = sidecar_ws_url(cfg)?;
    info!("connecting relay {}", ws_url);

    let (ws_stream, _) = connect_async(ws_url.as_str()).await?;
    info!("relay connected");

    refresh_pairing_banner(cfg).await;

    let (mut ws_writer, mut ws_reader) = ws_stream.split();
    let (high_cmd_tx, mut high_cmd_rx) = mpsc::unbounded_channel::<SidecarCommandEnvelope>();
    let (normal_cmd_tx, mut normal_cmd_rx) = mpsc::unbounded_channel::<SidecarCommandEnvelope>();
    let (chat_event_tx, mut chat_event_rx) = mpsc::unbounded_channel::<chat::ChatEventEnvelope>();
    let (report_event_tx, mut report_event_rx) =
        mpsc::unbounded_channel::<report::ReportEventEnvelope>();
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
                        let target = if is_priority_command(&command) {
                            &high_cmd_tx
                        } else {
                            &normal_cmd_tx
                        };
                        if target.send(command).is_err() {
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
    let mut chat_runtime = ChatRuntime::default();
    let mut report_runtime = ReportRuntime::default();
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
    let mut pairing_banner_ticker = tokio::time::interval(cfg.pairing_banner_refresh_interval);
    pairing_banner_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // 跳过 interval 的首次“立即触发”，避免连接后重复打印两次 banner。
    pairing_banner_ticker.tick().await;
    let mut details_ticker = tokio::time::interval(cfg.details_interval);
    details_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let details_dispatch_interval = cfg.details_refresh_debounce.max(Duration::from_millis(200));
    let mut details_dispatch_ticker = tokio::time::interval(details_dispatch_interval);
    details_dispatch_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // 跳过首次立即触发，避免连接瞬间重复跑一次详情。
    details_dispatch_ticker.tick().await;
    let mut pending_details_refresh = PendingDetailsRefresh::default();

    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                chat_runtime.abort_all();
                report_runtime.abort_all();
                return Ok(());
            },
            done = &mut reader_task => {
                chat_runtime.abort_all();
                report_runtime.abort_all();
                match done {
                    Ok(_) => return Err(anyhow!("relay read loop closed")),
                    Err(err) => return Err(anyhow!("relay read task join error: {err}")),
                }
            }
            maybe_cmd = high_cmd_rx.recv() => {
                let Some(command_envelope) = maybe_cmd else {
                    continue;
                };
                handle_command_envelope(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    &mut sys,
                    started_at,
                    &mut tool_core,
                    &mut discovered_tools,
                    &mut whitelist,
                    &mut controllers,
                    &mut chat_runtime,
                    &chat_event_tx,
                    &mut report_runtime,
                    &report_event_tx,
                    command_envelope,
                    &mut pending_details_refresh,
                )
                .await?;
            }
            maybe_cmd = normal_cmd_rx.recv() => {
                let Some(command_envelope) = maybe_cmd else {
                    continue;
                };
                handle_command_envelope(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    &mut sys,
                    started_at,
                    &mut tool_core,
                    &mut discovered_tools,
                    &mut whitelist,
                    &mut controllers,
                    &mut chat_runtime,
                    &chat_event_tx,
                    &mut report_runtime,
                    &report_event_tx,
                    command_envelope,
                    &mut pending_details_refresh,
                )
                .await?;
            }
            maybe_chat_event = chat_event_rx.recv() => {
                let Some(chat_event) = maybe_chat_event else {
                    continue;
                };
                if let Some(finalize_key) = chat_event.finalize.as_ref() {
                    chat_runtime.mark_finished(finalize_key);
                }
                send_event(
                    &mut ws_writer,
                    &cfg.system_id,
                    &mut seq,
                    chat_event.event_type,
                    chat_event.trace_id.as_deref(),
                    chat_event.payload,
                ).await?;
            }
            maybe_report_event = report_event_rx.recv() => {
                let Some(report_event) = maybe_report_event else {
                    continue;
                };
                if let Some(finalize_key) = report_event.finalize.as_ref() {
                    report_runtime.mark_finished(finalize_key);
                }
                send_event(
                    &mut ws_writer,
                    &cfg.system_id,
                    &mut seq,
                    report_event.event_type,
                    report_event.trace_id.as_deref(),
                    report_event.payload,
                ).await?;
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
            _ = pairing_banner_ticker.tick() => {
                refresh_pairing_banner(cfg).await;
            }
            _ = details_ticker.tick() => {
                merge_pending_details_refresh(&mut pending_details_refresh, None, false);
            }
            _ = details_dispatch_ticker.tick() => {
                if !pending_details_refresh.dirty {
                    continue;
                }
                let target_tool_id = pending_details_refresh.target_tool_id.clone();
                let force_refresh = pending_details_refresh.force;
                pending_details_refresh = PendingDetailsRefresh::default();
                refresh_and_send_details(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    &mut tool_core,
                    build_details_collect_request(
                        &discovered_tools,
                        &whitelist,
                        target_tool_id,
                        force_refresh,
                    ),
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
