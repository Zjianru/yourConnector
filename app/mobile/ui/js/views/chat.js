// 文件职责：
// 1. 渲染聊天会话列表与详情区。
// 2. 提供基础 Markdown 安全渲染（段落/列表/代码）。

import { escapeHtml } from "../utils/dom.js";

/**
 * 创建聊天视图渲染器。
 * @param {object} deps 依赖集合。
 * @returns {{renderChat:function}}
 */
export function createChatView({ state, ui }) {
  function formatTime(raw) {
    const ts = Date.parse(String(raw || ""));
    if (!Number.isFinite(ts)) return "--";
    const date = new Date(ts);
    const hh = String(date.getHours()).padStart(2, "0");
    const mm = String(date.getMinutes()).padStart(2, "0");
    return `${hh}:${mm}`;
  }

  function renderMarkdown(text) {
    const escaped = escapeHtml(String(text || ""))
      .replace(/\r\n/g, "\n")
      .replace(/\\n/g, "\n");

    const codeBlocks = [];
    const withCodeTokens = escaped.replace(/```([\s\S]*?)```/g, (_all, code) => {
      const tokenIndex = codeBlocks.push(
        `<pre><code>${String(code || "").replace(/^\n+|\n+$/g, "")}</code></pre>`,
      ) - 1;
      return `@@CODE_BLOCK_${tokenIndex}@@`;
    });

    function renderInline(line) {
      const inlineCodeTokens = [];
      let output = String(line || "").replace(/`([^`]+)`/g, (_all, code) => {
        const tokenIndex = inlineCodeTokens.push(`<code>${code}</code>`) - 1;
        return `@@INLINE_CODE_${tokenIndex}@@`;
      });
      output = output
        .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
        .replace(/__([^_]+)__/g, "<strong>$1</strong>");
      output = output.replace(/@@INLINE_CODE_(\d+)@@/g, (_all, idx) => {
        const token = inlineCodeTokens[Number(idx)];
        return token || "";
      });
      return output;
    }

    const html = [];
    const lines = withCodeTokens.split("\n");
    let paragraphLines = [];
    let listType = "";
    let listItems = [];

    function flushParagraph() {
      if (!paragraphLines.length) return;
      html.push(`<p>${paragraphLines.map((line) => renderInline(line)).join("<br />")}</p>`);
      paragraphLines = [];
    }

    function flushList() {
      if (!listItems.length) return;
      const tag = listType === "ordered" ? "ol" : "ul";
      html.push(`<${tag}>${listItems.map((item) => `<li>${renderInline(item)}</li>`).join("")}</${tag}>`);
      listItems = [];
      listType = "";
    }

    for (const rawLine of lines) {
      const line = String(rawLine || "");
      const trimmed = line.trim();

      if (!trimmed) {
        flushParagraph();
        flushList();
        continue;
      }

      if (/^@@CODE_BLOCK_\d+@@$/.test(trimmed)) {
        flushParagraph();
        flushList();
        html.push(trimmed);
        continue;
      }

      const heading = trimmed.match(/^(#{1,6})\s+(.+)$/);
      if (heading) {
        flushParagraph();
        flushList();
        const level = heading[1].length;
        html.push(`<h${level}>${renderInline(heading[2])}</h${level}>`);
        continue;
      }

      const unordered = trimmed.match(/^[\-*]\s+(.+)$/);
      const ordered = trimmed.match(/^\d+\.\s+(.+)$/);
      if (unordered || ordered) {
        flushParagraph();
        const nextType = ordered ? "ordered" : "unordered";
        if (listType && listType !== nextType) {
          flushList();
        }
        listType = nextType;
        listItems.push((ordered || unordered)[1]);
        continue;
      }

      if (listItems.length) {
        flushList();
      }
      paragraphLines.push(trimmed);
    }

    flushParagraph();
    flushList();

    return html.join("").replace(/@@CODE_BLOCK_(\d+)@@/g, (_all, idx) => {
      const token = codeBlocks[Number(idx)];
      return token || "";
    });
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

  function shouldUseCollapsedPreview(msg, selectionMode) {
    if (selectionMode) return false;
    if (!msg || msg.role === "user") return false;
    const normalized = normalizeMessageText(msg.text || "");
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
      ui.chatConversationList.innerHTML = '<div class="empty">暂无会话。先在运维页接入 OpenClaw/OpenCode 后即可开始聊天。</div>';
    } else {
      ui.chatConversationList.innerHTML = conversations
        .map((conv) => {
          const latest = conv.messages.length > 0 ? conv.messages[conv.messages.length - 1] : null;
          const preview = latest ? String(latest.text || "").slice(0, 48) : "暂无消息";
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
      chatState.viewMode = "list";
      toggleMainTabs(true);
      ui.chatOfflineHint.textContent = "";
      ui.chatMessages.innerHTML = '<div class="empty">请选择左侧会话。</div>';
      ui.chatQueueSummary.hidden = true;
      ui.chatQueue.innerHTML = "";
      ui.chatInput.value = "";
      ui.chatInput.disabled = true;
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
      ui.chatMessageTitle.textContent = `${roleLabel(target.role)} · 完整消息`;
      ui.chatMessageMeta.textContent = `${active.hostName || active.hostId} / ${active.toolName || active.toolId} · ${formatTime(target.ts)}`;
      ui.chatMessageZoomLabel.textContent = `${scalePercent}%`;
      ui.chatMessageZoomOutBtn.disabled = scale <= 0.8 + 0.001;
      ui.chatMessageZoomInBtn.disabled = scale >= 1.4 - 0.001;
      ui.chatMessageFullBody.style.fontSize = `${scalePercent}%`;
      ui.chatMessageFullBody.innerHTML = renderMarkdown(target.text || "");
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
          const selectClass = selectionMode
            ? `select-mode ${selectable ? "selectable" : "disabled-select"} ${selected ? "selected" : ""}`
            : "";
          const statusText = msg.status && msg.status !== "completed"
            ? `<div class="chat-message-status-row"><span class="chat-message-status">${escapeHtml(msg.status)}</span></div>`
            : "";
          const timeText = formatTime(msg.ts);
          return `
          <div
            class="chat-message ${escapeHtml(msg.role || "assistant")} ${escapeHtml(msg.status || "")} ${selectClass}"
            data-chat-message-id="${escapeHtml(messageId)}"
            data-chat-message-selectable="${selectable ? "1" : "0"}"
          >
            <div class="chat-message-body-wrap ${collapsed ? "collapsed" : ""}">
              <div class="chat-message-body markdown-body">${renderMarkdown(msg.text || "")}</div>
              ${collapsed ? '<div class="chat-message-body-fade"></div>' : ""}
              <span class="chat-message-time-inline">${timeText}</span>
            </div>
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
            <div class="chat-queue-text">${escapeHtml(String(item.text || "").slice(0, 120))}</div>
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

    ui.chatInput.disabled = isInvalid;
    ui.chatInput.value = String(active.draft || "");
    ui.chatSendBtn.disabled = isInvalid || !active.online
      || (!String(active.draft || "").trim() && !(active.queue.length > 0 && !active.running));
    const showStop = Boolean(active.running);
    ui.chatStopBtn.classList.toggle("hidden", !showStop);
    ui.chatStopBtn.disabled = !showStop;
  }

  return { renderChat };
}
