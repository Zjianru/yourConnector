//! OpenCode 会话解析内部类型定义。

use std::collections::HashMap;

use serde::Deserialize;
use yc_shared_protocol::{LatestTokensPayload, ModelUsagePayload};

/// OpenCode 会话状态：用于上报到 tools/metrics 事件。
#[derive(Debug, Clone, Default)]
pub(crate) struct OpenCodeSessionState {
    pub(crate) session_id: String,
    pub(crate) session_title: String,
    pub(crate) session_updated_at: String,
    pub(crate) workspace_dir: String,
    pub(crate) agent_mode: String,
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
    pub(crate) model: String,
    pub(crate) latest_tokens: LatestTokensPayload,
    pub(crate) model_usage: Vec<ModelUsagePayload>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(super) struct OpenCodeSessionMeta {
    #[serde(default)]
    pub(super) id: String,
    #[serde(rename = "projectID", default)]
    pub(super) _project_id: String,
    #[serde(default)]
    pub(super) directory: String,
    #[serde(default)]
    pub(super) title: String,
    #[serde(default)]
    pub(super) time: OpenCodeSessionTime,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(super) struct OpenCodeSessionTime {
    #[serde(default)]
    pub(super) updated: i64,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(super) struct OpenCodeMessageMeta {
    #[serde(default)]
    pub(super) role: String,
    #[serde(rename = "providerID", default)]
    pub(super) provider_id: String,
    #[serde(rename = "modelID", default)]
    pub(super) model_id: String,
    #[serde(default)]
    pub(super) mode: String,
    #[serde(default)]
    pub(super) time: OpenCodeMessageTime,
    #[serde(default)]
    pub(super) tokens: OpenCodeMessageTokens,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(super) struct OpenCodeMessageTime {
    #[serde(default)]
    pub(super) created: i64,
    #[serde(default)]
    pub(super) completed: i64,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(super) struct OpenCodeMessageTokens {
    #[serde(default)]
    pub(super) total: i64,
    #[serde(default)]
    pub(super) input: i64,
    #[serde(default)]
    pub(super) output: i64,
    #[serde(default)]
    pub(super) cache: OpenCodeTokenCache,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(super) struct OpenCodeTokenCache {
    #[serde(default)]
    pub(super) read: i64,
    #[serde(default)]
    pub(super) write: i64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub(super) struct DirSignature {
    pub(super) file_count: u64,
    pub(super) latest_mtime_ms: u128,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct OpenCodeStorageStamp {
    pub(super) session_signature: DirSignature,
    pub(super) session_id: String,
    pub(super) session_updated: i64,
    pub(super) message_signature: DirSignature,
}

#[derive(Debug, Clone)]
pub(super) struct OpenCodeSessionCacheEntry {
    pub(super) stamp: OpenCodeStorageStamp,
    pub(super) state: OpenCodeSessionState,
}

#[derive(Debug, Default)]
pub(super) struct OpenCodeSessionCache {
    pub(super) by_cwd: HashMap<String, OpenCodeSessionCacheEntry>,
}
