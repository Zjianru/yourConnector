//! 会话快照：tools_snapshot / tools_candidates / metrics_snapshot。

use anyhow::Result;
use futures_util::Sink;
use serde_json::json;
use sysinfo::{Disks, ProcessesToUpdate, System};
use tokio_tungstenite::tungstenite::Message;
use yc_shared_protocol::{
    MetricsSnapshotPayload, SidecarMetricsPayload, SystemMetricsPayload, ToolRuntimePayload,
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
        serde_json::to_value(collect_metrics_snapshot(sys, started_at, &connected_tools))?,
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

    for tool in discovered_tools.iter().cloned() {
        if whitelist.contains(&tool.tool_id) {
            connected.push(tool);
        } else {
            candidates.push(tool);
        }
    }

    (connected, candidates)
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
