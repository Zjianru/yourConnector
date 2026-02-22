// 文件职责：
// 1. 承载运维页顶部按钮与 Banner 交互的轻量渲染逻辑。
// 2. 减少入口文件中的 UI 细节代码。

import { deriveBannerActiveIndex, renderBannerDots as renderBannerDotsView } from "./banner.js";

/**
 * 渲染顶部按钮可用状态。
 * @param {object} ui 页面节点集合。
 * @param {number} hostCount 宿主机数量。
 * @param {boolean} hasConnectableHost 是否有可连接宿主机。
 * @param {boolean} isAnyHostConnected 是否至少有一台已连接。
 */
export function renderTopActions(ui, hostCount, hasConnectableHost, isAnyHostConnected) {
  ui.connectBtnTop.disabled = hostCount === 0 || !hasConnectableHost;
  ui.disconnectBtnTop.disabled = hostCount === 0 || !isAnyHostConnected;
  ui.replaceHostBtnTop.disabled = false;
}

/**
 * 同步 Banner 当前索引到状态，并刷新白点。
 * @param {object} ui 页面节点集合。
 * @param {object} state 全局状态。
 */
export function syncBannerActiveIndex(ui, state) {
  const active = deriveBannerActiveIndex(ui.hostBannerTrack);
  if (!active || active.index === state.bannerActiveIndex) {
    return;
  }
  state.bannerActiveIndex = active.index;
  renderBannerDotsView(ui, active.total, state.bannerActiveIndex);
}

/**
 * 从 Banner 点击事件中提取宿主机 ID。
 * @param {Event} event 点击事件。
 * @returns {string}
 */
export function extractBannerHostId(event) {
  const target = event.target;
  if (!(target instanceof Element)) {
    return "";
  }
  const card = target.closest("[data-banner-host-id]");
  if (!card) {
    return "";
  }
  return String(card.getAttribute("data-banner-host-id") || "").trim();
}
