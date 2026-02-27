//! 控制命令解析模块职责：
//! 1. 定义移动端可下发给 sidecar 的控制事件类型。
//! 2. 将 relay 转发的事件 JSON 解析为强类型命令。
//! 3. 提供统一的命令回执字段，减少主流程分支重复代码。

use serde_json::Value;
use uuid::Uuid;
use yc_shared_protocol::ToolDetailsRefreshPriority;

/// 请求接入某个候选工具。
pub(crate) const TOOL_CONNECT_REQUEST_EVENT: &str = "tool_connect_request";
/// 请求断开某个已接入工具。
pub(crate) const TOOL_DISCONNECT_REQUEST_EVENT: &str = "tool_disconnect_request";
/// 请求 sidecar 立即刷新工具快照。
pub(crate) const TOOLS_REFRESH_REQUEST_EVENT: &str = "tools_refresh_request";
/// 请求 sidecar 清空工具白名单（删除宿主机时使用）。
pub(crate) const TOOL_WHITELIST_RESET_REQUEST_EVENT: &str = "tool_whitelist_reset_request";
/// 请求 sidecar 立即刷新工具详情（支持指定 toolId）。
pub(crate) const TOOL_DETAILS_REFRESH_REQUEST_EVENT: &str = "tool_details_refresh_request";
/// sidecar 返回工具白名单更新结果。
pub(crate) const TOOL_WHITELIST_UPDATED_EVENT: &str = "tool_whitelist_updated";
/// 请求 sidecar 控制工具进程（停止/重启）。
pub(crate) const TOOL_PROCESS_CONTROL_REQUEST_EVENT: &str = "tool_process_control_request";
/// sidecar 返回工具进程控制结果。
pub(crate) const TOOL_PROCESS_CONTROL_UPDATED_EVENT: &str = "tool_process_control_updated";
/// 请求把当前（或指定）设备重绑为控制端。
pub(crate) const CONTROLLER_REBIND_REQUEST_EVENT: &str = "controller_rebind_request";
/// sidecar 返回控制端绑定更新结果。
pub(crate) const CONTROLLER_BIND_UPDATED_EVENT: &str = "controller_bind_updated";
/// 请求执行工具聊天（单条消息）。
pub(crate) const TOOL_CHAT_REQUEST_EVENT: &str = "tool_chat_request";
/// 请求取消当前工具聊天执行。
pub(crate) const TOOL_CHAT_CANCEL_REQUEST_EVENT: &str = "tool_chat_cancel_request";
/// sidecar 返回聊天开始事件。
pub(crate) const TOOL_CHAT_STARTED_EVENT: &str = "tool_chat_started";
/// sidecar 返回聊天流式分片事件。
pub(crate) const TOOL_CHAT_CHUNK_EVENT: &str = "tool_chat_chunk";
/// sidecar 返回聊天结束事件。
pub(crate) const TOOL_CHAT_FINISHED_EVENT: &str = "tool_chat_finished";
/// 请求拉取工具工作区下的报告文件（仅 .md）。
pub(crate) const TOOL_REPORT_FETCH_REQUEST_EVENT: &str = "tool_report_fetch_request";
/// sidecar 返回报告拉取开始事件。
pub(crate) const TOOL_REPORT_FETCH_STARTED_EVENT: &str = "tool_report_fetch_started";
/// sidecar 返回报告拉取分片事件。
pub(crate) const TOOL_REPORT_FETCH_CHUNK_EVENT: &str = "tool_report_fetch_chunk";
/// sidecar 返回报告拉取结束事件。
pub(crate) const TOOL_REPORT_FETCH_FINISHED_EVENT: &str = "tool_report_fetch_finished";
/// 请求 sidecar 暂存聊天多媒体附件。
pub(crate) const TOOL_MEDIA_STAGE_REQUEST_EVENT: &str = "tool_media_stage_request";
/// sidecar 返回多媒体暂存进度。
pub(crate) const TOOL_MEDIA_STAGE_PROGRESS_EVENT: &str = "tool_media_stage_progress";
/// sidecar 返回多媒体暂存完成。
pub(crate) const TOOL_MEDIA_STAGE_FINISHED_EVENT: &str = "tool_media_stage_finished";
/// sidecar 返回多媒体暂存失败。
pub(crate) const TOOL_MEDIA_STAGE_FAILED_EVENT: &str = "tool_media_stage_failed";
/// 请求 sidecar 以指定目录启动工具进程。
pub(crate) const TOOL_LAUNCH_REQUEST_EVENT: &str = "tool_launch_request";
/// sidecar 返回启动流程开始。
pub(crate) const TOOL_LAUNCH_STARTED_EVENT: &str = "tool_launch_started";
/// sidecar 返回启动流程结束。
pub(crate) const TOOL_LAUNCH_FINISHED_EVENT: &str = "tool_launch_finished";
/// sidecar 返回启动流程失败。
pub(crate) const TOOL_LAUNCH_FAILED_EVENT: &str = "tool_launch_failed";
/// 请求 sidecar 执行账号切换。
pub(crate) const TOOL_AUTH_SWITCH_REQUEST_EVENT: &str = "tool_auth_switch_request";
/// sidecar 返回账号切换完成。
pub(crate) const TOOL_AUTH_SWITCH_FINISHED_EVENT: &str = "tool_auth_switch_finished";
/// sidecar 返回账号切换失败。
pub(crate) const TOOL_AUTH_SWITCH_FAILED_EVENT: &str = "tool_auth_switch_failed";

