// 文件职责：
// 1. 提供 DOM 层可复用的安全转义函数。
// 2. 统一动态文本输出，避免注入与布局破坏。

export function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}
