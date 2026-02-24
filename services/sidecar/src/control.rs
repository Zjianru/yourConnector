//! 控制命令解析模块职责：
//! 1. 定义移动端可下发给 sidecar 的控制事件类型。
//! 2. 将 relay 转发的事件 JSON 解析为强类型命令。
//! 3. 提供统一的命令回执字段，减少主流程分支重复代码。

use serde_json::Value;

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
        tool_id: Option<String>,
        force: bool,
    },
    /// 控制工具进程：当前仅支持 OpenClaw 的停止/重启。
    ControlToolProcess {
        tool_id: String,
        action: ToolProcessAction,
    },
    /// 将控制端设备重绑为指定 deviceId。
    RebindController { device_id: String },
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
            Some(SidecarCommand::RefreshToolDetails { tool_id, force })
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
    }
}

/// 根据命令类型返回回执事件名。
pub(crate) fn command_feedback_event(command: &SidecarCommand) -> &'static str {
    match command {
        SidecarCommand::ControlToolProcess { .. } => TOOL_PROCESS_CONTROL_UPDATED_EVENT,
        _ => TOOL_WHITELIST_UPDATED_EVENT,
    }
}

#[cfg(test)]
mod tests {
    use super::{SidecarCommand, ToolProcessAction, parse_sidecar_command};

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
            "payload":{"toolId":"openclaw_xxx","force":true}
        }"#;

        let env = parse_sidecar_command(raw).expect("command should parse");
        match env.command {
            SidecarCommand::RefreshToolDetails { tool_id, force } => {
                assert_eq!(tool_id.unwrap_or_default(), "openclaw_xxx");
                assert!(force);
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
}
