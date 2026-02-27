//! 报告拉取执行器：
//! 1. 维护会话级单活跃报告读取任务。
//! 2. 校验文件路径安全边界（仅 workspace 内绝对 .md）。
//! 3. 按分片发送 started/chunk/finished 事件。

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tokio::{
    fs,
    io::AsyncReadExt,
    sync::{mpsc, watch},
};
use tracing::debug;
use yc_shared_protocol::ToolRuntimePayload;

use crate::control::{
    TOOL_REPORT_FETCH_CHUNK_EVENT, TOOL_REPORT_FETCH_FINISHED_EVENT,
    TOOL_REPORT_FETCH_STARTED_EVENT,
};

const REPORT_ALLOWED_ROOTS_ENV: &str = "YC_REPORT_ALLOWED_ROOTS";

/// 报告事件发送通道。
pub(crate) type ReportEventSender = mpsc::UnboundedSender<ReportEventEnvelope>;

/// 报告事件封装（由 run_session 主循环统一转发到 relay）。
#[derive(Debug, Clone)]
pub(crate) struct ReportEventEnvelope {
    /// 事件名（tool_report_fetch_started/chunk/finished）。
    pub(crate) event_type: &'static str,
    /// traceId（可选）。
    pub(crate) trace_id: Option<String>,
    /// 事件 payload。
    pub(crate) payload: Value,
    /// 结束事件时用于清理 active map 的键。
    pub(crate) finalize: Option<ReportFinalizeKey>,
}

/// 活跃任务清理键。
#[derive(Debug, Clone)]
pub(crate) struct ReportFinalizeKey {
    /// 会话键（hostId::toolId）。
    pub(crate) conversation_key: String,
    /// 请求 ID。
    pub(crate) request_id: String,
}

/// 单次报告拉取请求参数。
#[derive(Debug, Clone)]
pub(crate) struct ReportRequestInput {
    pub(crate) tool_id: String,
    pub(crate) conversation_key: String,
    pub(crate) request_id: String,
    pub(crate) file_path: String,
}

/// 发起报告拉取返回结果。
#[derive(Debug, Clone)]
pub(crate) enum StartReportOutcome {
    Started,
    Busy { reason: String },
}

/// 运行中的报告任务元数据。
#[derive(Debug)]
struct ActiveReportTask {
    request_id: String,
    cancel_tx: watch::Sender<bool>,
}

/// 会话级报告运行时。
#[derive(Debug, Default)]
pub(crate) struct ReportRuntime {
    active_by_conversation: HashMap<String, ActiveReportTask>,
}

impl ReportRuntime {
    /// 尝试在指定会话启动报告读取任务；若会话忙，返回 busy。
    pub(crate) fn start_request(
        &mut self,
        request: ReportRequestInput,
        tool: ToolRuntimePayload,
        trace_id: Option<String>,
        event_tx: ReportEventSender,
    ) -> StartReportOutcome {
        if let Some(active) = self.active_by_conversation.get(&request.conversation_key) {
            return StartReportOutcome::Busy {
                reason: format!("会话中已有进行中的报告请求：{}", active.request_id),
            };
        }

        let (cancel_tx, cancel_rx) = watch::channel(false);
        self.active_by_conversation.insert(
            request.conversation_key.clone(),
            ActiveReportTask {
                request_id: request.request_id.clone(),
                cancel_tx,
            },
        );

        tokio::spawn(run_report_task(
            request, tool, trace_id, event_tx, cancel_rx,
        ));
        StartReportOutcome::Started
    }

    /// 收到 finished 事件后释放会话占用。
    pub(crate) fn mark_finished(&mut self, key: &ReportFinalizeKey) {
        let should_remove = self
            .active_by_conversation
            .get(&key.conversation_key)
            .map(|active| active.request_id == key.request_id)
            .unwrap_or(false);
        if should_remove {
            self.active_by_conversation.remove(&key.conversation_key);
        }
    }

    /// 会话循环结束时取消全部任务。
    pub(crate) fn abort_all(&mut self) {
        let all_keys = self
            .active_by_conversation
            .keys()
            .cloned()
            .collect::<Vec<String>>();
        for key in all_keys {
            if let Some(active) = self.active_by_conversation.remove(&key) {
                let _ = active.cancel_tx.send(true);
            }
        }
    }
}

