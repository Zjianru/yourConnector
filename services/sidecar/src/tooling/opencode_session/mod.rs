//! OpenCode 会话解析与缓存。

mod cache;
mod fs;
mod types;

use std::{collections::HashMap, path::Path};

pub(crate) use types::OpenCodeSessionState;

use yc_shared_protocol::{LatestTokensPayload, ModelUsagePayload};

use crate::tooling::normalize_path;

use self::{
    cache::{
        evict_cached_opencode_state, opencode_cache_key, read_cached_opencode_state,
        write_cached_opencode_state,
    },
    fs::{
        collect_session_meta_files, files_signature, message_dir_signature, opencode_storage_root,
        read_json_file, select_session_meta,
    },
    types::{OpenCodeMessageMeta, OpenCodeSessionMeta, OpenCodeStorageStamp},
};

/// 入口：根据 process cwd 解析 OpenCode 会话状态。
pub(crate) fn collect_opencode_session_state(process_cwd: &str) -> OpenCodeSessionState {
    let normalized_cwd = normalize_path(process_cwd);
    let cache_key = opencode_cache_key(&normalized_cwd);

    let Some(root) = opencode_storage_root() else {
        evict_cached_opencode_state(&cache_key);
        return OpenCodeSessionState::default();
    };

    let session_files = collect_session_meta_files(&root);
    if session_files.is_empty() {
        evict_cached_opencode_state(&cache_key);
        return OpenCodeSessionState::default();
    }

    let Some(selected_session) = select_session_meta(&session_files, &normalized_cwd) else {
        evict_cached_opencode_state(&cache_key);
        return OpenCodeSessionState::default();
    };

    let stamp = OpenCodeStorageStamp {
        session_signature: files_signature(&session_files),
        session_id: selected_session.id.clone(),
        session_updated: selected_session.time.updated,
        message_signature: message_dir_signature(&root, &selected_session.id),
    };

    if let Some(state) = read_cached_opencode_state(&cache_key, &stamp) {
        return state;
    }

    let state = collect_opencode_session_state_for_session(&root, &selected_session);
    write_cached_opencode_state(cache_key, stamp, state.clone());
    state
}

/// 收集指定 session 的模型、模式与 token 用量。
fn collect_opencode_session_state_for_session(
    root: &Path,
    session: &OpenCodeSessionMeta,
) -> OpenCodeSessionState {
    let mut state = OpenCodeSessionState {
        session_id: session.id.clone(),
        session_title: session.title.clone(),
        session_updated_at: format_unix_ms(session.time.updated),
        workspace_dir: session.directory.clone(),
        ..OpenCodeSessionState::default()
    };

    let msg_dir = root.join("message").join(&session.id);
    let Ok(entries) = std::fs::read_dir(msg_dir) else {
        return state;
    };

    let mut usage_by_model: HashMap<String, ModelUsagePayload> = HashMap::new();
    let mut latest_message: Option<OpenCodeMessageMeta> = None;
    let mut latest_ts = 0_i64;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !path_has_json_ext(&path) {
            continue;
        }
        let Some(msg) = read_json_file::<OpenCodeMessageMeta>(&path) else {
            continue;
        };
        if msg.role != "assistant" {
            continue;
        }

        let model_key = {
            let value = build_model_name(&msg.provider_id, &msg.model_id);
            if value.is_empty() {
                "unknown".to_string()
            } else {
                value
            }
        };
        let row = usage_by_model
            .entry(model_key.clone())
            .or_insert_with(|| ModelUsagePayload {
                model: model_key,
                ..ModelUsagePayload::default()
            });

        row.messages += 1;
        row.token_total += msg.tokens.total;
        row.token_input += msg.tokens.input;
        row.token_output += msg.tokens.output;
        row.cache_read += msg.tokens.cache.read;
        row.cache_write += msg.tokens.cache.write;

        let ts = if msg.time.completed > 0 {
            msg.time.completed
        } else {
            msg.time.created
        };
        if latest_message.is_none() || ts > latest_ts {
            latest_ts = ts;
            latest_message = Some(msg);
        }
    }

    if let Some(latest) = latest_message {
        state.agent_mode = latest.mode.clone();
        state.provider_id = latest.provider_id.clone();
        state.model_id = latest.model_id.clone();
        state.model = build_model_name(&latest.provider_id, &latest.model_id);
        state.latest_tokens = LatestTokensPayload {
            total: latest.tokens.total,
            input: latest.tokens.input,
            output: latest.tokens.output,
            cache_read: latest.tokens.cache.read,
            cache_write: latest.tokens.cache.write,
        };
    }

    let mut rows = usage_by_model
        .into_values()
        .collect::<Vec<ModelUsagePayload>>();
    rows.sort_by(|a, b| {
        b.token_total
            .cmp(&a.token_total)
            .then_with(|| b.messages.cmp(&a.messages))
    });
    if rows.len() > 3 {
        rows.truncate(3);
    }
    state.model_usage = rows;
    state
}

/// 组合 provider/model 的展示名。
fn build_model_name(provider_id: &str, model_id: &str) -> String {
    let p = provider_id.trim();
    let m = model_id.trim();
    if !p.is_empty() && !m.is_empty() {
        return format!("{p}/{m}");
    }
    if !m.is_empty() {
        return m.to_string();
    }
    if !p.is_empty() {
        return p.to_string();
    }
    String::new()
}

/// 将 unix ms 格式化为 RFC3339 字符串。
fn format_unix_ms(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// 判断路径扩展名是否为 JSON。
fn path_has_json_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}
