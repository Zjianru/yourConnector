// 文件职责：
// 1. 承载移动端页面的全局常量、状态树和运行时默认值。
// 2. 提供 DOM 节点集合，避免在业务流程中重复查询。
// 3. 提供原始日志开关与 Runtime 创建器，供各功能模块复用。

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

export const ui = {
  // 基础页面。
  opsView: document.getElementById("opsView"),
  debugView: document.getElementById("debugView"),
  tabOps: document.getElementById("tabOps"),
  tabDebug: document.getElementById("tabDebug"),

  // 顶部操作。
  connectBtnTop: document.getElementById("connectBtnTop"),
  disconnectBtnTop: document.getElementById("disconnectBtnTop"),
  replaceHostBtnTop: document.getElementById("replaceHostBtnTop"),

  // 配对与总览。
  hostSetupCard: document.getElementById("hostSetupCard"),
  hostOverviewWrap: document.getElementById("hostOverviewWrap"),
  importPairLinkBtn: document.getElementById("importPairLinkBtn"),
  openManualPairBtn: document.getElementById("openManualPairBtn"),
  openDebugFromSetupBtn: document.getElementById("openDebugFromSetupBtn"),
  hostBannerTrack: document.getElementById("hostBannerTrack"),
  hostBannerDots: document.getElementById("hostBannerDots"),
  toolsGroupedList: document.getElementById("toolsGroupedList"),

  // 调试页。
  debugStatus: document.getElementById("debugStatus"),
  debugEvents: document.getElementById("debugEvents"),
  debugHostSelect: document.getElementById("debugHostSelect"),
  debugIdentity: document.getElementById("debugIdentity"),
  connectBtnDebug: document.getElementById("connectBtnDebug"),
  disconnectBtnDebug: document.getElementById("disconnectBtnDebug"),
  rebindControllerBtn: document.getElementById("rebindControllerBtn"),
  messageInput: document.getElementById("messageInput"),
  sendBtn: document.getElementById("sendBtn"),
  logBox: document.getElementById("logBox"),

  // 工具详情。
  toolModal: document.getElementById("toolModal"),
  toolModalTitle: document.getElementById("toolModalTitle"),
  toolModalClose: document.getElementById("toolModalClose"),
  summaryRows: document.getElementById("summaryRows"),
  detailRows: document.getElementById("detailRows"),
  detailTip: document.getElementById("detailTip"),
  toggleDetailsBtn: document.getElementById("toggleDetailsBtn"),
  usagePanel: document.getElementById("usagePanel"),
  usageRows: document.getElementById("usageRows"),

  // 添加工具。
  addToolModal: document.getElementById("addToolModal"),
  addToolModalClose: document.getElementById("addToolModalClose"),
  candidateList: document.getElementById("candidateList"),
  goDebugFromAddTool: document.getElementById("goDebugFromAddTool"),

  // 配对流程。
  pairFlowModal: document.getElementById("pairFlowModal"),
  pairFlowTitle: document.getElementById("pairFlowTitle"),
  pairFlowClose: document.getElementById("pairFlowClose"),
  pairFlowStepImport: document.getElementById("pairFlowStepImport"),
  pairFlowStepPaste: document.getElementById("pairFlowStepPaste"),
  pairFlowStepScan: document.getElementById("pairFlowStepScan"),
  pairFlowStepManual: document.getElementById("pairFlowStepManual"),
  pairOpenScanBtn: document.getElementById("pairOpenScanBtn"),
  pairOpenPasteBtn: document.getElementById("pairOpenPasteBtn"),
  pairPasteSubmitBtn: document.getElementById("pairPasteSubmitBtn"),
  pairPasteBackBtn: document.getElementById("pairPasteBackBtn"),
  pairScanVideo: document.getElementById("pairScanVideo"),
  pairScanStatus: document.getElementById("pairScanStatus"),
  pairScanFileInput: document.getElementById("pairScanFileInput"),
  pairScanGalleryBtn: document.getElementById("pairScanGalleryBtn"),
  pairScanBackBtn: document.getElementById("pairScanBackBtn"),
  hostRelayInput: document.getElementById("hostRelayInput"),
  hostSystemIdInput: document.getElementById("hostSystemIdInput"),
  hostPairTicketInput: document.getElementById("hostPairTicketInput"),
  hostNameInput: document.getElementById("hostNameInput"),
  pairManualSubmitBtn: document.getElementById("pairManualSubmitBtn"),
  pairManualBackBtn: document.getElementById("pairManualBackBtn"),
  pairLinkInput: document.getElementById("pairLinkInput"),

  // 配对失败弹窗。
  pairFailureModal: document.getElementById("pairFailureModal"),
  pairFailureClose: document.getElementById("pairFailureClose"),
  pairFailureReason: document.getElementById("pairFailureReason"),
  pairFailureSuggestion: document.getElementById("pairFailureSuggestion"),
  pairFailurePrimaryBtn: document.getElementById("pairFailurePrimaryBtn"),
  pairFailureSecondaryBtn: document.getElementById("pairFailureSecondaryBtn"),

  // 宿主管理。
  hostManageModal: document.getElementById("hostManageModal"),
  hostManageClose: document.getElementById("hostManageClose"),
  hostManageList: document.getElementById("hostManageList"),
  pendingDeleteList: document.getElementById("pendingDeleteList"),
  hostManageAddBtn: document.getElementById("hostManageAddBtn"),
  hostManageDebugBtn: document.getElementById("hostManageDebugBtn"),

  // 宿主机编辑。
  hostEditModal: document.getElementById("hostEditModal"),
  hostEditClose: document.getElementById("hostEditClose"),
  hostEditNameInput: document.getElementById("hostEditNameInput"),
  hostEditNoteInput: document.getElementById("hostEditNoteInput"),
  hostEditSaveBtn: document.getElementById("hostEditSaveBtn"),
  hostEditCancelBtn: document.getElementById("hostEditCancelBtn"),

  // 宿主机负载弹窗。
  hostMetricsModal: document.getElementById("hostMetricsModal"),
  hostMetricsTitle: document.getElementById("hostMetricsTitle"),
  hostMetricsClose: document.getElementById("hostMetricsClose"),
  hostMetricsRows: document.getElementById("hostMetricsRows"),
  hostSidecarRows: document.getElementById("hostSidecarRows"),
  hostMetricsTip: document.getElementById("hostMetricsTip"),

  // 通知弹窗（重名/删除补偿提示）。
  hostNoticeModal: document.getElementById("hostNoticeModal"),
  hostNoticeClose: document.getElementById("hostNoticeClose"),
  hostNoticeTitle: document.getElementById("hostNoticeTitle"),
  hostNoticeBody: document.getElementById("hostNoticeBody"),
  hostNoticePrimaryBtn: document.getElementById("hostNoticePrimaryBtn"),
  hostNoticeSecondaryBtn: document.getElementById("hostNoticeSecondaryBtn"),
};
