//! Relay 会话循环。

mod chat;
mod command;
mod report;
mod url;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use serde_json::json;
use sysinfo::System;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
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
        queue::{QueueKey, QueuePolicy, QueueScheduler},
        snapshots::{
            ToolDetailsSnapshotMeta, send_snapshots, send_tool_details_snapshot,
            summarize_wire_payload,
        },
        transport::send_event,
    },
    stores::{ControllerDevicesStore, ToolWhitelistStore},
    tooling::core::{ToolAdapterCore, types::ToolDetailsCollectRequest},
};
use yc_shared_protocol::{
    ToolDetailEnvelopePayload, ToolDetailsRefreshPriority, ToolDetailsSnapshotTrigger,
    ToolRuntimePayload,
};

#[derive(Debug, Clone)]
struct DetailsRefreshIntent {
    generation: u64,
    target_tool_id: Option<String>,
    force: bool,
    refresh_id: Option<String>,
    priority: ToolDetailsRefreshPriority,
    trigger: ToolDetailsSnapshotTrigger,
    queued_at: Instant,
    dropped_refreshes: u32,
}

#[derive(Debug)]
struct DetailsWorkerRequest {
    intent: DetailsRefreshIntent,
    collect_request: ToolDetailsCollectRequest,
}

#[derive(Debug)]
struct DetailsWorkerEvent {
    generation: u64,
    refresh_id: Option<String>,
    trigger: ToolDetailsSnapshotTrigger,
    target_tool_id: Option<String>,
    details: Vec<ToolDetailEnvelopePayload>,
    queue_wait_ms: u64,
    collect_ms: u64,
    dropped_refreshes: u32,
    connected_tools_count: usize,
}

/// 处理一条控制命令，并把详情刷新意图入队。
#[allow(clippy::too_many_arguments)]
async fn handle_command_envelope(
    ws_writer: &mut command::RelayWriter,
    cfg: &Config,
    seq: &mut u64,
    sys: &mut System,
    started_at: Instant,
    discover_core: &mut ToolAdapterCore,
    discovered_tools: &mut Vec<ToolRuntimePayload>,
    whitelist: &mut ToolWhitelistStore,
    controllers: &mut ControllerDevicesStore,
    chat_runtime: &mut ChatRuntime,
    chat_event_tx: &ChatEventSender,
    report_runtime: &mut ReportRuntime,
    report_event_tx: &ReportEventSender,
    command_envelope: SidecarCommandEnvelope,
    details_scheduler: &mut QueueScheduler<DetailsRefreshIntent>,
    latest_details_generation: &mut u64,
) -> Result<bool> {
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
        *discovered_tools = discover_core.discover_tools(sys);
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

    let mut dispatch_now = false;
    if outcome.refresh_details {
        enqueue_details_refresh(
            details_scheduler,
            latest_details_generation,
            outcome.detail_tool_id,
            outcome.force_detail_refresh,
            outcome.detail_refresh_id,
            outcome.detail_priority,
            outcome.detail_trigger,
        );
        dispatch_now = matches!(outcome.detail_priority, ToolDetailsRefreshPriority::User);
    }

    Ok(dispatch_now)
}

/// 判定是否应进入高优先级控制队列。
fn is_priority_command(command: &SidecarCommandEnvelope) -> bool {
    matches!(
        command.command,
        SidecarCommand::ToolChatRequest { .. }
            | SidecarCommand::ToolChatCancel { .. }
            | SidecarCommand::ToolReportFetchRequest { .. }
    )
}

/// 把详情刷新请求放入 latest-wins 队列，并累计被覆盖计数。
fn enqueue_details_refresh(
    scheduler: &mut QueueScheduler<DetailsRefreshIntent>,
    latest_generation: &mut u64,
    target_tool_id: Option<String>,
    force: bool,
    refresh_id: Option<String>,
    priority: ToolDetailsRefreshPriority,
    trigger: ToolDetailsSnapshotTrigger,
) {
    *latest_generation = latest_generation.saturating_add(1);
    let intent = DetailsRefreshIntent {
        generation: *latest_generation,
        target_tool_id,
        force,
        refresh_id,
        priority,
        trigger,
        queued_at: Instant::now(),
        dropped_refreshes: 0,
    };
    let report = scheduler.enqueue(QueueKey::ToolDetails, intent);
    if report.dropped > 0
        && let Some(latest) = scheduler.latest_mut(QueueKey::ToolDetails)
    {
        latest.dropped_refreshes = latest.dropped_refreshes.saturating_add(report.dropped);
    }
}

