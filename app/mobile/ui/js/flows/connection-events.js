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
  requestToolsRefresh,
  renderAddToolModal,
  addLog,
}) {
  function shouldAutoRebindByReason(reason) {
    const text = String(reason || "");
    return /未绑定控制设备|未被授权|未授权控制|控制设备|控制端|未授权/.test(text);
  }

  function shouldRetryCandidateByReason(reason) {
    const text = String(reason || "");
    return /工具不在当前候选列表|候选列表/.test(text);
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
      const traceId = String(event.traceId || "");
      const eventId = String(event.eventId || "");
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
        if (!runtime.toolConnectTraceIds) runtime.toolConnectTraceIds = {};
        for (const tool of runtime.tools) {
          const toolId = String(tool.toolId || "");
          if (toolId) {
            delete runtime.connectingToolIds[toolId];
            delete runtime.toolConnectTraceIds[toolId];
          }
        }
        return;
      }

      if (type === "tools_candidates") {
        runtime.candidateTools = sanitizeTools(hostId, asListOfMap(payload.tools), true);
        return;
      }

      if (type === "tool_whitelist_updated") {
        handleToolWhitelistUpdated(hostId, payload, { traceId, eventId, eventType: type });
        return;
      }

      if (type === "tool_details_snapshot") {
        applyToolDetailsSnapshot(hostId, payload);
        return;
      }

      if (type === "controller_bind_updated") {
        handleControllerBindUpdated(hostId, payload, { traceId, eventId, eventType: type });
        return;
      }

      if (type === "metrics_snapshot") {
        applyMetricsSnapshot(hostId, payload);
      }
    } catch (_) {
      // ignore invalid payload
    }
  }

  function handleToolWhitelistUpdated(hostId, payload, eventMeta = {}) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) return;
    if (!runtime.toolConnectTraceIds) runtime.toolConnectTraceIds = {};
    if (!runtime.toolConnectRetryCount) runtime.toolConnectRetryCount = {};

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

    if (action === "connect" && toolId) {
      const pendingTraceId = String(runtime.toolConnectTraceIds[toolId] || "");
      const incomingTraceId = String(eventMeta.traceId || "");
      if (pendingTraceId && incomingTraceId && pendingTraceId !== incomingTraceId) {
        addLog(
          `忽略过期工具接入回执 (${host ? host.displayName : hostId}): ${toolId}`,
          {
            level: "warn",
            scope: "tool_whitelist",
            action: "connect_tool",
            outcome: "ignored",
            traceId: incomingTraceId,
            eventId: String(eventMeta.eventId || ""),
            eventType: String(eventMeta.eventType || ""),
            hostId,
            hostName: host ? host.displayName : "",
            toolId,
            detail: `pending_trace=${pendingTraceId}`,
          },
        );
        return;
      }
    }

    if (toolId) {
      delete runtime.connectingToolIds[toolId];
      clearToolConnectTimer(runtime, toolId);
      if (action === "connect") {
        delete runtime.toolConnectTraceIds[toolId];
      }
    }

    if (!ok) {
      if (action === "disconnect" && toolId) {
        setToolHidden(hostId, toolId, false);
      }
      const actionLabel = action === "disconnect" ? "断开" : "接入";
      addLog(
        `${actionLabel}工具失败 (${host ? host.displayName : hostId}): ${toolId || "--"} ${reason}`,
        {
          level: "warn",
          scope: "tool_whitelist",
          action: action === "disconnect" ? "disconnect_tool" : "connect_tool",
          outcome: "failed",
          traceId: String(eventMeta.traceId || ""),
          eventId: String(eventMeta.eventId || ""),
          eventType: String(eventMeta.eventType || ""),
          hostId,
          hostName: host ? host.displayName : "",
          toolId,
          detail: reason,
        },
      );
      const retryCount = Number(runtime.toolConnectRetryCount[toolId] || 0);
      if (toolId && shouldAutoRebindByReason(reason) && retryCount < 1) {
        // 首次遇到控制端权限错误时自动重绑并重试一次，避免用户手工重复操作。
        runtime.toolConnectRetryCount[toolId] = retryCount + 1;
        requestControllerRebind(hostId);
        addLog(
          `检测到控制端权限限制，已自动重绑并重试 (${host ? host.displayName : hostId}): ${toolId}`,
          {
            scope: "tool_whitelist",
            action: "auto_rebind_retry",
            outcome: "started",
            traceId: String(eventMeta.traceId || ""),
            eventId: String(eventMeta.eventId || ""),
            eventType: String(eventMeta.eventType || ""),
            hostId,
            hostName: host ? host.displayName : "",
            toolId,
            detail: reason,
          },
        );
        requestToolsRefresh(hostId);
        setTimeout(() => connectCandidateTool(hostId, toolId), 350);
      } else if (toolId && shouldRetryCandidateByReason(reason) && retryCount < 1) {
        // 候选快照存在延迟时先主动刷新再重试一次，避免用户看到“先失败后成功”的误导弹窗。
        runtime.toolConnectRetryCount[toolId] = retryCount + 1;
        addLog(
          `候选快照尚未收敛，已自动刷新并重试 (${host ? host.displayName : hostId}): ${toolId}`,
          {
            scope: "tool_whitelist",
            action: "connect_tool_retry",
            outcome: "started",
            traceId: String(eventMeta.traceId || ""),
            eventId: String(eventMeta.eventId || ""),
            eventType: String(eventMeta.eventType || ""),
            hostId,
            hostName: host ? host.displayName : "",
            toolId,
            detail: reason,
          },
        );
        requestToolsRefresh(hostId);
        setTimeout(() => connectCandidateTool(hostId, toolId), 350);
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
        openHostNoticeModal("添加成功", `工具“${toolName}”已接入。`, {
          keepAddToolOpen: true,
        });
      } else if (action === "disconnect") {
        openHostNoticeModal("断开成功", `工具“${toolName}”已断开。`);
      }
      addLog(
        `工具${action === "disconnect" ? "断开" : "接入"}已生效 (${host ? host.displayName : hostId}): ${toolId}`,
        {
          scope: "tool_whitelist",
          action: action === "disconnect" ? "disconnect_tool" : "connect_tool",
          outcome: "success",
          traceId: String(eventMeta.traceId || ""),
          eventId: String(eventMeta.eventId || ""),
          eventType: String(eventMeta.eventType || ""),
          hostId,
          hostName: host ? host.displayName : "",
          toolId,
        },
      );
      requestToolsRefresh(hostId);
    }

    renderAddToolModal();
  }

  function handleControllerBindUpdated(hostId, payload, eventMeta = {}) {
    const ok = asBool(payload.ok);
    const changed = asBool(payload.changed);
    const deviceId = String(payload.deviceId || "--");
    const reason = String(payload.reason || "");
    const host = hostById(hostId);
    if (!ok) {
      addLog(`控制端重绑失败 (${host ? host.displayName : hostId}): ${deviceId} ${reason}`, {
        level: "warn",
        scope: "controller",
        action: "rebind_controller",
        outcome: "failed",
        traceId: String(eventMeta.traceId || ""),
        eventId: String(eventMeta.eventId || ""),
        eventType: String(eventMeta.eventType || ""),
        hostId,
        hostName: host ? host.displayName : "",
        detail: reason,
      });
    } else if (changed) {
      addLog(`控制端已切换为当前设备 (${host ? host.displayName : hostId}): ${deviceId}`, {
        scope: "controller",
        action: "rebind_controller",
        outcome: "success",
        traceId: String(eventMeta.traceId || ""),
        eventId: String(eventMeta.eventId || ""),
        eventType: String(eventMeta.eventType || ""),
        hostId,
        hostName: host ? host.displayName : "",
      });
    } else {
      addLog(`控制端已是当前设备 (${host ? host.displayName : hostId}): ${deviceId}`, {
        scope: "controller",
        action: "rebind_controller",
        outcome: "noop",
        traceId: String(eventMeta.traceId || ""),
        eventId: String(eventMeta.eventId || ""),
        eventType: String(eventMeta.eventType || ""),
        hostId,
        hostName: host ? host.displayName : "",
      });
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
