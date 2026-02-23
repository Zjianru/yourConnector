//! Tool Adapter Core 模块职责：
//! 1. 统一调度 OpenCode/OpenClaw 的发现与详情采集。
//! 2. 维护工具详情缓存、过期标记与按需刷新去抖策略。
//! 3. 对会话循环提供稳定的发现与详情快照接口。

pub(crate) mod cache;
pub(crate) mod scheduler;
pub(crate) mod types;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, Utc};
use sysinfo::{ProcessesToUpdate, System};
use yc_shared_protocol::{ToolDetailEnvelopePayload, ToolRuntimePayload, now_rfc3339_nanos};

use self::{
    cache::ToolDetailsCache,
    scheduler::{default_detail_ttl, filter_tools_by_target},
    types::{
        ToolDetailCollectOptions, ToolDetailCollectResult, ToolDetailsCollectRequest,
        ToolDiscoveryContext,
    },
};
use crate::{
    ProcInfo, fallback_tools_or_empty,
    tooling::{
        adapters::{OPENCLAW_SCHEMA_V1, OPENCODE_SCHEMA_V1, openclaw, opencode},
        bytes_to_mb,
    },
};

/// 工具核心组件：管理发现与详情缓存。
#[derive(Debug)]
pub(crate) struct ToolAdapterCore {
    /// 无工具时是否注入 fallback 占位。
    fallback_tool: bool,
    /// 详情缓存。
    details_cache: ToolDetailsCache,
    /// 详情采集选项。
    detail_options: ToolDetailCollectOptions,
    /// 按需刷新去抖窗口。
    detail_debounce: Duration,
}

impl ToolAdapterCore {
    /// 构造工具核心组件。
    pub(crate) fn new(
        fallback_tool: bool,
        detail_interval: Duration,
        detail_command_timeout: Duration,
        detail_max_parallel: usize,
        detail_debounce: Duration,
    ) -> Self {
        Self {
            fallback_tool,
            details_cache: ToolDetailsCache::new(),
            detail_options: ToolDetailCollectOptions {
                detail_ttl: default_detail_ttl(detail_interval),
                command_timeout: detail_command_timeout,
                max_parallel: detail_max_parallel.max(1),
            },
            detail_debounce,
        }
    }

    /// 扫描系统进程并发现工具实例。
    pub(crate) fn discover_tools(&self, sys: &mut System) -> Vec<ToolRuntimePayload> {
        let (all, children_by_ppid) = collect_process_snapshot(sys);
        let context = ToolDiscoveryContext {
            all: &all,
            children_by_ppid: &children_by_ppid,
        };

        let mut tools = Vec::new();
        tools.extend(opencode::discover(&context));
        tools.extend(openclaw::discover(&context));

        if tools.is_empty() {
            return fallback_tools_or_empty(self.fallback_tool);
        }

        tools.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.workspace_dir.cmp(&b.workspace_dir))
                .then_with(|| a.pid.unwrap_or_default().cmp(&b.pid.unwrap_or_default()))
                .then_with(|| a.tool_id.cmp(&b.tool_id))
        });
        tools
    }

    /// 执行详情采集并输出当前详情快照。
    pub(crate) async fn collect_details_snapshot(
        &mut self,
        request: ToolDetailsCollectRequest,
    ) -> Vec<ToolDetailEnvelopePayload> {
        let ordered_ids = request
            .tools
            .iter()
            .map(|tool| tool.tool_id.clone())
            .collect::<Vec<String>>();
        self.details_cache.prune_inactive(&ordered_ids);

        let target_tools =
            filter_tools_by_target(&request.tools, request.target_tool_id.as_deref());
        if target_tools.is_empty() {
            return self.details_cache.snapshot_for_tool_order(&ordered_ids);
        }

        let now = Instant::now();
        let mut collect_targets = Vec::new();
        for tool in target_tools {
            if !request.force
                && self
                    .details_cache
                    .is_debounced(&tool.tool_id, self.detail_debounce, now)
            {
                continue;
            }
            self.details_cache.mark_collect_attempt(&tool.tool_id, now);
            collect_targets.push(tool);
        }

        if collect_targets.is_empty() {
            return self.details_cache.snapshot_for_tool_order(&ordered_ids);
        }

        let (opencode_tools, openclaw_tools, unknown_tools) =
            partition_tools_by_adapter(&collect_targets);

        let mut results = Vec::new();
        results.extend(opencode::collect_details(
            &opencode_tools,
            &self.detail_options,
        ));
        let include_openclaw_deep_details = request.force && request.target_tool_id.is_some();
        results.extend(
            openclaw::collect_details(
                &openclaw_tools,
                &self.detail_options,
                include_openclaw_deep_details,
            )
            .await,
        );

        for tool in unknown_tools {
            results.push(ToolDetailCollectResult::failed(
                tool.tool_id,
                "unknown.v1",
                None,
                "当前工具类型未实现详情采集",
            ));
        }

        apply_collect_results(
            &mut self.details_cache,
            &collect_targets,
            results,
            &self.detail_options,
        );
        self.details_cache.snapshot_for_tool_order(&ordered_ids)
    }
}

