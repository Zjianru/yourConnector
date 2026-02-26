//! Sidecar 控制命令处理。

use anyhow::Result;
use futures_util::stream::SplitSink;
use serde_json::json;
use std::{
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::net::TcpStream;
use tokio::{process::Command, time::sleep};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tracing::{debug, info};
use yc_shared_protocol::ToolRuntimePayload;

use crate::{
    config::Config,
    control::{
        CONTROLLER_BIND_UPDATED_EVENT, SidecarCommand, SidecarCommandEnvelope,
        TOOL_CHAT_FINISHED_EVENT, TOOL_PROCESS_CONTROL_UPDATED_EVENT,
        TOOL_REPORT_FETCH_FINISHED_EVENT, TOOL_WHITELIST_UPDATED_EVENT, ToolProcessAction,
        command_feedback_event, command_feedback_parts,
    },
    session::{snapshots::is_fallback_tool, transport::send_event},
    stores::{ControllerDevicesStore, ToolWhitelistStore},
    tooling::adapters::{openclaw, opencode},
};

use super::chat::{
    CancelChatOutcome, ChatCancelInput, ChatEventSender, ChatRequestInput, ChatRuntime,
    StartChatOutcome,
};
use super::report::{ReportEventSender, ReportRequestInput, ReportRuntime, StartReportOutcome};

/// Relay WebSocket 写端类型别名。
pub(crate) type RelayWriter = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

/// sidecar 命令处理上下文。
pub(crate) struct SidecarCommandContext<'a> {
    pub(crate) ws_writer: &'a mut RelayWriter,
    pub(crate) cfg: &'a Config,
    pub(crate) seq: &'a mut u64,
    pub(crate) discovered_tools: &'a [ToolRuntimePayload],
    pub(crate) whitelist: &'a mut ToolWhitelistStore,
    pub(crate) controllers: &'a mut ControllerDevicesStore,
    pub(crate) chat_runtime: &'a mut ChatRuntime,
    pub(crate) chat_event_tx: &'a ChatEventSender,
    pub(crate) report_runtime: &'a mut ReportRuntime,
    pub(crate) report_event_tx: &'a ReportEventSender,
}

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

/// 进程停止结果。
#[derive(Debug, Clone)]
struct StopResult {
    /// 停止是否成功。
    ok: bool,
    /// 是否发生状态变更（true 表示原进程被停止）。
    changed: bool,
    /// 说明文本。
    reason: String,
}

/// 进程重启所需的最小启动规格。
#[derive(Debug, Clone)]
struct LaunchSpec {
    /// 启动命令及参数（argv）。
    argv: Vec<String>,
    /// 启动工作目录（可选）。
    cwd: Option<PathBuf>,
}

