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
export function createAddToolModal({
  state,
  ui,
  hostById,
  ensureRuntime,
  requestToolsRefresh,
  resolveToolDisplayName,
  queueDispatcher,
}) {
  let onConnectCandidateTool = null;

  /**
   * 生成候选工具的工作目录文案，帮助用户区分同类多实例。
   * @param {Record<string, any>} tool 候选工具。
   * @returns {string}
   */
  function workspaceLabel(tool) {
    const workspace = String(tool.workspaceDir || "").trim();
    return workspace || "未识别工作目录";
  }

  /**
   * 生成候选工具实例文案，优先展示 PID。
   * @param {Record<string, any>} tool 候选工具。
   * @returns {string}
   */
  function instanceLabel(tool) {
    const pid = Number(tool.pid || 0);
    if (pid > 0) {
      return `PID ${pid}`;
    }
    const toolId = String(tool.toolId || "").trim();
    if (!toolId) {
      return "实例未命名";
    }
    const suffix = toolId.split("_").pop() || toolId;
    return `实例 ${suffix}`;
  }

  function openAddToolModal(hostId) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (!host) {
      return;
    }
    state.addToolHostId = hostId;
    ui.addToolModal.classList.add("show");
    if (runtime) {
      runtime.candidateRefreshPending = true;
      runtime.candidateExpectedVersion = Number(runtime.candidateSnapshotVersion || 0) + 1;
      runtime.candidateRefreshTimer = queueDispatcher && typeof queueDispatcher.replaceTimeout === "function"
        ? queueDispatcher.replaceTimeout(
          runtime.candidateRefreshTimer,
          3000,
          () => {
            const latest = ensureRuntime(hostId);
            if (!latest || !latest.candidateRefreshPending) {
              return;
            }
            latest.candidateRefreshPending = false;
            latest.candidateExpectedVersion = 0;
            latest.candidateRefreshTimer = null;
            renderAddToolModal();
          },
          "candidate_refresh_timeout",
        )
        : setTimeout(() => {
        const latest = ensureRuntime(hostId);
        if (!latest || !latest.candidateRefreshPending) {
          return;
        }
        latest.candidateRefreshPending = false;
        latest.candidateExpectedVersion = 0;
        latest.candidateRefreshTimer = null;
        renderAddToolModal();
        }, 3000);
    }
    requestToolsRefresh(hostId);
    renderAddToolModal();
  }

  function closeAddToolModal() {
    const runtime = ensureRuntime(state.addToolHostId);
    if (runtime && runtime.candidateRefreshTimer) {
      if (queueDispatcher && typeof queueDispatcher.cancelTimeout === "function") {
        queueDispatcher.cancelTimeout(runtime.candidateRefreshTimer);
      } else {
        clearTimeout(runtime.candidateRefreshTimer);
      }
      runtime.candidateRefreshTimer = null;
    }
    if (runtime) {
      runtime.candidateRefreshPending = false;
      runtime.candidateExpectedVersion = 0;
    }
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

    if (runtime.candidateRefreshPending) {
      ui.candidateList.innerHTML = '<div class="empty">正在刷新候选工具，请稍候...</div>';
      return;
    }

    if (runtime.candidateTools.length === 0) {
      ui.candidateList.innerHTML = runtime.tools.length > 0
        ? '<div class="empty">当前没有候选工具。已发现的工具可能已接入（候选列表仅展示未接入工具）。</div>'
        : '<div class="empty">当前没有候选工具。请确认宿主机已运行 opencode/openclaw/codex/claude，并等待一次快照刷新。</div>';
      return;
    }

    const sortedCandidates = [...runtime.candidateTools].sort((a, b) => {
      const aName = String(a.name || "");
      const bName = String(b.name || "");
      const byName = aName.localeCompare(bName, "zh-Hans-CN");
      if (byName !== 0) {
        return byName;
      }
      const byWorkspace = workspaceLabel(a).localeCompare(workspaceLabel(b), "zh-Hans-CN");
      if (byWorkspace !== 0) {
        return byWorkspace;
      }
      return Number(a.pid || 0) - Number(b.pid || 0);
    });

    ui.candidateList.innerHTML = sortedCandidates
      .map((tool) => {
        const toolId = String(tool.toolId || "");
        const title = resolveToolDisplayName(state.addToolHostId, tool);
        const connecting = asBool(runtime.connectingToolIds[toolId]);
        const reason = String(tool.reason || "已发现可接入进程");
        const workspace = workspaceLabel(tool);
        const instance = instanceLabel(tool);
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
            <div class="candidate-extra">
              <div class="candidate-extra-row">
                <span class="candidate-extra-label">工作目录</span>
                <span class="candidate-extra-value">${escapeHtml(workspace)}</span>
              </div>
              <div class="candidate-extra-row">
                <span class="candidate-extra-label">实例</span>
                <span class="candidate-extra-value">${escapeHtml(instance)}</span>
              </div>
            </div>
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
  }

  function setHandlers({ connectCandidateTool }) {
    onConnectCandidateTool = connectCandidateTool || null;
  }

  return {
    openAddToolModal,
    closeAddToolModal,
    renderAddToolModal,
    bindAddToolModalEvents,
    setHandlers,
  };
}
