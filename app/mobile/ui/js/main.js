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
import { createHostNoticeModal } from "./modals/host-notice.js";
import { createPairFailureModal } from "./modals/pair-failure.js";
import { createToolDetailModal } from "./modals/tool-detail.js";
import { createAddToolModal } from "./modals/add-tool.js";

const hostState = createHostState();
const runtimeState = createRuntimeState({ persistConfig: hostState.persistConfig });

const noticeModal = createHostNoticeModal({ state, ui });
const pairFailureModal = createPairFailureModal({ state, ui });

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
  setToolHidden: runtimeState.setToolHidden,
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
  auth: authFlow,
});

const toolsView = createToolsView({
  state,
  ui,
  ensureRuntime: runtimeState.ensureRuntime,
  metricForTool: runtimeState.metricForTool,
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
  hostStatusLabel: runtimeState.hostStatusLabel,
});

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
  resolveToolDisplayName: runtimeState.resolveToolDisplayName,
});

const toolManageFlow = createToolManageFlow({
  state,
  hostById: hostState.hostById,
  ensureRuntime: runtimeState.ensureRuntime,
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
  openHostNoticeModal: noticeModal.openHostNoticeModal,
  openAddToolModal: addToolModal.openAddToolModal,
  renderAddToolModal: addToolModal.renderAddToolModal,
  closeAddToolModal: addToolModal.closeAddToolModal,
  openToolDetail: toolDetailModal.openToolDetail,
  openHostManageModal: () => hostManageFlowRef && hostManageFlowRef.openHostManageModal(),
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
  clearHostSession: authFlow.clearHostSession,
  addLog,
  openHostNoticeModal: noticeModal.openHostNoticeModal,
  connectHost: connectionFlow.connectHost,
  reconnectHost: connectionFlow.reconnectHost,
  disconnectHost: connectionFlow.disconnectHost,
  openPairFlow: pairingFlow.openPairFlow,
  render,
});

addToolModal.setHandlers({
  connectCandidateTool: toolManageFlow.connectCandidateTool,
  openDebug: () => switchTab("debug"),
});
connectionFlow.setHooks({
  openHostNoticeModal: noticeModal.openHostNoticeModal,
  closeAddToolModal: addToolModal.closeAddToolModal,
  renderAddToolModal: addToolModal.renderAddToolModal,
  connectCandidateTool: toolManageFlow.connectCandidateTool,
});

function addLog(text) {
  pushLog(state, text);
}

function formatWireLog(direction, hostName, rawText) {
  return formatWireLogRaw(direction, hostName, rawText, RAW_PAYLOAD_DEBUG);
}

function switchTab(tab) {
  state.activeTab = tab;
  render();
}

function render() {
  hostState.recomputeSelections();
  renderTabsView(state, ui);
  const hostCount = hostState.visibleHosts().length;
  renderTopActionsView(ui, hostCount, connectionFlow.hasConnectableHost(), connectionFlow.isAnyHostConnected());
  renderHostStageView(ui, hostState.visibleHosts().length > 0);
  renderBannerView(ui, hostState.visibleHosts(), state.bannerActiveIndex, runtimeState.hostStatusLabel, escapeHtml);
  toolsView.renderToolsByHost(hostState.visibleHosts());
  renderDebugPanelView(state, ui, hostState.visibleHosts, hostState.hostById, runtimeState.ensureRuntime, maskSecret, escapeHtml);
  toolDetailModal.renderToolModal();
  addToolModal.renderAddToolModal();
  hostManageFlowRef.renderHostManageModal();
  hostManageFlowRef.renderHostMetricsModal();
}

