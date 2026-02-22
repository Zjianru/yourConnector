// 文件职责：
// 1. 统一链路日志摘要与原文输出策略。
// 2. 提供日志队列写入函数，控制日志长度上限。

import { asMap } from "./type.js";

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
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") {
      return "non-object message";
    }
    const type = String(parsed.type || "unknown");
    const payload = asMap(parsed.payload);
    const toolId = String(payload.toolId || "");
    const status = String(payload.status || "");
    const action = String(payload.action || "");
    const detail = [
      toolId ? `tool=${toolId}` : "",
      status ? `status=${status}` : "",
      action ? `action=${action}` : "",
    ]
      .filter(Boolean)
      .join(" ");
    return detail ? `${type} ${detail}` : type;
  } catch (_) {
    return "non-json message";
  }
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
 * 写入日志并限制最大条数。
 * @param {object} state 全局状态。
 * @param {string} text 日志文本。
 * @param {number} maxCount 最多保留条数。
 */
export function addLog(state, text, maxCount = 300) {
  const line = `[${new Date().toISOString()}] ${text}`;
  state.logs.unshift(line);
  if (state.logs.length > maxCount) {
    state.logs.length = maxCount;
  }
}
