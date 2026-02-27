// 文件职责：
// 1. 渲染聊天会话列表与详情区。
// 2. 提供基础 Markdown 安全渲染（段落/列表/代码）。

import { escapeHtml } from "../utils/dom.js";
import { renderMarkdown, normalizeReportPathForPreview } from "../utils/markdown.js";

/**
 * 创建聊天视图渲染器。
 * @param {object} deps 依赖集合。
 * @returns {{renderChat:function}}
 */
export function createChatView({ state, ui }) {
  const AUTO_SCROLL_BOTTOM_GAP_PX = 48;
  const detailScrollState = {
    listenerBound: false,
    pinnedToBottom: true,
    conversationKey: "",
    messageCount: 0,
    latestFingerprint: "",
  };

  function bindChatMessagesScrollListener() {
    if (detailScrollState.listenerBound || !ui.chatMessages) return;
    ui.chatMessages.addEventListener("scroll", () => {
      const el = ui.chatMessages;
      const distanceToBottom = Math.max(0, el.scrollHeight - el.scrollTop - el.clientHeight);
      detailScrollState.pinnedToBottom = distanceToBottom <= AUTO_SCROLL_BOTTOM_GAP_PX;
    }, { passive: true });
    detailScrollState.listenerBound = true;
  }

  function messageTimelineFingerprint(conv) {
    const messages = Array.isArray(conv?.messages) ? conv.messages : [];
    if (messages.length === 0) return "";
    const latest = messages[messages.length - 1] || {};
    return [
      String(latest.id || ""),
      String(latest.status || ""),
      String(latest.ts || ""),
      String((latest.text || "").length),
    ].join("|");
  }

  function maybeAutoScrollChatMessages(conv) {
    if (!conv || !ui.chatMessages) return;
    bindChatMessagesScrollListener();
    const messageCount = Array.isArray(conv.messages) ? conv.messages.length : 0;
    const latestFingerprint = messageTimelineFingerprint(conv);
    const conversationChanged = detailScrollState.conversationKey !== String(conv.key || "");
    const timelineChanged = detailScrollState.messageCount !== messageCount
      || detailScrollState.latestFingerprint !== latestFingerprint;
    const shouldScroll = conversationChanged || (timelineChanged && detailScrollState.pinnedToBottom);

    detailScrollState.conversationKey = String(conv.key || "");
    detailScrollState.messageCount = messageCount;
    detailScrollState.latestFingerprint = latestFingerprint;

    if (!shouldScroll) return;
    requestAnimationFrame(() => {
      if (!ui.chatMessages) return;
      ui.chatMessages.scrollTop = ui.chatMessages.scrollHeight;
      detailScrollState.pinnedToBottom = true;
    });
  }

  function resetDetailScrollState() {
    detailScrollState.conversationKey = "";
    detailScrollState.messageCount = 0;
    detailScrollState.latestFingerprint = "";
    detailScrollState.pinnedToBottom = true;
  }

  function formatTime(raw) {
    const ts = Date.parse(String(raw || ""));
    if (!Number.isFinite(ts)) return "--";
    const date = new Date(ts);
    const hh = String(date.getHours()).padStart(2, "0");
    const mm = String(date.getMinutes()).padStart(2, "0");
    return `${hh}:${mm}`;
  }

  function renderBubbleMarkdown(text) {
    const html = String(renderMarkdown(text || "") || "").trim();
    if (html.startsWith("<p>") && html.endsWith("</p>")) {
      const inner = html.slice(3, -4);
      if (!inner.includes("<p>") && !inner.includes("</p>")) {
        return inner;
      }
    }
    return html;
  }

  function extractMessageReportPaths(msg) {
    const text = normalizeMessageText(msg?.text || "");
    const meta = msg && typeof msg.meta === "object" ? msg.meta : {};
    const rawList = Array.isArray(meta.reportPaths) ? meta.reportPaths : [];
    const paths = [];
    rawList.forEach((rawPath) => {
      const normalizedPath = normalizeReportPathForPreview(rawPath);
      if (!normalizedPath) return;
      if (paths.includes(normalizedPath)) return;
      if (text.includes(normalizedPath)) return;
      paths.push(normalizedPath);
    });
    return paths;
  }

  function reportPathLabel(path) {
    const normalized = String(path || "").trim();
    if (!normalized) return "打开报告";
    const chunks = normalized.split("/");
    const fileName = chunks[chunks.length - 1] || normalized;
    return `打开报告：${fileName}`;
  }

  function renderReportPathLinks(paths) {
    if (!Array.isArray(paths) || paths.length === 0) return "";
    const links = paths
      .map((path) => (
        `<a href="#" class="chat-report-link" data-chat-report-path="${escapeHtml(path)}">${escapeHtml(reportPathLabel(path))}</a>`
      ))
      .join('<span class="chat-report-link-sep">·</span>');
    return `
      <span class="chat-report-links">
        ${links}
      </span>
    `;
  }

  function initials(text) {
    const normalized = String(text || "").trim();
    if (!normalized) return "AI";
    return normalized.slice(0, 2).toUpperCase();
  }

  function normalizeMessageText(text) {
    return String(text || "")
      .replace(/\r\n/g, "\n")
      .replace(/\\n/g, "\n");
  }

  function normalizeMessageContent(rawParts) {
    if (!Array.isArray(rawParts)) return [];
    const out = [];
    rawParts.forEach((raw) => {
      if (!raw || typeof raw !== "object") return;
      const type = String(raw.type || "").trim();
      if (!type) return;
      out.push({
        type,
        text: String(raw.text || ""),
        mediaId: String(raw.mediaId || ""),
        mime: String(raw.mime || ""),
        size: Number(raw.size || 0),
        durationMs: Number(raw.durationMs || 0),
        pathHint: String(raw.pathHint || ""),
        previewUrl: String(raw.previewUrl || ""),
        fileName: String(raw.fileName || ""),
      });
    });
    return out;
  }

  function contentHasNonText(parts) {
    return parts.some((part) => part.type !== "text");
  }

  function contentSummary(parts) {
    const labels = [];
    parts.forEach((part) => {
      if (part.type === "text") return;
      if (part.type === "image") labels.push("图片");
      else if (part.type === "video") labels.push("视频");
      else if (part.type === "audio") labels.push("语音");
      else labels.push("文件");
    });
    return labels.join(" ");
  }

  function formatDurationMs(durationMs) {
    const sec = Math.max(0, Math.round(Number(durationMs || 0) / 1000));
    if (sec <= 0) return "";
    const mm = String(Math.floor(sec / 60)).padStart(2, "0");
    const ss = String(sec % 60).padStart(2, "0");
    return `${mm}:${ss}`;
  }

  function mediaPreviewUrl(part) {
    const preview = String(part.previewUrl || "").trim();
    if (preview) return preview;
    return "";
  }

  function renderMessageRichContent(msg) {
    const parts = normalizeMessageContent(msg?.content);
    if (!parts.length) {
      return {
        html: renderBubbleMarkdown(msg?.text || ""),
        textOnly: normalizeMessageText(msg?.text || ""),
        hasMedia: false,
      };
    }

    const blocks = [];
    const textBlocks = [];
    parts.forEach((part) => {
      if (part.type === "text") {
        const text = String(part.text || "");
        if (!text.trim()) return;
        textBlocks.push(text);
        blocks.push(`<div class="chat-content-block">${renderBubbleMarkdown(text)}</div>`);
        return;
      }
      const hint = escapeHtml(String(part.pathHint || part.fileName || ""));
      const duration = formatDurationMs(part.durationMs);
      const durationHtml = duration ? `<span class="chat-media-duration">${escapeHtml(duration)}</span>` : "";
      const preview = mediaPreviewUrl(part);
      if (part.type === "image" && preview) {
        blocks.push(`
          <div class="chat-content-block chat-media-block">
            <img src="${escapeHtml(preview)}" alt="image" class="chat-media-image" />
            ${hint ? `<div class="chat-media-caption">${hint}</div>` : ""}
          </div>
        `);
        return;
      }
      if (part.type === "video" && preview) {
        blocks.push(`
          <div class="chat-content-block chat-media-block">
            <video src="${escapeHtml(preview)}" class="chat-media-video" controls playsinline></video>
            <div class="chat-media-meta">${hint}${durationHtml}</div>
          </div>
        `);
        return;
      }
      if (part.type === "audio" && preview) {
        blocks.push(`
          <div class="chat-content-block chat-media-block">
            <audio src="${escapeHtml(preview)}" class="chat-media-audio" controls></audio>
            <div class="chat-media-meta">${hint}${durationHtml}</div>
          </div>
        `);
        return;
      }
      const fallbackLabel = part.type === "image"
        ? "图片"
        : (part.type === "video" ? "视频" : (part.type === "audio" ? "语音" : "文件"));
      blocks.push(`
        <div class="chat-content-block">
          <span class="chat-media-fallback">${escapeHtml(fallbackLabel)}${hint ? ` · ${hint}` : ""}</span>
        </div>
      `);
    });

    const textOnly = textBlocks.length > 0
      ? textBlocks.join("\n")
      : contentSummary(parts);
    return {
      html: blocks.join(""),
      textOnly,
      hasMedia: contentHasNonText(parts),
    };
  }

  function renderComposerMediaTray(parts) {
    if (!Array.isArray(parts) || parts.length === 0) {
      return "";
    }
    return parts
      .map((part) => {
        const mediaId = String(part.mediaId || "");
        const hint = escapeHtml(String(part.pathHint || part.fileName || ""));
        const duration = formatDurationMs(part.durationMs);
        const durationHtml = duration ? `<span class="chat-media-duration">${escapeHtml(duration)}</span>` : "";
        const preview = mediaPreviewUrl(part);
        const removeBtn = mediaId
          ? `<button type="button" class="chat-media-remove-btn" data-chat-remove-media="${escapeHtml(mediaId)}">移除</button>`
          : "";
        if (part.type === "image" && preview) {
          return `
            <div class="chat-composer-media-item">
              <img src="${escapeHtml(preview)}" alt="image" class="chat-media-image" />
              <div class="chat-media-meta">${hint}</div>
              ${removeBtn}
            </div>
          `;
        }
        if (part.type === "video" && preview) {
          return `
            <div class="chat-composer-media-item">
              <video src="${escapeHtml(preview)}" class="chat-media-video" muted playsinline></video>
              <div class="chat-media-meta">${hint}${durationHtml}</div>
              ${removeBtn}
            </div>
          `;
        }
        if (part.type === "audio" && preview) {
          return `
            <div class="chat-composer-media-item">
              <audio src="${escapeHtml(preview)}" class="chat-media-audio" controls></audio>
              <div class="chat-media-meta">${hint}${durationHtml}</div>
              ${removeBtn}
            </div>
          `;
        }
        const fallback = part.type === "video" ? "视频" : (part.type === "audio" ? "语音" : "图片");
        return `
          <div class="chat-composer-media-item">
            <span class="chat-media-fallback">${escapeHtml(fallback)}${hint ? ` · ${hint}` : ""}</span>
            ${removeBtn}
          </div>
        `;
      })
      .join("");
  }

  function shouldUseCollapsedPreview(msg, selectionMode) {
    if (selectionMode) return false;
    if (!msg || msg.role === "user") return false;
    const rich = renderMessageRichContent(msg);
    if (rich.hasMedia) return false;
    const normalized = normalizeMessageText(rich.textOnly || "");
    if (!normalized) return false;
    const lines = normalized.split("\n").length;
    return lines > 5 || normalized.length > 260;
  }

  function roleLabel(role) {
    if (role === "user") return "你";
    if (role === "system") return "系统";
    return "助手";
  }

  function conversationStatus(conv) {
    const availability = String(conv?.availability || "").trim().toLowerCase();
    if (availability === "invalid") {
      return { className: "offline", label: "未连接" };
    }
    if (conv?.online) {
      return { className: "online", label: "在线" };
    }
    return { className: "offline", label: "离线" };
  }

  function toggleMainTabs(visible) {
    if (!ui.mainTabs) return;
    ui.mainTabs.classList.toggle("hidden", !visible);
  }

  function renderChat() {
    const chatState = state.chat;
    const conversationKeys = chatState.conversationOrder
      .filter((key) => chatState.conversationsByKey[key])
      .slice(0, 500);
    const conversations = conversationKeys.map((key) => chatState.conversationsByKey[key]);
    const activeKey = String(chatState.activeConversationKey || "");
    const active = chatState.conversationsByKey[activeKey] || null;
    const onChatTab = state.activeTab === "chat";

    if ((chatState.viewMode === "detail" || chatState.viewMode === "message") && !active) {
      chatState.viewMode = "list";
    }
    const onListPage = onChatTab && chatState.viewMode === "list";
    const onDetailPage = onChatTab && chatState.viewMode === "detail" && Boolean(active);
    const onMessagePage = onChatTab && chatState.viewMode === "message" && Boolean(active);
    ui.chatListPage.classList.toggle("active", onListPage);
    ui.chatDetailPage.classList.toggle("active", onDetailPage);
    ui.chatMessagePage.classList.toggle("active", onMessagePage);
    if (ui.appRoot) {
      ui.appRoot.classList.toggle("chat-detail-layout", onDetailPage);
    }
    // 底部主 Tab 仅在聊天详情页隐藏；其它页面（含运维/聊天列表/完整消息页）均展示。
    toggleMainTabs(!onDetailPage);

    if (!conversations.length) {
      ui.chatConversationList.innerHTML = '<div class="empty">暂无会话。先在运维页接入 OpenClaw/OpenCode/Codex/Claude Code 后即可开始聊天。</div>';
    } else {
      ui.chatConversationList.innerHTML = conversations
        .map((conv) => {
          const latest = conv.messages.length > 0 ? conv.messages[conv.messages.length - 1] : null;
          const preview = latest
            ? String(renderMessageRichContent(latest).textOnly || "").slice(0, 48) || "暂无消息"
            : "暂无消息";
          const isActive = conv.key === activeKey;
          const isSwiped = String(chatState.swipedConversationKey || "") === String(conv.key || "");
          const status = conversationStatus(conv);
          return `
            <div class="chat-conversation-row ${isSwiped ? "swiped" : ""}" data-chat-row="${escapeHtml(conv.key)}">
              <div class="chat-conversation-actions">
                <button type="button" class="chat-conversation-clear-btn" data-chat-clear="${escapeHtml(conv.key)}">清空</button>
                <button type="button" class="chat-conversation-clear-btn" data-chat-delete="${escapeHtml(conv.key)}">删除</button>
              </div>
              <button
                type="button"
                class="chat-conversation-item ${isActive ? "active" : ""}"
                data-chat-open="${escapeHtml(conv.key)}"
              >
                <div class="chat-conversation-main">
                  <span class="chat-conversation-avatar">${escapeHtml(initials(conv.toolName || conv.toolId))}</span>
                  <div class="chat-conversation-content">
                    <div class="chat-conversation-title">
                      <span class="chat-conversation-name">${escapeHtml(conv.hostName || conv.hostId || "--")} · ${escapeHtml(conv.toolName || conv.toolId || "--")}</span>
                      <span class="chat-conversation-status ${status.className}">
                        ${status.label}
                      </span>
                    </div>
                    <div class="chat-conversation-preview">${escapeHtml(preview)}</div>
                  </div>
                  <div class="chat-conversation-time">${formatTime(conv.updatedAt)}</div>
                </div>
              </button>
            </div>
          `;
        })
        .join("");
    }

    if (!active) {
      resetDetailScrollState();
      chatState.viewMode = "list";
      toggleMainTabs(true);
      ui.chatOfflineHint.textContent = "";
      ui.chatMessages.innerHTML = '<div class="empty">请选择左侧会话。</div>';
      ui.chatQueueSummary.hidden = true;
      ui.chatQueue.innerHTML = "";
      ui.chatComposerMediaTray.innerHTML = "";
      ui.chatInput.value = "";
      ui.chatInput.disabled = true;
      ui.chatAttachBtn.disabled = true;
      ui.chatRecordBtn.disabled = true;
      ui.chatRecordBtn.textContent = "录音";
      ui.chatRecordStatus.textContent = "";
      ui.chatSendBtn.disabled = true;
      ui.chatStopBtn.classList.add("hidden");
      ui.chatSelectBtn.textContent = "选择";
      ui.chatDeleteSelectedBtn.classList.add("hidden");
      ui.chatDeleteSelectedBtn.disabled = true;
      ui.chatMessageZoomLabel.textContent = "100%";
      ui.chatMessageZoomOutBtn.disabled = false;
      ui.chatMessageZoomInBtn.disabled = false;
      ui.chatMessageFullBody.style.fontSize = "100%";
      return;
    }

    if (onMessagePage) {
      resetDetailScrollState();
      const viewer = chatState.messageViewer || {};
      const sameConversation = String(viewer.conversationKey || "") === String(active.key || "");
      const targetId = String(viewer.messageId || "");
      const target = sameConversation && targetId
        ? active.messages.find((msg) => String(msg.id || "") === targetId)
        : null;
      if (!target) {
        chatState.viewMode = "detail";
        ui.chatMessageTitle.textContent = "完整消息";
        ui.chatMessageMeta.textContent = "";
        ui.chatMessageFullBody.innerHTML = "";
        ui.chatMessageZoomLabel.textContent = "100%";
        ui.chatMessageZoomOutBtn.disabled = false;
        ui.chatMessageZoomInBtn.disabled = false;
        ui.chatMessageFullBody.style.fontSize = "100%";
        return;
      }
      const rawScale = Number(viewer.scale || 1);
      const scale = Number.isFinite(rawScale)
        ? Math.max(0.8, Math.min(1.4, rawScale))
        : 1;
      const scalePercent = Math.round(scale * 100);
      const reportLinks = renderReportPathLinks(extractMessageReportPaths(target));
      const rich = renderMessageRichContent(target);
      ui.chatMessageTitle.textContent = `${roleLabel(target.role)} · 完整消息`;
      ui.chatMessageMeta.textContent = `${active.hostName || active.hostId} / ${active.toolName || active.toolId} · ${formatTime(target.ts)}`;
      ui.chatMessageZoomLabel.textContent = `${scalePercent}%`;
      ui.chatMessageZoomOutBtn.disabled = scale <= 0.8 + 0.001;
      ui.chatMessageZoomInBtn.disabled = scale >= 1.4 - 0.001;
      ui.chatMessageFullBody.style.fontSize = `${scalePercent}%`;
      ui.chatMessageFullBody.innerHTML = `${rich.html || renderMarkdown(target.text || "")}${reportLinks}`;
      return;
    }

    if (!onDetailPage) {
      return;
    }

    ui.chatDetailTitle.textContent = `${active.hostName || active.hostId} / ${active.toolName || active.toolId}`;
    const selectionMode = Boolean(chatState.messageSelectionModeByKey[active.key]);
    const selectedMessageMap = chatState.selectedMessageIdsByKey[active.key] || {};
    const selectedCount = Object.keys(selectedMessageMap).filter((id) => selectedMessageMap[id]).length;
    ui.chatSelectBtn.textContent = selectionMode ? "取消" : "选择";
    ui.chatDeleteSelectedBtn.classList.toggle("hidden", !selectionMode);
    ui.chatDeleteSelectedBtn.disabled = selectedCount === 0;
    ui.chatDeleteSelectedBtn.textContent = selectedCount > 0 ? `删除(${selectedCount})` : "删除";
    const availability = String(active.availability || (active.online ? "online" : "offline")).toLowerCase();
    const isInvalid = availability === "invalid";
    ui.chatOfflineHint.textContent = isInvalid
      ? "当前进程已失效（PID 已变化），请删除卡片后重新接入。"
      : (active.online
        ? (active.running ? "正在生成中，可继续排队消息。" : "")
        : "当前会话离线，请先连接宿主机后发送。");
    ui.chatMessages.innerHTML = active.messages.length > 0
      ? active.messages
        .slice(-400)
        .map((msg) => {
          const status = String(msg.status || "").trim().toLowerCase();
          const messageId = String(msg.id || "");
          const selectable = status !== "streaming" && status !== "sending";
          const selected = selectionMode && messageId && selectedMessageMap[messageId];
          const collapsed = shouldUseCollapsedPreview(msg, selectionMode);
          const rich = renderMessageRichContent(msg);
          const selectClass = selectionMode
            ? `select-mode ${selectable ? "selectable" : "disabled-select"} ${selected ? "selected" : ""}`
            : "";
          const statusText = msg.status && msg.status !== "completed"
            ? `<div class="chat-message-status-row"><span class="chat-message-status">${escapeHtml(msg.status)}</span></div>`
            : "";
          const timeText = formatTime(msg.ts);
          const messageBodyHtml = rich.html || renderBubbleMarkdown(msg.text || "");
          const messageReportLinks = renderReportPathLinks(extractMessageReportPaths(msg));
          return `
          <div
            class="chat-message ${escapeHtml(msg.role || "assistant")} ${escapeHtml(msg.status || "")} ${selectClass}"
            data-chat-message-id="${escapeHtml(messageId)}"
            data-chat-message-selectable="${selectable ? "1" : "0"}"
          >
            <div class="chat-message-body-wrap ${collapsed ? "collapsed" : ""}">
              <div class="chat-message-body markdown-body">
                ${messageBodyHtml}
                ${messageReportLinks}
                ${collapsed ? "" : `<span class="chat-message-time-inline">${timeText}</span>`}
              </div>
              ${collapsed ? '<div class="chat-message-body-fade"></div>' : ""}
            </div>
            ${collapsed ? `<div class="chat-message-time-row"><span class="chat-message-time-inline">${timeText}</span></div>` : ""}
            ${collapsed ? `
              <div class="chat-message-expand-wrap">
                <button
                  type="button"
                  class="chat-message-expand-btn"
                  data-chat-expand-message="${escapeHtml(messageId)}"
                  aria-label="查看完整消息"
                >↩︎</button>
              </div>
            ` : ""}
            ${statusText}
          </div>
        `;
        })
        .join("")
      : '<div class="empty">暂无消息，发送第一条开始。</div>';

    const pendingQueue = Array.isArray(active.queue)
      ? active.queue.filter((item) => {
        if (!active.running) return true;
        return String(item.queueItemId || "") !== String(active.running.queueItemId || "")
          && String(item.requestId || "") !== String(active.running.requestId || "");
      })
      : [];
    const queueCount = pendingQueue.length;
    const queueExpanded = Boolean(chatState.queuePanelExpandedByKey[active.key]);
    if (queueCount > 0) {
      ui.chatQueueSummary.hidden = false;
      ui.chatQueueSummary.setAttribute("aria-expanded", queueExpanded ? "true" : "false");
      ui.chatQueueSummary.textContent = queueExpanded
        ? `收起待发送队列（${queueCount}）`
        : `待发送 ${queueCount} 条`;
      ui.chatQueue.innerHTML = queueExpanded
        ? pendingQueue.map((item) => `
          <div class="chat-queue-item">
            <div class="chat-queue-text">${escapeHtml((String(item.text || "").trim() || contentSummary(normalizeMessageContent(item.content))).slice(0, 120))}</div>
            <button
              type="button"
              class="tool-quick-btn stop"
              data-chat-queue-delete="${escapeHtml(item.queueItemId)}"
            >删除</button>
          </div>
        `).join("")
        : "";
    } else {
      ui.chatQueueSummary.hidden = true;
      ui.chatQueueSummary.setAttribute("aria-expanded", "false");
      ui.chatQueue.innerHTML = "";
    }

    const composerMedia = normalizeMessageContent(chatState.composerMediaByKey[active.key]);
    ui.chatComposerMediaTray.innerHTML = renderComposerMediaTray(composerMedia);
    ui.chatInput.disabled = isInvalid;
    ui.chatInput.value = String(active.draft || "");
    ui.chatAttachBtn.disabled = isInvalid;
    const recordingForActive = String(chatState.recordingConversationKey || "") === String(active.key || "");
    ui.chatRecordBtn.disabled = isInvalid || (Boolean(chatState.recordingPending) && !recordingForActive);
    ui.chatRecordBtn.textContent = recordingForActive ? "停止" : "录音";
    if (chatState.recordingPending) {
      ui.chatRecordStatus.textContent = "录音处理中...";
    } else if (recordingForActive) {
      ui.chatRecordStatus.textContent = "录音中...";
    } else {
      ui.chatRecordStatus.textContent = "";
    }
    const hasDraftPayload = Boolean(String(active.draft || "").trim()) || composerMedia.length > 0;
    ui.chatSendBtn.disabled = isInvalid || !active.online
      || (!hasDraftPayload && !(active.queue.length > 0 && !active.running));
    const showStop = Boolean(active.running);
    ui.chatStopBtn.classList.toggle("hidden", !showStop);
    ui.chatStopBtn.disabled = !showStop;

    maybeAutoScrollChatMessages(active);
  }

  return { renderChat };
}
