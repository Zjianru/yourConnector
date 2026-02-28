//! 聊天执行器：
//! 1. 维护单会话（conversationKey）单活跃任务。
//! 2. 按工具类型执行 OpenCode/OpenClaw 命令并转为统一事件。
//! 3. 支持取消运行中任务并在完成后释放会话占用。

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, watch},
    time::{Instant, sleep, timeout},
};
use tracing::{debug, warn};
use uuid::Uuid;
use yc_shared_protocol::ToolRuntimePayload;

use crate::control::{
    ChatContentPart, TOOL_CHAT_CHUNK_EVENT, TOOL_CHAT_FINISHED_EVENT, TOOL_CHAT_STARTED_EVENT,
};

/// 聊天事件发送通道。
pub(crate) type ChatEventSender = mpsc::UnboundedSender<ChatEventEnvelope>;

/// 聊天事件封装（由 run_session 主循环统一转发到 relay）。
#[derive(Debug, Clone)]
pub(crate) struct ChatEventEnvelope {
    /// 事件名（tool_chat_started/chunk/finished）。
    pub(crate) event_type: &'static str,
    /// traceId（可选）。
    pub(crate) trace_id: Option<String>,
    /// 事件 payload。
    pub(crate) payload: Value,
    /// 结束事件时用于清理 active map 的键。
    pub(crate) finalize: Option<ChatFinalizeKey>,
}

/// 活跃会话清理键。
#[derive(Debug, Clone)]
pub(crate) struct ChatFinalizeKey {
    /// 会话键（hostId::toolId）。
    pub(crate) conversation_key: String,
    /// 请求 ID。
    pub(crate) request_id: String,
}

/// 单次聊天请求参数。
#[derive(Debug, Clone)]
pub(crate) struct ChatRequestInput {
    pub(crate) tool_id: String,
    pub(crate) conversation_key: String,
    pub(crate) request_id: String,
    pub(crate) queue_item_id: String,
    pub(crate) text: String,
    pub(crate) content: Vec<ChatContentPart>,
}

/// 聊天取消参数。
#[derive(Debug, Clone)]
pub(crate) struct ChatCancelInput {
    pub(crate) tool_id: String,
    pub(crate) conversation_key: String,
    pub(crate) request_id: String,
    pub(crate) queue_item_id: String,
}

/// 发起聊天请求返回结果。
#[derive(Debug, Clone)]
pub(crate) enum StartChatOutcome {
    Started,
    Busy { reason: String },
}

/// 取消聊天请求返回结果。
#[derive(Debug, Clone)]
pub(crate) enum CancelChatOutcome {
    Accepted,
    NotFound,
}

/// 运行中的会话任务元数据。
#[derive(Debug)]
struct ActiveChatTask {
    tool_id: String,
    queue_item_id: String,
    request_id: String,
    cancel_tx: watch::Sender<bool>,
}

/// 会话级聊天运行时。
#[derive(Debug, Default)]
pub(crate) struct ChatRuntime {
    active_by_conversation: HashMap<String, ActiveChatTask>,
}

impl ChatRuntime {
    /// 尝试在指定会话启动聊天任务；若会话忙，返回 busy。
    pub(crate) fn start_request(
        &mut self,
        request: ChatRequestInput,
        tool: ToolRuntimePayload,
        trace_id: Option<String>,
        event_tx: ChatEventSender,
    ) -> StartChatOutcome {
        if let Some(active) = self.active_by_conversation.get(&request.conversation_key) {
            return StartChatOutcome::Busy {
                reason: format!("会话中已有进行中的请求：{}", active.request_id),
            };
        }

        let (cancel_tx, cancel_rx) = watch::channel(false);
        self.active_by_conversation.insert(
            request.conversation_key.clone(),
            ActiveChatTask {
                tool_id: request.tool_id.clone(),
                queue_item_id: request.queue_item_id.clone(),
                request_id: request.request_id.clone(),
                cancel_tx,
            },
        );

        tokio::spawn(run_chat_task(request, tool, trace_id, event_tx, cancel_rx));
        StartChatOutcome::Started
    }

    /// 取消会话内请求（requestId 匹配时生效）。
    pub(crate) fn cancel_request(&mut self, cancel: &ChatCancelInput) -> CancelChatOutcome {
        let Some(active) = self
            .active_by_conversation
            .get_mut(&cancel.conversation_key)
        else {
            return CancelChatOutcome::NotFound;
        };
        if !cancel.tool_id.trim().is_empty() && active.tool_id != cancel.tool_id {
            return CancelChatOutcome::NotFound;
        }
        if !cancel.queue_item_id.trim().is_empty() && active.queue_item_id != cancel.queue_item_id {
            return CancelChatOutcome::NotFound;
        }
        if active.request_id != cancel.request_id {
            return CancelChatOutcome::NotFound;
        }

        let _ = active.cancel_tx.send(true);
        CancelChatOutcome::Accepted
    }

