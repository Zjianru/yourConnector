// 文件职责：
// 1. 统一链路日志摘要与原文输出策略。
// 2. 提供日志队列写入函数，控制日志长度上限。

import { asMap } from "./type.js";

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
    const detail = [toolId ? `tool=${toolId}` : "", status ? `status=${status}` : "", action ? `action=${action}` : ""]
      .filter(Boolean)
      .join(" ");
    return detail ? `${type} ${detail}` : type;
  } catch (_) {
    return "non-json message";
  }
}

export function formatWireLog(direction, hostName, rawText, rawPayloadDebug = false) {
  const host = String(hostName || "--");
  if (rawPayloadDebug) {
    return `${direction}[${host}] ${String(rawText || "")}`;
  }
  return `${direction}[${host}] ${summarizeWirePayload(rawText)}`;
}

export function addLog(state, text, maxCount = 300) {
  const line = `[${new Date().toISOString()}] ${text}`;
  state.logs.unshift(line);
  if (state.logs.length > maxCount) {
    state.logs.length = maxCount;
  }
}