/// 从详情队列弹出一个请求并尝试派发给 worker。
fn dispatch_details_refresh(
    scheduler: &mut QueueScheduler<DetailsRefreshIntent>,
    details_req_tx: &mpsc::Sender<DetailsWorkerRequest>,
    discovered_tools: &[ToolRuntimePayload],
    whitelist: &ToolWhitelistStore,
) -> Result<()> {
    let Some((queue_key, intent)) = scheduler.pop_next() else {
        return Ok(());
    };
    if queue_key != QueueKey::ToolDetails {
        return Ok(());
    }

    let collect_request = build_details_collect_request(
        discovered_tools,
        whitelist,
        intent.target_tool_id.clone(),
        intent.force,
    );
    debug!(
        "dispatch details refresh generation={} priority={:?} trigger={:?} refresh_id={} target_tool_id={} force={} queue_depth={}",
        intent.generation,
        intent.priority,
        intent.trigger,
        intent.refresh_id.as_deref().unwrap_or_default(),
        intent.target_tool_id.as_deref().unwrap_or_default(),
        intent.force,
        scheduler.depth_for_key(QueueKey::ToolDetails),
    );
    let request = DetailsWorkerRequest {
        intent,
        collect_request,
    };

    match details_req_tx.try_send(request) {
        Ok(_) => Ok(()),
        Err(TrySendError::Full(request)) => {
            let report = scheduler.enqueue(QueueKey::ToolDetails, request.intent);
            if report.dropped > 0
                && let Some(latest) = scheduler.latest_mut(QueueKey::ToolDetails)
            {
                latest.dropped_refreshes = latest.dropped_refreshes.saturating_add(report.dropped);
            }
            Ok(())
        }
        Err(TrySendError::Closed(_)) => Err(anyhow!("details worker channel closed")),
    }
}

