// 文件职责：
// 1. 渲染宿主机 Banner 与白点索引。
// 2. 处理 Banner 横向滚动与索引更新。

export function renderHostStage(ui, hasHosts) {
  ui.hostSetupCard.style.display = hasHosts ? "none" : "block";
  ui.hostOverviewWrap.classList.toggle("hidden", !hasHosts);
}

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
