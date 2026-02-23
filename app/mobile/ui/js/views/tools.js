// 文件职责：
// 1. 渲染按宿主机分组的工具卡片视图。
// 2. 管理工具卡片左滑动作区展开/收起状态。
// 3. 提供 OpenClaw 专属摘要卡渲染（系统总览优先，状态点显示）。

import { fmt2 } from "../utils/format.js";
import { asMap, asBool } from "../utils/type.js";
import { escapeHtml } from "../utils/dom.js";
import { localizedCategory } from "../utils/host-format.js";

/**
 * 创建工具分组视图能力（含左滑操作区管理）。
 * @param {object} deps 依赖集合。
 * @returns {object} 视图渲染与左滑交互方法集合。
 */
export function createToolsView(deps) {
  const {
    state,
    ui,
    ensureRuntime,
    metricForTool,
    detailForTool,
    resolveToolDisplayName,
    hostStatusLabel,
  } = deps;
  /**
   * 渲染工具左滑操作按钮（编辑/删除）。
   * @param {string} hostId 宿主机标识。
   * @param {string} toolId 工具标识。
   * @returns {string}
   */
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

  /**
   * 判断工具是否是 OpenCode。
   * @param {Record<string, any>} tool 工具对象。
   * @returns {boolean}
   */
  function isOpenCodeTool(tool) {
    const toolId = String(tool.toolId || "").toLowerCase();
    const name = String(tool.name || "").toLowerCase();
    const vendor = String(tool.vendor || "").toLowerCase();
    return toolId.startsWith("opencode_") || name.includes("opencode") || vendor.includes("opencode");
  }

  /**
   * 判断工具是否是 OpenClaw。
   * @param {Record<string, any>} tool 工具对象。
   * @returns {boolean}
   */
  function isOpenClawTool(tool) {
    const toolId = String(tool.toolId || "").toLowerCase();
    const name = String(tool.name || "").toLowerCase();
    const vendor = String(tool.vendor || "").toLowerCase();
    return toolId.startsWith("openclaw_") || name.includes("openclaw") || vendor.includes("openclaw");
  }

  /**
   * 构建 OpenCode 卡片次要说明。
   * @param {object} input 输入字段。
   * @returns {string}
   */
  function buildOpenCodeCardNote(input) {
    const endpoint = String(input.endpoint || "").trim();
    const reason = String(input.reason || "").trim();
    const model = String(input.model || "").trim();
    const agentMode = String(input.agentMode || "").trim();
    const workspaceDir = String(input.workspaceDir || "").trim();
    if (workspaceDir) return `工作目录：${workspaceDir}`;
    if (endpoint) return endpoint;
    if (reason && !/等待会话|补充模式和模型信息/.test(reason)) return reason;
    if (model) return `当前模型：${model}`;
    if (agentMode) return `会话模式：${agentMode}`;
    return "";
  }

  /**
   * 从 OpenClaw 详情提取摘要展示字段。
   * @param {Record<string, any>} detailData 详情 data。
   * @param {boolean} stale 详情是否过期。
   * @returns {{
   *   channelDigest: string,
   *   defaultAgent: string,
   *   sessionDigest: string,
   *   usageHeadline: string,
   *   gatewayDot: string,
   *   dataDot: string,
   * }}
   */
  function summarizeOpenClaw(detailData, stale) {
    const overview = asMap(detailData.overview);
    const statusDots = asMap(detailData.statusDots);
    const channelIdentities = Array.isArray(overview.channelIdentities) ? overview.channelIdentities : [];
    const channelHiddenCountRaw = Number(overview.channelHiddenCount);
    const channelHiddenCount = Number.isFinite(channelHiddenCountRaw)
      ? Math.max(0, Math.trunc(channelHiddenCountRaw))
      : 0;

    let channelDigest = "--";
    if (channelIdentities.length > 0) {
      const labels = channelIdentities
        .slice(0, 2)
        .map((raw) => {
          const item = asMap(raw);
          const channel = String(item.displayLabel || item.channel || "Unknown").trim();
          const account = String(item.accountDisplay || item.username || item.accountId || "default").trim() || "default";
          const status = asBool(item.running) ? "在线" : "离线";
          return `${channel}@${account} · ${status}`;
        })
        .filter((line) => line.trim().length > 0);
      const hidden = Math.max(0, channelIdentities.length - labels.length, channelHiddenCount);
      channelDigest = labels.join(" / ") || "--";
      if (hidden > 0) {
        channelDigest = `${channelDigest} +${hidden}`;
      }
    }

    const defaultAgent = String(overview.defaultAgentName || overview.defaultAgentId || "--");

    const activeSessions = Number(overview.activeSessions24h);
    const abortedSessions = Number(overview.abortedSessions);
    const activeText = Number.isFinite(activeSessions) ? Math.max(0, Math.trunc(activeSessions)) : 0;
    const abortedText = Number.isFinite(abortedSessions) ? Math.max(0, Math.trunc(abortedSessions)) : 0;
    const sessionDigest = `24h 活跃 ${activeText} · 异常 ${abortedText}`;

    const usageHeadline = asMap(overview.usageHeadline);
    const usageLabel = String(usageHeadline.label || "--").trim();
    const usagePercent = Number(usageHeadline.percent);
    let usageHeadlineText = usageLabel || "--";
    if (Number.isFinite(usagePercent)) {
      usageHeadlineText = `${usageHeadlineText} · ${Math.trunc(usagePercent)}%`;
    }

    const gatewayRaw = String(statusDots.gateway || "").trim().toLowerCase();
    const gatewayDot = gatewayRaw === "online" || gatewayRaw === "offline" ? gatewayRaw : "unknown";
    const dataRaw = stale
      ? "stale"
      : String(detailData.collectState || statusDots.data || "").trim().toLowerCase();
    const dataDot = (dataRaw === "fresh" || dataRaw === "stale" || dataRaw === "collecting")
      ? dataRaw
      : "unknown";

    return {
      channelDigest,
      defaultAgent,
      sessionDigest,
      usageHeadline: usageHeadlineText,
      gatewayDot,
      dataDot,
    };
  }

  /**
   * 将 OpenClaw 状态点状态转为中文标签。
   * @param {string} kind 点位类型（gateway/data）。
   * @param {string} value 状态值。
   * @returns {string}
   */
  function statusDotLabel(kind, value) {
    if (kind === "gateway") {
      if (value === "online") return "网关在线";
      if (value === "offline") return "网关离线";
      return "网关未知";
    }
    if (value === "collecting") return "正在采集中";
    if (value === "fresh") return "已更新";
    if (value === "stale") return "数据过期";
    return "数据未知";
  }

  /**
   * 将时间戳转换为“距今秒数”。
   * @param {unknown} raw 原始时间值（毫秒/秒/ISO）。
   * @returns {number}
   */
  function ageSeconds(raw) {
    if (raw == null || raw === "") {
      return NaN;
    }
    const num = Number(raw);
    if (Number.isFinite(num) && num > 0) {
      const millis = num > 1_000_000_000_000 ? num : num * 1000;
      return Math.max(0, Math.trunc((Date.now() - millis) / 1000));
    }
    const parsed = Date.parse(String(raw));
    if (Number.isFinite(parsed)) {
      return Math.max(0, Math.trunc((Date.now() - parsed) / 1000));
    }
    return NaN;
  }

  /**
   * 生成 OpenClaw 数据时效文案。
   * @param {"fresh"|"stale"|"collecting"|"unknown"} value 数据状态。
   * @param {unknown} collectedAt 最近采集时间。
   * @returns {string}
   */
  function openClawFreshnessLabel(value, collectedAt) {
    if (value === "collecting") {
      return "正在采集中";
    }
    const sec = ageSeconds(collectedAt);
    if (value === "fresh") {
      if (Number.isFinite(sec)) {
        return `已更新 ${sec} 秒前`;
      }
      return "已更新";
    }
    if (value === "stale") {
      if (Number.isFinite(sec)) {
        return `已过期（上次更新 ${sec} 秒前）`;
      }
      return "已过期";
    }
    return "数据未知";
  }

  /**
   * 生成 OpenClaw 状态点样式类名。
   * @param {string} kind 点位类型。
   * @param {string} value 状态值。
   * @returns {string}
   */
  function statusDotClass(kind, value) {
    if (kind === "gateway") {
      if (value === "online") return "online";
      if (value === "offline") return "offline";
      return "unknown";
    }
    if (value === "collecting") return "collecting";
    if (value === "fresh") return "fresh";
    if (value === "stale") return "stale";
    return "unknown";
  }

  /**
   * 渲染 OpenCode 卡片。
   * @param {string} hostId 宿主机标识。
   * @param {Record<string, any>} tool 工具对象。
   * @param {Record<string, any>} metric 指标对象。
   * @returns {string}
   */
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

  /**
   * 渲染 OpenClaw 卡片（专属摘要：系统总览 + 状态点）。
   * @param {string} hostId 宿主机标识。
   * @param {Record<string, any>} tool 工具对象。
   * @param {Record<string, any>} metric 指标对象。
   * @returns {string}
   */
  function renderOpenClawCard(hostId, tool, metric) {
    const toolId = String(tool.toolId || "");
    const swipeKey = `${hostId}::${toolId}`;
    const displayName = resolveToolDisplayName(hostId, tool);
    const mode = String(metric.mode ?? tool.mode ?? "CLI");
    const connected = asBool(metric.connected ?? tool.connected);
    const detail = detailForTool(hostId, toolId);
    const detailData = asMap(detail.data);
    const summary = summarizeOpenClaw(detailData, asBool(detail.stale));
    const workspace = String(metric.workspaceDir ?? tool.workspaceDir ?? "").trim();

    const gatewayLabel = statusDotLabel("gateway", summary.gatewayDot);
    const dataLabel = openClawFreshnessLabel(
      summary.dataDot,
      detail.collectedAt || detailData.collectingSince,
    );
    const gatewayClass = statusDotClass("gateway", summary.gatewayDot);
    const dataClass = statusDotClass("data", summary.dataDot);

    return `
      <div class="tool-swipe" data-tool-swipe-key="${escapeHtml(swipeKey)}">
        <article
          class="tool-card tool-openclaw"
          data-host-id="${escapeHtml(hostId)}"
          data-tool-id="${escapeHtml(toolId)}"
        >
          <div class="tool-head">
            <div class="tool-logo openclaw">CL</div>
            <div class="tool-name">${escapeHtml(displayName)}</div>
            <span class="chip">${escapeHtml(mode.toUpperCase())}</span>
          </div>
          <div class="chip-wrap">
            <span class="chip">${escapeHtml(String(metric.status ?? tool.status ?? "UNKNOWN"))}</span>
            <span class="chip">${connected ? "已接入" : "未接入"}</span>
            ${workspace ? `<span class="chip">${escapeHtml(`工作目录：${workspace}`)}</span>` : ""}
          </div>
          <div class="tool-openclaw-dots">
            <span class="tool-dot-label">
              <i class="tool-dot ${gatewayClass}"></i>
              ${escapeHtml(gatewayLabel)}
            </span>
            <span class="tool-dot-label">
              <i class="tool-dot ${dataClass}"></i>
              ${escapeHtml(dataLabel)}
            </span>
          </div>
          <div class="tool-openclaw-summary">
            <div class="tool-openclaw-item">
              <span class="k">渠道身份</span>
              <span class="v">${escapeHtml(summary.channelDigest)}</span>
            </div>
            <div class="tool-openclaw-item">
              <span class="k">默认 Agent</span>
              <span class="v">${escapeHtml(summary.defaultAgent)}</span>
            </div>
            <div class="tool-openclaw-item">
              <span class="k">会话诊断</span>
              <span class="v">${escapeHtml(summary.sessionDigest)}</span>
            </div>
            <div class="tool-openclaw-item">
              <span class="k">混合用量头条</span>
              <span class="v">${escapeHtml(summary.usageHeadline)}</span>
            </div>
          </div>
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

  /**
   * 渲染通用工具卡片（非 OpenCode/OpenClaw）。
   * @param {string} hostId 宿主机标识。
   * @param {Record<string, any>} tool 工具对象。
   * @param {Record<string, any>} metric 指标对象。
   * @returns {string}
   */
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

  /**
   * 渲染指定宿主机的已接入工具。
   * @param {string} hostId 宿主机标识。
   * @returns {string}
   */
  function renderHostTools(hostId) {
    const runtime = ensureRuntime(hostId);
    if (!runtime || runtime.tools.length === 0) {
      return '<div class="empty">该宿主机暂无已接入工具。</div>';
    }
    return runtime.tools
      .map((tool) => {
        const toolId = String(tool.toolId || "");
        const metric = metricForTool(hostId, toolId);
        if (isOpenClawTool(tool)) {
          return renderOpenClawCard(hostId, tool, metric);
        }
        if (isOpenCodeTool(tool)) {
          return renderOpenCodeCard(hostId, tool, metric);
        }
        return renderGenericCard(hostId, tool, metric);
      })
      .join("");
  }

  /**
   * 渲染宿主机分组卡片。
   * @param {Record<string, any>} host 宿主机对象。
   * @returns {string}
   */
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

  /**
   * 同步左滑展开状态到 DOM（仅保留一个展开项）。
   */
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

  /**
   * 按宿主机分组渲染工具列表。
   * @param {Array<Record<string, any>>} hosts 宿主机列表。
   */
  function renderToolsByHost(hosts) {
    if (hosts.length === 0) {
      ui.toolsGroupedList.innerHTML = '<div class="empty">暂无宿主机，请先完成配对。</div>';
      state.activeToolSwipeKey = "";
      return;
    }
    ui.toolsGroupedList.innerHTML = hosts.map((host) => renderHostGroup(host)).join("");
    syncToolSwipePositions();
  }

  /** 收起当前展开的左滑操作区。 */
  function closeActiveToolSwipe() {
    if (!state.activeToolSwipeKey) {
      return;
    }
    state.activeToolSwipeKey = "";
    syncToolSwipePositions();
  }

  /**
   * 全局指针按下时，如果点击了卡片外区域则收起左滑操作区。
   * @param {PointerEvent} event 指针事件。
   */
  function onGlobalPointerDown(event) {
    if (!state.activeToolSwipeKey) {
      return;
    }
    const target = event.target;
    if (!(target instanceof Element)) {
      return;
    }
    if (target.closest(".tool-swipe")) {
      return;
    }
    closeActiveToolSwipe();
  }

  /**
   * 在捕获阶段监听横向滚动，维护左滑展开阈值逻辑。
   * @param {Event} event 滚动事件。
   */
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