/// 按适配器类型拆分工具集合。
fn partition_tools_by_adapter(
    tools: &[ToolRuntimePayload],
) -> (
    Vec<ToolRuntimePayload>,
    Vec<ToolRuntimePayload>,
    Vec<ToolRuntimePayload>,
) {
    let mut opencode_tools = Vec::new();
    let mut openclaw_tools = Vec::new();
    let mut unknown_tools = Vec::new();

    for tool in tools {
        if openclaw::matches_tool(tool) {
            openclaw_tools.push(tool.clone());
            continue;
        }
        if opencode::matches_tool(tool) {
            opencode_tools.push(tool.clone());
            continue;
        }
        unknown_tools.push(tool.clone());
    }

    (opencode_tools, openclaw_tools, unknown_tools)
}

/// 把采集结果合并到缓存：成功写新值，失败标记 stale 并保留旧 data。
fn apply_collect_results(
    cache: &mut ToolDetailsCache,
    targets: &[ToolRuntimePayload],
    results: Vec<ToolDetailCollectResult>,
    options: &ToolDetailCollectOptions,
) {
    let mut result_by_tool: HashMap<String, ToolDetailCollectResult> = HashMap::new();
    for result in results {
        result_by_tool.insert(result.tool_id.clone(), result);
    }

    let now = Utc::now();
    let ttl_secs = options.detail_ttl.as_secs().min(i64::MAX as u64) as i64;
    let expires = now + ChronoDuration::seconds(ttl_secs);
    let collected_at = now_rfc3339_nanos();
    let expires_at = expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    for tool in targets {
        let default_schema = schema_for_tool(tool);
        let Some(result) = result_by_tool.remove(&tool.tool_id) else {
            cache.mark_stale(
                &tool.tool_id,
                default_schema,
                None,
                "详情采集失败：未返回结果",
                Some(expires_at.clone()),
            );
            continue;
        };

        if let Some(data) = result.data {
            cache.upsert_success(ToolDetailEnvelopePayload {
                tool_id: tool.tool_id.clone(),
                schema: if result.schema.trim().is_empty() {
                    default_schema.to_string()
                } else {
                    result.schema
                },
                stale: false,
                collected_at: Some(collected_at.clone()),
                expires_at: Some(expires_at.clone()),
                profile_key: result.profile_key,
                data,
            });
            continue;
        }

        cache.mark_stale(
            &tool.tool_id,
            if result.schema.trim().is_empty() {
                default_schema
            } else {
                result.schema.as_str()
            },
            result.profile_key,
            result.error.as_deref().unwrap_or("详情采集失败：未知错误"),
            Some(expires_at.clone()),
        );
    }
}

/// 根据工具标识推断 schema，供失败兜底分支使用。
fn schema_for_tool(tool: &ToolRuntimePayload) -> &'static str {
    if openclaw::matches_tool(tool) {
        return OPENCLAW_SCHEMA_V1;
    }
    if opencode::matches_tool(tool) {
        return OPENCODE_SCHEMA_V1;
    }
    "unknown.v1"
}

/// 从 sysinfo 采集进程快照并构建父子关系索引。
fn collect_process_snapshot(sys: &mut System) -> (HashMap<i32, ProcInfo>, HashMap<i32, Vec<i32>>) {
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let mut all: HashMap<i32, ProcInfo> = HashMap::new();
    let mut children_by_ppid: HashMap<i32, Vec<i32>> = HashMap::new();

    for process in sys.processes().values() {
        let pid = process.pid().as_u32() as i32;
        let ppid = process
            .parent()
            .map(|parent| parent.as_u32() as i32)
            .unwrap_or(0);
        let cmd = process
            .cmd()
            .iter()
            .map(|item| item.to_string_lossy().to_string())
            .collect::<Vec<String>>()
            .join(" ");
        if cmd.is_empty() {
            continue;
        }
        let cwd = process
            .cwd()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();

        all.insert(
            pid,
            ProcInfo {
                pid,
                cmd,
                cwd,
                cpu_percent: process.cpu_usage() as f64,
                memory_mb: bytes_to_mb(process.memory()),
            },
        );
        children_by_ppid.entry(ppid).or_default().push(pid);
    }

    (all, children_by_ppid)
}

#[cfg(test)]
mod tests {
    use super::ToolAdapterCore;

    #[test]
    fn core_keeps_parallelism_positive() {
        let core = ToolAdapterCore::new(
            false,
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(2),
            0,
            std::time::Duration::from_secs(3),
        );
        assert!(core.detail_options.max_parallel >= 1);
    }
}
