// 文件职责（主入口）：
// 1. 组装状态层、流程层、渲染层与弹窗层。
// 2. 绑定页面事件并触发统一 render。
// 3. 保持业务逻辑不落在入口文件。

import { RAW_PAYLOAD_DEBUG, DELETE_RETRY_INTERVAL_MS, state, ui } from "./state/store.js";
import { tauriInvoke } from "./services/tauri.js";
import { formatWireLog as formatWireLogRaw, addLog as pushLog } from "./utils/log.js";
import { escapeHtml } from "./utils/dom.js";
import { maskSecret } from "./utils/format.js";
import { renderTabs as renderTabsView } from "./views/tabs.js";
import { renderDebugPanel as renderDebugPanelView } from "./views/debug.js";
import { createChatView } from "./views/chat.js";
import { renderBanner as renderBannerView, renderHostStage as renderHostStageView } from "./views/banner.js";
import { renderTopActions as renderTopActionsView, syncBannerActiveIndex, extractBannerHostId } from "./views/ops.js";
import { createToolsView } from "./views/tools.js";
import { createHostState } from "./state/hosts.js";
import { createRuntimeState } from "./state/runtime.js";
import { createConnectionAuth } from "./flows/connection-auth.js";
import { createConnectionFlow } from "./flows/connection.js";
import { createPairingFlow } from "./flows/pairing.js";
import { createToolManageFlow } from "./flows/tool-manage.js";
import { createHostManageFlow } from "./flows/host-manage.js";
import { createChatFlow } from "./flows/chat.js";
import { createHostNoticeModal } from "./modals/host-notice.js";
import { createPairFailureModal } from "./modals/pair-failure.js";
import { createToolDetailModal } from "./modals/tool-detail.js";
import { createAddToolModal } from "./modals/add-tool.js";
import { createReportViewerModal } from "./modals/report-viewer.js";

const hostState = createHostState();
const runtimeState = createRuntimeState({ persistConfig: hostState.persistConfig });

const noticeModal = createHostNoticeModal({ state, ui });
const pairFailureModal = createPairFailureModal({ state, ui });
const reportViewerModal = createReportViewerModal({ state, ui });

const authFlow = createConnectionAuth({
  state,
  hostById: hostState.hostById,
  ensureRuntime: runtimeState.ensureRuntime,
  createEventId: hostState.createEventId,
  tauriInvoke,
  addLog,
});

let hostManageFlowRef = null;
const pairingFlow = createPairingFlow({
  state,
  ui,
  hostById: hostState.hostById,
  showPairFailure: pairFailureModal.showPairFailure,
  closePairFailureModal: pairFailureModal.closePairFailureModal,
  closeHostManageModal: () => hostManageFlowRef && hostManageFlowRef.closeHostManageModal(),
  createEventId: hostState.createEventId,
  ensureRuntime: runtimeState.ensureRuntime,
  recomputeSelections: hostState.recomputeSelections,
  persistConfig: hostState.persistConfig,
  storeHostSession: authFlow.storeHostSession,
  connectHost: (...args) => connectionFlow.connectHost(...args),
  notifyIfDuplicateDisplayName: (hostId) => hostManageFlowRef && hostManageFlowRef.notifyIfDuplicateDisplayName(hostId),
  tauriInvoke,
});

const connectionFlow = createConnectionFlow({
  visibleHosts: hostState.visibleHosts,
  hostById: hostState.hostById,
  ensureRuntime: runtimeState.ensureRuntime,
  createEventId: hostState.createEventId,
  tauriInvoke,
  addLog,
  formatWireLog,
  render,
  clearReconnectTimer: runtimeState.clearReconnectTimer,
  clearToolConnectTimer: runtimeState.clearToolConnectTimer,
  sanitizeTools: runtimeState.sanitizeTools,
  resolveLogicalToolId: runtimeState.resolveLogicalToolId,
  resolveRuntimeToolId: runtimeState.resolveRuntimeToolId,
  syncOpencodeInvalidState: runtimeState.syncOpencodeInvalidState,
  setToolHidden: runtimeState.setToolHidden,
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
  auth: authFlow,
});

const toolsView = createToolsView({
  state,
  ui,
  ensureRuntime: runtimeState.ensureRuntime,
  metricForTool: runtimeState.metricForTool,
  detailForTool: runtimeState.detailForTool,
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
  hostStatusLabel: runtimeState.hostStatusLabel,
});
const chatView = createChatView({ state, ui });

const addToolModal = createAddToolModal({
  state,
  ui,
  hostById: hostState.hostById,
  ensureRuntime: runtimeState.ensureRuntime,
  requestToolsRefresh: connectionFlow.requestToolsRefresh,
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
});

