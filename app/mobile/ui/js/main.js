// 文件职责（脚本部分）：
// 1. 维护多宿主机状态、连接生命周期与删除补偿队列。
// 2. 提供配对流程（扫码/图库/粘贴/手动）与统一失败提示。
// 3. 渲染 Banner 总览、按宿主机分组的工具列表、调试入口与工具详情。

import {
  STORAGE_KEY,
  LEGACY_STORAGE_KEY,
  DEFAULT_RELAY_WS_URL,
  RECONNECT_INTERVAL_MS,
  MAX_RECONNECT_ATTEMPTS,
  DELETE_RETRY_INTERVAL_MS,
  RAW_PAYLOAD_DEBUG,
  state,
  createRuntime,
  ui,
} from "./state/store.js";
import { tauriInvoke } from "./services/tauri.js";
import { parseRelayWsUrl, relayRequestJson } from "./services/relay-api.js";
import { parsePairingLink } from "./services/pairing-link.js";
import { buildAppWsUrl } from "./services/ws.js";
import { fmt2, fmtInt, fmtTokenM, maskSecret, usageSummary } from "./utils/format.js";
import { asMap, asListOfMap, asBool } from "./utils/type.js";
import { escapeHtml } from "./utils/dom.js";
import { addLog as pushLog, formatWireLog as formatWireLogRaw } from "./utils/log.js";
import { renderTabs as renderTabsView } from "./views/tabs.js";
import {
  deriveBannerActiveIndex,
  renderBanner as renderBannerView,
  renderBannerDots as renderBannerDotsView,
  renderHostStage as renderHostStageView,
} from "./views/banner.js";
import { renderDebugPanel as renderDebugPanelView } from "./views/debug.js";

// 删除补偿中的终态错误码：出现后不应无限重试，应清理本地并移出队列。
const DELETE_TERMINAL_RELAY_CODES = new Set([
  "SYSTEM_NOT_REGISTERED",
  "DEVICE_REVOKED",
  "DEVICE_NOT_FOUND",
  "REFRESH_TOKEN_INVALID",
  "REFRESH_TOKEN_EXPIRED",
]);

function addLog(text) {
  pushLog(state, text);
}

function formatWireLog(direction, hostName, rawText) {
  return formatWireLogRaw(direction, hostName, rawText, RAW_PAYLOAD_DEBUG);
}

function init() {
  restoreConfig();
  ensureIdentity();
  bindPairingLinkBridge();
  tryApplyLaunchPairingLink();

  ui.messageInput.value = state.message;

  ui.tabOps.addEventListener("click", () => switchTab("ops"));
  ui.tabDebug.addEventListener("click", () => switchTab("debug"));

  ui.connectBtnTop.addEventListener("click", connectAllHosts);
  ui.disconnectBtnTop.addEventListener("click", disconnectAllHosts);
  ui.replaceHostBtnTop.addEventListener("click", openHostManageModal);

  ui.importPairLinkBtn.addEventListener("click", () => openPairFlow("import", ""));
  ui.openManualPairBtn.addEventListener("click", () => openPairFlow("manual", ""));
  ui.openDebugFromSetupBtn.addEventListener("click", () => switchTab("debug"));

  ui.connectBtnDebug.addEventListener("click", () => connectHost(state.debugHostId, { manual: true, resetRetry: true }));
  ui.disconnectBtnDebug.addEventListener("click", () => disconnectHost(state.debugHostId, { triggerReconnect: false }));
  ui.rebindControllerBtn.addEventListener("click", () => requestControllerRebind(state.debugHostId));
  ui.debugHostSelect.addEventListener("change", () => {
    state.debugHostId = String(ui.debugHostSelect.value || "");
    render();
  });

  ui.messageInput.addEventListener("input", () => {
    state.message = ui.messageInput.value;
    persistConfig();
    render();
  });
  ui.sendBtn.addEventListener("click", sendTestEvent);

  ui.toolsGroupedList.addEventListener("click", onToolsGroupedClick);
  // 监听工具卡容器的横向滚动，识别左滑展开/右滑收起状态。
  ui.toolsGroupedList.addEventListener("scroll", onToolSwipeScrollCapture, true);
  ui.hostBannerTrack.addEventListener("scroll", onHostBannerScroll);
  ui.hostBannerTrack.addEventListener("click", onHostBannerClick);

  ui.toolModalClose.addEventListener("click", closeToolDetail);
  ui.toolModal.addEventListener("click", (event) => {
    if (event.target === ui.toolModal) {
      closeToolDetail();
    }
  });
  ui.toggleDetailsBtn.addEventListener("click", () => {
    state.detailExpanded = !state.detailExpanded;
    renderToolModal();
  });

  ui.addToolModalClose.addEventListener("click", closeAddToolModal);
  ui.addToolModal.addEventListener("click", (event) => {
    if (event.target === ui.addToolModal) {
      closeAddToolModal();
    }
  });
  ui.candidateList.addEventListener("click", onCandidateListClick);
  ui.goDebugFromAddTool.addEventListener("click", () => {
    closeAddToolModal();
    switchTab("debug");
  });

  ui.pairFlowClose.addEventListener("click", closePairFlow);
  ui.pairFlowModal.addEventListener("click", (event) => {
    if (event.target === ui.pairFlowModal) {
      closePairFlow();
    }
  });
  ui.pairOpenScanBtn.addEventListener("click", () => openPairFlow("scan"));
  ui.pairOpenPasteBtn.addEventListener("click", () => openPairFlow("paste"));
  ui.pairPasteBackBtn.addEventListener("click", () => openPairFlow("import"));
  ui.pairScanBackBtn.addEventListener("click", () => openPairFlow("import"));
  ui.pairManualBackBtn.addEventListener("click", closePairFlow);
  ui.pairPasteSubmitBtn.addEventListener("click", () => runPairingFromLink(ui.pairLinkInput.value, "paste"));
  ui.pairManualSubmitBtn.addEventListener("click", runPairingFromManual);
  ui.pairLinkInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      void runPairingFromLink(ui.pairLinkInput.value, "paste");
    }
  });
  ui.pairScanGalleryBtn.addEventListener("click", () => ui.pairScanFileInput.click());
  ui.pairScanFileInput.addEventListener("change", onPairScanFileSelected);

  ui.pairFailureClose.addEventListener("click", closePairFailureModal);
  ui.pairFailureSecondaryBtn.addEventListener("click", closePairFailureModal);
  ui.pairFailurePrimaryBtn.addEventListener("click", () => {
    const action = state.pairFailurePrimaryAction;
    closePairFailureModal();
    if (action === "scan") {
      openPairFlow("scan");
    } else if (action === "manual") {
      openPairFlow("manual");
    } else {
      openPairFlow("paste");
    }
  });

  ui.hostManageClose.addEventListener("click", closeHostManageModal);
  ui.hostManageModal.addEventListener("click", (event) => {
    if (event.target === ui.hostManageModal) {
      closeHostManageModal();
    }
  });
  ui.hostManageAddBtn.addEventListener("click", () => {
    closeHostManageModal();
    openPairFlow("import", "");
  });
  ui.hostManageDebugBtn.addEventListener("click", () => {
    closeHostManageModal();
    switchTab("debug");
  });
  ui.hostManageList.addEventListener("click", onHostManageListClick);
  ui.pendingDeleteList.addEventListener("click", onPendingDeleteListClick);

  ui.hostEditClose.addEventListener("click", closeHostEditModal);
  ui.hostEditCancelBtn.addEventListener("click", closeHostEditModal);
  ui.hostEditSaveBtn.addEventListener("click", saveHostEdit);
  ui.hostEditModal.addEventListener("click", (event) => {
    if (event.target === ui.hostEditModal) {
      closeHostEditModal();
    }
  });

  ui.hostMetricsClose.addEventListener("click", closeHostMetricsModal);
  ui.hostMetricsModal.addEventListener("click", (event) => {
    if (event.target === ui.hostMetricsModal) {
      closeHostMetricsModal();
    }
  });

  ui.hostNoticeClose.addEventListener("click", closeHostNoticeModal);
  ui.hostNoticeSecondaryBtn.addEventListener("click", closeHostNoticeModal);
  ui.hostNoticePrimaryBtn.addEventListener("click", () => {
    const action = state.hostNoticePrimaryAction;
    const hostId = state.hostNoticeTargetId;
    closeHostNoticeModal();
    if (action === "edit" && hostId) {
      openHostEditModal(hostId);
    }
  });
  ui.hostNoticeModal.addEventListener("click", (event) => {
    if (event.target === ui.hostNoticeModal) {
      closeHostNoticeModal();
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") {
      return;
    }
    closePairFailureModal();
    closePairFlow();
    closeToolDetail();
    closeAddToolModal();
    closeHostManageModal();
    closeHostEditModal();
    closeHostMetricsModal();
    closeHostNoticeModal();
    closeActiveToolSwipe();
  });
  // 点击工具区外部时收起左滑操作区，避免误触后长期遮挡。
  document.addEventListener("pointerdown", onGlobalPointerDown, true);

  // 删除补偿与心跳重试都依赖同一个周期轮询。
  setInterval(() => {
    void processPendingDeletes();
  }, DELETE_RETRY_INTERVAL_MS);

  if (visibleHosts().length > 0) {
    connectAllHosts();
  }

  render();
}

function switchTab(tab) {
  state.activeTab = tab;
  render();
}

function visibleHosts() {
  return [...state.hosts]
    .sort((a, b) => new Date(a.pairedAt).getTime() - new Date(b.pairedAt).getTime());
}

function hostById(hostId) {
  const id = String(hostId || "");
  if (!id) {
    return null;
  }
  return state.hosts.find((host) => host.hostId === id) || null;
}

function ensureRuntime(hostId) {
  const id = String(hostId || "");
  if (!id) {
    return null;
  }
  if (!state.runtimes[id]) {
    state.runtimes[id] = createRuntime();
  }
  return state.runtimes[id];
}

function disposeRuntime(hostId) {
  const runtime = state.runtimes[hostId];
  if (!runtime) {
    return;
  }
  runtime.connectionEpoch += 1;
  if (runtime.reconnectTimer) {
    clearTimeout(runtime.reconnectTimer);
    runtime.reconnectTimer = null;
  }
  if (runtime.socket) {
    runtime.socket.close();
    runtime.socket = null;
  }
  delete state.runtimes[hostId];
}

function recomputeSelections() {
  const hosts = visibleHosts();
  if (hosts.length === 0) {
    state.selectedHostId = "";
    state.debugHostId = "";
    state.bannerActiveIndex = 0;
    return;
  }

  const hasSelected = hosts.some((host) => host.hostId === state.selectedHostId);
  if (!hasSelected) {
    state.selectedHostId = hosts[0].hostId;
  }

  const hasDebug = hosts.some((host) => host.hostId === state.debugHostId);
  if (!hasDebug) {
    state.debugHostId = state.selectedHostId;
  }

  const maxIndex = Math.max(0, hosts.length - 1);
  state.bannerActiveIndex = Math.min(Math.max(0, state.bannerActiveIndex), maxIndex);
}

function persistConfig() {
  const payload = {
    schemaVersion: 2,
    deviceId: state.deviceId,
    selectedHostId: state.selectedHostId,
    hosts: state.hosts.map((host) => ({
      hostId: host.hostId,
      systemId: host.systemId,
      relayUrl: host.relayUrl,
      displayName: host.displayName,
      note: host.note || "",
      pairedAt: host.pairedAt,
      updatedAt: host.updatedAt,
      autoConnect: host.autoConnect !== false,
    })),
    pendingHostDeletes: state.pendingHostDeletes,
    toolAliases: state.toolAliases,
    toolVisibility: state.toolVisibility,
    message: state.message,
  };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
}

function restoreConfig() {
  let parsed = null;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      parsed = JSON.parse(raw);
    }
  } catch (_) {
    parsed = null;
  }

  if (!parsed) {
    parsed = migrateLegacyConfig();
  }
  if (!parsed) {
    return;
  }

  state.deviceId = String(parsed.deviceId || "").trim();
  state.selectedHostId = String(parsed.selectedHostId || "").trim();
  state.message = String(parsed.message || state.message);

  const hosts = Array.isArray(parsed.hosts) ? parsed.hosts : [];
  state.hosts = hosts
    .map((item) => normalizeHostProfile(item))
    .filter((item) => item && item.hostId && item.systemId && item.relayUrl);

  const pending = Array.isArray(parsed.pendingHostDeletes) ? parsed.pendingHostDeletes : [];
  state.pendingHostDeletes = pending
    .map((item) => normalizePendingDelete(item))
    .filter((item) => item && item.hostId && item.systemId && item.relayUrl);
  state.toolAliases = normalizeToolMetaMap(parsed.toolAliases);
  state.toolVisibility = normalizeToolMetaMap(parsed.toolVisibility);

  recomputeSelections();
}

