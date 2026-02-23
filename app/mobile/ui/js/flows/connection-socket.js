// 文件职责：
// 1. 管理宿主机 WebSocket 生命周期（连接/断开/重连）。
// 2. 将消息收发事件桥接到事件处理器与渲染层。

import { buildAppWsUrl } from "../services/ws.js";
import { asBool } from "../utils/type.js";
import { MAX_RECONNECT_ATTEMPTS, RECONNECT_INTERVAL_MS, state } from "../state/store.js";
import { createSocketEventHandlers } from "./connection-socket-events.js";

/** 创建连接生命周期能力。 */
export function createConnectionSocketOps({
  visibleHosts,
  hostById,
  ensureRuntime,
  createEventId,
  tauriInvoke,
  addLog,
  formatWireLog,
  render,
  clearReconnectTimer,
  clearToolConnectTimer,
  auth,
  events,
  requestToolsRefresh,
}) {
  const socketEvents = createSocketEventHandlers({
    hostById,
    ensureRuntime,
    addLog,
    formatWireLog,
    render,
    clearReconnectTimer,
    requestToolsRefresh,
    events,
  });

  /**
   * 是否存在已连接宿主机。
   * @returns {boolean}
   */
  function isAnyHostConnected() {
    return visibleHosts().some((host) => {
      const runtime = ensureRuntime(host.hostId);
      return runtime && runtime.connected;
    });
  }

  /**
   * 是否存在可发起连接的宿主机。
   * @returns {boolean}
   */
  function hasConnectableHost() {
    return visibleHosts().some((host) => {
      const runtime = ensureRuntime(host.hostId);
      return runtime && !runtime.connected && !runtime.connecting;
    });
  }

  /**
   * 安排固定间隔重连。
   * @param {string} hostId 宿主机标识。
   * @param {string} reason 触发原因。
   * @param {boolean} manualTriggered 是否手动触发链路。
   */
  function scheduleReconnect(hostId, reason, manualTriggered) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (
      !host
      || !runtime
      || runtime.manualReconnectRequired
      || runtime.reconnectTimer
      || host.autoConnect === false
    ) {
      return;
    }
    if (runtime.retryCount >= MAX_RECONNECT_ATTEMPTS) {
      runtime.manualReconnectRequired = true;
      runtime.status = "DISCONNECTED";
      runtime.lastError = `重连失败已达 ${MAX_RECONNECT_ATTEMPTS} 次`;
      addLog(`reconnect paused (${host.displayName}): 超过 ${MAX_RECONNECT_ATTEMPTS} 次失败，请手动重连`);
      return;
    }

    runtime.retryCount += 1;
    runtime.reconnectTimer = setTimeout(() => {
      runtime.reconnectTimer = null;
      void connectHost(hostId, { manual: manualTriggered, resetRetry: false });
    }, RECONNECT_INTERVAL_MS);
    addLog(`reconnect scheduled (${host.displayName}) #${runtime.retryCount}: ${reason}`);
  }

  /**
   * 建立单宿主机连接。
   * @param {string} hostId 宿主机标识。
   * @param {{manual?: boolean, resetRetry?: boolean}} options 连接选项。
   */
  async function connectHost(hostId, options = {}) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (!host || !runtime) return;

    const manual = asBool(options.manual);
    const resetRetry = asBool(options.resetRetry);
    if (resetRetry) {
      runtime.retryCount = 0;
      runtime.manualReconnectRequired = false;
      runtime.lastError = "";
      clearReconnectTimer(runtime);
    }
    if (runtime.connecting || runtime.connected) return;

    runtime.connecting = true;
    runtime.status = "CONNECTING";
    render();

    try {
      await auth.loadHostSession(hostId);
      if (!runtime.accessToken || !runtime.refreshToken || !runtime.keyId) {
        runtime.connecting = false;
        runtime.connected = false;
        runtime.status = "AUTH_EXPIRED";
        runtime.manualReconnectRequired = true;
        runtime.lastError = "缺少设备凭证，请重新配对";
        addLog(`connect skipped (${host.displayName}): 缺少可用凭证，请先完成配对`);
        render();
        return;
      }

      await auth.refreshAccessTokenIfPossible(hostId);
      const connectionEpoch = runtime.connectionEpoch + 1;
      runtime.connectionEpoch = connectionEpoch;

      const ts = String(Math.floor(Date.now() / 1000));
      const nonce = createEventId();
      const payload = `ws\n${host.systemId}\n${state.deviceId}\n${runtime.keyId}\n${ts}\n${nonce}`;
      const signed = await tauriInvoke("auth_sign_payload", { deviceId: state.deviceId, payload });
      const url = buildAppWsUrl({
        relayUrl: host.relayUrl,
        systemId: host.systemId,
        deviceId: state.deviceId,
        accessToken: runtime.accessToken,
        keyId: String(signed.keyId || runtime.keyId),
        ts,
        nonce,
        sig: String(signed.signature || ""),
      });

      const socket = new WebSocket(url.toString());
      runtime.socket = socket;

      socket.addEventListener("open", () => {
        socketEvents.onSocketOpen(hostId, socket, connectionEpoch);
      });
      socket.addEventListener("message", (event) => {
        socketEvents.onSocketMessage(hostId, socket, connectionEpoch, event);
      });
      socket.addEventListener("close", () => {
        socketEvents.onSocketClose(
          hostId,
          socket,
          connectionEpoch,
          manual,
          scheduleReconnect,
        );
      });
      socket.addEventListener("error", (error) => {
        socketEvents.onSocketError(hostId, socket, connectionEpoch, error);
      });
    } catch (error) {
      runtime.connected = false;
      runtime.connecting = false;
      runtime.socket = null;
      runtime.status = "RELAY_UNREACHABLE";
      runtime.lastError = String(error || "connect failed");
      addLog(`connect failed (${host.displayName}): ${error}`);
      scheduleReconnect(hostId, String(error || "connect failed"), manual);
      render();
    }
  }

  /**
   * 断开单宿主机连接。
   * @param {string} hostId 宿主机标识。
   * @param {{triggerReconnect?: boolean}} options 断开选项。
   */
  async function disconnectHost(hostId, options = {}) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) return;

    const triggerReconnect = options.triggerReconnect !== false;
    runtime.connectionEpoch += 1;
    clearReconnectTimer(runtime);

    const socket = runtime.socket;
    runtime.socket = null;
    if (socket) socket.close();

    runtime.connected = false;
    runtime.connecting = false;
    runtime.status = "DISCONNECTED";
    runtime.sidecarStatus = "UNKNOWN";
    runtime.lastHeartbeatAt = null;
    runtime.candidateTools = [];
    runtime.connectingToolIds = {};
    runtime.toolConnectRetryCount = {};
    Object.keys(runtime.toolConnectTimers || {}).forEach((toolId) => clearToolConnectTimer(runtime, toolId));
    runtime.toolDetailsById = {};
    runtime.toolDetailStaleById = {};
    runtime.toolDetailUpdatedAtById = {};

    const host = hostById(hostId);
    addLog(`disconnected host: ${host ? host.displayName : hostId}`);
    if (triggerReconnect) scheduleReconnect(hostId, "manual disconnect", true);
    render();
  }

  /**
   * 并行连接全部宿主机。
   */
  async function connectAllHosts() {
    const hosts = visibleHosts();
    await Promise.allSettled(hosts.map((host) => connectHost(host.hostId, { manual: true, resetRetry: true })));
  }

  /**
   * 并行断开全部宿主机。
   */
  async function disconnectAllHosts() {
    const hosts = visibleHosts();
    await Promise.allSettled(hosts.map((host) => disconnectHost(host.hostId, { triggerReconnect: false })));
  }

  /**
   * 手动重连指定宿主机。
   * @param {string} hostId 宿主机标识。
   */
  async function reconnectHost(hostId) {
    await disconnectHost(hostId, { triggerReconnect: false });
    await connectHost(hostId, { manual: true, resetRetry: true });
  }

  return {
    isAnyHostConnected,
    hasConnectableHost,
    connectHost,
    disconnectHost,
    reconnectHost,
    connectAllHosts,
    disconnectAllHosts,
  };
}
