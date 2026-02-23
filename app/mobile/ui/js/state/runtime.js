// 文件职责：
// 1. 管理按 hostId 隔离的运行时对象与工具元数据状态。
// 2. 提供工具别名/隐藏、快照过滤与宿主机状态判定。
// 3. 提供连接相关计时器清理，避免跨重连遗留脏状态。

import { createRuntime, state } from "./store.js";
import { asBool } from "../utils/type.js";

/** 创建运行态管理器（按 hostId 维护连接与工具状态）。 */
export function createRuntimeState({ persistConfig }) {
  function ensureRuntime(hostId) {
    const id = String(hostId || "").trim();
    if (!id) {
      return null;
    }
    if (!state.runtimes[id]) {
      state.runtimes[id] = createRuntime();
    }
    return state.runtimes[id];
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

  function disposeRuntime(hostId) {
    const runtime = state.runtimes[hostId];
    if (!runtime) {
      return;
    }
    runtime.connectionEpoch += 1;
    clearReconnectTimer(runtime);
    if (runtime.socket) {
      runtime.socket.close();
      runtime.socket = null;
    }
    delete state.runtimes[hostId];
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
    return rawName || "Unknown Tool";
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

  /**
   * 读取工具详情缓存（由 `tool_details_snapshot` 下行驱动）。
   * @param {string} hostId 宿主机标识。
   * @param {string} toolId 工具标识。
   * @returns {{schema: string, stale: boolean, collectedAt: string, expiresAt: string, profileKey: string, data: object}}
   */
  function detailForTool(hostId, toolId) {
    const runtime = ensureRuntime(hostId);
    if (!runtime || !toolId) {
      return {
        schema: "",
        stale: false,
        collectedAt: "",
        expiresAt: "",
        profileKey: "",
        data: {},
      };
    }
    const detail = runtime.toolDetailsById[toolId] || {};
    return {
      schema: String(detail.schema || ""),
      stale: asBool(runtime.toolDetailStaleById[toolId]),
      collectedAt: String(runtime.toolDetailUpdatedAtById[toolId] || ""),
      expiresAt: String(detail.expiresAt || ""),
      profileKey: String(detail.profileKey || ""),
      data: detail.data && typeof detail.data === "object" ? detail.data : {},
    };
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

  return {
    ensureRuntime,
    disposeRuntime,
    clearReconnectTimer,
    clearToolConnectTimer,
    getToolAlias,
    setToolAlias,
    isToolHidden,
    setToolHidden,
    clearToolMetaForHost,
    resolveToolDisplayName,
    sanitizeTools,
    metricForTool,
    detailForTool,
    hostStatusLabel,
  };
}
