// 文件职责：
// 1. 宿主机指标展示格式化（MB->GB、运行时长）。
// 2. 工具类别中文化展示。

import { fmt2 } from "./format.js";

/**
 * MB 转 GB 字符串。
 * @param {unknown} value MB 数值。
 * @returns {string}
 */
export function formatGbFromMb(value) {
  const mb = Number(value);
  if (!Number.isFinite(mb)) {
    return "--";
  }
  return fmt2(mb / 1024);
}

/**
 * 秒级时长转短文本。
 * @param {unknown} value 秒数。
 * @returns {string}
 */
export function formatDurationShort(value) {
  const sec = Number(value);
  if (!Number.isFinite(sec) || sec < 0) {
    return "--";
  }
  const total = Math.floor(sec);
  const day = Math.floor(total / 86400);
  const hour = Math.floor((total % 86400) / 3600);
  const minute = Math.floor((total % 3600) / 60);
  if (day > 0) {
    return `${day}天 ${hour}小时`;
  }
  if (hour > 0) {
    return `${hour}小时 ${minute}分钟`;
  }
  return `${minute}分钟`;
}

/**
 * 工具类别中文映射。
 * @param {unknown} rawValue 原始类别。
 * @returns {string}
 */
export function localizedCategory(rawValue) {
  const raw = String(rawValue || "");
  if (raw === "CODE_AGENT") {
    return "代码助手";
  }
  if (raw === "DEV_WORKER") {
    return "开发工具";
  }
  if (raw === "UNKNOWN") {
    return "未知";
  }
  return raw || "--";
}
