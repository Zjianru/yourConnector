//! OpenCode 适配器职责：
//! 1. 基于进程与本地会话文件发现 OpenCode 工具实例。
//! 2. 输出 opencode.v1 详情数据，统一接入 Tool Adapter Core。

use std::{collections::HashSet, process::Command};

use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use yc_shared_protocol::{ToolRuntimePayload, now_rfc3339_nanos};

use crate::tooling::{
    adapters::OPENCODE_SCHEMA_V1,
    core::types::{ToolDetailCollectOptions, ToolDetailCollectResult, ToolDiscoveryContext},
};

/// 发现所有 OpenCode 工具实例。
pub(crate) fn discover(context: &ToolDiscoveryContext<'_>) -> Vec<ToolRuntimePayload> {
    // 第一层：优先发现 wrapper 进程（`opencode`），并绑定其 runtime 子进程。
    let mut wrapper_pids = context
        .all
        .values()
        .filter(|info| crate::is_opencode_candidate_command(&info.cmd.to_lowercase()))
        .filter(|info| crate::is_opencode_wrapper_command(&info.cmd.to_lowercase()))
        .map(|info| info.pid)
        .collect::<Vec<i32>>();

    wrapper_pids.sort_unstable();
    wrapper_pids.dedup();

    let mut tools = Vec::new();
    let mut claimed_runtime_pids = HashSet::new();

    for wrapper_pid in wrapper_pids {
        let Some(info) = context.all.get(&wrapper_pid) else {
            continue;
        };

        let mut candidate_pids = vec![wrapper_pid];
        if let Some(children) = context.children_by_ppid.get(&wrapper_pid) {
            candidate_pids.extend(children.iter().copied());
        }

        let runtime_pid = crate::pick_runtime_pid(wrapper_pid, &candidate_pids, context.all);
        if let Some(tool) = build_tool_from_process(context, wrapper_pid, runtime_pid, &info.cmd) {
            claimed_runtime_pids.insert(runtime_pid);
            tools.push(tool);
        }
    }

    // 第二层：补齐“独立 runtime 进程”场景（例如 wrapper 已退出但 runtime 仍存活）。
    let mut runtime_only_pids = context
        .all
        .values()
        .filter(|info| crate::is_opencode_candidate_command(&info.cmd.to_lowercase()))
        .filter(|info| is_opencode_runtime_binary(&info.cmd.to_lowercase()))
        .map(|info| info.pid)
        .collect::<Vec<i32>>();
    runtime_only_pids.sort_unstable();
    runtime_only_pids.dedup();

    for runtime_pid in runtime_only_pids {
        if claimed_runtime_pids.contains(&runtime_pid) {
            continue;
        }
        let Some(runtime_info) = context.all.get(&runtime_pid) else {
            continue;
        };
        if let Some(tool) =
            build_tool_from_process(context, runtime_pid, runtime_pid, &runtime_info.cmd)
        {
            claimed_runtime_pids.insert(runtime_pid);
            tools.push(tool);
        }
    }

    tools
}

/// 判断命令是否为 opencode runtime 二进制进程。
fn is_opencode_runtime_binary(cmd_lower: &str) -> bool {
    cmd_lower.contains("opencode-darwin-arm64/bin/opencode")
}

/// 按“wrapper+runtime”构建统一工具快照。
fn build_tool_from_process(
    context: &ToolDiscoveryContext<'_>,
    wrapper_pid: i32,
    runtime_pid: i32,
    cmd_for_mode: &str,
) -> Option<ToolRuntimePayload> {
    let runtime_info = context.all.get(&runtime_pid)?;
    let mode = crate::detect_opencode_mode(&cmd_for_mode.to_lowercase());
    let (host, configured_port) = crate::parse_serve_address(cmd_for_mode);
    let process_cwd = runtime_info.cwd.clone();
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
    let workspace = crate::first_non_empty(&state.workspace_dir, &process_cwd);
    let tool_id = crate::build_opencode_tool_id(&workspace, wrapper_pid);

    Some(ToolRuntimePayload {
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
        cpu_percent: Some(crate::round2(runtime_info.cpu_percent)),
        memory_mb: Some(crate::round2(runtime_info.memory_mb)),
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
    })
}

