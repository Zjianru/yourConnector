// 文件职责：
// 1. 管理工具接入/断开/改名与列表点击分发。
// 2. 协调“候选工具弹窗”和“工具详情弹窗”的交互。

import { asBool } from "../utils/type.js";

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
  function shouldAutoRebindByReason(reason) {
    const text = String(reason || "");
    return /未绑定控制设备|未被授权|未授权控制|控制设备/.test(text);
  }

  function connectCandidateTool(hostId, toolId) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    const id = String(toolId || "").trim();
    if (!host || !runtime || !id || runtime.connectingToolIds[id]) return;
    if (!runtime.connected) {
      addLog(`接入失败：宿主机未连接 (${host.displayName})`);
      return;
    }

    runtime.connectingToolIds[id] = true;
    setToolHidden(hostId, id, false);
    clearToolConnectTimer(runtime, id);
    runtime.toolConnectTimers[id] = setTimeout(() => {
      const current = ensureRuntime(hostId);
      if (!current || !current.connectingToolIds[id]) return;
      delete current.connectingToolIds[id];
      delete current.toolConnectRetryCount[id];
      clearToolConnectTimer(current, id);
      renderAddToolModal();
      openHostNoticeModal(
        "工具接入未响应",
        "工具“" + id + "”接入超时。请确认 relay/sidecar 正常连接后重试；必要时先重连宿主机。",
      );
    }, 5000);
    renderAddToolModal();

    const sent = sendSocketEvent(hostId, "tool_connect_request", { toolId: id });
    if (!sent) {
      delete runtime.connectingToolIds[id];
      delete runtime.toolConnectRetryCount[id];
      clearToolConnectTimer(runtime, id);
      openHostNoticeModal(
        "工具接入失败",
        `无法发送接入请求：工具“${id}”未接入。请先确认宿主机已连接。`,
      );
      render();
    }
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

    const sent = sendSocketEvent(hostId, "tool_disconnect_request", { toolId });
    if (!sent) {
      setToolHidden(hostId, toolId, false);
      requestToolsRefresh(hostId);
      render();
      return;
    }
    addLog(`已请求断开工具 (${host.displayName}): ${toolId}`);
    openHostNoticeModal("工具已断开", `工具“${name}”已从已接入列表移除，可在候选工具中重新接入。`);
    requestToolsRefresh(hostId);
  }

  function onToolsGroupedClick(event) {
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
