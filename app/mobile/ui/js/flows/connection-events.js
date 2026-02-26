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
  resolveLogicalToolId,
  resolveRuntimeToolId,
  syncOpencodeInvalidState,
  clearToolConnectTimer,
  resolveToolDisplayName,
  setToolHidden,
  requestControllerRebind,
  connectCandidateTool,
  openHostNoticeModal,
  requestToolsRefresh,
  renderAddToolModal,
  onToolChatStarted,
  onToolChatChunk,
  onToolChatFinished,
  onToolReportFetchStarted,
  onToolReportFetchChunk,
  onToolReportFetchFinished,
  addLog,
}) {
  function findRuntimeTool(runtime, hostId, toolId) {
    const rawToolId = String(toolId || "").trim();
    if (!runtime || !rawToolId) return null;
    const logicalToolId = typeof resolveLogicalToolId === "function"
      ? resolveLogicalToolId(hostId, rawToolId)
      : rawToolId;
    return runtime.tools.find((item) => (
      String(item.toolId || "") === logicalToolId
      || String(item.toolId || "") === rawToolId
      || String(item.runtimeToolId || "") === rawToolId
    )) || null;
  }

  function extractOpenclawCapabilities(detailData) {
    const data = asMap(detailData);
    const overview = asMap(data.overview);
    const agents = asListOfMap(data.agents);
    const primaryAgent = agents[0] || {};
    const usage = asMap(data.usage);
    const configuredModels = asListOfMap(usage.configuredModels);
    return {
      model: String(primaryAgent.model || "").trim(),
      contextMaxTokens: Number(primaryAgent.contextMaxTokens || 0),
      configuredModelCount: Number(configuredModels.length || 0),
      defaultAgent: String(overview.defaultAgentId || overview.defaultAgentName || "").trim(),
    };
  }

  function describeOpenclawCapabilityChanges(beforeCaps, afterCaps) {
    const before = asMap(beforeCaps);
    const after = asMap(afterCaps);
    const changes = [];
    const beforeModel = String(before.model || "").trim();
    const afterModel = String(after.model || "").trim();
    if (beforeModel && afterModel && beforeModel !== afterModel) {
      changes.push(`模型 ${beforeModel} -> ${afterModel}`);
    }
    const beforeCtx = Number(before.contextMaxTokens || 0);
    const afterCtx = Number(after.contextMaxTokens || 0);
    if (beforeCtx > 0 && afterCtx > 0 && beforeCtx !== afterCtx) {
      changes.push(`上下文窗口 ${beforeCtx} -> ${afterCtx}`);
    }
    const beforeCount = Number(before.configuredModelCount || 0);
    const afterCount = Number(after.configuredModelCount || 0);
    if (beforeCount >= 0 && afterCount >= 0 && beforeCount !== afterCount) {
      changes.push(`配置模型数 ${beforeCount} -> ${afterCount}`);
    }
    return changes;
  }

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
        if (typeof syncOpencodeInvalidState === "function") {
          syncOpencodeInvalidState(hostId);
        }
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
        if (typeof syncOpencodeInvalidState === "function") {
          syncOpencodeInvalidState(hostId);
        }
        runtime.candidateSnapshotVersion = Number(runtime.candidateSnapshotVersion || 0) + 1;
        if (runtime.candidateRefreshTimer) {
          clearTimeout(runtime.candidateRefreshTimer);
          runtime.candidateRefreshTimer = null;
        }
        const expectedVersion = Number(runtime.candidateExpectedVersion || 0);
        if (runtime.candidateRefreshPending && (!expectedVersion || runtime.candidateSnapshotVersion >= expectedVersion)) {
          runtime.candidateRefreshPending = false;
          runtime.candidateExpectedVersion = 0;
        }
        return;
      }

      if (type === "tool_whitelist_updated") {
        handleToolWhitelistUpdated(hostId, payload, { traceId, eventId, eventType: type });
        return;
      }

      if (type === "tool_process_control_updated") {
        handleToolProcessControlUpdated(hostId, payload, { traceId, eventId, eventType: type });
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
        return;
      }

      if (type === "tool_chat_started") {
        if (typeof onToolChatStarted === "function") {
          onToolChatStarted(hostId, payload, { traceId, eventId, eventType: type });
        }
        return;
      }

      if (type === "tool_chat_chunk") {
        if (typeof onToolChatChunk === "function") {
          onToolChatChunk(hostId, payload, { traceId, eventId, eventType: type });
        }
        return;
      }

      if (type === "tool_chat_finished") {
        if (typeof onToolChatFinished === "function") {
          onToolChatFinished(hostId, payload, { traceId, eventId, eventType: type });
        }
        return;
      }

      if (type === "tool_report_fetch_started") {
        if (typeof onToolReportFetchStarted === "function") {
          onToolReportFetchStarted(hostId, payload, { traceId, eventId, eventType: type });
        }
        return;
      }

      if (type === "tool_report_fetch_chunk") {
        if (typeof onToolReportFetchChunk === "function") {
          onToolReportFetchChunk(hostId, payload, { traceId, eventId, eventType: type });
        }
        return;
      }

      if (type === "tool_report_fetch_finished") {
        if (typeof onToolReportFetchFinished === "function") {
          onToolReportFetchFinished(hostId, payload, { traceId, eventId, eventType: type });
        }
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
    const rawAction = String(payload.action || "connect");
    const action = ["connect", "disconnect", "refresh", "reset"].includes(rawAction)
      ? rawAction
      : "connect";
    const host = hostById(hostId);
    const isConnect = action === "connect";
    const isDisconnect = action === "disconnect";
    const isRefresh = action === "refresh";
    const isReset = action === "reset";

    const logicalToolId = typeof resolveLogicalToolId === "function"
      ? resolveLogicalToolId(hostId, toolId)
      : toolId;
    const connectedTool = findRuntimeTool(runtime, hostId, toolId);
    const candidateTool = runtime.candidateTools.find((item) => String(item.toolId || "") === toolId);
    const toolName = resolveToolDisplayName(
      hostId,
      connectedTool || candidateTool || { name: logicalToolId || toolId, toolId: logicalToolId || toolId },
    );

    if (isConnect && toolId) {
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
      if (isConnect) {
        delete runtime.toolConnectTraceIds[toolId];
      }
    }

    if (!ok) {
      if (isDisconnect && toolId) {
        setToolHidden(hostId, toolId, false);
        if (logicalToolId && logicalToolId !== toolId) {
          setToolHidden(hostId, logicalToolId, false);
        }
      }

      if (isRefresh) {
        addLog(
          `工具列表刷新失败 (${host ? host.displayName : hostId}): ${reason || "--"}`,
          {
            level: "warn",
            scope: "tool_whitelist",
            action: "refresh_tools",
            outcome: "failed",
            traceId: String(eventMeta.traceId || ""),
            eventId: String(eventMeta.eventId || ""),
            eventType: String(eventMeta.eventType || ""),
            hostId,
            hostName: host ? host.displayName : "",
            detail: reason,
          },
        );
        if (shouldAutoRebindByReason(reason)) {
          requestControllerRebind(hostId);
          requestToolsRefresh(hostId);
          addLog(
            `检测到控制端权限限制，已自动尝试重绑并刷新工具列表 (${host ? host.displayName : hostId})`,
            {
              scope: "controller",
              action: "rebind_controller",
              outcome: "started",
              traceId: String(eventMeta.traceId || ""),
              eventId: String(eventMeta.eventId || ""),
              eventType: String(eventMeta.eventType || ""),
              hostId,
              hostName: host ? host.displayName : "",
              detail: reason,
            },
          );
        } else {
          openHostNoticeModal(
            "工具列表刷新失败",
            reason || "请检查宿主机连接状态后重试。",
          );
        }
        renderAddToolModal();
        return;
      }

      if (isReset) {
        addLog(
          `工具白名单清理失败 (${host ? host.displayName : hostId}): ${reason || "--"}`,
          {
            level: "warn",
            scope: "tool_whitelist",
            action: "reset_tool_whitelist",
            outcome: "failed",
            traceId: String(eventMeta.traceId || ""),
            eventId: String(eventMeta.eventId || ""),
            eventType: String(eventMeta.eventType || ""),
            hostId,
            hostName: host ? host.displayName : "",
            detail: reason,
          },
        );
        openHostNoticeModal("清理已接入工具失败", reason || "请稍后重试。");
        renderAddToolModal();
        return;
      }

      const actionLabel = isDisconnect ? "断开" : "接入";
      addLog(
        `${actionLabel}工具失败 (${host ? host.displayName : hostId}): ${toolId || "--"} ${reason}`,
        {
          level: "warn",
          scope: "tool_whitelist",
          action: isDisconnect ? "disconnect_tool" : "connect_tool",
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
          isDisconnect ? "工具断开失败" : "工具接入失败",
          reason || `工具“${toolName}”未接入成功，请检查宿主机连接状态后重试。`,
        );
      }
    } else if (isReset) {
      addLog(`工具白名单已清空 (${host ? host.displayName : hostId})`, {
        scope: "tool_whitelist",
        action: "reset_tool_whitelist",
        outcome: "success",
        traceId: String(eventMeta.traceId || ""),
        eventId: String(eventMeta.eventId || ""),
        eventType: String(eventMeta.eventType || ""),
        hostId,
        hostName: host ? host.displayName : "",
      });
      requestToolsRefresh(hostId);
    } else if (toolId) {
      delete runtime.toolConnectRetryCount[toolId];
      if (isConnect) {
        setToolHidden(hostId, logicalToolId || toolId, false);
        openHostNoticeModal("添加成功", `工具“${toolName}”已接入。`, {
          keepAddToolOpen: true,
        });
      } else if (isDisconnect) {
        openHostNoticeModal("断开成功", `工具“${toolName}”已断开。`);
      }
      addLog(
        `工具${isDisconnect ? "断开" : "接入"}已生效 (${host ? host.displayName : hostId}): ${toolId}`,
        {
          scope: "tool_whitelist",
          action: isDisconnect ? "disconnect_tool" : "connect_tool",
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
      openHostNoticeModal(
        "当前设备未授权",
        reason || "自动重绑控制端失败，请在调试入口手动绑定当前设备。",
      );
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

  function handleToolProcessControlUpdated(hostId, payload, eventMeta = {}) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) return;

    const host = hostById(hostId);
    const toolId = String(payload.toolId || "");
    const action = String(payload.action || "").toLowerCase() === "restart" ? "restart" : "stop";
    const ok = asBool(payload.ok);
    const changed = asBool(payload.changed);
    const reason = String(payload.reason || "");

    const logicalToolId = typeof resolveLogicalToolId === "function"
      ? resolveLogicalToolId(hostId, toolId)
      : toolId;
    const connectedTool = findRuntimeTool(runtime, hostId, toolId);
    const candidateTool = runtime.candidateTools.find((item) => String(item.toolId || "") === toolId);
    const toolName = resolveToolDisplayName(
      hostId,
      connectedTool || candidateTool || { name: logicalToolId || toolId || "OpenClaw", toolId: logicalToolId || toolId },
    );
    const actionLabel = action === "restart" ? "重启" : "停止";

    if (!ok) {
      addLog(`工具${actionLabel}失败 (${host ? host.displayName : hostId}): ${toolId || "--"} ${reason}`, {
        level: "warn",
        scope: "tool_process",
        action: action === "restart" ? "restart_tool_process" : "stop_tool_process",
        outcome: "failed",
        traceId: String(eventMeta.traceId || ""),
        eventId: String(eventMeta.eventId || ""),
        eventType: String(eventMeta.eventType || ""),
        hostId,
        hostName: host ? host.displayName : "",
        toolId,
        detail: reason,
      });
      openHostNoticeModal(
        `${actionLabel}失败`,
        reason || `工具“${toolName}”${actionLabel}失败，请稍后重试。`,
      );
      return;
    }

    addLog(`工具${actionLabel}已执行 (${host ? host.displayName : hostId}): ${toolId || "--"}`, {
      scope: "tool_process",
      action: action === "restart" ? "restart_tool_process" : "stop_tool_process",
      outcome: changed ? "success" : "noop",
      traceId: String(eventMeta.traceId || ""),
      eventId: String(eventMeta.eventId || ""),
      eventType: String(eventMeta.eventType || ""),
      hostId,
      hostName: host ? host.displayName : "",
      toolId,
      detail: reason,
    });

    openHostNoticeModal(
      `${actionLabel}成功`,
      changed
        ? `工具“${toolName}”已完成${actionLabel}。`
        : `工具“${toolName}”当前无需${actionLabel}。`,
    );
    requestToolsRefresh(hostId);
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

    if (runtime.tools.length !== 0) {
      if (typeof syncOpencodeInvalidState === "function") {
        syncOpencodeInvalidState(hostId);
      }
      return;
    }
    if (metricsTools.length > 0) {
      runtime.tools = sanitizeTools(hostId, metricsTools, false);
      if (typeof syncOpencodeInvalidState === "function") {
        syncOpencodeInvalidState(hostId);
      }
      return;
    }
    if (!primaryToolId) return;

    runtime.tools = sanitizeTools(
      hostId,
      [{
        toolId: primaryToolId,
        toolClass: String(runtime.primaryToolMetrics.toolClass || ""),
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
    if (typeof syncOpencodeInvalidState === "function") {
      syncOpencodeInvalidState(hostId);
    }
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

    const previousDetailsById = runtime.toolDetailsById || {};
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

      const logicalToolId = typeof resolveLogicalToolId === "function"
        ? resolveLogicalToolId(hostId, toolId)
        : toolId;
      if (logicalToolId === "openclaw_primary") {
        const previousRuntimeToolId = typeof resolveRuntimeToolId === "function"
          ? resolveRuntimeToolId(hostId, logicalToolId)
          : toolId;
        const previousDetail = asMap(previousDetailsById[previousRuntimeToolId] || previousDetailsById[toolId]);
        const beforeCaps = extractOpenclawCapabilities(previousDetail.data);
        const afterCaps = extractOpenclawCapabilities(item.data);
        const changes = describeOpenclawCapabilityChanges(beforeCaps, afterCaps);
        if (changes.length > 0) {
          runtime.toolCapabilityChangesByToolId[logicalToolId] = {
            summary: changes.join(" / "),
            ts: new Date().toISOString(),
          };
        }
      }
    }

    runtime.toolDetailsById = detailsById;
    runtime.toolDetailStaleById = staleById;
    runtime.toolDetailUpdatedAtById = updatedAtById;
  }

  return { ingestEvent };
}
