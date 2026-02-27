// 文件职责：
// 1. 管理宿主机管理弹窗、编辑弹窗、负载弹窗。
// 2. 协调删除补偿子流程与重名提示。

import { asMap } from "../utils/type.js";
import { formatDurationShort, formatGbFromMb, relayGatewayHint } from "../utils/host-format.js";
import { renderRows } from "../utils/rows.js";
import { fmt2 } from "../utils/format.js";
import { escapeHtml } from "../utils/dom.js";
import { createHostDeleteFlow } from "./host-delete.js";

/**
 * 创建宿主管理流程（管理列表/编辑/删除补偿/负载弹窗）。
 * @param {object} deps 依赖集合。
 */
export function createHostManageFlow({
  state,
  ui,
  visibleHosts,
  hostById,
  ensureRuntime,
  hostStatusLabel,
  recomputeSelections,
  persistConfig,
  createEventId,
  tauriInvoke,
  disposeRuntime,
  clearToolMetaForHost,
  deleteChatConversationsByHost,
  clearHostSession,
  addLog,
  openHostNoticeModal,
  connectHost,
  reconnectHost,
  disconnectHost,
  sendSocketEvent,
  openPairFlow,
  render,
}) {
  // 删除动作统一走“先隐藏 UI，再后台补偿”的一致性流程。
  const deleteFlow = createHostDeleteFlow({
    hostById,
    disposeRuntime,
    clearToolMetaForHost,
    deleteChatConversationsByHost,
    recomputeSelections,
    persistConfig,
    render,
    createEventId,
    tauriInvoke,
    clearHostSession,
    addLog,
    openHostNoticeModal,
    ensureRuntime,
    sendSocketEvent,
  });

  function openHostManageModal() {
    ui.hostManageModal.classList.add("show");
    renderHostManageModal();
  }

  function closeHostManageModal() {
    ui.hostManageModal.classList.remove("show");
  }

  function renderHostManageModal() {
    if (!ui.hostManageModal.classList.contains("show")) return;

    const hosts = visibleHosts();
    if (hosts.length === 0) {
      ui.hostManageList.innerHTML = '<div class="empty">暂无已配对宿主机。</div>';
    } else {
      ui.hostManageList.innerHTML = hosts
        .map((host) => {
          const runtime = ensureRuntime(host.hostId);
          const note = host.note ? ` · 备注: ${host.note}` : "";
          const connectLabel = runtime && runtime.connected
            ? "重连"
            : runtime && runtime.connecting
              ? "连接中"
              : "连接";
          return `
            <article class="host-manage-item">
              <div class="host-manage-name">${escapeHtml(host.displayName)}</div>
              <div class="host-manage-sub">
                状态: ${escapeHtml(hostStatusLabel(host.hostId))}${escapeHtml(note)}
              </div>
              <div class="host-manage-sub host-manage-relay">${escapeHtml(relayGatewayHint(host.relayUrl))}</div>
              <div class="host-manage-actions">
                <button class="btn btn-primary btn-sm" data-manage-connect="${escapeHtml(host.hostId)}">
                  ${escapeHtml(connectLabel)}
                </button>
                <button class="btn btn-outline btn-sm" data-manage-disconnect="${escapeHtml(host.hostId)}">
                  断开
                </button>
                <button class="btn btn-outline btn-sm" data-manage-edit="${escapeHtml(host.hostId)}">
                  编辑
                </button>
                <button class="btn btn-outline btn-sm" data-manage-repair="${escapeHtml(host.hostId)}">
                  重新配对
                </button>
                <button class="btn btn-outline btn-sm" data-manage-delete="${escapeHtml(host.hostId)}">
                  删除
                </button>
              </div>
            </article>
          `;
        })
        .join("");
    }

    ui.pendingDeleteList.innerHTML = state.pendingHostDeletes.length === 0
      ? '<div class="empty">当前无删除补偿任务。</div>'
      : state.pendingHostDeletes.map((item) => {
          const retryAt = new Date(Number(item.nextRetryAt || 0));
          const retryAtText = Number.isFinite(retryAt.getTime())
            ? retryAt.toLocaleString()
            : "--";
          return `
            <article class="host-manage-item">
              <div class="host-manage-name">${escapeHtml(item.displayName || item.systemId)}</div>
              <div class="host-manage-sub">
                删除处理中 · 重试 ${escapeHtml(String(item.retryCount || 0))} 次 ·
                下次: ${escapeHtml(retryAtText)}
              </div>
              <div class="host-manage-sub">最近错误: ${escapeHtml(item.lastError || "--")}</div>
              <div class="host-manage-actions">
                <button class="btn btn-outline btn-sm" data-pending-retry="${escapeHtml(item.hostId)}">立即重试删除</button>
                <button class="btn btn-outline btn-sm" data-pending-force-remove="${escapeHtml(item.hostId)}">强制移除任务</button>
              </div>
            </article>
          `;
        }).join("");
  }

  function openHostEditModal(hostId) {
    const host = hostById(hostId);
    if (!host) return;
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
    host.displayName = String(ui.hostEditNameInput.value || "").trim() || host.systemId;
    host.note = String(ui.hostEditNoteInput.value || "").trim();
    host.updatedAt = new Date().toISOString();
    persistConfig();
    closeHostEditModal();
    notifyIfDuplicateDisplayName(host.hostId);
    render();
  }

  function openHostMetricsModal(hostId) {
    if (!hostId) return;
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

    const systemRows = hasSystemMetrics
      ? [
        ["状态", hostStatusLabel(host.hostId)],
        ["CPU", `${fmt2(system.cpuPercent)}%`],
        [
          "内存",
          `${formatGbFromMb(system.memoryUsedMb)} / ${formatGbFromMb(system.memoryTotalMb)} GB (${fmt2(system.memoryUsedPercent)}%)`,
        ],
        [
          "磁盘",
          `${fmt2(system.diskUsedGb)} / ${fmt2(system.diskTotalGb)} GB (${fmt2(system.diskUsedPercent)}%)`,
        ],
        ["运行时长", formatDurationShort(system.uptimeSec)],
        [
          "最近心跳",
          runtime.lastHeartbeatAt ? runtime.lastHeartbeatAt.toLocaleString() : "--",
        ],
      ]
      : [
        ["状态", hostStatusLabel(host.hostId)],
        ["指标", "尚未收到系统负载快照，请等待 sidecar 下一次上报。"],
      ];

    const sidecarRows = hasSidecarMetrics
      ? [["CPU", `${fmt2(sidecar.cpuPercent)}%`], ["内存", `${fmt2(sidecar.memoryMb)} MB`]]
      : [["状态", "尚未收到 sidecar 负载快照。"]];

    ui.hostMetricsTitle.textContent = `${host.displayName} · 宿主机负载`;
    ui.hostMetricsRows.innerHTML = renderRows(systemRows);
    ui.hostSidecarRows.innerHTML = renderRows(sidecarRows);
    ui.hostMetricsTip.textContent = hasSystemMetrics
      ? "数据来源于 metrics_snapshot（实时刷新）。"
      : "提示：若持续没有数据，请确认 relay 与 sidecar 均在线。";
    ui.hostMetricsModal.classList.add("show");
  }

  function shortSystemId(systemId) {
    const raw = String(systemId || "").trim();
    if (!raw) return "host";
    return raw.length <= 8 ? raw : raw.slice(0, 8);
  }

  function notifyIfDuplicateDisplayName(hostId) {
    const host = hostById(hostId);
    if (!host) return;
    const sameNameHosts = visibleHosts().filter((item) => item.displayName === host.displayName);
    if (sameNameHosts.length <= 1) return;
    const suggested = `${host.displayName}-${shortSystemId(host.systemId)}`;
    // 重名只影响展示识别，不影响真实连接与唯一标识。
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
      return void disconnectHost(
        String(disconnectBtn.getAttribute("data-manage-disconnect") || ""),
        { triggerReconnect: false },
      );
    }
    const editBtn = event.target.closest("[data-manage-edit]");
    if (editBtn) {
      return void openHostEditModal(String(editBtn.getAttribute("data-manage-edit") || ""));
    }
    const repairBtn = event.target.closest("[data-manage-repair]");
    if (repairBtn) {
      closeHostManageModal();
      return void openPairFlow("import", String(repairBtn.getAttribute("data-manage-repair") || ""));
    }
    const deleteBtn = event.target.closest("[data-manage-delete]");
    if (deleteBtn) {
      return void deleteFlow.deleteHostWithCompensation(
        String(deleteBtn.getAttribute("data-manage-delete") || ""),
      );
    }
  }

  function onPendingDeleteListClick(event) {
    const retryBtn = event.target.closest("[data-pending-retry]");
    if (retryBtn) {
      return void deleteFlow.retryPendingDelete(
        String(retryBtn.getAttribute("data-pending-retry") || ""),
        true,
      );
    }
    const forceBtn = event.target.closest("[data-pending-force-remove]");
    if (forceBtn) {
      void deleteFlow.forceRemovePendingDelete(
        String(forceBtn.getAttribute("data-pending-force-remove") || ""),
        true,
      );
    }
  }

  return {
    openHostManageModal,
    closeHostManageModal,
    renderHostManageModal,
    openHostEditModal,
    closeHostEditModal,
    saveHostEdit,
    openHostMetricsModal,
    closeHostMetricsModal,
    renderHostMetricsModal,
    notifyIfDuplicateDisplayName,
    onHostManageListClick,
    onPendingDeleteListClick,
    processPendingDeletes: deleteFlow.processPendingDeletes,
  };
}
