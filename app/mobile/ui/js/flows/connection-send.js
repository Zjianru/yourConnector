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
}) {
  /**
   * 发送统一协议事件。
   * @param {string} hostId 宿主机标识。
   * @param {string} type 事件类型。
   * @param {object} payload 事件载荷。
   * @returns {boolean} 是否发送成功。
   */
  function sendSocketEvent(hostId, type, payload) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (!host || !runtime || !runtime.socket || !runtime.connected) {
      addLog(`send skipped: host not connected (${host ? host.displayName : hostId})`);
      return false;
    }

    const event = {
      v: 1,
      eventId: createEventId(),
      type,
      systemId: host.systemId,
      seq: Date.now(),
      ts: new Date().toISOString(),
      payload: asMap(payload),
    };
    const encoded = JSON.stringify(event);

    try {
      runtime.socket.send(encoded);
    } catch (error) {
      addLog(`send failed (${host.displayName}): ${error}`);
      return false;
    }

    state.eventOut += 1;
    addLog(formatWireLog("OUT", host.displayName, encoded));
    return true;
  }

  /**
   * 请求 sidecar 刷新工具与快照。
   * @param {string} hostId 宿主机标识。
   */
  function requestToolsRefresh(hostId) {
    sendSocketEvent(hostId, "tools_refresh_request", {});
  }

  /**
   * 请求 sidecar 刷新工具详情。
   * @param {string} hostId 宿主机标识。
   * @param {string} toolId 工具标识；为空时刷新全部工具详情。
   * @param {boolean} force 是否强制刷新。
   */
  function requestToolDetailsRefresh(hostId, toolId = "", force = false) {
    const normalizedToolId = String(toolId || "").trim();
    const payload = {
      force: Boolean(force),
    };
    if (normalizedToolId) {
      payload.toolId = normalizedToolId;
    }
    sendSocketEvent(hostId, "tool_details_refresh_request", payload);
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
    if (sendSocketEvent(hostId, "controller_rebind_request", { deviceId: state.deviceId })) {
      addLog(`已请求重绑控制端 (${host.displayName})`);
    }
  }

  return {
    sendSocketEvent,
    requestToolsRefresh,
    requestToolDetailsRefresh,
    requestControllerRebind,
  };
}