/// 处理一条 sidecar 控制命令，并返回后续刷新意图。
pub(crate) async fn handle_sidecar_command(
    ctx: SidecarCommandContext<'_>,
    command_envelope: SidecarCommandEnvelope,
) -> Result<SidecarCommandOutcome> {
    let SidecarCommandContext {
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
    } = ctx;

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
        match &command_envelope.command {
            SidecarCommand::ToolChatRequest {
                tool_id,
                conversation_key,
                request_id,
                queue_item_id,
                ..
            }
            | SidecarCommand::ToolChatCancel {
                tool_id,
                conversation_key,
                request_id,
                queue_item_id,
            } => {
                send_event(
                    ws_writer,
                    &cfg.system_id,
                    seq,
                    TOOL_CHAT_FINISHED_EVENT,
                    trace_id.as_deref(),
                    json!({
                        "toolId": tool_id,
                        "conversationKey": conversation_key,
                        "requestId": request_id,
                        "queueItemId": queue_item_id,
                        "status": "failed",
                        "text": "",
                        "reason": allow_reason,
                        "meta": {},
                    }),
                )
                .await?;
                return Ok(SidecarCommandOutcome::default());
            }
            SidecarCommand::ToolReportFetchRequest {
                tool_id,
                conversation_key,
                request_id,
                file_path,
            } => {
                send_event(
                    ws_writer,
                    &cfg.system_id,
                    seq,
                    TOOL_REPORT_FETCH_FINISHED_EVENT,
                    trace_id.as_deref(),
                    json!({
                        "toolId": tool_id,
                        "conversationKey": conversation_key,
                        "requestId": request_id,
                        "filePath": file_path,
                        "status": "failed",
                        "reason": allow_reason,
                        "bytesSent": 0,
                        "bytesTotal": 0,
                    }),
                )
                .await?;
                return Ok(SidecarCommandOutcome::default());
            }
            _ => {}
        }

        let (action, tool_id) = command_feedback_parts(&command_envelope.command);
        let response_event = command_feedback_event(&command_envelope.command);
        send_event(
            ws_writer,
            &cfg.system_id,
            seq,
            response_event,
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
        SidecarCommand::ResetToolWhitelist => {
            let (ok, changed, reason, removed_count) = match whitelist.clear() {
                Ok(removed) => (true, removed > 0, String::new(), removed),
                Err(err) => (false, false, format!("清空白名单失败: {err}"), 0),
            };

            send_event(
                ws_writer,
                &cfg.system_id,
                seq,
                TOOL_WHITELIST_UPDATED_EVENT,
                trace_id.as_deref(),
                json!({
                    "action": "reset",
                    "toolId": "",
                    "ok": ok,
                    "changed": changed,
                    "removedCount": removed_count,
                    "reason": reason,
                }),
            )
            .await?;

            SidecarCommandOutcome::snapshots_and_details()
        }
        SidecarCommand::RefreshToolDetails { tool_id, force } => {
            SidecarCommandOutcome::details_only(tool_id, force)
        }
        SidecarCommand::ControlToolProcess { tool_id, action } => {
            let candidate = discovered_tools.iter().find(|tool| tool.tool_id == tool_id);
            let pid = candidate.and_then(|tool| tool.pid);
            let (ok, changed, reason, new_pid) = match candidate {
                None => (
                    false,
                    false,
                    "工具不在当前列表，无法执行进程控制。".to_string(),
                    None,
                ),
                Some(tool) => {
                    let is_openclaw = openclaw::matches_tool(tool);
                    let is_opencode = opencode::matches_tool(tool);
                    if !is_openclaw && !is_opencode {
                        (
                            false,
                            false,
                            "当前仅支持对 OpenClaw 与代码工具执行进程控制。".to_string(),
                            None,
                        )
                    } else {
                        match action {
                            ToolProcessAction::Stop => {
                                if let Some(pid_value) = tool.pid {
                                    let result = stop_process(pid_value).await;
                                    (result.ok, result.changed, result.reason, None)
                                } else {
                                    (false, false, "未找到可控制的进程 PID。".to_string(), None)
                                }
                            }
                            ToolProcessAction::Restart => {
                                if !is_openclaw {
                                    (
                                        false,
                                        false,
                                        "代码工具当前仅支持停止；重启请手动拉起新进程。"
                                            .to_string(),
                                        None,
                                    )
                                } else if let Some(pid_value) = tool.pid {
                                    match restart_process(pid_value).await {
                                        Ok((changed, new_pid)) => (
                                            true,
                                            changed,
                                            if let Some(new_pid_value) = new_pid {
                                                format!(
                                                    "已重启 OpenClaw 进程，新 PID: {new_pid_value}"
                                                )
                                            } else {
                                                "已重启 OpenClaw 进程。".to_string()
                                            },
                                            new_pid,
                                        ),
                                        Err(err) => (false, false, err, None),
                                    }
                                } else {
                                    (false, false, "未找到可控制的进程 PID。".to_string(), None)
                                }
                            }
                        }
                    }
                }
            };

            send_event(
                ws_writer,
                &cfg.system_id,
                seq,
                TOOL_PROCESS_CONTROL_UPDATED_EVENT,
                trace_id.as_deref(),
                json!({
                    "action": action.as_str(),
                    "toolId": tool_id,
                    "ok": ok,
                    "changed": changed,
                    "reason": reason,
                    "pid": pid,
                    "newPid": new_pid,
                }),
            )
            .await?;

            if ok {
                SidecarCommandOutcome::snapshots_and_details()
            } else {
                SidecarCommandOutcome::default()
            }
        }
        SidecarCommand::ToolChatRequest {
            tool_id,
            conversation_key,
            request_id,
            queue_item_id,
            text,
        } => {
            let tool = discovered_tools
                .iter()
                .find(|item| item.tool_id == tool_id)
                .cloned();
            let Some(target_tool) = tool else {
                send_event(
                    ws_writer,
                    &cfg.system_id,
                    seq,
                    TOOL_CHAT_FINISHED_EVENT,
                    trace_id.as_deref(),
                    json!({
                        "toolId": tool_id,
                        "conversationKey": conversation_key,
                        "requestId": request_id,
                        "queueItemId": queue_item_id,
                        "status": "failed",
                        "text": "",
                        "reason": "工具未在线或未接入，无法发起聊天。",
                        "meta": {},
                    }),
                )
                .await?;
                return Ok(SidecarCommandOutcome::default());
            };

            let start = chat_runtime.start_request(
                ChatRequestInput {
                    tool_id: tool_id.clone(),
                    conversation_key: conversation_key.clone(),
                    request_id: request_id.clone(),
                    queue_item_id: queue_item_id.clone(),
                    text,
                },
                target_tool,
                trace_id.clone(),
                chat_event_tx.clone(),
            );

            match start {
                StartChatOutcome::Started => SidecarCommandOutcome::default(),
                StartChatOutcome::Busy { reason } => {
                    send_event(
                        ws_writer,
                        &cfg.system_id,
                        seq,
                        TOOL_CHAT_FINISHED_EVENT,
                        trace_id.as_deref(),
                        json!({
                            "toolId": tool_id,
                            "conversationKey": conversation_key,
                            "requestId": request_id,
                            "queueItemId": queue_item_id,
                            "status": "busy",
                            "text": "",
                            "reason": reason,
                            "meta": {},
                        }),
                    )
                    .await?;
                    SidecarCommandOutcome::default()
                }
            }
        }
        SidecarCommand::ToolChatCancel {
            tool_id,
            conversation_key,
            request_id,
            queue_item_id,
        } => {
            match chat_runtime.cancel_request(&ChatCancelInput {
                tool_id: tool_id.clone(),
                conversation_key: conversation_key.clone(),
                request_id: request_id.clone(),
                queue_item_id: queue_item_id.clone(),
            }) {
                CancelChatOutcome::Accepted => SidecarCommandOutcome::default(),
                CancelChatOutcome::NotFound => {
                    send_event(
                        ws_writer,
                        &cfg.system_id,
                        seq,
                        TOOL_CHAT_FINISHED_EVENT,
                        trace_id.as_deref(),
                        json!({
                            "toolId": tool_id,
                            "conversationKey": conversation_key,
                            "requestId": request_id,
                            "queueItemId": queue_item_id,
                            "status": "failed",
                            "text": "",
                            "reason": "未找到可取消的运行中请求。",
                            "meta": {},
                        }),
                    )
                    .await?;
                    SidecarCommandOutcome::default()
                }
            }
        }
        SidecarCommand::ToolReportFetchRequest {
            tool_id,
            conversation_key,
            request_id,
            file_path,
        } => {
            let tool = discovered_tools
                .iter()
                .find(|item| item.tool_id == tool_id)
                .cloned();
            let Some(target_tool) = tool else {
                send_event(
                    ws_writer,
                    &cfg.system_id,
                    seq,
                    TOOL_REPORT_FETCH_FINISHED_EVENT,
                    trace_id.as_deref(),
                    json!({
                        "toolId": tool_id,
                        "conversationKey": conversation_key,
                        "requestId": request_id,
                        "filePath": file_path,
                        "status": "failed",
                        "reason": "工具未在线或未接入，无法读取报告。",
                        "bytesSent": 0,
                        "bytesTotal": 0,
                    }),
                )
                .await?;
                return Ok(SidecarCommandOutcome::default());
            };

            let start = report_runtime.start_request(
                ReportRequestInput {
                    tool_id: tool_id.clone(),
                    conversation_key: conversation_key.clone(),
                    request_id: request_id.clone(),
                    file_path: file_path.clone(),
                },
                target_tool,
                trace_id.clone(),
                report_event_tx.clone(),
            );

            match start {
                StartReportOutcome::Started => SidecarCommandOutcome::default(),
                StartReportOutcome::Busy { reason } => {
                    send_event(
                        ws_writer,
                        &cfg.system_id,
                        seq,
                        TOOL_REPORT_FETCH_FINISHED_EVENT,
                        trace_id.as_deref(),
                        json!({
                            "toolId": tool_id,
                            "conversationKey": conversation_key,
                            "requestId": request_id,
                            "filePath": file_path,
                            "status": "busy",
                            "reason": reason,
                            "bytesSent": 0,
                            "bytesTotal": 0,
                        }),
                    )
                    .await?;
                    SidecarCommandOutcome::default()
                }
            }
        }
        SidecarCommand::RebindController { .. } => SidecarCommandOutcome::default(),
    };

    Ok(outcome)
}

