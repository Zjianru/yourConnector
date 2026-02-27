//! Claude Code 适配器职责：
//! 1. 基于进程命令行发现 Claude CLI 实例。
//! 2. 输出 claude-code.v1 详情数据，统一接入 Tool Adapter Core。

use serde_json::json;
use yc_shared_protocol::{LatestTokensPayload, ToolRuntimePayload, now_rfc3339_nanos};

use crate::tooling::{
    adapters::CLAUDE_CODE_SCHEMA_V1,
    core::types::{ToolDetailCollectOptions, ToolDetailCollectResult, ToolDiscoveryContext},
};

/// 发现所有 Claude Code 工具实例。
pub(crate) fn discover(context: &ToolDiscoveryContext<'_>) -> Vec<ToolRuntimePayload> {
    let mut pids = context
        .all
        .values()
        .filter(|info| crate::is_claude_code_candidate_command(&info.cmd.to_lowercase()))
        .map(|info| info.pid)
        .collect::<Vec<i32>>();
    pids.sort_unstable();
    pids.dedup();

    let mut tools = Vec::with_capacity(pids.len());
    for pid in pids {
        let Some(info) = context.all.get(&pid) else {
            continue;
        };
        let workspace = crate::normalize_path(&info.cwd);
        let model = crate::parse_cli_flag_value(&info.cmd, "--model").unwrap_or_default();
        let tool_id = crate::build_claude_code_tool_id(workspace.as_str(), pid);

        tools.push(ToolRuntimePayload {
            tool_id,
            name: "Claude Code".to_string(),
            tool_class: "code".to_string(),
            category: "CODE_AGENT".to_string(),
            vendor: "Anthropic".to_string(),
            mode: "CLI".to_string(),
            status: "RUNNING".to_string(),
            connected: true,
            endpoint: String::new(),
            pid: Some(pid),
            reason: crate::option_non_empty("已发现 claude 进程".to_string()),
            cpu_percent: Some(crate::round2(info.cpu_percent)),
            memory_mb: Some(crate::round2(info.memory_mb)),
            source: Some("claude-code-process-probe".to_string()),
            workspace_dir: crate::option_non_empty(workspace),
            session_id: None,
            session_title: None,
            session_updated_at: None,
            agent_mode: Some("cli".to_string()),
            provider_id: Some("anthropic".to_string()),
            model_id: crate::option_non_empty(model.clone()),
            model: crate::option_non_empty(model),
            latest_tokens: Some(LatestTokensPayload::default()),
            model_usage: Vec::new(),
            collected_at: Some(now_rfc3339_nanos()),
        });
    }
    tools
}

/// 判断指定工具是否归属于 Claude Code 适配器。
pub(crate) fn matches_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_ascii_lowercase();
    let name = tool.name.to_ascii_lowercase();
    let vendor = tool.vendor.to_ascii_lowercase();
    tool_id.starts_with("claude_code_")
        || name.contains("claude code")
        || (name == "claude")
        || vendor.contains("anthropic")
}

/// 采集 Claude Code 详情（claude-code.v1）。
pub(crate) fn collect_details(
    tools: &[ToolRuntimePayload],
    _options: &ToolDetailCollectOptions,
) -> Vec<ToolDetailCollectResult> {
    tools
        .iter()
        .map(|tool| {
            ToolDetailCollectResult::success(
                tool.tool_id.clone(),
                CLAUDE_CODE_SCHEMA_V1,
                None,
                json!({
                    "workspaceDir": tool.workspace_dir.clone().unwrap_or_default(),
                    "pid": tool.pid,
                    "model": tool.model.clone().unwrap_or_default(),
                    "providerId": tool.provider_id.clone().unwrap_or("anthropic".to_string()),
                    "collectedAt": now_rfc3339_nanos(),
                }),
            )
        })
        .collect()
}
