// 文件职责：
// 1. 管理宿主机配置与本地持久化（hosts/pending deletes/tool meta 基础字段）。
// 2. 维护设备标识、宿主机列表访问与选择状态重算。
// 3. 承载旧版本 localStorage 数据迁移逻辑。

import {
  STORAGE_KEY,
  LEGACY_STORAGE_KEY,
  state,
} from "./store.js";

/** 创建宿主机配置状态管理器。 */
export function createHostState() {
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

  function visibleHosts() {
    return [...state.hosts]
      .sort((a, b) => new Date(a.pairedAt).getTime() - new Date(b.pairedAt).getTime());
  }

  function hostById(hostId) {
    const id = String(hostId || "").trim();
    if (!id) {
      return null;
    }
    return state.hosts.find((host) => host.hostId === id) || null;
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

  function normalizeOperationLogs(rawValue) {
    if (!Array.isArray(rawValue)) {
      return [];
    }
    const out = [];
    for (const item of rawValue) {
      if (!item || typeof item !== "object") {
        continue;
      }
      const record = {
        ts: String(item.ts || ""),
        level: String(item.level || "info"),
        scope: String(item.scope || "app"),
        action: String(item.action || ""),
        outcome: String(item.outcome || "info"),
        source: String(item.source || "mobile"),
        direction: String(item.direction || ""),
        traceId: String(item.traceId || ""),
        eventId: String(item.eventId || ""),
        eventType: String(item.eventType || ""),
        hostId: String(item.hostId || ""),
        hostName: String(item.hostName || ""),
        toolId: String(item.toolId || ""),
        systemId: String(item.systemId || ""),
        sourceClientType: String(item.sourceClientType || ""),
        sourceDeviceId: String(item.sourceDeviceId || ""),
        seq: Number(item.seq || 0),
        message: String(item.message || ""),
        detail: String(item.detail || ""),
      };
      out.push(record);
      if (out.length >= 500) {
        break;
      }
    }
    return out;
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

    // Banner 索引必须落在现有宿主机区间内，避免删除后越界。
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
      operationLogs: state.operationLogs.slice(0, 500),
      message: state.message,
    };
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
    } catch (error) {
      // localStorage 写入失败不应阻断主流程（例如 iOS 存储策略限制）。
      console.warn("[storage] persist config failed", error);
    }
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
        operationLogs: [],
        message: String(legacy.message || "tool_ping"),
      };
    } catch (_) {
      return null;
    }
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
    state.operationLogs = normalizeOperationLogs(parsed.operationLogs);

    recomputeSelections();
  }

  function ensureIdentity() {
    if (state.deviceId) {
      return;
    }
    state.deviceId = createDeviceId();
    persistConfig();
  }

  return {
    createEventId,
    visibleHosts,
    hostById,
    recomputeSelections,
    restoreConfig,
    persistConfig,
    ensureIdentity,
  };
}
