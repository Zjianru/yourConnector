//! 聊天执行器：
//! 1. 维护单会话（conversationKey）单活跃任务。
//! 2. 按工具类型执行 OpenCode/OpenClaw 命令并转为统一事件。
//! 3. 支持取消运行中任务并在完成后释放会话占用。

use std::{collections::HashMap, process::Stdio, time::Duration};

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

use crate::control::{TOOL_CHAT_CHUNK_EVENT, TOOL_CHAT_FINISHED_EVENT, TOOL_CHAT_STARTED_EVENT};

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

    if is_opencode_tool(tool) {
        return run_opencode_request(request, tool, trace_id, event_tx, cancel_rx).await;
    }
    if is_openclaw_tool(tool) {
        return run_openclaw_request(request, tool, trace_id, event_tx, cancel_rx).await;
    }
    Err(ChatExecError::Failed(
        "当前工具类型不支持聊天执行".to_string(),
    ))
}

/// OpenCode: 使用 `opencode run --format json` 并按 text 事件流式回传。
async fn run_opencode_request(
    request: &ChatRequestInput,
    tool: &ToolRuntimePayload,
    trace_id: &Option<String>,
    event_tx: &ChatEventSender,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<ChatExecutionResult, ChatExecError> {
    let mut command = Command::new("opencode");
    command
        .arg("run")
        .arg(request.text.as_str())
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
        return Err(ChatExecError::Failed(format!(
            "opencode 执行失败: {}",
            shorten_error(&stderr)
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

/// OpenClaw: 渠道优先；失败或文本不可提取时回退 `--local`。
async fn run_openclaw_request(
    request: &ChatRequestInput,
    tool: &ToolRuntimePayload,
    trace_id: &Option<String>,
    event_tx: &ChatEventSender,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<ChatExecutionResult, ChatExecError> {
    let route = resolve_openclaw_route(tool).await;
    let result = if is_openclaw_slash_command(request.text.as_str()) {
        match run_openclaw_slash_request(request, tool, &route, cancel_rx).await {
            Ok(ok) => ok,
            Err(ChatExecError::Failed(reason))
                if should_fallback_openclaw_slash_to_agent(reason.as_str()) =>
            {
                run_openclaw_agent_request(request, tool, &route, cancel_rx).await?
            }
            Err(err) => return Err(err),
        }
    } else {
        run_openclaw_agent_request(request, tool, &route, cancel_rx).await?
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

#[derive(Debug, Clone)]
enum OpenClawControlCommand {
    Compact { instructions: Option<String> },
}

const OPENCLAW_CHAT_HISTORY_LIMIT: usize = 120;
const OPENCLAW_CHAT_POLL_INTERVAL: Duration = Duration::from_millis(700);
const OPENCLAW_CHAT_POLL_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Copy, Default)]
struct OpenClawHistoryAnchor {
    latest_timestamp: i64,
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
            session_id = value
                .pointer("/sessions/recent/0/sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            session_key = value
                .pointer("/sessions/recent/0/key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
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

async fn run_openclaw_agent_request(
    request: &ChatRequestInput,
    tool: &ToolRuntimePayload,
    route: &OpenClawRoute,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<OpenClawAttemptResult, ChatExecError> {
    let channel_attempt = run_openclaw_once(request, tool, route, false, cancel_rx).await;
    match decide_openclaw_route(&channel_attempt) {
        OpenClawRouteDecision::UseChannel => match channel_attempt {
            Ok(ok) => Ok(ok),
            Err(_) => Err(ChatExecError::Failed(
                "openclaw route decision mismatch".to_string(),
            )),
        },
        OpenClawRouteDecision::RetryLocal => {
            run_openclaw_once(request, tool, route, true, cancel_rx).await
        }
        OpenClawRouteDecision::Cancelled => Err(ChatExecError::Cancelled),
    }
}

async fn run_openclaw_slash_request(
    request: &ChatRequestInput,
    tool: &ToolRuntimePayload,
    route: &OpenClawRoute,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<OpenClawAttemptResult, ChatExecError> {
    if cancelled(cancel_rx) {
        return Err(ChatExecError::Cancelled);
    }

    let command_token =
        extract_openclaw_command_token(request.text.as_str()).unwrap_or_else(|| "/".to_string());
    let compact_command = parse_openclaw_control_command(request.text.as_str());
    let session_key = resolve_openclaw_compact_session_key(route);
    let history_anchor = run_openclaw_chat_history(tool, session_key.as_str(), cancel_rx)
        .await
        .map(|payload| capture_openclaw_history_anchor(&payload))
        .unwrap_or_default();
    let run_id = format!("sidecar-chat-{}", Uuid::new_v4());

    let mut send_payload = run_openclaw_chat_send(
        tool,
        session_key.as_str(),
        request.text.as_str(),
        run_id.as_str(),
        cancel_rx,
    )
    .await?;
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
            request.text.as_str(),
            run_id.as_str(),
            cancel_rx,
        )
        .await?;
        status = openclaw_chat_status(&send_payload).to_string();
    }

    if status == "error" {
        let summary = send_payload
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        if let Some(command) = compact_command.as_ref() {
            return run_openclaw_control_command(command, tool, route, cancel_rx).await;
        }
        return Err(ChatExecError::Failed(format!(
            "openclaw slash command 执行失败（{command_token}）: {summary}"
        )));
    }

    if status != "ok" {
        if let Some(command) = compact_command.as_ref() {
            let _ = run_openclaw_chat_abort(tool, session_key.as_str(), run_id.as_str()).await;
            return run_openclaw_control_command(command, tool, route, cancel_rx).await;
        }

        let text = format!(
            "命令 {command_token} 已下发，当前仍在执行中，请稍后在 OpenClaw 会话中查看结果。"
        );
        return Ok(OpenClawAttemptResult {
            text,
            meta: json!({
                "command": command_token,
                "runId": run_id,
                "sessionKey": session_key,
                "status": status,
                "source": "gateway.chat.send",
            }),
        });
    }

    let history_payload = run_openclaw_chat_history(tool, session_key.as_str(), cancel_rx).await?;
    if let Some(reply_text) = extract_openclaw_chat_reply_after(&history_payload, history_anchor) {
        return Ok(OpenClawAttemptResult {
            text: reply_text,
            meta: json!({
                "command": command_token,
                "runId": run_id,
                "sessionKey": session_key,
                "status": status,
                "source": "gateway.chat.send",
            }),
        });
    }

    if let Some(command) = compact_command.as_ref() {
        return run_openclaw_control_command(command, tool, route, cancel_rx).await;
    }

    let text = format!("已执行 {command_token}。");
    Ok(OpenClawAttemptResult {
        text,
        meta: json!({
            "command": command_token,
            "runId": run_id,
            "sessionKey": session_key,
            "status": status,
            "source": "gateway.chat.send",
        }),
    })
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
) -> Option<String> {
    let rows = payload.get("messages").and_then(Value::as_array)?;
    let mut assistant_reply: Option<String> = None;
    let mut tool_reply: Option<String> = None;

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
        let role = row.get("role").and_then(Value::as_str).unwrap_or_default();
        if role == "assistant" {
            assistant_reply = Some(text);
        } else if role == "toolResult" {
            tool_reply = Some(text);
        }
    }

    assistant_reply.or(tool_reply)
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

fn openclaw_chat_status(payload: &Value) -> &str {
    payload
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
}

async fn run_openclaw_control_command(
    command: &OpenClawControlCommand,
    tool: &ToolRuntimePayload,
    route: &OpenClawRoute,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<OpenClawAttemptResult, ChatExecError> {
    match command {
        OpenClawControlCommand::Compact { instructions } => {
            run_openclaw_compact_command(tool, route, instructions.as_deref(), cancel_rx).await
        }
    }
}

async fn run_openclaw_compact_command(
    tool: &ToolRuntimePayload,
    route: &OpenClawRoute,
    instructions: Option<&str>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<OpenClawAttemptResult, ChatExecError> {
    if cancelled(cancel_rx) {
        return Err(ChatExecError::Cancelled);
    }

    let session_key = resolve_openclaw_compact_session_key(route);
    let params = json!({
        "key": session_key,
    });

    let mut command = Command::new("openclaw");
    apply_openclaw_profile(tool, &mut command);
    command
        .arg("gateway")
        .arg("call")
        .arg("sessions.compact")
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
        .map_err(|err| ChatExecError::Failed(format!("启动 openclaw compact 失败: {err}")))?;
    let output = collect_child_output_with_cancel(child, cancel_rx).await?;
    if !output.success {
        return Err(ChatExecError::Failed(format!(
            "openclaw compact 执行失败: {}",
            shorten_error(&output.stderr)
        )));
    }

    let parsed = extract_json_payload(&output.stdout)
        .ok_or_else(|| ChatExecError::Failed("openclaw compact 返回非 JSON 输出".to_string()))?;
    if !parsed.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        let reason = parsed
            .get("reason")
            .and_then(Value::as_str)
            .filter(|raw| !raw.trim().is_empty())
            .unwrap_or("unknown reason");
        return Err(ChatExecError::Failed(format!(
            "openclaw compact 返回失败: {reason}"
        )));
    }

    let compacted = parsed
        .get("compacted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let kept = parsed
        .get("kept")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let reason = parsed
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = format_openclaw_compact_reply(compacted, kept, reason, instructions);
    let meta = json!({
        "command": "compact",
        "sessionKey": parsed
            .get("key")
            .and_then(Value::as_str)
            .filter(|raw| !raw.trim().is_empty())
            .unwrap_or(session_key.as_str()),
        "compacted": compacted,
        "kept": kept,
        "reason": reason,
        "instructions": instructions.unwrap_or(""),
        "instructionsIgnored": instructions
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
    });

    Ok(OpenClawAttemptResult { text, meta })
}

async fn run_openclaw_once(
    request: &ChatRequestInput,
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
        .arg(request.text.as_str())
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
    let trimmed = raw.trim();
    if !trimmed.starts_with('/') || trimmed.len() <= 1 {
        return None;
    }
    let token = trimmed.split_whitespace().next().unwrap_or_default().trim();
    if token.len() <= 1 || !token.starts_with('/') {
        return None;
    }
    Some(token.to_string())
}

fn is_openclaw_slash_command(raw: &str) -> bool {
    extract_openclaw_command_token(raw).is_some()
}

fn parse_openclaw_control_command(raw: &str) -> Option<OpenClawControlCommand> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.to_ascii_lowercase().starts_with("/compact") {
        return None;
    }
    let rest = &trimmed["/compact".len()..];
    if let Some(first) = rest.chars().next()
        && first != ':'
        && !first.is_whitespace()
    {
        return None;
    }

    let instructions_raw = if let Some(stripped) = rest.trim_start().strip_prefix(':') {
        stripped.trim()
    } else {
        rest.trim()
    };
    let instructions = if instructions_raw.is_empty() {
        None
    } else {
        Some(instructions_raw.to_string())
    };
    Some(OpenClawControlCommand::Compact { instructions })
}

fn resolve_openclaw_compact_session_key(route: &OpenClawRoute) -> String {
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

fn should_fallback_openclaw_slash_to_agent(reason: &str) -> bool {
    let normalized = reason.to_ascii_lowercase();
    normalized.contains("unknown method: chat.send")
        || normalized.contains("invalid chat.send")
        || normalized.contains("unknown method: chat.history")
        || normalized.contains("invalid chat.history")
}

fn format_openclaw_compact_reply(
    compacted: bool,
    kept: i64,
    reason: &str,
    instructions: Option<&str>,
) -> String {
    let mut text = if compacted {
        if kept > 0 {
            format!("已执行 /compact：会话已压缩，当前保留最近 {kept} 行。")
        } else {
            "已执行 /compact：会话已压缩。".to_string()
        }
    } else {
        match reason.trim() {
            "no sessionId" => "已执行 /compact：当前会话缺少 sessionId，未发生压缩。".to_string(),
            "no transcript" => "已执行 /compact：未找到会话转录文件，未发生压缩。".to_string(),
            _ if kept > 0 => format!(
                "已执行 /compact：当前会话已在压缩范围内（保留 {kept} 行），无需进一步压缩。"
            ),
            _ => "已执行 /compact：当前会话未发生压缩。".to_string(),
        }
    };

    if instructions
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        text.push_str("\n提示：当前 compact 接口不支持附加说明，已按默认策略执行。");
    }
    text
}

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
        ChatExecError, OpenClawAttemptResult, OpenClawControlCommand, OpenClawHistoryAnchor,
        OpenClawRoute, OpenClawRouteDecision, compact_json_text, decide_openclaw_route,
        extract_json_payload, extract_openclaw_chat_reply_after, extract_openclaw_command_token,
        extract_openclaw_text, format_openclaw_compact_reply, is_openclaw_slash_command,
        parse_openclaw_control_command, parse_opencode_line, resolve_openclaw_compact_session_key,
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
    fn parse_openclaw_control_command_should_detect_compact() {
        let parsed = parse_openclaw_control_command("/compact");
        assert!(matches!(
            parsed,
            Some(OpenClawControlCommand::Compact { instructions: None })
        ));

        let parsed_with_args = parse_openclaw_control_command("/COMPACT: keep latest tasks");
        match parsed_with_args {
            Some(OpenClawControlCommand::Compact { instructions }) => {
                assert_eq!(instructions.as_deref(), Some("keep latest tasks"));
            }
            _ => panic!("compact with args should be parsed"),
        }

        assert!(parse_openclaw_control_command("/compactx").is_none());
        assert!(parse_openclaw_control_command("hello /compact").is_none());
    }

    #[test]
    fn slash_command_helpers_should_detect_and_extract_token() {
        assert_eq!(
            extract_openclaw_command_token("   /compact keep latest"),
            Some("/compact".to_string())
        );
        assert_eq!(
            extract_openclaw_command_token("/status"),
            Some("/status".to_string())
        );
        assert!(extract_openclaw_command_token("status /compact").is_none());
        assert!(!is_openclaw_slash_command("hello"));
        assert!(is_openclaw_slash_command("/new"));
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
        let text = extract_openclaw_chat_reply_after(&payload, anchor)
            .expect("assistant reply should be extracted");
        assert_eq!(text, "final answer");
    }

    #[test]
    fn resolve_openclaw_compact_session_key_should_prefer_status_key() {
        let route = OpenClawRoute {
            session_id: String::new(),
            session_key: "agent:main:main".to_string(),
            agent_id: "main".to_string(),
        };
        assert_eq!(
            resolve_openclaw_compact_session_key(&route),
            "agent:main:main"
        );

        let fallback_route = OpenClawRoute {
            session_id: String::new(),
            session_key: String::new(),
            agent_id: "ops".to_string(),
        };
        assert_eq!(
            resolve_openclaw_compact_session_key(&fallback_route),
            "agent:ops:main"
        );
    }

    #[test]
    fn format_openclaw_compact_reply_should_mark_ignored_instructions() {
        let text = format_openclaw_compact_reply(false, 120, "", Some("only keep todos"));
        assert!(text.contains("不支持附加说明"));
        assert!(text.contains("120"));
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
