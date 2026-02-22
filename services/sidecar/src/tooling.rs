// 文件职责：
// 1) 提供工具识别所需的通用解析函数（命令模式、参数、ID、数值转换）。
// 2) 解析 OpenCode 本地会话与消息，构建会话状态快照。
// 3) 维护 OpenCode 会话状态缓存，降低高频采样下的磁盘读取开销。

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::Deserialize;
use yc_shared_protocol::{LatestTokensPayload, ModelUsagePayload};

use crate::ProcInfo;

// OpenCode 会话状态：用于上报到 tools/metrics 事件。
#[derive(Debug, Clone, Default)]
pub(crate) struct OpenCodeSessionState {
    // 会话唯一 ID（来自 opencode storage/session）。
    pub(crate) session_id: String,
    // 会话标题。
    pub(crate) session_title: String,
    // 会话最近更新时间（RFC3339）。
    pub(crate) session_updated_at: String,
    // 当前会话工作目录。
    pub(crate) workspace_dir: String,
    // agent 模式（如 plan/build）。
    pub(crate) agent_mode: String,
    // 模型供应商 ID。
    pub(crate) provider_id: String,
    // 模型 ID。
    pub(crate) model_id: String,
    // 聚合后的模型名称（provider/model）。
    pub(crate) model: String,
    // 最近一次 assistant 消息的 token 快照。
    pub(crate) latest_tokens: LatestTokensPayload,
    // 当前会话内按模型聚合的用量统计。
    pub(crate) model_usage: Vec<ModelUsagePayload>,
}

// ===== OpenCode 本地存储反序列化结构 =====

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

// 目录签名：用来判断目录内容是否变化。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
struct DirSignature {
    // json 文件数量。
    file_count: u64,
    // 最新修改时间（毫秒时间戳）。
    latest_mtime_ms: u128,
}

// 缓存签名：同时覆盖会话元数据与选中会话的消息目录。
#[derive(Debug, Clone, Eq, PartialEq)]
struct OpenCodeStorageStamp {
    // session 元数据文件集合签名。
    session_signature: DirSignature,
    // 本次选中的会话 ID。
    session_id: String,
    // 本次选中的会话更新时间。
    session_updated: i64,
    // message/<session_id> 的文件签名。
    message_signature: DirSignature,
}

// 缓存条目：签名一致时直接返回缓存状态，避免重复解析消息 JSON。
#[derive(Debug, Clone)]
struct OpenCodeSessionCacheEntry {
    stamp: OpenCodeStorageStamp,
    state: OpenCodeSessionState,
}

// 按 cwd 缓存，保证同一主机上多工作区互不干扰。
#[derive(Debug, Default)]
struct OpenCodeSessionCache {
    by_cwd: HashMap<String, OpenCodeSessionCacheEntry>,
}

// 全局缓存容器：sidecar 进程内共享。
static OPENCODE_SESSION_CACHE: OnceLock<Mutex<OpenCodeSessionCache>> = OnceLock::new();

// 入口：根据 process cwd 解析 OpenCode 会话状态。
pub(crate) fn collect_opencode_session_state(process_cwd: &str) -> OpenCodeSessionState {
    // 归一化 cwd，保证缓存 key 稳定。
    let normalized_cwd = normalize_path(process_cwd);
    let cache_key = opencode_cache_key(&normalized_cwd);

    let Some(root) = opencode_storage_root() else {
        evict_cached_opencode_state(&cache_key);
        return OpenCodeSessionState::default();
    };

    // 只扫描 session 元数据文件（不解析 message），用于选会话和构建签名。
    let session_files = collect_session_meta_files(&root);
    if session_files.is_empty() {
        evict_cached_opencode_state(&cache_key);
        return OpenCodeSessionState::default();
    }

    // 先确定本次应使用的会话，再构造缓存签名。
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

    // 缓存失效后才会解析消息内容。
    let state = collect_opencode_session_state_for_session(&root, &selected_session);
    write_cached_opencode_state(cache_key, stamp, state.clone());
    state
}

// 从 session 元数据中选出“最匹配 cwd 的最新会话”；若无匹配则回退全局最新。
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

// 解析指定会话的 message 文件，构建可展示的会话状态。
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

    // 按模型聚合当前会话的 token 用量。
    let mut usage_by_model: HashMap<String, ModelUsagePayload> = HashMap::new();
    // 记录最近一条 assistant 消息，用于展示 mode/model/latest tokens。
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

// 缓存 key：空 cwd 统一归入全局键。
fn opencode_cache_key(normalized_cwd: &str) -> String {
    if normalized_cwd.is_empty() {
        "__global__".to_string()
    } else {
        normalized_cwd.to_string()
    }
}

// 缓存命中判断：签名完全一致才复用状态。
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

// 更新缓存：限制条目数，避免长期运行内存无界增长。
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