/// Relay 注入的可信来源客户端类型字段。
const SOURCE_CLIENT_TYPE_FIELD: &str = "sourceClientType";
/// Relay 注入的可信来源设备 ID 字段。
const SOURCE_DEVICE_ID_FIELD: &str = "sourceDeviceId";
/// 统一事件字段：事件类型。
const EVENT_TYPE_FIELD: &str = "type";
/// 统一事件字段：事件 ID。
const EVENT_ID_FIELD: &str = "eventId";
/// 统一事件字段：链路追踪 ID。
const TRACE_ID_FIELD: &str = "traceId";
/// 兼容字段：旧链路通过 peerId 携带来源设备 ID。
const PEER_ID_FIELD: &str = "peerId";

/// Sidecar 可执行的控制命令。
#[derive(Debug)]
pub(crate) enum SidecarCommand {
    /// 仅刷新工具列表与指标，不修改白名单。
    Refresh,
    /// 将工具加入白名单并进入 Connected 列表。
    ConnectTool { tool_id: String },
    /// 将工具移出白名单并回到 Candidates 列表。
    DisconnectTool { tool_id: String },
    /// 清空全部工具白名单，断开全部已接入工具。
    ResetToolWhitelist,
    /// 刷新工具详情（可指定单工具）。
    RefreshToolDetails {
        refresh_id: String,
        tool_id: Option<String>,
        force: bool,
        priority: ToolDetailsRefreshPriority,
    },
    /// 控制工具进程：当前仅支持 OpenClaw 的停止/重启。
    ControlToolProcess {
        tool_id: String,
        action: ToolProcessAction,
    },
    /// 将控制端设备重绑为指定 deviceId。
    RebindController { device_id: String },
    /// 发起工具聊天请求。
    ToolChatRequest {
        tool_id: String,
        conversation_key: String,
        request_id: String,
        queue_item_id: String,
        text: String,
        content: Vec<ChatContentPart>,
    },
    /// 取消工具聊天请求。
    ToolChatCancel {
        tool_id: String,
        conversation_key: String,
        request_id: String,
        queue_item_id: String,
    },
    /// 拉取工具工作区内的 Markdown 报告文件。
    ToolReportFetchRequest {
        tool_id: String,
        conversation_key: String,
        request_id: String,
        file_path: String,
    },
    /// 暂存聊天附件。
    ToolMediaStageRequest {
        tool_id: String,
        conversation_key: String,
        request_id: String,
        media_id: String,
        mime: String,
        data_base64: String,
        path_hint: String,
    },
    /// 按目录启动工具 CLI。
    ToolLaunchRequest {
        tool_name: String,
        cwd: String,
        request_id: String,
    },
    /// 切换工具账号/配置。
    ToolAuthSwitchRequest {
        tool_name: String,
        profile: String,
        request_id: String,
    },
}

