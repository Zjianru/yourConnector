//! 会话快照：tools_snapshot / tools_candidates / metrics_snapshot。

use anyhow::Result;
use futures_util::Sink;
use serde_json::json;
use std::collections::HashSet;
use sysinfo::{Disks, ProcessesToUpdate, System};
use tokio_tungstenite::tungstenite::Message;
use yc_shared_protocol::{
    MetricsSnapshotPayload, SidecarMetricsPayload, SystemMetricsPayload, ToolDetailEnvelopePayload,
    ToolDetailsSnapshotPayload, ToolDetailsSnapshotTrigger, ToolRuntimePayload,
    ToolsSnapshotPayload, now_rfc3339_nanos,
};

use crate::{
    bytes_to_gb, bytes_to_mb, config::Config, round2, session::transport::send_event,
    stores::ToolWhitelistStore,
};

/// 已接入工具快照事件。
pub(crate) const TOOLS_SNAPSHOT_EVENT: &str = "tools_snapshot";
/// 候选工具快照事件。
pub(crate) const TOOLS_CANDIDATES_EVENT: &str = "tools_candidates";
/// 系统/sidecar/工具指标快照事件。
pub(crate) const METRICS_SNAPSHOT_EVENT: &str = "metrics_snapshot";
/// 工具详情快照事件。
pub(crate) const TOOL_DETAILS_SNAPSHOT_EVENT: &str = "tool_details_snapshot";

/// 详情快照下行元信息。
#[derive(Debug, Clone)]
pub(crate) struct ToolDetailsSnapshotMeta {
    pub(crate) snapshot_id: u64,
    pub(crate) refresh_id: Option<String>,
    pub(crate) trigger: ToolDetailsSnapshotTrigger,
    pub(crate) target_tool_id: Option<String>,
    pub(crate) queue_wait_ms: u64,
    pub(crate) collect_ms: u64,
    pub(crate) send_ms: u64,
    pub(crate) dropped_refreshes: u32,
}

/// 一次性发送 tools_snapshot / tools_candidates / metrics_snapshot 三个事件。
pub(crate) async fn send_snapshots<W>(
    ws_writer: &mut W,
    cfg: &Config,
    seq: &mut u64,
    sys: &mut System,
    started_at: std::time::Instant,
    discovered_tools: &[ToolRuntimePayload],
    whitelist: &ToolWhitelistStore,
) -> Result<()>
where
    W: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let (connected_tools, candidate_tools) = split_discovered_tools(discovered_tools, whitelist);

    send_event(
        ws_writer,
        &cfg.system_id,
        seq,
        TOOLS_SNAPSHOT_EVENT,
        None,
        serde_json::to_value(ToolsSnapshotPayload {
            tools: connected_tools.clone(),
        })?,
    )
    .await?;

    send_event(
        ws_writer,
        &cfg.system_id,
        seq,
        TOOLS_CANDIDATES_EVENT,
        None,
        serde_json::to_value(ToolsSnapshotPayload {
            tools: candidate_tools,
        })?,
    )
    .await?;

    send_event(
        ws_writer,
        &cfg.system_id,
        seq,
        METRICS_SNAPSHOT_EVENT,
        None,
        serde_json::to_value(collect_metrics_snapshot(sys, started_at, &connected_tools))?,
    )
    .await?;

    Ok(())
}

/// 发送工具详情快照（按 toolId 对齐）。
pub(crate) async fn send_tool_details_snapshot<W>(
    ws_writer: &mut W,
    system_id: &str,
    seq: &mut u64,
    details: &[ToolDetailEnvelopePayload],
    meta: ToolDetailsSnapshotMeta,
) -> Result<()>
where
    W: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    send_event(
        ws_writer,
        system_id,
        seq,
        TOOL_DETAILS_SNAPSHOT_EVENT,
        None,
        serde_json::to_value(ToolDetailsSnapshotPayload {
            snapshot_id: meta.snapshot_id,
            refresh_id: meta.refresh_id,
            trigger: meta.trigger,
            target_tool_id: meta.target_tool_id,
            queue_wait_ms: meta.queue_wait_ms,
            collect_ms: meta.collect_ms,
            send_ms: meta.send_ms,
            dropped_refreshes: meta.dropped_refreshes,
            details: details.to_vec(),
        })?,
    )
    .await?;
    Ok(())
}

