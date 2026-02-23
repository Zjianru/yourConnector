//! Tool Adapter Core 调度辅助职责：
//! 1. 提供详情采集默认周期与去抖常量。
//! 2. 提供面向目标工具的过滤函数，减少主流程样板代码。

use std::time::Duration;

use yc_shared_protocol::ToolRuntimePayload;

/// 详情补采默认周期（秒）。
pub(crate) const DEFAULT_DETAILS_INTERVAL_SEC: u64 = 45;
/// 详情按需刷新去抖窗口（秒）。
pub(crate) const DEFAULT_DETAILS_DEBOUNCE_SEC: u64 = 3;
/// 外部 CLI 命令默认超时（毫秒）。
pub(crate) const DEFAULT_DETAILS_COMMAND_TIMEOUT_MS: u64 = 8_000;
/// 详情采集默认并发上限。
pub(crate) const DEFAULT_DETAILS_MAX_PARALLEL: usize = 2;

/// 详情条目默认 TTL：取 `details_interval * 2`，避免短暂抖动导致频繁过期。
pub(crate) fn default_detail_ttl(details_interval: Duration) -> Duration {
    let base = details_interval.as_secs().max(1);
    Duration::from_secs(base.saturating_mul(2))
}

/// 根据 target_tool_id 过滤工具集合；空 target 表示全量。
pub(crate) fn filter_tools_by_target(
    tools: &[ToolRuntimePayload],
    target_tool_id: Option<&str>,
) -> Vec<ToolRuntimePayload> {
    let Some(target) = target_tool_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return tools.to_vec();
    };

    tools
        .iter()
        .filter(|tool| tool.tool_id.trim() == target)
        .cloned()
        .collect()
}
