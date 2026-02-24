// 文件职责：
// 1. 管理工具接入/断开/改名与列表点击分发。
// 2. 协调“候选工具弹窗”和“工具详情弹窗”的交互。

import { createTraceId } from "../utils/log.js";

/**
 * 创建工具管理流程（接入/断开/改名/列表交互）。
 * @param {object} deps 依赖集合。
 */
export function createToolManageFlow({
  state,
  hostById,
  ensureRuntime,
  setToolHidden,
  getToolAlias,
  setToolAlias,
  resolveToolDisplayName,
  clearToolConnectTimer,
  addLog,
  sendSocketEvent,
  requestToolsRefresh,
  requestControllerRebind,
  connectHost,
  reconnectHost,
  disconnectHost,
  openHostNoticeModal,
  openAddToolModal,
  renderAddToolModal,
  closeAddToolModal,
  openToolDetail,
  openHostManageModal,
  render,
}) {
  function isOpenClawTool(tool) {
    const toolId = String(tool?.toolId || "").toLowerCase();
    const name = String(tool?.name || "").toLowerCase();
    const vendor = String(tool?.vendor || "").toLowerCase();
    return toolId.startsWith("openclaw_") || name.includes("openclaw") || vendor.includes("openclaw");
  }

  function shouldAutoRebindByReason(reason) {
    const text = String(reason || "");
    return /未绑定控制设备|未被授权|未授权控制|控制设备|控制端|未授权/.test(text);
  }

  function requestOpenClawProcessControl(hostId, toolId, action) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    const normalizedToolId = String(toolId || "").trim();
    const normalizedAction = String(action || "").trim().toLowerCase();
    if (!host || !runtime || !normalizedToolId) return;
    if (!runtime.connected) {
      openHostNoticeModal("当前宿主机未连接", "请先连接宿主机后再执行启停操作。");
      return;
    }

    const connectedTool = runtime.tools.find((item) => String(item.toolId || "") === normalizedToolId);
    if (!connectedTool || !isOpenClawTool(connectedTool)) {
      openHostNoticeModal("操作失败", "当前仅支持对已接入的 OpenClaw 执行一键停止/重启。");
      return;
    }

    const traceId = createTraceId();
    const sent = sendSocketEvent(
      hostId,
      "tool_process_control_request",
      {
        toolId: normalizedToolId,
        action: normalizedAction,
      },
      {
        action: normalizedAction === "restart" ? "restart_tool_process" : "stop_tool_process",
        traceId,
        toolId: normalizedToolId,
      },
    );
    if (!sent) {
      openHostNoticeModal("发送失败", "无法发送工具进程控制命令，请检查宿主机连接状态。");
      return;
    }

    const actionLabel = normalizedAction === "restart" ? "重启" : "停止";
    addLog(`已发送 OpenClaw ${actionLabel}请求 (${host.displayName}): ${normalizedToolId}`, {
      scope: "tool_process",
      action: normalizedAction === "restart" ? "restart_tool_process" : "stop_tool_process",
      outcome: "started",
      traceId,
      hostId,
      hostName: host.displayName,
      toolId: normalizedToolId,
    });
    openHostNoticeModal("已发出命令", `已请求${actionLabel}工具进程，完成后会自动刷新状态。`);
  }

  function connectCandidateTool(hostId, toolId) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    const id = String(toolId || "").trim();
    if (!host || !runtime || !id || runtime.connectingToolIds[id]) return;
    if (!runtime.toolConnectTraceIds) runtime.toolConnectTraceIds = {};
    if (!runtime.toolConnectRetryCount) runtime.toolConnectRetryCount = {};
    if (!runtime.toolConnectTimers) runtime.toolConnectTimers = {};
    if (!runtime.connected) {
      addLog(`接入失败：宿主机未连接 (${host.displayName})`, {
        level: "warn",
        scope: "tool_whitelist",
        action: "connect_tool",
        outcome: "failed",
        hostId,
        hostName: host.displayName,
        toolId: id,
      });
      return;
    }

    runtime.connectingToolIds[id] = true;
    setToolHidden(hostId, id, false);
    clearToolConnectTimer(runtime, id);
    const traceId = createTraceId();
    runtime.toolConnectTraceIds[id] = traceId;
    runtime.toolConnectTimers[id] = setTimeout(() => {
      const current = ensureRuntime(hostId);
      if (!current || !current.connectingToolIds[id]) return;
      if (String(current.toolConnectTraceIds[id] || "") !== traceId) return;
      delete current.connectingToolIds[id];
      delete current.toolConnectRetryCount[id];
      delete current.toolConnectTraceIds[id];
      clearToolConnectTimer(current, id);
      requestToolsRefresh(hostId);
      renderAddToolModal();
      addLog(`工具接入等待超时，已自动刷新候选列表 (${host.displayName}): ${id}`, {
        level: "warn",
        scope: "tool_whitelist",
        action: "connect_tool",
        outcome: "timeout",
        traceId,
        hostId,
        hostName: host.displayName,
        toolId: id,
      });
    }, 5000);
    renderAddToolModal();

    const sent = sendSocketEvent(hostId, "tool_connect_request", { toolId: id }, {
      action: "connect_tool",
      traceId,
      toolId: id,
    });
    if (!sent) {
      delete runtime.connectingToolIds[id];
      delete runtime.toolConnectRetryCount[id];
      delete runtime.toolConnectTraceIds[id];
      clearToolConnectTimer(runtime, id);
      openHostNoticeModal(
        "工具接入失败",
        `无法发送接入请求：工具“${id}”未接入。请先确认宿主机已连接。`,
      );
      render();
      return;
    }
    addLog(`已发送工具接入请求 (${host.displayName}): ${id}`, {
      scope: "tool_whitelist",
      action: "connect_tool",
      outcome: "started",
      traceId,
      hostId,
      hostName: host.displayName,
      toolId: id,
    });
  }

  function openToolAliasEditor(hostId, toolId) {
    const runtime = ensureRuntime(hostId);
    const host = hostById(hostId);
    if (!runtime || !host || !toolId) return;

    const connectedTool = runtime.tools.find((item) => String(item.toolId || "") === toolId);
    const candidateTool = runtime.candidateTools.find((item) => String(item.toolId || "") === toolId);
    const tool = connectedTool || candidateTool;
    const currentAlias = getToolAlias(hostId, toolId);
    const defaultName = resolveToolDisplayName(hostId, tool || { name: "Unknown Tool", toolId });
    const nextName = window.prompt(
      `请输入工具显示名称（宿主机：${host.displayName}）`,
      currentAlias || defaultName,
    );
    if (nextName === null) return;

    const normalized = String(nextName || "").trim();
    setToolAlias(hostId, toolId, normalized);
    addLog(`工具名称已更新 (${host.displayName}): ${toolId} -> ${normalized || defaultName}`);
    render();
  }

  function disconnectConnectedTool(hostId, toolId) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (!host || !runtime) return;
    if (!runtime.connected) {
      openHostNoticeModal("当前宿主机未连接", "请先连接宿主机后再删除工具。");
      return;
    }

    const tool = runtime.tools.find((item) => String(item.toolId || "") === toolId);
    const name = resolveToolDisplayName(hostId, tool || { name: toolId, toolId });

    // 先本地乐观移除，等待白名单回执再做最终收敛。
    setToolHidden(hostId, toolId, true);
    runtime.tools = runtime.tools.filter((item) => String(item.toolId || "") !== toolId);
    render();

    const traceId = createTraceId();
    const sent = sendSocketEvent(hostId, "tool_disconnect_request", { toolId }, {
      action: "disconnect_tool",
      traceId,
      toolId,
    });
    if (!sent) {
      setToolHidden(hostId, toolId, false);
      requestToolsRefresh(hostId);
      render();
      return;
    }
    addLog(`已请求断开工具 (${host.displayName}): ${toolId}`, {
      scope: "tool_whitelist",
      action: "disconnect_tool",
      outcome: "started",
      traceId,
      hostId,
      hostName: host.displayName,
      toolId,
    });
    openHostNoticeModal("工具已断开", `工具“${name}”已从已接入列表移除，可在候选工具中重新接入。`);
    requestToolsRefresh(hostId);
  }

  function onToolsGroupedClick(event) {
    const swipeContainer = event.target.closest(".tool-swipe");
    if (swipeContainer) {
      const swipeKey = String(swipeContainer.getAttribute("data-tool-swipe-key") || "").trim();
      const activeSwipeKey = String(state.activeToolSwipeKey || "").trim();
      const isActionClick = Boolean(
        event.target.closest(
          "[data-tool-edit], [data-tool-delete], [data-tool-process-stop], [data-tool-process-restart]",
        ),
      );
      // 左滑操作区展开时，优先保证操作按钮可点；阻止误触详情弹窗。
      if (!isActionClick && activeSwipeKey && swipeKey === activeSwipeKey) {
        return;
      }
    }

    const restartBtn = event.target.closest("[data-tool-process-restart]");
    if (restartBtn) {
      const [hostId, toolId] = String(restartBtn.getAttribute("data-tool-process-restart") || "").split("::");
      if (hostId && toolId) requestOpenClawProcessControl(hostId, toolId, "restart");
      return;
    }

    const stopBtn = event.target.closest("[data-tool-process-stop]");
    if (stopBtn) {
      const [hostId, toolId] = String(stopBtn.getAttribute("data-tool-process-stop") || "").split("::");
      if (hostId && toolId) requestOpenClawProcessControl(hostId, toolId, "stop");
      return;
    }

    const connectBtn = event.target.closest("[data-host-connect]");
    if (connectBtn) {
      const hostId = String(connectBtn.getAttribute("data-host-connect") || "");
      const runtime = ensureRuntime(hostId);
      if (runtime && runtime.connected) {
        void reconnectHost(hostId);
      } else {
        void connectHost(hostId, { manual: true, resetRetry: true });
      }
      return;
    }

    const disconnectBtn = event.target.closest("[data-host-disconnect]");
    if (disconnectBtn) {
      void disconnectHost(
        String(disconnectBtn.getAttribute("data-host-disconnect") || ""),
        { triggerReconnect: false },
      );
      return;
    }

    const addToolBtn = event.target.closest("[data-host-add-tool]");
    if (addToolBtn) {
      openAddToolModal(String(addToolBtn.getAttribute("data-host-add-tool") || ""));
      return;
    }

    const manageBtn = event.target.closest("[data-host-open-manage]");
    if (manageBtn) {
      openHostManageModal();
      return;
    }

    const editToolBtn = event.target.closest("[data-tool-edit]");
    if (editToolBtn) {
      const [hostId, toolId] = String(editToolBtn.getAttribute("data-tool-edit") || "").split("::");
      if (hostId && toolId) openToolAliasEditor(hostId, toolId);
      return;
    }

    const deleteToolBtn = event.target.closest("[data-tool-delete]");
    if (deleteToolBtn) {
      const [hostId, toolId] = String(deleteToolBtn.getAttribute("data-tool-delete") || "").split("::");
      if (hostId && toolId) disconnectConnectedTool(hostId, toolId);
      return;
    }

    const card = event.target.closest("[data-host-id][data-tool-id]");
    if (!card) return;
    openToolDetail(String(card.getAttribute("data-host-id") || ""), String(card.getAttribute("data-tool-id") || ""));
  }

  return {
    connectCandidateTool,
    disconnectConnectedTool,
    openToolAliasEditor,
    onToolsGroupedClick,
    shouldAutoRebindByReason,
    closeAddToolModal,
  };
}
