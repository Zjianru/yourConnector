//! OpenCode 发现器职责：
//! 1. 从系统进程中定位 opencode wrapper/runtime 进程。
//! 2. 解析运行模式、端口、工作区与会话状态。
//! 3. 生成前端可直接展示的 `ToolRuntimePayload`。

use yc_shared_protocol::{ToolRuntimePayload, now_rfc3339_nanos};

use super::ToolDiscoveryContext;

/// 发现所有 OpenCode 工具实例。
pub(super) fn discover_opencode_tools(
    context: &ToolDiscoveryContext<'_>,
) -> Vec<ToolRuntimePayload> {
    // 先找 wrapper 进程（`/bin/opencode`），作为 runtime 归属起点。
    let mut wrapper_pids = context
        .all
        .values()
        .filter(|info| crate::is_opencode_candidate_command(&info.cmd.to_lowercase()))
        .filter(|info| crate::is_opencode_wrapper_command(&info.cmd.to_lowercase()))
        .map(|info| info.pid)
        .collect::<Vec<i32>>();

    // 兼容某些环境下 wrapper 识别不到的场景，回退到 runtime 命令特征。
    if wrapper_pids.is_empty() {
        wrapper_pids.extend(
            context
                .all
                .values()
                .filter(|info| {
                    info.cmd
                        .to_lowercase()
                        .contains("opencode-darwin-arm64/bin/opencode")
                })
                .map(|info| info.pid),
        );
    }

    wrapper_pids.sort_unstable();
    wrapper_pids.dedup();

    let mut tools = Vec::with_capacity(wrapper_pids.len());
    for wrapper_pid in wrapper_pids {
        let Some(info) = context.all.get(&wrapper_pid) else {
            continue;
        };
        // 命令模式与 serve 地址来自 wrapper 命令行。
        let mode = crate::detect_opencode_mode(&info.cmd.to_lowercase());
        let (host, configured_port) = crate::parse_serve_address(&info.cmd);
        let mut candidate_pids = vec![wrapper_pid];
        if let Some(children) = context.children_by_ppid.get(&wrapper_pid) {
            candidate_pids.extend(children.iter().copied());
        }

        let runtime_pid = crate::pick_runtime_pid(wrapper_pid, &candidate_pids, context.all);
        let process_cwd = context
            .all
            .get(&runtime_pid)
            .map(|proc_info| proc_info.cwd.clone())
            .unwrap_or_default();
        // 会话状态从 opencode 本地存储解析，支持缓存。
        let state = crate::collect_opencode_session_state(&process_cwd);

        let endpoint = if configured_port > 0 {
            format!(
                "http://{}:{}",
                crate::normalize_probe_host(&host),
                configured_port
            )
        } else {
            String::new()
        };

        let (connected, status, reason) = crate::evaluate_opencode_connection(mode, &state);
        let (tool_cpu, tool_mem) = context
            .all
            .get(&runtime_pid)
            .map(|proc_info| (proc_info.cpu_percent, proc_info.memory_mb))
            .unwrap_or((0.0, 0.0));
        // workspace 优先取会话目录，缺失时回退进程 cwd。
        let workspace = crate::first_non_empty(&state.workspace_dir, &process_cwd);
        let tool_id = crate::build_opencode_tool_id(&workspace, wrapper_pid);

        tools.push(ToolRuntimePayload {
            tool_id,
            name: "OpenCode".to_string(),
            category: "CODE_AGENT".to_string(),
            vendor: "OpenCode".to_string(),
            mode: mode.to_string(),
            status: status.to_string(),
            connected,
            endpoint,
            pid: Some(runtime_pid),
            reason: crate::option_non_empty(reason),
            cpu_percent: Some(crate::round2(tool_cpu)),
            memory_mb: Some(crate::round2(tool_mem)),
            source: Some("opencode-session-probe".to_string()),
            workspace_dir: crate::option_non_empty(workspace),
            session_id: crate::option_non_empty(state.session_id),
            session_title: crate::option_non_empty(state.session_title),
            session_updated_at: crate::option_non_empty(state.session_updated_at),
            agent_mode: crate::option_non_empty(state.agent_mode),
            provider_id: crate::option_non_empty(state.provider_id),
            model_id: crate::option_non_empty(state.model_id),
            model: crate::option_non_empty(state.model),
            latest_tokens: Some(state.latest_tokens),
            model_usage: state.model_usage,
            collected_at: Some(now_rfc3339_nanos()),
        });
    }

    tools
}
