//! OpenClaw 适配器职责：
//! 1. 从进程命令行发现 openclaw/openclaw-gateway 实例并构建实例级 toolId。
//! 2. 采集 OpenClaw 运行态数据并组装 `openclaw.v1` 结构化详情。
//! 3. 在采集失败时仅标记 stale，不清空最近一次成功数据。
//! 4. 仅读取非敏感本地配置白名单字段（上下文/模型窗口/费率）。

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    env, fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};
use futures_util::{StreamExt, stream};
use serde_json::{Map, Value, json};
use tokio::{process::Command, time::timeout};
use yc_shared_protocol::{LatestTokensPayload, ToolRuntimePayload, now_rfc3339_nanos};

use crate::tooling::{
    adapters::OPENCLAW_SCHEMA_V1,
    core::types::{ToolDetailCollectOptions, ToolDetailCollectResult, ToolDiscoveryContext},
};

/// `status --json --usage` 的超时上限（毫秒）。
const STATUS_TIMEOUT_CAP_MS: u64 = 8_000;
/// `health --json` 的超时上限（毫秒）。
const HEALTH_TIMEOUT_CAP_MS: u64 = 6_000;
/// `channels status --json` 的超时上限（毫秒）。
const CHANNELS_TIMEOUT_CAP_MS: u64 = 5_000;
/// `gateway status --json` 的超时上限（毫秒）。
const GATEWAY_TIMEOUT_CAP_MS: u64 = 8_000;
/// `memory status --json` 的超时上限（毫秒）。
const MEMORY_TIMEOUT_CAP_MS: u64 = 6_000;
/// `security audit --json` 的超时上限（毫秒）。
const SECURITY_TIMEOUT_CAP_MS: u64 = 6_000;
/// `models status --json` 的超时上限（毫秒）。
const MODELS_STATUS_TIMEOUT_CAP_MS: u64 = 6_000;
/// `agents/sessions` 的超时上限（毫秒）。
const AGENTS_SESSIONS_TIMEOUT_CAP_MS: u64 = 2_500;
/// Usage 页默认统计窗口（秒）。
const USAGE_WINDOW_SEC_1H: i64 = 3600;
/// 会话“长时间未更新”阈值（秒）。
const INACTIVE_SESSION_SEC: i64 = 6 * 3600;
/// 会话“24 小时活跃”阈值（秒）。
const ACTIVE_SESSION_24H_SEC: i64 = 24 * 3600;

/// 单模型费率配置（来自 openclaw.json 白名单字段）。
#[derive(Debug, Clone, Default)]
struct ModelPricing {
    /// provider 标识。
    provider: String,
    /// 模型 id。
    model_id: String,
    /// 模型展示名。
    model_name: String,
    /// 模型上下文窗口。
    context_window: i64,
    /// 输入 token 单价（按每百万 token）。
    input_rate: f64,
    /// 输出 token 单价（按每百万 token）。
    output_rate: f64,
    /// cache read 单价（按每百万 token）。
    cache_read_rate: f64,
    /// cache write 单价（按每百万 token）。
    cache_write_rate: f64,
}

/// Profile 级本地配置白名单数据。
#[derive(Debug, Clone, Default)]
struct LocalProfileConfig {
    /// agents.defaults.contextTokens。
    default_context_tokens: i64,
    /// 模型费率与窗口配置。
    models: Vec<ModelPricing>,
}

