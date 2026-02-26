// 文件职责：
// 1. 管理按 hostId 隔离的运行时对象与工具元数据状态。
// 2. 提供工具别名/隐藏、快照过滤与宿主机状态判定。
// 3. 提供连接相关计时器清理，避免跨重连遗留脏状态。

import { createRuntime, state } from "./store.js";
import { asBool } from "../utils/type.js";

export const OPENCLAW_LOGICAL_TOOL_ID = "openclaw_primary";

function isOpenClawToolId(toolId) {
  return /^openclaw_/i.test(String(toolId || "").trim());
}

function isOpenClawTool(tool) {
  const toolId = String(tool?.toolId || "").toLowerCase();
  const name = String(tool?.name || "").toLowerCase();
  const vendor = String(tool?.vendor || "").toLowerCase();
  return toolId.startsWith("openclaw_") || name.includes("openclaw") || vendor.includes("openclaw");
}

function isOpenCodeTool(tool) {
  const toolId = String(tool?.toolId || "").toLowerCase();
  const name = String(tool?.name || "").toLowerCase();
  const vendor = String(tool?.vendor || "").toLowerCase();
  return toolId.startsWith("opencode_") || name.includes("opencode") || vendor.includes("opencode");
}

function parsePidFromToolId(toolId) {
  const text = String(toolId || "").trim();
  const match = text.match(/_p(\d+)$/i);
  if (!match) return 0;
  const pid = Number(match[1] || 0);
  return Number.isFinite(pid) && pid > 0 ? pid : 0;
}

function parsePidFromTool(tool) {
  const explicit = Number(tool?.pid || 0);
  if (Number.isFinite(explicit) && explicit > 0) {
    return explicit;
  }
  return parsePidFromToolId(tool?.runtimeToolId || tool?.toolId || "");
}

function opencodeFamilyKey(toolId) {
  const normalized = String(toolId || "").trim().toLowerCase();
  const match = normalized.match(/^(opencode_[a-z0-9]+)_p\d+$/);
  if (match && match[1]) return match[1];
  if (normalized.startsWith("opencode_")) return normalized;
  return "";
}

function openclawSelectionScore(tool) {
  const status = String(tool?.status || "").trim().toLowerCase();
  const source = String(tool?.source || "").trim().toLowerCase();
  let score = 0;
  if (asBool(tool?.connected)) score += 2;
  if (status && status !== "offline") score += 4;
  if (source === "whitelist-placeholder") score -= 3;
  if (parsePidFromTool(tool) > 0) score += 1;
  return score;
}

