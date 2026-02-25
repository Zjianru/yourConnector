// 文件职责：
// 1. 提供聊天会话键与默认结构，避免 flow/view 重复拼装。
// 2. 维护会话级消息与队列的纯状态更新辅助函数。

export const CHAT_QUEUE_LIMIT = 20;

/**
 * 生成会话键：hostId::toolId。
 * @param {string} hostId 宿主机 ID。
 * @param {string} toolId 工具 ID。
 * @returns {string}
 */
export function chatConversationKey(hostId, toolId) {
  const host = String(hostId || "").trim();
  const tool = String(toolId || "").trim();
  if (!host || !tool) return "";
  return `${host}::${tool}`;
}

/**
 * 构建聊天子状态默认值。
 * @returns {object}
 */
export function createChatStateSlice() {
  return {
    activeConversationKey: "",
    viewMode: "list",
    messageViewer: {
      conversationKey: "",
      messageId: "",
      scale: 1,
    },
    queuePanelExpandedByKey: {},
    messageSelectionModeByKey: {},
    selectedMessageIdsByKey: {},
    swipedConversationKey: "",
    conversationsByKey: {},
    conversationOrder: [],
    hydrated: false,
  };
}

/**
 * 获取或创建会话状态。
 * @param {object} chatState 聊天状态树。
 * @param {string} key 会话键。
 * @param {object} meta 会话元信息。
 * @returns {object|null}
 */
export function ensureConversation(chatState, key, meta = {}) {
  const normalizedKey = String(key || "").trim();
  if (!normalizedKey) return null;
  if (!chatState.conversationsByKey[normalizedKey]) {
    chatState.conversationsByKey[normalizedKey] = {
      key: normalizedKey,
      hostId: String(meta.hostId || ""),
      toolId: String(meta.toolId || ""),
      toolClass: String(meta.toolClass || ""),
      hostName: String(meta.hostName || ""),
      toolName: String(meta.toolName || ""),
      updatedAt: String(meta.updatedAt || new Date().toISOString()),
      online: Boolean(meta.online),
      messages: [],
      queue: [],
      running: null,
      draft: "",
      error: "",
    };
  }
  const conv = chatState.conversationsByKey[normalizedKey];
  if (meta.hostId) conv.hostId = String(meta.hostId);
  if (meta.toolId) conv.toolId = String(meta.toolId);
  if (meta.toolClass) conv.toolClass = String(meta.toolClass);
  if (meta.hostName) conv.hostName = String(meta.hostName);
  if (meta.toolName) conv.toolName = String(meta.toolName);
  if ("online" in meta) conv.online = Boolean(meta.online);
  conv.updatedAt = String(meta.updatedAt || conv.updatedAt || new Date().toISOString());
  if (!chatState.conversationOrder.includes(normalizedKey)) {
    chatState.conversationOrder.unshift(normalizedKey);
  }
  return conv;
}

/**
 * 把会话键移动到列表最前。
 * @param {object} chatState 聊天状态树。
 * @param {string} key 会话键。
 */
export function bumpConversationOrder(chatState, key) {
  const normalizedKey = String(key || "").trim();
  if (!normalizedKey) return;
  chatState.conversationOrder = chatState.conversationOrder.filter((item) => item !== normalizedKey);
  chatState.conversationOrder.unshift(normalizedKey);
}

/**
 * 删除会话并维护 activeConversationKey。
 * @param {object} chatState 聊天状态树。
 * @param {string} key 会话键。
 * @returns {boolean} 是否存在并已删除。
 */
export function removeConversation(chatState, key) {
  const normalizedKey = String(key || "").trim();
  if (!normalizedKey || !chatState.conversationsByKey[normalizedKey]) {
    return false;
  }
  delete chatState.conversationsByKey[normalizedKey];
  delete chatState.queuePanelExpandedByKey[normalizedKey];
  delete chatState.messageSelectionModeByKey[normalizedKey];
  delete chatState.selectedMessageIdsByKey[normalizedKey];
  if (String(chatState.swipedConversationKey || "") === normalizedKey) {
    chatState.swipedConversationKey = "";
  }
  if (String(chatState.messageViewer?.conversationKey || "") === normalizedKey) {
    chatState.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
  }
  chatState.conversationOrder = chatState.conversationOrder.filter((item) => item !== normalizedKey);
  if (String(chatState.activeConversationKey || "") === normalizedKey) {
    chatState.activeConversationKey = chatState.conversationOrder[0] || "";
  }
  return true;
}