const toolDetailModal = createToolDetailModal({
  state,
  ui,
  hostById: hostState.hostById,
  ensureRuntime: runtimeState.ensureRuntime,
  metricForTool: runtimeState.metricForTool,
  detailForTool: runtimeState.detailForTool,
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
  requestToolDetailsRefresh: connectionFlow.requestToolDetailsRefresh,
});

/**
 * 统一关闭弹窗栈，避免多个 modal 叠加导致“关一个露一个”。
 * @param {"none"|"pairFailure"|"pairFlow"|"toolDetail"|"addTool"|"hostManage"|"hostEdit"|"hostMetrics"|"hostNotice"|"reportViewer"} keep 保留的弹窗类型。
 */
function closeModalStack(keep = "none") {
  if (keep !== "pairFailure") pairFailureModal.closePairFailureModal();
  if (keep !== "pairFlow") pairingFlow.closePairFlow();
  if (keep !== "toolDetail") toolDetailModal.closeToolDetail();
  if (keep !== "addTool") addToolModal.closeAddToolModal();
  if (keep !== "hostManage" && hostManageFlowRef) hostManageFlowRef.closeHostManageModal();
  if (keep !== "hostEdit" && hostManageFlowRef) hostManageFlowRef.closeHostEditModal();
  if (keep !== "hostMetrics" && hostManageFlowRef) hostManageFlowRef.closeHostMetricsModal();
  if (keep !== "hostNotice") noticeModal.closeHostNoticeModal();
  if (keep !== "reportViewer") reportViewerModal.closeReportViewer();
}

/** 打开宿主管理弹窗（互斥模式）。 */
function openHostManageModalGuard() {
  closeModalStack("hostManage");
  if (hostManageFlowRef) hostManageFlowRef.openHostManageModal();
}

/**
 * 打开配对流程弹窗（互斥模式）。
 * @param {string} step 配对步骤。
 * @param {string} targetHostId 目标宿主机。
 */
function openPairFlowGuard(step, targetHostId = "") {
  closeModalStack("pairFlow");
  pairingFlow.openPairFlow(step, targetHostId);
}

/**
 * 打开添加工具弹窗（互斥模式）。
 * @param {string} hostId 宿主机标识。
 */
function openAddToolModalGuard(hostId) {
  closeModalStack("addTool");
  addToolModal.openAddToolModal(hostId);
}

/**
 * 打开工具详情弹窗（互斥模式）。
 * @param {string} hostId 宿主机标识。
 * @param {string} toolId 工具标识。
 */
function openToolDetailGuard(hostId, toolId) {
  closeModalStack("toolDetail");
  toolDetailModal.openToolDetail(hostId, toolId);
}

/**
 * 打开宿主机负载弹窗（互斥模式）。
 * @param {string} hostId 宿主机标识。
 */
function openHostMetricsModalGuard(hostId) {
  closeModalStack("hostMetrics");
  if (hostManageFlowRef) hostManageFlowRef.openHostMetricsModal(hostId);
}

/**
 * 打开提示弹窗（互斥模式）。
 * @param {string} title 标题。
 * @param {string} body 正文。
 * @param {object|string} options 选项。
 */
function openHostNoticeModalGuard(title, body, options = {}) {
  const keepAddToolOpen = Boolean(
    options && typeof options === "object" && options.keepAddToolOpen,
  );
  closeModalStack(keepAddToolOpen ? "addTool" : "hostNotice");
  noticeModal.openHostNoticeModal(title, body, options);
}

const chatFlow = createChatFlow({
  state,
  visibleHosts: hostState.visibleHosts,
  hostById: hostState.hostById,
  ensureRuntime: runtimeState.ensureRuntime,
  resolveLogicalToolId: runtimeState.resolveLogicalToolId,
  resolveRuntimeToolId: runtimeState.resolveRuntimeToolId,
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
  sendSocketEvent: connectionFlow.sendSocketEvent,
  addLog,
  tauriInvoke,
  render,
});

