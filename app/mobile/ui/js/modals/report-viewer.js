// 文件职责：
// 1. 管理报告预览弹窗显示与关闭行为。
// 2. 根据聊天状态渲染进度、错误信息与 Markdown 内容。

import { renderMarkdown } from "../utils/markdown.js";

function ensureReportViewerState(state) {
  if (!state.chat || typeof state.chat !== "object") {
    return {
      visible: false,
      filePath: "",
      content: "",
      status: "idle",
      error: "",
      bytesSent: 0,
      bytesTotal: 0,
    };
  }
  if (!state.chat.reportViewer || typeof state.chat.reportViewer !== "object") {
    state.chat.reportViewer = {
      visible: false,
      conversationKey: "",
      hostId: "",
      toolId: "",
      requestId: "",
      filePath: "",
      content: "",
      status: "idle",
      error: "",
      bytesSent: 0,
      bytesTotal: 0,
    };
  }
  return state.chat.reportViewer;
}

function formatBytes(bytes) {
  const value = Number(bytes || 0);
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  if (value < 1024) return `${Math.round(value)} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / (1024 * 1024)).toFixed(2)} MB`;
}

/**
 * 创建报告查看弹窗能力。
 * @param {{state: object, ui: object}} deps 依赖集合。
 */
export function createReportViewerModal({ state, ui }) {
  function closeReportViewer() {
    const viewer = ensureReportViewerState(state);
    viewer.visible = false;
    viewer.conversationKey = "";
    viewer.hostId = "";
    viewer.toolId = "";
    viewer.requestId = "";
    viewer.filePath = "";
    viewer.content = "";
    viewer.status = "idle";
    viewer.error = "";
    viewer.bytesSent = 0;
    viewer.bytesTotal = 0;
    ui.reportViewerModal.classList.remove("show");
  }

  function renderReportViewer() {
    const viewer = ensureReportViewerState(state);
    const visible = Boolean(viewer.visible);
    ui.reportViewerModal.classList.toggle("show", visible);
    if (!visible) {
      ui.reportViewerPath.textContent = "";
      ui.reportViewerError.textContent = "";
      ui.reportViewerBody.innerHTML = "";
      ui.reportViewerProgressBar.style.width = "0%";
      ui.reportViewerProgressLabel.textContent = "0%";
      return;
    }

    const filePath = String(viewer.filePath || "").trim();
    ui.reportViewerPath.textContent = filePath || "正在读取报告…";
    ui.reportViewerError.textContent = String(viewer.error || "");

    const bytesSent = Math.max(0, Number(viewer.bytesSent || 0));
    const bytesTotal = Math.max(0, Number(viewer.bytesTotal || 0));
    const percent = bytesTotal > 0
      ? Math.min(100, Math.max(0, Math.round((bytesSent / bytesTotal) * 100)))
      : (String(viewer.status || "") === "completed" ? 100 : 0);
    ui.reportViewerProgressBar.style.width = `${percent}%`;
    if (bytesTotal > 0) {
      ui.reportViewerProgressLabel.textContent = `${formatBytes(bytesSent)} / ${formatBytes(bytesTotal)} (${percent}%)`;
    } else if (bytesSent > 0) {
      ui.reportViewerProgressLabel.textContent = `${formatBytes(bytesSent)} 已接收`;
    } else {
      ui.reportViewerProgressLabel.textContent = `${percent}%`;
    }

    const content = String(viewer.content || "");
    if (content) {
      ui.reportViewerBody.innerHTML = renderMarkdown(content);
      return;
    }

    const status = String(viewer.status || "").toLowerCase();
    if ((status === "failed" || status === "busy") && viewer.error) {
      ui.reportViewerBody.innerHTML = "";
      return;
    }
    ui.reportViewerBody.innerHTML = '<div class="empty">正在拉取报告内容…</div>';
  }

  function bindReportViewerModalEvents() {
    ui.reportViewerClose.addEventListener("click", closeReportViewer);
    ui.reportViewerModal.addEventListener("click", (event) => {
      if (event.target === ui.reportViewerModal) {
        closeReportViewer();
      }
    });
  }

  return {
    closeReportViewer,
    renderReportViewer,
    bindReportViewerModalEvents,
  };
}
