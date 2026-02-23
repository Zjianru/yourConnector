// 文件职责：
// 1. 处理 WebSocket 四类生命周期事件（open/message/close/error）。
// 2. 保证仅处理当前连接世代，避免旧连接污染运行态。

import { state } from "../state/store.js";
import { extractWireMeta } from "../utils/log.js";

/**
 * 创建 socket 事件处理器。
 * @param {object} deps 依赖集合。
 * @returns {{onSocketOpen: Function,onSocketMessage: Function,onSocketClose: Function,onSocketError: Function}}
 */
export function createSocketEventHandlers({
  hostById,
  ensureRuntime,
  addLog,
  formatWireLog,
  render,
  clearReconnectTimer,
  requestToolsRefresh,
  events,
}) {
  let renderFrame = null;

  /**
   * 合并同一帧内的多次渲染请求，降低快照高频刷新导致的抖动。
   */
  function requestRender() {
    if (renderFrame !== null) {
      return;
    }
    renderFrame = window.requestAnimationFrame(() => {
      renderFrame = null;
      render();
    });
  }

  /**
   * 左滑操作区打开时，延迟低优先级快照渲染，避免操作区被刷新打断。
   * @param {string} eventType 事件类型。
   * @returns {boolean}
   */
  function shouldDeferRenderWhenSwipeOpen(eventType) {
    if (!state.activeToolSwipeKey) {
      return false;
    }
    return [
      "heartbeat",
      "metrics_snapshot",
      "tools_snapshot",
      "tools_candidates",
      "tool_details_snapshot",
    ].includes(String(eventType || ""));
  }

  /**
   * 处理 socket open。
   * @param {string} hostId 宿主机标识。
   * @param {WebSocket} socket socket 实例。
   * @param {number} connectionEpoch 连接世代。
   */
  function onSocketOpen(hostId, socket, connectionEpoch) {
    const current = ensureRuntime(hostId);
    const host = hostById(hostId);
    if (!current || !host || current.connectionEpoch !== connectionEpoch || current.socket !== socket) return;

    current.connected = true;
    current.connecting = false;
    current.status = "CONNECTED";
    current.sidecarStatus = "ONLINE";
    current.retryCount = 0;
    current.manualReconnectRequired = false;
    current.lastError = "";
    clearReconnectTimer(current);
    addLog(`connected host: ${host.displayName}`, {
      scope: "connection",
      action: "connect_host",
      outcome: "success",
      hostId,
      hostName: host.displayName,
      systemId: host.systemId,
    });
    requestToolsRefresh(hostId);
    requestRender();
  }

  /**
   * 处理 socket message。
   * @param {string} hostId 宿主机标识。
   * @param {WebSocket} socket socket 实例。
   * @param {number} connectionEpoch 连接世代。
   * @param {MessageEvent<string>} event 浏览器消息事件。
   */
  function onSocketMessage(hostId, socket, connectionEpoch, event) {
    const current = ensureRuntime(hostId);
    const host = hostById(hostId);
    if (!current || !host || current.connectionEpoch !== connectionEpoch || current.socket !== socket) return;

    const text = String(event.data || "");
    const wireMeta = extractWireMeta(text);
    state.eventIn += 1;
    addLog(formatWireLog("IN", host.displayName, text), {
      scope: "ws_in",
      action: wireMeta.eventType || "incoming_event",
      outcome: "received",
      direction: "IN",
      traceId: wireMeta.traceId || "",
      eventId: wireMeta.eventId || "",
      eventType: wireMeta.eventType || "",
      hostId,
      hostName: host.displayName,
      toolId: wireMeta.toolId || "",
      systemId: wireMeta.systemId || host.systemId,
      sourceClientType: wireMeta.sourceClientType || "",
      sourceDeviceId: wireMeta.sourceDeviceId || "",
      seq: Number(wireMeta.seq || 0),
    });
    events.ingestEvent(hostId, text);
    if (!shouldDeferRenderWhenSwipeOpen(wireMeta.eventType)) {
      requestRender();
    }
  }

  /**
   * 处理 socket close。
   * @param {string} hostId 宿主机标识。
   * @param {WebSocket} socket socket 实例。
   * @param {number} connectionEpoch 连接世代。
   * @param {boolean} manual 是否来自手动触发。
   * @param {(hostId: string, reason: string, manual: boolean) => void} scheduleReconnect 重连调度函数。
   */
  function onSocketClose(hostId, socket, connectionEpoch, manual, scheduleReconnect) {
    const current = ensureRuntime(hostId);
    const host = hostById(hostId);
    if (!current || !host || current.connectionEpoch !== connectionEpoch || current.socket !== socket) return;

    current.connected = false;
    current.connecting = false;
    current.socket = null;
    current.status = "DISCONNECTED";
    addLog(`socket closed: ${host.displayName}`, {
      level: "warn",
      scope: "connection",
      action: "socket_close",
      outcome: "closed",
      hostId,
      hostName: host.displayName,
      systemId: host.systemId,
    });
    scheduleReconnect(hostId, `socket closed (${host.displayName})`, manual);
    requestRender();
  }

  /**
   * 处理 socket error。
   * @param {string} hostId 宿主机标识。
   * @param {WebSocket} socket socket 实例。
   * @param {number} connectionEpoch 连接世代。
   * @param {Event|Error} error 错误对象。
   */
  function onSocketError(hostId, socket, connectionEpoch, error) {
    const current = ensureRuntime(hostId);
    const host = hostById(hostId);
    if (!current || !host || current.connectionEpoch !== connectionEpoch || current.socket !== socket) return;

    current.connected = false;
    current.connecting = false;
    current.status = "RELAY_UNREACHABLE";
    current.lastError = String(error && error.message ? error.message : "socket error");
    addLog(`socket error (${host.displayName}): ${current.lastError}`, {
      level: "error",
      scope: "connection",
      action: "socket_error",
      outcome: "failed",
      hostId,
      hostName: host.displayName,
      systemId: host.systemId,
      detail: current.lastError,
    });
    requestRender();
  }

  return {
    onSocketOpen,
    onSocketMessage,
    onSocketClose,
    onSocketError,
  };
}