const toolManageFlow = createToolManageFlow({
  state,
  hostById: hostState.hostById,
  ensureRuntime: runtimeState.ensureRuntime,
  resolveLogicalToolId: runtimeState.resolveLogicalToolId,
  resolveRuntimeToolId: runtimeState.resolveRuntimeToolId,
  clearToolBinding: runtimeState.clearToolBinding,
  setToolHidden: runtimeState.setToolHidden,
  getToolAlias: runtimeState.getToolAlias,
  setToolAlias: runtimeState.setToolAlias,
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
  clearToolConnectTimer: runtimeState.clearToolConnectTimer,
  addLog,
  sendSocketEvent: connectionFlow.sendSocketEvent,
  requestToolsRefresh: connectionFlow.requestToolsRefresh,
  requestControllerRebind: connectionFlow.requestControllerRebind,
  connectHost: connectionFlow.connectHost,
  reconnectHost: connectionFlow.reconnectHost,
  disconnectHost: connectionFlow.disconnectHost,
  openHostNoticeModal: openHostNoticeModalGuard,
  openAddToolModal: openAddToolModalGuard,
  renderAddToolModal: addToolModal.renderAddToolModal,
  closeAddToolModal: addToolModal.closeAddToolModal,
  openToolDetail: openToolDetailGuard,
  openHostManageModal: openHostManageModalGuard,
  deleteChatConversationByTool: chatFlow.deleteConversationByTool,
  render,
});

hostManageFlowRef = createHostManageFlow({
  state,
  ui,
  visibleHosts: hostState.visibleHosts,
  hostById: hostState.hostById,
  ensureRuntime: runtimeState.ensureRuntime,
  hostStatusLabel: runtimeState.hostStatusLabel,
  recomputeSelections: hostState.recomputeSelections,
  persistConfig: hostState.persistConfig,
  createEventId: hostState.createEventId,
  tauriInvoke,
  disposeRuntime: runtimeState.disposeRuntime,
  clearToolMetaForHost: runtimeState.clearToolMetaForHost,
  deleteChatConversationsByHost: chatFlow.deleteConversationsByHost,
  clearHostSession: authFlow.clearHostSession,
  addLog,
  openHostNoticeModal: openHostNoticeModalGuard,
  connectHost: connectionFlow.connectHost,
  reconnectHost: connectionFlow.reconnectHost,
  disconnectHost: connectionFlow.disconnectHost,
  sendSocketEvent: connectionFlow.sendSocketEvent,
  openPairFlow: openPairFlowGuard,
  render,
});

addToolModal.setHandlers({
  connectCandidateTool: toolManageFlow.connectCandidateTool,
  openDebug: () => switchTab("debug"),
});
connectionFlow.setHooks({
  openHostNoticeModal: openHostNoticeModalGuard,
  renderAddToolModal: addToolModal.renderAddToolModal,
  connectCandidateTool: toolManageFlow.connectCandidateTool,
  onToolChatStarted: chatFlow.onToolChatStarted,
  onToolChatChunk: chatFlow.onToolChatChunk,
  onToolChatFinished: chatFlow.onToolChatFinished,
  onToolReportFetchStarted: chatFlow.onToolReportFetchStarted,
  onToolReportFetchChunk: chatFlow.onToolReportFetchChunk,
  onToolReportFetchFinished: chatFlow.onToolReportFetchFinished,
});

/**
 * 写入统一日志（文本 + 结构化）。
 * @param {string} text 文本日志。
 * @param {Record<string, any>} options 结构化字段。
 */
function addLog(text, options = {}) {
  pushLog(state, text, options);
}

function formatWireLog(direction, hostName, rawText) {
  return formatWireLogRaw(direction, hostName, rawText, RAW_PAYLOAD_DEBUG);
}

/**
 * 记录前端异常，避免异常直接中断交互链路。
 * @param {string} scope 异常来源范围。
 * @param {unknown} error 异常对象。
 */
function reportUiError(scope, error) {
  const message = String(error && typeof error === "object" && "message" in error ? error.message : error || "未知异常");
  addLog(`[ui_error] ${scope}: ${message}`, {
    level: "error",
    scope: "ui",
    action: scope,
    outcome: "failed",
    detail: message,
  });
  console.error(`[ui_error] ${scope}`, error);
}

/**
 * 保护 UI 事件处理器，捕获同步/异步异常。
 * @param {string} scope 异常来源范围。
 * @param {(event?: Event)=>unknown} handler 原始处理器。
 * @returns {(event?: Event)=>void}
 */
function guardUiHandler(scope, handler) {
  return (event) => {
    try {
      const result = handler(event);
      if (result && typeof result.then === "function") {
        void result.catch((error) => reportUiError(scope, error));
      }
    } catch (error) {
      reportUiError(scope, error);
    }
  };
}

function switchTab(tab) {
  // 切页前先释放输入焦点，避免 iOS 底部输入附件条遮挡交互。
  if (document.activeElement instanceof HTMLElement) {
    document.activeElement.blur();
  }
  if (tab === "chat") {
    chatFlow.enterChatTab();
  }
  state.activeTab = tab;
  render();
}