function migrateLegacyConfig() {
  try {
    const raw = localStorage.getItem(LEGACY_STORAGE_KEY);
    if (!raw) {
      return null;
    }
    const legacy = JSON.parse(raw);
    const relayUrl = String(legacy.relayUrl || "").trim();
    const systemId = String(legacy.systemId || "").trim();
    if (!relayUrl || !systemId) {
      return null;
    }

    const now = new Date().toISOString();
    const hostId = `host_${createEventId().slice(4)}`;
    return {
      schemaVersion: 2,
      deviceId: String(legacy.deviceId || "").trim(),
      selectedHostId: hostId,
      hosts: [
        {
          hostId,
          systemId,
          relayUrl,
          displayName: String(legacy.hostName || "").trim() || systemId,
          note: "",
          pairedAt: now,
          updatedAt: now,
          autoConnect: true,
        },
      ],
      pendingHostDeletes: [],
      toolAliases: {},
      toolVisibility: {},
      message: String(legacy.message || "tool_ping"),
    };
  } catch (_) {
    return null;
  }
}

function normalizeHostProfile(item) {
  const now = new Date().toISOString();
  const relayUrl = String(item.relayUrl || "").trim();
  const systemId = String(item.systemId || "").trim();
  if (!relayUrl || !systemId) {
    return null;
  }
  return {
    hostId: String(item.hostId || `host_${createEventId().slice(4)}`),
    systemId,
    relayUrl,
    displayName: String(item.displayName || item.hostName || systemId).trim() || systemId,
    note: String(item.note || "").trim(),
    pairedAt: String(item.pairedAt || now),
    updatedAt: String(item.updatedAt || now),
    autoConnect: item.autoConnect !== false,
  };
}

function normalizePendingDelete(item) {
  const now = Date.now();
  const relayUrl = String(item.relayUrl || "").trim();
  const systemId = String(item.systemId || "").trim();
  const hostId = String(item.hostId || "").trim();
  if (!relayUrl || !systemId || !hostId) {
    return null;
  }
  return {
    hostId,
    systemId,
    relayUrl,
    displayName: String(item.displayName || systemId),
    deviceId: String(item.deviceId || state.deviceId || "").trim(),
    enqueuedAt: Number(item.enqueuedAt || now),
    retryCount: Number(item.retryCount || 0),
    nextRetryAt: Number(item.nextRetryAt || now),
    lastError: String(item.lastError || ""),
    expectedCredentialId: String(item.expectedCredentialId || "").trim(),
    expectedKeyId: String(item.expectedKeyId || "").trim(),
  };
}

function normalizeToolMetaMap(rawValue) {
  if (!rawValue || typeof rawValue !== "object") {
    return {};
  }
  const out = {};
  Object.entries(rawValue).forEach(([key, value]) => {
    const normalizedKey = String(key || "").trim();
    if (!normalizedKey) {
      return;
    }
    out[normalizedKey] = String(value || "").trim();
  });
  return out;
}

function ensureIdentity() {
  if (!state.deviceId) {
    state.deviceId = createDeviceId();
    persistConfig();
  }
}

function createDeviceId() {
  if (window.crypto && typeof window.crypto.randomUUID === "function") {
    return `ios_${window.crypto.randomUUID()}`;
  }
  const rand = Math.random().toString(36).slice(2, 10);
  return `ios_${Date.now()}_${rand}`;
}

function createEventId() {
  if (window.crypto && typeof window.crypto.randomUUID === "function") {
    return `evt_${window.crypto.randomUUID()}`;
  }
  const rand = Math.random().toString(36).slice(2, 10);
  return `evt_${Date.now()}_${rand}`;
}

function toolStateKey(hostId, toolId) {
  const host = String(hostId || "").trim();
  const tool = String(toolId || "").trim();
  if (!host || !tool) {
    return "";
  }
  return `${host}::${tool}`;
}

function getToolAlias(hostId, toolId) {
  const key = toolStateKey(hostId, toolId);
  if (!key) {
    return "";
  }
  return String(state.toolAliases[key] || "").trim();
}

function setToolAlias(hostId, toolId, aliasValue) {
  const key = toolStateKey(hostId, toolId);
  if (!key) {
    return;
  }
  const normalized = String(aliasValue || "").trim();
  if (normalized) {
    state.toolAliases[key] = normalized;
  } else {
    delete state.toolAliases[key];
  }
  persistConfig();
}

function isToolHidden(hostId, toolId) {
  const key = toolStateKey(hostId, toolId);
  if (!key) {
    return false;
  }
  return String(state.toolVisibility[key] || "") === "hidden";
}

function setToolHidden(hostId, toolId, hidden) {
  const key = toolStateKey(hostId, toolId);
  if (!key) {
    return;
  }
  if (hidden) {
    state.toolVisibility[key] = "hidden";
  } else {
    delete state.toolVisibility[key];
  }
  persistConfig();
}

function clearToolMetaForHost(hostId) {
  const prefix = `${String(hostId || "").trim()}::`;
  if (!prefix || prefix === "::") {
    return;
  }
  Object.keys(state.toolAliases).forEach((key) => {
    if (key.startsWith(prefix)) {
      delete state.toolAliases[key];
    }
  });
  Object.keys(state.toolVisibility).forEach((key) => {
    if (key.startsWith(prefix)) {
      delete state.toolVisibility[key];
    }
  });
  persistConfig();
}

function resolveToolDisplayName(hostId, tool) {
  const toolId = String(tool && tool.toolId ? tool.toolId : "");
  const alias = getToolAlias(hostId, toolId);
  if (alias) {
    return alias;
  }
  const rawName = String((tool && tool.name) || "").trim();
  if (rawName) {
    return rawName;
  }
  return "Unknown Tool";
}

async function loadHostSession(hostId) {
  const host = hostById(hostId);
  const runtime = ensureRuntime(hostId);
  if (!host || !runtime) {
    return null;
  }
  try {
    const session = await tauriInvoke("auth_load_session", {
      systemId: host.systemId,
      deviceId: state.deviceId,
    });
    if (!session) {
      return null;
    }
    runtime.accessToken = String(session.accessToken || "");
    runtime.refreshToken = String(session.refreshToken || "");
    runtime.keyId = String(session.keyId || "");
    runtime.credentialId = String(session.credentialId || "");
    return session;
  } catch (error) {
    addLog(`load secure session failed (${host.displayName}): ${error}`);
    return null;
  }
}

async function storeHostSession(hostId) {
  const host = hostById(hostId);
  const runtime = ensureRuntime(hostId);
  if (!host || !runtime) {
    return;
  }
  await tauriInvoke("auth_store_session", {
    session: {
      systemId: host.systemId,
      deviceId: state.deviceId,
      accessToken: runtime.accessToken,
      refreshToken: runtime.refreshToken,
      keyId: runtime.keyId,
      credentialId: runtime.credentialId,
    },
  });
}

async function clearHostSession(systemId, deviceId = state.deviceId) {
  const normalizedDeviceId = String(deviceId || "").trim() || state.deviceId;
  await tauriInvoke("auth_clear_session", {
    systemId,
    deviceId: normalizedDeviceId,
  });
}

function openPairFlow(step = "import", targetHostId) {
  state.pairFlowStep = step;
  if (typeof targetHostId === "string") {
    state.pairTargetHostId = String(targetHostId || "");
  }

  const targetHost = hostById(state.pairTargetHostId);
  ui.hostRelayInput.value = targetHost ? targetHost.relayUrl : DEFAULT_RELAY_WS_URL;
  ui.hostNameInput.value = targetHost ? targetHost.displayName : "";
  ui.hostSystemIdInput.value = targetHost ? targetHost.systemId : "";
  ui.hostPairTicketInput.value = "";

  ui.pairFlowModal.classList.add("show");
  renderPairFlow();
}

function closePairFlow() {
  ui.pairFlowModal.classList.remove("show");
  stopPairScan();
  state.pairTargetHostId = "";
}

function renderPairFlow() {
  const step = state.pairFlowStep;
  ui.pairFlowStepImport.style.display = step === "import" ? "block" : "none";
  ui.pairFlowStepPaste.style.display = step === "paste" ? "block" : "none";
  ui.pairFlowStepScan.style.display = step === "scan" ? "block" : "none";
  ui.pairFlowStepManual.style.display = step === "manual" ? "block" : "none";

  const isRePair = Boolean(state.pairTargetHostId);
  if (isRePair) {
    ui.pairFlowTitle.textContent = step === "manual" ? "重新配对（手动）" : "重新配对";
  } else {
    ui.pairFlowTitle.textContent = step === "manual" ? "手动填写配对信息" : "导入配对链接";
  }

  if (step === "scan") {
    void startPairScan();
  } else {
    stopPairScan();
  }
}

function setPairScanStatus(text = "", level = "normal") {
  ui.pairScanStatus.textContent = String(text || "");
  ui.pairScanStatus.style.color = level === "warn" ? "var(--warn)" : "var(--text-sub)";
}

function closePairFailureModal() {
  ui.pairFailureModal.classList.remove("show");
}

function showPairFailure(code, message, suggestion, primaryAction = "paste") {
  const mapped = mapPairFailure(code, message, suggestion, primaryAction);
  state.pairFailurePrimaryAction = mapped.primaryAction;
  ui.pairFailureReason.textContent = mapped.reason;
  ui.pairFailureSuggestion.textContent = mapped.suggestion;
  ui.pairFailurePrimaryBtn.textContent = mapped.primaryLabel;
  ui.pairFailureModal.classList.add("show");
}