function bindEvents() {
  ui.tabOps.addEventListener("click", () => switchTab("ops"));
  ui.tabDebug.addEventListener("click", () => switchTab("debug"));
  ui.connectBtnTop.addEventListener("click", connectionFlow.connectAllHosts);
  ui.disconnectBtnTop.addEventListener("click", connectionFlow.disconnectAllHosts);
  ui.replaceHostBtnTop.addEventListener("click", hostManageFlowRef.openHostManageModal);

  ui.importPairLinkBtn.addEventListener("click", () => pairingFlow.openPairFlow("import", ""));
  ui.openManualPairBtn.addEventListener("click", () => pairingFlow.openPairFlow("manual", ""));

  ui.connectBtnDebug.addEventListener("click", () => connectionFlow.connectHost(state.debugHostId, { manual: true, resetRetry: true }));
  ui.disconnectBtnDebug.addEventListener("click", () => connectionFlow.disconnectHost(state.debugHostId, { triggerReconnect: false }));
  ui.rebindControllerBtn.addEventListener("click", () => connectionFlow.requestControllerRebind(state.debugHostId));
  ui.debugHostSelect.addEventListener("change", () => {
    state.debugHostId = String(ui.debugHostSelect.value || "");
    render();
  });

  ui.messageInput.addEventListener("input", () => {
    state.message = ui.messageInput.value;
    hostState.persistConfig();
    render();
  });
  ui.sendBtn.addEventListener("click", () => connectionFlow.sendTestEvent(state.debugHostId, state.message));

  ui.toolsGroupedList.addEventListener("click", toolManageFlow.onToolsGroupedClick);
  ui.toolsGroupedList.addEventListener("scroll", toolsView.onToolSwipeScrollCapture, true);
  ui.hostBannerTrack.addEventListener("scroll", () => syncBannerActiveIndex(ui, state));
  ui.hostBannerTrack.addEventListener("click", (event) => {
    const hostId = extractBannerHostId(event);
    if (!hostId) return;
    hostManageFlowRef.openHostMetricsModal(hostId);
  });

  pairingFlow.bindPairFlowEvents({ onOpenDebugTab: () => switchTab("debug") });
  pairFailureModal.bindPairFailureModalEvents({ onPrimaryAction: pairingFlow.bindFailureActionHandler() });
  noticeModal.bindHostNoticeModalEvents({ onEditHost: hostManageFlowRef.openHostEditModal });
  addToolModal.bindAddToolModalEvents();
  toolDetailModal.bindToolDetailModalEvents();

  ui.hostManageClose.addEventListener("click", hostManageFlowRef.closeHostManageModal);
  ui.hostManageModal.addEventListener("click", (event) => {
    if (event.target === ui.hostManageModal) hostManageFlowRef.closeHostManageModal();
  });
  ui.hostManageAddBtn.addEventListener("click", () => {
    hostManageFlowRef.closeHostManageModal();
    pairingFlow.openPairFlow("import", "");
  });
  ui.hostManageDebugBtn.addEventListener("click", () => {
    hostManageFlowRef.closeHostManageModal();
    switchTab("debug");
  });
  ui.hostManageList.addEventListener("click", (event) => hostManageFlowRef.onHostManageListClick(event, (hostId) => {
    state.debugHostId = hostId;
    state.activeTab = "debug";
  }));
  ui.pendingDeleteList.addEventListener("click", hostManageFlowRef.onPendingDeleteListClick);

  ui.hostEditClose.addEventListener("click", hostManageFlowRef.closeHostEditModal);
  ui.hostEditCancelBtn.addEventListener("click", hostManageFlowRef.closeHostEditModal);
  ui.hostEditSaveBtn.addEventListener("click", hostManageFlowRef.saveHostEdit);
  ui.hostEditModal.addEventListener("click", (event) => {
    if (event.target === ui.hostEditModal) hostManageFlowRef.closeHostEditModal();
  });

  ui.hostMetricsClose.addEventListener("click", hostManageFlowRef.closeHostMetricsModal);
  ui.hostMetricsModal.addEventListener("click", (event) => {
    if (event.target === ui.hostMetricsModal) hostManageFlowRef.closeHostMetricsModal();
  });

  document.addEventListener("pointerdown", toolsView.onGlobalPointerDown, true);
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    pairFailureModal.closePairFailureModal();
    pairingFlow.closePairFlow();
    toolDetailModal.closeToolDetail();
    addToolModal.closeAddToolModal();
    hostManageFlowRef.closeHostManageModal();
    hostManageFlowRef.closeHostEditModal();
    hostManageFlowRef.closeHostMetricsModal();
    noticeModal.closeHostNoticeModal();
    toolsView.closeActiveToolSwipe();
  });
}

function init() {
  hostState.restoreConfig();
  hostState.ensureIdentity();
  pairingFlow.bindPairingLinkBridge();
  pairingFlow.tryApplyLaunchPairingLink();
  ui.messageInput.value = state.message;

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
