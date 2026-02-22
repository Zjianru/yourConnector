// 文件职责：
// 1. 提供 key/value 行渲染工具。
// 2. 统一多处详情面板的行模板输出。

import { escapeHtml } from "./dom.js";

/**
 * 将 key/value 数组渲染为详情行 HTML。
 * @param {Array<[string, string]>} rows 行数据。
 * @returns {string}
 */
export function renderRows(rows) {
  return rows
    .map(
      ([key, value]) => `
        <div class="row">
          <div class="k">${escapeHtml(String(key))}</div>
          <div class="v">${escapeHtml(String(value ?? "--"))}</div>
        </div>
      `,
    )
    .join("");
}
