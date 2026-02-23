// 文件职责：
// 1. 管理工具详情弹窗的打开/关闭、摘要与详情渲染。
// 2. 为 OpenClaw 提供专属多屏详情结构（概览/Agents/Sessions/Usage/系统与服务）。
// 3. 保持 OpenCode 详情体验不退化，并在打开弹窗时触发按需刷新。

import { asMap, asListOfMap, asBool } from "../utils/type.js";
import { fmt2, fmtInt, fmtTokenM, usageSummary } from "../utils/format.js";
import { localizedCategory } from "../utils/host-format.js";
import { renderRows } from "../utils/rows.js";
import { escapeHtml } from "../utils/dom.js";

/**
 * 创建工具详情弹窗能力。
 * @param {object} deps 依赖集合。
 * @returns {{openToolDetail: Function, closeToolDetail: Function, renderToolModal: Function, bindToolDetailModalEvents: Function}}
 */
export function createToolDetailModal({
  state,
  ui,
  hostById,
  ensureRuntime,
  metricForTool,
  detailForTool,
  resolveToolDisplayName,
  requestToolDetailsRefresh,
}) {
  /**
   * 打开工具详情弹窗并主动触发详情刷新。
   * @param {string} hostId 宿主机标识。
   * @param {string} toolId 工具标识。
   */
  function openToolDetail(hostId, toolId) {
    if (!hostId || !toolId) {
      return;
    }
    state.detailHostId = hostId;
    state.detailToolId = toolId;
    state.detailExpanded = false;
    state.detailOpenClawPageIndex = 0;
    state.detailOpenClawSessionsSection = "diagnostics";
    state.detailOpenClawUsageWindowPreset = "1h";
    state.detailOpenClawAgentOpenIds = {};
    state.detailOpenClawSecurityExpanded = false;
    if (typeof requestToolDetailsRefresh === "function") {
      requestToolDetailsRefresh(hostId, toolId, true);
    }
    renderToolModal();
  }

  /** 关闭工具详情弹窗并清空上下文。 */
  function closeToolDetail() {
    state.detailHostId = "";
    state.detailToolId = "";
    state.detailExpanded = false;
    state.detailOpenClawPageIndex = 0;
    state.detailOpenClawSessionsSection = "diagnostics";
    state.detailOpenClawUsageWindowPreset = "1h";
    state.detailOpenClawAgentOpenIds = {};
    state.detailOpenClawSecurityExpanded = false;
    renderToolModal();
  }

  /**
   * 从 metric/tool 双来源读取字段，metric 优先。
   * @param {Record<string, any>} tool 工具对象。
   * @param {Record<string, any>} metric 指标对象。
   * @param {string} key 字段名。
   * @returns {string}
   */
  function pickMetric(tool, metric, key) {
    const value = metric[key] ?? tool[key];
    return value == null ? "" : String(value);
  }

  /**
   * 将时间戳归一化为本地可读时间。
   * @param {unknown} raw 原始时间值（支持毫秒时间戳、秒时间戳、ISO 字符串）。
   * @returns {string}
   */
  function formatTime(raw) {
    if (raw == null || raw === "") {
      return "--";
    }
    const asNumber = Number(raw);
    if (Number.isFinite(asNumber) && asNumber > 0) {
      const millis = asNumber > 1_000_000_000_000 ? asNumber : asNumber * 1000;
      const d = new Date(millis);
      if (!Number.isNaN(d.getTime())) {
        return d.toLocaleString();
      }
    }
    const d = new Date(String(raw));
    if (!Number.isNaN(d.getTime())) {
      return d.toLocaleString();
    }
    return String(raw);
  }

  /**
   * 将 token 规范到 K 单位（Sessions 页面专用）。
   * @param {unknown} raw 原始 token 数。
   * @returns {string}
   */
  function fmtTokenK(raw) {
    const value = Number(raw);
    if (!Number.isFinite(value)) {
      return "--";
    }
    const abs = Math.abs(value);
    if (abs < 1000) {
      return String(Math.trunc(value));
    }
    const scaled = value / 1000;
    const digits = Math.abs(scaled) >= 100 ? 0 : 1;
    return `${scaled.toFixed(digits)}K`;
  }

  /**
   * 将时间戳转换为“距今秒数”。
   * @param {unknown} raw 原始时间值（毫秒/秒/ISO）。
   * @returns {number}
   */
  function ageSeconds(raw) {
    if (raw == null || raw === "") {
      return NaN;
    }
    const num = Number(raw);
    if (Number.isFinite(num) && num > 0) {
      const millis = num > 1_000_000_000_000 ? num : num * 1000;
      return Math.max(0, Math.trunc((Date.now() - millis) / 1000));
    }
    const parsed = Date.parse(String(raw));
    if (Number.isFinite(parsed)) {
      return Math.max(0, Math.trunc((Date.now() - parsed) / 1000));
    }
    return NaN;
  }

  /**
   * 解析 OpenClaw 状态点（网关）。
   * @param {Record<string, any>} detailData 详情 data。
   * @returns {"online"|"offline"|"unknown"}
   */
  function openClawGatewayDot(detailData) {
    const statusDots = asMap(detailData.statusDots);
    const raw = String(statusDots.gateway || "").trim().toLowerCase();
    if (raw === "online" || raw === "offline") {
      return raw;
    }
    return "unknown";
  }

  /**
   * 解析 OpenClaw 状态点（数据新鲜度）。
   * @param {Record<string, any>} detailData 详情 data。
   * @param {boolean} stale 详情是否过期。
   * @returns {"fresh"|"stale"|"collecting"|"unknown"}
   */
  function openClawDataDot(detailData, stale) {
    if (stale) {
      return "stale";
    }
    const collectState = String(detailData.collectState || "").trim().toLowerCase();
    if (collectState === "collecting") {
      return "collecting";
    }
    const statusDots = asMap(detailData.statusDots);
    const raw = String(statusDots.data || "").trim().toLowerCase();
    if (raw === "fresh" || raw === "stale" || raw === "collecting") {
      return raw;
    }
    return "unknown";
  }

  /**
   * 将 OpenClaw 点位值映射到样式类名。
   * @param {"gateway"|"data"} kind 类型。
   * @param {string} value 值。
   * @returns {string}
   */
  function dotClass(kind, value) {
    if (kind === "gateway") {
      if (value === "online") return "online";
      if (value === "offline") return "offline";
      return "unknown";
    }
    if (value === "collecting") return "collecting";
    if (value === "fresh") return "fresh";
    if (value === "stale") return "stale";
    return "unknown";
  }

  /**
   * 将 OpenClaw 点位值映射到中文文案。
   * @param {"gateway"|"data"} kind 类型。
   * @param {string} value 值。
   * @returns {string}
   */
  function dotLabel(kind, value) {
    if (kind === "gateway") {
      if (value === "online") return "网关在线";
      if (value === "offline") return "网关离线";
      return "网关未知";
    }
    if (value === "collecting") return "正在采集中";
    if (value === "fresh") return "已更新";
    if (value === "stale") return "数据过期";
    return "数据未知";
  }

  /**
   * 渲染 OpenClaw 数据时效文案。
   * @param {"fresh"|"stale"|"collecting"|"unknown"} value 点位值。
   * @param {unknown} collectedAt 最近采集时间。
   * @returns {string}
   */
  function openClawFreshnessLabel(value, collectedAt) {
    if (value === "collecting") {
      return "正在采集中";
    }
    const seconds = ageSeconds(collectedAt);
    if (value === "fresh") {
      if (Number.isFinite(seconds)) {
        return `已更新 ${seconds} 秒前`;
      }
      return "已更新";
    }
    if (value === "stale") {
      if (Number.isFinite(seconds)) {
        return `已过期（上次更新 ${seconds} 秒前）`;
      }
      return "已过期";
    }
    return "数据未知";
  }
  /**
   * 将渠道身份列表格式化为摘要文案。
   * @param {Record<string, any>} overview 概览字段。
   * @returns {string}
   */
  function openClawChannelDigest(overview) {
    const identities = asListOfMap(overview.channelIdentities);
    if (identities.length === 0) {
      return "--";
    }
    const labels = identities
      .slice(0, 2)
      .map((row) => {
        const channel = String(row.displayLabel || row.channel || "Unknown").trim();
        const account = String(row.accountDisplay || row.username || row.accountId || "default").trim() || "default";
        return `${channel}@${account}`;
      })
      .filter((line) => line.trim().length > 0);
    const hidden = Math.max(0, identities.length - labels.length);
    if (labels.length === 0) {
      return "--";
    }
    return hidden > 0 ? `${labels.join(" / ")} +${hidden}` : labels.join(" / ");
  }

  /**
   * 构建 OpenClaw 用量头条文案（混合口径）。
   * @param {Record<string, any>} overview 概览字段。
   * @returns {string}
   */
  function openClawUsageHeadline(overview) {
    const usageHeadline = asMap(overview.usageHeadline);
    const label = String(usageHeadline.label || "--").trim() || "--";
    const percent = Number(usageHeadline.percent);
    if (Number.isFinite(percent)) {
      return `${label} · 已使用 ${Math.trunc(percent)}%`;
    }
    return label;
  }

  /**
   * 构建 OpenClaw 会话诊断摘要。
   * @param {Record<string, any>} overview 概览字段。
   * @returns {string}
   */
  function openClawSessionDigest(overview) {
    const activeSessions = Number(overview.activeSessions24h);
    const abortedSessions = Number(overview.abortedSessions);
    const activeText = Number.isFinite(activeSessions) ? Math.max(0, Math.trunc(activeSessions)) : 0;
    const abortedText = Number.isFinite(abortedSessions) ? Math.max(0, Math.trunc(abortedSessions)) : 0;
    return `24h 活跃 ${activeText} · 异常 ${abortedText}`;
  }

  /**
   * 构建 OpenClaw 摘要行（只展示关键安心信息）。
   * @param {Record<string, any>} detailData 详情 data。
   * @returns {Array<[string, string]>}
   */
  function buildOpenClawSummaryRows(detailData) {
    const overview = asMap(detailData.overview);
    const defaultAgent = String(overview.defaultAgentName || overview.defaultAgentId || "--");
    return [
      ["渠道身份", openClawChannelDigest(overview)],
      ["默认 Agent", defaultAgent],
      ["会话诊断", openClawSessionDigest(overview)],
      ["混合用量头条", openClawUsageHeadline(overview)],
    ];
  }

  /**
   * 计算 OpenClaw 页签激活索引。
   * @param {number} count 页签总数。
   * @returns {number}
   */
  function openClawActivePage(count) {
    const raw = Number(state.detailOpenClawPageIndex || 0);
    if (!Number.isFinite(raw) || count <= 0) {
      return 0;
    }
    return Math.max(0, Math.min(count - 1, Math.trunc(raw)));
  }

  /**
   * 计算 OpenClaw Sessions 分段激活键。
   * @returns {"diagnostics"|"timeline"|"ledger"}
   */
  function openClawActiveSessionsSection() {
    const value = String(state.detailOpenClawSessionsSection || "diagnostics").trim();
    if (value === "timeline" || value === "ledger") {
      return value;
    }
    return "diagnostics";
  }

  /**
   * 计算 OpenClaw Usage 窗口激活值。
   * @returns {"1h"|"24h"|"7d"|"all"}
   */
  function openClawActiveUsageWindow() {
    const value = String(state.detailOpenClawUsageWindowPreset || "1h").trim().toLowerCase();
    if (value === "24h" || value === "7d" || value === "all") {
      return value;
    }
    return "1h";
  }

  /**
   * 构建 OpenClaw 概览页。
   * @param {Record<string, any>} detailData 详情 data。
   * @returns {string}
   */
  function renderOpenClawOverviewPage(detailData) {
    const overview = asMap(detailData.overview);
    const systemService = asMap(detailData.systemService);
    const gatewayRuntime = asMap(systemService.gatewayRuntime);
    const dashboardMeta = asMap(overview.dashboardMeta);

    const bindMode = String(gatewayRuntime.bindMode || dashboardMeta.bindMode || "").trim();
    const bindHost = String(gatewayRuntime.bindHost || dashboardMeta.bindHost || "").trim();
    const bindPort = Number(gatewayRuntime.port || dashboardMeta.port);
    const bindAddress = bindHost
      ? (Number.isFinite(bindPort) && bindPort > 0 ? `${bindHost}:${Math.trunc(bindPort)}` : bindHost)
      : "";
    const bindSummary = bindMode && bindAddress
      ? `${bindMode} · ${bindAddress}`
      : bindMode || bindAddress || "未上报";
    const rpcOk = asBool(gatewayRuntime.rpcOk ?? dashboardMeta.rpcReachable);
    const gatewayReachable = asBool(dashboardMeta.gatewayReachable);
    const dashboardAvailable = asBool(dashboardMeta.available);
    const gatewayService = String(
      gatewayRuntime.serviceStatus
      || gatewayRuntime.serviceState
      || overview.gatewayServiceStatus
      || "未知",
    );
    const nodeService = String(overview.nodeServiceStatus || "未知");
    const updateChannel = String(dashboardMeta.updateChannel || "--");
    const updateVersion = String(dashboardMeta.updateVersion || "--");
    const probeUrl = String(gatewayRuntime.probeUrl || dashboardMeta.gatewayUrl || "--");

    return `
      <div class="openclaw-overview-grid">
        <div class="openclaw-overview-card tone-gateway">
          <div class="label">Gateway 绑定</div>
          <div class="value">${escapeHtml(bindSummary)}</div>
          <div class="meta">
            ${escapeHtml(`RPC ${rpcOk ? "可达" : "不可达"} · 网关 ${gatewayReachable ? "在线" : "离线/未知"}`)}
          </div>
        </div>
        <div class="openclaw-overview-card tone-service">
          <div class="label">服务运行态</div>
          <div class="value">${escapeHtml(`Gateway ${gatewayService}`)}</div>
          <div class="meta">${escapeHtml(`Node ${nodeService} · RPC ${rpcOk ? "可达" : "不可达"}`)}</div>
        </div>
      </div>

      <div class="openclaw-section-block tone-dashboard">
        <div class="openclaw-subtitle">Dashboard 状态</div>
        <div class="openclaw-list">
          <div class="openclaw-list-item">
            <div class="name">可用性</div>
            <div class="value">${dashboardAvailable ? "可用" : "不可用"}</div>
          </div>
          <div class="openclaw-list-item">
            <div class="name">连通性</div>
            <div class="value">
              ${escapeHtml(`Gateway ${gatewayReachable ? "可达" : "不可达"} · RPC ${rpcOk ? "可达" : "不可达"}`)}
            </div>
          </div>
          <div class="openclaw-list-item">
            <div class="name">绑定信息</div>
            <div class="value">${escapeHtml(bindSummary)}</div>
          </div>
          <div class="openclaw-list-item">
            <div class="name">RPC URL</div>
            <div class="value">${escapeHtml(probeUrl)}</div>
          </div>
        </div>
      </div>

      <div class="openclaw-section-block tone-version">
        <div class="openclaw-subtitle">版本与更新</div>
        <div class="openclaw-list">
          <div class="openclaw-list-item">
            <div class="name">Update Channel</div>
            <div class="value">${escapeHtml(updateChannel)}</div>
          </div>
          <div class="openclaw-list-item">
            <div class="name">Latest Version</div>
            <div class="value">${escapeHtml(updateVersion)}</div>
          </div>
        </div>
      </div>
    `;
  }

  /**
   * 构建 Agent 上下文文本（已用/上限）。
   * @param {Record<string, any>} agent Agent 数据。
   * @returns {string}
   */
  function openClawAgentContextText(agent) {
    const used = Number(agent.contextUsedTokens);
    const max = Number(agent.contextMaxTokens);
    const usedText = Number.isFinite(used) && used >= 0 ? fmtTokenK(used) : "--";
    const maxText = Number.isFinite(max) && max > 0 ? fmtTokenK(max) : "--";
    return `${usedText} / ${maxText}`;
  }

  /**
   * 构建 Agent 上下文上限来源文案。
   * @param {Record<string, any>} agent Agent 数据。
   * @returns {string}
   */
  function openClawAgentContextSource(agent) {
    const raw = String(agent.contextLimitSource || "").trim();
    if (raw === "session") return "来源：会话上下文";
    if (raw === "agentDefault") return "来源：Agent 默认配置";
    if (raw === "modelConfig") return "来源：模型配置";
    return "来源：未知";
  }

  /**
   * 构建 OpenClaw Agents 页（纵向列表展开）。
   * @param {Record<string, any>} detailData 详情 data。
   * @returns {string}
   */
  function renderOpenClawAgentsPage(detailData) {
    const agents = asListOfMap(detailData.agents);
    if (agents.length === 0) {
      return '<div class="openclaw-empty">暂无 Agent 信息</div>';
    }

    return `
      <div class="openclaw-agent-list">
        ${agents.map((agent) => {
      const agentId = String(agent.agentId || agent.name || "").trim();
      const isOpen = asBool(state.detailOpenClawAgentOpenIds[agentId]);
      const workspace = String(agent.workspaceDir || "--");
      const model = String(agent.model || "--");
      const recentPercent = Number(agent.latestPercentUsed);
      const recentTokens = Number(agent.latestTotalTokens);
      const recentText = Number.isFinite(recentPercent)
        ? `${Math.trunc(recentPercent)}% · ${fmtTokenK(recentTokens)}`
        : fmtTokenK(recentTokens);
      return `
        <article
          class="openclaw-agent-accordion ${isOpen ? "open" : ""}"
          data-openclaw-agent-toggle="${escapeHtml(agentId)}"
        >
          <div class="openclaw-agent-head">
            <div class="left">
              <div class="name">${escapeHtml(String(agent.name || agentId || "--"))}</div>
              ${asBool(agent.isDefault) ? '<span class="chip">默认</span>' : ""}
            </div>
            <div class="right">${escapeHtml(openClawAgentContextText(agent))}</div>
          </div>
          <div class="openclaw-agent-body">
            <div class="openclaw-list-item">
              <div class="name">模型</div>
              <div class="value">${escapeHtml(model)}</div>
            </div>
            <div class="openclaw-list-item">
              <div class="name">上下文（已用/上限）</div>
              <div class="value">${escapeHtml(openClawAgentContextText(agent))}</div>
            </div>
            <div class="openclaw-list-item">
              <div class="name">上下文来源</div>
              <div class="value">${escapeHtml(openClawAgentContextSource(agent))}</div>
            </div>
            <div class="openclaw-list-item">
              <div class="name">近期用量</div>
              <div class="value">${escapeHtml(recentText)}</div>
            </div>
            <div class="openclaw-list-item">
              <div class="name">工作目录</div>
              <div class="value openclaw-path">${escapeHtml(workspace)}</div>
            </div>
          </div>
        </article>
      `;
    }).join("")}
      </div>
    `;
  }

  /**
   * 构建 Sessions 诊断分段。
   * @param {Record<string, any>} diagnostics 诊断字段。
   * @returns {string}
   */
  function renderOpenClawSessionDiagnostics(diagnostics) {
    const abortedRatio = Number(
      diagnostics.abortedRate24h ?? diagnostics.abortedPercent,
    );
    const systemRatio = Number(diagnostics.systemRatio ?? diagnostics.systemPercent);
    const rows = [
      ["24h 活跃", `${fmtInt(diagnostics.active24hCount)} 条`],
      ["aborted 占比", Number.isFinite(abortedRatio) ? `${fmtInt(abortedRatio)}%` : "--"],
      ["system 会话占比", Number.isFinite(systemRatio) ? `${fmtInt(systemRatio)}%` : "--"],
      ["长时间未更新", `${fmtInt(diagnostics.inactiveOver6hCount)} 条`],
    ];
    return `
      <div class="openclaw-grid openclaw-grid-diagnostics">
        ${rows.map(([k, v]) => `
          <div class="openclaw-kv">
            <div class="k">${escapeHtml(k)}</div>
            <div class="v">${escapeHtml(v)}</div>
          </div>
        `).join("")}
      </div>
    `;
  }

  /**
   * 构建 Sessions 时间线分段。
   * @param {Array<Record<string, any>>} timeline 时间线列表。
   * @returns {string}
   */
  function renderOpenClawSessionTimeline(timeline) {
    if (timeline.length === 0) {
      return '<div class="openclaw-empty">暂无时间线数据，正在等待会话事件上报。</div>';
    }
    return `
      <div class="openclaw-list">
        ${timeline.slice(0, 18).map((row) => {
      const flags = Array.isArray(row.flags)
        ? row.flags.filter((item) => String(item || "").trim().length > 0).join(" · ")
        : "";
      const flagsText = flags ? ` · ${flags}` : "";
      const stateLine = `${asBool(row.abortedLastRun) ? "aborted" : "ok"} · ${asBool(row.systemSent) ? "system" : "user"}`;
      const ageSec = Number(row.updatedAgoSec);
      const ageText = Number.isFinite(ageSec) ? `${fmtInt(ageSec)}s 前` : formatTime(row.updatedAt);
      const tokenUsage = [
        `总 ${fmtTokenK(row.totalTokens)}`,
        `余 ${fmtTokenK(row.remainingTokens)}`,
        `${fmtInt(row.percentUsed)}%`,
      ].join(" · ");
      return `
          <div class="openclaw-list-item openclaw-timeline-item">
            <div class="name">
              ${escapeHtml(`${ageText} · ${String(row.agentId || "--")} · ${String(row.model || "--")}`)}
            </div>
            <div class="value">
              ${escapeHtml(`${String(row.kind || "--")} · ${stateLine}${flagsText} · ${tokenUsage}`)}
            </div>
          </div>
        `;
    }).join("")}
      </div>
    `;
  }

  /**
   * 构建 Sessions 台账分段。
   * @param {Array<Record<string, any>>} ledger 台账列表。
   * @returns {string}
   */
  function renderOpenClawSessionLedger(ledger) {
    if (ledger.length === 0) {
      return '<div class="openclaw-empty">暂无台账数据，采集到会话后会自动展示。</div>';
    }
    return `
      <div class="openclaw-list">
        ${ledger.slice(0, 24).map((row) => {
      const sessionId = String(row.sessionId || row.key || "--");
      const compactSessionId = sessionId.length > 18
        ? `${sessionId.slice(0, 9)}…${sessionId.slice(-6)}`
        : sessionId;
      const healthTag = String(row.healthTag || "").trim().toLowerCase();
      const healthLabel = healthTag === "critical"
        ? "高风险"
        : healthTag === "warning"
          ? "关注"
          : "正常";
      const ledgerText = [
        String(row.model || "--"),
        Number.isFinite(Number(row.updatedAgoSec))
          ? `${fmtInt(row.updatedAgoSec)}s 前`
          : formatTime(row.updatedAt),
        `已用 ${fmtTokenK(row.totalTokens)}`,
        `剩余 ${fmtTokenK(row.remainingTokens)}`,
        `占用 ${fmtInt(row.percentUsed)}%`,
      ].join(" · ");
      return `
          <div class="openclaw-list-item openclaw-ledger-item ${escapeHtml(healthTag || "ok")}">
            <div class="name">
              ${escapeHtml(compactSessionId)}
              <span class="openclaw-ledger-tag ${escapeHtml(healthTag || "ok")}">${escapeHtml(healthLabel)}</span>
            </div>
            <div class="value">
              ${escapeHtml(ledgerText)}
            </div>
          </div>
        `;
    }).join("")}
      </div>
    `;
  }

  /**
   * 构建 OpenClaw Sessions 页（三合一分段）。
   * @param {Record<string, any>} detailData 详情 data。
   * @returns {string}
   */
  function renderOpenClawSessionsPage(detailData) {
    const sessions = asMap(detailData.sessions);
    const diagnostics = asMap(sessions.diagnostics);
    const timeline = asListOfMap(sessions.timeline);
    const ledger = asListOfMap(sessions.ledger);
    const active = openClawActiveSessionsSection();
    const hasAny = Object.keys(diagnostics).length > 0 || timeline.length > 0 || ledger.length > 0;
    if (!hasAny) {
      return '<div class="openclaw-collecting-placeholder">正在采集中，会话数据稍后展示。</div>';
    }

    return `
      <div class="openclaw-sessions" data-openclaw-sessions>
        <div class="openclaw-segment-tabs">
          <button
            class="openclaw-segment-tab ${active === "diagnostics" ? "active" : ""}"
            type="button"
            data-openclaw-sessions-section="diagnostics"
          >
            诊断
          </button>
          <button
            class="openclaw-segment-tab ${active === "timeline" ? "active" : ""}"
            type="button"
            data-openclaw-sessions-section="timeline"
          >
            时间线
          </button>
          <button
            class="openclaw-segment-tab ${active === "ledger" ? "active" : ""}"
            type="button"
            data-openclaw-sessions-section="ledger"
          >
            台账
          </button>
        </div>
        <section
          class="openclaw-segment-panel ${active === "diagnostics" ? "active" : ""}"
          data-openclaw-sessions-panel="diagnostics"
        >
          ${renderOpenClawSessionDiagnostics(diagnostics)}
        </section>
        <section
          class="openclaw-segment-panel ${active === "timeline" ? "active" : ""}"
          data-openclaw-sessions-panel="timeline"
        >
          ${renderOpenClawSessionTimeline(timeline)}
        </section>
        <section
          class="openclaw-segment-panel ${active === "ledger" ? "active" : ""}"
          data-openclaw-sessions-panel="ledger"
        >
          ${renderOpenClawSessionLedger(ledger)}
        </section>
      </div>
    `;
  }

  /**
   * 构建 Usage 内“账号窗口”分组渲染。
   * @param {Array<Record<string, any>>} providerWindows provider 窗口。
   * @returns {string}
   */
  function renderOpenClawUsageProviderWindows(providerWindows) {
    if (providerWindows.length === 0) {
      return '<div class="openclaw-empty">暂无账号窗口数据</div>';
    }
    /** @type {Record<string, Array<Record<string, any>>>} */
    const groups = {};
    /** @type {Record<string, string>} */
    const providerUsers = {};
    for (const row of providerWindows) {
      const provider = String(row.displayName || row.provider || "Unknown");
      if (!groups[provider]) {
        groups[provider] = [];
      }
      groups[provider].push(row);
      const authUser = String(row.authUser || "").trim();
      if (authUser && !providerUsers[provider]) {
        providerUsers[provider] = authUser;
      }
    }

    return Object.keys(groups).sort().map((provider) => {
      const authUser = String(providerUsers[provider] || "").trim();
      const providerTitle = authUser ? `${provider}（账号 ${authUser}）` : provider;
      const rows = groups[provider]
        .sort((a, b) => Number(b.usedPercent || 0) - Number(a.usedPercent || 0))
        .map((row) => {
          const resetText = row.resetAt ? formatTime(row.resetAt) : "--";
          const percentText = Number.isFinite(Number(row.usedPercent))
            ? `${Math.trunc(Number(row.usedPercent))}%`
            : "--";
          const used = Number(row.used);
          const limit = Number(row.limit);
          const remaining = Number(row.remaining);
          const hasAmount = Number.isFinite(used) || Number.isFinite(limit) || Number.isFinite(remaining);
          const amountText = hasAmount
            ? `已用 ${Number.isFinite(used) ? fmt2(used) : "--"} / ${Number.isFinite(limit) ? fmt2(limit) : "--"}，剩余 ${Number.isFinite(remaining) ? fmt2(remaining) : "--"}`
            : "";
          return `
            <div class="openclaw-list-item">
              <div class="name">${escapeHtml(String(row.label || "--"))}</div>
              <div class="value">${escapeHtml(`已使用 ${percentText} · 重置 ${resetText}`)}</div>
              ${amountText ? `<div class="value">${escapeHtml(amountText)}</div>` : ""}
            </div>
          `;
        })
        .join("");
      return `
        <div class="openclaw-section-block tone-usage-provider">
          <div class="openclaw-subtitle">${escapeHtml(providerTitle)}</div>
          <div class="openclaw-list">${rows}</div>
        </div>
      `;
    }).join("");
  }

  /**
   * 构建 OpenClaw Usage 页（混合口径）。
   * @param {Record<string, any>} detailData 详情 data。
   * @returns {string}
   */
  function renderOpenClawUsagePage(detailData) {
    const usage = asMap(detailData.usage);
    const providerWindows = asListOfMap(usage.authWindows).length > 0
      ? asListOfMap(usage.authWindows)
      : asListOfMap(usage.providerWindows);
    const modelsWithCost = asListOfMap(usage.modelsWithCost);
    const modelsWithoutCost = asListOfMap(usage.modelsWithoutCost);
    const apiProviderCards = asListOfMap(usage.apiProviderCards);
    const coverage = asMap(usage.coverage);
    const activeWindow = openClawActiveUsageWindow();
    const onlyOneHourData = String(usage.windowPreset || "1h").trim().toLowerCase() === "1h";

    const renderModelRows = (rows, emptyText) => {
      if (!Array.isArray(rows) || rows.length === 0) {
        return `<div class="openclaw-empty">${escapeHtml(emptyText)}</div>`;
      }
      return rows.slice(0, 120).map((row) => {
        const provider = String(row.provider || "--");
        const model = String(row.model || "--");
        const tokenText = [
          `总 ${fmtTokenM(row.tokenTotal)}`,
          `输入 ${fmtTokenM(row.tokenInput)}`,
          `输出 ${fmtTokenM(row.tokenOutput)}`,
          `消息 ${fmtInt(row.messages)}`,
        ].join(" · ");
        const costText = `估算成本 ${fmt2(row.totalCost)} (${String(row.currency || "config-rate")})`;
        return `
          <div class="openclaw-list-item">
            <div class="name">${escapeHtml(`${provider} · ${model}`)}</div>
            <div class="value">${escapeHtml(tokenText)}</div>
            <div class="value">${escapeHtml(costText)}</div>
          </div>
        `;
      }).join("");
    };

    const apiCardsHtml = apiProviderCards.length > 0
      ? apiProviderCards.slice(0, 32).map((card) => {
        const provider = String(card.provider || "--");
        const rows = asListOfMap(card.models);
        const rowsHtml = rows.length > 0
          ? rows.slice(0, 24).map((row) => `
              <div class="openclaw-list-item">
                <div class="name">${escapeHtml(String(row.model || "--"))}</div>
                <div class="value">
                  ${escapeHtml(`总 ${fmtTokenM(row.tokenTotal)} · 输入 ${fmtTokenM(row.tokenInput)} · 输出 ${fmtTokenM(row.tokenOutput)}`)}
                </div>
                <div class="value">${escapeHtml(`估算成本 ${fmt2(row.totalCost)} (${String(row.currency || "config-rate")})`)}</div>
              </div>
            `).join("")
          : '<div class="openclaw-empty">当前 provider 没有模型明细</div>';
        const balanceStatus = String(card.balanceStatus || "unavailable");
        const balanceText = balanceStatus === "available"
          ? `${fmt2(card.providerBalance)}`
          : String(card.balanceNote || "未获取到");
        const statAt = Number(card.statAt);
        return `
          <div class="openclaw-section-block tone-usage-model">
            <div class="openclaw-subtitle">${escapeHtml(provider)}</div>
            <div class="openclaw-list">
              ${rowsHtml}
              <div class="openclaw-list-item">
                <div class="name">聚合统计</div>
                <div class="value">
                  ${escapeHtml(`总Token ${fmtTokenM(card.providerTokenTotal)} · 总估算成本 ${fmt2(card.providerCostTotal)}`)}
                </div>
                <div class="value">
                  ${escapeHtml(`余额 ${balanceText} · 统计时间 ${Number.isFinite(statAt) && statAt > 0 ? formatTime(statAt) : "--"}`)}
                </div>
              </div>
            </div>
          </div>
        `;
      }).join("")
      : '<div class="openclaw-empty">暂无 API 聚合数据</div>';

    const coverageWindowProviders = Array.isArray(coverage.windowProviders)
      ? coverage.windowProviders.map((row) => String(row || "")).filter((row) => row)
      : [];
    const coverageEstimatedModels = Array.isArray(coverage.estimatedModels)
      ? coverage.estimatedModels.map((row) => String(row || "")).filter((row) => row)
      : [];
    const coverageNote = String(coverage.note || "").trim();

    const windowTabs = [
      ["1h", "1小时"],
      ["24h", "24小时"],
      ["7d", "7天"],
      ["all", "全部"],
    ];
    const tabsHtml = windowTabs.map(([value, label]) => `
      <button
        class="openclaw-segment-tab ${activeWindow === value ? "active" : ""}"
        type="button"
        data-openclaw-usage-window="${value}"
      >
        ${label}
      </button>
    `).join("");
    const unsupportedHint = activeWindow !== "1h" && onlyOneHourData
      ? '<div class="openclaw-empty">当前版本仅采集“过去1小时”窗口，其他窗口将后续补齐。</div>'
      : "";

    return `
      <div class="openclaw-section-block tone-usage-coverage">
        <div class="openclaw-subtitle">统计窗口</div>
        <div class="openclaw-segment-tabs">${tabsHtml}</div>
      </div>
      ${unsupportedHint}
      <div class="openclaw-section-block tone-usage-window">
        <div class="openclaw-subtitle">账号窗口（真实值）</div>
        ${renderOpenClawUsageProviderWindows(providerWindows)}
      </div>
      <div class="openclaw-section-block tone-usage-model">
        <div class="openclaw-subtitle">已产生费用模型（1小时）</div>
        <div class="openclaw-list">${renderModelRows(modelsWithCost, "1小时内暂无已产生费用模型")}</div>
      </div>
      <div class="openclaw-section-block tone-usage-cost">
        <div class="openclaw-subtitle">未产生费用模型（1小时）</div>
        <div class="openclaw-list">${renderModelRows(modelsWithoutCost, "全部配置模型在1小时内都有消耗")}</div>
      </div>
      <div class="openclaw-section-block tone-usage-window">
        <div class="openclaw-subtitle">API 调用聚合（按 provider）</div>
        ${apiCardsHtml}
      </div>
      <div class="openclaw-section-block tone-usage-coverage">
        <div class="openclaw-subtitle">覆盖说明</div>
        <div class="openclaw-list">
          <div class="openclaw-list-item">
            <div class="name">账号窗口来源</div>
            <div class="value">${escapeHtml(coverageWindowProviders.join(" / ") || "--")}</div>
          </div>
          <div class="openclaw-list-item">
            <div class="name">已产生费用模型</div>
            <div class="value">${escapeHtml(coverageEstimatedModels.join(" / ") || "--")}</div>
          </div>
          <div class="openclaw-list-item">
            <div class="name">模型覆盖</div>
            <div class="value">${escapeHtml(`配置 ${fmtInt(coverage.configuredModelCount)} · 活跃(1h) ${fmtInt(coverage.activeModelCount1h)}`)}</div>
          </div>
          <div class="openclaw-list-item">
            <div class="name">说明</div>
            <div class="value">${escapeHtml(coverageNote || "--")}</div>
          </div>
        </div>
      </div>
    `;
  }

  /**
   * 构建 OpenClaw 系统与服务页。
   * @param {Record<string, any>} detailData 详情 data。
   * @returns {string}
   */
  function renderOpenClawSystemPage(detailData) {
    const systemService = asMap(detailData.systemService);
    const memoryIndex = asListOfMap(systemService.memoryIndex);
    const security = asMap(systemService.securitySummary);
    const findings = asListOfMap(systemService.securityFindings);
    const gatewayRuntime = asMap(systemService.gatewayRuntime);
    const health = asMap(systemService.healthSummary);
    const healthKnown = Object.keys(health).length > 0;
    const healthStatus = healthKnown ? (asBool(health.ok) ? "健康" : "异常") : "未知";
    const healthDuration = Number(health.durationMs);
    const healthDurationText = Number.isFinite(healthDuration) && healthDuration > 0
      ? `${fmtInt(healthDuration)}ms`
      : "未采集";
    const gatewayServiceText = String(gatewayRuntime.serviceStatus || gatewayRuntime.serviceState || "未知");
    const gatewayPid = Number(gatewayRuntime.pid);
    const gatewayPidText = Number.isFinite(gatewayPid) && gatewayPid > 0 ? `PID ${fmtInt(gatewayPid)}` : "PID 未上报";
    const findingCount = Number.isFinite(Number(security.findingsCount))
      ? Math.max(0, Math.trunc(Number(security.findingsCount)))
      : findings.length;
    const canExpandFindings = findingCount > 0 && findings.length > 0;
    const expandedFindings = canExpandFindings && asBool(state.detailOpenClawSecurityExpanded);

    const memoryHtml = memoryIndex.length > 0
      ? memoryIndex.slice(0, 16).map((row) => {
        const store = [
          `${fmtInt(row.files)} files`,
          `${fmtInt(row.chunks)} chunks`,
          `${asBool(row.dirty) ? "dirty" : "clean"}`,
        ].join(" · ");
        const vectorFts = [
          asBool(row.vectorAvailable) ? "Vector" : "Vector-",
          asBool(row.ftsAvailable) ? "FTS" : "FTS-",
        ].join(" / ");
        return `
          <div class="openclaw-list-item">
            <div class="name">${escapeHtml(String(row.agentId || "global"))}</div>
            <div class="value">
              ${escapeHtml(`${String(row.backend || "--")} · ${store} · Cache ${fmtInt(row.cacheEntries)} · ${vectorFts}`)}
            </div>
          </div>
        `;
      }).join("")
      : '<div class="openclaw-empty">暂无 memory index 数据</div>';

    return `
      <div class="openclaw-grid openclaw-grid-system">
        <div class="openclaw-kv tone-health">
          <div class="k">Health</div>
          <div class="v">${escapeHtml(healthStatus)}</div>
        </div>
        <div class="openclaw-kv tone-health">
          <div class="k">健康耗时</div>
          <div class="v">${escapeHtml(healthDurationText)}</div>
        </div>
        <div class="openclaw-kv tone-service">
          <div class="k">Gateway RPC</div>
          <div class="v">${asBool(gatewayRuntime.rpcOk) ? "可达" : "不可达/未知"}</div>
        </div>
        <div class="openclaw-kv tone-service">
          <div class="k">Gateway Service</div>
          <div class="v">${escapeHtml(`${gatewayServiceText} · ${gatewayPidText}`)}</div>
        </div>
      </div>

      <div class="openclaw-section-block tone-memory">
        <div class="openclaw-subtitle">记忆与索引</div>
        <div class="openclaw-list">${memoryHtml}</div>
      </div>

      <div
        class="openclaw-section-block tone-security ${canExpandFindings ? "clickable" : ""}"
        ${canExpandFindings ? 'data-openclaw-security-toggle="1"' : ""}
      >
        <div class="openclaw-subtitle">安全审计摘要</div>
        <div class="openclaw-list">
          <div class="openclaw-list-item">
            <div class="name">风险分布</div>
            <div class="value">
              ${escapeHtml(`critical ${fmtInt(security.critical)} · warn ${fmtInt(security.warn)} · info ${fmtInt(security.info)}`)}
            </div>
          </div>
          <div class="openclaw-list-item">
            <div class="name">发现数量</div>
            <div class="value">${escapeHtml(`${fmtInt(findingCount)} 条`)}</div>
          </div>
          ${canExpandFindings ? `
            <div class="openclaw-list-item">
              <div class="name">点击查看</div>
              <div class="value">${expandedFindings ? "收起审计详情" : "展开审计详情"}</div>
            </div>
          ` : ""}
        </div>
        ${expandedFindings ? `
          <div class="openclaw-security-findings">
            ${findings.slice(0, 20).map((row) => {
      const severity = String(row.severity || "info").trim().toLowerCase();
      const title = String(row.title || row.checkId || "未命名检查");
      const checkId = String(row.checkId || "--");
      const detail = String(row.detail || "").trim();
      const trimmedDetail = detail.length > 180 ? `${detail.slice(0, 180)}...` : detail;
      return `
                <div class="openclaw-security-item ${escapeHtml(severity)}">
                  <div class="name">${escapeHtml(`${severity.toUpperCase()} · ${title}`)}</div>
                  <div class="value">${escapeHtml(`checkId: ${checkId}`)}</div>
                  ${trimmedDetail ? `<div class="value">${escapeHtml(trimmedDetail)}</div>` : ""}
                </div>
              `;
    }).join("")}
          </div>
        ` : ""}
      </div>
    `;
  }

  /**
   * 构建 OpenClaw 多屏详情页定义。
   * @param {Record<string, any>} detailData 详情 data。
   * @returns {Array<{title: string, body: string}>}
   */
  function buildOpenClawPages(detailData) {
    return [
      { title: "概览", body: renderOpenClawOverviewPage(detailData) },
      { title: "Agents", body: renderOpenClawAgentsPage(detailData) },
      { title: "Sessions", body: renderOpenClawSessionsPage(detailData) },
      { title: "Usage", body: renderOpenClawUsagePage(detailData) },
      { title: "系统与服务", body: renderOpenClawSystemPage(detailData) },
    ];
  }

  /**
   * 渲染 OpenClaw 多屏详情容器。
   * @param {Array<{title: string, body: string}>} pages 页面数据。
   * @returns {string}
   */
  function renderOpenClawPages(pages) {
    const activeIndex = openClawActivePage(pages.length);
    const tabHtml = pages.map((page, index) => `
      <button
        class="openclaw-page-tab ${index === activeIndex ? "active" : ""}"
        type="button"
        data-openclaw-page-index="${index}"
      >
        ${escapeHtml(page.title)}
      </button>
    `).join("");

    const pagesHtml = pages.map((page) => `
      <article class="openclaw-page">
        ${page.body}
      </article>
    `).join("");

    const dotsHtml = pages.map((_, index) => `
      <button
        class="openclaw-page-dot ${index === activeIndex ? "active" : ""}"
        type="button"
        data-openclaw-page-index="${index}"
        aria-label="切换到第 ${index + 1} 页"
      ></button>
    `).join("");

    return `
      <div class="openclaw-pages" data-openclaw-pages>
        <div class="openclaw-page-tabs">${tabHtml}</div>
        <div class="openclaw-pages-track" data-openclaw-pages-track>${pagesHtml}</div>
        <div class="openclaw-page-dots">${dotsHtml}</div>
      </div>
    `;
  }

  /**
   * 同步 Sessions 分段高亮与内容可见性。
   */
  function syncOpenClawSessionsSectionUi() {
    const wrapper = ui.detailRows.querySelector("[data-openclaw-sessions]");
    if (!(wrapper instanceof Element)) {
      return;
    }
    const active = openClawActiveSessionsSection();
    const buttons = wrapper.querySelectorAll("[data-openclaw-sessions-section]");
    for (const button of buttons) {
      const section = String(button.getAttribute("data-openclaw-sessions-section") || "");
      button.classList.toggle("active", section === active);
    }

    const panels = wrapper.querySelectorAll("[data-openclaw-sessions-panel]");
    for (const panel of panels) {
      const section = String(panel.getAttribute("data-openclaw-sessions-panel") || "");
      panel.classList.toggle("active", section === active);
    }
  }

  /**
   * 将 OpenClaw 页面索引同步到按钮高亮与横向滚动位置。
   */
  function syncOpenClawPageUi() {
    const wrapper = ui.detailRows.querySelector("[data-openclaw-pages]");
    if (!(wrapper instanceof Element)) {
      return;
    }
    const track = wrapper.querySelector("[data-openclaw-pages-track]");
    if (!(track instanceof HTMLElement)) {
      return;
    }

    const pageCount = track.querySelectorAll(".openclaw-page").length;
    const activeIndex = openClawActivePage(pageCount);
    state.detailOpenClawPageIndex = activeIndex;

    const tabButtons = wrapper.querySelectorAll("[data-openclaw-page-index]");
    for (const button of tabButtons) {
      const index = Number(button.getAttribute("data-openclaw-page-index"));
      button.classList.toggle("active", Number.isFinite(index) && index === activeIndex);
    }

    const left = activeIndex * track.clientWidth;
    if (Math.abs(track.scrollLeft - left) > 1) {
      track.scrollTo({ left, behavior: "auto" });
    }
    syncOpenClawSessionsSectionUi();
  }

  /**
   * 渲染 OpenClaw 专属详情弹窗。
   * @param {object} input 渲染上下文。
   */
  function renderOpenClawModal(input) {
    const { runtime, tool, metric, detail, detailData, displayName } = input;
    const gatewayDot = openClawGatewayDot(detailData);
    const dataDot = openClawDataDot(detailData, asBool(detail.stale));
    const freshnessLabel = openClawFreshnessLabel(
      dataDot,
      detail.collectedAt || detailData.collectingSince,
    );
    const summaryRows = buildOpenClawSummaryRows(detailData);
    const pages = buildOpenClawPages(detailData);

    ui.toolModalTitle.textContent = displayName || "OpenClaw";
    ui.toolSummaryTitle.textContent = "OpenClaw 摘要";
    ui.toolDetailSectionTitle.textContent = "OpenClaw 详情";
    ui.usagePanelTitle.textContent = "模型用量（当前会话）";

    ui.summaryRows.innerHTML = renderRows(summaryRows);
    ui.summaryStatusDots.innerHTML = `
      <div class="tool-summary-dot-line">
        <span class="tool-dot ${dotClass("gateway", gatewayDot)}"></span>
        <span>${escapeHtml(dotLabel("gateway", gatewayDot))}</span>
      </div>
      <div class="tool-summary-dot-line">
        <span class="tool-dot ${dotClass("data", dataDot)}"></span>
        <span>${escapeHtml(freshnessLabel)}</span>
      </div>
    `;

    ui.toggleDetailsBtn.style.display = "none";
    ui.detailRows.innerHTML = renderOpenClawPages(pages);
    ui.detailTip.textContent = asBool(detail.stale)
      ? "当前展示的是最近一次成功采集结果，数据状态为过期。"
      : "点击上方标签切换概览、Agents、Sessions、Usage、系统与服务。";
    ui.usagePanel.style.display = "none";
    ui.usageRows.innerHTML = "";
    ui.toolModal.classList.add("show");

    // 每次渲染后根据状态同步页签高亮与滚动位置。
    syncOpenClawPageUi();

    // 这里保留基础字段采集，避免未来需要快速回看运行态时丢失上下文。
    void runtime;
    void tool;
    void metric;
  }

  /**
   * 渲染通用（OpenCode/Generic）详情弹窗。
   * @param {object} input 渲染上下文。
   */
  function renderDefaultModal(input) {
    const { host, runtime, tool, metric, detail, detailData, displayName } = input;
    const toolId = String(tool.toolId || "");
    const workspace = pickMetric(tool, metric, "workspaceDir") || "--";
    const connectedTool = asBool(metric.connected ?? tool.connected);
    const latestTokens = asMap(metric.latestTokens);
    const modelUsage = asListOfMap(metric.modelUsage);
    const schema = String(detail.schema || "").trim();
    const stale = asBool(detail.stale);

    const summaryRows = [
      ["工具名称", displayName],
      ["宿主机", String(host.displayName || "--")],
      ["工作目录", workspace],
      ["工具模式", pickMetric(tool, metric, "mode") || "--"],
      ["会话模式", pickMetric(tool, metric, "agentMode") || "--"],
      ["当前模型", pickMetric(tool, metric, "model") || "--"],
      ["状态", pickMetric(tool, metric, "status") || "--"],
      ["详情Schema", schema || "--"],
      ["详情状态", stale ? "数据过期（展示最近成功值）" : "已同步"],
      ["详情采集时间", String(detail.collectedAt || "--")],
      [
        "最近Token（总/输入/输出）",
        `${fmtTokenM(latestTokens.total)} / ${fmtTokenM(latestTokens.input)} / ${fmtTokenM(latestTokens.output)}`,
      ],
      ["最近缓存（读/写）", `${fmtTokenM(latestTokens.cacheRead)} / ${fmtTokenM(latestTokens.cacheWrite)}`],
      ["模型用量", usageSummary(modelUsage)],
    ];

    const reason = pickMetric(tool, metric, "reason");
    if (reason) {
      summaryRows.push(["原因", reason]);
    }

    const detailsRows = [
      ["App Link", runtime.connected ? "Connected" : "Disconnected"],
      [
        "Last Heartbeat",
        runtime.lastHeartbeatAt ? runtime.lastHeartbeatAt.toLocaleString() : "--",
      ],
      ["Tool Reachable", connectedTool ? "Yes" : "No"],
      ["Tool ID", toolId || "--"],
      ["Endpoint", pickMetric(tool, metric, "endpoint") || "--"],
      ["Session ID", pickMetric(tool, metric, "sessionId") || "--"],
      ["Session Title", pickMetric(tool, metric, "sessionTitle") || "--"],
      ["Session Updated", pickMetric(tool, metric, "sessionUpdatedAt") || "--"],
      ["厂商", pickMetric(tool, metric, "vendor") || "--"],
      ["类别", localizedCategory(tool.category)],
      ["CPU", `${fmt2(metric.cpuPercent)}%`],
      ["Memory", `${fmt2(metric.memoryMb)} MB`],
      ["Source", String(metric.source || "--")],
      [
        "Latest Cache",
        `R:${fmtTokenM(latestTokens.cacheRead)} W:${fmtTokenM(latestTokens.cacheWrite)}`,
      ],
    ];

    let extraRows = [];
    let extraUsageRows = [];

    if (schema === "opencode.v1") {
      const opencodeLatestTokens = asMap(detailData.latestTokens);
      const opencodeUsage = asListOfMap(detailData.modelUsage);
      extraRows = [
        ["Profile", String(detail.profileKey || "--")],
        ["Session ID", String(detailData.sessionId || "--")],
        ["Session Title", String(detailData.sessionTitle || "--")],
        ["Session Updated", String(detailData.sessionUpdatedAt || "--")],
        ["Agent Mode", String(detailData.agentMode || "--")],
        ["Provider", String(detailData.providerId || "--")],
      ];
      extraUsageRows = opencodeUsage.map((row) => {
        const modelName = String(row.model || "--");
        const total = fmtTokenM(row.tokenTotal);
        const input = fmtTokenM(row.tokenInput);
        const output = fmtTokenM(row.tokenOutput);
        const count = fmtInt(row.messages);
        return [modelName, `消息 ${count} 条 · 总Token ${total} · 输入 ${input} · 输出 ${output}`];
      });
      if (extraUsageRows.length === 0 && Object.keys(opencodeLatestTokens).length > 0) {
        extraUsageRows.push([
          "最近 Token",
          `${fmtTokenM(opencodeLatestTokens.total)} / ${fmtTokenM(opencodeLatestTokens.input)} / ${fmtTokenM(opencodeLatestTokens.output)}`,
        ]);
      }
    }

    if (extraRows.length > 0) {
      detailsRows.push(...extraRows);
    }

    ui.toolModalTitle.textContent = displayName || "Tool Detail";
    ui.toolSummaryTitle.textContent = schema === "opencode.v1" ? "OpenCode 摘要" : "工具摘要";
    ui.toolDetailSectionTitle.textContent = "更多信息";
    ui.usagePanelTitle.textContent = "模型用量（当前会话）";
    ui.summaryStatusDots.innerHTML = "";
    ui.summaryRows.innerHTML = renderRows(summaryRows);

    const previewCount = 2;
    const showingRows = state.detailExpanded ? detailsRows : detailsRows.slice(0, previewCount);
    ui.detailRows.innerHTML = renderRows(showingRows);
    ui.detailTip.textContent = !state.detailExpanded && detailsRows.length > previewCount
      ? `还有 ${detailsRows.length - previewCount} 项，点击箭头展开`
      : "";

    ui.toggleDetailsBtn.style.display = "";
    ui.toggleDetailsBtn.textContent = state.detailExpanded ? "⌃" : "⌄";

    if (state.detailExpanded && extraUsageRows.length > 0) {
      ui.usagePanel.style.display = "block";
      ui.usageRows.innerHTML = renderRows(extraUsageRows);
    } else if (state.detailExpanded && modelUsage.length > 0) {
      ui.usagePanel.style.display = "block";
      ui.usageRows.innerHTML = renderRows(
        modelUsage.map((row) => {
          const modelName = String(row.model || "--");
          const total = fmtTokenM(row.tokenTotal);
          const input = fmtTokenM(row.tokenInput);
          const output = fmtTokenM(row.tokenOutput);
          const count = fmtInt(row.messages);
          return [
            modelName,
            `消息 ${count} 条 · 总Token ${total} · 输入 ${input} · 输出 ${output}`,
          ];
        }),
      );
    } else {
      ui.usagePanel.style.display = "none";
      ui.usageRows.innerHTML = "";
    }

    ui.toolModal.classList.add("show");
  }

  /** 渲染工具详情弹窗。 */
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
    const detail = detailForTool(state.detailHostId, String(tool.toolId || ""));
    const detailData = asMap(detail.data);
    const schema = String(detail.schema || "").trim();
    const displayName = resolveToolDisplayName(state.detailHostId, tool);

    if (schema === "openclaw.v1") {
      renderOpenClawModal({
        host,
        runtime,
        tool,
        metric,
        detail,
        detailData,
        displayName,
      });
      return;
    }

    renderDefaultModal({
      host,
      runtime,
      tool,
      metric,
      detail,
      detailData,
      displayName,
    });
  }

  /**
   * 根据用户选择切换 OpenClaw 页签索引。
   * @param {number} index 目标索引。
   */
  function setOpenClawPage(index) {
    const wrapper = ui.detailRows.querySelector("[data-openclaw-pages]");
    if (!(wrapper instanceof Element)) {
      return;
    }
    const track = wrapper.querySelector("[data-openclaw-pages-track]");
    if (!(track instanceof HTMLElement)) {
      return;
    }
    const pageCount = track.querySelectorAll(".openclaw-page").length;
    if (pageCount <= 0) {
      return;
    }
    const safeIndex = Math.max(0, Math.min(pageCount - 1, Math.trunc(index)));
    state.detailOpenClawPageIndex = safeIndex;
    syncOpenClawPageUi();
  }

  /** 绑定详情弹窗相关事件。 */
  function bindToolDetailModalEvents() {
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

    // 详情区域承载 OpenClaw 页签与 Sessions 分段按钮，统一使用事件代理。
    ui.detailRows.addEventListener("click", (event) => {
      const target = event.target;
      if (!(target instanceof Element)) {
        return;
      }
      const pageBtn = target.closest("[data-openclaw-page-index]");
      if (pageBtn instanceof Element) {
        const index = Number(pageBtn.getAttribute("data-openclaw-page-index"));
        if (Number.isFinite(index)) {
          setOpenClawPage(index);
        }
        return;
      }

      const sessionsBtn = target.closest("[data-openclaw-sessions-section]");
      if (sessionsBtn instanceof Element) {
        const section = String(sessionsBtn.getAttribute("data-openclaw-sessions-section") || "");
        if (section === "diagnostics" || section === "timeline" || section === "ledger") {
          state.detailOpenClawSessionsSection = section;
          syncOpenClawSessionsSectionUi();
        }
        return;
      }

      const usageWindowBtn = target.closest("[data-openclaw-usage-window]");
      if (usageWindowBtn instanceof Element) {
        const preset = String(usageWindowBtn.getAttribute("data-openclaw-usage-window") || "")
          .trim()
          .toLowerCase();
        if (preset === "1h" || preset === "24h" || preset === "7d" || preset === "all") {
          state.detailOpenClawUsageWindowPreset = preset;
          renderToolModal();
        }
        return;
      }

      const securityToggle = target.closest("[data-openclaw-security-toggle]");
      if (securityToggle instanceof Element) {
        state.detailOpenClawSecurityExpanded = !asBool(state.detailOpenClawSecurityExpanded);
        renderToolModal();
        return;
      }

      const agentToggle = target.closest("[data-openclaw-agent-toggle]");
      if (agentToggle instanceof Element) {
        const agentId = String(agentToggle.getAttribute("data-openclaw-agent-toggle") || "").trim();
        if (!agentId) {
          return;
        }
        const next = !asBool(state.detailOpenClawAgentOpenIds[agentId]);
        if (next) {
          state.detailOpenClawAgentOpenIds[agentId] = true;
        } else {
          delete state.detailOpenClawAgentOpenIds[agentId];
        }
        renderToolModal();
      }
    });

    ui.detailRows.addEventListener("scroll", (event) => {
      const target = event.target;
      if (!(target instanceof HTMLElement) || !target.hasAttribute("data-openclaw-pages-track")) {
        return;
      }
      const width = target.clientWidth;
      if (width <= 0) {
        return;
      }
      const index = Math.round(target.scrollLeft / width);
      if (state.detailOpenClawPageIndex !== index) {
        state.detailOpenClawPageIndex = index;
        syncOpenClawPageUi();
      }
    }, true);
  }

  return {
    openToolDetail,
    closeToolDetail,
    renderToolModal,
    bindToolDetailModalEvents,
  };
}
