// 文件职责：
// 1. 处理 relay 下行事件并同步到运行时状态。
// 2. 将工具白名单结果、控制端回执映射为统一 UI 提示。

import { asMap, asListOfMap, asBool } from "../utils/type.js";

/**
 * 创建 Relay 下行事件处理器。
 * @param {object} deps 依赖集合。
 * @returns {{ingestEvent: Function}}
 */
export function createConnectionEvents({
  state,
  hostById,
  ensureRuntime,
  sanitizeTools,
  clearToolConnectTimer,
  resolveToolDisplayName,
  setToolHidden,
  requestControllerRebind,
  connectCandidateTool,
  openHostNoticeModal,
  closeAddToolModal,
  requestToolsRefresh,
  renderAddToolModal,
  addLog,
}) {
  function shouldAutoRebindByReason(reason) {
    const text = String(reason || "");
    return /未绑定控制设备|未被授权|未授权控制|控制设备/.test(text);
  }

  function ingestEvent(hostId, raw) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) {
      return;
    }

    try {
      const event = JSON.parse(raw);
      if (!event || typeof event !== "object") {
        return;
      }
      const type = String(event.type || "");
      const payload = asMap(event.payload);

      if (type === "heartbeat") {
        runtime.sidecarStatus = String(payload.status || "ONLINE");
        runtime.lastHeartbeatAt = new Date();
        runtime.status = runtime.connected ? "CONNECTED" : runtime.status;
        return;
      }

      if (type === "tools_snapshot") {
        const parsed = asListOfMap(payload.tools);
        runtime.tools = sanitizeTools(hostId, parsed, false);
        for (const tool of runtime.tools) {
          const toolId = String(tool.toolId || "");
          if (toolId) {
            delete runtime.connectingToolIds[toolId];
          }
        }
        return;
      }

      if (type === "tools_candidates") {
        runtime.candidateTools = sanitizeTools(hostId, asListOfMap(payload.tools), true);
        return;
      }

      if (type === "tool_whitelist_updated") {
        handleToolWhitelistUpdated(hostId, payload);
        return;
      }

      if (type === "tool_details_snapshot") {
        applyToolDetailsSnapshot(hostId, payload);
        return;
      }

      if (type === "controller_bind_updated") {
        handleControllerBindUpdated(hostId, payload);
        return;
      }

      if (type === "metrics_snapshot") {
        applyMetricsSnapshot(hostId, payload);
      }
    } catch (_) {
      // ignore invalid payload
    }
  }

  function handleToolWhitelistUpdated(hostId, payload) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) return;

    const toolId = String(payload.toolId || "");
    const ok = asBool(payload.ok);
    const reason = String(payload.reason || "");
    const action = String(payload.action || "connect");
    const host = hostById(hostId);

    const connectedTool = runtime.tools.find((item) => String(item.toolId || "") === toolId);
    const candidateTool = runtime.candidateTools.find((item) => String(item.toolId || "") === toolId);
    const toolName = resolveToolDisplayName(
      hostId,
      connectedTool || candidateTool || { name: toolId, toolId },
    );

    if (toolId) {
      delete runtime.connectingToolIds[toolId];
      clearToolConnectTimer(runtime, toolId);
    }

    if (!ok) {
      if (action === "disconnect" && toolId) {
        setToolHidden(hostId, toolId, false);
      }
      const actionLabel = action === "disconnect" ? "断开" : "接入";
      addLog(
        `${actionLabel}工具失败 (${host ? host.displayName : hostId}): ${toolId || "--"} ${reason}`,
      );
      const retryCount = Number(runtime.toolConnectRetryCount[toolId] || 0);
      if (toolId && shouldAutoRebindByReason(reason) && retryCount < 1) {
        // 首次遇到控制端权限错误时自动重绑并重试一次，避免用户手工重复操作。
        runtime.toolConnectRetryCount[toolId] = retryCount + 1;
        requestControllerRebind(hostId);
        addLog(
          `检测到控制端权限限制，已自动重绑并重试 (${host ? host.displayName : hostId}): ${toolId}`,
        );
        setTimeout(() => connectCandidateTool(hostId, toolId), 300);
      } else {
        if (toolId) {
          delete runtime.toolConnectRetryCount[toolId];
        }
        openHostNoticeModal(
          action === "disconnect" ? "工具断开失败" : "工具接入失败",
          reason || `工具“${toolName}”未接入成功，请检查宿主机连接状态后重试。`,
        );
      }
    } else if (toolId) {
      delete runtime.toolConnectRetryCount[toolId];
      if (action === "connect") {
        setToolHidden(hostId, toolId, false);
        closeAddToolModal();
        openHostNoticeModal("添加成功", `工具“${toolName}”已接入。`);
      } else if (action === "disconnect") {
        openHostNoticeModal("断开成功", `工具“${toolName}”已断开。`);
      }
      addLog(
        `工具${action === "disconnect" ? "断开" : "接入"}已生效 (${host ? host.displayName : hostId}): ${toolId}`,
      );
      requestToolsRefresh(hostId);
    }

    renderAddToolModal();
  }

  function handleControllerBindUpdated(hostId, payload) {
    const ok = asBool(payload.ok);
    const changed = asBool(payload.changed);
    const deviceId = String(payload.deviceId || "--");
    const reason = String(payload.reason || "");
    const host = hostById(hostId);
    if (!ok) {
      addLog(`控制端重绑失败 (${host ? host.displayName : hostId}): ${deviceId} ${reason}`);
    } else if (changed) {
      addLog(`控制端已切换为当前设备 (${host ? host.displayName : hostId}): ${deviceId}`);
    } else {
      addLog(`控制端已是当前设备 (${host ? host.displayName : hostId}): ${deviceId}`);
    }
  }

  function applyMetricsSnapshot(hostId, payload) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) {
      return;
    }

    runtime.systemMetrics = asMap(payload.system);
    runtime.sidecarMetrics = asMap(payload.sidecar);
    runtime.primaryToolMetrics = asMap(payload.tool);

    const metricsByToolId = {};
    const metricsTools = asListOfMap(payload.tools);
    for (const item of metricsTools) {
      const toolId = String(item.toolId || "");
      if (toolId) {
        metricsByToolId[toolId] = item;
      }
    }

    const primaryToolId = String(runtime.primaryToolMetrics.toolId || "");
    if (primaryToolId) metricsByToolId[primaryToolId] = runtime.primaryToolMetrics;
    runtime.toolMetricsById = metricsByToolId;

    if (runtime.tools.length !== 0) return;
    if (metricsTools.length > 0) {
      runtime.tools = sanitizeTools(hostId, metricsTools, false);
      return;
    }
    if (!primaryToolId) return;

    runtime.tools = sanitizeTools(
      hostId,
      [{
        toolId: primaryToolId,
        name: String(runtime.primaryToolMetrics.name || "Unknown Tool"),
        category: String(runtime.primaryToolMetrics.category || "UNKNOWN"),
        vendor: String(runtime.primaryToolMetrics.vendor || "-"),
        mode: String(runtime.primaryToolMetrics.mode || "-"),
        status: String(runtime.primaryToolMetrics.status || "RUNNING"),
        connected: runtime.primaryToolMetrics.connected,
        endpoint: String(runtime.primaryToolMetrics.endpoint || ""),
        reason: String(runtime.primaryToolMetrics.reason || ""),
      }],
      false,
    );
  }

  /**
   * 应用工具详情快照（tool_details_snapshot）。
   * @param {string} hostId 宿主机标识。
   * @param {Record<string, any>} payload 事件载荷。
   */
  function applyToolDetailsSnapshot(hostId, payload) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) {
      return;
    }

    const detailsById = {};
    const staleById = {};
    const updatedAtById = {};

    const details = asListOfMap(payload.details);
    for (const item of details) {
      const toolId = String(item.toolId || "").trim();
      if (!toolId) {
        continue;
      }
      detailsById[toolId] = {
        schema: String(item.schema || ""),
        data: asMap(item.data),
        profileKey: String(item.profileKey || ""),
        expiresAt: String(item.expiresAt || ""),
      };
      staleById[toolId] = asBool(item.stale);
      updatedAtById[toolId] = String(item.collectedAt || "");
    }

    runtime.toolDetailsById = detailsById;
    runtime.toolDetailStaleById = staleById;
    runtime.toolDetailUpdatedAtById = updatedAtById;
  }

  return { ingestEvent };
}