/// 判断指定工具是否归属于 OpenCode 适配器。
pub(crate) fn matches_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_lowercase();
    let name = tool.name.to_lowercase();
    let vendor = tool.vendor.to_lowercase();
    tool_id.starts_with("opencode_") || name.contains("opencode") || vendor.contains("opencode")
}

/// 采集 OpenCode 详情数据（opencode.v1）。
pub(crate) fn collect_details(
    tools: &[ToolRuntimePayload],
    options: &ToolDetailCollectOptions,
) -> Vec<ToolDetailCollectResult> {
    let mut results = Vec::with_capacity(tools.len());
    let config_json = run_opencode_debug_json(&["debug", "config"]);
    let skill_snapshot = collect_skill_snapshot(config_json.as_ref());
    let mcp_snapshot = collect_mcp_snapshot(config_json.as_ref());

    for tool in tools {
        let workspace = tool.workspace_dir.clone().unwrap_or_default();
        let session_state = crate::collect_opencode_session_state(&workspace);
        let data = json!({
            "workspaceDir": workspace,
            "sessionId": session_state.session_id,
            "sessionTitle": session_state.session_title,
            "sessionUpdatedAt": session_state.session_updated_at,
            "agentMode": session_state.agent_mode,
            "providerId": session_state.provider_id,
            "modelId": session_state.model_id,
            "model": session_state.model,
            "latestTokens": session_state.latest_tokens,
            "modelUsage": session_state.model_usage,
            "skills": skill_snapshot,
            "mcp": mcp_snapshot,
        });

        results.push(ToolDetailCollectResult::success(
            tool.tool_id.clone(),
            OPENCODE_SCHEMA_V1,
            None,
            inject_expire_fields(data, options),
        ));
    }

    results
}

