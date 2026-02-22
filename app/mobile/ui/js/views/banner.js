// 文件职责：
// 1. 渲染宿主机 Banner 与白点索引。
// 2. 处理 Banner 横向滚动与索引更新。

/**
 * 根据是否存在宿主机切换“配对卡”与“总览区”显示。
 * @param {Record<string, HTMLElement>} ui 页面节点集合。
 * @param {boolean} hasHosts 是否存在宿主机。
 */
export function renderHostStage(ui, hasHosts) {
  ui.hostSetupCard.style.display = hasHosts ? "none" : "block";
  ui.hostOverviewWrap.classList.toggle("hidden", !hasHosts);
}

/**
 * 渲染宿主机 Banner 卡片。
 * @param {Record<string, HTMLElement>} ui 页面节点集合。
 * @param {Array<object>} hosts 宿主机列表。
 * @param {number} activeIndex 当前索引。
 * @param {(hostId: string) => string} hostStatusLabel 状态文案函数。
 * @param {(value: unknown) => string} escapeHtml 转义函数。
 */
export function renderBanner(ui, hosts, activeIndex, hostStatusLabel, escapeHtml) {
  if (hosts.length === 0) {
    ui.hostBannerTrack.innerHTML = '<div class="empty">暂无已配对宿主机。</div>';
    ui.hostBannerDots.innerHTML = "";
    return;
  }

  const previousScroll = ui.hostBannerTrack.scrollLeft;
  ui.hostBannerTrack.innerHTML = hosts
    .map((host) => {
      const status = hostStatusLabel(host.hostId);
      const statusClass = status === "在线" ? "online" : "offline";
      return `
        <article
          class="host-banner-card host-banner-clickable"
          data-banner-host-id="${escapeHtml(host.hostId)}"
          title="点击查看宿主机负载"
        >
          <div class="host-banner-name">${escapeHtml(host.displayName)}</div>
          <div class="host-banner-status">
            <span class="host-status-light ${statusClass}"></span>
            ${escapeHtml(status)}
          </div>
        </article>
      `;
    })
    .join("");

  ui.hostBannerTrack.scrollLeft = previousScroll;
  renderBannerDots(ui, hosts.length, activeIndex);
}

/**
 * 渲染 Banner 白点索引。
 * @param {Record<string, HTMLElement>} ui 页面节点集合。
 * @param {number} count 卡片数量。
 * @param {number} activeIndex 当前索引。
 */
export function renderBannerDots(ui, count, activeIndex) {
  if (count <= 0) {
    ui.hostBannerDots.innerHTML = "";
    return;
  }
  const safeIndex = Math.min(Math.max(0, activeIndex), count - 1);
  ui.hostBannerDots.innerHTML = Array.from({ length: count })
    .map((_, idx) => `<span class="host-banner-dot ${idx === safeIndex ? "active" : ""}"></span>`)
    .join("");
}

/**
 * 根据滚动位置计算当前激活卡片索引。
 * @param {HTMLElement} trackElement 横向滚动容器。
 * @returns {{index: number, total: number}|null}
 */
export function deriveBannerActiveIndex(trackElement) {
  const cards = Array.from(trackElement.querySelectorAll(".host-banner-card"));
  if (cards.length === 0) {
    return null;
  }
  const center = trackElement.scrollLeft + trackElement.clientWidth / 2;
  let bestIdx = 0;
  let bestDiff = Number.POSITIVE_INFINITY;
  cards.forEach((card, idx) => {
    const cardCenter = card.offsetLeft + card.offsetWidth / 2;
    const diff = Math.abs(cardCenter - center);
    if (diff < bestDiff) {
      bestDiff = diff;
      bestIdx = idx;
    }
  });
  return { index: bestIdx, total: cards.length };
}