/// 尝试优雅停止进程；超时后自动升级为强制停止。
async fn stop_process(pid: i32) -> StopResult {
    if !is_pid_running(pid) {
        return StopResult {
            ok: true,
            changed: false,
            reason: "进程已是停止状态。".to_string(),
        };
    }

    let pid_text = pid.to_string();
    match Command::new("kill")
        .args(["-TERM", pid_text.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
    {
        Ok(status) if status.success() => {}
        Ok(_) => {
            if !is_pid_running(pid) {
                return StopResult {
                    ok: true,
                    changed: true,
                    reason: "进程已停止。".to_string(),
                };
            }
            return StopResult {
                ok: false,
                changed: false,
                reason: "发送停止信号失败（kill -TERM 返回非 0）。".to_string(),
            };
        }
        Err(err) => {
            return StopResult {
                ok: false,
                changed: false,
                reason: format!("发送停止信号失败: {err}"),
            };
        }
    }

    if wait_process_exit(pid, Duration::from_secs(3)).await {
        return StopResult {
            ok: true,
            changed: true,
            reason: "进程已停止。".to_string(),
        };
    }

    match Command::new("kill")
        .args(["-KILL", pid_text.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
    {
        Ok(status) if status.success() => {}
        Ok(_) => {
            if !is_pid_running(pid) {
                return StopResult {
                    ok: true,
                    changed: true,
                    reason: "进程已停止。".to_string(),
                };
            }
            return StopResult {
                ok: false,
                changed: false,
                reason: "强制停止失败（kill -KILL 返回非 0）。".to_string(),
            };
        }
        Err(err) => {
            return StopResult {
                ok: false,
                changed: false,
                reason: format!("强制停止失败: {err}"),
            };
        }
    }

    if wait_process_exit(pid, Duration::from_secs(2)).await {
        return StopResult {
            ok: true,
            changed: true,
            reason: "进程已停止（TERM 超时后执行强制停止）。".to_string(),
        };
    }

    StopResult {
        ok: false,
        changed: false,
        reason: "停止超时，进程仍在运行。".to_string(),
    }
}

/// 使用同一命令与工作目录重启进程。
async fn restart_process(pid: i32) -> std::result::Result<(bool, Option<i32>), String> {
    let launch_spec =
        capture_launch_spec(pid).ok_or_else(|| "读取进程启动参数失败，无法重启。".to_string())?;
    let stop_result = stop_process(pid).await;
    if !stop_result.ok {
        return Err(stop_result.reason);
    }
    if launch_spec.argv.is_empty() {
        return Err("缺少可执行命令，无法重启。".to_string());
    }

    let mut command = Command::new(&launch_spec.argv[0]);
    if launch_spec.argv.len() > 1 {
        command.args(&launch_spec.argv[1..]);
    }
    if let Some(cwd) = launch_spec.cwd {
        command.current_dir(cwd);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command
        .spawn()
        .map_err(|err| format!("重启失败：拉起新进程失败: {err}"))?;
    let new_pid = child.id().and_then(|value| i32::try_from(value).ok());
    Ok((true, new_pid))
}

/// 从当前 PID 读取命令行与工作目录，用于 restart。
fn capture_launch_spec(pid: i32) -> Option<LaunchSpec> {
    let pid_u32 = u32::try_from(pid).ok()?;
    let mut sys = System::new_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let process = sys.process(Pid::from_u32(pid_u32))?;
    let argv = process
        .cmd()
        .iter()
        .map(|item| item.to_string_lossy().to_string())
        .filter(|item| !item.trim().is_empty())
        .collect::<Vec<String>>();
    if argv.is_empty() {
        return None;
    }
    let cwd = process.cwd().map(|value| value.to_path_buf());
    Some(LaunchSpec { argv, cwd })
}

/// 轮询等待 PID 退出。
async fn wait_process_exit(pid: i32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !is_pid_running(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(200)).await;
    }
}

/// 判断指定 PID 是否仍然存在。
fn is_pid_running(pid: i32) -> bool {
    let Ok(pid_u32) = u32::try_from(pid) else {
        return false;
    };
    let mut sys = System::new_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.process(Pid::from_u32(pid_u32)).is_some()
}