/** 创建运行态管理器（按 hostId 维护连接与工具状态）。 */
export function createRuntimeState({ persistConfig }) {
  function ensureRuntimeShape(runtime) {
    if (!runtime || typeof runtime !== "object") return;
    if (!runtime.logicalToolIdToRuntimeToolId || typeof runtime.logicalToolIdToRuntimeToolId !== "object") {
      runtime.logicalToolIdToRuntimeToolId = {};
    }
    if (!runtime.runtimeToolIdToLogicalToolId || typeof runtime.runtimeToolIdToLogicalToolId !== "object") {
      runtime.runtimeToolIdToLogicalToolId = {};
    }
    if (!runtime.toolCapabilityChangesByToolId || typeof runtime.toolCapabilityChangesByToolId !== "object") {
      runtime.toolCapabilityChangesByToolId = {};
    }
    if (!runtime.opencodeInvalidByLogicalToolId || typeof runtime.opencodeInvalidByLogicalToolId !== "object") {
      runtime.opencodeInvalidByLogicalToolId = {};
    }
    if (!runtime.openclawWorkspaceByToolId || typeof runtime.openclawWorkspaceByToolId !== "object") {
      runtime.openclawWorkspaceByToolId = {};
    }
    if (!Number.isFinite(Number(runtime.toolDetailsLastSnapshotId || 0))) {
      runtime.toolDetailsLastSnapshotId = 0;
    }
    if (typeof runtime.toolDetailsLastRefreshId !== "string") {
      runtime.toolDetailsLastRefreshId = "";
    }
    if (typeof runtime.toolDetailsLastTrigger !== "string") {
      runtime.toolDetailsLastTrigger = "";
    }
    if (typeof runtime.toolDetailsPendingAllRefreshId !== "string") {
      runtime.toolDetailsPendingAllRefreshId = "";
    }
    if (!runtime.toolDetailsPendingRefreshByToolId || typeof runtime.toolDetailsPendingRefreshByToolId !== "object") {
      runtime.toolDetailsPendingRefreshByToolId = {};
    }
  }

  function ensureRuntime(hostId) {
    const id = String(hostId || "").trim();
    if (!id) {
      return null;
    }
    if (!state.runtimes[id]) {
      state.runtimes[id] = createRuntime();
    }
    ensureRuntimeShape(state.runtimes[id]);
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
    if (runtime.candidateRefreshTimer) {
      clearTimeout(runtime.candidateRefreshTimer);
      runtime.candidateRefreshTimer = null;
    }
    runtime.connectionEpoch += 1;
    clearReconnectTimer(runtime);
    if (runtime.socket) {
      runtime.socket.close();
      runtime.socket = null;
    }
    delete state.runtimes[hostId];
  }

  function resolveLogicalToolId(hostId, toolId) {
    const normalized = String(toolId || "").trim();
    if (!normalized) return "";
    if (normalized === OPENCLAW_LOGICAL_TOOL_ID) return normalized;
    const runtime = ensureRuntime(hostId);
    if (runtime && runtime.runtimeToolIdToLogicalToolId[normalized]) {
      return String(runtime.runtimeToolIdToLogicalToolId[normalized] || "").trim() || normalized;
    }
    if (isOpenClawToolId(normalized)) {
      return OPENCLAW_LOGICAL_TOOL_ID;
    }
    return normalized;
  }

  function resolveRuntimeToolId(hostId, logicalOrRuntimeToolId) {
    const normalized = String(logicalOrRuntimeToolId || "").trim();
    if (!normalized) return "";
    const runtime = ensureRuntime(hostId);
    if (!runtime) return normalized;
    if (runtime.logicalToolIdToRuntimeToolId[normalized]) {
      return String(runtime.logicalToolIdToRuntimeToolId[normalized] || "").trim() || normalized;
    }
    if (normalized === OPENCLAW_LOGICAL_TOOL_ID) {
      const fallback = (Array.isArray(runtime.tools) ? runtime.tools : []).find((tool) => isOpenClawTool(tool));
      if (!fallback) return normalized;
      return String(fallback.runtimeToolId || fallback.toolId || normalized).trim() || normalized;
    }
    return normalized;
  }

  function toolStateKey(hostId, toolId) {
    const host = String(hostId || "").trim();
    const tool = resolveLogicalToolId(hostId, toolId);
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

  function clearToolBinding(hostId, toolId) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) return;
    const logicalToolId = resolveLogicalToolId(hostId, toolId);
    if (!logicalToolId) return;
    const runtimeToolId = String(runtime.logicalToolIdToRuntimeToolId[logicalToolId] || "").trim();
    if (runtimeToolId) {
      delete runtime.runtimeToolIdToLogicalToolId[runtimeToolId];
    }
    delete runtime.logicalToolIdToRuntimeToolId[logicalToolId];
    delete runtime.opencodeInvalidByLogicalToolId[logicalToolId];
    delete runtime.toolCapabilityChangesByToolId[logicalToolId];
    delete runtime.openclawWorkspaceByToolId[logicalToolId];
  }

  function resolveToolDisplayName(hostId, tool) {
    const toolId = resolveLogicalToolId(hostId, String(tool && tool.toolId ? tool.toolId : ""));
    const alias = getToolAlias(hostId, toolId);
    if (alias) {
      return alias;
    }
    const rawName = String((tool && tool.name) || "").trim();
    return rawName || "Unknown Tool";
  }

  function sanitizeTools(hostId, source, includeHidden) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) return [];

    const hasOpenclawCard = Array.isArray(runtime.tools)
      && runtime.tools.some((tool) => String(tool.toolId || "") === OPENCLAW_LOGICAL_TOOL_ID || isOpenClawTool(tool));
    const connectedOpencodePids = new Set(
      (Array.isArray(runtime.tools) ? runtime.tools : [])
        .filter((tool) => isOpenCodeTool(tool))
        .map((tool) => {
          const runtimeToolId = resolveRuntimeToolId(hostId, String(tool.toolId || ""));
          const pid = parsePidFromTool({ ...tool, runtimeToolId });
          return pid > 0 ? pid : 0;
        })
        .filter((pid) => pid > 0),
    );

    const filtered = source.filter((tool) => {
      const name = String(tool.name || "").toLowerCase();
      const category = String(tool.category || "").toLowerCase();
      const toolId = String(tool.toolId || "").trim();
      const logicalToolId = isOpenClawTool(tool) ? OPENCLAW_LOGICAL_TOOL_ID : toolId;
      if (name.includes("mobile") || name.includes("client app")) {
        return false;
      }
      if (category.includes("client")) {
        return false;
      }
      if (!includeHidden && logicalToolId && isToolHidden(hostId, logicalToolId)) {
        return false;
      }
      if (includeHidden && isOpenClawTool(tool) && hasOpenclawCard) {
        return false;
      }
      if (includeHidden && isOpenCodeTool(tool)) {
        const pid = parsePidFromTool(tool);
        if (pid > 0 && connectedOpencodePids.has(pid)) {
          return false;
        }
      }
      return true;
    });

    if (includeHidden) {
      return filtered;
    }

    runtime.logicalToolIdToRuntimeToolId = {};
    runtime.runtimeToolIdToLogicalToolId = {};

    let selectedOpenclaw = null;
    const nonOpenclaw = [];
    for (const tool of filtered) {
      if (!isOpenClawTool(tool)) {
        nonOpenclaw.push(tool);
        continue;
      }
      if (!selectedOpenclaw || openclawSelectionScore(tool) > openclawSelectionScore(selectedOpenclaw)) {
        selectedOpenclaw = tool;
      }
    }

    const connectedList = [];
    if (selectedOpenclaw) connectedList.push(selectedOpenclaw);
    connectedList.push(...nonOpenclaw);

    return connectedList.map((tool) => {
      const rawToolId = String(tool.toolId || "").trim();
      const logicalToolId = isOpenClawTool(tool) ? OPENCLAW_LOGICAL_TOOL_ID : rawToolId;
      if (!rawToolId) {
        return { ...tool };
      }

      runtime.runtimeToolIdToLogicalToolId[rawToolId] = logicalToolId;
      runtime.logicalToolIdToRuntimeToolId[logicalToolId] = rawToolId;

      const nextTool = {
        ...tool,
        toolId: logicalToolId,
        logicalToolId,
        runtimeToolId: rawToolId,
      };

      if (logicalToolId === OPENCLAW_LOGICAL_TOOL_ID) {
        const workspace = String(tool.workspaceDir || "").trim();
        if (workspace && !runtime.openclawWorkspaceByToolId[logicalToolId]) {
          runtime.openclawWorkspaceByToolId[logicalToolId] = workspace;
        }
        if (runtime.openclawWorkspaceByToolId[logicalToolId]) {
          nextTool.workspaceDir = runtime.openclawWorkspaceByToolId[logicalToolId];
        }
      }

      if (isOpenCodeTool(nextTool) && runtime.opencodeInvalidByLogicalToolId[logicalToolId]) {
        nextTool.invalidPidChanged = true;
        nextTool.status = "INVALID";
        nextTool.reason = "检测到进程 PID 变化，请删除卡片后重新接入新进程。";
      } else {
        nextTool.invalidPidChanged = false;
      }

      return nextTool;
    });
  }

  function syncOpencodeInvalidState(hostId) {
    const runtime = ensureRuntime(hostId);
    if (!runtime) return;

    const candidateTools = Array.isArray(runtime.candidateTools) ? runtime.candidateTools : [];
    const nextInvalid = {};
    for (const tool of Array.isArray(runtime.tools) ? runtime.tools : []) {
      if (!isOpenCodeTool(tool)) continue;
      const logicalToolId = String(tool.toolId || "").trim();
      const runtimeToolId = resolveRuntimeToolId(hostId, logicalToolId);
      const connectedFamily = opencodeFamilyKey(runtimeToolId);
      if (!connectedFamily) continue;

      const replaced = candidateTools.some((candidate) => {
        if (!isOpenCodeTool(candidate)) return false;
        const candidateToolId = String(candidate.toolId || "").trim();
        return candidateToolId
          && candidateToolId !== runtimeToolId
          && opencodeFamilyKey(candidateToolId) === connectedFamily;
      });
      if (replaced) {
        nextInvalid[logicalToolId] = true;
      }
    }

    runtime.opencodeInvalidByLogicalToolId = nextInvalid;
    runtime.tools = (Array.isArray(runtime.tools) ? runtime.tools : []).map((tool) => {
      const logicalToolId = String(tool.toolId || "").trim();
      if (!isOpenCodeTool(tool)) {
        return { ...tool, invalidPidChanged: false };
      }
      if (!nextInvalid[logicalToolId]) {
        return { ...tool, invalidPidChanged: false };
      }
      return {
        ...tool,
        invalidPidChanged: true,
        status: "INVALID",
        reason: "检测到进程 PID 变化，请删除卡片后重新接入新进程。",
      };
    });
  }

  function metricForTool(hostId, toolId) {
    const runtime = ensureRuntime(hostId);
    if (!runtime || !toolId) {
      return {};
    }
    const logicalToolId = resolveLogicalToolId(hostId, toolId);
    const runtimeToolId = resolveRuntimeToolId(hostId, logicalToolId);
    if (runtime.toolMetricsById[runtimeToolId]) {
      return runtime.toolMetricsById[runtimeToolId];
    }
    if (String(runtime.primaryToolMetrics.toolId || "") === runtimeToolId) {
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
    const logicalToolId = resolveLogicalToolId(hostId, toolId);
    const runtimeToolId = resolveRuntimeToolId(hostId, logicalToolId);
    const detail = runtime.toolDetailsById[runtimeToolId] || {};
    return {
      schema: String(detail.schema || ""),
      stale: asBool(runtime.toolDetailStaleById[runtimeToolId]),
      collectedAt: String(runtime.toolDetailUpdatedAtById[runtimeToolId] || ""),
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
    clearToolBinding,
    resolveLogicalToolId,
    resolveRuntimeToolId,
    resolveToolDisplayName,
    sanitizeTools,
    syncOpencodeInvalidState,
    metricForTool,
    detailForTool,
    hostStatusLabel,
  };
}