function render() {
  try {
    hostState.recomputeSelections();
    chatFlow.renderSync();
    state.activeChatKey = String(state.chat.activeConversationKey || "");
    renderTabsView(state, ui);
    const hostCount = hostState.visibleHosts().length;
    renderTopActionsView(ui, hostCount, connectionFlow.hasConnectableHost(), connectionFlow.isAnyHostConnected());
    renderHostStageView(ui, hostState.visibleHosts().length > 0);
    renderBannerView(ui, hostState.visibleHosts(), state.bannerActiveIndex, runtimeState.hostStatusLabel, escapeHtml);
    toolsView.renderToolsByHost(hostState.visibleHosts());
    chatView.renderChat();
    renderDebugPanelView(state, ui, hostState.visibleHosts, hostState.hostById, runtimeState.ensureRuntime, maskSecret, escapeHtml);
    toolDetailModal.renderToolModal();
    addToolModal.renderAddToolModal();
    hostManageFlowRef.renderHostManageModal();
    hostManageFlowRef.renderHostMetricsModal();
    reportViewerModal.renderReportViewer();
  } catch (error) {
    reportUiError("render", error);
  }
}

function bindEvents() {
  ui.tabOps.addEventListener("click", guardUiHandler("tab_ops", () => switchTab("ops")));
  ui.tabChat.addEventListener("click", guardUiHandler("tab_chat", () => switchTab("chat")));
  ui.tabDebug.addEventListener("click", guardUiHandler("tab_debug", () => switchTab("debug")));
  ui.connectBtnTop.addEventListener("click", guardUiHandler("connect_all_hosts", () => connectionFlow.connectAllHosts()));
  ui.disconnectBtnTop.addEventListener("click", guardUiHandler("disconnect_all_hosts", () => connectionFlow.disconnectAllHosts()));
  ui.replaceHostBtnTop.addEventListener("click", guardUiHandler("open_host_manage", openHostManageModalGuard));

  ui.importPairLinkBtn.addEventListener("click", guardUiHandler("pair_flow_import", () => openPairFlowGuard("import", "")));
  ui.openManualPairBtn.addEventListener("click", guardUiHandler("pair_flow_manual", () => openPairFlowGuard("manual", "")));

  ui.connectBtnDebug.addEventListener("click", guardUiHandler(
    "connect_debug_host",
    () => connectionFlow.connectHost(state.debugHostId, { manual: true, resetRetry: true }),
  ));
  ui.disconnectBtnDebug.addEventListener("click", guardUiHandler(
    "disconnect_debug_host",
    () => connectionFlow.disconnectHost(state.debugHostId, { triggerReconnect: false }),
  ));
  ui.rebindControllerBtn.addEventListener("click", guardUiHandler(
    "rebind_controller",
    () => connectionFlow.requestControllerRebind(state.debugHostId),
  ));
  ui.debugHostSelect.addEventListener("change", guardUiHandler("change_debug_host", () => {
    state.debugHostId = String(ui.debugHostSelect.value || "");
    render();
  }));

  ui.messageInput.addEventListener("input", guardUiHandler("update_message", () => {
    state.message = ui.messageInput.value;
    hostState.persistConfig();
    render();
  }));
  ui.sendBtn.addEventListener("click", guardUiHandler("send_test_event", () => {
    connectionFlow.sendTestEvent(state.debugHostId, state.message);
  }));
  ui.copyOpLogsBtn.addEventListener("click", guardUiHandler("copy_operation_logs", async () => {
    const content = JSON.stringify(state.operationLogs, null, 2);
    if (navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
      await navigator.clipboard.writeText(content);
      addLog(`已复制结构化日志（${state.operationLogs.length} 条）`, {
        scope: "debug",
        action: "copy_operation_logs",
        outcome: "success",
      });
      return;
    }
    addLog("复制失败：当前环境不支持 clipboard 接口", {
      level: "warn",
      scope: "debug",
      action: "copy_operation_logs",
      outcome: "failed",
    });
  }));
  ui.clearLogsBtn.addEventListener("click", guardUiHandler("clear_logs", () => {
    state.logs = [];
    state.operationLogs = [];
    addLog("已清空调试日志", {
      scope: "debug",
      action: "clear_logs",
      outcome: "success",
    });
    render();
  }));

  ui.toolsGroupedList.addEventListener("click", guardUiHandler("tools_group_click", toolManageFlow.onToolsGroupedClick));
  ui.toolsGroupedList.addEventListener("scroll", toolsView.onToolSwipeScrollCapture, true);
  ui.toolsGroupedList.addEventListener("pointerup", toolsView.onToolSwipePointerUp, true);
  ui.toolsGroupedList.addEventListener("touchend", toolsView.onToolSwipePointerUp, true);
  ui.toolsGroupedList.addEventListener("mouseup", toolsView.onToolSwipePointerUp, true);
  ui.hostBannerTrack.addEventListener("scroll", guardUiHandler("host_banner_scroll", () => syncBannerActiveIndex(ui, state)));
  ui.hostBannerTrack.addEventListener("click", guardUiHandler("host_banner_click", (event) => {
    const hostId = extractBannerHostId(event);
    if (!hostId) return;
    openHostMetricsModalGuard(hostId);
  }));

  pairingFlow.bindPairFlowEvents({ onOpenDebugTab: guardUiHandler("pair_open_debug_tab", () => {
    closeModalStack("none");
    switchTab("debug");
  }) });
  pairFailureModal.bindPairFailureModalEvents({ onPrimaryAction: pairingFlow.bindFailureActionHandler() });
  noticeModal.bindHostNoticeModalEvents({ onEditHost: (hostId) => {
    closeModalStack("hostEdit");
    if (hostManageFlowRef) hostManageFlowRef.openHostEditModal(hostId);
  } });
  reportViewerModal.bindReportViewerModalEvents();
  addToolModal.bindAddToolModalEvents();
  toolDetailModal.bindToolDetailModalEvents();

  ui.hostManageClose.addEventListener("click", guardUiHandler("close_host_manage", hostManageFlowRef.closeHostManageModal));
  ui.hostManageModal.addEventListener("click", guardUiHandler("backdrop_host_manage", (event) => {
    if (event.target === ui.hostManageModal) hostManageFlowRef.closeHostManageModal();
  }));
  ui.hostManageAddBtn.addEventListener("click", guardUiHandler("host_manage_add", () => {
    hostManageFlowRef.closeHostManageModal();
    openPairFlowGuard("import", "");
  }));
  ui.hostManageDebugBtn.addEventListener("click", guardUiHandler("host_manage_open_debug", () => {
    hostManageFlowRef.closeHostManageModal();
    switchTab("debug");
  }));
  ui.hostManageList.addEventListener(
    "click",
    guardUiHandler(
      "host_manage_list_click",
      (event) => hostManageFlowRef.onHostManageListClick(event, (hostId) => {
        state.debugHostId = hostId;
        state.activeTab = "debug";
      }),
    ),
  );
  ui.pendingDeleteList.addEventListener("click", guardUiHandler("pending_delete_list_click", hostManageFlowRef.onPendingDeleteListClick));

  ui.hostEditClose.addEventListener("click", guardUiHandler("close_host_edit", hostManageFlowRef.closeHostEditModal));
  ui.hostEditCancelBtn.addEventListener("click", guardUiHandler("cancel_host_edit", hostManageFlowRef.closeHostEditModal));
  ui.hostEditSaveBtn.addEventListener("click", guardUiHandler("save_host_edit", hostManageFlowRef.saveHostEdit));
  ui.hostEditModal.addEventListener("click", guardUiHandler("backdrop_host_edit", (event) => {
    if (event.target === ui.hostEditModal) hostManageFlowRef.closeHostEditModal();
  }));

  ui.hostMetricsClose.addEventListener("click", guardUiHandler("close_host_metrics", hostManageFlowRef.closeHostMetricsModal));
  ui.hostMetricsModal.addEventListener("click", guardUiHandler("backdrop_host_metrics", (event) => {
    if (event.target === ui.hostMetricsModal) hostManageFlowRef.closeHostMetricsModal();
  }));

  document.addEventListener("pointerdown", toolsView.onGlobalPointerDown, true);
  document.addEventListener("keydown", guardUiHandler("global_keydown", (event) => {
    if (event.key !== "Escape") return;
    closeModalStack("none");
    toolsView.closeActiveToolSwipe();
  }));
  chatFlow.bindEvents(ui);
}

function init() {
  hostState.restoreConfig();
  hostState.ensureIdentity();
  window.addEventListener("error", (event) => {
    reportUiError("window_error", event.error || event.message || "unknown error");
  });
  window.addEventListener("unhandledrejection", (event) => {
    reportUiError("unhandled_rejection", event.reason || "unknown rejection");
  });
  pairingFlow.bindPairingLinkBridge();
  pairingFlow.tryApplyLaunchPairingLink();
  ui.messageInput.value = state.message;
  void chatFlow.hydrateChatState();

  bindEvents();

  setInterval(() => {
    void hostManageFlowRef.processPendingDeletes();
  }, DELETE_RETRY_INTERVAL_MS);

  if (hostState.visibleHosts().length > 0) {
    void connectionFlow.connectAllHosts();
  }

  render();
}

init();