/// 注入 `collectedAt` 与 `expiresAt` 到详情数据体，便于前端直接展示。
fn inject_expire_fields(
    data: serde_json::Value,
    options: &ToolDetailCollectOptions,
) -> serde_json::Value {
    let now = Utc::now();
    let ttl_secs = options.detail_ttl.as_secs().min(i64::MAX as u64) as i64;
    let expires = now + ChronoDuration::seconds(ttl_secs);

    if let Some(mut obj) = data.as_object().cloned() {
        obj.insert(
            "collectedAt".to_string(),
            serde_json::Value::String(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        );
        obj.insert(
            "expiresAt".to_string(),
            serde_json::Value::String(expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        );
        return serde_json::Value::Object(obj);
    }

    data
}

/// 执行 opencode debug 命令并尝试解析 JSON。
fn run_opencode_debug_json(args: &[&str]) -> Option<Value> {
    let output = Command::new("opencode").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<Value>(&output.stdout).ok()
}

/// 收集 skills 详情，并给出启用/未启用分类。
fn collect_skill_snapshot(config_json: Option<&Value>) -> Value {
    let skills_rows = run_opencode_debug_json(&["debug", "skill"])
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut installed = Vec::new();
    let mut enabled = Vec::new();
    let mut disabled = Vec::new();

    for row in skills_rows {
        let name = read_string_path(&row, &["name"]);
        if name.trim().is_empty() {
            continue;
        }
        let allowed = skill_allowed(config_json, &name);
        if allowed {
            enabled.push(name.clone());
        } else {
            disabled.push(name.clone());
        }
        installed.push(json!({
            "name": name,
            "description": read_string_path(&row, &["description"]),
            "location": read_string_path(&row, &["location"]),
        }));
    }

    json!({
        "installed": installed,
        "enabled": enabled,
        "disabled": disabled,
    })
}

/// 收集 mcp 配置，并给出启用/未启用分类。
fn collect_mcp_snapshot(config_json: Option<&Value>) -> Value {
    let mut servers = Vec::new();
    let mut enabled = Vec::new();
    let mut disabled = Vec::new();

    let mcp_obj = config_json
        .and_then(|cfg| cfg.get("mcp"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for (name, row) in mcp_obj {
        let enabled_flag = row
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let command = row
            .get("command")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| item.as_str().map(ToString::to_string))
            .collect::<Vec<String>>();
        if enabled_flag {
            enabled.push(name.clone());
        } else {
            disabled.push(name.clone());
        }
        servers.push(json!({
            "name": name,
            "type": read_string_path(&row, &["type"]),
            "enabled": enabled_flag,
            "command": command,
            "timeout": row.get("timeout").and_then(Value::as_i64).unwrap_or_default(),
        }));
    }

    json!({
        "servers": servers,
        "enabled": enabled,
        "disabled": disabled,
    })
}

/// 判断技能是否被 permission.skill 规则禁用。
fn skill_allowed(config_json: Option<&Value>, skill_name: &str) -> bool {
    let Some(permission_skill) = config_json
        .and_then(|cfg| cfg.get("permission"))
        .and_then(|v| v.get("skill"))
        .and_then(Value::as_object)
    else {
        return true;
    };
    if let Some(action) = permission_skill.get(skill_name).and_then(Value::as_str) {
        return !action.eq_ignore_ascii_case("deny");
    }
    if let Some(action) = permission_skill.get("*").and_then(Value::as_str) {
        return !action.eq_ignore_ascii_case("deny");
    }
    true
}

/// 读取路径字符串。
fn read_string_path(value: &Value, path: &[&str]) -> String {
    let mut cursor = value;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return String::new();
        };
        cursor = next;
    }
    cursor
        .as_str()
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::{collect_mcp_snapshot, collect_skill_snapshot, discover, skill_allowed};
    use crate::{ProcInfo, tooling::core::types::ToolDiscoveryContext};

    fn proc(pid: i32, cmd: &str, cwd: &str) -> ProcInfo {
        ProcInfo {
            pid,
            cmd: cmd.to_string(),
            cwd: cwd.to_string(),
            cpu_percent: 0.0,
            memory_mb: 0.0,
        }
    }

    #[test]
    fn discover_keeps_wrapper_and_standalone_runtime_instances() {
        let mut all = HashMap::new();
        all.insert(100, proc(100, "/opt/homebrew/bin/opencode", "/workspace/a"));
        all.insert(
            101,
            proc(
                101,
                "/opt/homebrew/.../opencode-darwin-arm64/bin/opencode",
                "/workspace/a",
            ),
        );
        all.insert(
            202,
            proc(
                202,
                "/opt/homebrew/.../opencode-darwin-arm64/bin/opencode",
                "/workspace/b",
            ),
        );

        let mut children_by_ppid = HashMap::new();
        children_by_ppid.insert(100, vec![101]);

        let context = ToolDiscoveryContext {
            all: &all,
            children_by_ppid: &children_by_ppid,
        };

        let discovered = discover(&context);
        assert_eq!(discovered.len(), 2);
        assert!(
            discovered
                .iter()
                .any(|item| item.tool_id.ends_with("_p100"))
        );
        assert!(
            discovered
                .iter()
                .any(|item| item.tool_id.ends_with("_p202"))
        );
    }

    #[test]
    fn skill_snapshot_respects_permission_deny() {
        let config = json!({
            "permission": {
                "skill": {
                    "*": "allow",
                    "yc-demo-skill": "deny"
                }
            }
        });
        assert!(!skill_allowed(Some(&config), "yc-demo-skill"));
        assert!(skill_allowed(Some(&config), "other-skill"));
    }

    #[test]
    fn mcp_snapshot_splits_enabled_and_disabled() {
        let config = json!({
            "mcp": {
                "a": {"type": "local", "enabled": true, "command": ["node", "a.js"]},
                "b": {"type": "local", "enabled": false, "command": ["node", "b.js"]}
            }
        });
        let snapshot = collect_mcp_snapshot(Some(&config));
        let enabled = snapshot
            .get("enabled")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let disabled = snapshot
            .get("disabled")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(enabled.len(), 1);
        assert_eq!(disabled.len(), 1);
    }

    #[test]
    fn skill_snapshot_handles_missing_data() {
        let snapshot = collect_skill_snapshot(None);
        assert!(snapshot.get("installed").and_then(|v| v.as_array()).is_some());
        assert!(snapshot.get("enabled").and_then(|v| v.as_array()).is_some());
        assert!(snapshot.get("disabled").and_then(|v| v.as_array()).is_some());
    }
}
