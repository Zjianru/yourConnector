// 文件职责：
// 1. 提供基础类型归一化工具，减少各流程对动态数据的分支判断。
// 2. 统一对象/数组/布尔值解析规则，避免协议字段兼容分歧。

/**
 * 将任意值归一化为普通对象。
 * @param {unknown} value 原始值。
 * @returns {Record<string, any>}
 */
export function asMap(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

/**
 * 将任意值归一化为对象数组。
 * @param {unknown} value 原始值。
 * @returns {Array<Record<string, any>>}
 */
export function asListOfMap(value) {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.filter((item) => item && typeof item === "object" && !Array.isArray(item));
}

/**
 * 宽松布尔解析。
 * @param {unknown} value 原始值。
 * @returns {boolean}
 */
export function asBool(value) {
  if (typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    return value !== 0;
  }
  if (typeof value === "string") {
    const lower = value.toLowerCase();
    return lower === "1" || lower === "true" || lower === "yes" || lower === "on";
  }
  return false;
}