/// 聊天多段内容（兼容 text + media/fileRef）。
#[derive(Debug, Clone, Default)]
pub(crate) struct ChatContentPart {
    /// 段类型：text/image/video/audio/fileRef。
    pub(crate) kind: String,
    /// 媒体唯一标识（可选）。
    pub(crate) media_id: String,
    /// MIME 类型（可选）。
    pub(crate) mime: String,
    /// 大小（字节，可选）。
    pub(crate) size: u64,
    /// 时长（毫秒，可选）。
    pub(crate) duration_ms: u64,
    /// 文本内容（text/fileRef 可用）。
    pub(crate) text: String,
    /// 路径提示（可选）。
    pub(crate) path_hint: String,
    /// base64 正文（可选，media staging 场景）。
    pub(crate) data_base64: String,
}

/// 工具进程控制动作枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolProcessAction {
    /// 停止目标进程。
    Stop,
    /// 重启目标进程。
    Restart,
}

impl ToolProcessAction {
    /// 把动作枚举转成协议字符串。
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// 命令与来源信息封装，用于权限判断与审计日志。
#[derive(Debug)]
pub(crate) struct SidecarCommandEnvelope {
    /// 原始事件类型。
    pub(crate) event_type: String,
    /// 原始事件 ID。
    pub(crate) event_id: String,
    /// 链路追踪 ID。
    pub(crate) trace_id: String,
    /// 解析出的控制命令。
    pub(crate) command: SidecarCommand,
    /// 来源客户端类型（app/sidecar）。
    pub(crate) source_client_type: String,
    /// 来源设备 ID。
    pub(crate) source_device_id: String,
}

fn parse_u64_field(value: Option<&Value>) -> u64 {
    let Some(raw) = value else {
        return 0;
    };
    if let Some(num) = raw.as_u64() {
        return num;
    }
    if let Some(num) = raw.as_i64() {
        return if num > 0 { num as u64 } else { 0 };
    }
    if let Some(text) = raw.as_str() {
        return text.trim().parse::<u64>().unwrap_or_default();
    }
    0
}

fn parse_chat_content_parts(raw: Option<&Value>) -> Vec<ChatContentPart> {
    const MAX_MEDIA_BASE64_LEN: usize = 40 * 1024 * 1024;
    let Some(rows) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in rows {
        let Some(obj) = row.as_object() else {
            continue;
        };
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if kind.is_empty() {
            continue;
        }
        let text = obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let path_hint = obj
            .get("pathHint")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let media_id = obj
            .get("mediaId")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let mime = obj
            .get("mime")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let data_base64 = obj
            .get("dataBase64")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if !data_base64.is_empty() && data_base64.len() > MAX_MEDIA_BASE64_LEN {
            continue;
        }
        let size = parse_u64_field(obj.get("size"));
        let duration_ms = parse_u64_field(obj.get("durationMs"));

        if kind == "text" && text.is_empty() {
            continue;
        }
        if kind.eq_ignore_ascii_case("fileref") && path_hint.is_empty() && text.is_empty() {
            continue;
        }
        out.push(ChatContentPart {
            kind,
            media_id,
            mime,
            size,
            duration_ms,
            text,
            path_hint,
            data_base64,
        });
    }
    out
}

/// 从原始事件 JSON 解析 sidecar 控制命令。
pub(crate) fn parse_sidecar_command(raw: &str) -> Option<SidecarCommandEnvelope> {
    let event: Value = serde_json::from_str(raw).ok()?;
    let event_type = event
        .get(EVENT_TYPE_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let event_id = event
        .get(EVENT_ID_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let trace_id = event
        .get(TRACE_ID_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let payload = event
        .get("payload")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let source_client_type = event
        .get(SOURCE_CLIENT_TYPE_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let source_device_id = event
        .get(SOURCE_DEVICE_ID_FIELD)
        .or_else(|| event.get(PEER_ID_FIELD))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    let command = match event_type {
        TOOLS_REFRESH_REQUEST_EVENT => Some(SidecarCommand::Refresh),
        TOOL_CONNECT_REQUEST_EVENT => payload
            .get("toolId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|tool_id| SidecarCommand::ConnectTool {
                tool_id: tool_id.to_string(),
            }),
        TOOL_DISCONNECT_REQUEST_EVENT => payload
            .get("toolId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|tool_id| SidecarCommand::DisconnectTool {
                tool_id: tool_id.to_string(),
            }),
        TOOL_WHITELIST_RESET_REQUEST_EVENT => Some(SidecarCommand::ResetToolWhitelist),
        TOOL_DETAILS_REFRESH_REQUEST_EVENT => {
            let refresh_id = payload
                .get("refreshId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("drf_{}", Uuid::new_v4()));
            let tool_id = payload
                .get("toolId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string);
            let force = payload
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let priority = payload
                .get("priority")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(|value| value.to_ascii_lowercase())
                .map(|value| match value.as_str() {
                    "user" => ToolDetailsRefreshPriority::User,
                    _ => ToolDetailsRefreshPriority::Background,
                })
                .unwrap_or(ToolDetailsRefreshPriority::Background);
            Some(SidecarCommand::RefreshToolDetails {
                refresh_id,
                tool_id,
                force,
                priority,
            })
        }
        TOOL_PROCESS_CONTROL_REQUEST_EVENT => {
            let tool_id = payload
                .get("toolId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let action = payload
                .get("action")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(|value| value.to_ascii_lowercase())
                .and_then(|value| match value.as_str() {
                    "stop" => Some(ToolProcessAction::Stop),
                    "restart" => Some(ToolProcessAction::Restart),
                    _ => None,
                })?;
            Some(SidecarCommand::ControlToolProcess { tool_id, action })
        }
        CONTROLLER_REBIND_REQUEST_EVENT => payload
            .get("deviceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                let value = source_device_id.trim();
                if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                }
            })
            .map(|device_id| SidecarCommand::RebindController { device_id }),
        TOOL_CHAT_REQUEST_EVENT => {
            let tool_id = payload
                .get("toolId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let conversation_key = payload
                .get("conversationKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let request_id = payload
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let text = payload
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            let content = parse_chat_content_parts(payload.get("content"));
            if text.trim().is_empty() && content.is_empty() {
                return None;
            }
            let queue_item_id = payload
                .get("queueItemId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| request_id.clone());

            Some(SidecarCommand::ToolChatRequest {
                tool_id,
                conversation_key,
                request_id,
                queue_item_id,
                text,
                content,
            })
        }
        TOOL_CHAT_CANCEL_REQUEST_EVENT => {
            let conversation_key = payload
                .get("conversationKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let request_id = payload
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let queue_item_id = payload
                .get("queueItemId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| request_id.clone());
            let tool_id = payload
                .get("toolId")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string();

            Some(SidecarCommand::ToolChatCancel {
                tool_id,
                conversation_key,
                request_id,
                queue_item_id,
            })
        }
        TOOL_REPORT_FETCH_REQUEST_EVENT => {
            let tool_id = payload
                .get("toolId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let conversation_key = payload
                .get("conversationKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let request_id = payload
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let file_path = payload
                .get("filePath")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;

            Some(SidecarCommand::ToolReportFetchRequest {
                tool_id,
                conversation_key,
                request_id,
                file_path,
            })
        }
        TOOL_MEDIA_STAGE_REQUEST_EVENT => {
            let tool_id = payload
                .get("toolId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let conversation_key = payload
                .get("conversationKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let request_id = payload
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let media_id = payload
                .get("mediaId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let mime = payload
                .get("mime")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let data_base64 = payload
                .get("dataBase64")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let path_hint = payload
                .get("pathHint")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            Some(SidecarCommand::ToolMediaStageRequest {
                tool_id,
                conversation_key,
                request_id,
                media_id,
                mime,
                data_base64,
                path_hint,
            })
        }
        TOOL_LAUNCH_REQUEST_EVENT => {
            let tool_name = payload
                .get("toolName")
                .or_else(|| payload.get("tool"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let cwd = payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let request_id = payload
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            Some(SidecarCommand::ToolLaunchRequest {
                tool_name,
                cwd,
                request_id,
            })
        }
        TOOL_AUTH_SWITCH_REQUEST_EVENT => {
            let tool_name = payload
                .get("toolName")
                .or_else(|| payload.get("tool"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let profile = payload
                .get("profile")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            let request_id = payload
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)?;
            Some(SidecarCommand::ToolAuthSwitchRequest {
                tool_name,
                profile,
                request_id,
            })
        }
        _ => None,
    }?;

    Some(SidecarCommandEnvelope {
        event_type: event_type.to_string(),
        event_id,
        trace_id,
        command,
        source_client_type,
        source_device_id,
    })
}

/// 构造命令回执所需的 action/toolId 字段。
pub(crate) fn command_feedback_parts(command: &SidecarCommand) -> (&'static str, String) {
    match command {
        SidecarCommand::Refresh => ("refresh", String::new()),
        SidecarCommand::ConnectTool { tool_id } => ("connect", tool_id.clone()),
        SidecarCommand::DisconnectTool { tool_id } => ("disconnect", tool_id.clone()),
        SidecarCommand::ResetToolWhitelist => ("reset", String::new()),
        SidecarCommand::RefreshToolDetails { tool_id, .. } => {
            ("refresh-details", tool_id.clone().unwrap_or_default())
        }
        SidecarCommand::ControlToolProcess { tool_id, action } => {
            (action.as_str(), tool_id.clone())
        }
        SidecarCommand::RebindController { device_id } => {
            ("rebind-controller", device_id.to_string())
        }
        SidecarCommand::ToolChatRequest { tool_id, .. } => ("chat-request", tool_id.clone()),
        SidecarCommand::ToolChatCancel { tool_id, .. } => ("chat-cancel", tool_id.clone()),
        SidecarCommand::ToolReportFetchRequest { tool_id, .. } => ("report-fetch", tool_id.clone()),
        SidecarCommand::ToolMediaStageRequest { tool_id, .. } => ("media-stage", tool_id.clone()),
        SidecarCommand::ToolLaunchRequest { tool_name, .. } => ("launch", tool_name.clone()),
        SidecarCommand::ToolAuthSwitchRequest { tool_name, .. } => {
            ("auth-switch", tool_name.clone())
        }
    }
}

/// 根据命令类型返回回执事件名。
pub(crate) fn command_feedback_event(command: &SidecarCommand) -> &'static str {
    match command {
        SidecarCommand::ControlToolProcess { .. } => TOOL_PROCESS_CONTROL_UPDATED_EVENT,
        SidecarCommand::ToolChatRequest { .. } => TOOL_CHAT_FINISHED_EVENT,
        SidecarCommand::ToolChatCancel { .. } => TOOL_CHAT_FINISHED_EVENT,
        SidecarCommand::ToolReportFetchRequest { .. } => TOOL_REPORT_FETCH_FINISHED_EVENT,
        SidecarCommand::ToolMediaStageRequest { .. } => TOOL_MEDIA_STAGE_FAILED_EVENT,
        SidecarCommand::ToolLaunchRequest { .. } => TOOL_LAUNCH_FAILED_EVENT,
        SidecarCommand::ToolAuthSwitchRequest { .. } => TOOL_AUTH_SWITCH_FAILED_EVENT,
        _ => TOOL_WHITELIST_UPDATED_EVENT,
    }
}

#[cfg(test)]
mod tests {
    use super::{SidecarCommand, ToolProcessAction, parse_sidecar_command};
    use yc_shared_protocol::ToolDetailsRefreshPriority;

    #[test]
    fn parse_rebind_command_prefers_payload_device_id() {
        let raw = r#"{
            "type":"controller_rebind_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{"deviceId":"ios_target"}
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::RebindController { device_id } => {
                assert_eq!(device_id, "ios_target");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_rebind_command_falls_back_to_source_device_id() {
        let raw = r#"{
            "type":"controller_rebind_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{}
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::RebindController { device_id } => {
                assert_eq!(device_id, "ios_source");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_tool_details_refresh_command_with_force_and_tool_id() {
        let raw = r#"{
            "type":"tool_details_refresh_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{"refreshId":"drf_1","toolId":"openclaw_xxx","force":true,"priority":"user"}
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::RefreshToolDetails {
                refresh_id,
                tool_id,
                force,
                priority,
            } => {
                assert_eq!(refresh_id, "drf_1");
                assert_eq!(tool_id.unwrap_or_default(), "openclaw_xxx");
                assert!(force);
                assert_eq!(priority, ToolDetailsRefreshPriority::User);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_tool_process_control_command_restart() {
        let raw = r#"{
            "type":"tool_process_control_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{"toolId":"openclaw_xxx","action":"restart"}
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::ControlToolProcess { tool_id, action } => {
                assert_eq!(tool_id, "openclaw_xxx");
                assert_eq!(action, ToolProcessAction::Restart);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_tool_whitelist_reset_command() {
        let raw = r#"{
            "type":"tool_whitelist_reset_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{}
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::ResetToolWhitelist => {}
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_tool_chat_request_command() {
        let raw = r#"{
            "type":"tool_chat_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{
                "toolId":"opencode_workspace_p1",
                "conversationKey":"host_a::opencode_workspace_p1",
                "requestId":"req_1",
                "queueItemId":"q_1",
                "text":"hello"
            }
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::ToolChatRequest {
                tool_id,
                conversation_key,
                request_id,
                queue_item_id,
                text,
                content,
            } => {
                assert_eq!(tool_id, "opencode_workspace_p1");
                assert_eq!(conversation_key, "host_a::opencode_workspace_p1");
                assert_eq!(request_id, "req_1");
                assert_eq!(queue_item_id, "q_1");
                assert_eq!(text, "hello");
                assert!(content.is_empty());
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_tool_chat_request_command_with_content_parts() {
        let raw = r#"{
            "type":"tool_chat_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{
                "toolId":"openclaw_workspace_p1",
                "conversationKey":"host_a::openclaw_workspace_p1",
                "requestId":"req_2",
                "queueItemId":"q_2",
                "text":"",
                "content":[
                    {"type":"text","text":"请分析附件"},
                    {"type":"image","mediaId":"media_1","mime":"image/png","size":1234,"pathHint":"cat.png","dataBase64":"abcd"}
                ]
            }
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::ToolChatRequest { text, content, .. } => {
                assert_eq!(text, "");
                assert_eq!(content.len(), 2);
                assert_eq!(content[0].kind, "text");
                assert_eq!(content[0].text, "请分析附件");
                assert_eq!(content[1].kind, "image");
                assert_eq!(content[1].media_id, "media_1");
                assert_eq!(content[1].mime, "image/png");
                assert_eq!(content[1].size, 1234);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_tool_chat_cancel_command_defaults_queue_item_id() {
        let raw = r#"{
            "type":"tool_chat_cancel_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{
                "toolId":"opencode_workspace_p1",
                "conversationKey":"host_a::opencode_workspace_p1",
                "requestId":"req_1"
            }
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::ToolChatCancel {
                tool_id,
                conversation_key,
                request_id,
                queue_item_id,
            } => {
                assert_eq!(tool_id, "opencode_workspace_p1");
                assert_eq!(conversation_key, "host_a::opencode_workspace_p1");
                assert_eq!(request_id, "req_1");
                assert_eq!(queue_item_id, "req_1");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_tool_report_fetch_request_command() {
        let raw = r#"{
            "type":"tool_report_fetch_request",
            "sourceClientType":"app",
            "sourceDeviceId":"ios_source",
            "payload":{
                "toolId":"opencode_workspace_p1",
                "conversationKey":"host_a::opencode_workspace_p1",
                "requestId":"rpt_1",
                "filePath":"/Users/codez/report.md"
            }
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::ToolReportFetchRequest {
                tool_id,
                conversation_key,
                request_id,
                file_path,
            } => {
                assert_eq!(tool_id, "opencode_workspace_p1");
                assert_eq!(conversation_key, "host_a::opencode_workspace_p1");
                assert_eq!(request_id, "rpt_1");
                assert_eq!(file_path, "/Users/codez/report.md");
            }
            _ => panic!("unexpected command"),
        }
    }
}
