//! Sidecar 控制命令处理。

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose};
use chrono::{Duration as ChronoDuration, Utc};
use futures_util::stream::SplitSink;
use serde_json::json;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::net::TcpStream;
use tokio::{process::Command, time::sleep};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tracing::{debug, info};
use yc_shared_protocol::{
    ToolDetailsRefreshPriority, ToolDetailsSnapshotTrigger, ToolRuntimePayload,
};

use crate::{
    config::Config,
    control::{
        CONTROLLER_BIND_UPDATED_EVENT, SidecarCommand, SidecarCommandEnvelope,
        TOOL_CHAT_FINISHED_EVENT, TOOL_LAUNCH_FAILED_EVENT, TOOL_LAUNCH_FINISHED_EVENT,
        TOOL_LAUNCH_STARTED_EVENT, TOOL_MEDIA_STAGE_FAILED_EVENT, TOOL_MEDIA_STAGE_FINISHED_EVENT,
        TOOL_MEDIA_STAGE_PROGRESS_EVENT, TOOL_PROCESS_CONTROL_UPDATED_EVENT,
        TOOL_REPORT_FETCH_FINISHED_EVENT, TOOL_WHITELIST_UPDATED_EVENT, ToolProcessAction,
        command_feedback_event, command_feedback_parts,
    },
    session::{snapshots::is_fallback_tool, transport::send_event},
    stores::{ControllerDevicesStore, ToolWhitelistStore},
    tooling::adapters::{claude_code, codex, openclaw, opencode},
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
    /// 详情刷新请求标识（来自 app）。
    pub(crate) detail_refresh_id: Option<String>,
    /// 详情刷新优先级。
    pub(crate) detail_priority: ToolDetailsRefreshPriority,
    /// 详情快照触发来源。
    pub(crate) detail_trigger: ToolDetailsSnapshotTrigger,
}

impl SidecarCommandOutcome {
    /// 刷新快照与详情。
    fn snapshots_and_details() -> Self {
        Self {
            refresh_snapshots: true,
            refresh_details: true,
            detail_tool_id: None,
            force_detail_refresh: true,
            detail_refresh_id: None,
            detail_priority: ToolDetailsRefreshPriority::Background,
            detail_trigger: ToolDetailsSnapshotTrigger::Command,
        }
    }

