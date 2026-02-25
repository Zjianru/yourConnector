//! 运行时探测模块职责：
//! 1. 从系统进程列表提取统一的进程快照结构。
//! 2. 向 Tool Adapter Core 提供统一进程结构（pid/cmd/cwd/cpu/memory）。
//! 3. 在未发现工具时按配置返回 fallback 占位工具。

use yc_shared_protocol::{LatestTokensPayload, ToolRuntimePayload, now_rfc3339_nanos};

/// 进程摘要信息，作为工具发现器的统一输入。
///
/// 该结构由 Tool Adapter Core 在每轮扫描时构建并传给各工具适配器。
#[derive(Debug, Clone)]
pub(crate) struct ProcInfo {
    /// 进程 PID。
    pub(crate) pid: i32,
    /// 完整命令行字符串。
    pub(crate) cmd: String,
    /// 当前工作目录。
    pub(crate) cwd: String,
    /// CPU 使用率（百分比）。
    pub(crate) cpu_percent: f64,
    /// 内存占用（MB）。
    pub(crate) memory_mb: f64,
}

/// 当开关开启且未发现真实工具时，返回单条 fallback 占位工具。
pub(crate) fn fallback_tools_or_empty(fallback_tool: bool) -> Vec<ToolRuntimePayload> {
    if !fallback_tool {
        return Vec::new();
    }
    vec![ToolRuntimePayload {
        tool_id: "tool_local".to_string(),
        name: "Local Tool".to_string(),
        tool_class: "assistant".to_string(),
        category: "DEV_WORKER".to_string(),
        vendor: "yourconnector".to_string(),
        mode: "TUI".to_string(),
        status: "IDLE".to_string(),
        connected: false,
        endpoint: String::new(),
        pid: None,
        reason: Some("未发现 OpenCode/OpenClaw 进程，展示 fallback 工具".to_string()),
        cpu_percent: Some(0.0),
        memory_mb: Some(0.0),
        source: Some("fallback".to_string()),
        workspace_dir: None,
        session_id: None,
        session_title: None,
        session_updated_at: None,
        agent_mode: None,
        provider_id: None,
        model_id: None,
        model: None,
        latest_tokens: Some(LatestTokensPayload::default()),
        model_usage: Vec::new(),
        collected_at: Some(now_rfc3339_nanos()),
    }]
}
