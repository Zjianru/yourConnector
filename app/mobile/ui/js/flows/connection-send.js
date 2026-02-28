// 文件职责：
// 1. 封装连接流对外发送能力（通用发送、工具刷新、控制端重绑）。
// 2. 统一发送前置校验与日志输出。

import { asMap } from "../utils/type.js";

/**
 * 创建连接发送能力。
 * @param {object} deps 依赖集合。
 * @returns {object} 发送能力集合（通用发送、工具刷新、详情刷新、控制端重绑）。
 */
export function createConnectionSendOps({
  state,
  hostById,
  ensureRuntime,
  createEventId,
  addLog,
  formatWireLog,
  resolveLogicalToolId,
  resolveRuntimeToolId,
}) {
  /**
   * 发送统一协议事件。
   * @param {string} hostId 宿主机标识。
   * @param {string} type 事件类型。
   * @param {object} payload 事件载荷。
   * @returns {boolean} 是否发送成功。
   */
  function sendSocketEvent(hostId, type, payload, options = {}) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    const eventType = String(type || "").trim();
    const payloadMap = asMap(payload);
    const traceId = String(options.traceId || "").trim() || createEventId().replace(/^evt_/, "trc_");
    const eventId = createEventId();
    const toolId = String(options.toolId || payloadMap.toolId || "").trim();
    const action = String(options.action || eventType || "send_event").trim();
    if (!host || !runtime || !runtime.socket || !runtime.connected) {
      addLog(`send skipped: host not connected (${host ? host.displayName : hostId})`, {
        level: "warn",
        scope: "ws_out",
        action,
        outcome: "skipped",
        traceId,
        eventId,
        eventType,
        hostId,
        hostName: host ? host.displayName : "",
        toolId,
      });
      return false;
    }

    const event = {
      v: 1,
      eventId,
      traceId,
      type: eventType,
      systemId: host.systemId,
      seq: Date.now(),
      ts: new Date().toISOString(),
      payload: payloadMap,
    };
    const encoded = JSON.stringify(event);

    try {
      runtime.socket.send(encoded);
    } catch (error) {
      addLog(`send failed (${host.displayName}): ${error}`, {
        level: "error",
        scope: "ws_out",
        action,
        outcome: "failed",
        traceId,
        eventId,
        eventType,
        hostId,
        hostName: host.displayName,
        toolId,
        detail: String(error || ""),
      });
      return false;
    }

    state.eventOut += 1;
    addLog(formatWireLog("OUT", host.displayName, encoded), {
      scope: "ws_out",
      action,
      outcome: "sent",
      direction: "OUT",
      traceId,
      eventId,
      eventType,
      hostId,
      hostName: host.displayName,
      toolId,
      systemId: host.systemId,
      seq: event.seq,
    });
    return true;
  }

  /**
   * 请求 sidecar 刷新工具与快照。
   * @param {string} hostId 宿主机标识。
   */
  function requestToolsRefresh(hostId) {
    sendSocketEvent(hostId, "tools_refresh_request", {}, { action: "refresh_tools" });
  }

  /**
   * 请求 sidecar 刷新工具详情。
   * @param {string} hostId 宿主机标识。
   * @param {string} toolId 工具标识；为空时刷新全部工具详情。
   * @param {boolean} force 是否强制刷新。
   * @param {"user"|"background"} priority 刷新优先级。
   */
  function requestToolDetailsRefresh(hostId, toolId = "", force = false, priority = "background") {
    const runtime = ensureRuntime(hostId);
    const normalizedToolId = String(toolId || "").trim();
    const logicalToolId = normalizedToolId && typeof resolveLogicalToolId === "function"
      ? String(resolveLogicalToolId(hostId, normalizedToolId) || "").trim() || normalizedToolId
      : normalizedToolId;
    const runtimeToolId = logicalToolId && typeof resolveRuntimeToolId === "function"
      ? String(resolveRuntimeToolId(hostId, logicalToolId) || "").trim() || logicalToolId
      : logicalToolId;
    const normalizedPriority = String(priority || "").trim().toLowerCase() === "user"
      ? "user"
      : "background";
    const refreshId = createEventId().replace(/^evt_/, "drf_");
    const payload = {
      refreshId,
      force: Boolean(force),
      priority: normalizedPriority,
    };
    if (runtimeToolId) {
      payload.toolId = runtimeToolId;
    }
    if (runtime) {
      if (runtimeToolId) {
        runtime.toolDetailsPendingRefreshByToolId[runtimeToolId] = refreshId;
      } else {
        runtime.toolDetailsPendingAllRefreshId = refreshId;
      }
    }
    const sent = sendSocketEvent(hostId, "tool_details_refresh_request", payload, {
      action: "refresh_tool_details",
      toolId: runtimeToolId,
    });
    if (!sent && runtime) {
      if (runtimeToolId) {
        delete runtime.toolDetailsPendingRefreshByToolId[runtimeToolId];
      } else if (runtime.toolDetailsPendingAllRefreshId === refreshId) {
        runtime.toolDetailsPendingAllRefreshId = "";
      }
    }
    return sent;
  }

  /**
   * 请求重绑控制端。
   * @param {string} hostId 宿主机标识。
   */
  function requestControllerRebind(hostId) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (!host) {
      addLog("重绑失败：未选择宿主机");
      return;
    }
    if (!runtime || !runtime.connected) {
      addLog(`重绑失败：宿主机未连接 (${host.displayName})`);
      return;
    }
    const traceId = createEventId().replace(/^evt_/, "trc_");
    if (sendSocketEvent(hostId, "controller_rebind_request", { deviceId: state.deviceId }, {
      action: "rebind_controller",
      traceId,
    })) {
      addLog(`已请求重绑控制端 (${host.displayName})`, {
        scope: "controller",
        action: "rebind_controller",
        outcome: "started",
        traceId,
        hostId,
        hostName: host.displayName,
      });
    }
  }

  /**
   * 请求 sidecar 在指定目录启动工具 CLI。
   * @param {string} hostId 宿主机标识。
   * @param {object} input 启动参数。
   * @returns {{ok:boolean, requestId:string}}
   */
  function requestToolLaunch(hostId, input = {}) {
    const toolName = String(input.toolName || input.tool || "").trim();
    const cwd = String(input.cwd || "").trim();
    const conversationKey = String(input.conversationKey || "").trim();
    const requestId = String(input.requestId || "").trim() || createEventId().replace(/^evt_/, "lch_");
    if (!toolName || !cwd) {
      return { ok: false, requestId };
    }
    const ok = sendSocketEvent(hostId, "tool_launch_request", {
      toolName,
      cwd,
      requestId,
      conversationKey,
    }, {
      action: "tool_launch_request",
      traceId: requestId.replace(/^lch_/, "trc_"),
    });
    return { ok, requestId };
  }

  return {
    sendSocketEvent,
    requestToolsRefresh,
    requestToolDetailsRefresh,
    requestControllerRebind,
    requestToolLaunch,
  };
}
