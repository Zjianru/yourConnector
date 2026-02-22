// 文件职责：
// 1. 管理“添加 AI 工具”弹窗的打开、关闭与候选渲染。
// 2. 统一候选接入交互，避免主流程直接操作弹窗 DOM。

import { asBool } from "../utils/type.js";
import { escapeHtml } from "../utils/dom.js";
import { localizedCategory } from "../utils/host-format.js";

/**
 * 创建“添加工具”弹窗能力。
 * @param {object} deps 依赖集合。
 */
export function createAddToolModal({ state, ui, hostById, ensureRuntime, requestToolsRefresh, resolveToolDisplayName }) {
  let onConnectCandidateTool = null;
  let onOpenDebug = null;

  function openAddToolModal(hostId) {
    const host = hostById(hostId);
    if (!host) {
      return;
    }
    state.addToolHostId = hostId;
    ui.addToolModal.classList.add("show");
    requestToolsRefresh(hostId);
    renderAddToolModal();
  }

  function closeAddToolModal() {
    ui.addToolModal.classList.remove("show");
    state.addToolHostId = "";
  }

  function onCandidateListClick(event) {
    const btn = event.target.closest("[data-connect-tool-id]");
    if (!btn || !state.addToolHostId) {
      return;
    }
    const toolId = String(btn.getAttribute("data-connect-tool-id") || "");
    if (toolId && typeof onConnectCandidateTool === "function") {
      onConnectCandidateTool(state.addToolHostId, toolId);
      renderAddToolModal();
    }
  }

  function renderAddToolModal() {
    if (!ui.addToolModal.classList.contains("show")) {
      return;
    }

    const host = hostById(state.addToolHostId);
    const runtime = ensureRuntime(state.addToolHostId);
    if (!host || !runtime) {
      ui.candidateList.innerHTML = '<div class="empty">未找到宿主机，请重新打开。</div>';
      return;
    }

    if (!runtime.connected) {
      ui.candidateList.innerHTML = '<div class="empty">请先连接该宿主机，再添加工具。</div>';
      return;
    }

    if (runtime.candidateTools.length === 0) {
      ui.candidateList.innerHTML = runtime.tools.length > 0
        ? '<div class="empty">当前没有候选工具。已发现的工具可能已接入（候选列表仅展示未接入工具）。</div>'
        : '<div class="empty">当前没有候选工具。请确认宿主机已运行 opencode/openclaw，并等待一次快照刷新。</div>';
      return;
    }

    ui.candidateList.innerHTML = runtime.candidateTools
      .map((tool) => {
        const toolId = String(tool.toolId || "");
        const title = resolveToolDisplayName(state.addToolHostId, tool);
        const connecting = asBool(runtime.connectingToolIds[toolId]);
        const reason = String(tool.reason || "已发现可接入进程");
        return `
          <article class="candidate-item">
            <div class="candidate-head">
              <div class="candidate-title">${escapeHtml(title)}</div>
              <span class="chip">${escapeHtml(localizedCategory(tool.category))}</span>
              <div class="candidate-actions">
                <button
                  class="btn btn-primary btn-sm"
                  data-connect-tool-id="${escapeHtml(toolId)}"
                  ${connecting ? "disabled" : ""}
                >
                  ${connecting ? "接入中..." : "接入"}
                </button>
              </div>
            </div>
            <div class="candidate-meta">${escapeHtml(reason)}</div>
          </article>
        `;
      })
      .join("");
  }

  function bindAddToolModalEvents() {
    ui.addToolModalClose.addEventListener("click", closeAddToolModal);
    ui.addToolModal.addEventListener("click", (event) => {
      if (event.target === ui.addToolModal) {
        closeAddToolModal();
      }
    });
    ui.candidateList.addEventListener("click", onCandidateListClick);
    ui.goDebugFromAddTool.addEventListener("click", () => {
      closeAddToolModal();
      if (typeof onOpenDebug === "function") {
        onOpenDebug();
      }
    });
  }

  function setHandlers({ connectCandidateTool, openDebug }) {
    onConnectCandidateTool = connectCandidateTool || null;
    onOpenDebug = openDebug || null;
  }

  return {
    openAddToolModal,
    closeAddToolModal,
    renderAddToolModal,
    bindAddToolModalEvents,
    setHandlers,
  };
}