#[derive(Debug)]
enum ReportExecError {
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
struct ReportExecutionResult {
    bytes_sent: u64,
    bytes_total: u64,
}

#[derive(Debug)]
struct ValidatedPath {
    path: PathBuf,
    bytes_total: u64,
}

const REPORT_CHUNK_SIZE: usize = 16 * 1024;

/// 任务入口：发送 started/chunk -> 发送 finished。
async fn run_report_task(
    request: ReportRequestInput,
    tool: ToolRuntimePayload,
    trace_id: Option<String>,
    event_tx: ReportEventSender,
    mut cancel_rx: watch::Receiver<bool>,
) {
    let result =
        execute_report_request(&request, &tool, &trace_id, &event_tx, &mut cancel_rx).await;

    match result {
        Ok(done) => emit_finished(
            &event_tx,
            trace_id,
            &request,
            "completed",
            "",
            done.bytes_sent,
            done.bytes_total,
        ),
        Err(ReportExecError::Cancelled) => {
            emit_finished(&event_tx, trace_id, &request, "failed", "请求已取消", 0, 0)
        }
        Err(ReportExecError::Failed(reason)) => {
            emit_finished(&event_tx, trace_id, &request, "failed", &reason, 0, 0)
        }
    }
}

/// 读取并按分片发送报告内容。
async fn execute_report_request(
    request: &ReportRequestInput,
    tool: &ToolRuntimePayload,
    trace_id: &Option<String>,
    event_tx: &ReportEventSender,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<ReportExecutionResult, ReportExecError> {
    if cancelled(cancel_rx) {
        return Err(ReportExecError::Cancelled);
    }

    let validated = validate_report_path(tool, &request.file_path)?;
    emit_started(event_tx, trace_id.clone(), request, validated.bytes_total);

    let mut file = fs::File::open(&validated.path)
        .await
        .map_err(|err| ReportExecError::Failed(format!("打开报告文件失败: {err}")))?;
    let mut buffer = vec![0_u8; REPORT_CHUNK_SIZE];
    let mut bytes_sent = 0_u64;
    let bytes_total = validated.bytes_total;
    let mut chunk_index = 0_u64;
    let mut utf8_carry = Vec::<u8>::new();

    loop {
        tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && cancelled(cancel_rx) {
                    return Err(ReportExecError::Cancelled);
                }
            }
            read = file.read(&mut buffer) => {
                let read = read
                    .map_err(|err| ReportExecError::Failed(format!("读取报告文件失败: {err}")))?;
                if read == 0 {
                    break;
                }
                bytes_sent = bytes_sent.saturating_add(read as u64);
                utf8_carry.extend_from_slice(&buffer[..read]);
                loop {
                    match std::str::from_utf8(&utf8_carry) {
                        Ok(text) => {
                            if !text.is_empty() {
                                emit_chunk(
                                    event_tx,
                                    trace_id.clone(),
                                    request,
                                    text,
                                    bytes_sent,
                                    bytes_total,
                                    chunk_index,
                                );
                                chunk_index = chunk_index.saturating_add(1);
                            }
                            utf8_carry.clear();
                            break;
                        }
                        Err(err) => {
                            let valid_up_to = err.valid_up_to();
                            if valid_up_to > 0 {
                                let valid = std::str::from_utf8(&utf8_carry[..valid_up_to])
                                    .map_err(|_| ReportExecError::Failed("报告文件编码异常（UTF-8）".to_string()))?;
                                emit_chunk(
                                    event_tx,
                                    trace_id.clone(),
                                    request,
                                    valid,
                                    bytes_sent,
                                    bytes_total,
                                    chunk_index,
                                );
                                chunk_index = chunk_index.saturating_add(1);
                                let remainder = utf8_carry.split_off(valid_up_to);
                                utf8_carry = remainder;
                                continue;
                            }
                            if err.error_len().is_none() {
                                // 结尾 UTF-8 序列不完整，等待下一次读入拼接。
                                break;
                            }
                            return Err(ReportExecError::Failed(
                                "报告文件不是有效 UTF-8 文本".to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }

    if !utf8_carry.is_empty() {
        let tail = std::str::from_utf8(&utf8_carry)
            .map_err(|_| ReportExecError::Failed("报告文件不是有效 UTF-8 文本".to_string()))?;
        if !tail.is_empty() {
            emit_chunk(
                event_tx,
                trace_id.clone(),
                request,
                tail,
                bytes_sent,
                bytes_total,
                chunk_index,
            );
        }
    }

    Ok(ReportExecutionResult {
        bytes_sent,
        bytes_total,
    })
}

fn validate_report_path(
    tool: &ToolRuntimePayload,
    file_path: &str,
) -> Result<ValidatedPath, ReportExecError> {
    let requested = normalize_report_request_path(file_path);
    if !requested.is_absolute() {
        return Err(ReportExecError::Failed(
            "报告路径必须为绝对路径。".to_string(),
        ));
    }
    if !is_markdown_file_path(&requested) {
        return Err(ReportExecError::Failed(
            "仅支持读取 .md 报告文件。".to_string(),
        ));
    }

    let canonical_file = std::fs::canonicalize(&requested)
        .map_err(|err| ReportExecError::Failed(format!("报告文件不存在或不可访问: {err}")))?;
    if is_sensitive_rule_markdown_path(&canonical_file) {
        return Err(ReportExecError::Failed(
            "该 Markdown 疑似系统规则文件，已禁止通过报告预览读取。".to_string(),
        ));
    }
    let allowed_roots = resolve_report_allowed_roots(tool)?;
    if !allowed_roots
        .iter()
        .any(|root| canonical_file.starts_with(root))
    {
        return Err(ReportExecError::Failed(
            "仅允许读取当前工具工作区、OpenClaw 产出目录或已配置白名单目录内的报告文件。"
                .to_string(),
        ));
    }
    let metadata = std::fs::metadata(&canonical_file)
        .map_err(|err| ReportExecError::Failed(format!("读取报告文件元数据失败: {err}")))?;
    if !metadata.is_file() {
        return Err(ReportExecError::Failed(
            "目标路径不是文件，无法读取报告。".to_string(),
        ));
    }

    Ok(ValidatedPath {
        path: canonical_file,
        bytes_total: metadata.len(),
    })
}

fn resolve_report_allowed_roots(tool: &ToolRuntimePayload) -> Result<Vec<PathBuf>, ReportExecError> {
    let mut candidates = Vec::<PathBuf>::new();
    if let Some(workspace) = tool
        .workspace_dir
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        candidates.push(normalize_report_request_path(workspace));
    }
    candidates.extend(parse_extra_allowed_roots_from_env());
    candidates.extend(derive_openclaw_allowed_roots(tool));

    let mut allowed_roots = Vec::<PathBuf>::new();
    for root in candidates {
        let canonical = match std::fs::canonicalize(&root) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !canonical.is_dir() {
            continue;
        }
        if allowed_roots.iter().any(|item| *item == canonical) {
            continue;
        }
        allowed_roots.push(canonical);
    }

    if allowed_roots.is_empty() {
        return Err(ReportExecError::Failed(
            "工具可访问目录不可用，无法读取报告。".to_string(),
        ));
    }
    Ok(allowed_roots)
}

fn parse_extra_allowed_roots_from_env() -> Vec<PathBuf> {
    let Some(raw) = env::var_os(REPORT_ALLOWED_ROOTS_ENV) else {
        return Vec::new();
    };
    env::split_paths(&raw)
        .map(|value| normalize_report_request_path(value.to_string_lossy().as_ref()))
        .filter(|value| !value.as_os_str().is_empty())
        .collect()
}

fn derive_openclaw_allowed_roots(tool: &ToolRuntimePayload) -> Vec<PathBuf> {
    if !is_openclaw_tool(tool) {
        return Vec::new();
    }
    let profile = parse_openclaw_profile_key_from_source(tool.source.as_deref());
    let Some(state_dir) = resolve_openclaw_state_dir(profile.as_str()) else {
        return Vec::new();
    };
    vec![state_dir.join("agents")]
}

fn is_openclaw_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_ascii_lowercase();
    let name = tool.name.to_ascii_lowercase();
    let vendor = tool.vendor.to_ascii_lowercase();
    tool_id.starts_with("openclaw_") || name.contains("openclaw") || vendor.contains("openclaw")
}

fn parse_openclaw_profile_key_from_source(source: Option<&str>) -> String {
    let raw = source.unwrap_or_default();
    let marker = "profile=";
    if let Some(pos) = raw.find(marker) {
        let value = raw[(pos + marker.len())..]
            .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';')
            .next()
            .unwrap_or_default()
            .trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    "default".to_string()
}

fn resolve_openclaw_state_dir(profile_key: &str) -> Option<PathBuf> {
    let home = env::var("HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let home = PathBuf::from(home);
    let normalized = profile_key.trim();
    let path = match normalized {
        "dev" => home.join(".openclaw-dev"),
        "" | "default" => home.join(".openclaw"),
        value => home.join(format!(".openclaw-{value}")),
    };
    Some(path)
}

fn normalize_report_request_path(file_path: &str) -> PathBuf {
    let trimmed = file_path.trim();
    if let Some(relative) = trimmed.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home).join(relative);
    }
    PathBuf::from(trimmed)
}

fn is_markdown_file_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn is_sensitive_rule_markdown_path(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let sensitive = matches!(
        file_name.as_str(),
        "agents.md"
            | "tools.md"
            | "identity.md"
            | "user.md"
            | "heartbeat.md"
            | "bootstrap.md"
            | "memory.md"
            | "soul.md"
    );
    if !sensitive {
        return false;
    }
    let lowered = path.to_string_lossy().to_ascii_lowercase();
    !(lowered.contains("/output/") || lowered.contains("/reports/") || lowered.contains("/report/"))
}

fn cancelled(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

fn emit_started(
    event_tx: &ReportEventSender,
    trace_id: Option<String>,
    request: &ReportRequestInput,
    bytes_total: u64,
) {
    emit_report_event(
        event_tx,
        ReportEventEnvelope {
            event_type: TOOL_REPORT_FETCH_STARTED_EVENT,
            trace_id,
            payload: json!({
                "toolId": request.tool_id,
                "conversationKey": request.conversation_key,
                "requestId": request.request_id,
                "filePath": request.file_path,
                "bytesTotal": bytes_total,
            }),
            finalize: None,
        },
    );
}

fn emit_chunk(
    event_tx: &ReportEventSender,
    trace_id: Option<String>,
    request: &ReportRequestInput,
    chunk: &str,
    bytes_sent: u64,
    bytes_total: u64,
    chunk_index: u64,
) {
    emit_report_event(
        event_tx,
        ReportEventEnvelope {
            event_type: TOOL_REPORT_FETCH_CHUNK_EVENT,
            trace_id,
            payload: json!({
                "toolId": request.tool_id,
                "conversationKey": request.conversation_key,
                "requestId": request.request_id,
                "filePath": request.file_path,
                "chunk": chunk,
                "bytesSent": bytes_sent,
                "bytesTotal": bytes_total,
                "chunkIndex": chunk_index,
            }),
            finalize: None,
        },
    );
}

fn emit_finished(
    event_tx: &ReportEventSender,
    trace_id: Option<String>,
    request: &ReportRequestInput,
    status: &str,
    reason: &str,
    bytes_sent: u64,
    bytes_total: u64,
) {
    emit_report_event(
        event_tx,
        ReportEventEnvelope {
            event_type: TOOL_REPORT_FETCH_FINISHED_EVENT,
            trace_id,
            payload: json!({
                "toolId": request.tool_id,
                "conversationKey": request.conversation_key,
                "requestId": request.request_id,
                "filePath": request.file_path,
                "status": status,
                "reason": reason,
                "bytesSent": bytes_sent,
                "bytesTotal": bytes_total,
            }),
            finalize: Some(ReportFinalizeKey {
                conversation_key: request.conversation_key.clone(),
                request_id: request.request_id.clone(),
            }),
        },
    );
}

fn emit_report_event(event_tx: &ReportEventSender, event: ReportEventEnvelope) {
    if event_tx.send(event).is_err() {
        debug!("report event channel closed, dropping event");
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use yc_shared_protocol::ToolRuntimePayload;

    use super::{
        ReportExecError, is_markdown_file_path, parse_openclaw_profile_key_from_source,
        validate_report_path,
    };

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "yc_sidecar_report_test_{prefix}_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn make_tool_with_workspace(workspace: &Path) -> ToolRuntimePayload {
        ToolRuntimePayload {
            tool_id: "tool_test".to_string(),
            workspace_dir: Some(workspace.to_string_lossy().to_string()),
            ..ToolRuntimePayload::default()
        }
    }

    #[test]
    fn validate_report_path_accepts_workspace_markdown_file() {
        let workspace = make_temp_dir("accept");
        let file_path = workspace.join("report.md");
        std::fs::write(&file_path, "# Report\nhello").expect("write report");

        let tool = make_tool_with_workspace(&workspace);
        let validated =
            validate_report_path(&tool, file_path.to_string_lossy().as_ref()).expect("valid path");
        assert_eq!(
            validated.path,
            std::fs::canonicalize(&file_path).expect("canonical file path")
        );
        assert!(validated.bytes_total > 0);

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn validate_report_path_rejects_non_markdown_file() {
        let workspace = make_temp_dir("non_md");
        let file_path = workspace.join("report.txt");
        std::fs::write(&file_path, "plain text").expect("write report");

        let tool = make_tool_with_workspace(&workspace);
        let result = validate_report_path(&tool, file_path.to_string_lossy().as_ref());
        assert!(matches!(
            result,
            Err(ReportExecError::Failed(reason)) if reason.contains(".md")
        ));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn validate_report_path_rejects_path_outside_workspace() {
        let workspace = make_temp_dir("workspace");
        let outside_root = make_temp_dir("outside");
        let file_path = outside_root.join("report.md");
        std::fs::write(&file_path, "# external").expect("write report");

        let tool = make_tool_with_workspace(&workspace);
        let result = validate_report_path(&tool, file_path.to_string_lossy().as_ref());
        assert!(matches!(
            result,
            Err(ReportExecError::Failed(reason)) if reason.contains("目录内")
        ));

        let _ = std::fs::remove_dir_all(workspace);
        let _ = std::fs::remove_dir_all(outside_root);
    }

    #[test]
    fn validate_report_path_accepts_openclaw_agents_directory() {
        let workspace = make_temp_dir("workspace_openclaw");
        let home = std::env::var("HOME").expect("HOME should be set");
        let marker = format!("yc_sidecar_report_agent_{}", uuid::Uuid::new_v4());
        let agent_root = PathBuf::from(home)
            .join(".openclaw")
            .join("agents")
            .join(marker);
        let file_path = agent_root.join("workspace").join("output").join("report.md");
        std::fs::create_dir_all(
            file_path
                .parent()
                .expect("report path should include parent directory"),
        )
        .expect("create report dir");
        std::fs::write(&file_path, "# OpenClaw report").expect("write report");

        let mut tool = make_tool_with_workspace(&workspace);
        tool.tool_id = "openclaw_test".to_string();
        tool.vendor = "OpenClaw".to_string();
        tool.source = Some("openclaw-process-probe:profile=default".to_string());

        let validated =
            validate_report_path(&tool, file_path.to_string_lossy().as_ref()).expect("valid path");
        assert_eq!(
            validated.path,
            std::fs::canonicalize(&file_path).expect("canonical file path")
        );

        let _ = std::fs::remove_dir_all(workspace);
        let _ = std::fs::remove_dir_all(agent_root);
    }

    #[test]
    fn validate_report_path_rejects_sensitive_rule_file_in_workspace() {
        let workspace = make_temp_dir("sensitive_rule");
        let file_path = workspace.join("AGENTS.md");
        std::fs::write(&file_path, "# rule").expect("write rule file");

        let tool = make_tool_with_workspace(&workspace);
        let result = validate_report_path(&tool, file_path.to_string_lossy().as_ref());
        assert!(matches!(
            result,
            Err(ReportExecError::Failed(reason)) if reason.contains("系统规则文件")
        ));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn validate_report_path_rejects_relative_path() {
        let workspace = make_temp_dir("relative");
        let tool = make_tool_with_workspace(&workspace);
        let result = validate_report_path(&tool, "report.md");
        assert!(matches!(
            result,
            Err(ReportExecError::Failed(reason)) if reason.contains("绝对路径")
        ));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn markdown_extension_match_is_case_insensitive() {
        assert!(is_markdown_file_path(&PathBuf::from("/tmp/a.md")));
        assert!(is_markdown_file_path(&PathBuf::from("/tmp/a.MD")));
        assert!(!is_markdown_file_path(&PathBuf::from("/tmp/a.txt")));
    }

    #[test]
    fn parse_openclaw_profile_key_falls_back_to_default() {
        assert_eq!(
            parse_openclaw_profile_key_from_source(Some("openclaw-process-probe:profile=team")),
            "team"
        );
        assert_eq!(parse_openclaw_profile_key_from_source(None), "default");
        assert_eq!(
            parse_openclaw_profile_key_from_source(Some("openclaw-process-probe")),
            "default"
        );
    }
}
