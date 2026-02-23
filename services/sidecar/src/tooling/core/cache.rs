//! Tool Adapter Core 缓存职责：
//! 1. 维护按 toolId 索引的详情快照缓存。
//! 2. 在采集失败时将缓存标记为 stale 并保留旧值。
//! 3. 提供刷新去抖与失效工具清理能力。

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use yc_shared_protocol::ToolDetailEnvelopePayload;

/// 单宿主机工具详情缓存。
#[derive(Debug, Default)]
pub(crate) struct ToolDetailsCache {
    /// toolId -> 详情 envelope。
    by_tool_id: HashMap<String, ToolDetailEnvelopePayload>,
    /// toolId -> 最近一次采集尝试时间（用于去抖）。
    last_collect_attempt: HashMap<String, Instant>,
}

impl ToolDetailsCache {
    /// 构造空缓存。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 判断指定工具是否处于去抖窗口内。
    pub(crate) fn is_debounced(&self, tool_id: &str, debounce: Duration, now: Instant) -> bool {
        if debounce.is_zero() {
            return false;
        }
        self.last_collect_attempt
            .get(tool_id)
            .map(|last| now.duration_since(*last) < debounce)
            .unwrap_or(false)
    }

    /// 记录一次采集尝试时间。
    pub(crate) fn mark_collect_attempt(&mut self, tool_id: &str, now: Instant) {
        self.last_collect_attempt.insert(tool_id.to_string(), now);
    }

    /// 写入成功采集结果。
    pub(crate) fn upsert_success(&mut self, envelope: ToolDetailEnvelopePayload) {
        self.by_tool_id.insert(envelope.tool_id.clone(), envelope);
    }

    /// 将指定工具标记为 stale，优先保留缓存中的旧 data。
    pub(crate) fn mark_stale(
        &mut self,
        tool_id: &str,
        schema: &str,
        profile_key: Option<String>,
        error: &str,
        expires_at: Option<String>,
    ) {
        if let Some(existing) = self.by_tool_id.get_mut(tool_id) {
            existing.stale = true;
            existing.expires_at = expires_at;
            if existing.schema.trim().is_empty() {
                existing.schema = schema.to_string();
            }
            if existing.profile_key.is_none() && profile_key.is_some() {
                existing.profile_key = profile_key;
            }
            attach_collect_error(&mut existing.data, error);
            return;
        }

        self.by_tool_id.insert(
            tool_id.to_string(),
            ToolDetailEnvelopePayload {
                tool_id: tool_id.to_string(),
                schema: schema.to_string(),
                stale: true,
                collected_at: None,
                expires_at,
                profile_key,
                data: json!({ "collectError": error }),
            },
        );
    }

    /// 按给定工具顺序提取详情快照。
    pub(crate) fn snapshot_for_tool_order(
        &self,
        ordered_tool_ids: &[String],
    ) -> Vec<ToolDetailEnvelopePayload> {
        ordered_tool_ids
            .iter()
            .filter_map(|tool_id| self.by_tool_id.get(tool_id).cloned())
            .collect()
    }

    /// 删除不在活动工具列表中的缓存条目。
    pub(crate) fn prune_inactive(&mut self, active_tool_ids: &[String]) {
        let active: HashSet<&str> = active_tool_ids.iter().map(String::as_str).collect();
        self.by_tool_id
            .retain(|tool_id, _| active.contains(tool_id.as_str()));
        self.last_collect_attempt
            .retain(|tool_id, _| active.contains(tool_id.as_str()));
    }
}

/// 为 data 注入采集失败描述，便于前端提示但不暴露原始敏感数据。
fn attach_collect_error(data: &mut Value, error: &str) {
    if let Some(obj) = data.as_object_mut() {
        obj.insert("collectError".to_string(), Value::String(error.to_string()));
        let status_dots = obj
            .entry("statusDots".to_string())
            .or_insert_with(|| json!({}));
        if let Some(status_obj) = status_dots.as_object_mut() {
            status_obj.insert("data".to_string(), Value::String("stale".to_string()));
        } else {
            *status_dots = json!({ "data": "stale" });
        }
        return;
    }
    *data = json!({
        "collectError": error,
        "statusDots": {
            "data": "stale"
        }
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use yc_shared_protocol::ToolDetailEnvelopePayload;

    use super::ToolDetailsCache;

    #[test]
    fn mark_stale_preserves_previous_data() {
        let mut cache = ToolDetailsCache::new();
        cache.upsert_success(ToolDetailEnvelopePayload {
            tool_id: "tool_a".to_string(),
            schema: "openclaw.v1".to_string(),
            stale: false,
            collected_at: Some("2026-01-01T00:00:00Z".to_string()),
            expires_at: Some("2026-01-01T00:00:30Z".to_string()),
            profile_key: Some("default".to_string()),
            data: json!({"agents":[{"agentId":"a1"}]}),
        });

        cache.mark_stale(
            "tool_a",
            "openclaw.v1",
            Some("default".to_string()),
            "timeout",
            Some("2026-01-01T00:01:00Z".to_string()),
        );

        let snapshot = cache.snapshot_for_tool_order(&["tool_a".to_string()]);
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot[0].stale);
        assert_eq!(snapshot[0].schema, "openclaw.v1");
        assert_eq!(snapshot[0].data["agents"][0]["agentId"], "a1");
        assert_eq!(snapshot[0].data["collectError"], "timeout");
        assert_eq!(snapshot[0].data["statusDots"]["data"], "stale");
    }

    #[test]
    fn prune_inactive_removes_orphan_details() {
        let mut cache = ToolDetailsCache::new();
        cache.upsert_success(ToolDetailEnvelopePayload {
            tool_id: "tool_a".to_string(),
            schema: "opencode.v1".to_string(),
            stale: false,
            collected_at: None,
            expires_at: None,
            profile_key: None,
            data: json!({}),
        });
        cache.upsert_success(ToolDetailEnvelopePayload {
            tool_id: "tool_b".to_string(),
            schema: "openclaw.v1".to_string(),
            stale: false,
            collected_at: None,
            expires_at: None,
            profile_key: None,
            data: json!({}),
        });

        cache.prune_inactive(&["tool_b".to_string()]);
        let snapshot = cache.snapshot_for_tool_order(&["tool_a".to_string(), "tool_b".to_string()]);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].tool_id, "tool_b");
    }
}
