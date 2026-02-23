// 文件职责：
// 1. 统一链路日志摘要、结构化操作日志和 traceId 生成规则。
// 2. 提供 UI 可读日志（state.logs）与可检索结构化日志（state.operationLogs）双写能力。
// 3. 对原始 payload 默认做摘要，避免日志泄漏完整业务正文。

import { asMap } from "./type.js";

const DEFAULT_TEXT_LOG_LIMIT = 300;
const DEFAULT_OPERATION_LOG_LIMIT = 1500;

/**
 * 生成链路 traceId，便于串联 mobile/relay/sidecar 日志。
 * @returns {string}
 */
export function createTraceId() {
  if (window.crypto && typeof window.crypto.randomUUID === "function") {
    return `trc_${window.crypto.randomUUID()}`;
  }
  const rand = Math.random().toString(36).slice(2, 10);
  return `trc_${Date.now()}_${rand}`;
}

/**
 * 从协议报文中提取链路元数据。
 * @param {unknown} rawText 原始报文字符串。
 * @returns {Record<string, string|number>}
 */
export function extractWireMeta(rawText) {
  const raw = String(rawText || "");
  if (!raw) {
    return {};
  }
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") {
      return {};
    }
    const payload = asMap(parsed.payload);
    const seq = Number(parsed.seq || 0);
    return {
      eventId: String(parsed.eventId || ""),
      traceId: String(parsed.traceId || ""),
      eventType: String(parsed.type || ""),
      systemId: String(parsed.systemId || ""),
      sourceClientType: String(parsed.sourceClientType || ""),
      sourceDeviceId: String(parsed.sourceDeviceId || ""),
      toolId: String(payload.toolId || ""),
      seq: Number.isFinite(seq) && seq > 0 ? seq : 0,
    };
  } catch (_) {
    return {};
  }
}

/**
 * 生成协议报文摘要，避免日志默认输出完整原文。
 * @param {unknown} rawText 原始报文字符串。
 * @returns {string}
 */
export function summarizeWirePayload(rawText) {
  const raw = String(rawText || "");
  if (!raw) {
    return "empty";
  }
  const wire = extractWireMeta(raw);
  if (!wire.eventType) {
    return "non-json message";
  }

  const detail = [
    wire.toolId ? `tool=${wire.toolId}` : "",
    wire.eventId ? `event_id=${wire.eventId}` : "",
    wire.traceId ? `trace_id=${wire.traceId}` : "",
  ]
    .filter(Boolean)
    .join(" ");
  return detail ? `${wire.eventType} ${detail}` : wire.eventType;
}

/**
 * 格式化链路日志文本。
 * @param {"IN"|"OUT"} direction 方向。
 * @param {string} hostName 宿主机名称。
 * @param {unknown} rawText 原文。
 * @param {boolean} rawPayloadDebug 是否输出原文。
 * @returns {string}
 */
export function formatWireLog(direction, hostName, rawText, rawPayloadDebug = false) {
  const host = String(hostName || "--");
  if (rawPayloadDebug) {
    return `${direction}[${host}] ${String(rawText || "")}`;
  }
  return `${direction}[${host}] ${summarizeWirePayload(rawText)}`;
}

/**
 * 归一化结构化日志字段，避免写入无效值。
 * @param {Record<string, any>} options 原始选项。
 * @returns {Record<string, any>}
 */
function normalizeOperationOptions(options = {}) {
  const source = String(options.source || "mobile");
  const level = String(options.level || "info");
  const outcome = String(options.outcome || "info");
  return {
    level,
    scope: String(options.scope || "app"),
    action: String(options.action || ""),
    outcome,
    source,
    direction: String(options.direction || ""),
    traceId: String(options.traceId || ""),
    eventId: String(options.eventId || ""),
    eventType: String(options.eventType || ""),
    hostId: String(options.hostId || ""),
    hostName: String(options.hostName || ""),
    toolId: String(options.toolId || ""),
    systemId: String(options.systemId || ""),
    sourceClientType: String(options.sourceClientType || ""),
    sourceDeviceId: String(options.sourceDeviceId || ""),
    seq: Number(options.seq || 0),
    detail: String(options.detail || ""),
  };
}

/**
 * 写入日志并限制最大条数（文本 + 结构化）。
 * @param {object} state 全局状态。
 * @param {string} text 日志文本。
 * @param {Record<string, any>} options 结构化日志字段。
 */
export function addLog(state, text, options = {}) {
  const normalized = normalizeOperationOptions(options);
  const ts = new Date().toISOString();
  const traceHint = normalized.traceId ? ` trace=${normalized.traceId}` : "";
  const line = `[${ts}] ${text}${traceHint}`;
  state.logs.unshift(line);
  if (state.logs.length > DEFAULT_TEXT_LOG_LIMIT) {
    state.logs.length = DEFAULT_TEXT_LOG_LIMIT;
  }

  state.operationLogs.unshift({
    ts,
    level: normalized.level,
    scope: normalized.scope,
    action: normalized.action,
    outcome: normalized.outcome,
    source: normalized.source,
    direction: normalized.direction,
    traceId: normalized.traceId,
    eventId: normalized.eventId,
    eventType: normalized.eventType,
    hostId: normalized.hostId,
    hostName: normalized.hostName,
    toolId: normalized.toolId,
    systemId: normalized.systemId,
    sourceClientType: normalized.sourceClientType,
    sourceDeviceId: normalized.sourceDeviceId,
    seq: normalized.seq > 0 ? normalized.seq : undefined,
    message: String(text || ""),
    detail: normalized.detail,
  });
  if (state.operationLogs.length > DEFAULT_OPERATION_LOG_LIMIT) {
    state.operationLogs.length = DEFAULT_OPERATION_LOG_LIMIT;
  }
}
