// 文件职责：
// 1) 定义 relay/sidecar/mobile 共用的协议数据结构。
// 2) 提供时间戳、clientType 归一化等跨端一致的基础函数。
// 3) 作为 Rust 侧协议唯一代码源，供其他服务复用。

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    // 协议版本号。
    pub v: u8,
    #[serde(rename = "eventId")]
    // 事件唯一 ID。
    pub event_id: String,
    #[serde(rename = "traceId", skip_serializing_if = "Option::is_none")]
    // 分布式链路追踪 ID（可选）。
    pub trace_id: Option<String>,
    #[serde(rename = "type")]
    // 事件类型。
    pub event_type: String,
    #[serde(rename = "systemId")]
    // 宿主系统标识。
    pub system_id: String,
    #[serde(rename = "toolId", skip_serializing_if = "Option::is_none")]
    // 工具标识（可选）。
    pub tool_id: Option<String>,
    #[serde(rename = "peerId", skip_serializing_if = "Option::is_none")]
    // 对端设备标识（可选）。
    pub peer_id: Option<String>,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    // 会话标识（可选）。
    pub session_id: Option<String>,
    #[serde(rename = "sourceClientType", skip_serializing_if = "Option::is_none")]
    // relay 注入的可信来源客户端类型（可选）。
    pub source_client_type: Option<String>,
    #[serde(rename = "sourceDeviceId", skip_serializing_if = "Option::is_none")]
    // relay 注入的可信来源设备 ID（可选）。
    pub source_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 事件序号（可选）。
    pub seq: Option<u64>,
    // 事件时间（RFC3339）。
    pub ts: String,
    #[serde(rename = "ackRequired", skip_serializing_if = "Option::is_none")]
    // 是否要求 ACK（可选）。
    pub ack_required: Option<bool>,
    // 事件负载。
    pub payload: Value,
}

impl EventEnvelope {
    /// 构造默认 envelope：自动填充版本、eventId、ts。
    pub fn new(
        event_type: impl Into<String>,
        system_id: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            v: 1,
            event_id: format!("evt_{}", Uuid::new_v4()),
            trace_id: Some(format!("trc_{}", Uuid::new_v4())),
            event_type: event_type.into(),
            system_id: system_id.into(),
            tool_id: None,
            peer_id: None,
            session_id: None,
            source_client_type: None,
            source_device_id: None,
            seq: None,
            ts: now_rfc3339_nanos(),
            ack_required: None,
            payload,
        }
    }
}

/// 生成纳秒精度 UTC 时间戳（RFC3339）。
pub fn now_rfc3339_nanos() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

/// 归一化 clientType，保持历史兼容（mobile -> app）。
pub fn normalize_client_type(raw: &str) -> String {
    match raw {
        "mobile" => "app".to_string(),
        _ => raw.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LatestTokensPayload {
    // Token 总量。
    pub total: i64,
    // 输入 Token。
    pub input: i64,
    // 输出 Token。
    pub output: i64,
    // 缓存读取 Token。
    pub cache_read: i64,
    // 缓存写入 Token。
    pub cache_write: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsagePayload {
    // 模型名称（provider/model）。
    pub model: String,
    // 消息数量。
    pub messages: i64,
    // 总 token。
    pub token_total: i64,
    // 输入 token。
    pub token_input: i64,
    // 输出 token。
    pub token_output: i64,
    // 缓存读 token。
    pub cache_read: i64,
    // 缓存写 token。
    pub cache_write: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolRuntimePayload {
    // 工具唯一 ID。
    pub tool_id: String,
    // 工具显示名称。
    pub name: String,
    // 工具类别。
    pub category: String,
    // 工具厂商。
    pub vendor: String,
    // 运行模式（TUI/CLI/SERVE）。
    pub mode: String,
    // 运行状态。
    pub status: String,
    // 是否可连接。
    pub connected: bool,
    // 可访问地址（如有）。
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 运行进程 PID（可选）。
    pub pid: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 运行说明或异常原因（可选）。
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 工具 CPU 使用率（可选）。
    pub cpu_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 工具内存占用 MB（可选）。
    pub memory_mb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 采集来源标识（可选）。
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 工作目录（可选）。
    pub workspace_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 会话 ID（可选）。
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 会话标题（可选）。
    pub session_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 会话更新时间（可选）。
    pub session_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // agent 工作模式（可选）。
    pub agent_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 模型供应商 ID（可选）。
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 模型 ID（可选）。
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 模型展示名（可选）。
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 最近一次消息 token 快照（可选）。
    pub latest_tokens: Option<LatestTokensPayload>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    // 当前会话按模型聚合的用量（可选）。
    pub model_usage: Vec<ModelUsagePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 采集时间（可选）。
    pub collected_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SidecarMetricsPayload {
    // sidecar CPU 使用率。
    pub cpu_percent: f64,
    // sidecar 内存（MB）。
    pub memory_mb: f64,
    // 历史兼容字段（Go 版本遗留）。
    pub goroutines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetricsPayload {
    // 系统 CPU 百分比。
    pub cpu_percent: f64,
    // 总内存（MB）。
    pub memory_total_mb: f64,
    // 已用内存（MB）。
    pub memory_used_mb: f64,
    // 内存使用率。
    pub memory_used_percent: f64,
    // 磁盘总量（GB）。
    pub disk_total_gb: f64,
    // 磁盘已用（GB）。
    pub disk_used_gb: f64,
    // 磁盘使用率。
    pub disk_used_percent: f64,
    // sidecar 启动后运行秒数。
    pub uptime_sec: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolsSnapshotPayload {
    // 当前工具列表（connected 或 candidates）。
    pub tools: Vec<ToolRuntimePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshotPayload {
    // 系统指标。
    pub system: SystemMetricsPayload,
    // sidecar 指标。
    pub sidecar: SidecarMetricsPayload,
    // 主工具指标（兼容字段）。
    pub tool: Value,
    // 所有工具指标。
    pub tools: Vec<ToolRuntimePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetailEnvelopePayload {
    // 目标工具 ID。
    pub tool_id: String,
    // 数据结构版本（如 openclaw.v1 / opencode.v1）。
    pub schema: String,
    // 是否为过期缓存（true 表示本次采集失败或超时）。
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 本次数据采集时间。
    pub collected_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 本次数据过期时间。
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 配置来源分组键（如 openclaw profile）。
    pub profile_key: Option<String>,
    // 结构化详情数据（按 schema 解释）。
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetailsSnapshotPayload {
    // 当前详情快照列表。
    pub details: Vec<ToolDetailEnvelopePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetailsRefreshRequestPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    // 指定刷新目标工具；空表示刷新当前宿主机全部工具详情。
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    // 是否忽略去抖与缓存直接强制刷新。
    pub force: Option<bool>,
}