function mapPairFailure(code, message, suggestion, primaryAction) {
  const normalizedCode = String(code || "").trim();
  const fallbackMessage = String(message || "").trim();
  if (normalizedCode === "INVALID_LINK") {
    return {
      reason: "配对链接无效",
      suggestion: "请重新扫码或检查粘贴内容是否完整。",
      primaryLabel: "重新粘贴",
      primaryAction: "paste",
    };
  }
  if (normalizedCode === "PAIR_TICKET_EXPIRED" || normalizedCode === "PAIR_TICKET_REPLAYED") {
    return {
      reason: normalizedCode === "PAIR_TICKET_EXPIRED" ? "配对信息已过期" : "配对二维码已使用",
      suggestion: "请重新扫码获取最新二维码。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  if (normalizedCode === "PAIR_TICKET_INVALID") {
    return {
      reason: "配对信息无效",
      suggestion: "请重新扫码获取最新二维码，或改用手动配对。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  if (normalizedCode === "PAIR_TOKEN_NOT_SUPPORTED") {
    return {
      reason: "配对信息已过时",
      suggestion: "当前版本仅支持 sid + ticket 配对，请重新扫码获取最新链接。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  if (normalizedCode === "SYSTEM_NOT_REGISTERED") {
    return {
      reason: "宿主机未在线",
      suggestion: "请先在宿主机启动 sidecar，再进行配对。",
      primaryLabel: "去手动输入",
      primaryAction: "manual",
    };
  }
  if (normalizedCode === "RELAY_URL_INVALID") {
    return {
      reason: "Relay 地址格式无效",
      suggestion: "请使用 ws:// 或 wss:// 开头的 Relay 地址。",
      primaryLabel: "去手动输入",
      primaryAction: "manual",
    };
  }
  if (normalizedCode === "RELAY_UNREACHABLE") {
    const action = primaryAction === "manual" ? "manual" : primaryAction === "scan" ? "scan" : "paste";
    return {
      reason: "无法连接 Relay",
      suggestion:
        suggestion ||
        "请检查 Relay 地址、宿主机网络，并确认 relay 已启动（make run-relay）。本机调试可尝试 127.0.0.1 与 localhost 两种地址。",
      primaryLabel: action === "manual" ? "去手动输入" : action === "scan" ? "重新扫码" : "重新粘贴",
      primaryAction: action,
    };
  }
  if (normalizedCode === "QR_SCANNER_UNAVAILABLE") {
    return {
      reason: "当前设备不支持实时扫码",
      suggestion: "请改用“从图库导入二维码”或“粘贴配对链接”。",
      primaryLabel: "重新粘贴",
      primaryAction: "paste",
    };
  }
  if (normalizedCode === "CAMERA_UNAVAILABLE") {
    return {
      reason: "无法打开相机",
      suggestion: "请检查相机权限，或改用“从图库导入二维码/粘贴配对链接”。",
      primaryLabel: "重新粘贴",
      primaryAction: "paste",
    };
  }
  if (normalizedCode === "PAIR_TOKEN_MISMATCH") {
    return {
      reason: "配对信息无效",
      suggestion: "请重新生成配对信息后再试。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  if (normalizedCode === "ACCESS_TOKEN_EXPIRED" || normalizedCode === "ACCESS_TOKEN_INVALID") {
    return {
      reason: "设备凭证失效",
      suggestion: "请重新扫码配对，更新设备凭证。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  return {
    reason: fallbackMessage || "配对失败",
    suggestion: suggestion || "请重试；若仍失败可切换到手动填写配对信息。",
    primaryLabel: primaryAction === "manual" ? "去手动输入" : primaryAction === "scan" ? "重新扫码" : "重试",
    primaryAction: primaryAction || "paste",
  };
}

async function runPairingFromLink(rawValue, source = "paste") {
  const parsed = parsePairingLink(rawValue);
  if (!parsed) {
    showPairFailure("INVALID_LINK", "配对链接格式无效", "请检查链接是否完整。", "paste");
    return;
  }
  await runPairing(parsed, source);
}

async function runPairingFromManual() {
  const relayUrl = String(ui.hostRelayInput.value || "").trim();
  const systemId = String(ui.hostSystemIdInput.value || "").trim();
  const pairTicket = String(ui.hostPairTicketInput.value || "").trim();
  const hostName = String(ui.hostNameInput.value || "").trim();
  if (!relayUrl || !systemId || !pairTicket) {
    showPairFailure("PAIR_TICKET_INVALID", "手动配对信息不完整", "请确认 Relay 地址、System ID 与配对票据。", "manual");
    return;
  }
  try {
    parseRelayWsUrl(relayUrl);
  } catch (_) {
    showPairFailure("RELAY_URL_INVALID", "Relay 地址格式无效", "请填写 ws:// 或 wss:// 开头的地址。", "manual");
    return;
  }

  await runPairing(
    {
      relayUrl,
      pairCode: "",
      systemId,
      pairToken: "",
      pairTicket,
      hostName,
    },
    "manual",
  );
}

async function runPairing(parsed, source) {
  if (state.pairingBusy) {
    return;
  }

  state.pairingBusy = true;
  try {
    const relayUrl = String(parsed.relayUrl || "").trim();
    const systemId = String(parsed.systemId || "").trim();
    const pairToken = String(parsed.pairToken || "").trim();
    const pairTicket = String(parsed.pairTicket || "").trim();
    if (pairToken && !pairTicket) {
      showPairFailure("PAIR_TOKEN_NOT_SUPPORTED", "当前版本不支持 pairToken 配对", "请重新生成包含 sid + ticket 的配对链接。", source);
      return;
    }
    if (!relayUrl || !systemId || !pairTicket) {
      showPairFailure("PAIR_TICKET_INVALID", "配对信息不完整", "请重新导入配对信息后重试。", source);
      return;
    }

    const preflightReq = {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        systemId,
        deviceId: state.deviceId,
        pairTicket: pairTicket || undefined,
      }),
    };
    const { resp: preflightResp, body: preflightBody } = await relayRequestJson(relayUrl, "/pair/preflight", preflightReq);
    if (!preflightResp.ok || !preflightBody.ok) {
      const failureAction = source === "manual" ? "manual" : source === "scan" || source === "gallery" ? "scan" : "paste";
      showPairFailure(preflightBody.code, preflightBody.message, preflightBody.suggestion, failureAction);
      return;
    }

    const binding = await tauriInvoke("auth_get_device_binding", {
      deviceId: state.deviceId,
    });
    const keyId = String(binding.keyId || "");
    const devicePubKey = String(binding.publicKey || "");

    const proofPayload = `pair-exchange\n${systemId}\n${state.deviceId}\n${keyId}`;
    const proofSigned = await tauriInvoke("auth_sign_payload", {
      deviceId: state.deviceId,
      payload: proofPayload,
    });

    const exchangeReq = {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        systemId,
        deviceId: state.deviceId,
        deviceName: normalizedDeviceName(),
        pairTicket: pairTicket || undefined,
        keyId,
        devicePubKey,
        proof: String(proofSigned.signature || ""),
      }),
    };
    const { resp: exchangeResp, body: exchangeBody } = await relayRequestJson(relayUrl, "/pair/exchange", exchangeReq);
    if (!exchangeResp.ok || !exchangeBody.ok) {
      const failureAction = source === "manual" ? "manual" : source === "scan" || source === "gallery" ? "scan" : "paste";
      showPairFailure(exchangeBody.code, exchangeBody.message, exchangeBody.suggestion, failureAction);
      return;
    }

    const exchangeData = asMap(exchangeBody.data);
    const targetHost = hostById(state.pairTargetHostId);
    const existingBySystem = state.hosts.find((host) => host.systemId === systemId && host.relayUrl === relayUrl);
    const host = targetHost || existingBySystem;
    const nowIso = new Date().toISOString();

    let hostId = "";
    if (host) {
      hostId = host.hostId;
      host.systemId = systemId;
      host.relayUrl = relayUrl;
      host.displayName = String(parsed.hostName || host.displayName || systemId).trim() || systemId;
      host.updatedAt = nowIso;
    } else {
      hostId = `host_${createEventId().slice(4)}`;
      state.hosts.push({
        hostId,
        systemId,
        relayUrl,
        displayName: String(parsed.hostName || systemId).trim() || systemId,
        note: "",
        pairedAt: nowIso,
        updatedAt: nowIso,
        autoConnect: true,
      });
    }

    recomputeSelections();
    state.selectedHostId = hostId;
    state.debugHostId = hostId;

    const runtime = ensureRuntime(hostId);
    runtime.accessToken = String(exchangeData.accessToken || "");
    runtime.refreshToken = String(exchangeData.refreshToken || "");
    runtime.keyId = String(exchangeData.keyId || keyId);
    runtime.credentialId = String(exchangeData.credentialId || "");
    runtime.devicePublicKey = devicePubKey;
    runtime.manualReconnectRequired = false;
    runtime.retryCount = 0;
    runtime.lastError = "";

    await storeHostSession(hostId);
    // 同一宿主机重新配对后，移除历史删除补偿任务，避免旧任务误伤新会话。
    // 这里按 systemId + relayUrl 收口，不再依赖 deviceId 绝对匹配。
    state.pendingHostDeletes = state.pendingHostDeletes.filter(
      (item) => !(item.systemId === systemId && item.relayUrl === relayUrl),
    );
    persistConfig();

    closePairFlow();
    closeHostManageModal();

    await connectHost(hostId, { manual: true, resetRetry: true });
    notifyIfDuplicateDisplayName(hostId);
    render();
  } catch (error) {
    const code = String(error && error.code ? error.code : "").trim();
    const failureCode = code || "RELAY_UNREACHABLE";
    showPairFailure(failureCode, `配对请求失败：${error}`, "请检查网络与 Relay 地址。", source);
  } finally {
    state.pairingBusy = false;
  }
}

function normalizedDeviceName() {
  return "ios_mobile";
}

function bindPairingLinkBridge() {
  window.__YC_HANDLE_PAIR_LINK__ = (rawUrl) => {
    openPairFlow("import", "");
    void runPairingFromLink(rawUrl, "deep-link");
  };
}

function tryApplyLaunchPairingLink() {
  try {
    const launchUrl = new URL(window.location.href);
    if (launchUrl.protocol === "yc:" && launchUrl.hostname === "pair") {
      openPairFlow("import", "");
      void runPairingFromLink(launchUrl.toString(), "launch-url");
      return;
    }

    const relay = String(launchUrl.searchParams.get("relay") || "").trim();
    const code = String(launchUrl.searchParams.get("code") || "").trim();
    const sid = String(launchUrl.searchParams.get("sid") || "").trim();
    const ticket = String(launchUrl.searchParams.get("ticket") || "").trim();
    const name = String(launchUrl.searchParams.get("name") || "").trim();
    if (!relay || (!code && !(sid && ticket))) {
      return;
    }

    let syntheticLink = `yc://pair?relay=${encodeURIComponent(relay)}`;
    if (sid && ticket) {
      syntheticLink += `&sid=${encodeURIComponent(sid)}&ticket=${encodeURIComponent(ticket)}`;
    } else {
      syntheticLink += `&code=${encodeURIComponent(code)}`;
    }
    if (name) {
      syntheticLink += `&name=${encodeURIComponent(name)}`;
    }
    openPairFlow("import", "");
    void runPairingFromLink(syntheticLink, "launch-url");
  } catch (_) {
    // ignore malformed launch url
  }
}

async function startPairScan() {
  if (state.scanning) {
    return;
  }
  if (typeof window.BarcodeDetector !== "function") {
    setPairScanStatus("当前环境不支持实时扫码，可改用“从图库导入二维码”或“粘贴配对链接”。", "warn");
    return;
  }

  try {
    state.scanning = true;
    setPairScanStatus("请将二维码放入取景框，识别后会自动配对。");
    state.scanDetector = state.scanDetector || new window.BarcodeDetector({ formats: ["qr_code"] });
    state.scanStream = await navigator.mediaDevices.getUserMedia({
      video: { facingMode: "environment" },
      audio: false,
    });
    ui.pairScanVideo.srcObject = state.scanStream;
    await ui.pairScanVideo.play();
    void scanLoop();
  } catch (error) {
    stopPairScan();
    setPairScanStatus("无法打开相机，请检查权限后重试。", "warn");
  }
}

function stopPairScan() {
  state.scanning = false;
  if (state.scanStream) {
    const tracks = state.scanStream.getTracks();
    for (const track of tracks) {
      track.stop();
    }
    state.scanStream = null;
  }
  if (ui.pairScanVideo) {
    ui.pairScanVideo.srcObject = null;
  }
}

async function scanLoop() {
  while (state.scanning) {
    try {
      if (!state.scanDetector || !ui.pairScanVideo || ui.pairScanVideo.readyState < 2) {
        await sleep(120);
        continue;
      }
      const found = await state.scanDetector.detect(ui.pairScanVideo);
      if (Array.isArray(found) && found.length > 0) {
        const raw = String(found[0].rawValue || "").trim();
        if (raw) {
          await runPairingFromLink(raw, "scan");
          stopPairScan();
          break;
        }
      }
    } catch (_) {
      // 扫码帧允许偶发失败，不中断扫描。
    }
    await sleep(120);
  }
}

async function onPairScanFileSelected(event) {
  const file = event.target && event.target.files && event.target.files[0];
  event.target.value = "";
  if (!file) {
    return;
  }
  if (typeof window.BarcodeDetector !== "function") {
    showPairFailure("QR_SCANNER_UNAVAILABLE", "当前环境不支持二维码识别", "请改用粘贴链接方式。", "paste");
    return;
  }
  try {
    const bitmap = await createImageBitmap(file);
    const detector = state.scanDetector || new window.BarcodeDetector({ formats: ["qr_code"] });
    const detected = await detector.detect(bitmap);
    const first = Array.isArray(detected) && detected.length > 0 ? detected[0] : null;
    const rawValue = first && typeof first.rawValue === "string" ? first.rawValue : "";
    if (!rawValue) {
      showPairFailure("INVALID_LINK", "未识别到有效二维码", "请更换清晰图片后重试。", "scan");
      return;
    }
    await runPairingFromLink(rawValue, "gallery");
  } catch (error) {
    showPairFailure("INVALID_LINK", `图片识别失败：${error}`, "请改用扫码或粘贴链接。", "paste");
  }
}

async function connectAllHosts() {
  const hosts = visibleHosts();
  await Promise.allSettled(
    hosts.map((host) => connectHost(host.hostId, { manual: true, resetRetry: true })),
  );
}

async function disconnectAllHosts() {
  const hosts = visibleHosts();
  await Promise.allSettled(
    hosts.map((host) => disconnectHost(host.hostId, { triggerReconnect: false })),
  );
}

async function reconnectHost(hostId) {
  await disconnectHost(hostId, { triggerReconnect: false });
  await connectHost(hostId, { manual: true, resetRetry: true });
}

async function connectHost(hostId, options = {}) {
  const host = hostById(hostId);
  const runtime = ensureRuntime(hostId);
  if (!host || !runtime) {
    return;
  }

  const manual = asBool(options.manual);
  const resetRetry = asBool(options.resetRetry);
  if (resetRetry) {
    runtime.retryCount = 0;
    runtime.manualReconnectRequired = false;
    runtime.lastError = "";
    clearReconnectTimer(runtime);
  }

  if (runtime.connecting || runtime.connected) {
    return;
  }

  runtime.connecting = true;
  runtime.status = "CONNECTING";
  render();

  try {
    await loadHostSession(hostId);
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

    await refreshAccessTokenIfPossible(hostId);

    const connectionEpoch = runtime.connectionEpoch + 1;
    runtime.connectionEpoch = connectionEpoch;

    const ts = String(Math.floor(Date.now() / 1000));
    const nonce = createEventId();
    const payload = `ws\n${host.systemId}\n${state.deviceId}\n${runtime.keyId}\n${ts}\n${nonce}`;
    const signed = await tauriInvoke("auth_sign_payload", {
      deviceId: state.deviceId,
      payload,
    });
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
      const current = ensureRuntime(hostId);
      if (!current || current.connectionEpoch !== connectionEpoch || current.socket !== socket) {
        return;
      }
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
    });

    socket.addEventListener("message", (event) => {
      const current = ensureRuntime(hostId);
      if (!current || current.connectionEpoch !== connectionEpoch || current.socket !== socket) {
        return;
      }
      const text = String(event.data || "");
      state.eventIn += 1;
      addLog(formatWireLog("IN", host.displayName, text));
      ingestEvent(hostId, text);
      render();
    });

    socket.addEventListener("close", () => {
      const current = ensureRuntime(hostId);
      if (!current || current.connectionEpoch !== connectionEpoch || current.socket !== socket) {
        return;
      }
      current.connected = false;
      current.connecting = false;
      current.socket = null;
      current.status = "DISCONNECTED";
      addLog(`socket closed: ${host.displayName}`);
      scheduleReconnect(hostId, `socket closed (${host.displayName})`, manual);
      render();
    });

    socket.addEventListener("error", (error) => {
      const current = ensureRuntime(hostId);
      if (!current || current.connectionEpoch !== connectionEpoch || current.socket !== socket) {
        return;
      }
      current.connected = false;
      current.connecting = false;
      current.status = "RELAY_UNREACHABLE";
      current.lastError = String(error && error.message ? error.message : "socket error");
      addLog(`socket error (${host.displayName}): ${current.lastError}`);
      render();
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

async function disconnectHost(hostId, options = {}) {
  const runtime = ensureRuntime(hostId);
  if (!runtime) {
    return;
  }

  const triggerReconnect = options.triggerReconnect !== false;
  runtime.connectionEpoch += 1;
  clearReconnectTimer(runtime);

  const socket = runtime.socket;
  runtime.socket = null;
  if (socket) {
    socket.close();
  }
  runtime.connected = false;
  runtime.connecting = false;
  runtime.status = "DISCONNECTED";
  runtime.sidecarStatus = "UNKNOWN";
  runtime.lastHeartbeatAt = null;
  runtime.candidateTools = [];
  runtime.connectingToolIds = {};
  runtime.toolConnectRetryCount = {};
  Object.keys(runtime.toolConnectTimers || {}).forEach((toolId) => clearToolConnectTimer(runtime, toolId));

  const host = hostById(hostId);
  addLog(`disconnected host: ${host ? host.displayName : hostId}`);

  if (triggerReconnect) {
    scheduleReconnect(hostId, "manual disconnect", true);
  }
  render();
}

function clearReconnectTimer(runtime) {
  if (runtime && runtime.reconnectTimer) {
    clearTimeout(runtime.reconnectTimer);
    runtime.reconnectTimer = null;
  }
}

function clearToolConnectTimer(runtime, toolId) {
  if (!runtime || !toolId) {
    return;
  }
  const timer = runtime.toolConnectTimers && runtime.toolConnectTimers[toolId];
  if (!timer) {
    return;
  }
  clearTimeout(timer);
  delete runtime.toolConnectTimers[toolId];
}

function scheduleReconnect(hostId, reason, manualTriggered) {
  const host = hostById(hostId);
  const runtime = ensureRuntime(hostId);
  if (!host || !runtime) {
    return;
  }
  if (runtime.manualReconnectRequired) {
    return;
  }
  if (runtime.reconnectTimer) {
    return;
  }
  if (host.autoConnect === false) {
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

async function refreshAccessTokenIfPossible(hostId) {
  const host = hostById(hostId);
  const runtime = ensureRuntime(hostId);
  if (!host || !runtime) {
    return false;
  }
  if (!runtime.refreshToken || !runtime.keyId || !host.systemId || !state.deviceId) {
    return false;
  }

  try {
    const ts = String(Math.floor(Date.now() / 1000));
    const nonce = createEventId();
    const payload = `auth-refresh\n${host.systemId}\n${state.deviceId}\n${runtime.keyId}\n${ts}\n${nonce}`;
    const signed = await tauriInvoke("auth_sign_payload", {
      deviceId: state.deviceId,
      payload,
    });

    const { resp, body } = await relayRequestJson(host.relayUrl, "/auth/refresh", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        systemId: host.systemId,
        deviceId: state.deviceId,
        refreshToken: runtime.refreshToken,
        keyId: String(signed.keyId || runtime.keyId),
        ts,
        nonce,
        sig: String(signed.signature || ""),
      }),
    });

    if (!resp.ok || !body.ok) {
      addLog(`refresh skipped (${host.displayName}): ${body.code || resp.status} ${body.message || ""}`);
      return false;
    }

    const data = asMap(body.data);
    runtime.accessToken = String(data.accessToken || runtime.accessToken);
    runtime.refreshToken = String(data.refreshToken || runtime.refreshToken);
    runtime.keyId = String(data.keyId || runtime.keyId);
    runtime.credentialId = String(data.credentialId || runtime.credentialId);
    await storeHostSession(hostId);
    return true;
  } catch (error) {
    addLog(`refresh failed (${host.displayName}): ${error}`);
    return false;
  }
}

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

function requestToolsRefresh(hostId) {
  sendSocketEvent(hostId, "tools_refresh_request", {});
}

function requestControllerRebind(hostId) {
  const host = hostById(hostId);
  if (!host) {
    addLog("重绑失败：未选择宿主机");
    return;
  }
  const runtime = ensureRuntime(hostId);
  if (!runtime || !runtime.connected) {
    addLog(`重绑失败：宿主机未连接 (${host.displayName})`);
    return;
  }
  const sent = sendSocketEvent(hostId, "controller_rebind_request", {
    deviceId: state.deviceId,
  });
  if (sent) {
    addLog(`已请求重绑控制端 (${host.displayName})`);
  }
}

function shouldAutoRebindByReason(reason) {
  const text = String(reason || "");
  return /未绑定控制设备|未被授权|未授权控制|控制设备/.test(text);
}

function connectCandidateTool(hostId, toolId) {
  const host = hostById(hostId);
  const runtime = ensureRuntime(hostId);
  const id = String(toolId || "").trim();
  if (!host || !runtime || !id || runtime.connectingToolIds[id]) {
    return;
  }
  if (!runtime.connected) {
    addLog(`接入失败：宿主机未连接 (${host.displayName})`);
    return;
  }

  runtime.connectingToolIds[id] = true;
  setToolHidden(hostId, id, false);
  clearToolConnectTimer(runtime, id);
  runtime.toolConnectTimers[id] = setTimeout(() => {
    const current = ensureRuntime(hostId);
    if (!current || !current.connectingToolIds[id]) {
      return;
    }
    delete current.connectingToolIds[id];
    delete current.toolConnectRetryCount[id];
    clearToolConnectTimer(current, id);
    renderAddToolModal();
    openHostNoticeModal(
      "工具接入未响应",
      `工具“${id}”接入超时。请确认 relay/sidecar 正常连接后重试；必要时先重连宿主机。`,
    );
  }, 5000);
  renderAddToolModal();

  const sent = sendSocketEvent(hostId, "tool_connect_request", { toolId: id });
  if (!sent) {
    delete runtime.connectingToolIds[id];
    delete runtime.toolConnectRetryCount[id];
    clearToolConnectTimer(runtime, id);
    openHostNoticeModal("工具接入失败", `无法发送接入请求：工具“${id}”未接入。请先确认宿主机已连接。`);
    render();
  }
}

function openToolAliasEditor(hostId, toolId) {
  const runtime = ensureRuntime(hostId);
  const host = hostById(hostId);
  if (!runtime || !host || !toolId) {
    return;
  }

  const connectedTool = runtime.tools.find((item) => String(item.toolId || "") === toolId);
  const candidateTool = runtime.candidateTools.find((item) => String(item.toolId || "") === toolId);
  const tool = connectedTool || candidateTool;
  const currentAlias = getToolAlias(hostId, toolId);
  const defaultName = resolveToolDisplayName(hostId, tool || { name: "Unknown Tool", toolId });
  const nextName = window.prompt(`请输入工具显示名称（宿主机：${host.displayName}）`, currentAlias || defaultName);
  if (nextName === null) {
    return;
  }
  const normalized = String(nextName || "").trim();
  setToolAlias(hostId, toolId, normalized);
  addLog(`工具名称已更新 (${host.displayName}): ${toolId} -> ${normalized || defaultName}`);
  render();
}

function disconnectConnectedTool(hostId, toolId) {
  const host = hostById(hostId);
  const runtime = ensureRuntime(hostId);
  if (!host || !runtime) {
    return;
  }
  if (!runtime.connected) {
    openHostNoticeModal("当前宿主机未连接", "请先连接宿主机后再删除工具。");
    return;
  }
  const tool = runtime.tools.find((item) => String(item.toolId || "") === toolId);
  const name = resolveToolDisplayName(hostId, tool || { name: toolId, toolId });

  setToolHidden(hostId, toolId, true);
  runtime.tools = runtime.tools.filter((item) => String(item.toolId || "") !== toolId);
  render();

  const sent = sendSocketEvent(hostId, "tool_disconnect_request", { toolId });
  if (!sent) {
    setToolHidden(hostId, toolId, false);
    requestToolsRefresh(hostId);
    render();
    return;
  }
  addLog(`已请求断开工具 (${host.displayName}): ${toolId}`);
  openHostNoticeModal("工具已断开", `工具“${name}”已从已接入列表移除，可在候选工具中重新接入。`);
  requestToolsRefresh(hostId);
}

function sendTestEvent() {
  const hostId = state.debugHostId;
  if (!hostId) {
    addLog("发送失败：请先选择调试宿主机");
    return;
  }
  sendSocketEvent(hostId, "chat_message", {
    text: state.message,
  });
  render();
}

function ingestEvent(hostId, raw) {
  const runtime = ensureRuntime(hostId);
  if (!runtime) {
    return;
  }

  try {
    const event = JSON.parse(raw);
    if (!event || typeof event !== "object") {
      return;
    }
    const type = String(event.type || "");
    const payload = asMap(event.payload);

    if (type === "heartbeat") {
      runtime.sidecarStatus = String(payload.status || "ONLINE");
      runtime.lastHeartbeatAt = new Date();
      runtime.status = runtime.connected ? "CONNECTED" : runtime.status;
      return;
    }

    if (type === "tools_snapshot") {
      const parsed = asListOfMap(payload.tools);
      runtime.tools = sanitizeTools(hostId, parsed, false);
      for (const tool of runtime.tools) {
        const toolId = String(tool.toolId || "");
        if (toolId) {
          delete runtime.connectingToolIds[toolId];
        }
      }
      return;
    }

    if (type === "tools_candidates") {
      const parsed = asListOfMap(payload.tools);
      runtime.candidateTools = sanitizeTools(hostId, parsed, true);
      return;
    }

    if (type === "tool_whitelist_updated") {
      const toolId = String(payload.toolId || "");
      const ok = asBool(payload.ok);
      const reason = String(payload.reason || "");
      const action = String(payload.action || "connect");
      const toolName = (() => {
        const connectedTool = runtime.tools.find((item) => String(item.toolId || "") === toolId);
        const candidateTool = runtime.candidateTools.find((item) => String(item.toolId || "") === toolId);
        return resolveToolDisplayName(hostId, connectedTool || candidateTool || { name: toolId, toolId });
      })();
      if (toolId) {
        delete runtime.connectingToolIds[toolId];
        clearToolConnectTimer(runtime, toolId);
      }
      const host = hostById(hostId);
      if (!ok) {
        if (action === "disconnect" && toolId) {
          setToolHidden(hostId, toolId, false);
        }
        const actionLabel = action === "disconnect" ? "断开" : "接入";
        addLog(`${actionLabel}工具失败 (${host ? host.displayName : hostId}): ${toolId || "--"} ${reason}`);
        const retryCount = Number(runtime.toolConnectRetryCount[toolId] || 0);
        if (toolId && shouldAutoRebindByReason(reason) && retryCount < 1) {
          runtime.toolConnectRetryCount[toolId] = retryCount + 1;
          requestControllerRebind(hostId);
          addLog(`检测到控制端权限限制，已自动重绑并重试 (${host ? host.displayName : hostId}): ${toolId}`);
          setTimeout(() => connectCandidateTool(hostId, toolId), 300);
        } else {
          if (toolId) {
            delete runtime.toolConnectRetryCount[toolId];
          }
          openHostNoticeModal(
            action === "disconnect" ? "工具断开失败" : "工具接入失败",
            reason || `工具“${toolName}”未接入成功，请检查宿主机连接状态后重试。`,
          );
        }
      } else if (toolId) {
        delete runtime.toolConnectRetryCount[toolId];
        if (action === "connect") {
          setToolHidden(hostId, toolId, false);
          closeAddToolModal();
          openHostNoticeModal("添加成功", `工具“${toolName}”已接入。`);
        } else if (action === "disconnect") {
          openHostNoticeModal("断开成功", `工具“${toolName}”已断开。`);
        }
        const actionLabel = action === "disconnect" ? "断开" : "接入";
        addLog(`工具${actionLabel}已生效 (${host ? host.displayName : hostId}): ${toolId}`);
        requestToolsRefresh(hostId);
      }
      renderAddToolModal();
      return;
    }

    if (type === "controller_bind_updated") {
      const ok = asBool(payload.ok);
      const changed = asBool(payload.changed);
      const deviceId = String(payload.deviceId || "--");
      const reason = String(payload.reason || "");
      const host = hostById(hostId);
      if (!ok) {
        addLog(`控制端重绑失败 (${host ? host.displayName : hostId}): ${deviceId} ${reason}`);
      } else if (changed) {
        addLog(`控制端已切换为当前设备 (${host ? host.displayName : hostId}): ${deviceId}`);
      } else {
        addLog(`控制端已是当前设备 (${host ? host.displayName : hostId}): ${deviceId}`);
      }
      return;
    }

    if (type !== "metrics_snapshot") {
      return;
    }

    runtime.systemMetrics = asMap(payload.system);
    runtime.sidecarMetrics = asMap(payload.sidecar);
    runtime.primaryToolMetrics = asMap(payload.tool);

    const metricsByToolId = {};
    const metricsTools = asListOfMap(payload.tools);
    for (const item of metricsTools) {
      const toolId = String(item.toolId || "");
      if (toolId) {
        metricsByToolId[toolId] = item;
      }
    }

    const primaryToolId = String(runtime.primaryToolMetrics.toolId || "");
    if (primaryToolId) {
      metricsByToolId[primaryToolId] = runtime.primaryToolMetrics;
    }
    runtime.toolMetricsById = metricsByToolId;

    if (runtime.tools.length === 0) {
      if (metricsTools.length > 0) {
        runtime.tools = sanitizeTools(hostId, metricsTools, false);
      } else if (primaryToolId) {
        runtime.tools = sanitizeTools(hostId, [
          {
            toolId: primaryToolId,
            name: String(runtime.primaryToolMetrics.name || "Unknown Tool"),
            category: String(runtime.primaryToolMetrics.category || "UNKNOWN"),
            vendor: String(runtime.primaryToolMetrics.vendor || "-"),
            mode: String(runtime.primaryToolMetrics.mode || "-"),
            status: String(runtime.primaryToolMetrics.status || "RUNNING"),
            connected: runtime.primaryToolMetrics.connected,
            endpoint: String(runtime.primaryToolMetrics.endpoint || ""),
            reason: String(runtime.primaryToolMetrics.reason || ""),
          },
        ], false);
      }
    }
  } catch (_) {
    // ignore invalid payload
  }
}

function sanitizeTools(hostId, source, includeHidden) {
  return source.filter((tool) => {
    const name = String(tool.name || "").toLowerCase();
    const category = String(tool.category || "").toLowerCase();
    const toolId = String(tool.toolId || "").trim();
    if (name.includes("mobile") || name.includes("client app")) {
      return false;
    }
    if (category.includes("client")) {
      return false;
    }
    if (!includeHidden && toolId && isToolHidden(hostId, toolId)) {
      return false;
    }
    return true;
  });
}

function metricForTool(hostId, toolId) {
  const runtime = ensureRuntime(hostId);
  if (!runtime || !toolId) {
    return {};
  }
  if (runtime.toolMetricsById[toolId]) {
    return runtime.toolMetricsById[toolId];
  }
  if (runtime.primaryToolMetrics.toolId === toolId) {
    return runtime.primaryToolMetrics;
  }
  return {};
}

function hostStatusLabel(hostId) {
  const runtime = ensureRuntime(hostId);
  if (!runtime) {
    return "离线";
  }
  if (runtime.connecting) {
    return "连接中";
  }
  if (runtime.connected) {
    return runtime.sidecarStatus === "ONLINE" ? "在线" : runtime.sidecarStatus;
  }
  if (runtime.manualReconnectRequired) {
    return "需手动重连";
  }
  if (runtime.status === "AUTH_EXPIRED") {
    return "凭证失效";
  }
  if (runtime.status === "RELAY_UNREACHABLE") {
    return "Relay 不可达";
  }
  return "离线";
}

function isAnyHostConnected() {
  return visibleHosts().some((host) => {
    const runtime = ensureRuntime(host.hostId);
    return runtime && runtime.connected;
  });
}

function hasConnectableHost() {
  return visibleHosts().some((host) => {
    const runtime = ensureRuntime(host.hostId);
    return runtime && !runtime.connected && !runtime.connecting;
  });
}

function render() {
  recomputeSelections();
  renderTabs();
  renderTopActions();
  renderHostStage();
  renderBanner();
  renderToolsGrouped();
  renderDebugPanel();
  renderToolModal();
  renderAddToolModal();
  renderHostManageModal();
  renderHostMetricsModal();
}

function renderTabs() {
  renderTabsView(state, ui);
}

function renderTopActions() {
  const hostCount = visibleHosts().length;
  ui.connectBtnTop.disabled = hostCount === 0 || !hasConnectableHost();
  ui.disconnectBtnTop.disabled = hostCount === 0 || !isAnyHostConnected();
  ui.replaceHostBtnTop.disabled = false;
}

function renderHostStage() {
  const hasHosts = visibleHosts().length > 0;
  renderHostStageView(ui, hasHosts);
}

function renderBanner() {
  const hosts = visibleHosts();
  renderBannerView(ui, hosts, state.bannerActiveIndex, hostStatusLabel, escapeHtml);
}

function renderBannerDots(count, activeIndex) {
  renderBannerDotsView(ui, count, activeIndex);
}

function onHostBannerScroll() {
  const active = deriveBannerActiveIndex(ui.hostBannerTrack);
  if (!active) {
    return;
  }
  if (active.index !== state.bannerActiveIndex) {
    state.bannerActiveIndex = active.index;
    renderBannerDots(active.total, state.bannerActiveIndex);
  }
}

function onHostBannerClick(event) {
  const target = event.target;
  if (!(target instanceof Element)) {
    return;
  }
  const card = target.closest("[data-banner-host-id]");
  if (!card) {
    return;
  }
  const hostId = String(card.getAttribute("data-banner-host-id") || "").trim();
  openHostMetricsModal(hostId);
}

function renderToolsGrouped() {
  const hosts = visibleHosts();
  if (hosts.length === 0) {
    ui.toolsGroupedList.innerHTML = '<div class="empty">暂无宿主机，请先完成配对。</div>';
    state.activeToolSwipeKey = "";
    return;
  }

  ui.toolsGroupedList.innerHTML = hosts
    .map((host) => renderHostGroup(host))
    .join("");
  syncToolSwipePositions();
}

function renderHostGroup(host) {
  const runtime = ensureRuntime(host.hostId);
  const status = hostStatusLabel(host.hostId);
  const canAddTool = runtime && runtime.connected;
  const toolCards = renderHostTools(host.hostId);

  return `
    <article class="host-group" data-host-group-id="${escapeHtml(host.hostId)}">
      <div class="host-group-head">
        <div class="host-group-title">${escapeHtml(host.displayName)}</div>
        <span class="host-status-chip">${escapeHtml(status)}</span>
      </div>
      <div class="host-group-actions" style="grid-template-columns: 1fr">
        <button class="btn btn-outline btn-sm" data-host-add-tool="${escapeHtml(host.hostId)}" ${canAddTool ? "" : "disabled"}>+ 工具</button>
      </div>
      <div class="host-group-tools">
        ${toolCards}
      </div>
    </article>
  `;
}

function renderHostTools(hostId) {
  const runtime = ensureRuntime(hostId);
  if (!runtime || runtime.tools.length === 0) {
    return '<div class="empty">该宿主机暂无已接入工具。</div>';
  }

  return runtime.tools
    .map((tool) => {
      const toolId = String(tool.toolId || "");
      const metric = metricForTool(hostId, toolId);
      if (isOpenCodeTool(tool)) {
        return renderOpenCodeCard(hostId, tool, metric);
      }
      return renderGenericCard(hostId, tool, metric);
    })
    .join("");
}

function onToolsGroupedClick(event) {
  const connectBtn = event.target.closest("[data-host-connect]");
  if (connectBtn) {
    const hostId = String(connectBtn.getAttribute("data-host-connect") || "");
    const runtime = ensureRuntime(hostId);
    if (runtime && runtime.connected) {
      void reconnectHost(hostId);
    } else {
      void connectHost(hostId, { manual: true, resetRetry: true });
    }
    return;
  }

  const disconnectBtn = event.target.closest("[data-host-disconnect]");
  if (disconnectBtn) {
    const hostId = String(disconnectBtn.getAttribute("data-host-disconnect") || "");
    void disconnectHost(hostId, { triggerReconnect: false });
    return;
  }

  const addToolBtn = event.target.closest("[data-host-add-tool]");
  if (addToolBtn) {
    const hostId = String(addToolBtn.getAttribute("data-host-add-tool") || "");
    openAddToolModal(hostId);
    return;
  }

  const manualReconnectBtn = event.target.closest("[data-host-manual-reconnect]");
  if (manualReconnectBtn) {
    const hostId = String(manualReconnectBtn.getAttribute("data-host-manual-reconnect") || "");
    const runtime = ensureRuntime(hostId);
    if (runtime) {
      runtime.manualReconnectRequired = false;
      runtime.retryCount = 0;
    }
    void connectHost(hostId, { manual: true, resetRetry: true });
    return;
  }

  const manageBtn = event.target.closest("[data-host-open-manage]");
  if (manageBtn) {
    openHostManageModal();
    return;
  }

  const editToolBtn = event.target.closest("[data-tool-edit]");
  if (editToolBtn) {
    const raw = String(editToolBtn.getAttribute("data-tool-edit") || "");
    const [hostId, toolId] = raw.split("::");
    if (hostId && toolId) {
      openToolAliasEditor(hostId, toolId);
    }
    return;
  }

  const deleteToolBtn = event.target.closest("[data-tool-delete]");
  if (deleteToolBtn) {
    const raw = String(deleteToolBtn.getAttribute("data-tool-delete") || "");
    const [hostId, toolId] = raw.split("::");
    if (hostId && toolId) {
      disconnectConnectedTool(hostId, toolId);
    }
    return;
  }

  const card = event.target.closest("[data-host-id][data-tool-id]");
  if (!card) {
    return;
  }
  const hostId = String(card.getAttribute("data-host-id") || "");
  const toolId = String(card.getAttribute("data-tool-id") || "");
  openToolDetail(hostId, toolId);
}

function openAddToolModal(hostId) {
  const host = hostById(hostId);
  if (!host) {
    return;
  }
  state.addToolHostId = hostId;
  ui.addToolModal.classList.add("show");
  requestToolsRefresh(hostId);
  renderAddToolModal();
}

function closeAddToolModal() {
  ui.addToolModal.classList.remove("show");
  state.addToolHostId = "";
}

function onCandidateListClick(event) {
  const btn = event.target.closest("[data-connect-tool-id]");
  if (!btn) {
    return;
  }
  const toolId = String(btn.getAttribute("data-connect-tool-id") || "");
  if (!state.addToolHostId) {
    return;
  }
  connectCandidateTool(state.addToolHostId, toolId);
  renderAddToolModal();
}

function renderAddToolModal() {
  if (!ui.addToolModal.classList.contains("show")) {
    return;
  }

  const host = hostById(state.addToolHostId);
  const runtime = ensureRuntime(state.addToolHostId);
  if (!host || !runtime) {
    ui.candidateList.innerHTML = '<div class="empty">未找到宿主机，请重新打开。</div>';
    return;
  }

  if (!runtime.connected) {
    ui.candidateList.innerHTML = '<div class="empty">请先连接该宿主机，再添加工具。</div>';
    return;
  }

  if (runtime.candidateTools.length === 0) {
    if (runtime.tools.length > 0) {
      ui.candidateList.innerHTML =
        '<div class="empty">当前没有候选工具。已发现的工具可能已接入（候选列表仅展示未接入工具）。</div>';
    } else {
      ui.candidateList.innerHTML =
        '<div class="empty">当前没有候选工具。请确认宿主机已运行 opencode/openclaw，并等待一次快照刷新。</div>';
    }
    return;
  }

  ui.candidateList.innerHTML = runtime.candidateTools
    .map((tool) => {
      const toolId = String(tool.toolId || "");
      const title = resolveToolDisplayName(state.addToolHostId, tool);
      const connecting = asBool(runtime.connectingToolIds[toolId]);
      const reason = String(tool.reason || "已发现可接入进程");
      return `
        <article class="candidate-item">
          <div class="candidate-head">
            <div class="candidate-title">${escapeHtml(title)}</div>
            <span class="chip">${escapeHtml(localizedCategory(tool.category))}</span>
            <div class="candidate-actions">
              <button class="btn btn-primary btn-sm" data-connect-tool-id="${escapeHtml(toolId)}" ${connecting ? "disabled" : ""}>
                ${connecting ? "接入中..." : "接入"}
              </button>
            </div>
          </div>
          <div class="candidate-meta">${escapeHtml(reason)}</div>
        </article>
      `;
    })
    .join("");
}

function renderDebugPanel() {
  renderDebugPanelView(
    state,
    ui,
    visibleHosts,
    hostById,
    ensureRuntime,
    maskSecret,
    escapeHtml,
  );
}

function renderToolSwipeActions(hostId, toolId) {
  return `
    <div class="tool-swipe-actions">
      <button class="tool-action-btn edit" type="button" data-tool-edit="${escapeHtml(hostId)}::${escapeHtml(toolId)}" aria-label="编辑工具名称">
        <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path d="M4 20h4l9.8-9.8-4-4L4 16v4z" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"></path>
          <path d="M13.6 6.2l4 4" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"></path>
        </svg>
        编辑
      </button>
      <button class="tool-action-btn delete" type="button" data-tool-delete="${escapeHtml(hostId)}::${escapeHtml(toolId)}" aria-label="删除已接入工具">
        <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path d="M5 7h14" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"></path>
          <path d="M9 7V5h6v2" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"></path>
          <path d="M8 7l.8 11.1a1.5 1.5 0 0 0 1.5 1.4h3.4a1.5 1.5 0 0 0 1.5-1.4L16 7" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"></path>
        </svg>
        删除
      </button>
    </div>
  `;
}

function renderOpenCodeCard(hostId, tool, metric) {
  const toolId = String(tool.toolId || "");
  const swipeKey = `${hostId}::${toolId}`;
  const displayName = resolveToolDisplayName(hostId, tool);
  const mode = String(metric.mode ?? tool.mode ?? "TUI");
  const endpoint = String(metric.endpoint ?? tool.endpoint ?? "");
  const reason = String(metric.reason ?? tool.reason ?? "");
  const connected = asBool(metric.connected ?? tool.connected);
  const status = String(metric.status ?? tool.status ?? "UNKNOWN");
  const model = String(metric.model ?? tool.model ?? "");
  const agentMode = String(metric.agentMode ?? tool.agentMode ?? "");
  const workspaceDir = String(metric.workspaceDir ?? tool.workspaceDir ?? "");
  const note = buildOpenCodeCardNote({
    endpoint,
    reason,
    model,
    agentMode,
    workspaceDir,
  });
  const noteSection = note ? `<p class="tool-note">${escapeHtml(note)}</p>` : "";

  return `
    <div class="tool-swipe" data-tool-swipe-key="${escapeHtml(swipeKey)}">
      <article class="tool-card tool-opencode" data-host-id="${escapeHtml(hostId)}" data-tool-id="${escapeHtml(toolId)}">
        <div class="tool-head">
          <div class="tool-logo">OC</div>
          <div class="tool-name">${escapeHtml(displayName)}</div>
          <span class="chip">${escapeHtml(mode.toUpperCase())}</span>
        </div>
        <div class="chip-wrap">
          <span class="chip">${escapeHtml(status)}</span>
          <span class="chip">${connected ? "已接入" : "未接入"}</span>
        </div>
        ${noteSection}
        <div class="tool-metrics">
          <div class="tool-metric">
            <div class="name">CPU</div>
            <div class="value">${escapeHtml(fmt2(metric.cpuPercent))}%</div>
          </div>
          <div class="tool-metric">
            <div class="name">Memory</div>
            <div class="value">${escapeHtml(fmt2(metric.memoryMb))} MB</div>
          </div>
        </div>
      </article>
      ${renderToolSwipeActions(hostId, toolId)}
    </div>
  `;
}

function buildOpenCodeCardNote(input) {
  const endpoint = String(input.endpoint || "").trim();
  const reason = String(input.reason || "").trim();
  const model = String(input.model || "").trim();
  const agentMode = String(input.agentMode || "").trim();
  const workspaceDir = String(input.workspaceDir || "").trim();

  if (endpoint) {
    return endpoint;
  }
  // 过滤“等待会话同步”类占位文案，只保留明确错误/限制原因。
  if (reason && !/等待会话|补充模式和模型信息/.test(reason)) {
    return reason;
  }
  if (model) {
    return `当前模型：${model}`;
  }
  if (agentMode) {
    return `会话模式：${agentMode}`;
  }
  if (workspaceDir) {
    return `工作目录：${workspaceDir}`;
  }
  return "";
}

function renderGenericCard(hostId, tool, metric) {
  const toolId = String(tool.toolId || "");
  const swipeKey = `${hostId}::${toolId}`;
  const displayName = resolveToolDisplayName(hostId, tool);
  return `
    <div class="tool-swipe" data-tool-swipe-key="${escapeHtml(swipeKey)}">
      <article class="tool-card tool-generic" data-host-id="${escapeHtml(hostId)}" data-tool-id="${escapeHtml(toolId)}">
        <div class="bar"></div>
        <div>
          <div class="title">${escapeHtml(displayName)}</div>
          <div class="sub">${escapeHtml(localizedCategory(tool.category))} · ${escapeHtml(String(tool.status || "-"))}</div>
        </div>
        <div class="right">
          <div>${escapeHtml(fmt2(metric.cpuPercent))}% CPU</div>
          <div class="sub">${escapeHtml(fmt2(metric.memoryMb))} MB</div>
        </div>
      </article>
      ${renderToolSwipeActions(hostId, toolId)}
    </div>
  `;
}

function isOpenCodeTool(tool) {
  const toolId = String(tool.toolId || "").toLowerCase();
  const name = String(tool.name || "").toLowerCase();
  const vendor = String(tool.vendor || "").toLowerCase();
  return toolId.startsWith("opencode_") || name.includes("opencode") || vendor.includes("opencode");
}

function closeActiveToolSwipe() {
  if (!state.activeToolSwipeKey) {
    return;
  }
  state.activeToolSwipeKey = "";
  syncToolSwipePositions();
}

function onGlobalPointerDown(event) {
  if (!state.activeToolSwipeKey) {
    return;
  }
  const target = event.target;
  if (!(target instanceof Element)) {
    return;
  }
  if (target.closest(".tool-swipe")) {
    return;
  }
  closeActiveToolSwipe();
}

function onToolSwipeScrollCapture(event) {
  const swipe = event.target;
  if (!(swipe instanceof Element) || !swipe.classList.contains("tool-swipe")) {
    return;
  }
  const key = String(swipe.getAttribute("data-tool-swipe-key") || "").trim();
  if (!key) {
    return;
  }
  const maxOffset = Math.max(0, swipe.scrollWidth - swipe.clientWidth);
  if (maxOffset <= 0) {
    return;
  }
  // 阈值偏小，确保在模拟器鼠标拖动时也能快速锁定为“展开”状态。
  const openThreshold = Math.max(8, maxOffset * 0.12);
  const closeThreshold = Math.max(4, maxOffset * 0.08);
  if (swipe.scrollLeft >= openThreshold) {
    if (state.activeToolSwipeKey !== key) {
      state.activeToolSwipeKey = key;
      syncToolSwipePositions();
    }
    return;
  }
  if (state.activeToolSwipeKey === key && swipe.scrollLeft <= closeThreshold) {
    state.activeToolSwipeKey = "";
    syncToolSwipePositions();
  }
}

function syncToolSwipePositions() {
  const swipes = Array.from(ui.toolsGroupedList.querySelectorAll(".tool-swipe[data-tool-swipe-key]"));
  if (swipes.length === 0) {
    state.activeToolSwipeKey = "";
    return;
  }
  let activeExists = false;
  for (const swipe of swipes) {
    const key = String(swipe.getAttribute("data-tool-swipe-key") || "").trim();
    const maxOffset = Math.max(0, swipe.scrollWidth - swipe.clientWidth);
    const shouldOpen = Boolean(state.activeToolSwipeKey) && key === state.activeToolSwipeKey && maxOffset > 0;
    if (shouldOpen) {
      activeExists = true;
      if (Math.abs(swipe.scrollLeft - maxOffset) > 1) {
        swipe.scrollLeft = maxOffset;
      }
    } else if (swipe.scrollLeft > 1) {
      swipe.scrollLeft = 0;
    }
  }
  if (state.activeToolSwipeKey && !activeExists) {
    state.activeToolSwipeKey = "";
  }
}

function openToolDetail(hostId, toolId) {
  if (!hostId || !toolId) {
    return;
  }
  state.detailHostId = hostId;
  state.detailToolId = toolId;
  state.detailExpanded = false;
  renderToolModal();
}

function closeToolDetail() {
  state.detailHostId = "";
  state.detailToolId = "";
  state.detailExpanded = false;
  renderToolModal();
}

function renderToolModal() {
  if (!state.detailHostId || !state.detailToolId) {
    ui.toolModal.classList.remove("show");
    return;
  }

  const host = hostById(state.detailHostId);
  const runtime = ensureRuntime(state.detailHostId);
  if (!host || !runtime) {
    ui.toolModal.classList.remove("show");
    return;
  }

  const tool = runtime.tools.find((item) => String(item.toolId || "") === state.detailToolId);
  if (!tool) {
    ui.toolModal.classList.remove("show");
    return;
  }

  const metric = metricForTool(state.detailHostId, String(tool.toolId || ""));
  const pick = (key) => {
    const value = metric[key] ?? tool[key];
    return value == null ? "" : String(value);
  };

  const toolId = String(tool.toolId || "");
  const endpoint = pick("endpoint");
  const mode = pick("mode");
  const status = pick("status");
  const reason = pick("reason");
  const vendor = pick("vendor");
  const workspaceDir = pick("workspaceDir");
  const sessionId = pick("sessionId");
  const sessionTitle = pick("sessionTitle");
  const sessionUpdatedAt = pick("sessionUpdatedAt");
  const agentMode = pick("agentMode");
  const model = pick("model");

  const connectedTool = asBool(metric.connected ?? tool.connected);
  const latestTokens = asMap(metric.latestTokens);
  const modelUsage = asListOfMap(metric.modelUsage);
  const displayName = resolveToolDisplayName(state.detailHostId, tool);

  const summaryRows = [
    ["宿主机", host.displayName],
    ["工具名称", displayName],
    ["工具模式", mode || "--"],
    ["会话模式", agentMode || "--"],
    ["当前模型", model || "--"],
    ["状态", status || "--"],
    [
      "最近Token（总/输入/输出）",
      `${fmtTokenM(latestTokens.total)} / ${fmtTokenM(latestTokens.input)} / ${fmtTokenM(latestTokens.output)}`,
    ],
    [
      "最近缓存（读/写）",
      `${fmtTokenM(latestTokens.cacheRead)} / ${fmtTokenM(latestTokens.cacheWrite)}`,
    ],
    ["模型用量", usageSummary(modelUsage)],
  ];
  if (reason) {
    summaryRows.push(["原因", reason]);
  }

  const detailsRows = [
    ["App Link", runtime.connected ? "Connected" : "Disconnected"],
    ["Last Heartbeat", runtime.lastHeartbeatAt ? runtime.lastHeartbeatAt.toLocaleString() : "--"],
    ["Tool Reachable", connectedTool ? "Yes" : "No"],
    ["Tool ID", toolId || "--"],
    ["Endpoint", endpoint || "--"],
    ["Workspace", workspaceDir || "--"],
    ["Session ID", sessionId || "--"],
    ["Session Title", sessionTitle || "--"],
    ["Session Updated", sessionUpdatedAt || "--"],
    ["厂商", vendor || "--"],
    ["类别", localizedCategory(tool.category)],
    ["CPU", `${fmt2(metric.cpuPercent)}%`],
    ["Memory", `${fmt2(metric.memoryMb)} MB`],
    ["Source", String(metric.source || "--")],
    ["Latest Cache", `R:${fmtTokenM(latestTokens.cacheRead)} W:${fmtTokenM(latestTokens.cacheWrite)}`],
  ];

  ui.toolModalTitle.textContent = displayName || "Tool Detail";
  ui.summaryRows.innerHTML = renderRows(summaryRows);

  const previewCount = 2;
  const showingRows = state.detailExpanded ? detailsRows : detailsRows.slice(0, previewCount);
  ui.detailRows.innerHTML = renderRows(showingRows);
  ui.detailTip.textContent =
    !state.detailExpanded && detailsRows.length > previewCount
      ? `还有 ${detailsRows.length - previewCount} 项，点击箭头展开`
      : "";

  ui.toggleDetailsBtn.textContent = state.detailExpanded ? "⌃" : "⌄";

  if (state.detailExpanded && modelUsage.length > 0) {
    ui.usagePanel.style.display = "block";
    ui.usageRows.innerHTML = renderRows(
      modelUsage.map((row) => {
        const modelName = String(row.model || "--");
        const total = fmtTokenM(row.tokenTotal);
        const input = fmtTokenM(row.tokenInput);
        const output = fmtTokenM(row.tokenOutput);
        const count = fmtInt(row.messages);
        return [modelName, `消息 ${count} 条 · 总Token ${total} · 输入 ${input} · 输出 ${output}`];
      }),
    );
  } else {
    ui.usagePanel.style.display = "none";
    ui.usageRows.innerHTML = "";
  }

  ui.toolModal.classList.add("show");
}

function openHostManageModal() {
  ui.hostManageModal.classList.add("show");
  renderHostManageModal();
}

function closeHostManageModal() {
  ui.hostManageModal.classList.remove("show");
}

function renderHostManageModal() {
  if (!ui.hostManageModal.classList.contains("show")) {
    return;
  }

  const hosts = visibleHosts();
  if (hosts.length === 0) {
    ui.hostManageList.innerHTML = '<div class="empty">暂无已配对宿主机。</div>';
  } else {
    ui.hostManageList.innerHTML = hosts
      .map((host) => {
        const runtime = ensureRuntime(host.hostId);
        const status = hostStatusLabel(host.hostId);
        const note = host.note ? ` · 备注: ${host.note}` : "";
        const connectLabel = runtime && runtime.connected ? "重连" : runtime && runtime.connecting ? "连接中" : "连接";
        return `
          <article class="host-manage-item">
            <div class="host-manage-name">${escapeHtml(host.displayName)}</div>
            <div class="host-manage-sub">状态: ${escapeHtml(status)}${escapeHtml(note)}</div>
            <div class="host-manage-actions">
              <button class="btn btn-primary btn-sm" data-manage-connect="${escapeHtml(host.hostId)}">${escapeHtml(connectLabel)}</button>
              <button class="btn btn-outline btn-sm" data-manage-disconnect="${escapeHtml(host.hostId)}">断开</button>
              <button class="btn btn-outline btn-sm" data-manage-edit="${escapeHtml(host.hostId)}">编辑</button>
              <button class="btn btn-outline btn-sm" data-manage-repair="${escapeHtml(host.hostId)}">重新配对</button>
              <button class="btn btn-outline btn-sm" data-manage-delete="${escapeHtml(host.hostId)}">删除</button>
              <button class="btn btn-outline btn-sm" data-manage-open-debug="${escapeHtml(host.hostId)}">调试此宿主机</button>
            </div>
          </article>
        `;
      })
      .join("");
  }

  if (state.pendingHostDeletes.length === 0) {
    ui.pendingDeleteList.innerHTML = '<div class="empty">当前无删除补偿任务。</div>';
  } else {
      ui.pendingDeleteList.innerHTML = state.pendingHostDeletes
        .map((item) => {
          const retryAt = new Date(Number(item.nextRetryAt || 0));
          return `
            <article class="host-manage-item">
            <div class="host-manage-name">${escapeHtml(item.displayName || item.systemId)}</div>
            <div class="host-manage-sub">
              删除处理中 · 重试 ${escapeHtml(String(item.retryCount || 0))} 次 · 下次: ${escapeHtml(
                Number.isFinite(retryAt.getTime()) ? retryAt.toLocaleString() : "--",
              )}
            </div>
              <div class="host-manage-sub">最近错误: ${escapeHtml(item.lastError || "--")}</div>
              <div class="host-manage-actions">
                <button class="btn btn-outline btn-sm" data-pending-retry="${escapeHtml(item.hostId)}">立即重试删除</button>
                <button class="btn btn-outline btn-sm" data-pending-force-remove="${escapeHtml(item.hostId)}">强制移除任务</button>
              </div>
            </article>
          `;
        })
        .join("");
  }
}

function onHostManageListClick(event) {
  const connectBtn = event.target.closest("[data-manage-connect]");
  if (connectBtn) {
    const hostId = String(connectBtn.getAttribute("data-manage-connect") || "");
    const runtime = ensureRuntime(hostId);
    if (runtime && runtime.connected) {
      void reconnectHost(hostId);
    } else {
      void connectHost(hostId, { manual: true, resetRetry: true });
    }
    return;
  }

  const disconnectBtn = event.target.closest("[data-manage-disconnect]");
  if (disconnectBtn) {
    const hostId = String(disconnectBtn.getAttribute("data-manage-disconnect") || "");
    void disconnectHost(hostId, { triggerReconnect: false });
    return;
  }

  const editBtn = event.target.closest("[data-manage-edit]");
  if (editBtn) {
    const hostId = String(editBtn.getAttribute("data-manage-edit") || "");
    openHostEditModal(hostId);
    return;
  }

  const repairBtn = event.target.closest("[data-manage-repair]");
  if (repairBtn) {
    const hostId = String(repairBtn.getAttribute("data-manage-repair") || "");
    closeHostManageModal();
    openPairFlow("import", hostId);
    return;
  }

  const deleteBtn = event.target.closest("[data-manage-delete]");
  if (deleteBtn) {
    const hostId = String(deleteBtn.getAttribute("data-manage-delete") || "");
    void deleteHostWithCompensation(hostId);
    return;
  }

  const debugBtn = event.target.closest("[data-manage-open-debug]");
  if (debugBtn) {
    state.debugHostId = String(debugBtn.getAttribute("data-manage-open-debug") || "");
    state.activeTab = "debug";
    closeHostManageModal();
    render();
  }
}

function onPendingDeleteListClick(event) {
  const retryBtn = event.target.closest("[data-pending-retry]");
  if (retryBtn) {
    const hostId = String(retryBtn.getAttribute("data-pending-retry") || "");
    void retryPendingDelete(hostId, true);
    return;
  }

  const forceRemoveBtn = event.target.closest("[data-pending-force-remove]");
  if (!forceRemoveBtn) {
    return;
  }
  const hostId = String(forceRemoveBtn.getAttribute("data-pending-force-remove") || "");
  void forceRemovePendingDelete(hostId, true);
}

function openHostEditModal(hostId) {
  const host = hostById(hostId);
  if (!host) {
    return;
  }
  state.editingHostId = hostId;
  ui.hostEditNameInput.value = host.displayName || "";
  ui.hostEditNoteInput.value = host.note || "";
  ui.hostEditModal.classList.add("show");
}

function closeHostEditModal() {
  ui.hostEditModal.classList.remove("show");
  state.editingHostId = "";
}

function saveHostEdit() {
  const host = hostById(state.editingHostId);
  if (!host) {
    closeHostEditModal();
    return;
  }

  const newName = String(ui.hostEditNameInput.value || "").trim();
  const newNote = String(ui.hostEditNoteInput.value || "").trim();
  host.displayName = newName || host.systemId;
  host.note = newNote;
  host.updatedAt = new Date().toISOString();

  persistConfig();
  closeHostEditModal();
  notifyIfDuplicateDisplayName(host.hostId);
  render();
}

function openHostMetricsModal(hostId) {
  if (!hostId) {
    return;
  }
  state.hostMetricsHostId = hostId;
  renderHostMetricsModal();
}

function closeHostMetricsModal() {
  state.hostMetricsHostId = "";
  ui.hostMetricsModal.classList.remove("show");
}

function renderHostMetricsModal() {
  if (!state.hostMetricsHostId) {
    ui.hostMetricsModal.classList.remove("show");
    return;
  }

  const host = hostById(state.hostMetricsHostId);
  const runtime = ensureRuntime(state.hostMetricsHostId);
  if (!host || !runtime) {
    closeHostMetricsModal();
    return;
  }

  const system = asMap(runtime.systemMetrics);
  const sidecar = asMap(runtime.sidecarMetrics);
  const hasSystemMetrics = Object.keys(system).length > 0;
  const hasSidecarMetrics = Object.keys(sidecar).length > 0;

  const memoryUsedGb = formatGbFromMb(system.memoryUsedMb);
  const memoryTotalGb = formatGbFromMb(system.memoryTotalMb);
  const diskUsedGb = fmt2(system.diskUsedGb);
  const diskTotalGb = fmt2(system.diskTotalGb);

  const systemRows = hasSystemMetrics
    ? [
        ["状态", hostStatusLabel(host.hostId)],
        ["CPU", `${fmt2(system.cpuPercent)}%`],
        ["内存", `${memoryUsedGb} / ${memoryTotalGb} GB (${fmt2(system.memoryUsedPercent)}%)`],
        ["磁盘", `${diskUsedGb} / ${diskTotalGb} GB (${fmt2(system.diskUsedPercent)}%)`],
        ["运行时长", formatDurationShort(system.uptimeSec)],
        ["最近心跳", runtime.lastHeartbeatAt ? runtime.lastHeartbeatAt.toLocaleString() : "--"],
      ]
    : [
        ["状态", hostStatusLabel(host.hostId)],
        ["指标", "尚未收到系统负载快照，请等待 sidecar 下一次上报。"],
      ];

  const sidecarRows = hasSidecarMetrics
    ? [
        ["CPU", `${fmt2(sidecar.cpuPercent)}%`],
        ["内存", `${fmt2(sidecar.memoryMb)} MB`],
      ]
    : [["状态", "尚未收到 sidecar 负载快照。"]];

  ui.hostMetricsTitle.textContent = `${host.displayName} · 宿主机负载`;
  ui.hostMetricsRows.innerHTML = renderRows(systemRows);
  ui.hostSidecarRows.innerHTML = renderRows(sidecarRows);
  ui.hostMetricsTip.textContent = hasSystemMetrics
    ? "数据来源于 metrics_snapshot（实时刷新）。"
    : "提示：若持续没有数据，请确认 relay 与 sidecar 均在线。";
  ui.hostMetricsModal.classList.add("show");
}

function openHostNoticeModal(title, body, options = {}) {
  // 通知弹窗支持两种模式：
  // 1. 默认提示：仅“知道了”单按钮；
  // 2. 重名引导：显示“去修改名称 / 稍后处理”双按钮。
  const normalized =
    typeof options === "string"
      ? {
          targetHostId: options,
          primaryAction: options ? "edit" : "dismiss",
          primaryLabel: options ? "去修改名称" : "知道了",
          secondaryLabel: options ? "稍后处理" : "",
        }
      : asMap(options);
  const targetHostId = String(normalized.targetHostId || "").trim();
  const primaryAction = String(normalized.primaryAction || (targetHostId ? "edit" : "dismiss")).trim();
  const primaryLabel = String(normalized.primaryLabel || (primaryAction === "edit" ? "去修改名称" : "知道了"))
    .trim();
  const secondaryLabel = String(
    normalized.secondaryLabel === undefined
      ? primaryAction === "edit"
        ? "稍后处理"
        : ""
      : normalized.secondaryLabel,
  ).trim();
  ui.hostNoticeTitle.textContent = title;
  ui.hostNoticeBody.textContent = body;
  state.hostNoticeTargetId = targetHostId;
  state.hostNoticePrimaryAction = primaryAction || "dismiss";
  ui.hostNoticePrimaryBtn.textContent = primaryLabel || "知道了";
  if (secondaryLabel) {
    ui.hostNoticeSecondaryBtn.textContent = secondaryLabel;
    ui.hostNoticeSecondaryBtn.style.display = "";
  } else {
    ui.hostNoticeSecondaryBtn.style.display = "none";
  }
  ui.hostNoticeModal.classList.add("show");
}

function closeHostNoticeModal() {
  ui.hostNoticeModal.classList.remove("show");
  state.hostNoticeTargetId = "";
  state.hostNoticePrimaryAction = "dismiss";
  ui.hostNoticePrimaryBtn.textContent = "知道了";
  ui.hostNoticeSecondaryBtn.textContent = "稍后处理";
  ui.hostNoticeSecondaryBtn.style.display = "";
}

function notifyIfDuplicateDisplayName(hostId) {
  const host = hostById(hostId);
  if (!host) {
    return;
  }
  const sameNameHosts = visibleHosts().filter((item) => item.displayName === host.displayName);
  if (sameNameHosts.length <= 1) {
    return;
  }

  const suggested = `${host.displayName}-${shortSystemId(host.systemId)}`;
  openHostNoticeModal(
    "宿主机名称重复",
    `检测到多个宿主机使用同名“${host.displayName}”。建议改名为“${suggested}”便于识别，不影响当前连接。`,
    {
      targetHostId: hostId,
      primaryAction: "edit",
      primaryLabel: "去修改名称",
      secondaryLabel: "稍后处理",
    },
  );
}

function shortSystemId(systemId) {
  const raw = String(systemId || "").trim();
  if (!raw) {
    return "host";
  }
  return raw.length <= 8 ? raw : raw.slice(0, 8);
}

async function deleteHostWithCompensation(hostId) {
  const host = hostById(hostId);
  if (!host) {
    return;
  }

  let expectedCredentialId = "";
  let expectedKeyId = "";
  try {
    const session = await tauriInvoke("auth_load_session", {
      systemId: host.systemId,
      deviceId: state.deviceId,
    });
    if (session) {
      expectedCredentialId = String(session.credentialId || "").trim();
      expectedKeyId = String(session.keyId || "").trim();
    }
  } catch (error) {
    addLog(`delete preload session failed (${host.displayName}): ${error}`);
  }

  // 从主视图移除并写入补偿队列，保证页面即时隐藏。
  state.hosts = state.hosts.filter((item) => item.hostId !== hostId);
  disposeRuntime(hostId);
  clearToolMetaForHost(hostId);

  state.pendingHostDeletes.push({
    hostId: host.hostId,
    systemId: host.systemId,
    relayUrl: host.relayUrl,
    displayName: host.displayName,
    deviceId: state.deviceId,
    enqueuedAt: Date.now(),
    retryCount: 0,
    nextRetryAt: Date.now(),
    lastError: "",
    expectedCredentialId,
    expectedKeyId,
  });

  recomputeSelections();
  persistConfig();
  render();

  openHostNoticeModal(
    "删除任务已接收",
    "当前 Relay 可能不可达。系统将在可连接 Relay 后自动执行删除；该宿主机已从主页面隐藏。",
  );

  await retryPendingDelete(hostId, true);
}

async function processPendingDeletes() {
  if (state.deleteCompensating) {
    return;
  }
  if (state.pendingHostDeletes.length === 0) {
    return;
  }
  state.deleteCompensating = true;
  try {
    const now = Date.now();
    const due = state.pendingHostDeletes.filter((item) => Number(item.nextRetryAt || 0) <= now);
    for (const item of due) {
      await retryPendingDelete(item.hostId, false);
    }
  } finally {
    state.deleteCompensating = false;
  }
}

async function retryPendingDelete(hostId, manual) {
  const index = state.pendingHostDeletes.findIndex((item) => item.hostId === hostId);
  if (index < 0) {
    return;
  }
  const item = state.pendingHostDeletes[index];

  try {
    await revokeAndClearPendingHost(item);
    state.pendingHostDeletes.splice(index, 1);
    persistConfig();
    addLog(`删除补偿完成: ${item.displayName}`);
    if (manual) {
      openHostNoticeModal("删除完成", `宿主机“${item.displayName}”已完成最终删除。`);
    }
    render();
  } catch (error) {
    const errorCode = normalizeDeleteCompensationErrorCode(error);
    if (errorCode === "DELETE_COMPENSATION_STALE") {
      state.pendingHostDeletes.splice(index, 1);
      persistConfig();
      addLog(`删除补偿已跳过(${item.displayName})：检测到宿主机已重新配对，避免误吊销新会话`);
      if (manual) {
        openHostNoticeModal(
          "删除任务已取消",
          `检测到宿主机“${item.displayName}”已重新配对，旧删除任务已自动取消。`,
        );
      }
      render();
      return;
    }
    if (errorCode === "DELETE_COMPENSATION_TERMINAL" || errorCode === "DELETE_COMPENSATION_NO_SESSION") {
      state.pendingHostDeletes.splice(index, 1);
      persistConfig();
      const deviceId = pendingDeleteDeviceId(item);
      try {
        await clearHostSession(item.systemId, deviceId);
      } catch (_) {
        // local session may already be absent; ignore cleanup errors.
      }
      addLog(`删除补偿终止(${item.displayName})：${String(error || "设备凭证不可用")}，已移出补偿队列`);
      if (manual) {
        openHostNoticeModal(
          "删除已完成本地收口",
          `宿主机“${item.displayName}”的凭证已失效或不可用，已从删除补偿队列移除。`,
        );
      }
      render();
      return;
    }

    item.retryCount = Number(item.retryCount || 0) + 1;
    item.nextRetryAt = Date.now() + DELETE_RETRY_INTERVAL_MS;
    item.lastError = String(error || "revoke failed");
    persistConfig();
    addLog(`删除补偿失败(${item.displayName}) #${item.retryCount}: ${item.lastError}`);
    if (manual) {
      openHostNoticeModal(
        "删除暂未完成",
        `Relay 暂不可达或鉴权失败：${item.lastError}。系统会继续自动补偿删除。`,
      );
    }
    renderHostManageModal();
  }
}

async function forceRemovePendingDelete(hostId, manual) {
  const index = state.pendingHostDeletes.findIndex((item) => item.hostId === hostId);
  if (index < 0) {
    return;
  }
  const item = state.pendingHostDeletes[index];
  const deviceId = pendingDeleteDeviceId(item);

  state.pendingHostDeletes.splice(index, 1);
  persistConfig();
  try {
    await clearHostSession(item.systemId, deviceId);
  } catch (_) {
    // local session may already be absent; ignore cleanup errors.
  }
  addLog(`删除补偿任务已强制移除: ${item.displayName}`);
  if (manual) {
    openHostNoticeModal("任务已移除", `已移除“${item.displayName}”的删除补偿任务，并清理本地会话。`);
  }
  render();
}

async function revokeAndClearPendingHost(item) {
  const deviceId = pendingDeleteDeviceId(item);
  const session = await tauriInvoke("auth_load_session", {
    systemId: item.systemId,
    deviceId,
  });
  if (!session) {
    throw errorWithCode("DELETE_COMPENSATION_NO_SESSION", "本地设备凭证不存在");
  }

  const currentCredentialId = String(session.credentialId || "").trim();
  const expectedCredentialId = String(item.expectedCredentialId || "").trim();
  const currentKeyId = String(session.keyId || "").trim();
  const expectedKeyId = String(item.expectedKeyId || "").trim();
  if (
    (expectedCredentialId && currentCredentialId && expectedCredentialId !== currentCredentialId) ||
    (expectedKeyId && currentKeyId && expectedKeyId !== currentKeyId)
  ) {
    const err = new Error("stale pending delete");
    err.code = "DELETE_COMPENSATION_STALE";
    throw err;
  }

  const hostRuntime = createRuntime();
  hostRuntime.accessToken = String(session.accessToken || "");
  hostRuntime.refreshToken = String(session.refreshToken || "");
  hostRuntime.keyId = String(session.keyId || "");
  hostRuntime.credentialId = String(session.credentialId || "");

  if (!hostRuntime.accessToken || !hostRuntime.refreshToken || !hostRuntime.keyId) {
    throw new Error("设备凭证不完整");
  }

  await refreshPendingSessionIfPossible(item, hostRuntime);

  const ts = String(Math.floor(Date.now() / 1000));
  const nonce = createEventId();
  const payload = `auth-revoke\n${item.systemId}\n${deviceId}\n${deviceId}\n${hostRuntime.keyId}\n${ts}\n${nonce}`;
  const signed = await tauriInvoke("auth_sign_payload", {
    deviceId,
    payload,
  });

  const { resp, body } = await relayRequestJson(item.relayUrl, "/auth/revoke-device", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      systemId: item.systemId,
      deviceId,
      targetDeviceId: deviceId,
      accessToken: hostRuntime.accessToken,
      keyId: String(signed.keyId || hostRuntime.keyId),
      ts,
      nonce,
      sig: String(signed.signature || ""),
    }),
  });
  if (!resp.ok || !body.ok) {
    const code = String(body && body.code ? body.code : "").trim();
    const message = String(body && body.message ? body.message : "吊销失败");
    if (DELETE_TERMINAL_RELAY_CODES.has(code)) {
      throw errorWithCode("DELETE_COMPENSATION_TERMINAL", `${code} ${message}`);
    }
    throw errorWithCode(code || "DELETE_COMPENSATION_RETRYABLE", `${code || resp.status} ${message}`);
  }

  await clearHostSession(item.systemId, deviceId);
}

