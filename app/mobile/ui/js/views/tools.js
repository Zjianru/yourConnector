// 文件职责：
// 1. 渲染按宿主机分组的工具卡片视图。
// 2. 管理工具卡片左滑动作区展开/收起状态。

import { fmt2 } from "../utils/format.js";
import { asBool } from "../utils/type.js";
import { escapeHtml } from "../utils/dom.js";
import { localizedCategory } from "../utils/host-format.js";

/** 创建工具分组视图能力（含左滑操作区管理）。 */
export function createToolsView({
  state,
  ui,
  ensureRuntime,
  metricForTool,
  resolveToolDisplayName,
  hostStatusLabel,
}) {
  function renderToolSwipeActions(hostId, toolId) {
    const pairKey = `${escapeHtml(hostId)}::${escapeHtml(toolId)}`;
    return `
      <div class="tool-swipe-actions">
        <button
          class="tool-action-btn edit"
          type="button"
          data-tool-edit="${pairKey}"
          aria-label="编辑工具名称"
        >
          <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <path
              d="M4 20h4l9.8-9.8-4-4L4 16v4z"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linejoin="round"
            ></path>
            <path
              d="M13.6 6.2l4 4"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linecap="round"
            ></path>
          </svg>
          编辑
        </button>
        <button
          class="tool-action-btn delete"
          type="button"
          data-tool-delete="${pairKey}"
          aria-label="删除已接入工具"
        >
          <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <path
              d="M5 7h14"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linecap="round"
            ></path>
            <path
              d="M9 7V5h6v2"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linejoin="round"
            ></path>
            <path
              d="M8 7l.8 11.1a1.5 1.5 0 0 0 1.5 1.4h3.4a1.5 1.5 0 0 0 1.5-1.4L16 7"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linejoin="round"
            ></path>
          </svg>
          删除
        </button>
      </div>
    `;
  }

  function buildOpenCodeCardNote(input) {
    const endpoint = String(input.endpoint || "").trim();
    const reason = String(input.reason || "").trim();
    const model = String(input.model || "").trim();
    const agentMode = String(input.agentMode || "").trim();
    const workspaceDir = String(input.workspaceDir || "").trim();
    if (endpoint) return endpoint;
    if (reason && !/等待会话|补充模式和模型信息/.test(reason)) return reason;
    if (model) return `当前模型：${model}`;
    if (agentMode) return `会话模式：${agentMode}`;
    if (workspaceDir) return `工作目录：${workspaceDir}`;
    return "";
  }

  function renderOpenCodeCard(hostId, tool, metric) {
    const toolId = String(tool.toolId || "");
    const swipeKey = `${hostId}::${toolId}`;
    const displayName = resolveToolDisplayName(hostId, tool);
    const mode = String(metric.mode ?? tool.mode ?? "TUI");
    const note = buildOpenCodeCardNote({
      endpoint: String(metric.endpoint ?? tool.endpoint ?? ""),
      reason: String(metric.reason ?? tool.reason ?? ""),
      model: String(metric.model ?? tool.model ?? ""),
      agentMode: String(metric.agentMode ?? tool.agentMode ?? ""),
      workspaceDir: String(metric.workspaceDir ?? tool.workspaceDir ?? ""),
    });
    const connected = asBool(metric.connected ?? tool.connected);
    return `
      <div class="tool-swipe" data-tool-swipe-key="${escapeHtml(swipeKey)}">
        <article
          class="tool-card tool-opencode"
          data-host-id="${escapeHtml(hostId)}"
          data-tool-id="${escapeHtml(toolId)}"
        >
          <div class="tool-head">
            <div class="tool-logo">OC</div>
            <div class="tool-name">${escapeHtml(displayName)}</div>
            <span class="chip">${escapeHtml(mode.toUpperCase())}</span>
          </div>
          <div class="chip-wrap">
            <span class="chip">${escapeHtml(String(metric.status ?? tool.status ?? "UNKNOWN"))}</span>
            <span class="chip">${connected ? "已接入" : "未接入"}</span>
          </div>
          ${note ? `<p class="tool-note">${escapeHtml(note)}</p>` : ""}
          <div class="tool-metrics">
            <div class="tool-metric">
              <div class="name">CPU</div>
              <div class="value">${escapeHtml(fmt2(metric.cpuPercent))}%</div>
            </div>
            <div class="tool-metric">
              <div class="name">Memory</div>
              <div class="value">${escapeHtml(fmt2(metric.memoryMb))} MB</div>
            </div>
          </div>
        </article>
        ${renderToolSwipeActions(hostId, toolId)}
      </div>
    `;
  }

  function renderGenericCard(hostId, tool, metric) {
    const toolId = String(tool.toolId || "");
    const swipeKey = `${hostId}::${toolId}`;
    const displayName = resolveToolDisplayName(hostId, tool);
    return `
      <div class="tool-swipe" data-tool-swipe-key="${escapeHtml(swipeKey)}">
        <article
          class="tool-card tool-generic"
          data-host-id="${escapeHtml(hostId)}"
          data-tool-id="${escapeHtml(toolId)}"
        >
          <div class="bar"></div>
          <div>
            <div class="title">${escapeHtml(displayName)}</div>
            <div class="sub">
              ${escapeHtml(localizedCategory(tool.category))} · ${escapeHtml(String(tool.status || "-"))}
            </div>
          </div>
          <div class="right">
            <div>${escapeHtml(fmt2(metric.cpuPercent))}% CPU</div>
            <div class="sub">${escapeHtml(fmt2(metric.memoryMb))} MB</div>
          </div>
        </article>
        ${renderToolSwipeActions(hostId, toolId)}
      </div>
    `;
  }

  function isOpenCodeTool(tool) {
    const toolId = String(tool.toolId || "").toLowerCase();
    const name = String(tool.name || "").toLowerCase();
    const vendor = String(tool.vendor || "").toLowerCase();
    return toolId.startsWith("opencode_") || name.includes("opencode") || vendor.includes("opencode");
  }

  function renderHostTools(hostId) {
    const runtime = ensureRuntime(hostId);
    if (!runtime || runtime.tools.length === 0) {
      return '<div class="empty">该宿主机暂无已接入工具。</div>';
    }
    return runtime.tools
      .map((tool) => {
        const toolId = String(tool.toolId || "");
        const metric = metricForTool(hostId, toolId);
        return isOpenCodeTool(tool)
          ? renderOpenCodeCard(hostId, tool, metric)
          : renderGenericCard(hostId, tool, metric);
      })
      .join("");
  }

  function renderHostGroup(host) {
    const runtime = ensureRuntime(host.hostId);
    const canAddTool = runtime && runtime.connected;
    const status = hostStatusLabel(host.hostId);
    return `
      <article class="host-group" data-host-group-id="${escapeHtml(host.hostId)}">
        <div class="host-group-head">
          <div class="host-group-title">${escapeHtml(host.displayName)}</div>
          <span class="host-status-chip">${escapeHtml(status)}</span>
        </div>
        <div class="host-group-actions" style="grid-template-columns: 1fr">
          <button
            class="btn btn-outline btn-sm"
            data-host-add-tool="${escapeHtml(host.hostId)}"
            ${canAddTool ? "" : "disabled"}
          >
            + 工具
          </button>
        </div>
        <div class="host-group-tools">${renderHostTools(host.hostId)}</div>
      </article>
    `;
  }

  function syncToolSwipePositions() {
    const swipes = Array.from(ui.toolsGroupedList.querySelectorAll(".tool-swipe[data-tool-swipe-key]"));
    if (swipes.length === 0) {
      state.activeToolSwipeKey = "";
      return;
    }
    let activeExists = false;
    for (const swipe of swipes) {
      const key = String(swipe.getAttribute("data-tool-swipe-key") || "").trim();
      const maxOffset = Math.max(0, swipe.scrollWidth - swipe.clientWidth);
      const shouldOpen = Boolean(state.activeToolSwipeKey)
        && key === state.activeToolSwipeKey
        && maxOffset > 0;
      if (shouldOpen) {
        activeExists = true;
        if (Math.abs(swipe.scrollLeft - maxOffset) > 1) {
          swipe.scrollLeft = maxOffset;
        }
      } else if (swipe.scrollLeft > 1) {
        swipe.scrollLeft = 0;
      }
    }
    if (state.activeToolSwipeKey && !activeExists) {
      state.activeToolSwipeKey = "";
    }
  }

  function renderToolsByHost(hosts) {
    if (hosts.length === 0) {
      ui.toolsGroupedList.innerHTML = '<div class="empty">暂无宿主机，请先完成配对。</div>';
      state.activeToolSwipeKey = "";
      return;
    }
    ui.toolsGroupedList.innerHTML = hosts.map((host) => renderHostGroup(host)).join("");
    syncToolSwipePositions();
  }

  function closeActiveToolSwipe() {
    if (!state.activeToolSwipeKey) {
      return;
    }
    state.activeToolSwipeKey = "";
    syncToolSwipePositions();
  }

  function onGlobalPointerDown(event) {
    if (!state.activeToolSwipeKey) {
      return;
    }
    const target = event.target;
    if (!(target instanceof Element) {
      return;
    }
    if (target.closest(".tool-swipe")) {
      return;
    }
    closeActiveToolSwipe();
  }

  function onToolSwipeScrollCapture(event) {
    const swipe = event.target;
    if (!(swipe instanceof Element) || !swipe.classList.contains("tool-swipe")) {
      return;
    }
    const key = String(swipe.getAttribute("data-tool-swipe-key") || "").trim();
    const maxOffset = Math.max(0, swipe.scrollWidth - swipe.clientWidth);
    if (!key || maxOffset <= 0) {
      return;
    }

    // 左滑阈值采用比例 + 最小像素，兼顾不同机型和卡片宽度。
    const openThreshold = Math.max(8, maxOffset * 0.12);
    const closeThreshold = Math.max(4, maxOffset * 0.08);
    if (swipe.scrollLeft >= openThreshold) {
      if (state.activeToolSwipeKey !== key) {
        state.activeToolSwipeKey = key;
        syncToolSwipePositions();
      }
      return;
    }
    if (state.activeToolSwipeKey === key && swipe.scrollLeft <= closeThreshold) {
      state.activeToolSwipeKey = "";
      syncToolSwipePositions();
    }
  }

  return {
    renderToolsByHost,
    onGlobalPointerDown,
    onToolSwipeScrollCapture,
    closeActiveToolSwipe,
  };
}