    /// 收到 finished 事件后释放会话占用。
    pub(crate) fn mark_finished(&mut self, key: &ChatFinalizeKey) {
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
enum ChatExecError {
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
struct ChatExecutionResult {
    text: String,
    emitted_chunk: bool,
    meta: Value,
}

#[derive(Debug, Clone)]
struct PreparedPrompt {
    prompt_text: String,
    attachment_delivery: Value,
}

#[derive(Debug, Clone)]
struct PreparedMediaAttachment {
    media_id: String,
    kind: String,
    mime: String,
    path: String,
    path_hint: String,
    size: u64,
    duration_ms: u64,
}

#[derive(Debug, Clone)]
struct MediaDeliveryFailure {
    media_id: String,
    kind: String,
    mime: String,
    path_hint: String,
    code: &'static str,
    reason: String,
}

/// 任务入口：发送 started -> 执行工具命令 -> 发送 finished。
async fn run_chat_task(
    request: ChatRequestInput,
    tool: ToolRuntimePayload,
    trace_id: Option<String>,
    event_tx: ChatEventSender,
    mut cancel_rx: watch::Receiver<bool>,
) {
    emit_started(&event_tx, trace_id.clone(), &request);

    let result = execute_chat_request(&request, &tool, &trace_id, &event_tx, &mut cancel_rx).await;

    match result {
        Ok(done) => {
            emit_finished(
                &event_tx,
                trace_id,
                &request,
                "completed",
                if done.emitted_chunk {
                    ""
                } else {
                    done.text.as_str()
                },
                "",
                done.meta,
            );
        }
        Err(ChatExecError::Cancelled) => {
            emit_finished(
                &event_tx,
                trace_id,
                &request,
                "cancelled",
                "",
                "请求已取消",
                json!({}),
            );
        }
        Err(ChatExecError::Failed(reason)) => {
            emit_finished(
                &event_tx,
                trace_id,
                &request,
                "failed",
                "",
                &reason,
                json!({}),
            );
        }
    }
}

/// 根据工具类型执行聊天。
async fn execute_chat_request(
    request: &ChatRequestInput,
    tool: &ToolRuntimePayload,
    trace_id: &Option<String>,
    event_tx: &ChatEventSender,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<ChatExecutionResult, ChatExecError> {
    if cancelled(cancel_rx) {
        return Err(ChatExecError::Cancelled);
    }

    let prepared = prepare_request_prompt(request, tool);
    let prompt_text = prepared.prompt_text;

    let mut result = if is_opencode_tool(tool) {
        run_opencode_request(request, &prompt_text, tool, trace_id, event_tx, cancel_rx).await?
    } else if is_openclaw_tool(tool) {
        run_openclaw_request(request, &prompt_text, tool, trace_id, event_tx, cancel_rx).await?
    } else if is_codex_tool(tool) {
        run_codex_request(request, &prompt_text, tool, cancel_rx).await?
    } else if is_claude_code_tool(tool) {
        run_claude_code_request(request, &prompt_text, tool, cancel_rx).await?
    } else {
        return Err(ChatExecError::Failed("当前工具类型不支持聊天执行".to_string()));
    };

    result.meta = merge_attachment_delivery_meta(result.meta, prepared.attachment_delivery);
    Ok(result)
}

fn prepare_request_prompt(request: &ChatRequestInput, tool: &ToolRuntimePayload) -> PreparedPrompt {
    let mut text_blocks = Vec::new();
    let mut file_ref_blocks = Vec::new();
    let mut sent_media = Vec::<PreparedMediaAttachment>::new();
    let mut failed_media = Vec::<MediaDeliveryFailure>::new();

    for part in &request.content {
        let kind = part.kind.trim().to_ascii_lowercase();
        if kind == "text" {
            let value = part.text.trim();
            if !value.is_empty() {
                text_blocks.push(value.to_string());
            }
            continue;
        }
        if kind == "fileref" {
            let path_hint = part.path_hint.trim();
            let note = if !path_hint.is_empty() {
                format!("file: {path_hint}")
            } else if !part.text.trim().is_empty() {
                format!("file: {}", part.text.trim())
            } else {
                "file reference".to_string()
            };
            file_ref_blocks.push(note);
            continue;
        }
        if kind == "image" || kind == "video" || kind == "audio" {
            match resolve_media_attachment_for_prompt(request, tool, part, kind.as_str()) {
                Ok(ok) => sent_media.push(ok),
                Err(err) => failed_media.push(err),
            }
        }
    }

    if text_blocks.is_empty() && !request.text.trim().is_empty() {
        text_blocks.push(request.text.trim().to_string());
    }
    if !file_ref_blocks.is_empty() {
        text_blocks.push(format!("Attached files:\n- {}", file_ref_blocks.join("\n- ")));
    }

    let media_context = build_media_context_block(request, &sent_media, &failed_media);
    let prompt_text = if media_context.is_empty() {
        text_blocks.join("\n").trim().to_string()
    } else if text_blocks.is_empty() {
        media_context
    } else {
        format!("{media_context}\n\n{}", text_blocks.join("\n"))
    };

    PreparedPrompt {
        prompt_text,
        attachment_delivery: build_attachment_delivery_json(&sent_media, &failed_media),
    }
}

fn merge_attachment_delivery_meta(meta: Value, attachment_delivery: Value) -> Value {
    if let Some(mut obj) = meta.as_object().cloned() {
        obj.insert("attachmentDelivery".to_string(), attachment_delivery);
        Value::Object(obj)
    } else {
        json!({
            "attachmentDelivery": attachment_delivery,
            "providerMeta": meta,
        })
    }
}

fn resolve_media_attachment_for_prompt(
    request: &ChatRequestInput,
    tool: &ToolRuntimePayload,
    part: &ChatContentPart,
    kind: &str,
) -> Result<PreparedMediaAttachment, MediaDeliveryFailure> {
    if !part.stage_error_code.trim().is_empty() {
        return Err(MediaDeliveryFailure {
            media_id: resolve_media_id(part, kind),
            kind: kind.to_string(),
            mime: part.mime.trim().to_string(),
            path_hint: part.path_hint.trim().to_string(),
            code: map_error_code(part.stage_error_code.trim()),
            reason: if part.stage_error_reason.trim().is_empty() {
                "附件暂存失败，已跳过。".to_string()
            } else {
                part.stage_error_reason.trim().to_string()
            },
        });
    }

    let staged_path = if !part.staged_media_id.trim().is_empty() {
        resolve_staged_media_path(tool, part.staged_media_id.trim()).map_err(|(code, reason)| {
            MediaDeliveryFailure {
                media_id: resolve_media_id(part, kind),
                kind: kind.to_string(),
                mime: part.mime.trim().to_string(),
                path_hint: part.path_hint.trim().to_string(),
                code,
                reason,
            }
        })?
    } else if !part.data_base64.trim().is_empty() {
        stage_inline_media_attachment(request, tool, part, kind).map_err(|(code, reason)| {
            MediaDeliveryFailure {
                media_id: resolve_media_id(part, kind),
                kind: kind.to_string(),
                mime: part.mime.trim().to_string(),
                path_hint: part.path_hint.trim().to_string(),
                code,
                reason,
            }
        })?
    } else {
        return Err(MediaDeliveryFailure {
            media_id: resolve_media_id(part, kind),
            kind: kind.to_string(),
            mime: part.mime.trim().to_string(),
            path_hint: part.path_hint.trim().to_string(),
            code: MEDIA_STAGE_NOT_FOUND,
            reason: "附件缺少暂存引用，已跳过。".to_string(),
        });
    };

    let staged_path_text = staged_path.to_string_lossy().to_string();
    Ok(PreparedMediaAttachment {
        media_id: resolve_media_id(part, kind),
        kind: kind.to_string(),
        mime: part.mime.trim().to_string(),
        path: staged_path_text,
        path_hint: part.path_hint.trim().to_string(),
        size: part.size,
        duration_ms: part.duration_ms,
    })
}

fn resolve_media_id(part: &ChatContentPart, kind: &str) -> String {
    let media_id = part.media_id.trim();
    if !media_id.is_empty() {
        return media_id.to_string();
    }
    format!("inline-{kind}")
}

fn map_error_code(raw: &str) -> &'static str {
    match raw.trim() {
        MEDIA_UNSUPPORTED_MIME => MEDIA_UNSUPPORTED_MIME,
        MEDIA_TOO_LARGE => MEDIA_TOO_LARGE,
        MEDIA_DECODE_FAILED => MEDIA_DECODE_FAILED,
        MEDIA_STAGE_NOT_FOUND => MEDIA_STAGE_NOT_FOUND,
        MEDIA_STAGE_EXPIRED => MEDIA_STAGE_EXPIRED,
        MEDIA_PATH_FORBIDDEN => MEDIA_PATH_FORBIDDEN,
        _ => MEDIA_STAGE_NOT_FOUND,
    }
}

fn resolve_staged_media_path(
    tool: &ToolRuntimePayload,
    staged_media_id: &str,
) -> Result<PathBuf, (&'static str, String)> {
    let root = resolve_media_inbox_root(tool).map_err(|reason| (MEDIA_PATH_FORBIDDEN, reason))?;
    let normalized = staged_media_id.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains("../")
        || normalized.contains("/..")
    {
        return Err((MEDIA_PATH_FORBIDDEN, "暂存附件路径非法。".to_string()));
    }
    let candidate = root.join(normalized);
    let canonical_candidate = fs::canonicalize(&candidate).map_err(|err| {
        (
            MEDIA_STAGE_NOT_FOUND,
            format!("暂存附件不存在或不可访问: {err}"),
        )
    })?;
    let canonical_root = fs::canonicalize(&root).map_err(|err| {
        (
            MEDIA_PATH_FORBIDDEN,
            format!("暂存目录不可访问: {err}"),
        )
    })?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err((MEDIA_PATH_FORBIDDEN, "暂存附件路径越界。".to_string()));
    }
    if !canonical_candidate.is_file() {
        return Err((MEDIA_STAGE_NOT_FOUND, "暂存附件不是文件。".to_string()));
    }
    if is_stage_file_expired(&canonical_candidate) {
        return Err((MEDIA_STAGE_EXPIRED, "暂存附件已过期。".to_string()));
    }
    Ok(canonical_candidate)
}

fn stage_inline_media_attachment(
    request: &ChatRequestInput,
    tool: &ToolRuntimePayload,
    part: &ChatContentPart,
    kind: &str,
) -> Result<PathBuf, (&'static str, String)> {
    let provided_mime = part.mime.trim().to_ascii_lowercase();
    if !provided_mime.starts_with("image/")
        && !provided_mime.starts_with("video/")
        && !provided_mime.starts_with("audio/")
    {
        return Err((MEDIA_UNSUPPORTED_MIME, "仅支持 image/video/audio MIME。".to_string()));
    }
    let raw_payload = part.data_base64.trim();
    if raw_payload.is_empty() {
        return Err((MEDIA_DECODE_FAILED, "附件内容为空。".to_string()));
    }
    let (_, base64_payload) = parse_base64_payload(raw_payload);
    let bytes = general_purpose::STANDARD
        .decode(base64_payload.as_bytes())
        .map_err(|err| (MEDIA_DECODE_FAILED, format!("附件 base64 解码失败: {err}")))?;
    if bytes.is_empty() {
        return Err((MEDIA_DECODE_FAILED, "附件内容为空。".to_string()));
    }
    if bytes.len() > MEDIA_STAGE_MAX_BYTES {
        return Err((
            MEDIA_TOO_LARGE,
            format!("附件超过大小限制（{} MB）。", MEDIA_STAGE_MAX_BYTES / (1024 * 1024)),
        ));
    }

    let stage_root = resolve_media_inbox_root(tool).map_err(|reason| (MEDIA_PATH_FORBIDDEN, reason))?;
    cleanup_media_stage_dir(&stage_root);
    let conv_segment = sanitize_path_segment(request.conversation_key.as_str());
    let req_segment = sanitize_path_segment(request.request_id.as_str());
    let media_segment = sanitize_path_segment(resolve_media_id(part, kind).as_str());
    let ext = mime_extension(&provided_mime);
    let dir = stage_root.join(conv_segment).join(req_segment);
    fs::create_dir_all(&dir)
        .map_err(|err| (MEDIA_PATH_FORBIDDEN, format!("创建暂存目录失败: {err}")))?;
    let path = dir.join(format!("{media_segment}.{ext}"));
    fs::write(&path, &bytes).map_err(|err| (MEDIA_PATH_FORBIDDEN, format!("写入附件失败: {err}")))?;
    Ok(path)
}

fn resolve_media_inbox_root(tool: &ToolRuntimePayload) -> Result<PathBuf, String> {
    if let Some(raw) = env::var_os(MEDIA_STAGE_DIR_ENV) {
        let candidate = PathBuf::from(raw);
        if !candidate.as_os_str().is_empty() {
            return Ok(candidate);
        }
    }
    let Some(workspace) = tool
        .workspace_dir
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    else {
        return Err("工具缺少工作区，无法解析附件。".to_string());
    };
    let canonical =
        fs::canonicalize(workspace).map_err(|err| format!("工作区不可访问或不存在: {err}"))?;
    if !canonical.is_dir() {
        return Err("工具工作区不是目录。".to_string());
    }
    Ok(canonical.join(MEDIA_STAGE_INBOX_DIR))
}

fn parse_base64_payload(raw: &str) -> (String, String) {
    if !raw.starts_with("data:") {
        return (String::new(), raw.trim().to_string());
    }
    let marker = ";base64,";
    if let Some(index) = raw.find(marker) {
        let mime = raw[5..index].trim().to_ascii_lowercase();
        let payload = raw[(index + marker.len())..].trim().to_string();
        return (mime, payload);
    }
    (String::new(), raw.trim().to_string())
}

fn sanitize_path_segment(raw: &str) -> String {
    let mut output = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "unknown".to_string()
    } else {
        output
    }
}

fn mime_extension(mime: &str) -> String {
    let sub = mime
        .split('/')
        .nth(1)
        .unwrap_or("bin")
        .split(';')
        .next()
        .unwrap_or("bin")
        .trim()
        .to_ascii_lowercase();
    let sanitized = sanitize_path_segment(sub.as_str());
    if sanitized.is_empty() {
        "bin".to_string()
    } else {
        sanitized
    }
}

fn cleanup_media_stage_dir(root: &Path) {
    if !root.exists() {
        return;
    }
    let mut stack = vec![root.to_path_buf()];
    let now = std::time::SystemTime::now();
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let Ok(elapsed) = now.duration_since(modified) else {
                continue;
            };
            if elapsed.as_secs() > MEDIA_STAGE_TTL_SEC {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn is_stage_file_expired(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = meta.modified() else {
        return true;
    };
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(modified) else {
        return false;
    };
    elapsed.as_secs() > MEDIA_STAGE_TTL_SEC
}

fn build_media_context_block(
    request: &ChatRequestInput,
    sent_media: &[PreparedMediaAttachment],
    failed_media: &[MediaDeliveryFailure],
) -> String {
    if sent_media.is_empty() && failed_media.is_empty() {
        return String::new();
    }
    let attachments = sent_media
        .iter()
        .map(|item| {
            json!({
                "media_id": item.media_id,
                "kind": item.kind,
                "mime": item.mime,
                "path": item.path,
                "path_hint": item.path_hint,
                "size": item.size,
                "duration_ms": item.duration_ms,
            })
        })
        .collect::<Vec<Value>>();
    let failed = failed_media
        .iter()
        .map(|item| {
            json!({
                "media_id": item.media_id,
                "kind": item.kind,
                "mime": item.mime,
                "path_hint": item.path_hint,
                "code": item.code,
                "reason": item.reason,
            })
        })
        .collect::<Vec<Value>>();
    let payload = json!({
        "request_id": request.request_id,
        "conversation_key": request.conversation_key,
        "attachments": attachments,
        "failed_attachments": failed,
    });
    let serialized = serde_json::to_string_pretty(&payload)
        .unwrap_or_else(|_| payload.to_string());
    format!("[YC_MEDIA_CONTEXT_V1]\n{serialized}\n[/YC_MEDIA_CONTEXT_V1]")
}

fn build_attachment_delivery_json(
    sent_media: &[PreparedMediaAttachment],
    failed_media: &[MediaDeliveryFailure],
) -> Value {
    let status = if sent_media.is_empty() && failed_media.is_empty() {
        "none"
    } else if !sent_media.is_empty() && failed_media.is_empty() {
        "full"
    } else if sent_media.is_empty() {
        "none"
    } else {
        "partial"
    };
    json!({
        "status": status,
        "sent": sent_media
            .iter()
            .map(|item| {
                json!({
                    "mediaId": item.media_id,
                    "kind": item.kind,
                    "mime": item.mime,
                    "path": item.path,
                    "pathHint": item.path_hint,
                    "size": item.size,
                    "durationMs": item.duration_ms,
                })
            })
            .collect::<Vec<Value>>(),
        "failed": failed_media
            .iter()
            .map(|item| {
                json!({
                    "mediaId": item.media_id,
                    "kind": item.kind,
                    "mime": item.mime,
                    "pathHint": item.path_hint,
                    "code": item.code,
                    "reason": item.reason,
                })
            })
            .collect::<Vec<Value>>(),
    })
}

/// OpenCode: 使用 `opencode run --format json` 并按 text 事件流式回传。
async fn run_opencode_request(
    request: &ChatRequestInput,
    prompt_text: &str,
    tool: &ToolRuntimePayload,
    trace_id: &Option<String>,
    event_tx: &ChatEventSender,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<ChatExecutionResult, ChatExecError> {
    let mut command = Command::new("opencode");
    command
        .arg("run")
        .arg(prompt_text)
        .arg("--format")
        .arg("json")
        .arg("--continue")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(session_id) = tool
        .session_id
        .as_ref()
        .filter(|raw| !raw.trim().is_empty())
    {
        command.arg("--session").arg(session_id);
    }
    if let Some(workspace) = tool
        .workspace_dir
        .as_ref()
        .filter(|raw| !raw.trim().is_empty())
    {
        command.current_dir(workspace);
    }

    let mut child = command
        .spawn()
        .map_err(|err| ChatExecError::Failed(format!("启动 opencode 失败: {err}")))?;

    let Some(stdout) = child.stdout.take() else {
        return Err(ChatExecError::Failed("opencode stdout 不可用".to_string()));
    };
    let stderr_task = spawn_stderr_reader(child.stderr.take());
    let mut lines = BufReader::new(stdout).lines();
    let mut emitted_chunk = false;
    let mut merged_text = String::new();
    let mut session_id = tool.session_id.clone().unwrap_or_default();
    let mut usage = json!({});

    loop {
        tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && cancelled(cancel_rx) {
                    kill_child(&mut child).await;
                    let _ = stderr_task.await;
                    return Err(ChatExecError::Cancelled);
                }
            }
            line = lines.next_line() => {
                let line = line
                    .map_err(|err| ChatExecError::Failed(format!("读取 opencode 输出失败: {err}")))?;
                let Some(raw_line) = line else {
                    break;
                };
                let trimmed = raw_line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let Some(parsed) = parse_opencode_line(trimmed) else {
                    continue;
                };
                if session_id.is_empty() && !parsed.session_id.is_empty() {
                    session_id = parsed.session_id;
                }
                if let Some(text) = parsed.chunk_text {
                    merged_text.push_str(&text);
                    emitted_chunk = true;
                    emit_chunk(
                        event_tx,
                        trace_id.clone(),
                        request,
                        &text,
                        json!({ "sessionId": session_id }),
                    );
                }
                if let Some(tokens) = parsed.usage {
                    usage = tokens;
                }
            }
        }
    }

    let status = wait_child_with_cancel(&mut child, cancel_rx).await?;
    let stderr = join_reader_task(stderr_task).await;
    if !status.success() {
        let mut reason = shorten_error(&stderr);
        if reason == "未知错误" {
            let fallback = merged_text
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            if !fallback.is_empty() {
                reason = fallback;
            }
        }
        return Err(ChatExecError::Failed(format!(
            "opencode 执行失败: {}",
            reason
        )));
    }

    Ok(ChatExecutionResult {
        text: merged_text,
        emitted_chunk,
        meta: json!({
            "sessionId": session_id,
            "usage": usage,
        }),
    })
}

async fn run_codex_request(
    _request: &ChatRequestInput,
    prompt_text: &str,
    tool: &ToolRuntimePayload,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<ChatExecutionResult, ChatExecError> {
    let mut command = Command::new("codex");
    command.arg("exec").arg(prompt_text).arg("--json");
    if let Some(profile) = tool
        .source
        .as_deref()
        .and_then(|raw| raw.split("profile=").nth(1))
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        command.arg("--profile").arg(profile);
    }
    if let Some(workspace) = tool
        .workspace_dir
        .as_ref()
        .filter(|raw| !raw.trim().is_empty())
    {
        command.current_dir(workspace);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = command
        .spawn()
        .map_err(|err| ChatExecError::Failed(format!("启动 codex 失败: {err}")))?;
    let output = collect_child_output_with_cancel(child, cancel_rx).await?;
    if !output.success {
        let reason = extract_codex_exec_text(&output.stdout)
            .or_else(|| {
                extract_json_payload(&output.stdout).and_then(|value| extract_generic_text(&value))
            })
            .unwrap_or_else(|| {
                let stderr_reason = shorten_error(&output.stderr);
                if stderr_reason == "未知错误" {
                    shorten_error(&output.stdout)
                } else {
                    stderr_reason
                }
            });
        return Err(ChatExecError::Failed(format!(
            "codex 执行失败: {}",
            reason
        )));
    }

    let text = extract_codex_exec_text(&output.stdout)
        .or_else(|| {
            extract_json_payload(&output.stdout).and_then(|value| extract_generic_text(&value))
        })
        .or_else(|| {
            let trimmed = output.stdout.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or_else(|| "Codex 已完成请求，但未返回可读输出。".to_string());

    Ok(ChatExecutionResult {
        text,
        emitted_chunk: false,
        meta: json!({ "provider": "codex" }),
    })
}

async fn run_claude_code_request(
    _request: &ChatRequestInput,
    prompt_text: &str,
    tool: &ToolRuntimePayload,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<ChatExecutionResult, ChatExecError> {
    let mut command = Command::new("claude");
    command
        .arg("-p")
        .arg(prompt_text)
        .arg("--output-format")
        .arg("json");
    if let Some(profile) = tool
        .source
        .as_deref()
        .and_then(|raw| raw.split("profile=").nth(1))
        .map(str::trim)
        .filter(|raw| !raw.is_empty() && *raw != "default")
    {
        command.arg("--profile").arg(profile);
    }
    if let Some(workspace) = tool
        .workspace_dir
        .as_ref()
        .filter(|raw| !raw.trim().is_empty())
    {
        command.current_dir(workspace);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = command
        .spawn()
        .map_err(|err| ChatExecError::Failed(format!("启动 claude 失败: {err}")))?;
    let output = collect_child_output_with_cancel(child, cancel_rx).await?;
    if !output.success {
        let reason = extract_json_payload(&output.stdout)
            .and_then(|value| extract_generic_text(&value))
            .unwrap_or_else(|| {
                let stderr_reason = shorten_error(&output.stderr);
                if stderr_reason == "未知错误" {
                    shorten_error(&output.stdout)
                } else {
                    stderr_reason
                }
            });
        return Err(ChatExecError::Failed(format!(
            "claude 执行失败: {}",
            reason
        )));
    }

    let text = extract_json_payload(&output.stdout)
        .and_then(|value| extract_generic_text(&value))
        .or_else(|| {
            let trimmed = output.stdout.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or_else(|| "Claude Code 已完成请求，但未返回可读输出。".to_string());

    Ok(ChatExecutionResult {
        text,
        emitted_chunk: false,
        meta: json!({ "provider": "claude-code" }),
    })
}

/// OpenClaw: 已知 slash 命令走 gateway chat 通道，其余消息沿用 agent 通道。
async fn run_openclaw_request(
    request: &ChatRequestInput,
    prompt_text: &str,
    tool: &ToolRuntimePayload,
    trace_id: &Option<String>,
    event_tx: &ChatEventSender,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<ChatExecutionResult, ChatExecError> {
    let route = resolve_openclaw_route(tool).await;
    let result = if is_openclaw_known_slash_command(request.text.as_str()) {
        run_openclaw_slash_request(request, prompt_text, tool, &route, cancel_rx).await?
    } else {
        run_openclaw_agent_request(request, prompt_text, tool, &route, cancel_rx).await?
    };

    emit_chunk(
        event_tx,
        trace_id.clone(),
        request,
        &result.text,
        result.meta.clone(),
    );
    Ok(ChatExecutionResult {
        text: result.text,
        emitted_chunk: true,
        meta: result.meta,
    })
}

#[derive(Debug, Clone)]
struct OpenClawRoute {
    session_id: String,
    session_key: String,
    agent_id: String,
}

#[derive(Debug, Clone)]
struct OpenClawAttemptResult {
    text: String,
    meta: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenClawRouteDecision {
    UseChannel,
    RetryLocal,
    Cancelled,
}

const OPENCLAW_CHAT_HISTORY_LIMIT: usize = 120;
const OPENCLAW_CHAT_POLL_INTERVAL: Duration = Duration::from_millis(700);
const OPENCLAW_CHAT_POLL_TIMEOUT: Duration = Duration::from_secs(12);
const OPENCLAW_CHAT_HISTORY_SYNC_POLL_INTERVAL: Duration = Duration::from_millis(300);
const OPENCLAW_CHAT_HISTORY_SYNC_POLL_TIMEOUT: Duration = Duration::from_secs(4);
const OPENCLAW_GATEWAY_UPGRADE_HINT: &str =
    "当前 OpenClaw 版本不支持 chat.send/chat.history，请升级 OpenClaw 后重试。";
const MEDIA_STAGE_MAX_BYTES: usize = 32 * 1024 * 1024;
const MEDIA_STAGE_TTL_SEC: u64 = 24 * 3600;
const MEDIA_STAGE_DIR_ENV: &str = "YC_MEDIA_STAGE_DIR";
const MEDIA_STAGE_INBOX_DIR: &str = ".yc/inbox";
const MEDIA_UNSUPPORTED_MIME: &str = "MEDIA_UNSUPPORTED_MIME";
const MEDIA_TOO_LARGE: &str = "MEDIA_TOO_LARGE";
const MEDIA_DECODE_FAILED: &str = "MEDIA_DECODE_FAILED";
const MEDIA_STAGE_NOT_FOUND: &str = "MEDIA_STAGE_NOT_FOUND";
const MEDIA_STAGE_EXPIRED: &str = "MEDIA_STAGE_EXPIRED";
const MEDIA_PATH_FORBIDDEN: &str = "MEDIA_PATH_FORBIDDEN";

#[derive(Debug, Clone, Copy, Default)]
struct OpenClawHistoryAnchor {
    latest_timestamp: i64,
}

#[derive(Debug, Clone, Default)]
struct OpenClawChatReply {
    text: String,
    report_paths: Vec<String>,
}

async fn resolve_openclaw_route(tool: &ToolRuntimePayload) -> OpenClawRoute {
    let mut command = Command::new("openclaw");
    apply_openclaw_profile(tool, &mut command);
    command.arg("status").arg("--json");
    let output = timeout(Duration::from_secs(5), command.output()).await;
    let mut session_id = String::new();
    let mut session_key = String::new();
    let mut agent_id = "main".to_string();

    if let Ok(Ok(raw)) = output
        && raw.status.success()
    {
        let stdout = String::from_utf8_lossy(&raw.stdout).to_string();
        if let Some(value) = extract_json_payload(&stdout) {
            if let Some((picked_session_id, picked_session_key, picked_agent_id)) =
                select_openclaw_recent_session(&value)
            {
                session_id = picked_session_id;
                session_key = picked_session_key;
                if !picked_agent_id.is_empty() {
                    agent_id = picked_agent_id;
                }
            }
            let candidate_agent = value
                .pointer("/heartbeat/defaultAgentId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !candidate_agent.is_empty() {
                agent_id = candidate_agent;
            }
        }
    }

    OpenClawRoute {
        session_id,
        session_key,
        agent_id,
    }
}

fn select_openclaw_recent_session(status_json: &Value) -> Option<(String, String, String)> {
    let recent = status_json
        .pointer("/sessions/recent")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for row in recent {
        if is_openclaw_system_session(&row) {
            continue;
        }
        let session_id = read_string_any(&row, &["sessionId", "sessionID"])
            .trim()
            .to_string();
        let session_key = read_string_any(&row, &["key"]).trim().to_string();
        if session_id.is_empty() && session_key.is_empty() {
            continue;
        }
        let agent_id = read_string_any(&row, &["agentId"]).trim().to_string();
        return Some((session_id, session_key, agent_id));
    }
    None
}

fn is_openclaw_system_session(session_row: &Value) -> bool {
    if session_row
        .get("systemSent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }

    let kind = read_string_any(session_row, &["kind"])
        .trim()
        .to_ascii_lowercase();
    if kind.contains("system") || kind.contains("cron") || kind.contains("task") {
        return true;
    }

    let key = read_string_any(session_row, &["key"])
        .trim()
        .to_ascii_lowercase();
    if key.contains("session-cleanup") || key.starts_with("cron:") || key.starts_with("system:") {
        return true;
    }

    let Some(flags) = session_row.get("flags").and_then(Value::as_array) else {
        return false;
    };
    for flag in flags {
        let normalized = flag
            .as_str()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if normalized.contains("system")
            || normalized.contains("cron")
            || normalized.contains("task")
        {
            return true;
        }
    }
    false
}

async fn run_openclaw_agent_request(
    request: &ChatRequestInput,
    prompt_text: &str,
    tool: &ToolRuntimePayload,
    route: &OpenClawRoute,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<OpenClawAttemptResult, ChatExecError> {
    let channel_attempt =
        run_openclaw_once(request, prompt_text, tool, route, false, cancel_rx).await;
    match decide_openclaw_route(&channel_attempt) {
        OpenClawRouteDecision::UseChannel => match channel_attempt {
            Ok(ok) => Ok(ok),
            Err(_) => Err(ChatExecError::Failed(
                "openclaw route decision mismatch".to_string(),
            )),
        },
        OpenClawRouteDecision::RetryLocal => {
            run_openclaw_once(request, prompt_text, tool, route, true, cancel_rx).await
        }
        OpenClawRouteDecision::Cancelled => Err(ChatExecError::Cancelled),
    }
}

async fn run_openclaw_slash_request(
    request: &ChatRequestInput,
    prompt_text: &str,
    tool: &ToolRuntimePayload,
    route: &OpenClawRoute,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<OpenClawAttemptResult, ChatExecError> {
    if cancelled(cancel_rx) {
        return Err(ChatExecError::Cancelled);
    }

    let command_token = extract_openclaw_command_token(request.text.as_str())
        .unwrap_or_else(|| "/".to_string())
        .to_ascii_lowercase();
    let session_key = resolve_openclaw_session_key(route);
    let history_anchor = run_openclaw_chat_history(tool, session_key.as_str(), cancel_rx)
        .await
        .map(|payload| capture_openclaw_history_anchor(&payload))
        .unwrap_or_default();
    let run_id = format!("sidecar-chat-{}", Uuid::new_v4());

    let mut send_payload = run_openclaw_chat_send(
        tool,
        session_key.as_str(),
        prompt_text,
        run_id.as_str(),
        cancel_rx,
    )
    .await
    .map_err(map_openclaw_slash_gateway_error)?;
    let poll_started_at = Instant::now();
    let mut status = openclaw_chat_status(&send_payload).to_string();

    while status != "ok" && status != "error" {
        if poll_started_at.elapsed() >= OPENCLAW_CHAT_POLL_TIMEOUT {
            break;
        }
        if cancelled(cancel_rx) {
            let _ = run_openclaw_chat_abort(tool, session_key.as_str(), run_id.as_str()).await;
            return Err(ChatExecError::Cancelled);
        }
        sleep(OPENCLAW_CHAT_POLL_INTERVAL).await;
        send_payload = run_openclaw_chat_send(
            tool,
            session_key.as_str(),
            prompt_text,
            run_id.as_str(),
            cancel_rx,
        )
        .await
        .map_err(map_openclaw_slash_gateway_error)?;
        status = openclaw_chat_status(&send_payload).to_string();
    }

    if status == "error" {
        let summary = send_payload
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        if is_openclaw_legacy_chat_api_error(summary) {
            return Err(ChatExecError::Failed(
                OPENCLAW_GATEWAY_UPGRADE_HINT.to_string(),
            ));
        }
        return Err(ChatExecError::Failed(format!(
            "openclaw slash command 执行失败（{command_token}）: {summary}"
        )));
    }

    if status != "ok" {
        let _ = run_openclaw_chat_abort(tool, session_key.as_str(), run_id.as_str()).await;
        let text = format!("命令 {command_token} 已提交，等待 OpenClaw 产出历史消息。");
        return Ok(OpenClawAttemptResult {
            text,
            meta: json!({
                "command": command_token,
                "runId": run_id,
                "sessionKey": session_key,
                "status": status,
                "source": "gateway.chat.send",
                "reportPaths": [],
            }),
        });
    }

    if let Some(reply) =
        poll_openclaw_chat_reply_after(tool, session_key.as_str(), history_anchor, cancel_rx)
            .await?
    {
        let OpenClawChatReply {
            text: reply_text,
            report_paths,
        } = reply;
        return Ok(OpenClawAttemptResult {
            text: reply_text,
            meta: json!({
                "command": command_token,
                "runId": run_id,
                "sessionKey": session_key,
                "status": status,
                "source": "gateway.chat.send",
                "reportPaths": report_paths,
            }),
        });
    }

    let text = format!("命令 {command_token} 已提交，等待 OpenClaw 产出历史消息。");
    Ok(OpenClawAttemptResult {
        text,
        meta: json!({
            "command": command_token,
            "runId": run_id,
            "sessionKey": session_key,
            "status": status,
            "source": "gateway.chat.send",
            "reportPaths": [],
        }),
    })
}

async fn poll_openclaw_chat_reply_after(
    tool: &ToolRuntimePayload,
    session_key: &str,
    anchor: OpenClawHistoryAnchor,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<Option<OpenClawChatReply>, ChatExecError> {
    let started_at = Instant::now();
    loop {
        if cancelled(cancel_rx) {
            return Err(ChatExecError::Cancelled);
        }
        let history_payload = run_openclaw_chat_history(tool, session_key, cancel_rx)
            .await
            .map_err(map_openclaw_slash_gateway_error)?;
        if let Some(reply) = extract_openclaw_chat_reply_after(&history_payload, anchor) {
            return Ok(Some(reply));
        }
        if started_at.elapsed() >= OPENCLAW_CHAT_HISTORY_SYNC_POLL_TIMEOUT {
            return Ok(None);
        }
        sleep(OPENCLAW_CHAT_HISTORY_SYNC_POLL_INTERVAL).await;
    }
}

async fn run_openclaw_chat_send(
    tool: &ToolRuntimePayload,
    session_key: &str,
    message: &str,
    idempotency_key: &str,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<Value, ChatExecError> {
    let params = json!({
        "sessionKey": session_key,
        "message": message,
        "idempotencyKey": idempotency_key,
    });
    run_openclaw_gateway_call(tool, "chat.send", params, cancel_rx, "chat.send").await
}

async fn run_openclaw_chat_history(
    tool: &ToolRuntimePayload,
    session_key: &str,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<Value, ChatExecError> {
    let params = json!({
        "sessionKey": session_key,
        "limit": OPENCLAW_CHAT_HISTORY_LIMIT,
    });
    run_openclaw_gateway_call(tool, "chat.history", params, cancel_rx, "chat.history").await
}

async fn run_openclaw_gateway_call(
    tool: &ToolRuntimePayload,
    method: &str,
    params: Value,
    cancel_rx: &mut watch::Receiver<bool>,
    action_name: &str,
) -> Result<Value, ChatExecError> {
    if cancelled(cancel_rx) {
        return Err(ChatExecError::Cancelled);
    }

    let mut command = Command::new("openclaw");
    apply_openclaw_profile(tool, &mut command);
    command
        .arg("gateway")
        .arg("call")
        .arg(method)
        .arg("--json")
        .arg("--params")
        .arg(params.to_string());
    if let Some(workspace) = tool
        .workspace_dir
        .as_ref()
        .filter(|raw| !raw.trim().is_empty())
    {
        command.current_dir(workspace);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = command
        .spawn()
        .map_err(|err| ChatExecError::Failed(format!("启动 openclaw {action_name} 失败: {err}")))?;
    let output = collect_child_output_with_cancel(child, cancel_rx).await?;
    if !output.success {
        return Err(ChatExecError::Failed(format!(
            "openclaw {action_name} 执行失败: {}",
            shorten_error(&output.stderr)
        )));
    }

    extract_json_payload(&output.stdout)
        .ok_or_else(|| ChatExecError::Failed(format!("openclaw {action_name} 返回非 JSON 输出")))
}

async fn run_openclaw_chat_abort(
    tool: &ToolRuntimePayload,
    session_key: &str,
    run_id: &str,
) -> Result<(), ChatExecError> {
    let params = json!({
        "sessionKey": session_key,
        "runId": run_id,
    });
    let mut command = Command::new("openclaw");
    apply_openclaw_profile(tool, &mut command);
    command
        .arg("gateway")
        .arg("call")
        .arg("chat.abort")
        .arg("--json")
        .arg("--params")
        .arg(params.to_string());
    if let Some(workspace) = tool
        .workspace_dir
        .as_ref()
        .filter(|raw| !raw.trim().is_empty())
    {
        command.current_dir(workspace);
    }

    let outcome = timeout(Duration::from_secs(3), command.output()).await;
    match outcome {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => Err(ChatExecError::Failed(format!(
            "openclaw chat.abort 执行失败: {}",
            shorten_error(&String::from_utf8_lossy(&output.stderr))
        ))),
        Ok(Err(err)) => Err(ChatExecError::Failed(format!(
            "启动 openclaw chat.abort 失败: {err}"
        ))),
        Err(_) => Err(ChatExecError::Failed(
            "openclaw chat.abort 超时".to_string(),
        )),
    }
}

fn capture_openclaw_history_anchor(payload: &Value) -> OpenClawHistoryAnchor {
    let latest_timestamp = payload
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .filter_map(|row| row.get("timestamp").and_then(Value::as_i64))
                .max()
        })
        .unwrap_or(0)
        .max(0);
    OpenClawHistoryAnchor { latest_timestamp }
}

fn extract_openclaw_chat_reply_after(
    payload: &Value,
    anchor: OpenClawHistoryAnchor,
) -> Option<OpenClawChatReply> {
    let rows = payload.get("messages").and_then(Value::as_array)?;
    let mut assistant_rows = Vec::new();
    let mut tool_rows = Vec::new();
    let mut system_rows = Vec::new();
    let mut assistant_report_paths = Vec::new();
    let mut tool_report_paths = Vec::new();
    let mut system_report_paths = Vec::new();

    for row in rows {
        let timestamp = row
            .get("timestamp")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        if timestamp <= anchor.latest_timestamp {
            continue;
        }
        let text = extract_openclaw_chat_message_text(row);
        if text.is_empty() {
            continue;
        }
        let report_paths = collect_markdown_report_paths(row);
        let role = row.get("role").and_then(Value::as_str).unwrap_or_default();
        if role == "assistant" {
            assistant_rows.push(text);
            append_unique_paths(&mut assistant_report_paths, report_paths);
        } else if role == "toolResult" {
            tool_rows.push(text);
            append_unique_paths(&mut tool_report_paths, report_paths);
        } else if role == "system" {
            system_rows.push(text);
            append_unique_paths(&mut system_report_paths, report_paths);
        }
    }

    if !assistant_rows.is_empty() {
        return Some(OpenClawChatReply {
            text: assistant_rows.join("\n"),
            report_paths: assistant_report_paths,
        });
    }
    if !tool_rows.is_empty() {
        return Some(OpenClawChatReply {
            text: tool_rows.join("\n"),
            report_paths: tool_report_paths,
        });
    }
    if !system_rows.is_empty() {
        return Some(OpenClawChatReply {
            text: system_rows.join("\n"),
            report_paths: system_report_paths,
        });
    }
    None
}

fn extract_openclaw_chat_message_text(row: &Value) -> String {
    let from_content = row
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !from_content.is_empty() {
        return from_content;
    }
    row.get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

fn collect_markdown_report_paths(value: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    collect_markdown_report_paths_recursive(value, None, &mut paths);
    paths
}

fn collect_markdown_report_paths_recursive(
    value: &Value,
    context_key: Option<&str>,
    output: &mut Vec<String>,
) {
    match value {
        Value::String(text) => {
            let Some(context) = context_key else {
                return;
            };
            if !is_report_path_context_key(context) && !is_report_path_container_key(context) {
                return;
            }
            collect_markdown_paths_from_text(text, output);
        }
        Value::Array(rows) => {
            for row in rows {
                collect_markdown_report_paths_recursive(row, context_key, output);
            }
        }
        Value::Object(map) => {
            let parent_is_container = context_key
                .map(is_report_path_container_key)
                .unwrap_or(false);
            for (key, nested) in map {
                let key_text = key.as_str();
                if !parent_is_container
                    && !is_report_path_context_key(key_text)
                    && !is_report_path_container_key(key_text)
                {
                    continue;
                }
                collect_markdown_report_paths_recursive(nested, Some(key_text), output);
            }
        }
        _ => {}
    }
}

fn collect_markdown_paths_from_text(raw: &str, output: &mut Vec<String>) {
    let text = raw.trim();
    if text.is_empty() {
        return;
    }
    let chars = text.char_indices().collect::<Vec<(usize, char)>>();
    if chars.is_empty() {
        return;
    }

    let mut index = 0usize;
    while index < chars.len() {
        let (start_byte, ch) = chars[index];
        let is_absolute = ch == '/';
        let is_home_relative = ch == '~'
            && chars
                .get(index + 1)
                .map(|(_, next)| *next == '/')
                .unwrap_or(false);
        if !is_absolute && !is_home_relative {
            index += 1;
            continue;
        }
        if !has_path_boundary(text, start_byte) {
            index += 1;
            continue;
        }

        let mut end_index = if is_home_relative {
            index + 2
        } else {
            index + 1
        };
        while end_index < chars.len() && !is_path_terminator(chars[end_index].1) {
            end_index += 1;
        }
        let end_byte = if end_index < chars.len() {
            chars[end_index].0
        } else {
            text.len()
        };
        let candidate = text[start_byte..end_byte]
            .trim_end_matches(|char| matches!(char, '.' | ',' | ';' | ':' | '!' | '?'));
        if is_markdown_report_path_candidate(candidate) {
            push_unique_path(output, candidate);
            index = end_index;
            continue;
        }
        index += 1;
    }
}

fn has_path_boundary(text: &str, start_byte: usize) -> bool {
    if start_byte == 0 {
        return true;
    }
    if text[..start_byte].ends_with("](") {
        return false;
    }
    let prev = text[..start_byte].chars().next_back().unwrap_or(' ');
    if prev == ':' {
        return false;
    }
    prev.is_whitespace() || matches!(prev, '(' | '[' | '{' | '"' | '\'' | '<' | '>' | '|')
}

fn is_path_terminator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '`' | '<' | '>' | '[' | ']' | '(' | ')' | '"' | '\'' | '{' | '}'
        )
}

fn is_markdown_report_path_candidate(raw: &str) -> bool {
    let normalized = raw.trim();
    if !(normalized.starts_with('/') || normalized.starts_with("~/")) {
        return false;
    }
    if !normalized.to_ascii_lowercase().ends_with(".md") {
        return false;
    }
    !is_sensitive_rule_markdown_path(normalized)
}

fn is_report_path_context_key(raw_key: &str) -> bool {
    let normalized = normalize_context_key(raw_key);
    normalized.contains("path")
        || normalized == "file"
        || normalized == "files"
        || normalized == "uri"
        || normalized == "uris"
}

fn is_report_path_container_key(raw_key: &str) -> bool {
    let normalized = normalize_context_key(raw_key);
    normalized.contains("artifact")
        || normalized.contains("report")
        || normalized.contains("output")
        || normalized.contains("attachment")
        || normalized == "files"
}

fn normalize_context_key(raw_key: &str) -> String {
    raw_key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn is_sensitive_rule_markdown_path(raw: &str) -> bool {
    let lowered = raw.trim().to_ascii_lowercase();
    let file_name = lowered.rsplit('/').next().unwrap_or_default();
    let sensitive = matches!(
        file_name,
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
    !(lowered.contains("/output/") || lowered.contains("/reports/") || lowered.contains("/report/"))
}

fn append_unique_paths(target: &mut Vec<String>, incoming: Vec<String>) {
    for path in incoming {
        push_unique_path(target, path.as_str());
    }
}

fn push_unique_path(target: &mut Vec<String>, raw: &str) {
    let normalized = raw.trim();
    if normalized.is_empty() {
        return;
    }
    if target.iter().any(|existing| existing == normalized) {
        return;
    }
    target.push(normalized.to_string());
}

fn openclaw_chat_status(payload: &Value) -> &str {
    payload
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
}

fn map_openclaw_slash_gateway_error(err: ChatExecError) -> ChatExecError {
    match err {
        ChatExecError::Failed(reason) if is_openclaw_legacy_chat_api_error(reason.as_str()) => {
            ChatExecError::Failed(OPENCLAW_GATEWAY_UPGRADE_HINT.to_string())
        }
        other => other,
    }
}

async fn run_openclaw_once(
    _request: &ChatRequestInput,
    prompt_text: &str,
    tool: &ToolRuntimePayload,
    route: &OpenClawRoute,
    local_mode: bool,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<OpenClawAttemptResult, ChatExecError> {
    if cancelled(cancel_rx) {
        return Err(ChatExecError::Cancelled);
    }

    let mut command = Command::new("openclaw");
    apply_openclaw_profile(tool, &mut command);
    command
        .arg("agent")
        .arg("--json")
        .arg("-m")
        .arg(prompt_text)
        .arg("--timeout")
        .arg("600");
    if !route.session_id.trim().is_empty() {
        command.arg("--session-id").arg(route.session_id.as_str());
    } else if !route.agent_id.trim().is_empty() {
        command.arg("--agent").arg(route.agent_id.as_str());
    }
    if local_mode {
        command.arg("--local");
    }
    if let Some(workspace) = tool
        .workspace_dir
        .as_ref()
        .filter(|raw| !raw.trim().is_empty())
    {
        command.current_dir(workspace);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = command.spawn().map_err(|err| {
        ChatExecError::Failed(format!("启动 openclaw 失败（local={}）: {err}", local_mode))
    })?;
    let output = collect_child_output_with_cancel(child, cancel_rx).await?;
    if !output.success {
        return Err(ChatExecError::Failed(format!(
            "openclaw 执行失败（local={}）: {}",
            local_mode,
            shorten_error(&output.stderr)
        )));
    }

    let parsed = extract_json_payload(&output.stdout).ok_or_else(|| {
        ChatExecError::Failed(format!("openclaw 返回非 JSON 输出（local={}）", local_mode))
    })?;
    let extracted_text =
        extract_openclaw_text(&parsed).unwrap_or_else(|| compact_json_text(&parsed, 1200));
    let report_paths = collect_markdown_report_paths(&parsed);
    let meta = json!({
        "sessionId": parsed
            .pointer("/result/meta/agentMeta/sessionId")
            .and_then(Value::as_str)
            .or_else(|| parsed.pointer("/meta/agentMeta/sessionId").and_then(Value::as_str))
            .unwrap_or_default(),
        "provider": parsed
            .pointer("/result/meta/agentMeta/provider")
            .and_then(Value::as_str)
            .or_else(|| parsed.pointer("/meta/agentMeta/provider").and_then(Value::as_str))
            .unwrap_or_default(),
        "model": parsed
            .pointer("/result/meta/agentMeta/model")
            .and_then(Value::as_str)
            .or_else(|| parsed.pointer("/meta/agentMeta/model").and_then(Value::as_str))
            .unwrap_or_default(),
        "usage": parsed
            .pointer("/result/meta/agentMeta/usage")
            .cloned()
            .unwrap_or_else(|| json!({})),
        "local": local_mode,
        "reportPaths": report_paths,
    });

    Ok(OpenClawAttemptResult {
        text: extracted_text,
        meta,
    })
}

struct CommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

async fn collect_child_output_with_cancel(
    mut child: Child,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<CommandOutput, ChatExecError> {
    let stdout_task = spawn_reader(child.stdout.take());
    let stderr_task = spawn_stderr_reader(child.stderr.take());

    let status = wait_child_with_cancel(&mut child, cancel_rx).await?;
    let stdout = join_reader_task(stdout_task).await;
    let stderr = join_reader_task(stderr_task).await;

    Ok(CommandOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

fn spawn_reader(reader: Option<tokio::process::ChildStdout>) -> tokio::task::JoinHandle<String> {
    match reader {
        Some(mut stream) => tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        }),
        None => tokio::spawn(async { String::new() }),
    }
}

fn spawn_stderr_reader(
    reader: Option<tokio::process::ChildStderr>,
) -> tokio::task::JoinHandle<String> {
    match reader {
        Some(mut stream) => tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).to_string()
        }),
        None => tokio::spawn(async { String::new() }),
    }
}

async fn join_reader_task(task: tokio::task::JoinHandle<String>) -> String {
    match task.await {
        Ok(text) => text,
        Err(err) => {
            warn!("join reader task failed: {err}");
            String::new()
        }
    }
}

async fn wait_child_with_cancel(
    child: &mut Child,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<std::process::ExitStatus, ChatExecError> {
    loop {
        tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && cancelled(cancel_rx) {
                    kill_child(child).await;
                    return Err(ChatExecError::Cancelled);
                }
            }
            status = child.wait() => {
                return status.map_err(|err| ChatExecError::Failed(format!("等待子进程结束失败: {err}")));
            }
        }
    }
}

async fn kill_child(child: &mut Child) {
    if child.id().is_some() {
        let _ = child.kill().await;
    }
    let _ = child.wait().await;
}

fn cancelled(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

fn emit_started(event_tx: &ChatEventSender, trace_id: Option<String>, request: &ChatRequestInput) {
    emit_chat_event(
        event_tx,
        ChatEventEnvelope {
            event_type: TOOL_CHAT_STARTED_EVENT,
            trace_id,
            payload: json!({
                "toolId": request.tool_id,
                "conversationKey": request.conversation_key,
                "requestId": request.request_id,
                "queueItemId": request.queue_item_id,
                "status": "started",
                "text": "",
                "meta": {},
            }),
            finalize: None,
        },
    );
}

fn emit_chunk(
    event_tx: &ChatEventSender,
    trace_id: Option<String>,
    request: &ChatRequestInput,
    text: &str,
    meta: Value,
) {
    emit_chat_event(
        event_tx,
        ChatEventEnvelope {
            event_type: TOOL_CHAT_CHUNK_EVENT,
            trace_id,
            payload: json!({
                "toolId": request.tool_id,
                "conversationKey": request.conversation_key,
                "requestId": request.request_id,
                "queueItemId": request.queue_item_id,
                "status": "streaming",
                "text": text,
                "meta": meta,
            }),
            finalize: None,
        },
    );
}

fn emit_finished(
    event_tx: &ChatEventSender,
    trace_id: Option<String>,
    request: &ChatRequestInput,
    status: &str,
    text: &str,
    reason: &str,
    meta: Value,
) {
    emit_chat_event(
        event_tx,
        ChatEventEnvelope {
            event_type: TOOL_CHAT_FINISHED_EVENT,
            trace_id,
            payload: json!({
                "toolId": request.tool_id,
                "conversationKey": request.conversation_key,
                "requestId": request.request_id,
                "queueItemId": request.queue_item_id,
                "status": status,
                "text": text,
                "reason": reason,
                "meta": meta,
            }),
            finalize: Some(ChatFinalizeKey {
                conversation_key: request.conversation_key.clone(),
                request_id: request.request_id.clone(),
            }),
        },
    );
}

fn emit_chat_event(event_tx: &ChatEventSender, event: ChatEventEnvelope) {
    if event_tx.send(event).is_err() {
        debug!("chat event channel closed, dropping event");
    }
}

fn is_opencode_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_lowercase();
    let name = tool.name.to_lowercase();
    let vendor = tool.vendor.to_lowercase();
    tool_id.starts_with("opencode_") || name.contains("opencode") || vendor.contains("opencode")
}

fn is_openclaw_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_lowercase();
    let name = tool.name.to_lowercase();
    let vendor = tool.vendor.to_lowercase();
    tool_id.starts_with("openclaw_") || name.contains("openclaw") || vendor.contains("openclaw")
}

fn is_codex_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_lowercase();
    let name = tool.name.to_lowercase();
    let vendor = tool.vendor.to_lowercase();
    tool_id.starts_with("codex_") || name.contains("codex") || vendor.contains("openai")
}

fn is_claude_code_tool(tool: &ToolRuntimePayload) -> bool {
    let tool_id = tool.tool_id.to_lowercase();
    let name = tool.name.to_lowercase();
    let vendor = tool.vendor.to_lowercase();
    tool_id.starts_with("claude_code_")
        || name.contains("claude code")
        || (name == "claude")
        || vendor.contains("anthropic")
}

fn extract_generic_text(value: &Value) -> Option<String> {
    for path in [
        "/text",
        "/output_text",
        "/output",
        "/message",
        "/result",
        "/result/text",
        "/result/output_text",
        "/result/message",
    ] {
        if let Some(text) = value.pointer(path).and_then(Value::as_str) {
            let normalized = text.trim();
            if !normalized.is_empty() {
                return Some(normalized.to_string());
            }
        }
    }
    None
}

fn extract_codex_exec_text(raw: &str) -> Option<String> {
    let mut blocks = Vec::<String>::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let event_type = value.get("type").and_then(Value::as_str).unwrap_or_default();
        if event_type != "item.completed" {
            continue;
        }
        let Some(item) = value.get("item") else {
            continue;
        };
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if item_type != "agent_message" {
            continue;
        }
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if !text.is_empty() {
            blocks.push(text);
        }
    }
    if blocks.is_empty() {
        None
    } else {
        Some(blocks.join("\n"))
    }
}

fn apply_openclaw_profile(tool: &ToolRuntimePayload, command: &mut Command) {
    let profile_key = tool
        .source
        .as_deref()
        .and_then(|source| source.split("profile=").nth(1))
        .map(str::trim)
        .unwrap_or_default();
    if profile_key.is_empty() || profile_key == "default" {
        return;
    }
    if profile_key == "dev" {
        command.arg("--dev");
        return;
    }
    command.arg("--profile").arg(profile_key);
}

fn extract_openclaw_text(payload: &Value) -> Option<String> {
    let mut texts = Vec::new();
    for path in ["/result/payloads", "/payloads"] {
        if let Some(rows) = payload.pointer(path).and_then(Value::as_array) {
            for row in rows {
                for key in ["text", "content", "message", "output"] {
                    if let Some(text) = row.get(key).and_then(Value::as_str) {
                        let normalized = text.trim();
                        if !normalized.is_empty() {
                            texts.push(normalized.to_string());
                        }
                    }
                }
            }
        }
    }
    if !texts.is_empty() {
        return Some(texts.join("\n"));
    }

    let summary = payload
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !summary.is_empty() {
        return Some(summary.to_string());
    }

    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !status.is_empty() {
        return Some(status.to_string());
    }
    None
}

fn extract_openclaw_command_token(raw: &str) -> Option<String> {
    let first_line = raw.trim().lines().next()?.trim();
    if !first_line.starts_with('/') || first_line.len() <= 1 {
        return None;
    }
    let mut token_end = first_line.len();
    for (idx, ch) in first_line.char_indices().skip(1) {
        if ch.is_whitespace() || ch == ':' {
            token_end = idx;
            break;
        }
    }
    if token_end <= 1 {
        return None;
    }
    Some(first_line[..token_end].to_string())
}

fn is_openclaw_known_slash_command(raw: &str) -> bool {
    let Some(token) = extract_openclaw_command_token(raw) else {
        return false;
    };
    let normalized = token.to_ascii_lowercase();
    OPENCLAW_KNOWN_COMMAND_ALIASES
        .iter()
        .any(|candidate| *candidate == normalized)
}

fn resolve_openclaw_session_key(route: &OpenClawRoute) -> String {
    let session_key = route.session_key.trim();
    if !session_key.is_empty() {
        return session_key.to_string();
    }
    let agent_id = route.agent_id.trim();
    if !agent_id.is_empty() {
        return format!("agent:{agent_id}:main");
    }
    "agent:main:main".to_string()
}

fn is_openclaw_legacy_chat_api_error(reason: &str) -> bool {
    let normalized = reason.to_ascii_lowercase();
    normalized.contains("unknown method: chat.send")
        || normalized.contains("invalid chat.send")
        || normalized.contains("unknown method: chat.history")
        || normalized.contains("invalid chat.history")
}

const OPENCLAW_KNOWN_COMMAND_ALIASES: &[&str] = &[
    "/activation",
    "/agents",
    "/allowlist",
    "/approve",
    "/bash",
    "/commands",
    "/compact",
    "/config",
    "/context",
    "/debug",
    "/dock-discord",
    "/dock-slack",
    "/dock-telegram",
    "/dock_discord",
    "/dock_slack",
    "/dock_telegram",
    "/elev",
    "/elevated",
    "/exec",
    "/export",
    "/export-session",
    "/focus",
    "/help",
    "/id",
    "/kill",
    "/model",
    "/models",
    "/new",
    "/queue",
    "/reason",
    "/reasoning",
    "/reset",
    "/restart",
    "/send",
    "/session",
    "/skill",
    "/status",
    "/steer",
    "/stop",
    "/subagents",
    "/t",
    "/tell",
    "/think",
    "/thinking",
    "/tts",
    "/unfocus",
    "/usage",
    "/v",
    "/verbose",
    "/whoami",
];

fn extract_json_payload(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }

    for line in trimmed.lines().rev() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            return Some(value);
        }
    }

    let first = trimmed.find('{')?;
    let candidate = &trimmed[first..];
    serde_json::from_str::<Value>(candidate).ok()
}

fn compact_json_text(value: &Value, limit: usize) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    if text.len() <= limit {
        return text;
    }
    let end = limit.saturating_sub(3);
    format!("{}...", &text[..end])
}

fn shorten_error(raw: &str) -> String {
    let line = raw.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return "未知错误".to_string();
    }
    line.to_string()
}

#[derive(Debug, Default)]
struct OpencodeParsedLine {
    session_id: String,
    chunk_text: Option<String>,
    usage: Option<Value>,
}

fn parse_opencode_line(line: &str) -> Option<OpencodeParsedLine> {
    let parsed = serde_json::from_str::<Value>(line).ok()?;
    let event_type = read_string_any(&parsed, &["type"]);
    let session_id = read_string_any(&parsed, &["sessionID", "sessionId"]);

    if event_type == "text" {
        let text = parsed
            .get("part")
            .and_then(|part| part.get("text"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_default();
        return Some(OpencodeParsedLine {
            session_id,
            chunk_text: if text.trim().is_empty() {
                None
            } else {
                Some(text)
            },
            usage: None,
        });
    }

    if event_type == "step_finish" {
        return Some(OpencodeParsedLine {
            session_id,
            chunk_text: None,
            usage: Some(
                parsed
                    .get("part")
                    .and_then(|part| part.get("tokens"))
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            ),
        });
    }

    Some(OpencodeParsedLine {
        session_id,
        chunk_text: None,
        usage: None,
    })
}

fn decide_openclaw_route(
    result: &Result<OpenClawAttemptResult, ChatExecError>,
) -> OpenClawRouteDecision {
    match result {
        Ok(success) if !success.text.trim().is_empty() => OpenClawRouteDecision::UseChannel,
        Ok(_) | Err(ChatExecError::Failed(_)) => OpenClawRouteDecision::RetryLocal,
        Err(ChatExecError::Cancelled) => OpenClawRouteDecision::Cancelled,
    }
}

fn read_string_any(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ChatExecError, OpenClawAttemptResult, OpenClawHistoryAnchor, OpenClawRoute,
        OpenClawRouteDecision, collect_markdown_report_paths, compact_json_text,
        decide_openclaw_route, extract_json_payload, extract_openclaw_chat_reply_after,
        extract_openclaw_command_token, extract_openclaw_text, is_openclaw_known_slash_command,
        parse_opencode_line, resolve_openclaw_session_key, select_openclaw_recent_session,
        wait_child_with_cancel,
    };

    #[test]
    fn extract_json_payload_should_fallback_to_last_json_line() {
        let raw = "warn line\n{\"status\":\"ok\"}\n";
        let parsed = extract_json_payload(raw).expect("json should be parsed");
        assert_eq!(parsed["status"], "ok");
    }

    #[test]
    fn extract_openclaw_text_prefers_payloads() {
        let payload = json!({
            "result": {
                "payloads": [
                    {"text": "hello"},
                    {"content": "world"}
                ]
            }
        });
        let text = extract_openclaw_text(&payload).expect("text should exist");
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn compact_json_text_should_truncate() {
        let input = json!({"x":"abcdefghijklmnopqrstuvwxyz"});
        let text = compact_json_text(&input, 12);
        assert!(text.ends_with("..."));
    }

    #[test]
    fn parse_opencode_lines_should_collect_chunks_and_usage() {
        let lines = [
            "noise line",
            r#"{"type":"text","sessionID":"sess_1","part":{"text":"Hello"}}"#,
            r#"{"type":"text","part":{"text":" "}}"#,
            r#"{"type":"step_finish","part":{"tokens":{"input":12,"output":34}}}"#,
        ];

        let mut session_id = String::new();
        let mut merged = String::new();
        let mut usage = json!({});
        for raw in lines {
            let Some(parsed) = parse_opencode_line(raw) else {
                continue;
            };
            if session_id.is_empty() && !parsed.session_id.is_empty() {
                session_id = parsed.session_id;
            }
            if let Some(chunk) = parsed.chunk_text {
                merged.push_str(&chunk);
            }
            if let Some(tokens) = parsed.usage {
                usage = tokens;
            }
        }

        assert_eq!(session_id, "sess_1");
        assert_eq!(merged, "Hello");
        assert_eq!(usage["input"], 12);
        assert_eq!(usage["output"], 34);
    }

    #[test]
    fn decide_openclaw_route_should_retry_local_on_failed_or_empty_channel_result() {
        let ok_empty = Ok(OpenClawAttemptResult {
            text: String::new(),
            meta: json!({}),
        });
        assert_eq!(
            decide_openclaw_route(&ok_empty),
            OpenClawRouteDecision::RetryLocal
        );

        let failed = Err(ChatExecError::Failed("boom".to_string()));
        assert_eq!(
            decide_openclaw_route(&failed),
            OpenClawRouteDecision::RetryLocal
        );
    }

    #[test]
    fn slash_command_helpers_should_detect_known_command_tokens() {
        assert_eq!(
            extract_openclaw_command_token("   /compact keep latest"),
            Some("/compact".to_string())
        );
        assert_eq!(
            extract_openclaw_command_token("/status"),
            Some("/status".to_string())
        );
        assert_eq!(
            extract_openclaw_command_token("  /THINK: high"),
            Some("/THINK".to_string())
        );
        assert_eq!(
            extract_openclaw_command_token("/queue: collect"),
            Some("/queue".to_string())
        );
        assert!(extract_openclaw_command_token("status /compact").is_none());
        assert!(!is_openclaw_known_slash_command("hello"));
        assert!(is_openclaw_known_slash_command("/new"));
        assert!(is_openclaw_known_slash_command("   /COMPACT: keep recent"));
        assert!(is_openclaw_known_slash_command("/reasoning on"));
        assert!(!is_openclaw_known_slash_command("/compactx"));
        assert!(!is_openclaw_known_slash_command("/nosuchcmd abc"));
    }

    #[test]
    fn extract_openclaw_chat_reply_after_should_prefer_assistant_message() {
        let payload = json!({
            "messages": [
                {
                    "role": "assistant",
                    "timestamp": 10,
                    "content": [{"type": "text", "text": "old"}]
                },
                {
                    "role": "toolResult",
                    "timestamp": 20,
                    "content": [{"type": "text", "text": "tool output"}]
                },
                {
                    "role": "assistant",
                    "timestamp": 30,
                    "content": [{"type": "text", "text": "final answer"}]
                }
            ]
        });
        let anchor = OpenClawHistoryAnchor {
            latest_timestamp: 15,
        };
        let reply = extract_openclaw_chat_reply_after(&payload, anchor)
            .expect("assistant reply should be extracted");
        assert_eq!(reply.text, "final answer");
    }

    #[test]
    fn extract_openclaw_chat_reply_after_should_fallback_to_tool_then_system() {
        let tool_payload = json!({
            "messages": [
                {"role": "system", "timestamp": 20, "content": [{"type": "text", "text": "system note"}]},
                {"role": "toolResult", "timestamp": 21, "content": [{"type": "text", "text": "tool output"}]}
            ]
        });
        let system_payload = json!({
            "messages": [
                {"role": "system", "timestamp": 30, "content": [{"type": "text", "text": "compacted 1"}]},
                {"role": "system", "timestamp": 31, "content": [{"type": "text", "text": "compacted 2"}]}
            ]
        });
        let anchor = OpenClawHistoryAnchor {
            latest_timestamp: 10,
        };

        let tool_reply = extract_openclaw_chat_reply_after(&tool_payload, anchor)
            .expect("tool reply should be extracted");
        assert_eq!(tool_reply.text, "tool output");

        let system_reply = extract_openclaw_chat_reply_after(&system_payload, anchor)
            .expect("system reply should be extracted");
        assert_eq!(system_reply.text, "compacted 1\ncompacted 2");
    }

    #[test]
    fn extract_openclaw_chat_reply_after_should_collect_report_paths() {
        let payload = json!({
            "messages": [
                {
                    "role": "assistant",
                    "timestamp": 30,
                    "content": [{"type": "text", "text": "已输出报告"}],
                    "artifacts": [
                        {"path":"/Users/codez/.openclaw/agents/main/workspace/output/a.md"},
                        {"path":"~/reports/summary.md"}
                    ]
                }
            ]
        });
        let anchor = OpenClawHistoryAnchor {
            latest_timestamp: 10,
        };
        let reply =
            extract_openclaw_chat_reply_after(&payload, anchor).expect("reply should be extracted");
        assert_eq!(reply.text, "已输出报告");
        assert_eq!(
            reply.report_paths,
            vec![
                "/Users/codez/.openclaw/agents/main/workspace/output/a.md",
                "~/reports/summary.md"
            ]
        );
    }

    #[test]
    fn collect_markdown_report_paths_should_dedupe_and_filter_non_md() {
        let payload = json!({
            "reportPaths":[
                "/tmp/a.md",
                "/tmp/a.md",
                "~/demo/report.MD"
            ],
            "artifacts":[
                {"filePath":"report.txt"},
                {"path":"~/demo/report.MD"}
            ]
        });
        let mut paths = collect_markdown_report_paths(&payload);
        paths.sort();
        assert_eq!(paths, vec!["/tmp/a.md", "~/demo/report.MD"]);
    }

    #[test]
    fn collect_markdown_report_paths_should_filter_sensitive_rule_docs() {
        let payload = json!({
            "reportPaths":[
                "/Users/codez/develop/yourConnector/AGENTS.md",
                "/Users/codez/.openclaw/agents/main/workspace/output/AGENTS.md",
                "/Users/codez/develop/yourConnector/output/real-report.md"
            ]
        });
        let mut paths = collect_markdown_report_paths(&payload);
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "/Users/codez/.openclaw/agents/main/workspace/output/AGENTS.md",
                "/Users/codez/develop/yourConnector/output/real-report.md"
            ]
        );
    }

    #[test]
    fn resolve_openclaw_session_key_should_prefer_status_key() {
        let route = OpenClawRoute {
            session_id: String::new(),
            session_key: "agent:main:main".to_string(),
            agent_id: "main".to_string(),
        };
        assert_eq!(resolve_openclaw_session_key(&route), "agent:main:main");

        let fallback_route = OpenClawRoute {
            session_id: String::new(),
            session_key: String::new(),
            agent_id: "ops".to_string(),
        };
        assert_eq!(
            resolve_openclaw_session_key(&fallback_route),
            "agent:ops:main"
        );
    }

    #[test]
    fn select_openclaw_recent_session_should_skip_system_rows() {
        let status = json!({
            "sessions": {
                "recent": [
                    {
                        "sessionId": "cron_1",
                        "key": "cron:session-cleanup",
                        "agentId": "ops",
                        "systemSent": true
                    },
                    {
                        "sessionId": "user_1",
                        "key": "agent:main:main",
                        "agentId": "main",
                        "systemSent": false
                    }
                ]
            }
        });
        let picked = select_openclaw_recent_session(&status).expect("should pick user session");
        assert_eq!(picked.0, "user_1");
        assert_eq!(picked.1, "agent:main:main");
        assert_eq!(picked.2, "main");
    }

    #[test]
    fn select_openclaw_recent_session_should_return_none_when_only_system_rows() {
        let status = json!({
            "sessions": {
                "recent": [
                    {
                        "sessionId": "cron_1",
                        "key": "cron:session-cleanup",
                        "agentId": "ops",
                        "systemSent": true
                    }
                ]
            }
        });
        assert!(select_openclaw_recent_session(&status).is_none());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn wait_child_with_cancel_should_kill_process_and_return_cancelled() {
        use std::{process::Stdio, time::Duration};
        use tokio::{process::Command, sync::watch, time::sleep};

        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 5")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep child");

        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let cancel_task = tokio::spawn(async move {
            sleep(Duration::from_millis(80)).await;
            let _ = cancel_tx.send(true);
        });

        let outcome = wait_child_with_cancel(&mut child, &mut cancel_rx).await;
        cancel_task.await.expect("cancel task should finish");

        assert!(matches!(outcome, Err(ChatExecError::Cancelled)));
        assert!(child.try_wait().expect("query child status").is_some());
    }
}
