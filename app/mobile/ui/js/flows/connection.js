// 文件职责：
// 1. 组装连接发送、连接生命周期、事件摄取三个子能力。
// 2. 对外暴露宿主机连接、调试发送与重绑等统一接口。

import { asMap } from "../utils/type.js";
import { state } from "../state/store.js";
import { createConnectionEvents } from "./connection-events.js";
import { createConnectionSendOps } from "./connection-send.js";
import { createConnectionSocketOps } from "./connection-socket.js";

/** 创建连接流程编排器。 */
export function createConnectionFlow({
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
  sanitizeTools,
  resolveLogicalToolId,
  resolveRuntimeToolId,
  syncOpencodeInvalidState,
  setToolHidden,
  resolveToolDisplayName,
  auth,
  queueDispatcher,
}) {
  const hooks = {
    openHostNoticeModal: () => {},
    renderAddToolModal: () => {},
    connectCandidateTool: () => {},
    onToolMediaStageProgress: () => {},
    onToolMediaStageFinished: () => {},
    onToolMediaStageFailed: () => {},
    onToolChatStarted: () => {},
    onToolChatChunk: () => {},
    onToolChatFinished: () => {},
    onToolReportFetchStarted: () => {},
    onToolReportFetchChunk: () => {},
    onToolReportFetchFinished: () => {},
    onToolLaunchStarted: () => {},
    onToolLaunchFinished: () => {},
    onToolLaunchFailed: () => {},
  };

  /**
   * 运行时注入来自其它流程的 UI hooks。
   * @param {object} nextHooks hook 集合。
   */
  function setHooks(nextHooks = {}) {
    Object.assign(hooks, asMap(nextHooks));
  }

  const sendOps = createConnectionSendOps({
    state,
    hostById,
    ensureRuntime,
    createEventId,
    addLog,
    formatWireLog,
    resolveLogicalToolId,
    resolveRuntimeToolId,
  });

  const events = createConnectionEvents({
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
    requestControllerRebind: sendOps.requestControllerRebind,
    connectCandidateTool: (hostId, toolId) => hooks.connectCandidateTool(hostId, toolId),
    openHostNoticeModal: (...args) => hooks.openHostNoticeModal(...args),
    requestToolsRefresh: sendOps.requestToolsRefresh,
    requestToolDetailsRefresh: sendOps.requestToolDetailsRefresh,
    renderAddToolModal: () => hooks.renderAddToolModal(),
    onToolMediaStageProgress: (...args) => hooks.onToolMediaStageProgress(...args),
    onToolMediaStageFinished: (...args) => hooks.onToolMediaStageFinished(...args),
    onToolMediaStageFailed: (...args) => hooks.onToolMediaStageFailed(...args),
    onToolChatStarted: (...args) => hooks.onToolChatStarted(...args),
    onToolChatChunk: (...args) => hooks.onToolChatChunk(...args),
    onToolChatFinished: (...args) => hooks.onToolChatFinished(...args),
    onToolReportFetchStarted: (...args) => hooks.onToolReportFetchStarted(...args),
    onToolReportFetchChunk: (...args) => hooks.onToolReportFetchChunk(...args),
    onToolReportFetchFinished: (...args) => hooks.onToolReportFetchFinished(...args),
    onToolLaunchStarted: (...args) => hooks.onToolLaunchStarted(...args),
    onToolLaunchFinished: (...args) => hooks.onToolLaunchFinished(...args),
    onToolLaunchFailed: (...args) => hooks.onToolLaunchFailed(...args),
    addLog,
    queueDispatcher,
  });

  const socketOps = createConnectionSocketOps({
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
    requestToolsRefresh: sendOps.requestToolsRefresh,
  });

  return {
    setHooks,
    connectAllHosts: socketOps.connectAllHosts,
    disconnectAllHosts: socketOps.disconnectAllHosts,
    reconnectHost: socketOps.reconnectHost,
    connectHost: socketOps.connectHost,
    disconnectHost: socketOps.disconnectHost,
    sendSocketEvent: sendOps.sendSocketEvent,
    requestToolsRefresh: sendOps.requestToolsRefresh,
    requestToolDetailsRefresh: sendOps.requestToolDetailsRefresh,
    requestControllerRebind: sendOps.requestControllerRebind,
    requestToolLaunch: sendOps.requestToolLaunch,
    isAnyHostConnected: socketOps.isAnyHostConnected,
    hasConnectableHost: socketOps.hasConnectableHost,
  };
}