    /// 仅刷新详情。
    fn details_only(
        detail_tool_id: Option<String>,
        force: bool,
        detail_refresh_id: Option<String>,
        detail_priority: ToolDetailsRefreshPriority,
        detail_trigger: ToolDetailsSnapshotTrigger,
    ) -> Self {
        Self {
            refresh_snapshots: false,
            refresh_details: true,
            detail_tool_id,
            force_detail_refresh: force,
            detail_refresh_id,
            detail_priority,
            detail_trigger,
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

/// 临时附件落盘结果。
#[derive(Debug, Clone)]
struct StagedMedia {
    staged_media_id: String,
    mime: String,
    path_hint: String,
    relative_path: String,
    staged_path: String,
    expires_at: String,
    size: usize,
}

#[derive(Debug, Clone)]
struct StageError {
    code: &'static str,
    reason: String,
}

impl StageError {
    fn new(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
}

/// 启动请求上下文。
#[derive(Debug, Clone)]
struct LaunchContext {
    tool_name: String,
    cwd: String,
    request_id: String,
    conversation_key: String,
}

/// 启动目标类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchTool {
    OpenClaw,
    OpenCode,
    Codex,
    ClaudeCode,
}

/// 预处理后的启动参数。
#[derive(Debug, Clone)]
struct PreparedLaunch {
    tool: LaunchTool,
    cwd: PathBuf,
}

/// 附件 base64 最大长度（约 32MB 原始数据）。
const MEDIA_STAGE_MAX_BYTES: usize = 32 * 1024 * 1024;
/// 附件暂存目录名（工作区内）。
const MEDIA_STAGE_INBOX_DIR: &str = ".yc/inbox";
/// 附件暂存目录环境变量。
const MEDIA_STAGE_DIR_ENV: &str = "YC_MEDIA_STAGE_DIR";
/// 附件暂存文件生存时间（秒）。
const MEDIA_STAGE_TTL_SEC: u64 = 24 * 3600;
/// 启动目录白名单环境变量（PATH 语义）。
const LAUNCH_ALLOWED_ROOTS_ENV: &str = "YC_LAUNCH_ALLOWED_ROOTS";
/// 媒体错误码：不支持的 MIME。
const MEDIA_UNSUPPORTED_MIME: &str = "MEDIA_UNSUPPORTED_MIME";
/// 媒体错误码：超出体积限制。
const MEDIA_TOO_LARGE: &str = "MEDIA_TOO_LARGE";
/// 媒体错误码：base64 解码失败或内容为空。
const MEDIA_DECODE_FAILED: &str = "MEDIA_DECODE_FAILED";
/// 媒体错误码：无法定位目标工具。
const MEDIA_STAGE_NOT_FOUND: &str = "MEDIA_STAGE_NOT_FOUND";
/// 媒体错误码：路径越界或无可用工作区。
const MEDIA_PATH_FORBIDDEN: &str = "MEDIA_PATH_FORBIDDEN";

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
        SidecarCommand::RefreshToolDetails {
            refresh_id,
            tool_id,
            force,
            priority,
        } => SidecarCommandOutcome::details_only(
            tool_id,
            force,
            Some(refresh_id),
            priority,
            ToolDetailsSnapshotTrigger::Request,
        ),
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
            content,
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
                    content,
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
        SidecarCommand::ToolMediaStageRequest {
            tool_id,
            conversation_key,
            request_id,
            media_id,
            mime,
            data_base64,
            path_hint,
        } => {
            send_event(
                ws_writer,
                &cfg.system_id,
                seq,
                TOOL_MEDIA_STAGE_PROGRESS_EVENT,
                trace_id.as_deref(),
                json!({
                    "toolId": tool_id,
                    "conversationKey": conversation_key,
                    "requestId": request_id,
                    "mediaId": media_id,
                    "progress": 0,
                }),
            )
            .await?;
            let workspace_dir = discovered_tools
                .iter()
                .find(|item| item.tool_id == tool_id)
                .and_then(|item| item.workspace_dir.clone());
            match stage_media_attachment(
                &tool_id,
                &conversation_key,
                &request_id,
                &media_id,
                &mime,
                &data_base64,
                &path_hint,
                workspace_dir.as_deref(),
            ) {
                Ok(staged) => {
                    send_event(
                        ws_writer,
                        &cfg.system_id,
                        seq,
                        TOOL_MEDIA_STAGE_FINISHED_EVENT,
                        trace_id.as_deref(),
                        json!({
                            "toolId": tool_id,
                            "conversationKey": conversation_key,
                            "requestId": request_id,
                            "mediaId": media_id,
                            "stagedMediaId": staged.staged_media_id,
                            "mime": staged.mime,
                            "size": staged.size,
                            "pathHint": staged.path_hint,
                            "relativePath": staged.relative_path,
                            "stagedPath": staged.staged_path,
                            "expiresAt": staged.expires_at,
                            "progress": 100,
                        }),
                    )
                    .await?;
                }
                Err(err) => {
                    send_event(
                        ws_writer,
                        &cfg.system_id,
                        seq,
                        TOOL_MEDIA_STAGE_FAILED_EVENT,
                        trace_id.as_deref(),
                        json!({
                            "toolId": tool_id,
                            "conversationKey": conversation_key,
                            "requestId": request_id,
                            "mediaId": media_id,
                            "code": err.code,
                            "reason": err.reason,
                        }),
                    )
                    .await?;
                }
            }
            SidecarCommandOutcome::default()
        }
        SidecarCommand::ToolLaunchRequest {
            tool_name,
            cwd,
            request_id,
            conversation_key,
        } => {
            let launch_context = LaunchContext {
                tool_name: tool_name.clone(),
                cwd: cwd.clone(),
                request_id: request_id.clone(),
                conversation_key: conversation_key.clone(),
            };
            match prepare_launch_request(&launch_context, discovered_tools) {
                Ok(prepared) => {
                    send_event(
                        ws_writer,
                        &cfg.system_id,
                        seq,
                        TOOL_LAUNCH_STARTED_EVENT,
                        trace_id.as_deref(),
                        json!({
                            "toolName": launch_context.tool_name,
                            "cwd": launch_context.cwd,
                            "requestId": launch_context.request_id,
                            "conversationKey": launch_context.conversation_key,
                        }),
                    )
                    .await?;
                    match spawn_launch_target(&prepared).await {
                        Ok(pid) => {
                            send_event(
                                ws_writer,
                                &cfg.system_id,
                                seq,
                                TOOL_LAUNCH_FINISHED_EVENT,
                                trace_id.as_deref(),
                                json!({
                                    "toolName": launch_context.tool_name,
                                    "cwd": launch_context.cwd,
                                    "requestId": launch_context.request_id,
                                    "conversationKey": launch_context.conversation_key,
                                    "pid": pid,
                                    "status": "started",
                                    "reason": "工具进程已启动。",
                                }),
                            )
                            .await?;
                            SidecarCommandOutcome::snapshots_and_details()
                        }
                        Err(reason) => {
                            send_event(
                                ws_writer,
                                &cfg.system_id,
                                seq,
                                TOOL_LAUNCH_FAILED_EVENT,
                                trace_id.as_deref(),
                                json!({
                                    "toolName": launch_context.tool_name,
                                    "cwd": launch_context.cwd,
                                    "requestId": launch_context.request_id,
                                    "conversationKey": launch_context.conversation_key,
                                    "reason": reason,
                                }),
                            )
                            .await?;
                            SidecarCommandOutcome::default()
                        }
                    }
                }
                Err(reason) => {
                    send_event(
                        ws_writer,
                        &cfg.system_id,
                        seq,
                        TOOL_LAUNCH_FAILED_EVENT,
                        trace_id.as_deref(),
                        json!({
                            "toolName": launch_context.tool_name,
                            "cwd": launch_context.cwd,
                            "requestId": launch_context.request_id,
                            "conversationKey": launch_context.conversation_key,
                            "reason": reason,
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

fn stage_media_attachment(
    tool_id: &str,
    conversation_key: &str,
    request_id: &str,
    media_id: &str,
    mime: &str,
    data_base64: &str,
    path_hint: &str,
    workspace_dir: Option<&str>,
) -> std::result::Result<StagedMedia, StageError> {
    let provided_mime = mime.trim().to_ascii_lowercase();
    let raw_payload = data_base64.trim();
    if raw_payload.is_empty() {
        return Err(StageError::new(
            MEDIA_DECODE_FAILED,
            "缺少附件内容，无法暂存。",
        ));
    }

    let (inline_mime, base64_payload) = parse_base64_payload(raw_payload);
    let effective_mime = if !provided_mime.is_empty() {
        provided_mime
    } else {
        inline_mime
    };
    if !effective_mime.starts_with("image/")
        && !effective_mime.starts_with("video/")
        && !effective_mime.starts_with("audio/")
    {
        return Err(StageError::new(
            MEDIA_UNSUPPORTED_MIME,
            "仅支持 image/video/audio MIME 类型。",
        ));
    }

    let bytes = general_purpose::STANDARD
        .decode(base64_payload.as_bytes())
        .map_err(|err| {
            StageError::new(MEDIA_DECODE_FAILED, format!("附件 base64 解码失败: {err}"))
        })?;
    if bytes.is_empty() {
        return Err(StageError::new(
            MEDIA_DECODE_FAILED,
            "附件内容为空，无法暂存。",
        ));
    }
    if bytes.len() > MEDIA_STAGE_MAX_BYTES {
        return Err(StageError::new(
            MEDIA_TOO_LARGE,
            format!(
                "附件超过大小限制（{} MB）。",
                MEDIA_STAGE_MAX_BYTES / (1024 * 1024)
            ),
        ));
    }

    let stage_root = resolve_media_stage_root(workspace_dir).map_err(|reason| {
        StageError::new(MEDIA_STAGE_NOT_FOUND, format!("{tool_id} 暂存目录不可用: {reason}"))
    })?;
    cleanup_media_stage_dir(&stage_root);
    let conv_segment = sanitize_path_segment(conversation_key);
    let req_segment = sanitize_path_segment(request_id);
    let dir = stage_root.join(&conv_segment).join(&req_segment);
    fs::create_dir_all(&dir).map_err(|err| {
        StageError::new(MEDIA_PATH_FORBIDDEN, format!("创建暂存目录失败: {err}"))
    })?;
    let ext = mime_extension(&effective_mime);
    let file_name = format!("{}.{}", sanitize_path_segment(media_id), ext);
    let file_path = dir.join(file_name);
    fs::write(&file_path, &bytes).map_err(|err| {
        StageError::new(MEDIA_PATH_FORBIDDEN, format!("写入附件暂存文件失败: {err}"))
    })?;
    let relative_path = format!(
        "{}/{}/{}",
        conv_segment,
        req_segment,
        file_path
            .file_name()
            .and_then(|item| item.to_str())
            .unwrap_or_default()
    );
    let expires_at = (Utc::now() + ChronoDuration::seconds(MEDIA_STAGE_TTL_SEC as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    Ok(StagedMedia {
        staged_media_id: relative_path.clone(),
        mime: effective_mime,
        path_hint: path_hint.trim().to_string(),
        relative_path,
        staged_path: file_path.to_string_lossy().to_string(),
        expires_at,
        size: bytes.len(),
    })
}

fn parse_base64_payload(raw: &str) -> (String, String) {
    if !raw.starts_with("data:") {
        return (String::new(), raw.trim().to_string());
    }
    let marker = ";base64,";
    if let Some(index) = raw.find(marker) {
        let mime = raw[5..index].trim().to_ascii_lowercase();
        let payload = raw[(index + marker.len())..].trim().to_string();
        return (mime, payload);
    }
    (String::new(), raw.trim().to_string())
}

fn resolve_media_stage_root(workspace_dir: Option<&str>) -> std::result::Result<PathBuf, String> {
    if let Some(raw) = env::var_os(MEDIA_STAGE_DIR_ENV) {
        let candidate = PathBuf::from(raw);
        if !candidate.as_os_str().is_empty() {
            return Ok(candidate);
        }
    }
    let Some(workspace) = workspace_dir.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err("工具缺少工作目录。".to_string());
    };
    let canonical =
        fs::canonicalize(workspace).map_err(|err| format!("工作目录不可访问或不存在: {err}"))?;
    if !canonical.is_dir() {
        return Err("工作目录不是目录。".to_string());
    }
    Ok(canonical.join(MEDIA_STAGE_INBOX_DIR))
}

fn cleanup_media_stage_dir(root: &Path) {
    if !root.exists() {
        return;
    }
    let mut stack = vec![root.to_path_buf()];
    let now = std::time::SystemTime::now();
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let Ok(elapsed) = now.duration_since(modified) else {
                continue;
            };
            if elapsed.as_secs() > MEDIA_STAGE_TTL_SEC {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn sanitize_path_segment(raw: &str) -> String {
    let mut output = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "unknown".to_string()
    } else {
        output
    }
}

fn mime_extension(mime: &str) -> String {
    let sub = mime
        .split('/')
        .nth(1)
        .unwrap_or("bin")
        .split(';')
        .next()
        .unwrap_or("bin")
        .trim()
        .to_ascii_lowercase();
    let sanitized = sanitize_path_segment(sub.as_str());
    if sanitized.is_empty() {
        "bin".to_string()
    } else {
        sanitized
    }
}

fn prepare_launch_request(
    request: &LaunchContext,
    discovered_tools: &[ToolRuntimePayload],
) -> std::result::Result<PreparedLaunch, String> {
    let Some(tool) = parse_launch_tool(request.tool_name.as_str()) else {
        return Err("不支持的工具类型，仅支持 OpenClaw/OpenCode/Codex/Claude Code。".to_string());
    };
    let cwd = canonicalize_launch_cwd(request.cwd.as_str())?;
    let allowed_roots = resolve_launch_allowed_roots(discovered_tools, tool);
    if !allowed_roots.iter().any(|root| cwd.starts_with(root)) {
        return Err("目标目录不在授权范围内，请切换到工作区目录后重试。".to_string());
    }
    Ok(PreparedLaunch { tool, cwd })
}

fn parse_launch_tool(raw: &str) -> Option<LaunchTool> {
    let normalized = raw.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    if normalized.contains("openclaw") || normalized == "claw" {
        return Some(LaunchTool::OpenClaw);
    }
    if normalized.contains("opencode") || normalized == "open-code" {
        return Some(LaunchTool::OpenCode);
    }
    if normalized.contains("codex") {
        return Some(LaunchTool::Codex);
    }
    if normalized.contains("claude") {
        return Some(LaunchTool::ClaudeCode);
    }
    None
}

fn canonicalize_launch_cwd(raw: &str) -> std::result::Result<PathBuf, String> {
    let cwd = expand_tilde(raw);
    if cwd.trim().is_empty() {
        return Err("缺少目标目录，无法启动工具。".to_string());
    }
    let path = PathBuf::from(cwd.trim());
    if !path.is_absolute() {
        return Err("仅支持绝对路径目录启动。".to_string());
    }
    let canonical =
        fs::canonicalize(&path).map_err(|err| format!("目录不可访问或不存在: {err}"))?;
    if !canonical.is_dir() {
        return Err("目标路径不是目录。".to_string());
    }
    Ok(canonical)
}

fn expand_tilde(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(suffix) = trimmed.strip_prefix("~/")
        && let Ok(home) = env::var("HOME")
        && !home.trim().is_empty()
    {
        return format!("{}/{}", home.trim_end_matches('/'), suffix);
    }
    trimmed.to_string()
}

fn resolve_launch_allowed_roots(
    discovered_tools: &[ToolRuntimePayload],
    target: LaunchTool,
) -> Vec<PathBuf> {
    let mut roots = Vec::<PathBuf>::new();
    for tool in discovered_tools {
        if !tool_matches_launch_target(tool, target) {
            continue;
        }
        let Some(workspace) = tool.workspace_dir.as_deref() else {
            continue;
        };
        let normalized = workspace.trim();
        if normalized.is_empty() {
            continue;
        }
        if let Ok(path) = fs::canonicalize(normalized) {
            roots.push(path);
        }
    }
    if let Some(raw) = env::var_os(LAUNCH_ALLOWED_ROOTS_ENV) {
        for value in env::split_paths(&raw) {
            if let Ok(path) = fs::canonicalize(&value) {
                roots.push(path);
            }
        }
    }
    if let Ok(home) = env::var("HOME")
        && !home.trim().is_empty()
        && let Ok(path) = fs::canonicalize(home)
    {
        roots.push(path);
    }
    roots.sort();
    roots.dedup();
    roots
}

fn tool_matches_launch_target(tool: &ToolRuntimePayload, target: LaunchTool) -> bool {
    match target {
        LaunchTool::OpenClaw => openclaw::matches_tool(tool),
        LaunchTool::OpenCode => opencode::matches_tool(tool),
        LaunchTool::Codex => codex::matches_tool(tool),
        LaunchTool::ClaudeCode => claude_code::matches_tool(tool),
    }
}

async fn spawn_launch_target(
    prepared: &PreparedLaunch,
) -> std::result::Result<Option<i32>, String> {
    let program = match prepared.tool {
        LaunchTool::OpenClaw => "openclaw",
        LaunchTool::OpenCode => "opencode",
        LaunchTool::Codex => "codex",
        LaunchTool::ClaudeCode => "claude",
    };
    let mut command = Command::new(program);
    command
        .current_dir(&prepared.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command
        .spawn()
        .map_err(|err| format!("启动 {} 失败: {err}", program))?;
    Ok(child.id().and_then(|pid| i32::try_from(pid).ok()))
}
