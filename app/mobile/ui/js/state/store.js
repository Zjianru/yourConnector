// 文件职责：
// 1. 承载移动端页面的全局常量、状态树和运行时默认值。
// 2. 暴露由 ui-refs 构建的 DOM 节点引用集合。
// 3. 提供原始日志开关与 Runtime 创建器，供各功能模块复用。

import { createUiRefs } from "./ui-refs.js";

export const STORAGE_KEY = "yc_mobile_tauri_hosts_v2";
export const LEGACY_STORAGE_KEY = "yc_mobile_tauri_debug";
export const DEFAULT_RELAY_WS_URL = "ws://127.0.0.1:18080/v1/ws";
export const RECONNECT_INTERVAL_MS = 2000;
export const MAX_RECONNECT_ATTEMPTS = 5;
export const DELETE_RETRY_INTERVAL_MS = 2000;

export const RAW_PAYLOAD_DEBUG = (() => {
  try {
    const byLocalStorage = localStorage.getItem("yc_debug_raw_payload");
    if (byLocalStorage === "1") {
      return true;
    }
  } catch (_) {
    // ignore localStorage errors
  }

  try {
    const query = new URLSearchParams(window.location.search || "");
    return query.get("rawPayload") === "1";
  } catch (_) {
    return false;
  }
})();

/**
 * 构造单宿主机运行时状态默认值。
 * @returns {object} 新建的运行时状态对象。
 */
export function createRuntime() {
  return {
    socket: null,
    connectionEpoch: 0,
    connecting: false,
    connected: false,
    status: "DISCONNECTED",
    sidecarStatus: "UNKNOWN",
    lastHeartbeatAt: null,
    reconnectTimer: null,
    retryCount: 0,
    manualReconnectRequired: false,
    lastError: "",

    systemMetrics: {},
    sidecarMetrics: {},
    primaryToolMetrics: {},
    toolMetricsById: {},
    tools: [],
    candidateTools: [],
    connectingToolIds: {},
    toolConnectRetryCount: {},
    toolConnectTimers: {},

    accessToken: "",
    refreshToken: "",
    keyId: "",
    credentialId: "",
    devicePublicKey: "",
  };
}

export const state = {
  // 宿主机配置与运行时。
  deviceId: "",
  hosts: [],
  selectedHostId: "",
  runtimes: {},
  pendingHostDeletes: [],
  toolAliases: {},
  toolVisibility: {},

  // 全局 UI 状态。
  activeTab: "ops",
  bannerActiveIndex: 0,
  logs: [],
  eventIn: 0,
  eventOut: 0,
  message: "tool_ping",
  debugHostId: "",

  // 弹窗与临时操作状态。
  detailHostId: "",
  detailToolId: "",
  detailExpanded: false,
  addToolHostId: "",
  pairingBusy: false,
  pairFlowStep: "import",
  pairFailurePrimaryAction: "",
  pairTargetHostId: "",
  activeToolSwipeKey: "",
  editingHostId: "",
  hostMetricsHostId: "",
  hostNoticeTargetId: "",
  hostNoticePrimaryAction: "dismiss",

  // 扫码状态。
  scanDetector: null,
  scanStream: null,
  scanning: false,

  // 删除补偿执行状态。
  deleteCompensating: false,
};

// 统一 DOM 引用出口，供流程层与渲染层共享。
export const ui = createUiRefs();
