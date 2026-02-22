// 文件职责：
// 1. 管理宿主机删除补偿队列（入队/重试/终止/强制移除）。
// 2. 在 UI 层提供“立即隐藏 + 最终一致性补偿”删除语义。

import { DELETE_RETRY_INTERVAL_MS, state } from "../state/store.js";
import {
  pendingDeleteDeviceId,
  normalizeDeleteCompensationErrorCode,
} from "./host-delete-errors.js";
import { createHostDeleteRemote } from "./host-delete-remote.js";

/** 创建宿主机删除补偿流程。 */
export function createHostDeleteFlow({
  hostById,
  disposeRuntime,
  clearToolMetaForHost,
  recomputeSelections,
  persistConfig,
  render,
  createEventId,
  tauriInvoke,
  clearHostSession,
  addLog,
  openHostNoticeModal,
}) {
  const remote = createHostDeleteRemote({
    createEventId,
    tauriInvoke,
    clearHostSession,
  });

  /**
   * 发起删除：先隐藏宿主机，再进入补偿队列。
   * @param {string} hostId 宿主机标识。
   */
  async function deleteHostWithCompensation(hostId) {
    const host = hostById(hostId);
    if (!host) return;

    let expectedCredentialId = "";
    let expectedKeyId = "";
    try {
      const session = await tauriInvoke("auth_load_session", { systemId: host.systemId, deviceId: state.deviceId });
      if (session) {
        expectedCredentialId = String(session.credentialId || "").trim();
        expectedKeyId = String(session.keyId || "").trim();
      }
    } catch (error) {
      addLog(`delete preload session failed (${host.displayName}): ${error}`);
    }

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

  /**
   * 处理已到期的删除补偿任务。
   */
  async function processPendingDeletes() {
    if (state.deleteCompensating || state.pendingHostDeletes.length === 0) return;
    state.deleteCompensating = true;
    try {
      const now = Date.now();
      const due = state.pendingHostDeletes.filter((item) => Number(item.nextRetryAt || 0) <= now);
      for (const item of due) await retryPendingDelete(item.hostId, false);
    } finally {
      state.deleteCompensating = false;
    }
  }

  /**
   * 重试单条删除补偿任务。
   * @param {string} hostId 宿主机标识。
   * @param {boolean} manual 是否手动触发。
   */
  async function retryPendingDelete(hostId, manual) {
    const index = state.pendingHostDeletes.findIndex((item) => item.hostId === hostId);
    if (index < 0) return;
    const item = state.pendingHostDeletes[index];

    try {
      await remote.revokeAndClearPendingHost(item);
      state.pendingHostDeletes.splice(index, 1);
      persistConfig();
      addLog(`删除补偿完成: ${item.displayName}`);
      if (manual) openHostNoticeModal("删除完成", `宿主机“${item.displayName}”已完成最终删除。`);
      render();
    } catch (error) {
      const code = normalizeDeleteCompensationErrorCode(error);
      if (code === "DELETE_COMPENSATION_STALE") {
        state.pendingHostDeletes.splice(index, 1);
        persistConfig();
        addLog(`删除补偿已跳过(${item.displayName})：检测到宿主机已重新配对，避免误吊销新会话`);
        if (manual) openHostNoticeModal("删除任务已取消", `检测到宿主机“${item.displayName}”已重新配对，旧删除任务已自动取消。`);
        render();
        return;
      }

      if (code === "DELETE_COMPENSATION_TERMINAL" || code === "DELETE_COMPENSATION_NO_SESSION") {
        state.pendingHostDeletes.splice(index, 1);
        persistConfig();
        const deviceId = pendingDeleteDeviceId(item);
        try {
          await clearHostSession(item.systemId, deviceId);
        } catch (_) {
          // 本地会话可能已经不存在。
        }
        addLog(
          `删除补偿终止(${item.displayName})：${String(error || "设备凭证不可用")}，已移出补偿队列`,
        );
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
    }
  }

  /**
   * 强制移除补偿任务，仅保留本地收口。
   * @param {string} hostId 宿主机标识。
   * @param {boolean} manual 是否手动触发。
   */
  async function forceRemovePendingDelete(hostId, manual) {
    const index = state.pendingHostDeletes.findIndex((item) => item.hostId === hostId);
    if (index < 0) return;
    const item = state.pendingHostDeletes[index];
    const deviceId = pendingDeleteDeviceId(item);

    state.pendingHostDeletes.splice(index, 1);
    persistConfig();
    try {
      await clearHostSession(item.systemId, deviceId);
    } catch (_) {
      // 本地会话可能已经不存在。
    }
    addLog(`删除补偿任务已强制移除: ${item.displayName}`);
    if (manual) openHostNoticeModal("任务已移除", `已移除“${item.displayName}”的删除补偿任务，并清理本地会话。`);
    render();
  }

  return {
    deleteHostWithCompensation,
    processPendingDeletes,
    retryPendingDelete,
    forceRemovePendingDelete,
  };
}
