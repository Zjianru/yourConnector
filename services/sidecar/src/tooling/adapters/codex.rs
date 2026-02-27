//! Codex 适配器职责：
//! 1. 基于进程命令行发现 Codex CLI 实例。
//! 2. 输出 codex.v1 详情数据，统一接入 Tool Adapter Core。

use serde_json::json;
use yc_shared_protocol::{LatestTokensPayload, ToolRuntimePayload, now_rfc3339_nanos};

use crate::tooling::{
    adapters::CODEX_SCHEMA_V1,
    core::types::{ToolDetailCollectOptions, ToolDetailCollectResult, ToolDiscoveryContext},
};

/// 发现所有 Codex 工具实例。
pub(crate) fn discover(context: &ToolDiscoveryContext<'_>) -> Vec<ToolRuntimePayload> {
    let mut pids = context
        .all
        .values()
        .filter(|info| crate::is_codex_candidate_command(&info.cmd.to_lowercase()))
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
        let profile = crate::parse_cli_flag_value(&info.cmd, "--profile").unwrap_or_default();
        let tool_id = crate::build_codex_tool_id(workspace.as_str(), pid);
        let mut reason = "已发现 codex 进程".to_string();
        if !profile.trim().is_empty() {
            reason = format!("已发现 codex 进程，profile={profile}");
        }

        tools.push(ToolRuntimePayload {
            tool_id,
            name: "Codex".to_string(),
            tool_class: "code".to_string(),
            category: "CODE_AGENT".to_string(),
            vendor: "OpenAI".to_string(),
            mode: "CLI".to_string(),
            status: "RUNNING".to_string(),
            connected: true,
            endpoint: String::new(),
            pid: Some(pid),
            reason: crate::option_non_empty(reason),
            cpu_percent: Some(crate::round2(info.cpu_percent)),
            memory_mb: Some(crate::round2(info.memory_mb)),
            source: Some("codex-process-probe".to_string()),
            workspace_dir: crate::option_non_empty(workspace),
            session_id: None,
            session_title: None,
            session_updated_at: None,
            agent_mode: Some("cli".to_string()),
            provider_id: Some("openai".to_string()),
            model_id: crate::option_non_empty(model.clone()),
            model: crate::option_non_empty(model),
            latest_tokens: Some(LatestTokensPayload::default()),
            model_usage: Vec::new(),
            collected_at: Some(now_rfc3339_nanos()),
        });
    }
    tools
}

/// 判断指定工具是否归属于 Codex 适配器。
pub(crate) fn matches_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_ascii_lowercase();
    let name = tool.name.to_ascii_lowercase();
    let vendor = tool.vendor.to_ascii_lowercase();
    tool_id.starts_with("codex_") || name.contains("codex") || vendor.contains("openai")
}

/// 采集 Codex 详情（codex.v1）。
pub(crate) fn collect_details(
    tools: &[ToolRuntimePayload],
    _options: &ToolDetailCollectOptions,
) -> Vec<ToolDetailCollectResult> {
    tools
        .iter()
        .map(|tool| {
            ToolDetailCollectResult::success(
                tool.tool_id.clone(),
                CODEX_SCHEMA_V1,
                None,
                json!({
                    "workspaceDir": tool.workspace_dir.clone().unwrap_or_default(),
                    "pid": tool.pid,
                    "model": tool.model.clone().unwrap_or_default(),
                    "providerId": tool.provider_id.clone().unwrap_or("openai".to_string()),
                    "collectedAt": now_rfc3339_nanos(),
                }),
            )
        })
        .collect()
}