/// 默认队列策略：详情/快照类 latest-wins，控制串行，业务 FIFO。
fn default_queue_policies() -> HashMap<QueueKey, QueuePolicy> {
    HashMap::from([
        (QueueKey::ToolDetails, QueuePolicy::latest_wins()),
        (QueueKey::ToolsRefresh, QueuePolicy::latest_wins()),
        (QueueKey::Metrics, QueuePolicy::latest_wins()),
        (QueueKey::PairingBanner, QueuePolicy::latest_wins()),
        (QueueKey::Control, QueuePolicy::serialized(64)),
        (QueueKey::Chat, QueuePolicy::fifo(128)),
        (QueueKey::Report, QueuePolicy::fifo(64)),
    ])
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

    let startup_banner_cfg = cfg.clone();
    tokio::spawn(async move {
        refresh_pairing_banner(&startup_banner_cfg).await;
    });

    let (mut ws_writer, mut ws_reader) = ws_stream.split();
    let (high_cmd_tx, mut high_cmd_rx) = mpsc::unbounded_channel::<SidecarCommandEnvelope>();
    let (normal_cmd_tx, mut normal_cmd_rx) = mpsc::unbounded_channel::<SidecarCommandEnvelope>();
    let (chat_event_tx, mut chat_event_rx) = mpsc::unbounded_channel::<chat::ChatEventEnvelope>();
    let (report_event_tx, mut report_event_rx) =
        mpsc::unbounded_channel::<report::ReportEventEnvelope>();
    let (details_req_tx, mut details_req_rx) = mpsc::channel::<DetailsWorkerRequest>(8);
    let (details_event_tx, mut details_event_rx) = mpsc::unbounded_channel::<DetailsWorkerEvent>();
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
    let details_worker_cfg = cfg.clone();
    let mut details_worker = tokio::spawn(async move {
        let mut details_core = ToolAdapterCore::new(
            details_worker_cfg.fallback_tool,
            details_worker_cfg.details_interval,
            details_worker_cfg.details_command_timeout,
            details_worker_cfg.details_max_parallel,
            details_worker_cfg.details_refresh_debounce,
        );
        while let Some(first_request) = details_req_rx.recv().await {
            let mut active = first_request;
            let mut dropped_refreshes = active.intent.dropped_refreshes;
            while let Ok(next_request) = details_req_rx.try_recv() {
                dropped_refreshes = dropped_refreshes
                    .saturating_add(1)
                    .saturating_add(next_request.intent.dropped_refreshes);
                active = next_request;
            }
            active.intent.dropped_refreshes = dropped_refreshes;
            let queue_wait_ms = active
                .intent
                .queued_at
                .elapsed()
                .as_millis()
                .min(u64::MAX as u128) as u64;
            let generation = active.intent.generation;
            let refresh_id = active.intent.refresh_id.clone();
            let target_tool_id = active.intent.target_tool_id.clone();
            let trigger = active.intent.trigger;
            let connected_tools_count = active.collect_request.tools.len();

            let cache_details = details_core.cached_details_snapshot(&active.collect_request.tools);
            if !cache_details.is_empty() {
                let _ = details_event_tx.send(DetailsWorkerEvent {
                    generation,
                    refresh_id: refresh_id.clone(),
                    trigger: ToolDetailsSnapshotTrigger::Cache,
                    target_tool_id: target_tool_id.clone(),
                    details: cache_details,
                    queue_wait_ms,
                    collect_ms: 0,
                    dropped_refreshes,
                    connected_tools_count,
                });
            }

            let collect_started_at = Instant::now();
            let details = details_core
                .collect_details_snapshot(active.collect_request)
                .await;
            let collect_ms = collect_started_at
                .elapsed()
                .as_millis()
                .min(u64::MAX as u128) as u64;
            let _ = details_event_tx.send(DetailsWorkerEvent {
                generation,
                refresh_id,
                trigger,
                target_tool_id,
                details,
                queue_wait_ms,
                collect_ms,
                dropped_refreshes,
                connected_tools_count,
            });
        }
    });

    let mut seq = 0_u64;
    let mut details_snapshot_id = 0_u64;
    let started_at = Instant::now();
    let mut sys = System::new_all();
    let mut discover_core = ToolAdapterCore::new(
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
    let mut discovered_tools = discover_core.discover_tools(&mut sys);
    let mut details_scheduler =
        QueueScheduler::new(QueuePolicy::fifo(256), default_queue_policies());
    let mut latest_details_generation = 0_u64;

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
    enqueue_details_refresh(
        &mut details_scheduler,
        &mut latest_details_generation,
        None,
        true,
        None,
        ToolDetailsRefreshPriority::Background,
        ToolDetailsSnapshotTrigger::Command,
    );
    dispatch_details_refresh(
        &mut details_scheduler,
        &details_req_tx,
        &discovered_tools,
        &whitelist,
    )?;

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

    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                chat_runtime.abort_all();
                report_runtime.abort_all();
                details_worker.abort();
                return Ok(());
            },
            done = &mut reader_task => {
                chat_runtime.abort_all();
                report_runtime.abort_all();
                details_worker.abort();
                match done {
                    Ok(_) => return Err(anyhow!("relay read loop closed")),
                    Err(err) => return Err(anyhow!("relay read task join error: {err}")),
                }
            }
            done = &mut details_worker => {
                chat_runtime.abort_all();
                report_runtime.abort_all();
                match done {
                    Ok(_) => return Err(anyhow!("details worker exited unexpectedly")),
                    Err(err) => return Err(anyhow!("details worker join error: {err}")),
                }
            }
            maybe_cmd = high_cmd_rx.recv() => {
                let Some(command_envelope) = maybe_cmd else {
                    continue;
                };
                let dispatch_now = handle_command_envelope(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    &mut sys,
                    started_at,
                    &mut discover_core,
                    &mut discovered_tools,
                    &mut whitelist,
                    &mut controllers,
                    &mut chat_runtime,
                    &chat_event_tx,
                    &mut report_runtime,
                    &report_event_tx,
                    command_envelope,
                    &mut details_scheduler,
                    &mut latest_details_generation,
                )
                .await?;
                if dispatch_now {
                    dispatch_details_refresh(
                        &mut details_scheduler,
                        &details_req_tx,
                        &discovered_tools,
                        &whitelist,
                    )?;
                }
            }
            maybe_cmd = normal_cmd_rx.recv() => {
                let Some(command_envelope) = maybe_cmd else {
                    continue;
                };
                let dispatch_now = handle_command_envelope(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    &mut sys,
                    started_at,
                    &mut discover_core,
                    &mut discovered_tools,
                    &mut whitelist,
                    &mut controllers,
                    &mut chat_runtime,
                    &chat_event_tx,
                    &mut report_runtime,
                    &report_event_tx,
                    command_envelope,
                    &mut details_scheduler,
                    &mut latest_details_generation,
                )
                .await?;
                if dispatch_now {
                    dispatch_details_refresh(
                        &mut details_scheduler,
                        &details_req_tx,
                        &discovered_tools,
                        &whitelist,
                    )?;
                }
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
            maybe_details_event = details_event_rx.recv() => {
                let Some(details_event) = maybe_details_event else {
                    continue;
                };
                if details_event.generation < latest_details_generation {
                    debug!(
                        "drop stale details snapshot generation={} latest={} trigger={:?} refresh_id={} target_tool_id={}",
                        details_event.generation,
                        latest_details_generation,
                        details_event.trigger,
                        details_event.refresh_id.as_deref().unwrap_or_default(),
                        details_event.target_tool_id.as_deref().unwrap_or_default(),
                    );
                    continue;
                }
                if details_event.details.is_empty() && details_event.connected_tools_count == 0 {
                    continue;
                }

                details_snapshot_id = details_snapshot_id.saturating_add(1);
                let send_started_at = Instant::now();
                send_tool_details_snapshot(
                    &mut ws_writer,
                    &cfg.system_id,
                    &mut seq,
                    &details_event.details,
                    ToolDetailsSnapshotMeta {
                        snapshot_id: details_snapshot_id,
                        refresh_id: details_event.refresh_id.clone(),
                        trigger: details_event.trigger,
                        target_tool_id: details_event.target_tool_id.clone(),
                        queue_wait_ms: details_event.queue_wait_ms,
                        collect_ms: details_event.collect_ms,
                        send_ms: 0,
                        dropped_refreshes: details_event.dropped_refreshes,
                    },
                )
                .await?;
                let send_ms = send_started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
                debug!(
                    concat!(
                        "tool details snapshot sent snapshot_id={} generation={} trigger={:?} ",
                        "refresh_id={} target_tool_id={} details={} queue_wait_ms={} ",
                        "collect_ms={} send_ms={} dropped_refreshes={}"
                    ),
                    details_snapshot_id,
                    details_event.generation,
                    details_event.trigger,
                    details_event.refresh_id.as_deref().unwrap_or_default(),
                    details_event.target_tool_id.as_deref().unwrap_or_default(),
                    details_event.details.len(),
                    details_event.queue_wait_ms,
                    details_event.collect_ms,
                    send_ms,
                    details_event.dropped_refreshes,
                );
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
                discovered_tools = discover_core.discover_tools(&mut sys);
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
                let refresh_cfg = cfg.clone();
                tokio::spawn(async move {
                    refresh_pairing_banner(&refresh_cfg).await;
                });
            }
            _ = details_ticker.tick() => {
                enqueue_details_refresh(
                    &mut details_scheduler,
                    &mut latest_details_generation,
                    None,
                    false,
                    None,
                    ToolDetailsRefreshPriority::Background,
                    ToolDetailsSnapshotTrigger::Periodic,
                );
            }
            _ = details_dispatch_ticker.tick() => {
                dispatch_details_refresh(
                    &mut details_scheduler,
                    &details_req_tx,
                    &discovered_tools,
                    &whitelist,
                )?;
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

/// session 模块总入口，供 main 调用。
pub(crate) async fn run(cfg: Config) -> Result<()> {
    if let Err(err) = run_relay_loop(cfg).await {
        error!("relay loop exited: {err}");
        return Err(err);
    }
    Ok(())
}
