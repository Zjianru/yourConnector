//! Codex 适配器职责：
//! 1. 基于进程命令行发现 Codex CLI 实例。
//! 2. 输出 codex.v1 详情数据，统一接入 Tool Adapter Core。

use std::collections::{HashMap, HashSet};

use serde_json::json;
use yc_shared_protocol::{LatestTokensPayload, ToolRuntimePayload, now_rfc3339_nanos};

use crate::tooling::{
    adapters::CODEX_SCHEMA_V1,
    core::types::{ToolDetailCollectOptions, ToolDetailCollectResult, ToolDiscoveryContext},
};

/// 发现所有 Codex 工具实例。
pub(crate) fn discover(context: &ToolDiscoveryContext<'_>) -> Vec<ToolRuntimePayload> {
    let mut candidate_pids = context
        .all
        .values()
        .filter(|info| crate::is_codex_candidate_command(&info.cmd.to_lowercase()))
        .map(|info| info.pid)
        .collect::<Vec<i32>>();
    candidate_pids.sort_unstable();
    candidate_pids.dedup();

    let candidate_set = candidate_pids.iter().copied().collect::<HashSet<i32>>();
    let mut wrapper_pids = HashSet::<i32>::new();
    for pid in &candidate_pids {
        let has_codex_child = context
            .children_by_ppid
            .get(pid)
            .map(|children| children.iter().any(|child| candidate_set.contains(child)))
            .unwrap_or(false);
        if has_codex_child {
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
        let metadata_cmd = resolve_codex_metadata_cmd(info.cmd.as_str(), pid, &parent_by_pid, context);
        let model = crate::parse_cli_flag_value(metadata_cmd.as_str(), "--model").unwrap_or_default();
        let profile =
            crate::parse_cli_flag_value(metadata_cmd.as_str(), "--profile").unwrap_or_default();
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
            source: Some(format!(
                "codex-process-probe:profile={}",
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

fn build_parent_index(children_by_ppid: &HashMap<i32, Vec<i32>>) -> HashMap<i32, i32> {
    let mut index = HashMap::<i32, i32>::new();
    for (ppid, children) in children_by_ppid {
        for child in children {
            index.insert(*child, *ppid);
        }
    }
    index
}

fn resolve_codex_metadata_cmd(
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
        if crate::is_codex_candidate_command(&parent_cmd.to_lowercase())
            && (crate::parse_cli_flag_value(parent_cmd, "--model").is_some()
                || crate::parse_cli_flag_value(parent_cmd, "--profile").is_some())
        {
            return parent_cmd.to_string();
        }
        current = parent_pid;
    }
    fallback_cmd.to_string()
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
                    "profile": tool
                        .source
                        .as_deref()
                        .and_then(|raw| raw.split("profile=").nth(1))
                        .map(str::trim)
                        .filter(|raw| !raw.is_empty())
                        .unwrap_or("default"),
                    "providerId": tool.provider_id.clone().unwrap_or("openai".to_string()),
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
            1001,
            proc_info(
                1001,
                "node /Users/codez/.nvm/versions/node/v22.21.1/bin/codex --profile team",
                "/workspace/project",
            ),
        );
        all.insert(
            1002,
            proc_info(
                1002,
                "/Users/codez/.nvm/versions/node/v22.21.1/lib/node_modules/@openai/codex/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/codex/codex",
                "/workspace/project",
            ),
        );
        let mut children_by_ppid = HashMap::<i32, Vec<i32>>::new();
        children_by_ppid.insert(1001, vec![1002]);

        let context = ToolDiscoveryContext {
            all: &all,
            children_by_ppid: &children_by_ppid,
        };
        let tools = discover(&context);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].pid, Some(1002));
        assert_eq!(tools[0].workspace_dir.as_deref(), Some("/workspace/project"));
        assert!(
            tools[0]
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("profile=team")
        );
    }
}