// 清理单个 key 的缓存，避免在无会话时复用旧状态。
fn evict_cached_opencode_state(cache_key: &str) {
    let Ok(mut cache) = opencode_session_cache().lock() else {
        return;
    };
    cache.by_cwd.remove(cache_key);
}

// 获取全局会话缓存容器（惰性初始化）。
fn opencode_session_cache() -> &'static Mutex<OpenCodeSessionCache> {
    OPENCODE_SESSION_CACHE.get_or_init(|| Mutex::new(OpenCodeSessionCache::default()))
}

// 计算 session 元数据文件集合签名。
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

// 计算 message/<session_id> 目录签名。
fn message_dir_signature(root: &Path, session_id: &str) -> DirSignature {
    let message_dir = root.join("message").join(session_id);
    dir_json_signature(&message_dir)
}

// 仅扫描目录下 json 文件的数量和最近修改时间，不解析文件内容。
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

// 读取文件修改时间并转为毫秒时间戳。
fn path_mtime_ms(path: &Path) -> u128 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|ts| ts.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|dur| dur.as_millis())
        .unwrap_or(0)
}

// 搜集所有 session 元数据文件：storage/session/*/ses_*.json。
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

// 判断路径是否以 `.json` 结尾。
fn has_json_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

// 读取并反序列化 JSON 文件；失败返回 None。
fn read_json_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice::<T>(&data).ok()
}

// OpenCode 本地存储根目录：`~/.local/share/opencode/storage`。
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

// 当工具会话目录缺失时，用进程 cwd 兜底。
pub(crate) fn first_non_empty(primary: &str, fallback: &str) -> String {
    if !primary.trim().is_empty() {
        return primary.to_string();
    }
    fallback.to_string()
}

// 将非空字符串包装为 Option，减少构造 payload 时的样板代码。
pub(crate) fn option_non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

// 组装模型展示名：优先 provider/model，缺失时退化为单字段。
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

// 将毫秒时间戳格式化为 RFC3339 字符串。
fn format_unix_ms(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

// 路径归一化：移除 ./ 与 ../ 影响，提升 ID 与匹配稳定性。
pub(crate) fn normalize_path(path: &str) -> String {
    if path.trim().is_empty() {
        return String::new();
    }
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            _ => normalized.push(component),
        }
    }
    normalized.to_string_lossy().to_string()
}

// 解析 serve 命令的 host/port 参数，兼容 --key value 和 --key=value 两种写法。
pub(crate) fn parse_serve_address(cmd: &str) -> (String, i32) {
    let mut host = "127.0.0.1".to_string();
    let mut port = 0_i32;
    let tokens = cmd.split_whitespace().collect::<Vec<&str>>();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token == "--hostname" && idx + 1 < tokens.len() {
            host = tokens[idx + 1].trim().to_string();
            idx += 2;
            continue;
        }
        if let Some(value) = token.strip_prefix("--hostname=") {
            host = value.trim().to_string();
            idx += 1;
            continue;
        }
        if token == "--port" && idx + 1 < tokens.len() {
            if let Ok(value) = tokens[idx + 1].trim().parse::<i32>() {
                port = value;
            }
            idx += 2;
            continue;
        }
        if let Some(value) = token.strip_prefix("--port=")
            && let Ok(parsed) = value.trim().parse::<i32>()
        {
            port = parsed;
        }
        idx += 1;
    }
    (host, port)
}