async function refreshPendingSessionIfPossible(item, runtimeLike) {
  if (!runtimeLike.refreshToken || !runtimeLike.keyId) {
    return false;
  }

  const ts = String(Math.floor(Date.now() / 1000));
  const nonce = createEventId();
  const deviceId = pendingDeleteDeviceId(item);
  const payload = `auth-refresh\n${item.systemId}\n${deviceId}\n${runtimeLike.keyId}\n${ts}\n${nonce}`;
  const signed = await tauriInvoke("auth_sign_payload", {
    deviceId,
    payload,
  });

  const { resp, body } = await relayRequestJson(item.relayUrl, "/auth/refresh", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      systemId: item.systemId,
      deviceId,
      refreshToken: runtimeLike.refreshToken,
      keyId: String(signed.keyId || runtimeLike.keyId),
      ts,
      nonce,
      sig: String(signed.signature || ""),
    }),
  });

  if (!resp.ok || !body.ok) {
    const code = String(body && body.code ? body.code : "").trim();
    const message = String(body && body.message ? body.message : "refresh failed");
    if (DELETE_TERMINAL_RELAY_CODES.has(code)) {
      throw errorWithCode("DELETE_COMPENSATION_TERMINAL", `${code} ${message}`);
    }
    return false;
  }

  const data = asMap(body.data);
  runtimeLike.accessToken = String(data.accessToken || runtimeLike.accessToken);
  runtimeLike.refreshToken = String(data.refreshToken || runtimeLike.refreshToken);
  runtimeLike.keyId = String(data.keyId || runtimeLike.keyId);
  runtimeLike.credentialId = String(data.credentialId || runtimeLike.credentialId);

  await tauriInvoke("auth_store_session", {
    session: {
      systemId: item.systemId,
      deviceId,
      accessToken: runtimeLike.accessToken,
      refreshToken: runtimeLike.refreshToken,
      keyId: runtimeLike.keyId,
      credentialId: runtimeLike.credentialId,
    },
  });
  return true;
}

