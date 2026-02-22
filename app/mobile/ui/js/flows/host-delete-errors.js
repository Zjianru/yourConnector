// 文件职责：
// 1. 提供宿主机删除补偿链路使用的错误码与归一化工具。
// 2. 统一删除补偿的终态/重试态判定规则。

import { state } from "../state/store.js";

/** 删除补偿中的终态 Relay 错误码集合。 */
export const DELETE_TERMINAL_RELAY_CODES = new Set([
  "SYSTEM_NOT_REGISTERED",
  "DEVICE_REVOKED",
  "DEVICE_NOT_FOUND",
  "REFRESH_TOKEN_INVALID",
  "REFRESH_TOKEN_EXPIRED",
]);

/**
 * 解析补偿条目的设备标识。
 * @param {object} item 补偿条目。
 * @returns {string}
 */
export function pendingDeleteDeviceId(item) {
  return String((item && item.deviceId) || state.deviceId || "").trim();
}

/**
 * 构建带 code 的 Error。
 * @param {string} code 业务错误码。
 * @param {string} message 错误信息。
 * @returns {Error}
 */
export function errorWithCode(code, message) {
  const err = new Error(String(message || "unexpected error"));
  err.code = String(code || "").trim();
  return err;
}

/**
 * 归一化删除补偿错误码。
 * @param {unknown} error 错误对象。
 * @returns {string}
 */
export function normalizeDeleteCompensationErrorCode(error) {
  const directCode = String(error && error.code ? error.code : "").trim();
  if (["DELETE_COMPENSATION_STALE", "DELETE_COMPENSATION_TERMINAL", "DELETE_COMPENSATION_NO_SESSION"].includes(directCode)) {
    return directCode;
  }
  const token = String(error || "").match(/\b[A-Z][A-Z_]{2,}\b/);
  if (token && DELETE_TERMINAL_RELAY_CODES.has(token[0])) {
    return "DELETE_COMPENSATION_TERMINAL";
  }
  return directCode;
}
