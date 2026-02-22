// 文件职责：
// 1. 处理 WebSocket 四类生命周期事件（open/message/close/error）。
// 2. 保证仅处理当前连接世代，避免旧连接污染运行态。

import { state } from "../state/store.js";

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
    addLog(`connected host: ${host.displayName}`);
    requestToolsRefresh(hostId);
    render();
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
    state.eventIn += 1;
    addLog(formatWireLog("IN", host.displayName, text));
    events.ingestEvent(hostId, text);
    render();
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
    addLog(`socket closed: ${host.displayName}`);
    scheduleReconnect(hostId, `socket closed (${host.displayName})`, manual);
    render();
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
    addLog(`socket error (${host.displayName}): ${current.lastError}`);
    render();
  }

  return {
    onSocketOpen,
    onSocketMessage,
    onSocketClose,
    onSocketError,
  };
}
