//! OpenClaw 发现器职责：
//! 1. 从进程命令行识别可接入的 openclaw 进程。
//! 2. 解析模型参数与运行模式，构建实例级 toolId。
//! 3. 产出统一的 `ToolRuntimePayload` 供前端展示。

use yc_shared_protocol::{LatestTokensPayload, ToolRuntimePayload, now_rfc3339_nanos};

use super::ToolDiscoveryContext;

/// 发现所有 OpenClaw 工具实例。
pub(super) fn discover_openclaw_tools(
    context: &ToolDiscoveryContext<'_>,
) -> Vec<ToolRuntimePayload> {
    let mut pids = context
        .all
        .values()
        .filter(|info| crate::is_openclaw_candidate_command(&info.cmd.to_lowercase()))
        .map(|info| info.pid)
        .collect::<Vec<i32>>();
    pids.sort_unstable();
    pids.dedup();

    let mut tools = Vec::with_capacity(pids.len());
    for pid in pids {
        let Some(info) = context.all.get(&pid) else {
            continue;
        };
        // OpenClaw 默认以进程 cwd 作为 workspace。
        let workspace = crate::normalize_path(&info.cwd);
        // 模型参数来自命令行 `--model`。
        let model = crate::parse_cli_flag_value(&info.cmd, "--model").unwrap_or_default();
        let tool_id = crate::build_openclaw_tool_id(&workspace, &info.cmd, pid);

        let reason = if model.trim().is_empty() {
            "已发现 openclaw 进程。".to_string()
        } else {
            format!("已发现 openclaw 进程，模型：{model}")
        };

        tools.push(ToolRuntimePayload {
            tool_id,
            name: "OpenClaw".to_string(),
            category: "DEV_WORKER".to_string(),
            vendor: "OpenClaw".to_string(),
            mode: crate::detect_openclaw_mode(&info.cmd.to_lowercase()).to_string(),
            status: "RUNNING".to_string(),
            connected: true,
            endpoint: String::new(),
            pid: Some(info.pid),
            reason: Some(reason),
            cpu_percent: Some(crate::round2(info.cpu_percent)),
            memory_mb: Some(crate::round2(info.memory_mb)),
            source: Some("openclaw-process-probe".to_string()),
            workspace_dir: crate::option_non_empty(workspace),
            session_id: None,
            session_title: None,
            session_updated_at: None,
            agent_mode: None,
            provider_id: None,
            model_id: None,
            model: crate::option_non_empty(model),
            latest_tokens: Some(LatestTokensPayload::default()),
            model_usage: Vec::new(),
            collected_at: Some(now_rfc3339_nanos()),
        });
    }

    tools
}