/// 根据白名单把“发现到的工具”分成已接入与候选两组。
fn split_discovered_tools(
    discovered_tools: &[ToolRuntimePayload],
    whitelist: &ToolWhitelistStore,
) -> (Vec<ToolRuntimePayload>, Vec<ToolRuntimePayload>) {
    let mut connected = Vec::new();
    let mut candidates = Vec::new();
    let mut connected_identity_keys = HashSet::new();
    let whitelist_ids = whitelist.list_ids();
    let single_openclaw_binding = whitelist_ids
        .iter()
        .filter(|id| id.starts_with("openclaw_"))
        .count()
        == 1;

    for tool in discovered_tools.iter().cloned() {
        if whitelist.contains_compatible(&tool.tool_id) {
            connected_identity_keys.insert(tool_identity_key(&tool.tool_id));
            connected.push(tool);
        } else {
            candidates.push(tool);
        }
    }

    // 已接入工具即使当前进程暂时不可见，也应继续展示在 connected 列表。
    let has_connected_openclaw = connected
        .iter()
        .any(|tool| tool.tool_id.starts_with("openclaw_"));
    for tool_id in whitelist_ids {
        let identity_key = tool_identity_key(&tool_id);
        if connected_identity_keys.contains(&identity_key) {
            continue;
        }
        if single_openclaw_binding && has_connected_openclaw && tool_id.starts_with("openclaw_") {
            // OpenClaw 单实例策略下，白名单 hash 漂移时保留真实在线项，不再补离线占位。
            continue;
        }
        connected.push(build_whitelist_placeholder_tool(&tool_id));
        connected_identity_keys.insert(identity_key);
    }

    (connected, candidates)
}

/// 生成白名单离线占位工具，保证“仅左滑删除才从 connected 消失”。
fn build_whitelist_placeholder_tool(tool_id: &str) -> ToolRuntimePayload {
    let (name, vendor, category, mode, tool_class) = if tool_id.starts_with("openclaw_") {
        ("OpenClaw", "OpenClaw", "CLI", "CLI", "assistant")
    } else if tool_id.starts_with("opencode_") {
        ("OpenCode", "OpenCode", "TUI", "TUI", "code")
    } else {
        ("Connected Tool", "Unknown", "UNKNOWN", "-", "assistant")
    };

    ToolRuntimePayload {
        tool_id: tool_id.to_string(),
        name: name.to_string(),
        tool_class: tool_class.to_string(),
        category: category.to_string(),
        vendor: vendor.to_string(),
        mode: mode.to_string(),
        status: "OFFLINE".to_string(),
        connected: true,
        reason: Some("已接入，当前进程未运行。重新启动后会自动恢复。".to_string()),
        source: Some("whitelist-placeholder".to_string()),
        collected_at: Some(now_rfc3339_nanos()),
        ..ToolRuntimePayload::default()
    }
}

/// 生成白名单匹配身份键，用于兼容 OpenClaw gateway 旧/新 toolId 形态。
fn tool_identity_key(tool_id: &str) -> String {
    let Some(rest) = tool_id.strip_prefix("openclaw_") else {
        return tool_id.to_string();
    };

    if let Some(hash) = rest.strip_suffix("_gw") {
        return format!("openclaw::{hash}");
    }

    if let Some((hash, pid_text)) = rest.rsplit_once("_p")
        && !hash.trim().is_empty()
        && !pid_text.trim().is_empty()
        && pid_text.chars().all(|ch| ch.is_ascii_digit())
    {
        return format!("openclaw::{hash}");
    }

    tool_id.to_string()
}

/// 判定是否为 fallback 占位工具（不可接入）。
pub(crate) fn is_fallback_tool(tool: &ToolRuntimePayload) -> bool {
    if tool.tool_id == "tool_local" {
        return true;
    }
    matches!(tool.source.as_deref(), Some("fallback"))
}

/// 将下行原始 payload 压缩为事件摘要，避免默认日志泄漏业务正文。
pub(crate) fn summarize_wire_payload(raw: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(raw);
    let Ok(value) = parsed else {
        return "non-json message".to_string();
    };
    let event_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let event_id = value.get("eventId").and_then(|v| v.as_str()).unwrap_or("");
    let trace_id = value.get("traceId").and_then(|v| v.as_str()).unwrap_or("");
    let payload = value
        .get("payload")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let tool_id = payload.get("toolId").and_then(|v| v.as_str()).unwrap_or("");
    let status = payload.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("");

    let mut parts = vec![event_type.to_string()];
    if !tool_id.is_empty() {
        parts.push(format!("tool={tool_id}"));
    }
    if !status.is_empty() {
        parts.push(format!("status={status}"));
    }
    if !action.is_empty() {
        parts.push(format!("action={action}"));
    }
    if !event_id.is_empty() {
        parts.push(format!("event_id={event_id}"));
    }
    if !trace_id.is_empty() {
        parts.push(format!("trace_id={trace_id}"));
    }
    parts.join(" ")
}