/// 发现所有 OpenClaw 工具实例。
pub(crate) fn discover(context: &ToolDiscoveryContext<'_>) -> Vec<ToolRuntimePayload> {
    let mut pids = context
        .all
        .values()
        .filter(|info| crate::is_openclaw_candidate_command(&info.cmd.to_lowercase()))
        .map(|info| info.pid)
        .collect::<Vec<i32>>();
    pids.sort_unstable();
    pids.dedup();

    // `openclaw` 常作为父进程拉起 `openclaw-gateway`；
    // 当父子同时存在时，只保留 gateway，避免候选列表重复与闪烁。
    let shadowed_cli_pids = find_gateway_shadowed_cli_pids(&pids, context);

    let mut tools = Vec::with_capacity(pids.len());
    for pid in pids {
        if shadowed_cli_pids.contains(&pid) {
            continue;
        }
        let Some(info) = context.all.get(&pid) else {
            continue;
        };

        let cmd_lower = info.cmd.to_lowercase();
        let workspace = crate::normalize_path(&info.cwd);
        let model = crate::parse_cli_flag_value(&info.cmd, "--model")
            .or_else(|| crate::parse_cli_flag_value(&info.cmd, "-m"))
            .unwrap_or_default();
        let tool_id = crate::build_openclaw_tool_id(&workspace, &info.cmd, pid);
        let mode = crate::detect_openclaw_mode(&cmd_lower);
        let (connected, status, reason) = crate::evaluate_openclaw_connection(mode, &model);
        let profile_key = parse_profile_key_from_cmd(&info.cmd);

        tools.push(ToolRuntimePayload {
            tool_id,
            name: "OpenClaw".to_string(),
            tool_class: "assistant".to_string(),
            category: "DEV_WORKER".to_string(),
            vendor: "OpenClaw".to_string(),
            mode: mode.to_string(),
            status: status.to_string(),
            connected,
            endpoint: String::new(),
            pid: Some(info.pid),
            reason: crate::option_non_empty(reason),
            cpu_percent: Some(crate::round2(info.cpu_percent)),
            memory_mb: Some(crate::round2(info.memory_mb)),
            source: Some(format!("openclaw-process-probe:profile={profile_key}")),
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

/// 在候选进程集合中找出应被 gateway 子进程覆盖的 openclaw 父进程。
fn find_gateway_shadowed_cli_pids(
    candidate_pids: &[i32],
    context: &ToolDiscoveryContext<'_>,
) -> HashSet<i32> {
    let mut shadowed = HashSet::new();
    for pid in candidate_pids {
        let Some(parent_info) = context.all.get(pid) else {
            continue;
        };
        let parent_cmd_lower = parent_info.cmd.to_lowercase();
        if !crate::is_openclaw_candidate_command(&parent_cmd_lower)
            || is_openclaw_gateway_process(&parent_cmd_lower)
        {
            continue;
        }
        let has_gateway_child = context
            .children_by_ppid
            .get(pid)
            .map(|children| {
                children.iter().any(|child_pid| {
                    context
                        .all
                        .get(child_pid)
                        .map(|child| is_openclaw_gateway_process(&child.cmd.to_lowercase()))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if has_gateway_child {
            shadowed.insert(*pid);
        }
    }
    shadowed
}

/// 判断进程命令是否为 openclaw gateway。
fn is_openclaw_gateway_process(cmd_lower: &str) -> bool {
    cmd_lower.contains("openclaw-gateway")
}

/// 判断指定工具是否归属于 OpenClaw 适配器。
pub(crate) fn matches_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_lowercase();
    let name = tool.name.to_lowercase();
    let vendor = tool.vendor.to_lowercase();
    tool_id.starts_with("openclaw_") || name.contains("openclaw") || vendor.contains("openclaw")
}

/// 采集 OpenClaw 详情（openclaw.v1）。
///
/// 分层策略：
/// 1. 慢周期：固定采 `status --json --usage` + `agents list --json --bindings` + `channels status --json`。
/// 2. 兜底层：当 status 缺 recent sessions 时补采 `sessions --json`。
/// 3. 按需层：仅当 `include_deep_details=true` 时补采 gateway/memory/security。
///    health 固定纳入慢层采集，用于渠道身份与健康摘要。
pub(crate) async fn collect_details(
    tools: &[ToolRuntimePayload],
    options: &ToolDetailCollectOptions,
    include_deep_details: bool,
) -> Vec<ToolDetailCollectResult> {
    let mut grouped: HashMap<String, Vec<ToolRuntimePayload>> = HashMap::new();
    for tool in tools {
        let profile_key = parse_profile_key_from_tool(tool);
        grouped.entry(profile_key).or_default().push(tool.clone());
    }

    let max_parallel = options.max_parallel.max(1);
    stream::iter(grouped.into_iter())
        .map(|(profile_key, profile_tools)| async move {
            collect_profile_details(&profile_key, &profile_tools, options, include_deep_details)
                .await
        })
        .buffer_unordered(max_parallel)
        .collect::<Vec<Vec<ToolDetailCollectResult>>>()
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// 采集单 profile 的详情并映射到各工具实例。
async fn collect_profile_details(
    profile_key: &str,
    tools: &[ToolRuntimePayload],
    options: &ToolDetailCollectOptions,
    include_deep_details: bool,
) -> Vec<ToolDetailCollectResult> {
    let status_timeout = effective_timeout(options.command_timeout, STATUS_TIMEOUT_CAP_MS);
    let status_json = match run_status_json(profile_key, status_timeout).await {
        Ok(value) => value,
        Err(err) => {
            return tools
                .iter()
                .map(|tool| {
                    ToolDetailCollectResult::failed(
                        tool.tool_id.clone(),
                        OPENCLAW_SCHEMA_V1,
                        Some(profile_key.to_string()),
                        format!("openclaw status 采集失败：{err}"),
                    )
                })
                .collect();
        }
    };

    let profile_config = load_profile_config_whitelist(profile_key);
    let model_lookup = build_model_lookup(&profile_config.models);

    let agents_timeout = effective_timeout(options.command_timeout, AGENTS_SESSIONS_TIMEOUT_CAP_MS);
    let agents_list_json = run_openclaw_json(
        profile_key,
        &["agents", "list", "--json", "--bindings"],
        agents_timeout,
    )
    .await
    .ok();

    let channels_timeout = effective_timeout(options.command_timeout, CHANNELS_TIMEOUT_CAP_MS);
    let channels_status_json = run_openclaw_json(
        profile_key,
        &["channels", "status", "--json"],
        channels_timeout,
    )
    .await
    .ok();
    let models_status_timeout =
        effective_timeout(options.command_timeout, MODELS_STATUS_TIMEOUT_CAP_MS);
    let models_status_json = run_openclaw_json(
        profile_key,
        &["models", "status", "--json"],
        models_status_timeout,
    )
    .await
    .ok();

    let sessions_json = run_openclaw_json(profile_key, &["sessions", "--json"], agents_timeout)
        .await
        .ok();
    let mut sessions_all = sessions_json
        .as_ref()
        .map(parse_sessions_rows_from_command)
        .filter(|rows| !rows.is_empty())
        .unwrap_or_else(|| parse_status_recent_sessions(&status_json));
    sessions_all.sort_by_key(|row| Reverse(read_i64(row, "updatedAt")));
    sessions_all = dedupe_sessions_by_identity(&sessions_all);

    let usage_window_to_ms = now_epoch_sec().saturating_mul(1000);
    let usage_window_from_ms = usage_window_to_ms.saturating_sub(USAGE_WINDOW_SEC_1H * 1000);
    let sessions_in_usage_window =
        filter_sessions_by_updated_window(&sessions_all, usage_window_from_ms, usage_window_to_ms);

    let default_agent_id = parse_status_default_agent_id(&status_json);
    let heartbeat_by_agent = parse_heartbeat_agents(&status_json);
    let sessions_default_context =
        read_i64_path(&status_json, &["sessions", "defaults", "contextTokens"]);
    let status_agents = parse_status_agents(&status_json, &heartbeat_by_agent, &default_agent_id);
    let agent_list = parse_agents_list(agents_list_json.as_ref());
    let merged_agents = merge_agents(
        status_agents,
        agent_list,
        &default_agent_id,
        &sessions_all,
        sessions_default_context,
    );

    let auth_user_by_provider = parse_auth_user_by_provider(models_status_json.as_ref());
    let usage_provider_windows = parse_usage_windows(&status_json, &auth_user_by_provider);
    let usage_model_totals = aggregate_model_totals(&sessions_in_usage_window, &model_lookup);
    let usage_estimated_cost = estimate_model_cost(&usage_model_totals, &model_lookup);
    let usage_configured_models = build_configured_model_rows(&profile_config.models);
    let usage_merged_models = merge_usage_model_rows(
        &usage_configured_models,
        &usage_model_totals,
        &usage_estimated_cost,
        usage_window_from_ms,
        usage_window_to_ms,
    );
    let (usage_models_with_cost, usage_models_without_cost) =
        split_usage_model_rows_by_activity(&usage_merged_models);
    let usage_api_provider_cards = build_usage_api_provider_cards(
        &usage_models_with_cost,
        &usage_provider_windows,
        usage_window_from_ms,
        usage_window_to_ms,
    );
    let usage_coverage = build_usage_coverage(
        &usage_provider_windows,
        &usage_models_with_cost,
        &usage_models_without_cost,
    );
    let usage_headline = build_usage_headline(
        &usage_provider_windows,
        &usage_model_totals,
        &usage_estimated_cost,
    );

    let health_timeout = effective_timeout(options.command_timeout, HEALTH_TIMEOUT_CAP_MS);
    let health_status = run_openclaw_json(profile_key, &["health", "--json"], health_timeout)
        .await
        .ok();

    let channel_identities = parse_channel_identities(
        channels_status_json.as_ref(),
        health_status.as_ref(),
        &status_json,
    );
    let channel_overview = parse_channel_overview(channels_status_json.as_ref());

    let (gateway_status, memory_status, security_status) = if include_deep_details {
        let gateway_timeout = effective_timeout(options.command_timeout, GATEWAY_TIMEOUT_CAP_MS);
        let memory_timeout = effective_timeout(options.command_timeout, MEMORY_TIMEOUT_CAP_MS);
        let security_timeout = effective_timeout(options.command_timeout, SECURITY_TIMEOUT_CAP_MS);

        (
            run_openclaw_json(
                profile_key,
                &["gateway", "status", "--json"],
                gateway_timeout,
            )
            .await
            .ok(),
            run_openclaw_json(profile_key, &["memory", "status", "--json"], memory_timeout)
                .await
                .ok(),
            run_openclaw_json(
                profile_key,
                &["security", "audit", "--json"],
                security_timeout,
            )
            .await
            .ok(),
        )
    } else {
        (None, None, None)
    };

    let health_summary = parse_health_summary(health_status.as_ref());
    let gateway_runtime = parse_gateway_runtime(&status_json, gateway_status.as_ref());
    let security_summary = parse_security_summary(&status_json, security_status.as_ref());
    let security_findings = parse_security_findings(&status_json, security_status.as_ref());
    let memory_index = parse_memory_index(&status_json, memory_status.as_ref());
    let dashboard_meta = parse_dashboard_meta(&status_json, gateway_status.as_ref());

    tools
        .iter()
        .map(|tool| {
            let workspace = tool.workspace_dir.clone().unwrap_or_default();
            let scoped_agents = select_agents_by_workspace(&merged_agents, &workspace);
            let scoped_sessions = select_sessions_by_agents(&sessions_all, &scoped_agents);
            let sessions_payload = build_sessions_payload(&scoped_sessions);
            let scoped_agents = attach_agent_context_metrics(
                scoped_agents,
                &scoped_sessions,
                profile_config.default_context_tokens,
                &model_lookup,
            );

            let overview = build_overview(
                &status_json,
                &default_agent_id,
                &scoped_agents,
                &channel_identities,
                &sessions_payload,
                usage_headline.clone(),
                dashboard_meta.clone(),
            );

            let usage_payload = json!({
                "windowPreset": "1h",
                "windowFromMs": usage_window_from_ms,
                "windowToMs": usage_window_to_ms,
                "authWindows": usage_provider_windows,
                "apiProviderCards": usage_api_provider_cards,
                "modelsWithCost": usage_models_with_cost,
                "modelsWithoutCost": usage_models_without_cost,
                "configuredModels": usage_configured_models,
                "providerWindows": usage_provider_windows,
                "modelTotals": usage_model_totals,
                "estimatedCost": usage_estimated_cost,
                "coverage": usage_coverage,
            });

            let system_service = json!({
                "memoryIndex": memory_index,
                "securitySummary": security_summary,
                "securityFindings": security_findings,
                "gatewayRuntime": gateway_runtime,
                "healthSummary": health_summary,
            });

            let data = json!({
                "overview": overview,
                "agents": scoped_agents,
                "sessions": sessions_payload,
                "usage": usage_payload,
                "systemService": system_service,
                "statusDots": {
                    "gateway": parse_gateway_status_dot(&status_json, gateway_status.as_ref()),
                    "data": "fresh"
                },
                "workspaceDir": workspace,
                // 向后兼容字段，避免旧 UI 临时读取失败。
                "channelOverview": channel_overview,
                "healthSummary": health_summary,
            });

            ToolDetailCollectResult::success(
                tool.tool_id.clone(),
                OPENCLAW_SCHEMA_V1,
                Some(profile_key.to_string()),
                data,
            )
        })
        .collect()
}

/// 运行 status：优先 `--usage`，失败时自动降级到纯 status。
async fn run_status_json(profile_key: &str, command_timeout: Duration) -> Result<Value> {
    match run_openclaw_json(
        profile_key,
        &["status", "--json", "--usage"],
        command_timeout,
    )
    .await
    {
        Ok(value) => Ok(value),
        Err(_) => run_openclaw_json(profile_key, &["status", "--json"], command_timeout).await,
    }
}

/// 按“全局超时 + 命令级上限”计算本次命令的有效超时。
fn effective_timeout(global_timeout: Duration, command_cap_ms: u64) -> Duration {
    let command_cap = Duration::from_millis(command_cap_ms.max(1));
    if global_timeout.is_zero() {
        return command_cap;
    }
    global_timeout.min(command_cap)
}

/// 执行 openclaw 子命令并解析 JSON 输出。
async fn run_openclaw_json(
    profile_key: &str,
    args: &[&str],
    command_timeout: Duration,
) -> Result<Value> {
    let mut command = Command::new("openclaw");
    apply_profile_args(profile_key, &mut command);
    command.args(args);

    let output = timeout(command_timeout, command.output())
        .await
        .map_err(|_| anyhow!("命令执行超时（{}ms）", command_timeout.as_millis()))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let short = stderr
            .lines()
            .next()
            .unwrap_or("openclaw command failed")
            .trim();
        return Err(anyhow!(short.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err(anyhow!("命令输出为空"));
    }

    serde_json::from_str::<Value>(&stdout).map_err(|err| anyhow!(format!("JSON 解析失败: {err}")))
}

/// 根据 profileKey 注入 `--profile` 或 `--dev` 参数。
fn apply_profile_args(profile_key: &str, command: &mut Command) {
    if profile_key == "dev" {
        command.arg("--dev");
        return;
    }
    if profile_key != "default" && !profile_key.trim().is_empty() {
        command.arg("--profile");
        command.arg(profile_key);
    }
}

/// 从命令行解析 profileKey。
fn parse_profile_key_from_cmd(cmd: &str) -> String {
    let tokens = cmd.split_whitespace().collect::<Vec<&str>>();

    if tokens.contains(&"--dev") {
        return "dev".to_string();
    }

    let mut idx = 0usize;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token == "--profile" && idx + 1 < tokens.len() {
            let value = tokens[idx + 1].trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
        if let Some(value) = token.strip_prefix("--profile=") {
            let normalized = value.trim();
            if !normalized.is_empty() {
                return normalized.to_string();
            }
        }
        idx += 1;
    }

    "default".to_string()
}

/// 从 tool source 中提取 profileKey；缺失时回退 default。
fn parse_profile_key_from_tool(tool: &ToolRuntimePayload) -> String {
    let source = tool.source.clone().unwrap_or_default();
    let marker = "profile=";
    if let Some(pos) = source.find(marker) {
        let value = source[(pos + marker.len())..].trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    "default".to_string()
}

/// 根据 profileKey 推导本地状态目录。
fn resolve_profile_state_dir(profile_key: &str) -> PathBuf {
    let home = env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    match profile_key.trim() {
        "dev" => home.join(".openclaw-dev"),
        "" | "default" => home.join(".openclaw"),
        name => home.join(format!(".openclaw-{name}")),
    }
}

/// 读取 openclaw.json 白名单字段（上下文/模型窗口/费率）。
fn load_profile_config_whitelist(profile_key: &str) -> LocalProfileConfig {
    let state_dir = resolve_profile_state_dir(profile_key);
    let config_path = state_dir.join("openclaw.json");
    let raw = fs::read_to_string(config_path).ok();
    let Some(text) = raw else {
        return LocalProfileConfig::default();
    };
    let parsed = serde_json::from_str::<Value>(&text).ok();
    let Some(value) = parsed else {
        return LocalProfileConfig::default();
    };

    let default_context_tokens = read_i64_path(&value, &["agents", "defaults", "contextTokens"]);
    let mut models = Vec::new();

    let providers = value
        .get("models")
        .and_then(|raw| raw.get("providers"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for (provider_id, provider_cfg) in providers {
        let model_rows = provider_cfg
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for model_row in model_rows {
            let model_id = read_string_or(&model_row, "id", "key");
            let model_name = read_string_or(&model_row, "name", "id");
            if model_id.is_empty() && model_name.is_empty() {
                continue;
            }
            let rates = model_row.get("cost").cloned().unwrap_or_else(|| json!({}));
            models.push(ModelPricing {
                provider: provider_id.clone(),
                model_id,
                model_name,
                context_window: read_i64(&model_row, "contextWindow"),
                input_rate: read_f64(&rates, "input"),
                output_rate: read_f64(&rates, "output"),
                cache_read_rate: read_f64(&rates, "cacheRead"),
                cache_write_rate: read_f64(&rates, "cacheWrite"),
            });
        }
    }

    LocalProfileConfig {
        default_context_tokens,
        models,
    }
}

/// 建立模型查找索引（id/name -> pricing）。
fn build_model_lookup(models: &[ModelPricing]) -> HashMap<String, ModelPricing> {
    let mut lookup = HashMap::new();
    for row in models {
        let by_id = normalize_lookup_key(&row.model_id);
        if !by_id.is_empty() {
            lookup.insert(by_id, row.clone());
        }
        let by_name = normalize_lookup_key(&row.model_name);
        if !by_name.is_empty() {
            lookup.entry(by_name).or_insert_with(|| row.clone());
        }
    }
    lookup
}

/// 归一化模型查找键。
fn normalize_lookup_key(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

/// 从 status 中读取 defaultAgentId，优先 heartbeat.defaultAgentId。
fn parse_status_default_agent_id(status_json: &Value) -> String {
    let heartbeat_default = read_string_path(status_json, &["heartbeat", "defaultAgentId"]);
    if !heartbeat_default.is_empty() {
        return heartbeat_default;
    }
    read_string_path(status_json, &["agents", "defaultId"])
}

/// 从 status 中读取 heartbeat agent 配置（enabled/everyMs）。
fn parse_heartbeat_agents(status_json: &Value) -> HashMap<String, (bool, i64)> {
    let mut map = HashMap::new();
    let list = read_array_path(status_json, &["heartbeat", "agents"]);
    for raw in list {
        let agent_id = read_string(&raw, "agentId");
        if agent_id.is_empty() {
            continue;
        }
        map.insert(
            agent_id,
            (read_bool(&raw, "enabled"), read_i64(&raw, "everyMs")),
        );
    }
    map
}

/// 解析 status.agents.agents[]。
fn parse_status_agents(
    status_json: &Value,
    heartbeat_by_agent: &HashMap<String, (bool, i64)>,
    default_agent_id: &str,
) -> Vec<Value> {
    let list = read_array_path(status_json, &["agents", "agents"]);
    let mut rows = Vec::with_capacity(list.len());

    for raw in list {
        let agent_id = read_string_or(&raw, "id", "agentId");
        if agent_id.is_empty() {
            continue;
        }
        let (heartbeat_enabled, heartbeat_every_ms) = heartbeat_by_agent
            .get(&agent_id)
            .copied()
            .unwrap_or((false, 0));

        rows.push(json!({
            "agentId": agent_id,
            "name": read_string_or(&raw, "name", "id"),
            "model": read_string(&raw, "model"),
            "workspaceDir": read_string(&raw, "workspaceDir"),
            "sessionsCount": read_i64(&raw, "sessionsCount"),
            "lastUpdatedAt": read_i64_or_null(&raw, "lastUpdatedAt"),
            "isDefault": read_string_or(&raw, "id", "agentId") == default_agent_id,
            "heartbeatEnabled": heartbeat_enabled,
            "heartbeatEveryMs": if heartbeat_every_ms > 0 {
                json!(heartbeat_every_ms)
            } else {
                Value::Null
            },
            "bindings": 0,
            "routes": [],
        }));
    }

    rows
}

/// 解析 agents list --json --bindings。
fn parse_agents_list(agents_list_json: Option<&Value>) -> Vec<Value> {
    let Some(list_json) = agents_list_json else {
        return Vec::new();
    };
    let Some(list) = list_json.as_array() else {
        return Vec::new();
    };

    let mut rows = Vec::with_capacity(list.len());
    for raw in list {
        let agent_id = read_string_or(raw, "id", "agentId");
        if agent_id.is_empty() {
            continue;
        }
        rows.push(json!({
            "agentId": agent_id,
            "name": read_string_or(raw, "name", "id"),
            "model": read_string(raw, "model"),
            "workspaceDir": read_string_or(raw, "workspace", "workspaceDir"),
            "isDefault": read_bool(raw, "isDefault"),
            "bindings": read_i64(raw, "bindings"),
            "routes": raw.get("routes").cloned().unwrap_or_else(|| json!([])),
            "identityName": read_string(raw, "identityName"),
            "identityEmoji": read_string(raw, "identityEmoji"),
        }));
    }
    rows
}

/// 从 status.sessions.recent 解析 sessions。
fn parse_status_recent_sessions(status_json: &Value) -> Vec<Value> {
    read_array_path(status_json, &["sessions", "recent"])
        .into_iter()
        .filter_map(|raw| parse_session_row(&raw))
        .collect()
}

/// 从 sessions --json 解析 sessions（兜底路径）。
fn parse_sessions_rows_from_command(sessions_json: &Value) -> Vec<Value> {
    let Some(list) = sessions_json.get("sessions").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(parse_session_row)
        .collect::<Vec<Value>>()
}

/// 把单条 session 归一化为统一字段。
fn parse_session_row(raw: &Value) -> Option<Value> {
    let session_id = read_string(raw, "sessionId");
    let key = read_string(raw, "key");
    if session_id.is_empty() && key.is_empty() {
        return None;
    }

    let mut agent_id = read_string(raw, "agentId");
    if agent_id.is_empty() {
        agent_id = parse_agent_id_from_session_key(&key);
    }

    Some(json!({
        "key": key,
        "kind": read_string(raw, "kind"),
        "flags": read_string_array(raw, "flags"),
        "sessionId": session_id,
        "agentId": agent_id,
        "model": read_string(raw, "model"),
        "modelProvider": read_string(raw, "modelProvider"),
        "inputTokens": read_i64(raw, "inputTokens"),
        "outputTokens": read_i64(raw, "outputTokens"),
        "cacheRead": read_i64(raw, "cacheRead"),
        "cacheWrite": read_i64(raw, "cacheWrite"),
        "totalTokens": read_i64(raw, "totalTokens"),
        "totalTokensFresh": read_bool(raw, "totalTokensFresh"),
        "contextTokens": read_i64(raw, "contextTokens"),
        "remainingTokens": read_i64(raw, "remainingTokens"),
        "percentUsed": read_i64(raw, "percentUsed"),
        "updatedAt": read_i64(raw, "updatedAt"),
        "ageMs": read_i64(raw, "age"),
        "systemSent": read_bool(raw, "systemSent"),
        "abortedLastRun": read_bool(raw, "abortedLastRun"),
    }))
}

/// 从 session key 中解析 agentId（格式：agent:<id>:...）。
fn parse_agent_id_from_session_key(key: &str) -> String {
    let mut parts = key.split(':');
    let first = parts.next().unwrap_or_default();
    if first != "agent" {
        return String::new();
    }
    parts.next().unwrap_or_default().trim().to_string()
}

/// 解析 provider -> auth 用户名映射（来自 `openclaw models status --json`）。
fn parse_auth_user_by_provider(models_status_json: Option<&Value>) -> HashMap<String, String> {
    let Some(raw) = models_status_json else {
        return HashMap::new();
    };

    let mut rows = HashMap::new();
    let providers = read_array_path(raw, &["auth", "oauth", "providers"]);
    for provider_row in providers {
        let provider = normalize_lookup_key(&read_string(&provider_row, "provider"));
        if provider.is_empty() {
            continue;
        }
        let profiles = provider_row
            .get("profiles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut username = String::new();
        for profile in profiles {
            let profile_id = read_string(&profile, "profileId");
            let label = read_string(&profile, "label");
            username = parse_auth_user_name(&profile_id, &label);
            if !username.is_empty() {
                break;
            }
        }
        if !username.is_empty() {
            rows.insert(provider, username);
        }
    }

    rows
}

/// 从 profileId/label 中提取 auth 用户名（优先具体用户名，回退 profile 后缀）。
fn parse_auth_user_name(profile_id: &str, label: &str) -> String {
    let normalized_id = profile_id.trim();
    let normalized_label = label.trim();

    if let Some(open_idx) = normalized_label.rfind('(')
        && let Some(close_idx) = normalized_label.rfind(')')
        && close_idx > open_idx + 1
    {
        let value = normalized_label[(open_idx + 1)..close_idx].trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }

    if let Some((_, suffix)) = normalized_id.split_once(':') {
        let value = suffix.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }

    String::new()
}

/// 解析 usage 窗口（provider + window 维度）。
fn parse_usage_windows(
    status_json: &Value,
    auth_user_by_provider: &HashMap<String, String>,
) -> Vec<Value> {
    let providers = read_array_path(status_json, &["usage", "providers"]);
    let mut windows = Vec::new();

    for provider in providers {
        let provider_id = read_string(&provider, "provider");
        let provider_name = read_string_or(&provider, "displayName", "provider");
        let auth_user = auth_user_by_provider
            .get(&normalize_lookup_key(&provider_id))
            .cloned()
            .unwrap_or_default();
        let list = provider
            .get("windows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for window in list {
            windows.push(json!({
                "provider": provider_id,
                "displayName": provider_name,
                "authUser": auth_user,
                "label": read_string(&window, "label"),
                "usedPercent": read_i64(&window, "usedPercent"),
                "usedPercentMeaning": "used",
                "used": read_f64_or_null(&window, "used"),
                "limit": read_f64_or_null(&window, "limit"),
                "remaining": read_f64_or_null(&window, "remaining"),
                "currency": read_string(&window, "currency"),
                "cost": read_f64_or_null(&window, "cost"),
                "costUsd": read_f64_or_null(&window, "costUsd"),
                "resetAt": read_i64_or_null(&window, "resetAt"),
            }));
        }
    }

    windows.sort_by_key(|row| Reverse(read_i64(row, "usedPercent")));
    windows
}

/// 解析渠道账户身份（`Channel@account`）视图。
fn parse_channel_identities(
    channels_status_json: Option<&Value>,
    health_json: Option<&Value>,
    status_json: &Value,
) -> Vec<Value> {
    let username_lookup = build_channel_username_lookup(health_json);

    if let Some(raw) = channels_status_json {
        let labels = raw
            .get("channelLabels")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let default_accounts = raw
            .get("channelDefaultAccountId")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let accounts_by_channel = raw
            .get("channelAccounts")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let mut channel_order = raw
            .get("channelOrder")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| item.as_str().map(ToString::to_string))
            .collect::<Vec<String>>();

        if channel_order.is_empty() {
            channel_order = accounts_by_channel
                .keys()
                .map(ToString::to_string)
                .collect();
        }

        let mut rows = Vec::new();
        for channel in channel_order {
            let display_label = labels
                .get(&channel)
                .and_then(Value::as_str)
                .unwrap_or(channel.as_str());
            let default_account = default_accounts
                .get(&channel)
                .and_then(Value::as_str)
                .unwrap_or("default");
            let accounts = accounts_by_channel
                .get(&channel)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            if accounts.is_empty() {
                let username = lookup_channel_username(&username_lookup, &channel, default_account);
                let account_display = if username.is_empty() {
                    default_account.to_string()
                } else {
                    username.clone()
                };
                rows.push(json!({
                    "channel": channel.clone(),
                    "accountId": default_account,
                    "username": if username.is_empty() { Value::Null } else { Value::String(username) },
                    "accountDisplay": account_display,
                    "displayLabel": display_label,
                    "running": false,
                    "configured": false,
                    "mode": "",
                    "lastInboundAt": Value::Null,
                    "lastOutboundAt": Value::Null,
                }));
                continue;
            }

            for account in accounts {
                let account_id = read_string_or(&account, "accountId", "id");
                let normalized_account_id = if account_id.is_empty() {
                    default_account.to_string()
                } else {
                    account_id
                };
                let username =
                    lookup_channel_username(&username_lookup, &channel, &normalized_account_id);
                let account_display = if username.is_empty() {
                    normalized_account_id.clone()
                } else {
                    username.clone()
                };
                rows.push(json!({
                    "channel": channel.clone(),
                    "accountId": normalized_account_id,
                    "username": if username.is_empty() { Value::Null } else { Value::String(username) },
                    "accountDisplay": account_display,
                    "displayLabel": display_label,
                    "running": read_bool(&account, "running"),
                    "configured": read_bool(&account, "configured"),
                    "mode": read_string(&account, "mode"),
                    "lastInboundAt": read_i64_or_null(&account, "lastInboundAt"),
                    "lastOutboundAt": read_i64_or_null(&account, "lastOutboundAt"),
                }));
            }
        }

        rows.sort_by(|a, b| {
            read_string(a, "channel")
                .cmp(&read_string(b, "channel"))
                .then_with(|| read_string(a, "accountId").cmp(&read_string(b, "accountId")))
        });
        if !rows.is_empty() {
            return rows;
        }
    }

    parse_channel_identities_from_summary(status_json)
}

/// 构建渠道账号用户名索引（channel + accountId -> username）。
fn build_channel_username_lookup(health_json: Option<&Value>) -> HashMap<String, String> {
    let mut lookup = HashMap::new();
    let Some(raw) = health_json else {
        return lookup;
    };
    let Some(channels) = raw.get("channels").and_then(Value::as_object) else {
        return lookup;
    };

    for (channel, channel_value) in channels {
        let default_username = read_string_path(channel_value, &["probe", "bot", "username"]);
        if !default_username.is_empty() {
            lookup.insert(
                format!(
                    "{}::{}",
                    channel.trim().to_ascii_lowercase(),
                    "default".to_ascii_lowercase()
                ),
                default_username,
            );
        }
        if let Some(accounts) = channel_value.get("accounts").and_then(Value::as_object) {
            for (account_id, account_value) in accounts {
                let username = read_string_path(account_value, &["probe", "bot", "username"]);
                if username.is_empty() {
                    continue;
                }
                lookup.insert(
                    format!(
                        "{}::{}",
                        channel.trim().to_ascii_lowercase(),
                        account_id.trim().to_ascii_lowercase()
                    ),
                    username,
                );
            }
        }
    }
    lookup
}

/// 按 channel + accountId 读取用户名，缺省回退 default 账号。
fn lookup_channel_username(
    lookup: &HashMap<String, String>,
    channel: &str,
    account_id: &str,
) -> String {
    let key = format!(
        "{}::{}",
        channel.trim().to_ascii_lowercase(),
        account_id.trim().to_ascii_lowercase()
    );
    if let Some(username) = lookup.get(&key) {
        return username.clone();
    }
    let fallback_key = format!("{}::{}", channel.trim().to_ascii_lowercase(), "default");
    lookup.get(&fallback_key).cloned().unwrap_or_default()
}

/// 从 status.channelSummary 兜底解析渠道身份。
fn parse_channel_identities_from_summary(status_json: &Value) -> Vec<Value> {
    let lines = status_json
        .get("channelSummary")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| row.as_str().map(str::trim).map(ToString::to_string))
        .filter(|line| !line.is_empty())
        .collect::<Vec<String>>();

    if lines.is_empty() {
        return Vec::new();
    }

    let mut current_channel = String::new();
    let mut rows = Vec::new();
    for line in lines {
        if line.starts_with('-') {
            let account = line
                .trim_start_matches('-')
                .split_whitespace()
                .next()
                .unwrap_or("default")
                .to_string();
            if !current_channel.is_empty() {
                let channel_name = current_channel.clone();
                let account_display = account.clone();
                rows.push(json!({
                    "channel": channel_name.clone(),
                    "accountId": account,
                    "username": Value::Null,
                    "accountDisplay": account_display,
                    "displayLabel": channel_name,
                    "running": true,
                    "configured": true,
                    "mode": "",
                    "lastInboundAt": Value::Null,
                    "lastOutboundAt": Value::Null,
                }));
            }
            continue;
        }

        let channel = line
            .split(':')
            .next()
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if channel.is_empty() {
            continue;
        }
        current_channel = channel.clone();
        let display_label = current_channel.clone();
        rows.push(json!({
            "channel": channel,
            "accountId": "default",
            "username": Value::Null,
            "accountDisplay": "default",
            "displayLabel": display_label,
            "running": true,
            "configured": true,
            "mode": "",
            "lastInboundAt": Value::Null,
            "lastOutboundAt": Value::Null,
        }));
    }
    rows
}

/// 解析 channels status --json 的结构化渠道概览（兼容旧 UI）。
fn parse_channel_overview(channels_status_json: Option<&Value>) -> Vec<Value> {
    let Some(raw) = channels_status_json else {
        return Vec::new();
    };
    let Some(channels) = raw.get("channels").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut rows = channels
        .iter()
        .map(|(channel, value)| {
            json!({
                "channel": channel,
                "configured": read_bool(value, "configured"),
                "running": read_bool(value, "running"),
                "mode": read_string(value, "mode"),
                "lastError": read_string(value, "lastError"),
                "lastStartAt": read_i64_or_null(value, "lastStartAt"),
            })
        })
        .collect::<Vec<Value>>();
    rows.sort_by_key(|row| read_string(row, "channel"));
    rows
}

/// 以 sessionId（回退 key）去重会话，避免 run 镜像记录重复计费。
fn dedupe_sessions_by_identity(sessions: &[Value]) -> Vec<Value> {
    let mut bucket: HashMap<String, Value> = HashMap::new();
    for row in sessions {
        let session_id = read_string(row, "sessionId");
        let key = if session_id.is_empty() {
            read_string(row, "key")
        } else {
            session_id
        };
        if key.is_empty() {
            continue;
        }

        if let Some(existing) = bucket.get(&key) {
            let current_key = read_string(row, "key");
            let existing_key = read_string(existing, "key");
            let current_is_run = current_key.contains(":run:");
            let existing_is_run = existing_key.contains(":run:");
            let current_updated_at = read_i64(row, "updatedAt");
            let existing_updated_at = read_i64(existing, "updatedAt");
            let should_replace = (!current_is_run && existing_is_run)
                || (current_is_run == existing_is_run && current_updated_at > existing_updated_at);
            if should_replace {
                bucket.insert(key, row.clone());
            }
            continue;
        }
        bucket.insert(key, row.clone());
    }

    let mut rows = bucket.into_values().collect::<Vec<Value>>();
    rows.sort_by_key(|row| Reverse(read_i64(row, "updatedAt")));
    rows
}

/// 过滤指定更新时间窗口内的 sessions（毫秒时间戳）。
fn filter_sessions_by_updated_window(
    sessions: &[Value],
    window_from_ms: i64,
    window_to_ms: i64,
) -> Vec<Value> {
    sessions
        .iter()
        .filter(|row| {
            let updated_at = read_i64(row, "updatedAt");
            updated_at >= window_from_ms && updated_at <= window_to_ms
        })
        .cloned()
        .collect::<Vec<Value>>()
}

/// 聚合模型用量（本地会话聚合）。
fn aggregate_model_totals(
    sessions: &[Value],
    model_lookup: &HashMap<String, ModelPricing>,
) -> Vec<Value> {
    #[derive(Default)]
    struct ModelTotal {
        provider: String,
        model: String,
        messages: i64,
        token_input: i64,
        token_output: i64,
        token_total: i64,
        cache_read: i64,
        cache_write: i64,
        latest_updated_at: i64,
    }

    let mut bucket: HashMap<String, ModelTotal> = HashMap::new();
    for row in sessions {
        let model = read_string(row, "model");
        if model.is_empty() {
            continue;
        }
        let provider = infer_session_provider(row, &model, model_lookup);
        let key = usage_model_key(&provider, &model);
        let entry = bucket.entry(key).or_insert_with(|| ModelTotal {
            provider: provider.clone(),
            model: model.clone(),
            ..Default::default()
        });
        entry.messages += 1;
        entry.token_input += read_i64(row, "inputTokens");
        entry.token_output += read_i64(row, "outputTokens");
        entry.token_total += read_i64(row, "totalTokens");
        entry.cache_read += read_i64(row, "cacheRead");
        entry.cache_write += read_i64(row, "cacheWrite");
        entry.latest_updated_at = entry.latest_updated_at.max(read_i64(row, "updatedAt"));
    }

    let mut rows = bucket
        .into_values()
        .map(|row| {
            json!({
                "provider": row.provider,
                "model": row.model,
                "messages": row.messages,
                "tokenInput": row.token_input,
                "tokenOutput": row.token_output,
                "tokenTotal": row.token_total,
                "cacheRead": row.cache_read,
                "cacheWrite": row.cache_write,
                "latestUpdatedAt": row.latest_updated_at,
            })
        })
        .collect::<Vec<Value>>();

    rows.sort_by_key(|row| Reverse(read_i64(row, "tokenTotal")));
    rows
}

/// 读取配置中的全量模型列表（provider + model）。
fn build_configured_model_rows(models: &[ModelPricing]) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut seen: HashMap<String, bool> = HashMap::new();
    for model in models {
        let provider = model.provider.trim().to_ascii_lowercase();
        let display_model = if model.model_name.trim().is_empty() {
            model.model_id.trim().to_string()
        } else {
            model.model_name.trim().to_string()
        };
        if provider.is_empty() || display_model.is_empty() {
            continue;
        }
        let key = usage_model_key(&provider, &display_model);
        if seen.contains_key(&key) {
            continue;
        }
        seen.insert(key, true);
        rows.push(json!({
            "provider": provider,
            "model": display_model,
        }));
    }
    rows.sort_by(|a, b| {
        let ap = read_string(a, "provider");
        let bp = read_string(b, "provider");
        ap.cmp(&bp)
            .then_with(|| read_string(a, "model").cmp(&read_string(b, "model")))
    });
    rows
}

/// 估算模型成本（按 openclaw.json 的 cost 费率）。
fn estimate_model_cost(
    model_totals: &[Value],
    model_lookup: &HashMap<String, ModelPricing>,
) -> Vec<Value> {
    let mut rows = Vec::new();
    for total in model_totals {
        let model_name = read_string(total, "model");
        if model_name.is_empty() {
            continue;
        }
        let lookup_key = normalize_lookup_key(&model_name);
        let Some(pricing) = model_lookup.get(&lookup_key) else {
            continue;
        };

        let input_cost = calc_cost_m(read_i64(total, "tokenInput"), pricing.input_rate);
        let output_cost = calc_cost_m(read_i64(total, "tokenOutput"), pricing.output_rate);
        let cache_read_cost = calc_cost_m(read_i64(total, "cacheRead"), pricing.cache_read_rate);
        let cache_write_cost = calc_cost_m(read_i64(total, "cacheWrite"), pricing.cache_write_rate);
        let total_cost = input_cost + output_cost + cache_read_cost + cache_write_cost;

        rows.push(json!({
            "provider": if read_string(total, "provider").is_empty() {
                pricing.provider.clone()
            } else {
                read_string(total, "provider")
            },
            "model": model_name,
            "inputCost": round4(input_cost),
            "outputCost": round4(output_cost),
            "cacheReadCost": round4(cache_read_cost),
            "cacheWriteCost": round4(cache_write_cost),
            "totalCost": round4(total_cost),
            "currency": "config-rate",
            "rateSource": "openclaw.json",
        }));
    }

    rows.sort_by(|a, b| {
        read_f64(b, "totalCost")
            .partial_cmp(&read_f64(a, "totalCost"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

/// 合并配置模型与 1h 聚合数据，确保“全模型展示”。
fn merge_usage_model_rows(
    configured_models: &[Value],
    model_totals: &[Value],
    estimated_cost: &[Value],
    window_from_ms: i64,
    window_to_ms: i64,
) -> Vec<Value> {
    let mut totals_by_key: HashMap<String, Value> = HashMap::new();
    for row in model_totals {
        let provider = read_string(row, "provider");
        let model = read_string(row, "model");
        let key = usage_model_key(&provider, &model);
        if !key.is_empty() {
            totals_by_key.insert(key, row.clone());
        }
    }

    let mut costs_by_key: HashMap<String, Value> = HashMap::new();
    for row in estimated_cost {
        let provider = read_string(row, "provider");
        let model = read_string(row, "model");
        let key = usage_model_key(&provider, &model);
        if !key.is_empty() {
            costs_by_key.insert(key, row.clone());
        }
    }

    let mut rows = Vec::new();
    let mut merged_keys: HashMap<String, bool> = HashMap::new();
    for configured in configured_models {
        let provider = read_string(configured, "provider");
        let model = read_string(configured, "model");
        let key = usage_model_key(&provider, &model);
        if key.is_empty() {
            continue;
        }
        merged_keys.insert(key.clone(), true);
        rows.push(build_usage_model_row(
            &provider,
            &model,
            totals_by_key.get(&key),
            costs_by_key.get(&key),
            true,
            window_from_ms,
            window_to_ms,
        ));
    }

    for (key, total) in totals_by_key {
        if merged_keys.contains_key(&key) {
            continue;
        }
        let provider = read_string(&total, "provider");
        let model = read_string(&total, "model");
        rows.push(build_usage_model_row(
            &provider,
            &model,
            Some(&total),
            costs_by_key.get(&key),
            false,
            window_from_ms,
            window_to_ms,
        ));
    }

    rows.sort_by(|a, b| {
        let a_active = usage_model_activity_tokens(a);
        let b_active = usage_model_activity_tokens(b);
        b_active
            .cmp(&a_active)
            .then_with(|| read_i64(b, "tokenTotal").cmp(&read_i64(a, "tokenTotal")))
            .then_with(|| read_string(a, "provider").cmp(&read_string(b, "provider")))
            .then_with(|| read_string(a, "model").cmp(&read_string(b, "model")))
    });
    rows
}

/// 构建单模型 usage 行。
fn build_usage_model_row(
    provider: &str,
    model: &str,
    total: Option<&Value>,
    cost: Option<&Value>,
    configured: bool,
    window_from_ms: i64,
    window_to_ms: i64,
) -> Value {
    json!({
        "provider": provider,
        "model": model,
        "configured": configured,
        "messages": total.map(|row| read_i64(row, "messages")).unwrap_or(0),
        "tokenInput": total.map(|row| read_i64(row, "tokenInput")).unwrap_or(0),
        "tokenOutput": total.map(|row| read_i64(row, "tokenOutput")).unwrap_or(0),
        "tokenTotal": total.map(|row| read_i64(row, "tokenTotal")).unwrap_or(0),
        "cacheRead": total.map(|row| read_i64(row, "cacheRead")).unwrap_or(0),
        "cacheWrite": total.map(|row| read_i64(row, "cacheWrite")).unwrap_or(0),
        "latestUpdatedAt": total.map(|row| read_i64(row, "latestUpdatedAt")).unwrap_or(0),
        "totalCost": cost.map(|row| read_f64(row, "totalCost")).unwrap_or(0.0),
        "currency": cost
            .map(|row| read_string(row, "currency"))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "config-rate".to_string()),
        "rateSource": cost
            .map(|row| read_string(row, "rateSource"))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "openclaw.json".to_string()),
        "windowPreset": "1h",
        "windowFromMs": window_from_ms,
        "windowToMs": window_to_ms,
    })
}

/// 将模型按“1h 内是否有 token 活动”拆分。
fn split_usage_model_rows_by_activity(model_rows: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let mut with_cost = Vec::new();
    let mut without_cost = Vec::new();
    for row in model_rows {
        if usage_model_activity_tokens(row) > 0 {
            with_cost.push(row.clone());
        } else {
            without_cost.push(row.clone());
        }
    }
    (with_cost, without_cost)
}

/// 构建 API 来源的 provider 聚合卡片（auth provider 不进入该分组）。
fn build_usage_api_provider_cards(
    models_with_cost: &[Value],
    provider_windows: &[Value],
    window_from_ms: i64,
    window_to_ms: i64,
) -> Vec<Value> {
    let mut auth_provider_map: HashMap<String, bool> = HashMap::new();
    for window in provider_windows {
        let provider = normalize_lookup_key(&read_string(window, "provider"));
        if !provider.is_empty() {
            auth_provider_map.insert(provider, true);
        }
    }

    let mut grouped: HashMap<String, Vec<Value>> = HashMap::new();
    for model in models_with_cost {
        let provider = normalize_lookup_key(&read_string(model, "provider"));
        if provider.is_empty() {
            continue;
        }
        if auth_provider_map.contains_key(&provider) {
            continue;
        }
        grouped.entry(provider).or_default().push(model.clone());
    }

    let mut cards = Vec::new();
    for (provider, mut models) in grouped {
        models.sort_by_key(|row| Reverse(read_i64(row, "tokenTotal")));
        let provider_token_total = models
            .iter()
            .map(|row| read_i64(row, "tokenTotal"))
            .sum::<i64>();
        let provider_cost_total = models
            .iter()
            .map(|row| read_f64(row, "totalCost"))
            .sum::<f64>();
        let stat_at = models
            .iter()
            .map(|row| read_i64(row, "latestUpdatedAt"))
            .max()
            .unwrap_or(0);
        cards.push(json!({
            "provider": provider,
            "models": models,
            "providerTokenTotal": provider_token_total,
            "providerCostTotal": round4(provider_cost_total),
            "providerBalance": Value::Null,
            "balanceStatus": "unavailable",
            "balanceNote": "未获取到",
            "windowPreset": "1h",
            "windowFromMs": window_from_ms,
            "windowToMs": window_to_ms,
            "statAt": stat_at,
        }));
    }
    cards.sort_by_key(|row| read_string(row, "provider"));
    cards
}

/// 构建 usage 覆盖率说明。
fn build_usage_coverage(
    provider_windows: &[Value],
    models_with_cost: &[Value],
    models_without_cost: &[Value],
) -> Value {
    let mut window_providers = provider_windows
        .iter()
        .map(|row| read_string_or(row, "displayName", "provider"))
        .filter(|v| !v.is_empty())
        .collect::<Vec<String>>();
    window_providers.sort();
    window_providers.dedup();

    let mut estimated_models = models_with_cost
        .iter()
        .map(|row| read_string(row, "model"))
        .filter(|v| !v.is_empty())
        .collect::<Vec<String>>();
    estimated_models.sort();
    estimated_models.dedup();

    json!({
        "hasWindowData": !provider_windows.is_empty(),
        "hasEstimateData": !models_with_cost.is_empty(),
        "windowProviders": window_providers,
        "estimatedModels": estimated_models,
        "configuredModelCount": models_with_cost.len() + models_without_cost.len(),
        "activeModelCount1h": models_with_cost.len(),
        "note": "Usage 仅依赖 OpenClaw 官方 CLI 与 openclaw.json，不读取 custom/state 自定义统计文件。",
    })
}

/// 构建 usage 摘要头条（供摘要卡展示）。
fn build_usage_headline(
    provider_windows: &[Value],
    model_totals: &[Value],
    estimated_cost: &[Value],
) -> Value {
    if let Some(top_window) = provider_windows
        .iter()
        .max_by_key(|row| read_i64(row, "usedPercent"))
    {
        let provider = read_string_or(top_window, "displayName", "provider");
        let label = read_string(top_window, "label");
        let used_percent = read_i64(top_window, "usedPercent");
        return json!({
            "label": format!("{provider} · {label}"),
            "percent": used_percent,
            "source": "providerWindow",
            "provider": read_string(top_window, "provider"),
            "model": Value::Null,
        });
    }

    if let Some(top_cost) = estimated_cost.first() {
        return json!({
            "label": format!("{} · 估算成本 {}", read_string(top_cost, "model"), read_f64(top_cost, "totalCost")),
            "percent": Value::Null,
            "source": "estimatedCost",
            "provider": read_string(top_cost, "provider"),
            "model": read_string(top_cost, "model"),
        });
    }

    if let Some(top_total) = model_totals.first() {
        return json!({
            "label": format!("{} · {}", read_string(top_total, "model"), read_i64(top_total, "tokenTotal")),
            "percent": Value::Null,
            "source": "modelTotal",
            "provider": read_string(top_total, "provider"),
            "model": read_string(top_total, "model"),
        });
    }

    json!({
        "label": "--",
        "percent": Value::Null,
        "source": "none",
        "provider": Value::Null,
        "model": Value::Null,
    })
}

/// 构造 provider+model 的稳定键。
fn usage_model_key(provider: &str, model: &str) -> String {
    let p = normalize_lookup_key(provider);
    let m = normalize_lookup_key(model);
    if p.is_empty() || m.is_empty() {
        return String::new();
    }
    format!("{p}::{m}")
}

/// 计算模型行的 token 活动总量（用于“是否已产生费用”判定）。
fn usage_model_activity_tokens(row: &Value) -> i64 {
    let token_total = read_i64(row, "tokenTotal");
    if token_total > 0 {
        return token_total;
    }
    read_i64(row, "tokenInput")
        + read_i64(row, "tokenOutput")
        + read_i64(row, "cacheRead")
        + read_i64(row, "cacheWrite")
}

/// 根据 session/model 映射推断 provider。
fn infer_session_provider(
    session: &Value,
    model_name: &str,
    model_lookup: &HashMap<String, ModelPricing>,
) -> String {
    let by_session = read_string(session, "modelProvider");
    if !by_session.is_empty() {
        return by_session;
    }

    let key = normalize_lookup_key(model_name);
    if let Some(model_cfg) = model_lookup.get(&key) {
        return model_cfg.provider.clone();
    }

    if let Some(prefix) = model_name.split('/').next() {
        let normalized = prefix.trim().to_ascii_lowercase();
        if !normalized.is_empty() {
            return normalized;
        }
    }

    "unknown".to_string()
}

/// 计算 token 成本（每百万 token 费率）。
fn calc_cost_m(tokens: i64, rate_per_million: f64) -> f64 {
    if tokens <= 0 || rate_per_million <= 0.0 {
        return 0.0;
    }
    (tokens as f64 / 1_000_000f64) * rate_per_million
}

/// 构建 sessions 诊断 + 时间线 + 台账三合一结构。
fn build_sessions_payload(sessions: &[Value]) -> Value {
    let mut timeline = sessions.to_vec();
    timeline.sort_by_key(|row| Reverse(read_i64(row, "updatedAt")));

    let now_sec = now_epoch_sec();
    let mut aborted_count = 0i64;
    let mut system_count = 0i64;
    let mut stale_count = 0i64;
    let mut inactive_over_6h_count = 0i64;
    let mut active_24h_count = 0i64;

    for row in &timeline {
        if read_bool(row, "abortedLastRun") {
            aborted_count += 1;
        }
        if read_bool(row, "systemSent") {
            system_count += 1;
        }
        if !read_bool(row, "totalTokensFresh") {
            stale_count += 1;
        }
        let updated_sec = read_i64(row, "updatedAt") / 1000;
        if updated_sec > 0 {
            if now_sec.saturating_sub(updated_sec) >= INACTIVE_SESSION_SEC {
                inactive_over_6h_count += 1;
            }
            if now_sec.saturating_sub(updated_sec) <= ACTIVE_SESSION_24H_SEC {
                active_24h_count += 1;
            }
        }
    }

    let total = timeline.len() as i64;
    let diagnostics = json!({
        "total": total,
        "abortedCount": aborted_count,
        "abortedPercent": to_percent(aborted_count, total),
        "abortedRate24h": to_percent(aborted_count, total),
        "systemCount": system_count,
        "systemPercent": to_percent(system_count, total),
        "systemRatio": to_percent(system_count, total),
        "staleCount": stale_count,
        "inactiveOver6hCount": inactive_over_6h_count,
        "active24hCount": active_24h_count,
    });

    let timeline_rows = timeline
        .iter()
        .take(24)
        .map(session_event_row)
        .collect::<Vec<Value>>();
    let ledger_rows = timeline
        .iter()
        .take(60)
        .map(session_ledger_row)
        .collect::<Vec<Value>>();

    json!({
        "diagnostics": diagnostics,
        "timeline": timeline_rows,
        "ledger": ledger_rows,
    })
}

/// 时间线单行（诊断视角）。
fn session_event_row(raw: &Value) -> Value {
    let updated_at = read_i64(raw, "updatedAt");
    json!({
        "sessionId": read_string(raw, "sessionId"),
        "key": read_string(raw, "key"),
        "agentId": read_string(raw, "agentId"),
        "model": read_string(raw, "model"),
        "kind": read_string(raw, "kind"),
        "flags": read_string_array(raw, "flags"),
        "updatedAt": updated_at,
        "updatedAgoSec": age_sec_from_ms(updated_at),
        "ageMs": read_i64(raw, "ageMs"),
        "systemSent": read_bool(raw, "systemSent"),
        "abortedLastRun": read_bool(raw, "abortedLastRun"),
        "percentUsed": read_i64(raw, "percentUsed"),
        "remainingTokens": read_i64(raw, "remainingTokens"),
        "totalTokens": read_i64(raw, "totalTokens"),
        "totalTokensFresh": read_bool(raw, "totalTokensFresh"),
    })
}

/// 台账单行（资产视角）。
fn session_ledger_row(raw: &Value) -> Value {
    let updated_at = read_i64(raw, "updatedAt");
    let health_tag = if read_bool(raw, "abortedLastRun") {
        "critical"
    } else if !read_bool(raw, "totalTokensFresh") {
        "warning"
    } else {
        "ok"
    };
    json!({
        "sessionId": read_string(raw, "sessionId"),
        "key": read_string(raw, "key"),
        "agentId": read_string(raw, "agentId"),
        "model": read_string(raw, "model"),
        "updatedAt": updated_at,
        "updatedAgoSec": age_sec_from_ms(updated_at),
        "remainingTokens": read_i64(raw, "remainingTokens"),
        "totalTokens": read_i64(raw, "totalTokens"),
        "contextTokens": read_i64(raw, "contextTokens"),
        "percentUsed": read_i64(raw, "percentUsed"),
        "healthTag": health_tag,
    })
}

/// 给 agents 注入上下文“已用/上限/来源”信息。
fn attach_agent_context_metrics(
    mut agents: Vec<Value>,
    sessions: &[Value],
    default_context_tokens: i64,
    model_lookup: &HashMap<String, ModelPricing>,
) -> Vec<Value> {
    let latest_by_agent = latest_session_by_agent(sessions);

    for row in &mut agents {
        let agent_id = read_string(row, "agentId");
        let latest = latest_by_agent.get(&agent_id);
        let model = if let Some(latest_row) = latest {
            read_string(latest_row, "model")
        } else {
            read_string(row, "model")
        };
        let model_ctx = lookup_model_context_window(&model, model_lookup);
        let session_ctx = latest
            .map(|latest_row| read_i64(latest_row, "contextTokens"))
            .unwrap_or(0);
        let remaining = latest
            .map(|latest_row| read_i64(latest_row, "remainingTokens"))
            .unwrap_or(0);
        let percent_used = latest
            .map(|latest_row| read_i64(latest_row, "percentUsed"))
            .unwrap_or(0);

        let context_max = choose_context_max(session_ctx, default_context_tokens, model_ctx);
        let (context_used, source) = choose_context_used_source(
            context_max,
            remaining,
            percent_used,
            session_ctx,
            default_context_tokens,
            model_ctx,
        );

        if let Some(obj) = row.as_object_mut() {
            obj.insert("contextMaxTokens".to_string(), json!(context_max.max(0)));
            obj.insert("contextUsedTokens".to_string(), json!(context_used.max(0)));
            obj.insert("contextLimitSource".to_string(), Value::String(source));
            obj.insert("remainingTokens".to_string(), json!(remaining.max(0)));
        }
    }

    agents
}

/// 选择上下文上限（优先 session，再 agentDefault，再 modelConfig）。
fn choose_context_max(session_ctx: i64, default_ctx: i64, model_ctx: i64) -> i64 {
    if session_ctx > 0 {
        return session_ctx;
    }
    if default_ctx > 0 && model_ctx > 0 {
        return default_ctx.min(model_ctx);
    }
    if default_ctx > 0 {
        return default_ctx;
    }
    if model_ctx > 0 {
        return model_ctx;
    }
    0
}

/// 选择“已用上下文 + 来源”。
fn choose_context_used_source(
    context_max: i64,
    remaining: i64,
    percent_used: i64,
    session_ctx: i64,
    default_ctx: i64,
    model_ctx: i64,
) -> (i64, String) {
    let source = if session_ctx > 0 {
        "session"
    } else if default_ctx > 0 {
        "agentDefault"
    } else if model_ctx > 0 {
        "modelConfig"
    } else {
        "unknown"
    };

    if context_max > 0 && remaining >= 0 && remaining <= context_max {
        return (context_max - remaining, source.to_string());
    }
    if context_max > 0 && percent_used > 0 {
        let used = ((context_max as f64) * (percent_used as f64 / 100.0)).round() as i64;
        return (used, source.to_string());
    }
    (0, source.to_string())
}

/// 查询模型上下文窗口。
fn lookup_model_context_window(
    model_name: &str,
    model_lookup: &HashMap<String, ModelPricing>,
) -> i64 {
    let key = normalize_lookup_key(model_name);
    model_lookup
        .get(&key)
        .map(|row| row.context_window)
        .unwrap_or(0)
}

/// 构建 overview（摘要主数据）。
fn build_overview(
    status_json: &Value,
    default_agent_id: &str,
    agents: &[Value],
    channel_identities: &[Value],
    sessions_payload: &Value,
    usage_headline: Value,
    dashboard_meta: Value,
) -> Value {
    let default_agent_name = agents
        .iter()
        .find(|row| read_bool(row, "isDefault"))
        .map(|row| read_string(row, "name"))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| default_agent_id.to_string());

    let diagnostics = sessions_payload
        .get("diagnostics")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let active_sessions_24h = diagnostics
        .get("active24hCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let aborted_sessions = diagnostics
        .get("abortedCount")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    json!({
        "defaultAgentId": default_agent_id,
        "defaultAgentName": default_agent_name,
        "totalAgents": agents.len(),
        "activeSessions24h": active_sessions_24h,
        "abortedSessions": aborted_sessions,
        "channelIdentities": channel_identities,
        "channelHiddenCount": 0,
        "usageHeadline": usage_headline,
        "dashboardMeta": dashboard_meta,
        // 兼容旧字段。
        "gatewayServiceStatus": read_string_path(status_json, &["gatewayService", "runtimeShort"]),
        "nodeServiceStatus": read_string_path(status_json, &["nodeService", "runtimeShort"]),
    })
}

/// 解析 dashboard 元信息（只读状态，不提供打开跳转）。
fn parse_dashboard_meta(status_json: &Value, gateway_status_json: Option<&Value>) -> Value {
    let gateway_reachable = read_bool_path(status_json, &["gateway", "reachable"]);
    let gateway_url = read_string_path(status_json, &["gateway", "url"]);
    let rpc_reachable = gateway_status_json
        .map(|row| read_bool_path(row, &["rpc", "ok"]))
        .unwrap_or(false);
    let bind_mode = gateway_status_json
        .map(|row| read_string_path(row, &["gateway", "bindMode"]))
        .unwrap_or_default();
    let bind_host = gateway_status_json
        .map(|row| read_string_path(row, &["gateway", "bindHost"]))
        .unwrap_or_default();
    let port = gateway_status_json
        .map(|row| read_i64_path(row, &["gateway", "port"]))
        .unwrap_or(0);
    let update_channel = read_string(status_json, "updateChannel");
    let update_version = read_string_path(status_json, &["update", "registry", "latestVersion"]);

    json!({
        "available": gateway_reachable || rpc_reachable,
        "gatewayReachable": gateway_reachable,
        "rpcReachable": rpc_reachable,
        "bindMode": bind_mode,
        "bindHost": bind_host,
        "port": if port > 0 { json!(port) } else { Value::Null },
        "gatewayUrl": if gateway_url.is_empty() { Value::Null } else { Value::String(gateway_url) },
        "updateChannel": if update_channel.is_empty() { Value::Null } else { Value::String(update_channel) },
        "updateVersion": if update_version.is_empty() { Value::Null } else { Value::String(update_version) },
    })
}

/// 解析 health --json 摘要。
fn parse_health_summary(health_json: Option<&Value>) -> Value {
    let Some(raw) = health_json else {
        return json!({});
    };
    json!({
        "ok": read_bool(raw, "ok"),
        "durationMs": read_i64(raw, "durationMs"),
        "heartbeatSeconds": read_i64(raw, "heartbeatSeconds"),
        "defaultAgentId": read_string(raw, "defaultAgentId"),
        "channelsCount": read_array_len(raw, "channels"),
        "agentsCount": read_array_len(raw, "agents"),
        "sessionsCount": read_array_len(raw, "sessions"),
        "lastCheckedAt": now_rfc3339_nanos(),
    })
}

/// 解析网关运行态。
fn parse_gateway_runtime(status_json: &Value, gateway_status_json: Option<&Value>) -> Value {
    if let Some(raw) = gateway_status_json {
        let runtime = raw.get("service").and_then(|v| v.get("runtime")).cloned();
        return json!({
            "bindMode": read_string_path(raw, &["gateway", "bindMode"]),
            "bindHost": read_string_path(raw, &["gateway", "bindHost"]),
            "port": read_i64_path(raw, &["gateway", "port"]),
            "probeUrl": read_string_path(raw, &["gateway", "probeUrl"]),
            "rpcOk": read_bool_path(raw, &["rpc", "ok"]),
            "serviceStatus": runtime.as_ref().map(|v| read_string(v, "status")).unwrap_or_default(),
            "serviceState": runtime.as_ref().map(|v| read_string(v, "state")).unwrap_or_default(),
            "pid": runtime.as_ref().map(|v| read_i64(v, "pid")).unwrap_or(0),
        });
    }

    json!({
        "bindMode": "",
        "bindHost": "",
        "port": 0,
        "probeUrl": read_string_path(status_json, &["gateway", "url"]),
        "rpcOk": false,
        "serviceStatus": read_string_path(status_json, &["gatewayService", "runtimeShort"]),
        "serviceState": "",
        "pid": 0,
    })
}

/// 解析 memory status（数组）或 status.memory 的摘要。
fn parse_memory_index(status_json: &Value, memory_status_json: Option<&Value>) -> Vec<Value> {
    if let Some(raw) = memory_status_json
        && let Some(rows) = raw.as_array()
    {
        let mut memory_rows = rows
            .iter()
            .map(|item| {
                let status = item.get("status").cloned().unwrap_or_else(|| json!({}));
                let scan = item.get("scan").cloned().unwrap_or_else(|| json!({}));
                json!({
                    "agentId": read_string(item, "agentId"),
                    "backend": read_string(&status, "backend"),
                    "files": read_i64(&status, "files"),
                    "chunks": read_i64(&status, "chunks"),
                    "dirty": read_bool(&status, "dirty"),
                    "provider": read_string(&status, "provider"),
                    "model": read_string(&status, "model"),
                    "cacheEntries": read_i64_path(&status, &["cache", "entries"]),
                    "vectorAvailable": read_bool_path(&status, &["vector", "available"]),
                    "ftsAvailable": read_bool_path(&status, &["fts", "available"]),
                    "totalFiles": read_i64(&scan, "totalFiles"),
                    "scanIssues": read_i64_array_len(&scan, "issues"),
                })
            })
            .collect::<Vec<Value>>();
        memory_rows.sort_by_key(|row| read_string(row, "agentId"));
        return memory_rows;
    }

    let memory = status_json
        .get("memory")
        .cloned()
        .unwrap_or_else(|| json!({}));
    vec![json!({
        "agentId": read_string(&memory, "agentId"),
        "backend": read_string(&memory, "backend"),
        "files": read_i64(&memory, "files"),
        "chunks": read_i64(&memory, "chunks"),
        "dirty": read_bool(&memory, "dirty"),
        "provider": read_string(&memory, "provider"),
        "model": read_string(&memory, "model"),
        "cacheEntries": read_i64_path(&memory, &["cache", "entries"]),
        "vectorAvailable": read_bool_path(&memory, &["vector", "available"]),
        "ftsAvailable": read_bool_path(&memory, &["fts", "available"]),
        "totalFiles": 0,
        "scanIssues": 0,
    })]
}

/// 解析 security summary，兼容 status/security audit 两种来源。
fn parse_security_summary(status_json: &Value, security_audit_json: Option<&Value>) -> Value {
    if let Some(raw) = security_audit_json {
        let findings = raw
            .get("findings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let summary = raw.get("summary").cloned().unwrap_or_else(|| json!({}));
        return json!({
            "critical": read_i64(&summary, "critical"),
            "warn": read_i64(&summary, "warn"),
            "info": read_i64(&summary, "info"),
            "findingsCount": findings.len(),
        });
    }

    let summary = status_json
        .get("securityAudit")
        .and_then(|value| value.get("summary"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let findings_count = status_json
        .get("securityAudit")
        .and_then(|value| value.get("findings"))
        .and_then(Value::as_array)
        .map(|rows| rows.len())
        .unwrap_or(0);

    json!({
        "critical": read_i64(&summary, "critical"),
        "warn": read_i64(&summary, "warn"),
        "info": read_i64(&summary, "info"),
        "findingsCount": findings_count,
    })
}

/// 解析安全审计明细，输出前端可渲染的精简列表。
fn parse_security_findings(status_json: &Value, security_audit_json: Option<&Value>) -> Vec<Value> {
    let findings = if let Some(raw) = security_audit_json {
        raw.get("findings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        status_json
            .get("securityAudit")
            .and_then(|value| value.get("findings"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };

    findings
        .iter()
        .take(40)
        .map(|item| {
            let detail = read_string(item, "detail");
            json!({
                "severity": read_string(item, "severity"),
                "title": read_string(item, "title"),
                "checkId": read_string(item, "checkId"),
                "detail": truncate_text(&detail, 280),
            })
        })
        .collect()
}

/// 截断超长文案，防止详情 payload 过大。
fn truncate_text(raw: &str, limit: usize) -> String {
    if raw.chars().count() <= limit {
        return raw.to_string();
    }
    let mut out = String::with_capacity(limit + 1);
    for (idx, ch) in raw.chars().enumerate() {
        if idx >= limit {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// 解析网关状态点：online/offline/unknown。
fn parse_gateway_status_dot(
    status_json: &Value,
    gateway_status_json: Option<&Value>,
) -> &'static str {
    if let Some(raw) = gateway_status_json
        && read_bool_path(raw, &["rpc", "ok"])
    {
        return "online";
    }
    match status_json
        .get("gateway")
        .and_then(|value| value.get("reachable"))
        .and_then(Value::as_bool)
    {
        Some(true) => "online",
        Some(false) => "offline",
        None => "unknown",
    }
}

/// 合并 status agents 与 agents list，并附加 session 统计字段。
fn merge_agents(
    status_agents: Vec<Value>,
    listed_agents: Vec<Value>,
    default_agent_id: &str,
    sessions: &[Value],
    sessions_default_context_tokens: i64,
) -> Vec<Value> {
    let mut merged: HashMap<String, Value> = HashMap::new();
    for raw in status_agents {
        let agent_id = read_string(&raw, "agentId");
        if agent_id.is_empty() {
            continue;
        }
        merged.insert(agent_id, raw);
    }

    for raw in listed_agents {
        let agent_id = read_string(&raw, "agentId");
        if agent_id.is_empty() {
            continue;
        }
        let mut row = merged
            .remove(&agent_id)
            .unwrap_or_else(|| json!({"agentId": agent_id}));
        if let Some(obj) = row.as_object_mut() {
            set_if_non_empty(obj, "name", read_string(&raw, "name"));
            set_if_non_empty(obj, "model", read_string(&raw, "model"));
            set_if_non_empty(obj, "workspaceDir", read_string(&raw, "workspaceDir"));
            if read_i64(&raw, "bindings") > 0 {
                obj.insert("bindings".to_string(), json!(read_i64(&raw, "bindings")));
            } else if !obj.contains_key("bindings") {
                obj.insert("bindings".to_string(), json!(0));
            }
            if let Some(routes) = raw.get("routes").cloned() {
                obj.insert("routes".to_string(), routes);
            } else if !obj.contains_key("routes") {
                obj.insert("routes".to_string(), json!([]));
            }
            let is_default = read_bool(&raw, "isDefault") || agent_id == default_agent_id;
            obj.insert("isDefault".to_string(), json!(is_default));
            set_if_non_empty(obj, "identityName", read_string(&raw, "identityName"));
            set_if_non_empty(obj, "identityEmoji", read_string(&raw, "identityEmoji"));
        }
        merged.insert(agent_id, row);
    }

    let latest_by_agent = latest_session_by_agent(sessions);
    for (agent_id, row) in &mut merged {
        if let Some(obj) = row.as_object_mut() {
            if !obj.contains_key("isDefault") {
                obj.insert("isDefault".to_string(), json!(agent_id == default_agent_id));
            }
            if !obj.contains_key("heartbeatEnabled") {
                obj.insert("heartbeatEnabled".to_string(), json!(false));
            }
            if !obj.contains_key("heartbeatEveryMs") {
                obj.insert("heartbeatEveryMs".to_string(), Value::Null);
            }
            if !obj.contains_key("bindings") {
                obj.insert("bindings".to_string(), json!(0));
            }
            if !obj.contains_key("routes") {
                obj.insert("routes".to_string(), json!([]));
            }

            if let Some(latest) = latest_by_agent.get(agent_id) {
                obj.insert(
                    "latestSessionId".to_string(),
                    Value::String(read_string(latest, "sessionId")),
                );
                obj.insert(
                    "latestSessionModel".to_string(),
                    Value::String(read_string(latest, "model")),
                );
                obj.insert(
                    "latestTotalTokens".to_string(),
                    json!(read_i64(latest, "totalTokens")),
                );
                obj.insert(
                    "latestPercentUsed".to_string(),
                    json!(read_i64(latest, "percentUsed")),
                );
                obj.insert(
                    "latestUpdatedAt".to_string(),
                    json!(read_i64(latest, "updatedAt")),
                );
                let latest_context = read_i64(latest, "contextTokens");
                if latest_context > 0 {
                    obj.insert("contextTokens".to_string(), json!(latest_context));
                } else if sessions_default_context_tokens > 0 {
                    obj.insert(
                        "contextTokens".to_string(),
                        json!(sessions_default_context_tokens),
                    );
                }
            } else if sessions_default_context_tokens > 0 {
                obj.insert(
                    "contextTokens".to_string(),
                    json!(sessions_default_context_tokens),
                );
            }
        }
    }

    let mut rows = merged.into_values().collect::<Vec<Value>>();
    rows.sort_by(|a, b| {
        let a_default = read_bool(a, "isDefault");
        let b_default = read_bool(b, "isDefault");
        b_default
            .cmp(&a_default)
            .then_with(|| read_i64(b, "latestUpdatedAt").cmp(&read_i64(a, "latestUpdatedAt")))
            .then_with(|| read_i64(b, "lastUpdatedAt").cmp(&read_i64(a, "lastUpdatedAt")))
            .then_with(|| read_string(a, "name").cmp(&read_string(b, "name")))
    });
    rows
}

/// 生成每个 agent 的最近会话索引。
fn latest_session_by_agent(sessions: &[Value]) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    for raw in sessions {
        let agent_id = read_string(raw, "agentId");
        if agent_id.is_empty() {
            continue;
        }
        let updated_at = read_i64(raw, "updatedAt");
        let replace = map
            .get(&agent_id)
            .map(|old| read_i64(old, "updatedAt") < updated_at)
            .unwrap_or(true);
        if replace {
            map.insert(agent_id, raw.clone());
        }
    }
    map
}

/// 按 workspace 优先匹配 agent；匹配失败时回退全量列表。
fn select_agents_by_workspace(agents: &[Value], workspace_dir: &str) -> Vec<Value> {
    let workspace = workspace_dir.trim();
    if workspace.is_empty() {
        return agents.to_vec();
    }

    let filtered = agents
        .iter()
        .filter(|item| read_string(item, "workspaceDir") == workspace)
        .cloned()
        .collect::<Vec<Value>>();

    if filtered.is_empty() {
        agents.to_vec()
    } else {
        filtered
    }
}

/// 按 agentId 过滤 session；若过滤后为空则回退原始列表。
fn select_sessions_by_agents(sessions: &[Value], scoped_agents: &[Value]) -> Vec<Value> {
    let agent_ids = scoped_agents
        .iter()
        .map(|agent| read_string(agent, "agentId"))
        .filter(|agent_id| !agent_id.is_empty())
        .collect::<Vec<String>>();

    if agent_ids.is_empty() {
        return sessions.to_vec();
    }

    let filtered = sessions
        .iter()
        .filter(|session| {
            let session_agent = read_string(session, "agentId");
            if !session_agent.is_empty() {
                return agent_ids.contains(&session_agent);
            }
            let key = read_string(session, "key");
            agent_ids
                .iter()
                .any(|agent_id| key.contains(&format!("agent:{agent_id}:")))
        })
        .cloned()
        .collect::<Vec<Value>>();

    if filtered.is_empty() {
        sessions.to_vec()
    } else {
        filtered
    }
}

/// 当前时间戳（秒）。
fn now_epoch_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 将毫秒级时间戳转换为“距今秒数”。
fn age_sec_from_ms(updated_at_ms: i64) -> i64 {
    if updated_at_ms <= 0 {
        return 0;
    }
    let updated_sec = updated_at_ms / 1000;
    now_epoch_sec().saturating_sub(updated_sec)
}

/// 计算占比（0~100）。
fn to_percent(part: i64, total: i64) -> i64 {
    if part <= 0 || total <= 0 {
        return 0;
    }
    ((part as f64 / total as f64) * 100.0).round() as i64
}

/// 四位小数取整。
fn round4(value: f64) -> f64 {
    (value * 10_000f64).round() / 10_000f64
}

/// 读取字符串字段。
fn read_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

/// 读取字符串字段，优先 key_a，缺失时回退 key_b。
fn read_string_or(value: &Value, key_a: &str, key_b: &str) -> String {
    let first = read_string(value, key_a);
    if !first.is_empty() {
        return first;
    }
    read_string(value, key_b)
}

/// 读取布尔字段。
fn read_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// 读取 i64 字段。
fn read_i64(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// 读取 f64 字段。
fn read_f64(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// 读取 f64；若字段不存在则返回 null。
fn read_f64_or_null(value: &Value, key: &str) -> Value {
    if let Some(v) = value.get(key).and_then(Value::as_f64) {
        return json!(v);
    }
    Value::Null
}

/// 读取 i64，若字段不存在则返回 null。
fn read_i64_or_null(value: &Value, key: &str) -> Value {
    let v = read_i64(value, key);
    if v > 0 { json!(v) } else { Value::Null }
}

/// 读取字符串数组字段。
fn read_string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| row.as_str().map(ToString::to_string))
        .collect()
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

/// 读取路径布尔值。
fn read_bool_path(value: &Value, path: &[&str]) -> bool {
    let mut cursor = value;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return false;
        };
        cursor = next;
    }
    cursor.as_bool().unwrap_or(false)
}

/// 读取路径 i64。
fn read_i64_path(value: &Value, path: &[&str]) -> i64 {
    let mut cursor = value;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return 0;
        };
        cursor = next;
    }
    cursor.as_i64().unwrap_or(0)
}

/// 读取数组长度字段。
fn read_array_len(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|rows| rows.len() as i64)
        .unwrap_or(0)
}

/// 读取数组长度字段（默认 0）。
fn read_i64_array_len(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|rows| rows.len() as i64)
        .unwrap_or(0)
}

/// 读取路径数组；不存在时返回空数组。
fn read_array_path(value: &Value, path: &[&str]) -> Vec<Value> {
    let mut cursor = value;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return Vec::new();
        };
        cursor = next;
    }
    cursor.as_array().cloned().unwrap_or_default()
}

/// 仅在值非空时覆盖对象字段。
fn set_if_non_empty(obj: &mut Map<String, Value>, key: &str, value: String) {
    if value.trim().is_empty() {
        return;
    }
    obj.insert(key.to_string(), Value::String(value));
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::{
        attach_agent_context_metrics, build_model_lookup, build_sessions_payload, discover,
        parse_auth_user_by_provider, parse_channel_identities, parse_profile_key_from_cmd,
        parse_status_default_agent_id, parse_status_recent_sessions, parse_usage_windows,
        resolve_profile_state_dir, select_agents_by_workspace, select_sessions_by_agents,
        to_percent,
    };
    use crate::{ProcInfo, tooling::core::types::ToolDiscoveryContext};

    #[test]
    fn parse_profile_key_supports_dev_profile_and_default() {
        assert_eq!(parse_profile_key_from_cmd("openclaw --dev"), "dev");
        assert_eq!(
            parse_profile_key_from_cmd("openclaw --profile work-ai"),
            "work-ai"
        );
        assert_eq!(
            parse_profile_key_from_cmd("openclaw --profile=research"),
            "research"
        );
        assert_eq!(parse_profile_key_from_cmd("openclaw run"), "default");
    }

    #[test]
    fn resolve_state_dir_by_profile() {
        assert!(
            resolve_profile_state_dir("default")
                .to_string_lossy()
                .contains(".openclaw")
        );
        assert!(
            resolve_profile_state_dir("dev")
                .to_string_lossy()
                .contains(".openclaw-dev")
        );
        assert!(
            resolve_profile_state_dir("team")
                .to_string_lossy()
                .contains(".openclaw-team")
        );
    }

    #[test]
    fn parse_status_paths_from_nested_objects() {
        let status = json!({
            "heartbeat": {"defaultAgentId":"main"},
            "sessions": {"recent":[{"sessionId":"s1","agentId":"main","updatedAt":1000}]},
            "usage": {"providers":[{"provider":"codex","displayName":"Codex","windows":[{"label":"5h","usedPercent":12}]}]}
        });
        assert_eq!(parse_status_default_agent_id(&status), "main");
        assert_eq!(parse_status_recent_sessions(&status).len(), 1);
        assert_eq!(parse_usage_windows(&status, &HashMap::new()).len(), 1);
    }

    #[test]
    fn parse_auth_user_prefers_label_parentheses() {
        let models_status = json!({
            "auth": {
                "oauth": {
                    "providers": [{
                        "provider": "google-gemini-cli",
                        "profiles": [{
                            "profileId": "google-gemini-cli:default",
                            "label": "google-gemini-cli:default (demo@example.com)"
                        }]
                    }]
                }
            }
        });
        let rows = parse_auth_user_by_provider(Some(&models_status));
        assert_eq!(
            rows.get("google-gemini-cli").cloned().unwrap_or_default(),
            "demo@example.com"
        );
    }

    #[test]
    fn workspace_filter_falls_back_to_all_agents() {
        let agents = vec![
            json!({"agentId":"a1","workspaceDir":"/w/a"}),
            json!({"agentId":"a2","workspaceDir":"/w/b"}),
        ];

        let matched = select_agents_by_workspace(&agents, "/w/a");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0]["agentId"], "a1");

        let fallback = select_agents_by_workspace(&agents, "/w/c");
        assert_eq!(fallback.len(), 2);
    }

    #[test]
    fn session_filter_falls_back_when_agent_keys_miss() {
        let scoped_agents = vec![json!({"agentId":"a1"})];
        let sessions = vec![
            json!({"agentId":"a1","sessionId":"s1","updatedAt":2}),
            json!({"agentId":"a2","sessionId":"s2","updatedAt":1}),
        ];

        let matched = select_sessions_by_agents(&sessions, &scoped_agents);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0]["sessionId"], "s1");

        let fallback = select_sessions_by_agents(&sessions, &[json!({"agentId":"unknown"})]);
        assert_eq!(fallback.len(), 2);
    }

    #[test]
    fn parse_channel_identities_from_channel_accounts() {
        let channels = json!({
            "channelOrder": ["telegram"],
            "channelLabels": {"telegram":"Telegram"},
            "channelDefaultAccountId": {"telegram":"default"},
            "channelAccounts": {
                "telegram": [{
                    "accountId":"default",
                    "running":true,
                    "configured":true,
                    "mode":"polling"
                }]
            }
        });
        let status = json!({});
        let ids = parse_channel_identities(Some(&channels), None, &status);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0]["displayLabel"], "Telegram");
        assert_eq!(ids[0]["accountId"], "default");
        assert_eq!(ids[0]["accountDisplay"], "default");
    }

    #[test]
    fn attach_context_prefers_session_context() {
        let agents = vec![json!({
            "agentId":"main",
            "name":"main",
            "model":"deepseek-chat",
            "isDefault":true
        })];
        let sessions = vec![json!({
            "agentId":"main",
            "model":"deepseek-chat",
            "contextTokens":10000,
            "remainingTokens":2500,
            "percentUsed":75,
            "updatedAt":100
        })];
        let model_lookup = build_model_lookup(&[]);
        let rows = attach_agent_context_metrics(agents, &sessions, 9000, &model_lookup);
        assert_eq!(rows[0]["contextMaxTokens"], 10000);
        assert_eq!(rows[0]["contextUsedTokens"], 7500);
        assert_eq!(rows[0]["contextLimitSource"], "session");
    }

    #[test]
    fn sessions_payload_builds_diagnostics() {
        let sessions = vec![
            json!({"sessionId":"s1","updatedAt":1,"systemSent":true,"abortedLastRun":false}),
            json!({"sessionId":"s2","updatedAt":1,"systemSent":false,"abortedLastRun":true}),
        ];
        let payload = build_sessions_payload(&sessions);
        assert_eq!(payload["diagnostics"]["total"], 2);
        assert_eq!(payload["diagnostics"]["abortedCount"], 1);
        assert_eq!(payload["diagnostics"]["abortedRate24h"], 50);
        assert_eq!(payload["diagnostics"]["systemCount"], 1);
        assert_eq!(payload["diagnostics"]["systemRatio"], 50);
    }

    #[test]
    fn percent_handles_zero_safely() {
        assert_eq!(to_percent(0, 0), 0);
        assert_eq!(to_percent(5, 0), 0);
        assert_eq!(to_percent(1, 4), 25);
    }

    #[test]
    fn discover_prefers_gateway_child_over_parent_openclaw() {
        let mut all = HashMap::new();
        all.insert(
            57565,
            ProcInfo {
                pid: 57565,
                cmd: "openclaw".to_string(),
                cwd: "/workspace/demo".to_string(),
                cpu_percent: 0.1,
                memory_mb: 10.0,
            },
        );
        all.insert(
            57567,
            ProcInfo {
                pid: 57567,
                cmd: "openclaw-gateway --port 18789".to_string(),
                cwd: "/workspace/demo".to_string(),
                cpu_percent: 0.2,
                memory_mb: 11.0,
            },
        );
        let mut children_by_ppid = HashMap::new();
        children_by_ppid.insert(57565, vec![57567]);
        let context = ToolDiscoveryContext {
            all: &all,
            children_by_ppid: &children_by_ppid,
        };

        let discovered = discover(&context);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].pid, Some(57567));
        assert!(discovered[0].tool_id.ends_with("_gw"));
    }

    #[test]
    fn discover_keeps_openclaw_when_gateway_child_missing() {
        let mut all = HashMap::new();
        all.insert(
            57565,
            ProcInfo {
                pid: 57565,
                cmd: "openclaw --profile team".to_string(),
                cwd: "/workspace/demo".to_string(),
                cpu_percent: 0.1,
                memory_mb: 10.0,
            },
        );
        let children_by_ppid = HashMap::new();
        let context = ToolDiscoveryContext {
            all: &all,
            children_by_ppid: &children_by_ppid,
        };

        let discovered = discover(&context);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].pid, Some(57565));
        assert!(!discovered[0].tool_id.ends_with("_gw"));
    }
}