// 通用 CLI 参数读取函数。
pub(crate) fn parse_cli_flag_value(cmd: &str, flag: &str) -> Option<String> {
    let tokens = cmd.split_whitespace().collect::<Vec<&str>>();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token == flag {
            if idx + 1 < tokens.len() {
                let value = tokens[idx + 1].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
            idx += 2;
            continue;
        }
        let prefix = format!("{flag}=");
        if let Some(value) = token.strip_prefix(&prefix) {
            let cleaned = value.trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
        idx += 1;
    }
    None
}

// 判断命令行中是否包含独立命令词，避免子串误判。
fn contains_command_word(cmd_lower: &str, word: &str) -> bool {
    cmd_lower == word
        || cmd_lower.starts_with(&format!("{word} "))
        || cmd_lower.contains(&format!(" {word} "))
        || cmd_lower.ends_with(&format!("/{word}"))
        || cmd_lower.contains(&format!("/{word} "))
}

// 判断是否是可接入的 opencode 运行命令。
pub(crate) fn is_opencode_candidate_command(cmd_lower: &str) -> bool {
    if !contains_command_word(cmd_lower, "opencode") {
        return false;
    }
    if cmd_lower.contains("--help")
        || cmd_lower.contains("--version")
        || cmd_lower.contains(" opencode debug")
        || cmd_lower.contains(" opencode completion")
    {
        return false;
    }
    true
}

// 判断是否是 opencode 的 wrapper 进程命令。
pub(crate) fn is_opencode_wrapper_command(cmd_lower: &str) -> bool {
    // 只要是可接入 opencode 命令，且不是已知 runtime 二进制路径，就可视为 wrapper。
    // 这样可兼容 Homebrew/npm/裸命令等多种启动形态，避免候选工具漏检。
    is_opencode_candidate_command(cmd_lower)
        && !cmd_lower.contains("opencode-darwin-arm64/bin/opencode")
}

// 判断是否是可接入的 openclaw 命令。
pub(crate) fn is_openclaw_candidate_command(cmd_lower: &str) -> bool {
    if !contains_command_word(cmd_lower, "openclaw") {
        return false;
    }
    if cmd_lower.contains("--help")
        || cmd_lower.contains("--version")
        || cmd_lower.contains(" openclaw debug")
        || cmd_lower.contains(" openclaw completion")
    {
        return false;
    }
    true
}

// 统一探测主机地址：0.0.0.0/:: 对外展示为本机可访问地址。
pub(crate) fn normalize_probe_host(host: &str) -> String {
    match host.trim() {
        "" | "0.0.0.0" | "::" => "127.0.0.1".to_string(),
        raw => raw.to_string(),
    }
}

// 在 wrapper + children 中优先挑选真实 runtime 进程 pid。
pub(crate) fn pick_runtime_pid(
    wrapper_pid: i32,
    candidate_pids: &[i32],
    all: &HashMap<i32, ProcInfo>,
) -> i32 {
    for pid in candidate_pids {
        let Some(info) = all.get(pid) else {
            continue;
        };
        if info
            .cmd
            .to_lowercase()
            .contains("opencode-darwin-arm64/bin/opencode")
        {
            return *pid;
        }
    }
    wrapper_pid
}

// 基于运行模式和会话状态判断连接状态与提示。
pub(crate) fn evaluate_opencode_connection(
    mode: &str,
    state: &OpenCodeSessionState,
) -> (bool, &'static str, String) {
    if mode == "SERVE" {
        return (
            false,
            "UNSUPPORTED_MODE",
            "当前策略只支持通过 opencode 命令运行的会话，不支持 opencode serve。".to_string(),
        );
    }
    if state.session_id.is_empty() {
        return (
            true,
            "RUNNING",
            "已接入 opencode 进程，等待会话消息后补充模式和模型信息。".to_string(),
        );
    }
    (true, "RUNNING", String::new())
}

// 根据命令行特征判断 OpenCode 当前模式。
pub(crate) fn detect_opencode_mode(cmd: &str) -> &'static str {
    if cmd.contains("opencode serve") || cmd.contains("opencode web") {
        return "SERVE";
    }
    "TUI"
}

// 根据命令行特征判断 OpenClaw 当前模式。
pub(crate) fn detect_openclaw_mode(cmd: &str) -> &'static str {
    if cmd.contains("openclaw serve") || cmd.contains("openclaw web") {
        return "SERVE";
    }
    "CLI"
}

// 依据 workspace 生成稳定 opencode 工具 ID。
pub(crate) fn build_opencode_tool_id(workspace: &str, fallback_pid: i32) -> String {
    let normalized = normalize_path(workspace);
    if normalized.trim().is_empty() {
        return format!("opencode_{fallback_pid}");
    }
    let hex = format!("{:016x}", fnv1a64(normalized.as_bytes()));
    format!("opencode_{}", &hex[..12])
}

// 依据 workspace 或命令内容生成稳定 openclaw 工具 ID。
pub(crate) fn build_openclaw_tool_id(workspace: &str, cmd: &str, fallback_pid: i32) -> String {
    let normalized_workspace = normalize_path(workspace);
    if !normalized_workspace.trim().is_empty() {
        let hex = format!("{:016x}", fnv1a64(normalized_workspace.as_bytes()));
        return format!("openclaw_{}", &hex[..12]);
    }

    let normalized_cmd = cmd.trim().to_ascii_lowercase();
    if !normalized_cmd.is_empty() {
        let hex = format!("{:016x}", fnv1a64(normalized_cmd.as_bytes()));
        return format!("openclaw_{}", &hex[..12]);
    }

    format!("openclaw_{fallback_pid}")
}

// 将字节数换算为 MB。
pub(crate) fn bytes_to_mb(v: u64) -> f64 {
    v as f64 / 1024.0 / 1024.0
}

// 将字节数换算为 GB。
pub(crate) fn bytes_to_gb(v: u64) -> f64 {
    v as f64 / 1024.0 / 1024.0 / 1024.0
}

// 四舍五入保留两位小数，用于前端展示。
pub(crate) fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// FNV-1a 64 位哈希，用于稳定生成 toolId。
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}