/// 采集系统/sidecar/工具指标，生成统一的 metrics payload。
fn collect_metrics_snapshot(
    sys: &mut System,
    started_at: std::time::Instant,
    tools: &[ToolRuntimePayload],
) -> MetricsSnapshotPayload {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let cpu_percent = round2(sys.global_cpu_usage() as f64);
    let memory_total_mb = round2(bytes_to_mb(sys.total_memory()));
    let memory_used_mb = round2(bytes_to_mb(sys.used_memory()));
    let memory_used_percent = if memory_total_mb <= 0.0 {
        0.0
    } else {
        round2(memory_used_mb / memory_total_mb * 100.0)
    };

    let disks = Disks::new_with_refreshed_list();
    let disk_total = disks.list().iter().map(|d| d.total_space()).sum::<u64>();
    let disk_available = disks
        .list()
        .iter()
        .map(|d| d.available_space())
        .sum::<u64>();
    let disk_used = disk_total.saturating_sub(disk_available);

    let disk_total_gb = round2(bytes_to_gb(disk_total));
    let disk_used_gb = round2(bytes_to_gb(disk_used));
    let disk_used_percent = if disk_total_gb <= 0.0 {
        0.0
    } else {
        round2(disk_used_gb / disk_total_gb * 100.0)
    };

    let mut sidecar_cpu = 0.0;
    let mut sidecar_mem_mb = 0.0;
    if let Ok(pid) = sysinfo::get_current_pid()
        && let Some(proc_info) = sys.process(pid)
    {
        sidecar_cpu = round2(proc_info.cpu_usage() as f64);
        sidecar_mem_mb = round2(bytes_to_mb(proc_info.memory()));
    }

    let tool_value = tools
        .first()
        .and_then(|tool| serde_json::to_value(tool).ok())
        .unwrap_or_else(|| json!({}));

    MetricsSnapshotPayload {
        system: SystemMetricsPayload {
            cpu_percent,
            memory_total_mb,
            memory_used_mb,
            memory_used_percent,
            disk_total_gb,
            disk_used_gb,
            disk_used_percent,
            uptime_sec: started_at.elapsed().as_secs(),
        },
        sidecar: SidecarMetricsPayload {
            cpu_percent: sidecar_cpu,
            memory_mb: sidecar_mem_mb,
            goroutines: 0,
        },
        tool: tool_value,
        tools: tools
            .iter()
            .cloned()
            .map(|mut tool| {
                tool.collected_at = Some(now_rfc3339_nanos());
                tool
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::split_discovered_tools;
    use crate::stores::ToolWhitelistStore;
    use yc_shared_protocol::ToolRuntimePayload;

    fn make_tool(tool_id: &str) -> ToolRuntimePayload {
        ToolRuntimePayload {
            tool_id: tool_id.to_string(),
            name: "OpenClaw".to_string(),
            category: "CLI".to_string(),
            vendor: "OpenClaw".to_string(),
            mode: "CLI".to_string(),
            status: "RUNNING".to_string(),
            connected: true,
            ..ToolRuntimePayload::default()
        }
    }

    #[test]
    fn whitelisted_tool_without_running_process_should_remain_connected() {
        let whitelist = ToolWhitelistStore::from_ids_for_test(&["openclaw_abcd1234ef56_gw"]);
        let discovered = Vec::<ToolRuntimePayload>::new();

        let (connected, candidates) = split_discovered_tools(&discovered, &whitelist);
        assert_eq!(candidates.len(), 0);
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].tool_id, "openclaw_abcd1234ef56_gw");
        assert_eq!(connected[0].status, "OFFLINE");
        assert!(connected[0].connected);
    }

    #[test]
    fn openclaw_gateway_legacy_and_stable_ids_should_not_duplicate() {
        let whitelist = ToolWhitelistStore::from_ids_for_test(&["openclaw_abcd1234ef56_p1024"]);
        let discovered = vec![make_tool("openclaw_abcd1234ef56_gw")];

        let (connected, candidates) = split_discovered_tools(&discovered, &whitelist);
        assert_eq!(candidates.len(), 0);
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].tool_id, "openclaw_abcd1234ef56_gw");
    }

    #[test]
    fn openclaw_hash_drift_should_still_bind_single_instance_card() {
        let whitelist = ToolWhitelistStore::from_ids_for_test(&["openclaw_abcd1234ef56_gw"]);
        let discovered = vec![make_tool("openclaw_ffffeeee1111_gw")];

        let (connected, candidates) = split_discovered_tools(&discovered, &whitelist);
        assert_eq!(candidates.len(), 0);
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].tool_id, "openclaw_ffffeeee1111_gw");
        assert_eq!(connected[0].status, "RUNNING");
    }
}
