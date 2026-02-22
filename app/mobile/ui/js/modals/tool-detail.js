// 文件职责：
// 1. 管理工具详情弹窗的打开/关闭与摘要渲染。
// 2. 将“摘要 + 展开详情 + 模型用量”渲染逻辑从主流程剥离。

import { asMap, asListOfMap, asBool } from "../utils/type.js";
import { fmt2, fmtInt, fmtTokenM, usageSummary } from "../utils/format.js";
import { localizedCategory } from "../utils/host-format.js";
import { renderRows } from "../utils/rows.js";

/**
 * 创建工具详情弹窗能力。
 * @param {object} deps 依赖集合。
 */
export function createToolDetailModal({ state, ui, hostById, ensureRuntime, metricForTool, resolveToolDisplayName }) {
  function openToolDetail(hostId, toolId) {
    if (!hostId || !toolId) {
      return;
    }
    state.detailHostId = hostId;
    state.detailToolId = toolId;
    state.detailExpanded = false;
    renderToolModal();
  }

  function closeToolDetail() {
    state.detailHostId = "";
    state.detailToolId = "";
    state.detailExpanded = false;
    renderToolModal();
  }

  function pickMetric(tool, metric, key) {
    const value = metric[key] ?? tool[key];
    return value == null ? "" : String(value);
  }

  function renderToolModal() {
    if (!state.detailHostId || !state.detailToolId) {
      ui.toolModal.classList.remove("show");
      return;
    }

    const host = hostById(state.detailHostId);
    const runtime = ensureRuntime(state.detailHostId);
    if (!host || !runtime) {
      ui.toolModal.classList.remove("show");
      return;
    }

    const tool = runtime.tools.find((item) => String(item.toolId || "") === state.detailToolId);
    if (!tool) {
      ui.toolModal.classList.remove("show");
      return;
    }

    const metric = metricForTool(state.detailHostId, String(tool.toolId || ""));
    const toolId = String(tool.toolId || "");
    const displayName = resolveToolDisplayName(state.detailHostId, tool);
    const connectedTool = asBool(metric.connected ?? tool.connected);
    const latestTokens = asMap(metric.latestTokens);
    const modelUsage = asListOfMap(metric.modelUsage);

    const summaryRows = [
      ["宿主机", host.displayName],
      ["工具名称", displayName],
      ["工具模式", pickMetric(tool, metric, "mode") || "--"],
      ["会话模式", pickMetric(tool, metric, "agentMode") || "--"],
      ["当前模型", pickMetric(tool, metric, "model") || "--"],
      ["状态", pickMetric(tool, metric, "status") || "--"],
      [
        "最近Token（总/输入/输出）",
        `${fmtTokenM(latestTokens.total)} / ${fmtTokenM(latestTokens.input)} / ${fmtTokenM(latestTokens.output)}`,
      ],
      ["最近缓存（读/写）", `${fmtTokenM(latestTokens.cacheRead)} / ${fmtTokenM(latestTokens.cacheWrite)}`],
      ["模型用量", usageSummary(modelUsage)],
    ];
    const reason = pickMetric(tool, metric, "reason");
    if (reason) {
      summaryRows.push(["原因", reason]);
    }

    const detailsRows = [
      ["App Link", runtime.connected ? "Connected" : "Disconnected"],
      [
        "Last Heartbeat",
        runtime.lastHeartbeatAt ? runtime.lastHeartbeatAt.toLocaleString() : "--",
      ],
      ["Tool Reachable", connectedTool ? "Yes" : "No"],
      ["Tool ID", toolId || "--"],
      ["Endpoint", pickMetric(tool, metric, "endpoint") || "--"],
      ["Workspace", pickMetric(tool, metric, "workspaceDir") || "--"],
      ["Session ID", pickMetric(tool, metric, "sessionId") || "--"],
      ["Session Title", pickMetric(tool, metric, "sessionTitle") || "--"],
      ["Session Updated", pickMetric(tool, metric, "sessionUpdatedAt") || "--"],
      ["厂商", pickMetric(tool, metric, "vendor") || "--"],
      ["类别", localizedCategory(tool.category)],
      ["CPU", `${fmt2(metric.cpuPercent)}%`],
      ["Memory", `${fmt2(metric.memoryMb)} MB`],
      ["Source", String(metric.source || "--")],
      [
        "Latest Cache",
        `R:${fmtTokenM(latestTokens.cacheRead)} W:${fmtTokenM(latestTokens.cacheWrite)}`,
      ],
    ];

    ui.toolModalTitle.textContent = displayName || "Tool Detail";
    ui.summaryRows.innerHTML = renderRows(summaryRows);

    const previewCount = 2;
    const showingRows = state.detailExpanded ? detailsRows : detailsRows.slice(0, previewCount);
    ui.detailRows.innerHTML = renderRows(showingRows);
    ui.detailTip.textContent = !state.detailExpanded && detailsRows.length > previewCount
      ? `还有 ${detailsRows.length - previewCount} 项，点击箭头展开`
      : "";

    ui.toggleDetailsBtn.textContent = state.detailExpanded ? "⌃" : "⌄";

    if (state.detailExpanded && modelUsage.length > 0) {
      ui.usagePanel.style.display = "block";
      ui.usageRows.innerHTML = renderRows(
        modelUsage.map((row) => {
          const modelName = String(row.model || "--");
          const total = fmtTokenM(row.tokenTotal);
          const input = fmtTokenM(row.tokenInput);
          const output = fmtTokenM(row.tokenOutput);
          const count = fmtInt(row.messages);
          return [
            modelName,
            `消息 ${count} 条 · 总Token ${total} · 输入 ${input} · 输出 ${output}`,
          ];
        }),
      );
    } else {
      ui.usagePanel.style.display = "none";
      ui.usageRows.innerHTML = "";
    }

    ui.toolModal.classList.add("show");
  }

  function bindToolDetailModalEvents() {
    ui.toolModalClose.addEventListener("click", closeToolDetail);
    ui.toolModal.addEventListener("click", (event) => {
      if (event.target === ui.toolModal) {
        closeToolDetail();
      }
    });
    ui.toggleDetailsBtn.addEventListener("click", () => {
      state.detailExpanded = !state.detailExpanded;
      renderToolModal();
    });
  }

  return {
    openToolDetail,
    closeToolDetail,
    renderToolModal,
    bindToolDetailModalEvents,
  };
}
