//! Claude Code 适配器职责：
//! 1. 基于进程命令行发现 Claude CLI 实例。
//! 2. 输出 claude-code.v1 详情数据，统一接入 Tool Adapter Core。

use std::collections::{HashMap, HashSet};

use serde_json::json;
use yc_shared_protocol::{LatestTokensPayload, ToolRuntimePayload, now_rfc3339_nanos};

use crate::tooling::{
    adapters::CLAUDE_CODE_SCHEMA_V1,
    core::types::{ToolDetailCollectOptions, ToolDetailCollectResult, ToolDiscoveryContext},
};

/// 发现所有 Claude Code 工具实例。
pub(crate) fn discover(context: &ToolDiscoveryContext<'_>) -> Vec<ToolRuntimePayload> {
    let mut candidate_pids = context
        .all
        .values()
        .filter(|info| crate::is_claude_code_candidate_command(&info.cmd.to_lowercase()))
        .map(|info| info.pid)
        .collect::<Vec<i32>>();
    candidate_pids.sort_unstable();
    candidate_pids.dedup();

    let candidate_set = candidate_pids.iter().copied().collect::<HashSet<i32>>();
    let mut wrapper_pids = HashSet::<i32>::new();
    for pid in &candidate_pids {
        let has_claude_child = context
            .children_by_ppid
            .get(pid)
            .map(|children| children.iter().any(|child| candidate_set.contains(child)))
            .unwrap_or(false);
        if has_claude_child {
            wrapper_pids.insert(*pid);
        }
    }

    let parent_by_pid = build_parent_index(context.children_by_ppid);

    let mut tools = Vec::with_capacity(candidate_pids.len());
    for pid in candidate_pids {
        if wrapper_pids.contains(&pid) {
            continue;
        }
        let Some(info) = context.all.get(&pid) else {
            continue;
        };
        let workspace = crate::normalize_path(&info.cwd);
        let metadata_cmd =
            resolve_claude_metadata_cmd(info.cmd.as_str(), pid, &parent_by_pid, context);
        let model = crate::parse_cli_flag_value(metadata_cmd.as_str(), "--model").unwrap_or_default();
        let profile =
            crate::parse_cli_flag_value(metadata_cmd.as_str(), "--profile").unwrap_or_default();
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
            source: Some(format!(
                "claude-code-process-probe:profile={}",
                if profile.trim().is_empty() {
                    "default"
                } else {
                    profile.trim()
                }
            )),
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

fn build_parent_index(children_by_ppid: &HashMap<i32, Vec<i32>>) -> HashMap<i32, i32> {
    let mut index = HashMap::<i32, i32>::new();
    for (ppid, children) in children_by_ppid {
        for child in children {
            index.insert(*child, *ppid);
        }
    }
    index
}

fn resolve_claude_metadata_cmd(
    fallback_cmd: &str,
    pid: i32,
    parent_by_pid: &HashMap<i32, i32>,
    context: &ToolDiscoveryContext<'_>,
) -> String {
    let mut current = pid;
    for _ in 0..4 {
        let Some(parent_pid) = parent_by_pid.get(&current).copied() else {
            break;
        };
        let Some(parent_info) = context.all.get(&parent_pid) else {
            break;
        };
        let parent_cmd = parent_info.cmd.as_str();
        if crate::is_claude_code_candidate_command(&parent_cmd.to_lowercase())
            && (crate::parse_cli_flag_value(parent_cmd, "--model").is_some()
                || crate::parse_cli_flag_value(parent_cmd, "--profile").is_some())
        {
            return parent_cmd.to_string();
        }
        current = parent_pid;
    }
    fallback_cmd.to_string()
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
                    "profile": tool
                        .source
                        .as_deref()
                        .and_then(|raw| raw.split("profile=").nth(1))
                        .map(str::trim)
                        .filter(|raw| !raw.is_empty())
                        .unwrap_or("default"),
                    "providerId": tool.provider_id.clone().unwrap_or("anthropic".to_string()),
                    "collectedAt": now_rfc3339_nanos(),
                }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{ProcInfo, tooling::core::types::ToolDiscoveryContext};

    use super::discover;

    fn proc_info(pid: i32, cmd: &str, cwd: &str) -> ProcInfo {
        ProcInfo {
            pid,
            cmd: cmd.to_string(),
            cwd: cwd.to_string(),
            cpu_percent: 0.0,
            memory_mb: 0.0,
        }
    }

    #[test]
    fn discover_should_deduplicate_wrapper_and_runtime_process() {
        let mut all = HashMap::<i32, ProcInfo>::new();
        all.insert(
            3001,
            proc_info(
                3001,
                "node /Users/codez/.nvm/versions/node/v22.21.1/bin/claude --profile team",
                "/workspace/project",
            ),
        );
        all.insert(
            3002,
            proc_info(
                3002,
                "/Users/codez/.local/share/claude/vendor/aarch64-apple-darwin/claude/claude",
                "/workspace/project",
            ),
        );
        let mut children_by_ppid = HashMap::<i32, Vec<i32>>::new();
        children_by_ppid.insert(3001, vec![3002]);

        let context = ToolDiscoveryContext {
            all: &all,
            children_by_ppid: &children_by_ppid,
        };
        let tools = discover(&context);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].pid, Some(3002));
        assert_eq!(tools[0].workspace_dir.as_deref(), Some("/workspace/project"));
        assert_eq!(tools[0].name, "Claude Code");
    }
}
