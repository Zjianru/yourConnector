//! WebSocket 消息净化与 server_presence 发送。

use axum::extract::ws::Message;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;
use yc_shared_protocol::{EventEnvelope, now_rfc3339_nanos};

use crate::state::RelayWriteCommand;

/// 事件摘要：用于日志追踪，避免打印完整 payload。
#[derive(Debug, Clone, Default)]
pub(crate) struct EnvelopeSummary {
    /// 事件类型。
    pub(crate) event_type: String,
    /// 事件 ID。
    pub(crate) event_id: String,
    /// 链路追踪 ID。
    pub(crate) trace_id: String,
    /// 目标工具 ID（可选）。
    pub(crate) tool_id: String,
}

/// 校验并修正上行 envelope。
pub(crate) fn sanitize_envelope(
    raw: &str,
    system_id: &str,
    source_client_type: &str,
    source_device_id: &str,
) -> Result<String, String> {
    let mut env: Value = serde_json::from_str(raw).map_err(|err| err.to_string())?;
    let obj = env
        .as_object_mut()
        .ok_or_else(|| "envelope must be an object".to_string())?;

    if !obj.contains_key("v") {
        obj.insert("v".to_string(), json!(1));
    }

    let event_id_empty = obj
        .get("eventId")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::is_empty)
        .unwrap_or(true);
    if event_id_empty {
        obj.insert(
            "eventId".to_string(),
            Value::String(format!("evt_{}", Uuid::new_v4())),
        );
    }

    let trace_id_empty = obj
        .get("traceId")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::is_empty)
        .unwrap_or(true);
    if trace_id_empty {
        obj.insert(
            "traceId".to_string(),
            Value::String(format!("trc_{}", Uuid::new_v4())),
        );
    }

    let event_type = obj
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if event_type.is_empty() {
        return Err("missing type".to_string());
    }

    if let Some(sid) = obj.get("systemId").and_then(Value::as_str)
        && sid != system_id
    {
        return Err("systemId mismatch".to_string());
    }

    obj.insert("systemId".to_string(), Value::String(system_id.to_string()));
    obj.insert(
        "sourceClientType".to_string(),
        Value::String(source_client_type.to_string()),
    );
    obj.insert(
        "sourceDeviceId".to_string(),
        Value::String(source_device_id.to_string()),
    );
    obj.insert(
        "peerId".to_string(),
        Value::String(source_device_id.to_string()),
    );

    let ts_empty = obj
        .get("ts")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::is_empty)
        .unwrap_or(true);
    if ts_empty {
        obj.insert("ts".to_string(), Value::String(now_rfc3339_nanos()));
    }

    if !matches!(obj.get("payload"), Some(v) if v.is_object()) {
        obj.insert("payload".to_string(), json!({}));
    }

    serde_json::to_string(&env).map_err(|err| err.to_string())
}

/// 提取日志摘要字段，供 relay/sidecar 记录链路日志。
pub(crate) fn summarize_envelope(raw: &str) -> EnvelopeSummary {
    let parsed = serde_json::from_str::<Value>(raw);
    let Ok(value) = parsed else {
        return EnvelopeSummary::default();
    };
    let payload = value
        .get("payload")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    EnvelopeSummary {
        event_type: value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        event_id: value
            .get("eventId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        trace_id: value
            .get("traceId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tool_id: payload
            .get("toolId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

/// 连接成功后回推 server_presence。
pub(crate) fn send_server_presence(
    tx: &mpsc::Sender<RelayWriteCommand>,
    system_id: &str,
    client_type: &str,
    device_id: &str,
) {
    let env = EventEnvelope::new(
        "server_presence",
        system_id,
        json!({
            "status": "connected",
            "clientType": client_type,
            "deviceId": device_id,
        }),
    );

    if let Ok(raw) = serde_json::to_string(&env) {
        let _ = tx.try_send(RelayWriteCommand::Direct(Message::Text(raw.into())));
    }
}
