//! Sidecar 控制命令处理。

use anyhow::Result;
use futures_util::stream::SplitSink;
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tracing::{debug, info};
use yc_shared_protocol::ToolRuntimePayload;

use crate::{
    config::Config,
    control::{
        CONTROLLER_BIND_UPDATED_EVENT, SidecarCommand, SidecarCommandEnvelope,
        TOOL_WHITELIST_UPDATED_EVENT, command_feedback_parts,
    },
    session::{snapshots::is_fallback_tool, transport::send_event},
    stores::{ControllerDevicesStore, ToolWhitelistStore},
};

/// Relay WebSocket 写端类型别名。
pub(crate) type RelayWriter = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

/// sidecar 命令处理结果：声明后续是否需要刷新快照/详情。
#[derive(Debug, Clone, Default)]
pub(crate) struct SidecarCommandOutcome {
    /// 是否需要刷新 tools_snapshot / metrics_snapshot。
    pub(crate) refresh_snapshots: bool,
    /// 是否需要刷新 tool_details_snapshot。
    pub(crate) refresh_details: bool,
    /// 详情刷新目标工具；为空表示刷新全部已接入工具。
    pub(crate) detail_tool_id: Option<String>,
    /// 是否强制刷新详情（忽略去抖）。
    pub(crate) force_detail_refresh: bool,
}

impl SidecarCommandOutcome {
    /// 刷新快照与详情。
    fn snapshots_and_details() -> Self {
        Self {
            refresh_snapshots: true,
            refresh_details: true,
            detail_tool_id: None,
            force_detail_refresh: true,
        }
    }

    /// 仅刷新详情。
    fn details_only(detail_tool_id: Option<String>, force: bool) -> Self {
        Self {
            refresh_snapshots: false,
            refresh_details: true,
            detail_tool_id,
            force_detail_refresh: force,
        }
    }
}

/// 处理一条 sidecar 控制命令，并返回后续刷新意图。
pub(crate) async fn handle_sidecar_command(
    ws_writer: &mut RelayWriter,
    cfg: &Config,
    seq: &mut u64,
    command_envelope: SidecarCommandEnvelope,
    discovered_tools: &[ToolRuntimePayload],
    whitelist: &mut ToolWhitelistStore,
    controllers: &mut ControllerDevicesStore,
) -> Result<SidecarCommandOutcome> {
    let trace_id = if command_envelope.trace_id.trim().is_empty() {
        None
    } else {
        Some(command_envelope.trace_id.clone())
    };
    debug!(
        "handle command type={} event_id={} trace_id={} source_type={} source_device={}",
        command_envelope.event_type,
        command_envelope.event_id,
        command_envelope.trace_id,
        command_envelope.source_client_type,
        command_envelope.source_device_id
    );

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
                Err(err) => (false, false, format!("重绑控制设备失败: {err}")),
            }
        };

        send_event(
            ws_writer,
            &cfg.system_id,
            seq,
            CONTROLLER_BIND_UPDATED_EVENT,
            trace_id.as_deref(),
            json!({
                "ok": ok,
                "changed": changed,
                "deviceId": device,
                "reason": reason,
            }),
        )
        .await?;

        return Ok(SidecarCommandOutcome::default());
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
            ws_writer,
            &cfg.system_id,
            seq,
            TOOL_WHITELIST_UPDATED_EVENT,
            trace_id.as_deref(),
            json!({
                "action": action,
                "toolId": tool_id,
                "ok": false,
                "changed": false,
                "reason": allow_reason,
            }),
        )
        .await?;
        return Ok(SidecarCommandOutcome::default());
    }

    let outcome = match command_envelope.command {
        SidecarCommand::Refresh => SidecarCommandOutcome::snapshots_and_details(),
        SidecarCommand::ConnectTool { tool_id } => {
            let candidate = discovered_tools.iter().find(|tool| tool.tool_id == tool_id);
            let (ok, changed, reason) = if candidate.is_none() {
                (false, false, "工具不在当前候选列表，无法接入。".to_string())
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
                    Err(err) => (false, false, format!("更新白名单失败: {err}")),
                }
            };

            send_event(
                ws_writer,
                &cfg.system_id,
                seq,
                TOOL_WHITELIST_UPDATED_EVENT,
                trace_id.as_deref(),
                json!({
                    "action": "connect",
                    "toolId": tool_id,
                    "ok": ok,
                    "changed": changed,
                    "reason": reason,
                }),
            )
            .await?;

            SidecarCommandOutcome::snapshots_and_details()
        }
        SidecarCommand::DisconnectTool { tool_id } => {
            let (ok, changed, reason) = match whitelist.remove(&tool_id) {
                Ok(changed) => (true, changed, String::new()),
                Err(err) => (false, false, format!("更新白名单失败: {err}")),
            };

            send_event(
                ws_writer,
                &cfg.system_id,
                seq,
                TOOL_WHITELIST_UPDATED_EVENT,
                trace_id.as_deref(),
                json!({
                    "action": "disconnect",
                    "toolId": tool_id,
                    "ok": ok,
                    "changed": changed,
                    "reason": reason,
                }),
            )
            .await?;

            SidecarCommandOutcome::snapshots_and_details()
        }
        SidecarCommand::RefreshToolDetails { tool_id, force } => {
            SidecarCommandOutcome::details_only(tool_id, force)
        }
        SidecarCommand::RebindController { .. } => SidecarCommandOutcome::default(),
    };

    Ok(outcome)
}