function pendingDeleteDeviceId(item) {
  return String((item && item.deviceId) || state.deviceId || "").trim();
}

function errorWithCode(code, message) {
  const err = new Error(String(message || "unexpected error"));
  err.code = String(code || "").trim();
  return err;
}

function normalizeDeleteCompensationErrorCode(error) {
  const directCode = String(error && error.code ? error.code : "").trim();
  if (directCode) {
    if (directCode === "DELETE_COMPENSATION_STALE") {
      return directCode;
    }
    if (directCode === "DELETE_COMPENSATION_TERMINAL") {
      return directCode;
    }
    if (directCode === "DELETE_COMPENSATION_NO_SESSION") {
      return directCode;
    }
  }
  const message = String(error || "");
  const token = message.match(/\b[A-Z][A-Z_]{2,}\b/);
  if (token && DELETE_TERMINAL_RELAY_CODES.has(token[0])) {
    return "DELETE_COMPENSATION_TERMINAL";
  }
  return directCode;
}

function formatGbFromMb(value) {
  const mb = Number(value);
  if (!Number.isFinite(mb)) {
    return "--";
  }
  return fmt2(mb / 1024);
}

function formatDurationShort(value) {
  const sec = Number(value);
  if (!Number.isFinite(sec) || sec < 0) {
    return "--";
  }
  const total = Math.floor(sec);
  const day = Math.floor(total / 86400);
  const hour = Math.floor((total % 86400) / 3600);
  const minute = Math.floor((total % 3600) / 60);
  if (day > 0) {
    return `${day}天 ${hour}小时`;
  }
  if (hour > 0) {
    return `${hour}小时 ${minute}分钟`;
  }
  return `${minute}分钟`;
}

function renderRows(rows) {
  return rows
    .map(
      ([key, value]) => `
        <div class="row">
          <div class="k">${escapeHtml(String(key))}</div>
          <div class="v">${escapeHtml(String(value ?? "--"))}</div>
        </div>
      `,
    )
    .join("");
}

function localizedCategory(rawValue) {
  const raw = String(rawValue || "");
  if (raw === "CODE_AGENT") {
    return "代码助手";
  }
  if (raw === "DEV_WORKER") {
    return "开发工具";
  }
  if (raw === "UNKNOWN") {
    return "未知";
  }
  return raw || "--";
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

init();
