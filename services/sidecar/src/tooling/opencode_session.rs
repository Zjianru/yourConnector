//! OpenCode 会话解析与缓存。

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::Deserialize;
use yc_shared_protocol::{LatestTokensPayload, ModelUsagePayload};

use super::cli_parse::normalize_path;

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
struct OpenCodeSessionMeta {
    #[serde(default)]
    id: String,
    #[serde(rename = "projectID", default)]
    _project_id: String,
    #[serde(default)]
    directory: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    time: OpenCodeSessionTime,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenCodeSessionTime {
    #[serde(default)]
    updated: i64,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenCodeMessageMeta {
    #[serde(default)]
    role: String,
    #[serde(rename = "providerID", default)]
    provider_id: String,
    #[serde(rename = "modelID", default)]
    model_id: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    time: OpenCodeMessageTime,
    #[serde(default)]
    tokens: OpenCodeMessageTokens,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenCodeMessageTime {
    #[serde(default)]
    created: i64,
    #[serde(default)]
    completed: i64,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenCodeMessageTokens {
    #[serde(default)]
    total: i64,
    #[serde(default)]
    input: i64,
    #[serde(default)]
    output: i64,
    #[serde(default)]
    cache: OpenCodeTokenCache,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenCodeTokenCache {
    #[serde(default)]
    read: i64,
    #[serde(default)]
    write: i64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
struct DirSignature {
    file_count: u64,
    latest_mtime_ms: u128,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct OpenCodeStorageStamp {
    session_signature: DirSignature,
    session_id: String,
    session_updated: i64,
    message_signature: DirSignature,
}

#[derive(Debug, Clone)]
struct OpenCodeSessionCacheEntry {
    stamp: OpenCodeStorageStamp,
    state: OpenCodeSessionState,
}

#[derive(Debug, Default)]
struct OpenCodeSessionCache {
    by_cwd: HashMap<String, OpenCodeSessionCacheEntry>,
}

static OPENCODE_SESSION_CACHE: OnceLock<Mutex<OpenCodeSessionCache>> = OnceLock::new();

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

fn select_session_meta(
    session_files: &[PathBuf],
    normalized_cwd: &str,
) -> Option<OpenCodeSessionMeta> {
    let metas = session_files
        .iter()
        .filter_map(|path| read_json_file::<OpenCodeSessionMeta>(path))
        .filter(|meta| !meta.id.trim().is_empty())
        .collect::<Vec<OpenCodeSessionMeta>>();

    if metas.is_empty() {
        return None;
    }

    if !normalized_cwd.is_empty() {
        let matched = metas
            .iter()
            .filter(|meta| normalize_path(&meta.directory) == normalized_cwd)
            .max_by_key(|meta| meta.time.updated)
            .cloned();
        if matched.is_some() {
            return matched;
        }
    }

    metas.into_iter().max_by_key(|meta| meta.time.updated)
}

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
    let Ok(entries) = fs::read_dir(msg_dir) else {
        return state;
    };

    let mut usage_by_model: HashMap<String, ModelUsagePayload> = HashMap::new();
    let mut latest_message: Option<OpenCodeMessageMeta> = None;
    let mut latest_ts = 0_i64;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !has_json_ext(&path) {
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

fn opencode_cache_key(normalized_cwd: &str) -> String {
    if normalized_cwd.is_empty() {
        "__global__".to_string()
    } else {
        normalized_cwd.to_string()
    }
}

fn read_cached_opencode_state(
    cache_key: &str,
    stamp: &OpenCodeStorageStamp,
) -> Option<OpenCodeSessionState> {
    let cache = opencode_session_cache().lock().ok()?;
    let entry = cache.by_cwd.get(cache_key)?;
    if &entry.stamp != stamp {
        return None;
    }
    Some(entry.state.clone())
}

fn write_cached_opencode_state(
    cache_key: String,
    stamp: OpenCodeStorageStamp,
    state: OpenCodeSessionState,
) {
    let Ok(mut cache) = opencode_session_cache().lock() else {
        return;
    };

    if cache.by_cwd.len() >= 256 {
        cache.by_cwd.clear();
    }
    cache
        .by_cwd
        .insert(cache_key, OpenCodeSessionCacheEntry { stamp, state });
}

fn evict_cached_opencode_state(cache_key: &str) {
    let Ok(mut cache) = opencode_session_cache().lock() else {
        return;
    };
    cache.by_cwd.remove(cache_key);
}

fn opencode_session_cache() -> &'static Mutex<OpenCodeSessionCache> {
    OPENCODE_SESSION_CACHE.get_or_init(|| Mutex::new(OpenCodeSessionCache::default()))
}

fn files_signature(paths: &[PathBuf]) -> DirSignature {
    let mut signature = DirSignature::default();

    for path in paths {
        if !path.is_file() {
            continue;
        }
        signature.file_count += 1;
        signature.latest_mtime_ms = signature.latest_mtime_ms.max(path_mtime_ms(path));
    }

    signature
}

fn message_dir_signature(root: &Path, session_id: &str) -> DirSignature {
    let message_dir = root.join("message").join(session_id);
    dir_json_signature(&message_dir)
}

fn dir_json_signature(path: &Path) -> DirSignature {
    let Ok(entries) = fs::read_dir(path) else {
        return DirSignature::default();
    };

    let mut signature = DirSignature::default();
    for entry in entries.flatten() {
        let file_path = entry.path();
        if !file_path.is_file() || !has_json_ext(&file_path) {
            continue;
        }
        signature.file_count += 1;
        signature.latest_mtime_ms = signature.latest_mtime_ms.max(path_mtime_ms(&file_path));
    }
    signature
}

fn path_mtime_ms(path: &Path) -> u128 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|ts| ts.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|dur| dur.as_millis())
        .unwrap_or(0)
}

fn collect_session_meta_files(root: &Path) -> Vec<PathBuf> {
    let session_root = root.join("session");
    let Ok(project_dirs) = fs::read_dir(session_root) else {
        return Vec::new();
    };
    let mut files = Vec::new();

    for project_dir in project_dirs.flatten() {
        let project_path = project_dir.path();
        if !project_path.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(project_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !has_json_ext(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("ses_") {
                files.push(path);
            }
        }
    }

    files
}

fn has_json_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

fn read_json_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice::<T>(&data).ok()
}

fn opencode_storage_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        Path::new(&home)
            .join(".local")
            .join("share")
            .join("opencode")
            .join("storage"),
    )
}

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

fn format_unix_ms(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}
