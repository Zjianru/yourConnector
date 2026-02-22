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
use tracing::{error, info, warn};

use self::{
    command::handle_sidecar_command,
    url::{raw_payload_logging_enabled, sidecar_ws_url},
};
use crate::{
    config::Config,
    control::{SidecarCommandEnvelope, parse_sidecar_command},
    discover_tools,
    pairing::{banner::print_pairing_banner, bootstrap_client::fetch_pair_bootstrap},
    session::{
        snapshots::{send_snapshots, summarize_wire_payload},
        transport::send_event,
    },
    stores::{ControllerDevicesStore, ToolWhitelistStore},
};

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

                let should_refresh = handle_sidecar_command(
                    &mut ws_writer,
                    cfg,
                    &mut seq,
                    command_envelope,
                    &discovered_tools,
                    &mut whitelist,
                    &mut controllers,
                ).await?;

                if should_refresh {
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

/// session 模块总入口，供 main 调用。
pub(crate) async fn run(cfg: Config) -> Result<()> {
    if let Err(err) = run_relay_loop(cfg).await {
        error!("relay loop exited: {err}");
        return Err(err);
    }
    Ok(())
}
