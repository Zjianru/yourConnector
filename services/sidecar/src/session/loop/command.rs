//! Sidecar 控制命令处理。

use anyhow::Result;
use futures_util::stream::SplitSink;
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tracing::info;
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

/// 处理一条 sidecar 控制命令。
/// 返回值：`true` 表示命令后需要刷新快照；`false` 表示无需刷新。
pub(crate) async fn handle_sidecar_command(
    ws_writer: &mut RelayWriter,
    cfg: &Config,
    seq: &mut u64,
    command_envelope: SidecarCommandEnvelope,
    discovered_tools: &[ToolRuntimePayload],
    whitelist: &mut ToolWhitelistStore,
    controllers: &mut ControllerDevicesStore,
) -> Result<bool> {
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
            json!({
                "ok": ok,
                "changed": changed,
                "deviceId": device,
                "reason": reason,
            }),
        )
        .await?;

        return Ok(false);
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
            json!({
                "action": action,
                "toolId": tool_id,
                "ok": false,
                "changed": false,
                "reason": allow_reason,
            }),
        )
        .await?;
        return Ok(false);
    }

    match command_envelope.command {
        SidecarCommand::Refresh => {}
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
                json!({
                    "action": "connect",
                    "toolId": tool_id,
                    "ok": ok,
                    "changed": changed,
                    "reason": reason,
                }),
            )
            .await?;
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
                json!({
                    "action": "disconnect",
                    "toolId": tool_id,
                    "ok": ok,
                    "changed": changed,
                    "reason": reason,
                }),
            )
            .await?;
        }
        SidecarCommand::RebindController { .. } => {}
    }

    Ok(true)
}
