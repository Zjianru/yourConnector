// 文件职责：
// 1. 管理聊天会话状态（队列、运行中、消息流）。
// 2. 对接 WS 聊天事件与 Tauri 文件存储。

import { asMap, asListOfMap } from "../utils/type.js";
import { resolveReportPathFromTarget, normalizeReportPathForPreview } from "../utils/markdown.js";
import {
  CHAT_QUEUE_LIMIT,
  chatConversationKey,
  createChatStateSlice,
  ensureConversation,
  bumpConversationOrder,
  removeConversation,
} from "../state/chat.js";

/**
 * 创建聊天流程控制器。
 * @param {object} deps 依赖集合。
 */
export function createChatFlow({
  state,
  visibleHosts,
  hostById,
  ensureRuntime,
  resolveLogicalToolId,
  resolveRuntimeToolId,
  resolveToolDisplayName,
  sendSocketEvent,
  addLog,
  tauriInvoke,
  render,
}) {
  const persistTimers = {};
  let persistingIndex = false;
  let pendingIndexPersist = false;
  const recordState = {
    recording: false,
    pending: false,
    stream: null,
    recorder: null,
    chunks: [],
    conversationKey: "",
  };

  if (!state.chat || typeof state.chat !== "object") {
    state.chat = createChatStateSlice();
  }
  if (typeof state.chat.viewMode !== "string") {
    state.chat.viewMode = "list";
  }
  if (!state.chat.messageViewer || typeof state.chat.messageViewer !== "object") {
    state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
  }
  if (typeof state.chat.messageViewer.conversationKey !== "string") {
    state.chat.messageViewer.conversationKey = "";
  }
  if (typeof state.chat.messageViewer.messageId !== "string") {
    state.chat.messageViewer.messageId = "";
  }
  if (!Number.isFinite(Number(state.chat.messageViewer.scale))) {
    state.chat.messageViewer.scale = 1;
  }
  if (!state.chat.queuePanelExpandedByKey || typeof state.chat.queuePanelExpandedByKey !== "object") {
    state.chat.queuePanelExpandedByKey = {};
  }
  if (!state.chat.composerMediaByKey || typeof state.chat.composerMediaByKey !== "object") {
    state.chat.composerMediaByKey = {};
  }
  if (typeof state.chat.recordingConversationKey !== "string") {
    state.chat.recordingConversationKey = "";
  }
  if (typeof state.chat.recordingPending !== "boolean") {
    state.chat.recordingPending = false;
  }
  if (!state.chat.messageSelectionModeByKey || typeof state.chat.messageSelectionModeByKey !== "object") {
    state.chat.messageSelectionModeByKey = {};
  }
  if (!state.chat.selectedMessageIdsByKey || typeof state.chat.selectedMessageIdsByKey !== "object") {
    state.chat.selectedMessageIdsByKey = {};
  }
  if (typeof state.chat.swipedConversationKey !== "string") {
    state.chat.swipedConversationKey = "";
  }
  if (!state.chat.reportViewer || typeof state.chat.reportViewer !== "object") {
    state.chat.reportViewer = {
      visible: false,
      conversationKey: "",
      hostId: "",
      toolId: "",
      requestId: "",
      filePath: "",
      content: "",
      status: "idle",
      error: "",
      bytesSent: 0,
      bytesTotal: 0,
    };
  }
  if (!state.chat.reportTransfersByRequestId || typeof state.chat.reportTransfersByRequestId !== "object") {
    state.chat.reportTransfersByRequestId = {};
  }

  let suppressOpenUntil = 0;
  const swipeState = {
    key: "",
    startX: 0,
    startY: 0,
    active: false,
  };

  function createId(prefix) {
    if (window.crypto && typeof window.crypto.randomUUID === "function") {
      return `${prefix}_${window.crypto.randomUUID()}`;
    }
    return `${prefix}_${Date.now()}_${Math.random().toString(36).slice(2, 10)}`;
  }

  function createReportViewerState(overrides = {}) {
    return {
      visible: false,
      conversationKey: "",
      hostId: "",
      toolId: "",
      requestId: "",
      filePath: "",
      content: "",
      status: "idle",
      error: "",
      bytesSent: 0,
      bytesTotal: 0,
      ...overrides,
    };
  }

  function syncRecordState() {
    state.chat.recordingConversationKey = String(recordState.conversationKey || "");
    state.chat.recordingPending = Boolean(recordState.pending);
  }

  function inferMediaType(mime, fileName = "") {
    const normalizedMime = String(mime || "").toLowerCase();
    if (normalizedMime.startsWith("image/")) return "image";
    if (normalizedMime.startsWith("video/")) return "video";
    if (normalizedMime.startsWith("audio/")) return "audio";
    const lowerName = String(fileName || "").toLowerCase();
    if (/\.(png|jpg|jpeg|gif|webp|bmp|heic|heif|svg)$/.test(lowerName)) return "image";
    if (/\.(mp4|mov|m4v|webm|mkv|avi)$/.test(lowerName)) return "video";
    if (/\.(m4a|mp3|wav|ogg|webm|aac|flac)$/.test(lowerName)) return "audio";
    return "";
  }

  function normalizeContentPart(rawPart) {
    const part = asMap(rawPart);
    const type = String(part.type || "").trim().toLowerCase();
    if (!type) return null;

    if (type === "text") {
      const text = String(part.text || "").trim();
      if (!text) return null;
      return { type: "text", text };
    }

    if (type === "fileref") {
      const pathHint = String(part.pathHint || "").trim();
      const text = String(part.text || "").trim();
      if (!pathHint && !text) return null;
      const normalized = { type: "fileRef" };
      if (pathHint) normalized.pathHint = pathHint;
      if (text) normalized.text = text;
      return normalized;
    }

    if (type !== "image" && type !== "video" && type !== "audio") {
      return null;
    }

    const normalized = { type };
    const mediaId = String(part.mediaId || "").trim();
    const mime = String(part.mime || "").trim();
    const size = Number(part.size || 0);
    const durationMs = Number(part.durationMs || 0);
    const pathHint = String(part.pathHint || "").trim();
    const text = String(part.text || "").trim();
    const dataBase64 = String(part.dataBase64 || "").trim();
    const previewUrl = String(part.previewUrl || "").trim();
    const fileName = String(part.fileName || "").trim();

    if (mediaId) normalized.mediaId = mediaId;
    if (mime) normalized.mime = mime;
    if (Number.isFinite(size) && size > 0) normalized.size = Math.trunc(size);
    if (Number.isFinite(durationMs) && durationMs > 0) normalized.durationMs = Math.trunc(durationMs);
    if (pathHint) normalized.pathHint = pathHint;
    if (text) normalized.text = text;
    if (dataBase64) normalized.dataBase64 = dataBase64;
    if (previewUrl) normalized.previewUrl = previewUrl;
    if (fileName) normalized.fileName = fileName;
    return normalized;
  }

  function normalizeContentParts(rawParts) {
    if (!Array.isArray(rawParts)) return [];
    const out = [];
    rawParts.forEach((item) => {
      const normalized = normalizeContentPart(item);
      if (normalized) out.push(normalized);
    });
    return out;
  }

  function cloneContentParts(rawParts) {
    return normalizeContentParts(rawParts).map((part) => ({ ...part }));
  }

  function contentSummaryText(rawParts) {
    const parts = normalizeContentParts(rawParts);
    if (!parts.length) return "";
    const lines = [];
    const mediaLines = [];
    parts.forEach((part) => {
      if (part.type === "text") {
        lines.push(String(part.text || "").trim());
        return;
      }
      if (part.type === "fileRef") {
        const label = String(part.pathHint || part.text || "").trim();
        mediaLines.push(label ? `文件：${label}` : "文件引用");
        return;
      }
      const labelByType = part.type === "image"
        ? "图片"
        : (part.type === "video" ? "视频" : "语音");
      const hint = String(part.pathHint || part.fileName || "").trim();
      mediaLines.push(hint ? `${labelByType}：${hint}` : labelByType);
    });
    const text = lines.filter(Boolean).join("\n").trim();
    if (!mediaLines.length) return text;
    const mediaSummary = mediaLines.map((item) => `- ${item}`).join("\n");
    if (!text) return `已发送附件：\n${mediaSummary}`;
    return `${text}\n\n附件：\n${mediaSummary}`;
  }

  function stripTransientContentFields(rawParts) {
    return normalizeContentParts(rawParts).map((part) => {
      const next = { ...part };
      delete next.dataBase64;
      delete next.previewUrl;
      delete next.fileName;
      return next;
    });
  }

  function composerMediaForKey(conversationKey) {
    const key = String(conversationKey || "").trim();
    if (!key) return [];
    if (!Array.isArray(state.chat.composerMediaByKey[key])) {
      state.chat.composerMediaByKey[key] = [];
    }
    return state.chat.composerMediaByKey[key];
  }

  function activeComposerMedia() {
    const conv = activeConversation();
    if (!conv) return [];
    return composerMediaForKey(conv.key);
  }

  function clearComposerMediaByKey(conversationKey) {
    const key = String(conversationKey || "").trim();
    if (!key) return;
    delete state.chat.composerMediaByKey[key];
  }

  async function readBlobAsBase64(blob) {
    const buffer = await blob.arrayBuffer();
    let binary = "";
    const bytes = new Uint8Array(buffer);
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
    }
    return window.btoa(binary);
  }

  async function readMediaDurationMs(blobUrl, mediaType) {
    if (mediaType !== "audio" && mediaType !== "video") {
      return 0;
    }
    return new Promise((resolve) => {
      const node = document.createElement(mediaType);
      let done = false;
      const timer = setTimeout(() => {
        if (done) return;
        done = true;
        resolve(0);
      }, 4000);

      node.preload = "metadata";
      node.src = blobUrl;
      node.onloadedmetadata = () => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        const duration = Number(node.duration || 0);
        resolve(Number.isFinite(duration) && duration > 0 ? Math.trunc(duration * 1000) : 0);
      };
      node.onerror = () => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        resolve(0);
      };
    });
  }

  async function buildComposerMediaPartFromBlob({ blob, mime, fileName, pathHint }) {
    const mediaType = inferMediaType(mime, fileName);
    if (!mediaType) {
      throw new Error("仅支持图片/视频/语音文件");
    }
    const previewUrl = URL.createObjectURL(blob);
    const durationMs = await readMediaDurationMs(previewUrl, mediaType);
    const dataBase64 = await readBlobAsBase64(blob);
    return normalizeContentPart({
      type: mediaType,
      mediaId: createId("media"),
      mime: String(mime || blob.type || "").trim(),
      size: Number(blob.size || 0),
      durationMs,
      pathHint: String(pathHint || fileName || "").trim(),
      fileName: String(fileName || "").trim(),
      previewUrl,
      dataBase64,
    });
  }

  function stopRecordStreamTracks() {
    if (!recordState.stream) return;
    recordState.stream.getTracks().forEach((track) => {
      try {
        track.stop();
      } catch (_) {
        // ignore
      }
    });
    recordState.stream = null;
  }

  function resetRecordState() {
    recordState.recording = false;
    recordState.pending = false;
    recordState.recorder = null;
    recordState.chunks = [];
    recordState.conversationKey = "";
    stopRecordStreamTracks();
    syncRecordState();
  }

  function removeComposerMediaItem(partId) {
    const conv = activeConversation();
    if (!conv) return;
    const key = conv.key;
    const list = composerMediaForKey(key);
    const normalizedId = String(partId || "").trim();
    if (!normalizedId) return;
    const next = list.filter((item) => String(item.mediaId || "") !== normalizedId);
    state.chat.composerMediaByKey[key] = next;
    render();
  }

  async function onComposerMediaPicked(inputElement) {
    const conv = activeConversation();
    const files = Array.from(inputElement?.files || []);
    if (inputElement) {
      inputElement.value = "";
    }
    if (!conv || files.length === 0) {
      return;
    }

    const current = composerMediaForKey(conv.key);
    const limit = 6;
    if (current.length >= limit) {
      conv.error = `附件上限 ${limit} 个`;
      render();
      return;
    }

    const maxBytes = 25 * 1024 * 1024;
    const nextItems = [...current];
    for (const file of files) {
      if (nextItems.length >= limit) break;
      const mime = String(file.type || "").trim();
      const mediaType = inferMediaType(mime, file.name);
      if (!mediaType) {
        conv.error = "仅支持图片/视频/语音文件";
        continue;
      }
      if (Number(file.size || 0) <= 0) {
        conv.error = "附件大小为 0，无法发送";
        continue;
      }
      if (Number(file.size || 0) > maxBytes) {
        conv.error = "单个附件超过 25MB，请压缩后重试";
        continue;
      }
      try {
        const part = await buildComposerMediaPartFromBlob({
          blob: file,
          mime,
          fileName: file.name,
          pathHint: file.name,
        });
        if (part) nextItems.push(part);
      } catch (error) {
        conv.error = `附件处理失败：${error}`;
      }
    }
    state.chat.composerMediaByKey[conv.key] = nextItems;
    touchConversation(conv, { persist: false });
    render();
  }

  async function startVoiceRecord() {
    if (recordState.pending || recordState.recording) return;
    const conv = activeConversation();
    if (!conv) return;
    if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== "function") {
      conv.error = "当前环境不支持录音";
      render();
      return;
    }
    try {
      recordState.pending = true;
      syncRecordState();
      render();
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const preferredMime = window.MediaRecorder && typeof window.MediaRecorder.isTypeSupported === "function"
        && window.MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
        ? "audio/webm;codecs=opus"
        : "";
      const recorder = preferredMime
        ? new MediaRecorder(stream, { mimeType: preferredMime })
        : new MediaRecorder(stream);
      recordState.stream = stream;
      recordState.recorder = recorder;
      recordState.chunks = [];
      recordState.conversationKey = conv.key;
      recordState.recording = true;
      recordState.pending = false;
      syncRecordState();

      recorder.ondataavailable = (event) => {
        if (event.data && event.data.size > 0) {
          recordState.chunks.push(event.data);
        }
      };
      recorder.onstop = async () => {
        const chunks = Array.isArray(recordState.chunks) ? [...recordState.chunks] : [];
        const targetConversationKey = String(recordState.conversationKey || "");
        const mime = String(recorder.mimeType || "audio/webm").trim();
        resetRecordState();
        if (!chunks.length || !targetConversationKey) {
          render();
          return;
        }
        const target = state.chat.conversationsByKey[targetConversationKey];
        if (!target) {
          render();
          return;
        }
        try {
          const blob = new Blob(chunks, { type: mime || "audio/webm" });
          const part = await buildComposerMediaPartFromBlob({
            blob,
            mime: blob.type || mime,
            fileName: `voice-${new Date().toISOString().replace(/[:.]/g, "-")}.webm`,
            pathHint: "语音消息",
          });
          if (part) {
            const list = composerMediaForKey(targetConversationKey);
            list.push(part);
            state.chat.composerMediaByKey[targetConversationKey] = list;
            target.error = "";
          }
        } catch (error) {
          target.error = `录音处理失败：${error}`;
        }
        render();
      };
      recorder.start();
      render();
    } catch (error) {
      resetRecordState();
      conv.error = `无法开始录音：${error}`;
      render();
    }
  }

  function stopVoiceRecord() {
    if (!recordState.recording || !recordState.recorder) {
      return;
    }
    try {
      recordState.recording = false;
      recordState.pending = true;
      syncRecordState();
      recordState.recorder.stop();
    } catch (_) {
      resetRecordState();
    }
    render();
  }

  function toggleVoiceRecord() {
    if (recordState.recording) {
      stopVoiceRecord();
      return;
    }
    void startVoiceRecord();
  }

  function stopVoiceRecordForConversation(conversationKey) {
    const key = String(conversationKey || "").trim();
    if (!key) return;
    if (String(recordState.conversationKey || "") !== key) return;
    if (recordState.recording) {
      stopVoiceRecord();
      return;
    }
    resetRecordState();
  }

  function isChatTool(tool) {
    const toolClass = String(tool?.toolClass || "").trim().toLowerCase();
    return toolClass === "assistant" || toolClass === "code";
  }

  function mapLogicalToolId(hostId, toolId) {
    const raw = String(toolId || "").trim();
    if (!raw) return "";
    if (typeof resolveLogicalToolId === "function") {
      return String(resolveLogicalToolId(hostId, raw) || "").trim() || raw;
    }
    return raw;
  }

  function mapRuntimeToolId(hostId, toolId) {
    const raw = String(toolId || "").trim();
    if (!raw) return "";
    if (typeof resolveRuntimeToolId === "function") {
      return String(resolveRuntimeToolId(hostId, raw) || "").trim() || raw;
    }
    return raw;
  }

  function isOpenCodeConversation(conv) {
    const toolId = String(conv?.toolId || "").toLowerCase();
    const toolName = String(conv?.toolName || "").toLowerCase();
    return toolId.startsWith("opencode_") || toolName.includes("opencode");
  }

  function resolveConversationRuntimeToolId(conv) {
    if (!conv) return "";
    const hostId = String(conv.hostId || "").trim();
    if (!hostId) return "";
    const logicalToolId = String(conv.toolId || "").trim();
    const mapped = mapRuntimeToolId(hostId, logicalToolId);
    if (mapped && mapped !== logicalToolId) return mapped;
    if (String(conv.runtimeToolId || "").trim()) return String(conv.runtimeToolId || "").trim();
    const runtime = ensureRuntime(hostId);
    if (!runtime || !Array.isArray(runtime.tools)) return mapped || logicalToolId;
    const found = runtime.tools.find((tool) => String(tool.toolId || "") === logicalToolId);
    return String(found?.runtimeToolId || found?.toolId || mapped || logicalToolId || "").trim();
  }

  function finalizeInterruptedMessages(conv, reason) {
    if (!conv) return false;
    let changed = false;
    for (const msg of Array.isArray(conv.messages) ? conv.messages : []) {
      const status = String(msg.status || "").trim().toLowerCase();
      if (status === "streaming" || status === "sending") {
        msg.status = "interrupted";
        if (reason && !String(msg.text || "").trim() && msg.role !== "user") {
          msg.text = reason;
        }
        changed = true;
      }
    }
    return changed;
  }

  function isToolOnline(runtime, tool) {
    if (!runtime || !runtime.connected || !tool) return false;
    const status = String(tool.status || "").trim().toLowerCase();
    return Boolean(tool.connected) && status !== "offline" && status !== "invalid";
  }

  function toPersistedConversation(conv) {
    const runningAsQueue = conv.running
      ? [{
        queueItemId: String(conv.running.queueItemId || ""),
        requestId: String(conv.running.requestId || ""),
        text: String(conv.running.text || ""),
        content: stripTransientContentFields(conv.running.content),
        createdAt: String(conv.running.startedAt || conv.updatedAt || new Date().toISOString()),
      }]
      : [];
    const queue = [...runningAsQueue, ...(Array.isArray(conv.queue) ? conv.queue : [])]
      .filter((item) => item && item.queueItemId && item.requestId)
      .map((item) => ({
        ...item,
        text: String(item.text || ""),
        content: stripTransientContentFields(item.content),
      }));
    const messages = Array.isArray(conv.messages)
      ? conv.messages.map((msg) => ({
        ...msg,
        text: String(msg.text || ""),
        content: stripTransientContentFields(msg.content),
      }))
      : [];
    return {
      key: String(conv.key || ""),
      hostId: String(conv.hostId || ""),
      toolId: String(conv.toolId || ""),
      runtimeToolId: String(conv.runtimeToolId || ""),
      toolClass: String(conv.toolClass || ""),
      hostName: String(conv.hostName || ""),
      toolName: String(conv.toolName || ""),
      availability: String(conv.availability || "offline"),
      updatedAt: String(conv.updatedAt || ""),
      messages,
      queue,
      draft: String(conv.draft || ""),
      error: String(conv.error || ""),
    };
  }

  function restoreConversation(rawConv) {
    const conv = asMap(rawConv);
    const key = String(conv.key || "");
    if (!key) return null;
    return {
      key,
      hostId: String(conv.hostId || ""),
      toolId: String(conv.toolId || ""),
      runtimeToolId: String(conv.runtimeToolId || ""),
      toolClass: String(conv.toolClass || ""),
      hostName: String(conv.hostName || ""),
      toolName: String(conv.toolName || ""),
      availability: String(conv.availability || "offline"),
      updatedAt: String(conv.updatedAt || new Date().toISOString()),
      online: false,
      messages: Array.isArray(conv.messages)
        ? conv.messages.map((msg) => ({
          ...msg,
          text: String(msg.text || ""),
          content: stripTransientContentFields(msg.content),
        }))
        : [],
      queue: Array.isArray(conv.queue)
        ? conv.queue.map((item) => ({
          ...item,
          text: String(item.text || ""),
          content: stripTransientContentFields(item.content),
        }))
        : [],
      running: null,
      draft: String(conv.draft || ""),
      error: String(conv.error || ""),
    };
  }

  function buildPersistedIndex() {
    const byKey = {};
    Object.entries(state.chat.conversationsByKey).forEach(([key, rawConv]) => {
      byKey[key] = toPersistedConversation(rawConv);
    });
    return {
      schemaVersion: 2,
      activeConversationKey: String(state.chat.activeConversationKey || ""),
      conversationOrder: Array.isArray(state.chat.conversationOrder)
        ? state.chat.conversationOrder
        : [],
      conversationsByKey: byKey,
    };
  }

  async function persistIndex() {
    if (persistingIndex) {
      pendingIndexPersist = true;
      return;
    }
    persistingIndex = true;
    try {
      await tauriInvoke("chat_store_upsert_index", { index: buildPersistedIndex() });
    } catch (error) {
      addLog(`chat persist index failed: ${error}`, {
        level: "warn",
        scope: "chat",
        action: "persist_index",
        outcome: "failed",
      });
    } finally {
      persistingIndex = false;
      if (pendingIndexPersist) {
        pendingIndexPersist = false;
        void persistIndex();
      }
    }
  }

  function schedulePersistConversation(key) {
    const normalizedKey = String(key || "").trim();
    if (!normalizedKey) return;
    if (persistTimers[normalizedKey]) {
      clearTimeout(persistTimers[normalizedKey]);
    }
    persistTimers[normalizedKey] = setTimeout(async () => {
      delete persistTimers[normalizedKey];
      const conv = state.chat.conversationsByKey[normalizedKey];
      if (!conv) return;
      try {
        await tauriInvoke("chat_store_append_events", {
          conversationKey: normalizedKey,
          events: [{
            type: "snapshot",
            ts: new Date().toISOString(),
            conversation: toPersistedConversation(conv),
          }],
        });
      } catch (error) {
        addLog(`chat append snapshot failed: ${error}`, {
          level: "warn",
          scope: "chat",
          action: "append_snapshot",
          outcome: "failed",
          detail: String(error || ""),
        });
      }
      await persistIndex();
    }, 260);
  }

  function touchConversation(conv, { persist = true } = {}) {
    normalizeMessageSelectionByConversation(conv);
    conv.updatedAt = new Date().toISOString();
    bumpConversationOrder(state.chat, conv.key);
    if (persist) {
      schedulePersistConversation(conv.key);
    }
  }

  function findMessage(conv, requestId, role) {
    return conv.messages.find(
      (item) => String(item.requestId || "") === String(requestId || "")
        && String(item.role || "") === String(role || ""),
    );
  }

  function removeQueueItems(conv, { queueItemId = "", requestId = "" } = {}) {
    if (!conv || !Array.isArray(conv.queue)) return false;
    const normalizedQueueItemId = String(queueItemId || "").trim();
    const normalizedRequestId = String(requestId || "").trim();
    if (!normalizedQueueItemId && !normalizedRequestId) return false;
    const before = conv.queue.length;
    conv.queue = conv.queue.filter((item) => {
      const itemQueueItemId = String(item.queueItemId || "");
      const itemRequestId = String(item.requestId || "");
      if (normalizedQueueItemId && itemQueueItemId === normalizedQueueItemId) return false;
      if (normalizedRequestId && itemRequestId === normalizedRequestId) return false;
      return true;
    });
    const changed = conv.queue.length !== before;
    if (changed) {
      normalizeQueuePanelByConversation(conv);
    }
    return changed;
  }

  function isMessageSelectable(msg) {
    const status = String(msg?.status || "").trim().toLowerCase();
    return status !== "streaming" && status !== "sending";
  }

  function selectedMessageIds(conversationKey) {
    const key = String(conversationKey || "").trim();
    if (!key) return [];
    const raw = asMap(state.chat.selectedMessageIdsByKey[key]);
    return Object.keys(raw).filter((id) => raw[id]);
  }

  function clearMessageSelection(conversationKey) {
    const key = String(conversationKey || "").trim();
    if (!key) return;
    delete state.chat.messageSelectionModeByKey[key];
    delete state.chat.selectedMessageIdsByKey[key];
  }

  function normalizeMessageSelectionByConversation(conv) {
    if (!conv || !conv.key) return;
    const key = conv.key;
    if (!state.chat.messageSelectionModeByKey[key]) {
      delete state.chat.selectedMessageIdsByKey[key];
      return;
    }
    const raw = asMap(state.chat.selectedMessageIdsByKey[key]);
    const valid = {};
    const existing = new Set(
      (Array.isArray(conv.messages) ? conv.messages : [])
        .filter((msg) => isMessageSelectable(msg))
        .map((msg) => String(msg.id || "").trim())
        .filter(Boolean),
    );
    Object.keys(raw).forEach((messageId) => {
      const normalizedId = String(messageId || "").trim();
      if (raw[messageId] && existing.has(normalizedId)) {
        valid[normalizedId] = true;
      }
    });
    if (Object.keys(valid).length > 0) {
      state.chat.selectedMessageIdsByKey[key] = valid;
      return;
    }
    clearMessageSelection(key);
  }

  function setSwipedConversation(conversationKey) {
    const key = String(conversationKey || "").trim();
    if (state.chat.swipedConversationKey === key) {
      return false;
    }
    state.chat.swipedConversationKey = key;
    suppressOpenUntil = Date.now() + 220;
    return true;
  }

  function openMessageViewer(messageId) {
    const conv = activeConversation();
    const normalizedMessageId = String(messageId || "").trim();
    if (!conv || !normalizedMessageId) return;
    const found = (Array.isArray(conv.messages) ? conv.messages : [])
      .some((msg) => String(msg.id || "") === normalizedMessageId);
    if (!found) return;
    state.chat.messageViewer = {
      conversationKey: conv.key,
      messageId: normalizedMessageId,
      scale: 1,
    };
    state.chat.viewMode = "message";
    render();
  }

  function closeMessageViewer() {
    const conv = activeConversation();
    state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
    if (conv) {
      state.chat.viewMode = "detail";
    } else {
      state.chat.viewMode = "list";
    }
    render();
  }

  function zoomMessageViewer(delta) {
    if (state.chat.viewMode !== "message") return;
    const current = Number(state.chat.messageViewer.scale || 1);
    const next = Number((current + delta).toFixed(2));
    const clamped = Math.max(0.8, Math.min(1.4, next));
    if (Math.abs(clamped - current) < 0.001) return;
    state.chat.messageViewer.scale = clamped;
    render();
  }

  function setQueuePanelExpanded(conversationKey, expanded) {
    const key = String(conversationKey || "").trim();
    if (!key) return;
    if (expanded) {
      state.chat.queuePanelExpandedByKey[key] = true;
      return;
    }
    delete state.chat.queuePanelExpandedByKey[key];
  }

  function normalizeQueuePanelByConversation(conv) {
    if (!conv || !conv.key) return;
    if (!Array.isArray(conv.queue) || conv.queue.length === 0) {
      setQueuePanelExpanded(conv.key, false);
    }
  }

  function normalizeConversationOnlineState() {
    const onlineKeys = new Set();
    const invalidKeys = new Set();
    const runtimeToolIdByKey = {};

    visibleHosts().forEach((host) => {
      const runtime = ensureRuntime(host.hostId);
      if (!runtime) return;
      const tools = Array.isArray(runtime.tools) ? runtime.tools : [];
      tools.filter(isChatTool).forEach((tool) => {
        const logicalToolId = mapLogicalToolId(host.hostId, tool.toolId);
        const key = chatConversationKey(host.hostId, logicalToolId);
        if (!key) return;
        const online = isToolOnline(runtime, tool);
        const invalid = String(tool.status || "").trim().toLowerCase() === "invalid"
          || Boolean(tool.invalidPidChanged);
        const runtimeToolId = String(tool.runtimeToolId || tool.toolId || "").trim();
        const conv = ensureConversation(state.chat, key, {
          hostId: host.hostId,
          toolId: logicalToolId,
          runtimeToolId,
          toolClass: String(tool.toolClass || ""),
          hostName: String(host.displayName || host.hostId),
          toolName: resolveToolDisplayName(host.hostId, tool),
          online,
          availability: invalid ? "invalid" : (online ? "online" : "offline"),
        });
        if (conv) {
          conv.online = online;
          conv.runtimeToolId = runtimeToolId;
          conv.availability = invalid ? "invalid" : (online ? "online" : "offline");
          if (online) onlineKeys.add(key);
          if (invalid) invalidKeys.add(key);
          if (runtimeToolId) runtimeToolIdByKey[key] = runtimeToolId;
        }
      });
    });

    Object.values(state.chat.conversationsByKey).forEach((conv) => {
      const isOnline = onlineKeys.has(conv.key);
      const isInvalid = invalidKeys.has(conv.key);
      if (!isOnline && conv.running && conv.running.requestId && conv.running.queueItemId) {
        const exists = conv.queue.some((item) => item.queueItemId === conv.running.queueItemId);
        if (!exists) {
          conv.queue.unshift({
            queueItemId: conv.running.queueItemId,
            requestId: conv.running.requestId,
            text: String(conv.running.text || ""),
            content: stripTransientContentFields(conv.running.content),
            createdAt: String(conv.running.startedAt || new Date().toISOString()),
          });
        }
        conv.running = null;
      }
      if (!isOnline) {
        const interrupted = finalizeInterruptedMessages(conv, "连接中断，消息输出已中止。");
        if (interrupted && !conv.error) {
          conv.error = "连接中断，消息输出已中止。";
        }
      }
      if (runtimeToolIdByKey[conv.key]) {
        conv.runtimeToolId = runtimeToolIdByKey[conv.key];
      }
      conv.online = isOnline;
      conv.availability = isInvalid ? "invalid" : (isOnline ? "online" : "offline");
    });

    if (!state.chat.activeConversationKey) {
      const firstKey = state.chat.conversationOrder[0] || "";
      if (firstKey) state.chat.activeConversationKey = firstKey;
    }
  }

  function activeConversation() {
    const key = String(state.chat.activeConversationKey || "");
    if (!key) return null;
    return state.chat.conversationsByKey[key] || null;
  }

  function openConversation(key) {
    const normalizedKey = String(key || "").trim();
    if (!normalizedKey || !state.chat.conversationsByKey[normalizedKey]) return;
    const current = activeConversation();
    if (current && current.key !== normalizedKey) {
      stopVoiceRecordForConversation(current.key);
    }
    state.chat.swipedConversationKey = "";
    state.chat.activeConversationKey = normalizedKey;
    state.chat.viewMode = "detail";
    state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
    clearMessageSelection(normalizedKey);
    void persistIndex();
    void hydrateConversationFromLog(normalizedKey);
    render();
  }

  function backToList() {
    const conv = activeConversation();
    if (conv) clearMessageSelection(conv.key);
    if (conv) stopVoiceRecordForConversation(conv.key);
    state.chat.swipedConversationKey = "";
    state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
    state.chat.viewMode = "list";
    render();
  }

  function enterChatTab() {
    const conv = activeConversation();
    if (conv) clearMessageSelection(conv.key);
    if (conv) stopVoiceRecordForConversation(conv.key);
    state.chat.swipedConversationKey = "";
    state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
    state.chat.viewMode = "list";
  }

  async function deleteConversationByKey(conversationKey, { deleteStore = false } = {}) {
    const normalizedKey = String(conversationKey || "").trim();
    if (!normalizedKey) return false;

    if (persistTimers[normalizedKey]) {
      clearTimeout(persistTimers[normalizedKey]);
      delete persistTimers[normalizedKey];
    }
    const removed = removeConversation(state.chat, normalizedKey);
    if (!removed) return false;
    stopVoiceRecordForConversation(normalizedKey);
    clearComposerMediaByKey(normalizedKey);
    if (String(state.chat.messageViewer.conversationKey || "") === normalizedKey) {
      state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
    }

    if (state.chat.conversationOrder.length === 0) {
      state.chat.viewMode = "list";
    }
    render();

    if (deleteStore) {
      try {
        await tauriInvoke("chat_store_delete_conversation", {
          conversationKey: normalizedKey,
        });
      } catch (error) {
        addLog(`chat delete conversation failed: ${error}`, {
          level: "warn",
          scope: "chat",
          action: "delete_conversation",
          outcome: "failed",
          detail: String(error || ""),
        });
      }
    }

    await persistIndex();
    return true;
  }

  async function deleteConversationByTool(hostId, toolId, { deleteStore = true } = {}) {
    const logicalToolId = mapLogicalToolId(hostId, toolId);
    const key = chatConversationKey(hostId, logicalToolId);
    if (!key) return false;
    return deleteConversationByKey(key, { deleteStore });
  }

  async function deleteConversationsByHost(hostId, { deleteStore = true } = {}) {
    const normalizedHostId = String(hostId || "").trim();
    if (!normalizedHostId) return 0;
    const keys = Object.values(state.chat.conversationsByKey)
      .filter((conv) => String(conv.hostId || "") === normalizedHostId)
      .map((conv) => String(conv.key || ""))
      .filter(Boolean);
    let removedCount = 0;
    for (const key of keys) {
      // eslint-disable-next-line no-await-in-loop
      const removed = await deleteConversationByKey(key, { deleteStore });
      if (removed) removedCount += 1;
    }
    return removedCount;
  }

  async function clearConversationMessages(conversationKey) {
    const normalizedKey = String(conversationKey || "").trim();
    const conv = state.chat.conversationsByKey[normalizedKey];
    if (!normalizedKey || !conv) return false;
    const conversationLabel = `${conv.hostName || conv.hostId || "--"} · ${conv.toolName || conv.toolId || "--"}`;
    const confirmed = window.confirm(`确认清空「${conversationLabel}」的聊天记录吗？此操作不可恢复。`);
    if (!confirmed) return false;

    if (persistTimers[normalizedKey]) {
      clearTimeout(persistTimers[normalizedKey]);
      delete persistTimers[normalizedKey];
    }

    try {
      await tauriInvoke("chat_store_delete_conversation", {
        conversationKey: normalizedKey,
      });
    } catch (error) {
      addLog(`chat clear conversation storage failed: ${error}`, {
        level: "warn",
        scope: "chat",
        action: "clear_conversation",
        outcome: "failed",
        detail: String(error || ""),
      });
    }

    conv.messages = [];
    conv.queue = [];
    conv.running = null;
    conv.draft = "";
    conv.error = "";
    stopVoiceRecordForConversation(normalizedKey);
    clearComposerMediaByKey(normalizedKey);
    setQueuePanelExpanded(normalizedKey, false);
    clearMessageSelection(normalizedKey);
    if (state.chat.swipedConversationKey === normalizedKey) {
      state.chat.swipedConversationKey = "";
    }
    if (String(state.chat.messageViewer.conversationKey || "") === normalizedKey) {
      state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
      state.chat.viewMode = "detail";
    }
    touchConversation(conv);
    render();
    return true;
  }

  async function hydrateConversationFromLog(key) {
    try {
      const rows = await tauriInvoke("chat_store_load_conversation", {
        conversationKey: key,
        limit: 120,
      });
      const list = Array.isArray(rows) ? rows : [];
      if (!list.length) return;
      const snapshots = list
        .map((item) => asMap(item))
        .filter((item) => String(item.type || "") === "snapshot")
        .map((item) => restoreConversation(item.conversation))
        .filter((item) => item && item.key === key);
      if (!snapshots.length) return;
      const latest = snapshots[snapshots.length - 1];
      const current = state.chat.conversationsByKey[key];
      if (!current) return;
      current.messages = latest.messages;
      current.queue = latest.queue;
      current.draft = current.draft || latest.draft;
      current.error = latest.error || current.error;
      current.updatedAt = latest.updatedAt || current.updatedAt;
      render();
    } catch (_) {
      // ignore load failures
    }
  }

  function maybeDispatchNext(conversationKey) {
    const conv = state.chat.conversationsByKey[conversationKey];
    if (!conv || !conv.online || conv.running || conv.queue.length === 0) return;
    if (String(conv.availability || "").toLowerCase() === "invalid") {
      conv.error = "当前工具实例已失效，请删除卡片后重新接入新进程。";
      touchConversation(conv);
      render();
      return;
    }
    const item = conv.queue[0];
    const runtimeToolId = resolveConversationRuntimeToolId(conv);
    if (!runtimeToolId) {
      conv.error = "当前工具未在线，无法发送。";
      touchConversation(conv);
      render();
      return;
    }
    const outboundContent = normalizeContentParts(item.content);
    const text = String(item.text || "").trim() || contentSummaryText(outboundContent);
    const payload = {
      toolId: runtimeToolId,
      conversationKey: conv.key,
      requestId: item.requestId,
      queueItemId: item.queueItemId,
      text,
    };
    if (outboundContent.length > 0) {
      payload.content = outboundContent;
    }
    const sent = sendSocketEvent(
      conv.hostId,
      "tool_chat_request",
      payload,
      {
        action: "tool_chat_request",
        traceId: item.requestId.replace(/^req_/, "trc_"),
        toolId: runtimeToolId,
      },
    );
    if (!sent) {
      conv.error = "发送失败：宿主机未连接";
      const userMsg = conv.messages.find((msg) => String(msg.queueItemId || "") === item.queueItemId);
      if (userMsg) userMsg.status = "failed";
      touchConversation(conv);
      render();
      return;
    }

    conv.running = {
      requestId: item.requestId,
      queueItemId: item.queueItemId,
      text,
      content: stripTransientContentFields(outboundContent),
      startedAt: new Date().toISOString(),
    };
    conv.queue.shift();
    normalizeQueuePanelByConversation(conv);
    const userMsg = conv.messages.find((msg) => String(msg.queueItemId || "") === item.queueItemId);
    if (userMsg) {
      userMsg.status = "sending";
      userMsg.content = stripTransientContentFields(userMsg.content);
    }
    conv.error = "";
    touchConversation(conv);
    render();
  }

  function enqueueMessage() {
    const conv = activeConversation();
    if (!conv) return;
    const text = String(conv.draft || "").trim();
    const content = [];
    if (text) {
      content.push({ type: "text", text });
    }
    cloneContentParts(activeComposerMedia()).forEach((part) => content.push(part));
    if (content.length === 0) {
      maybeDispatchNext(conv.key);
      return;
    }
    if (String(conv.availability || "").toLowerCase() === "invalid" && isOpenCodeConversation(conv)) {
      conv.error = "当前进程已失效（PID 已变化），请删除卡片后重新接入。";
      render();
      return;
    }
    if (!conv.online) {
      conv.error = "当前会话离线，无法发送";
      render();
      return;
    }
    if (conv.queue.length + (conv.running ? 1 : 0) >= CHAT_QUEUE_LIMIT) {
      conv.error = `队列已达上限（${CHAT_QUEUE_LIMIT}）`;
      render();
      return;
    }

    const queueItemId = createId("q");
    const requestId = createId("req");
    const requestText = text || contentSummaryText(content);
    conv.queue.push({
      queueItemId,
      requestId,
      text: requestText,
      content: cloneContentParts(content),
      createdAt: new Date().toISOString(),
    });
    conv.messages.push({
      id: createId("msg"),
      role: "user",
      text,
      content: cloneContentParts(content),
      status: "queued",
      ts: new Date().toISOString(),
      queueItemId,
      requestId,
    });
    conv.draft = "";
    clearComposerMediaByKey(conv.key);
    conv.error = "";
    touchConversation(conv);
    render();
    maybeDispatchNext(conv.key);
  }

  function deleteQueuedMessage(queueItemId) {
    const conv = activeConversation();
    if (!conv) return;
    const key = String(queueItemId || "");
    if (!key) return;
    conv.queue = conv.queue.filter((item) => String(item.queueItemId || "") !== key);
    conv.messages = conv.messages.filter((msg) => {
      if (String(msg.queueItemId || "") !== key) return true;
      return String(msg.status || "") !== "queued";
    });
    normalizeQueuePanelByConversation(conv);
    touchConversation(conv);
    render();
  }

  function stopRunningMessage() {
    const conv = activeConversation();
    if (!conv || !conv.running) return;
    const runtimeToolId = resolveConversationRuntimeToolId(conv);
    if (!runtimeToolId) {
      conv.error = "停止失败：当前工具未在线";
      render();
      return;
    }
    const sent = sendSocketEvent(
      conv.hostId,
      "tool_chat_cancel_request",
      {
        toolId: runtimeToolId,
        conversationKey: conv.key,
        requestId: conv.running.requestId,
        queueItemId: conv.running.queueItemId,
      },
      {
        action: "tool_chat_cancel_request",
        traceId: conv.running.requestId.replace(/^req_/, "trc_"),
        toolId: runtimeToolId,
      },
    );
    if (!sent) {
      conv.error = "停止失败：无法发送取消请求";
      render();
      return;
    }
    addLog(`已发送停止生成请求: ${conv.key}`, {
      scope: "chat",
      action: "tool_chat_cancel_request",
      outcome: "sent",
      hostId: conv.hostId,
      toolId: runtimeToolId,
      traceId: conv.running.requestId.replace(/^req_/, "trc_"),
    });
  }

  function ensureReportTransfer(requestId, meta = {}) {
    const normalizedRequestId = String(requestId || "").trim();
    if (!normalizedRequestId) return null;
    const existing = asMap(state.chat.reportTransfersByRequestId[normalizedRequestId]);
    const next = {
      requestId: normalizedRequestId,
      conversationKey: String(meta.conversationKey || existing.conversationKey || ""),
      hostId: String(meta.hostId || existing.hostId || ""),
      runtimeToolId: String(meta.runtimeToolId || existing.runtimeToolId || ""),
      filePath: String(meta.filePath || existing.filePath || ""),
      content: String(existing.content || ""),
      status: String(meta.status || existing.status || "requested"),
      error: String(meta.error || existing.error || ""),
      bytesSent: Number(meta.bytesSent ?? existing.bytesSent ?? 0),
      bytesTotal: Number(meta.bytesTotal ?? existing.bytesTotal ?? 0),
      chunkIndex: Number(meta.chunkIndex ?? existing.chunkIndex ?? -1),
    };
    state.chat.reportTransfersByRequestId[normalizedRequestId] = next;
    return next;
  }

  function removeReportTransfer(requestId) {
    const normalizedRequestId = String(requestId || "").trim();
    if (!normalizedRequestId) return;
    delete state.chat.reportTransfersByRequestId[normalizedRequestId];
  }

  function syncReportViewerByRequestId(requestId, overrides = {}) {
    const viewer = asMap(state.chat.reportViewer);
    const normalizedRequestId = String(requestId || "").trim();
    if (!normalizedRequestId || String(viewer.requestId || "") !== normalizedRequestId) return;
    const transfer = asMap(state.chat.reportTransfersByRequestId[normalizedRequestId]);
    state.chat.reportViewer = createReportViewerState({
      ...viewer,
      visible: Boolean(viewer.visible),
      requestId: normalizedRequestId,
      conversationKey: String(transfer.conversationKey || viewer.conversationKey || ""),
      hostId: String(transfer.hostId || viewer.hostId || ""),
      toolId: String(transfer.runtimeToolId || viewer.toolId || ""),
      filePath: String(transfer.filePath || viewer.filePath || ""),
      content: String(transfer.content || viewer.content || ""),
      status: String(transfer.status || viewer.status || "loading"),
      error: String(transfer.error || viewer.error || ""),
      bytesSent: Number(transfer.bytesSent || 0),
      bytesTotal: Number(transfer.bytesTotal || 0),
      ...overrides,
    });
  }

  function extractHomeDirFromText(raw) {
    const text = String(raw || "");
    if (!text) return "";
    const match = text.match(/(\/Users\/[^\s/]+|\/home\/[^\s/]+)/);
    return match ? String(match[1] || "") : "";
  }

  function inferConversationHomeDir(conv) {
    if (!conv) return "";
    const runtime = ensureRuntime(String(conv.hostId || ""));
    const runtimeToolId = resolveConversationRuntimeToolId(conv);
    if (runtime && Array.isArray(runtime.tools)) {
      const tool = runtime.tools.find((item) => {
        const logical = String(item.toolId || "").trim();
        const runtimeId = String(item.runtimeToolId || item.toolId || "").trim();
        return runtimeId === runtimeToolId || logical === String(conv.toolId || "");
      });
      const workspace = String(tool?.workspaceDir || "").trim();
      const byWorkspace = extractHomeDirFromText(workspace);
      if (byWorkspace) return byWorkspace;
    }

    const messages = Array.isArray(conv.messages) ? conv.messages : [];
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      const byMessage = extractHomeDirFromText(messages[i]?.text || "");
      if (byMessage) return byMessage;
    }
    return "";
  }

  function normalizeReportFetchPath(conv, filePath) {
    let next = String(filePath || "").trim();
    if (!next) return "";

    if (next.startsWith("yc-report://")) {
      next = next.slice("yc-report://".length);
    } else if (next.toLowerCase().startsWith("file://")) {
      try {
        const parsed = new URL(next);
        next = String(parsed.pathname || "");
      } catch (_) {
        return "";
      }
    }

    try {
      next = decodeURIComponent(next);
    } catch (_) {
      // keep raw value when decode fails
    }

    if (next.startsWith("~/")) {
      const homeDir = inferConversationHomeDir(conv);
      if (homeDir) {
        next = `${homeDir}/${next.slice(2)}`;
      }
    }

    return String(next || "").trim();
  }

  function showReportError(conv, filePath, reason) {
    const viewer = createReportViewerState({
      visible: true,
      conversationKey: String(conv?.key || ""),
      hostId: String(conv?.hostId || ""),
      toolId: resolveConversationRuntimeToolId(conv),
      requestId: "",
      filePath: String(filePath || ""),
      content: "",
      status: "failed",
      error: String(reason || "报告拉取失败"),
      bytesSent: 0,
      bytesTotal: 0,
    });
    state.chat.reportViewer = viewer;
  }

  function requestReportFetch(conv, filePath) {
    const rawPath = String(filePath || "").trim();
    const normalizedPath = normalizeReportFetchPath(conv, rawPath);
    if (!conv || !rawPath) return;
    if (!normalizedPath.toLowerCase().endsWith(".md")) {
      showReportError(conv, rawPath, "仅支持读取 .md 报告。");
      render();
      return;
    }
    if (!normalizedPath.startsWith("/") && !normalizedPath.startsWith("~/")) {
      const reason = rawPath.startsWith("~/")
        ? "当前消息使用了 ~/ 路径，暂未推断到宿主机 home 目录。请让助手返回绝对路径。"
        : "仅支持读取绝对路径下的 .md 报告。";
      showReportError(conv, rawPath, reason);
      render();
      return;
    }
    if (!normalizeReportPathForPreview(normalizedPath)) {
      showReportError(conv, normalizedPath, "该文件疑似系统规则文档，已禁止预览。");
      render();
      return;
    }
    const runtimeToolId = resolveConversationRuntimeToolId(conv);
    if (!runtimeToolId) {
      showReportError(conv, normalizedPath, "当前工具未在线，无法读取报告。");
      render();
      return;
    }
    const requestId = createId("rpt");
    const transfer = ensureReportTransfer(requestId, {
      conversationKey: conv.key,
      hostId: conv.hostId,
      runtimeToolId,
      filePath: normalizedPath,
      status: "requested",
      error: "",
      bytesSent: 0,
      bytesTotal: 0,
      chunkIndex: -1,
    });
    if (!transfer) return;
    state.chat.reportViewer = createReportViewerState({
      visible: true,
      conversationKey: conv.key,
      hostId: conv.hostId,
      toolId: runtimeToolId,
      requestId,
      filePath: normalizedPath,
      content: "",
      status: "loading",
      error: "",
      bytesSent: 0,
      bytesTotal: 0,
    });
    const sent = sendSocketEvent(
      conv.hostId,
      "tool_report_fetch_request",
      {
        toolId: runtimeToolId,
        conversationKey: conv.key,
        requestId,
        filePath: normalizedPath,
      },
      {
        action: "tool_report_fetch_request",
        traceId: requestId.replace(/^rpt_/, "trc_"),
        toolId: runtimeToolId,
      },
    );
    if (!sent) {
      removeReportTransfer(requestId);
      showReportError(conv, normalizedPath, "发送失败：宿主机未连接。");
    }
    render();
  }

  function onReportPathClick(event) {
    const reportPath = resolveReportPathFromTarget(event.target);
    if (!reportPath) return false;
    const conv = activeConversation();
    if (!conv) return false;
    if (state.chat.messageSelectionModeByKey[conv.key]) return false;
    event.preventDefault();
    event.stopPropagation();
    requestReportFetch(conv, reportPath);
    return true;
  }

  function ensureConversationByPayload(hostId, payload) {
    const runtimeToolId = String(payload.toolId || "").trim();
    const logicalToolId = mapLogicalToolId(hostId, runtimeToolId);
    const key = chatConversationKey(hostId, logicalToolId);
    if (!key || !logicalToolId) return null;
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    const tool = runtime && Array.isArray(runtime.tools)
      ? runtime.tools.find((item) => (
        String(item.toolId || "") === logicalToolId
        || String(item.runtimeToolId || "") === runtimeToolId
      ))
      : null;
    const conv = ensureConversation(state.chat, key, {
      hostId,
      toolId: logicalToolId,
      runtimeToolId: runtimeToolId || String(tool?.runtimeToolId || tool?.toolId || ""),
      toolClass: tool ? String(tool.toolClass || "") : "",
      hostName: host ? host.displayName : hostId,
      toolName: tool ? resolveToolDisplayName(hostId, tool) : logicalToolId,
      online: isToolOnline(runtime, tool),
      availability: tool && (String(tool.status || "").toLowerCase() === "invalid" || tool.invalidPidChanged)
        ? "invalid"
        : (isToolOnline(runtime, tool) ? "online" : "offline"),
      updatedAt: new Date().toISOString(),
    });
    if (!state.chat.activeConversationKey) {
      state.chat.activeConversationKey = key;
    }
    return conv;
  }

  function onToolChatStarted(hostId, payload) {
    const data = asMap(payload);
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    const requestId = String(data.requestId || "");
    const queueItemId = String(data.queueItemId || requestId);
    if (requestId) {
      const queued = conv.queue.find(
        (item) => String(item.requestId || "") === requestId
          || String(item.queueItemId || "") === queueItemId,
      );
      conv.running = {
        requestId,
        queueItemId,
        text: conv.running && conv.running.requestId === requestId
          ? conv.running.text
          : String((queued && queued.text) || ""),
        content: conv.running && conv.running.requestId === requestId
          ? stripTransientContentFields(conv.running.content)
          : stripTransientContentFields(queued && queued.content),
        startedAt: new Date().toISOString(),
      };
      removeQueueItems(conv, { queueItemId, requestId });
      const userMsg = conv.messages.find((msg) => String(msg.requestId || "") === requestId && msg.role === "user");
      if (userMsg) userMsg.status = "sent";
      if (!findMessage(conv, requestId, "assistant")) {
        conv.messages.push({
          id: createId("msg"),
          role: "assistant",
          text: "",
          status: "streaming",
          ts: new Date().toISOString(),
          queueItemId,
          requestId,
          meta: {},
        });
      }
    }
    touchConversation(conv);
    render();
  }

  function onToolChatChunk(hostId, payload) {
    const data = asMap(payload);
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    const requestId = String(data.requestId || "");
    const queueItemId = String(data.queueItemId || requestId);
    if (!requestId) return;
    if (!conv.running || conv.running.requestId !== requestId) {
      const userMsg = conv.messages.find((msg) => String(msg.requestId || "") === requestId && msg.role === "user");
      conv.running = {
        requestId,
        queueItemId,
        text: String((userMsg && userMsg.text) || ""),
        content: stripTransientContentFields(userMsg && userMsg.content),
        startedAt: new Date().toISOString(),
      };
    }
    removeQueueItems(conv, { queueItemId, requestId });
    let assistant = findMessage(conv, requestId, "assistant");
    if (!assistant) {
      assistant = {
        id: createId("msg"),
        role: "assistant",
        text: "",
        status: "streaming",
        ts: new Date().toISOString(),
        queueItemId: String(data.queueItemId || requestId),
        requestId,
        meta: {},
      };
      conv.messages.push(assistant);
    }
    assistant.text = `${assistant.text || ""}${String(data.text || "")}`;
    assistant.status = "streaming";
    assistant.meta = asMap(data.meta);
    touchConversation(conv);
    render();
  }

  function onToolChatFinished(hostId, payload) {
    const data = asMap(payload);
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    const requestId = String(data.requestId || "");
    const queueItemId = String(data.queueItemId || requestId);
    const status = String(data.status || "completed");
    const text = String(data.text || "");
    const reason = String(data.reason || "").trim();

    let assistant = requestId ? findMessage(conv, requestId, "assistant") : null;
    if (!assistant && (text || status !== "completed")) {
      assistant = {
        id: createId("msg"),
        role: status === "failed" || status === "busy" ? "system" : "assistant",
        text: text || reason || `请求结束：${status}`,
        status,
        ts: new Date().toISOString(),
        queueItemId: String(data.queueItemId || requestId),
        requestId,
        meta: asMap(data.meta),
      };
      conv.messages.push(assistant);
    }

    if (assistant) {
      if (text && !assistant.text) assistant.text = text;
      assistant.status = status;
      assistant.meta = asMap(data.meta);
      if (reason && !assistant.text) assistant.text = reason;
    }

    if (requestId) {
      removeQueueItems(conv, { queueItemId, requestId });
      const userMsg = conv.messages.find((msg) => String(msg.requestId || "") === requestId && msg.role === "user");
      if (userMsg) {
        if (status === "busy" || status === "failed") userMsg.status = "failed";
        else if (status === "cancelled") userMsg.status = "cancelled";
        else userMsg.status = "completed";
      }
    }

    if (conv.running && (!requestId || conv.running.requestId === requestId)) {
      conv.running = null;
    }
    normalizeQueuePanelByConversation(conv);
    conv.error = (status === "failed" || status === "busy") ? (reason || `请求结束：${status}`) : "";
    touchConversation(conv);
    render();
    maybeDispatchNext(conv.key);
  }

  function onToolReportFetchStarted(hostId, payload) {
    const data = asMap(payload);
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    const requestId = String(data.requestId || "").trim();
    if (!requestId) return;
    const transfer = ensureReportTransfer(requestId, {
      conversationKey: conv.key,
      hostId: conv.hostId,
      runtimeToolId: String(data.toolId || conv.runtimeToolId || ""),
      filePath: String(data.filePath || ""),
      status: "started",
      error: "",
      bytesSent: Number(data.bytesSent || 0),
      bytesTotal: Number(data.bytesTotal || 0),
      chunkIndex: -1,
    });
    if (!transfer) return;
    syncReportViewerByRequestId(requestId, {
      status: "loading",
      error: "",
      bytesSent: transfer.bytesSent,
      bytesTotal: transfer.bytesTotal,
    });
    render();
  }

  function onToolReportFetchChunk(hostId, payload) {
    const data = asMap(payload);
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    const requestId = String(data.requestId || "").trim();
    if (!requestId) return;
    const transfer = ensureReportTransfer(requestId, {
      conversationKey: conv.key,
      hostId: conv.hostId,
      runtimeToolId: String(data.toolId || conv.runtimeToolId || ""),
      filePath: String(data.filePath || ""),
      status: "streaming",
      error: "",
      bytesSent: Number(data.bytesSent || 0),
      bytesTotal: Number(data.bytesTotal || 0),
      chunkIndex: Number(data.chunkIndex || 0),
    });
    if (!transfer) return;
    transfer.content = `${transfer.content || ""}${String(data.chunk || "")}`;
    syncReportViewerByRequestId(requestId, {
      status: "streaming",
      content: transfer.content,
      error: "",
      bytesSent: Number(data.bytesSent || 0),
      bytesTotal: Number(data.bytesTotal || transfer.bytesTotal || 0),
    });
    render();
  }

  function onToolReportFetchFinished(hostId, payload) {
    const data = asMap(payload);
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    const requestId = String(data.requestId || "").trim();
    if (!requestId) return;
    const status = String(data.status || "failed").trim().toLowerCase();
    const reason = String(data.reason || "").trim();
    const transfer = ensureReportTransfer(requestId, {
      conversationKey: conv.key,
      hostId: conv.hostId,
      runtimeToolId: String(data.toolId || conv.runtimeToolId || ""),
      filePath: String(data.filePath || ""),
      status,
      error: reason,
      bytesSent: Number(data.bytesSent || 0),
      bytesTotal: Number(data.bytesTotal || 0),
      chunkIndex: Number(data.chunkIndex || 0),
    });
    if (!transfer) return;
    if (status === "completed") {
      syncReportViewerByRequestId(requestId, {
        status: "completed",
        error: "",
        content: transfer.content || "",
        bytesSent: Number(data.bytesSent || transfer.bytesSent || 0),
        bytesTotal: Number(data.bytesTotal || transfer.bytesTotal || 0),
      });
    } else {
      syncReportViewerByRequestId(requestId, {
        status,
        error: reason || "报告拉取失败",
        bytesSent: Number(data.bytesSent || transfer.bytesSent || 0),
        bytesTotal: Number(data.bytesTotal || transfer.bytesTotal || 0),
      });
    }
    removeReportTransfer(requestId);
    render();
  }

  function onChatListTouchStart(event) {
    const row = event.target.closest("[data-chat-row]");
    const touch = event.touches && event.touches[0];
    if (!row || !touch) return;
    swipeState.key = String(row.getAttribute("data-chat-row") || "");
    swipeState.startX = Number(touch.clientX || 0);
    swipeState.startY = Number(touch.clientY || 0);
    swipeState.active = Boolean(swipeState.key);
  }

  function onChatListTouchMove(event) {
    if (!swipeState.active) return;
    const touch = event.touches && event.touches[0];
    if (!touch) return;
    const deltaX = Number(touch.clientX || 0) - swipeState.startX;
    const deltaY = Number(touch.clientY || 0) - swipeState.startY;
    if (Math.abs(deltaX) < 20 || Math.abs(deltaY) > Math.abs(deltaX)) return;
    if (deltaX <= -36) {
      swipeState.active = false;
      if (setSwipedConversation(swipeState.key)) {
        render();
      }
      return;
    }
    if (deltaX >= 24) {
      swipeState.active = false;
      if (setSwipedConversation("")) {
        render();
      }
    }
  }

  function onChatListTouchEnd() {
    swipeState.active = false;
  }

  function onChatListClick(event) {
    const deleteBtn = event.target.closest("[data-chat-delete]");
    if (deleteBtn) {
      const key = String(deleteBtn.getAttribute("data-chat-delete") || "");
      if (key) {
        const conv = state.chat.conversationsByKey[key];
        const label = conv
          ? `${conv.hostName || conv.hostId || "--"} · ${conv.toolName || conv.toolId || "--"}`
          : key;
        const confirmed = window.confirm(`确认删除会话「${label}」吗？此操作不可恢复。`);
        if (confirmed) {
          void deleteConversationByKey(key, { deleteStore: true });
        }
      }
      return;
    }

    const clearBtn = event.target.closest("[data-chat-clear]");
    if (clearBtn) {
      void clearConversationMessages(String(clearBtn.getAttribute("data-chat-clear") || ""));
      return;
    }

    const openBtn = event.target.closest("[data-chat-open]");
    if (!openBtn) {
      if (state.chat.swipedConversationKey && setSwipedConversation("")) {
        render();
      }
      return;
    }

    if (Date.now() < suppressOpenUntil) {
      return;
    }
    const targetKey = String(openBtn.getAttribute("data-chat-open") || "");
    if (state.chat.swipedConversationKey) {
      if (state.chat.swipedConversationKey === targetKey) {
        if (setSwipedConversation("")) render();
        return;
      }
      if (setSwipedConversation("")) {
        render();
        return;
      }
    }

    openConversation(targetKey);
  }

  function onQueueClick(event) {
    const deleteBtn = event.target.closest("[data-chat-queue-delete]");
    if (!deleteBtn) return;
    deleteQueuedMessage(String(deleteBtn.getAttribute("data-chat-queue-delete") || ""));
  }

  function onQueueSummaryClick() {
    const conv = activeConversation();
    if (!conv || !Array.isArray(conv.queue) || conv.queue.length === 0) return;
    const expanded = Boolean(state.chat.queuePanelExpandedByKey[conv.key]);
    setQueuePanelExpanded(conv.key, !expanded);
    render();
  }

  function onComposerMediaTrayClick(event) {
    const removeBtn = event.target.closest("[data-chat-remove-media]");
    if (!removeBtn) return;
    const mediaId = String(removeBtn.getAttribute("data-chat-remove-media") || "").trim();
    if (!mediaId) return;
    removeComposerMediaItem(mediaId);
  }

  function onDraftInput(value) {
    const conv = activeConversation();
    if (!conv) return;
    conv.draft = String(value || "");
    render();
  }

  function toggleMessageSelectionMode() {
    const conv = activeConversation();
    if (!conv) return;
    const enabled = Boolean(state.chat.messageSelectionModeByKey[conv.key]);
    if (enabled) {
      clearMessageSelection(conv.key);
      render();
      return;
    }
    if (!Array.isArray(conv.messages) || conv.messages.length === 0) {
      return;
    }
    state.chat.messageSelectionModeByKey[conv.key] = true;
    state.chat.selectedMessageIdsByKey[conv.key] = {};
    render();
  }

  function onMessagesClick(event) {
    const conv = activeConversation();
    if (!conv) return;
    if (onReportPathClick(event)) {
      return;
    }
    const expandBtn = event.target.closest("[data-chat-expand-message]");
    if (expandBtn) {
      if (state.chat.messageSelectionModeByKey[conv.key]) return;
      openMessageViewer(String(expandBtn.getAttribute("data-chat-expand-message") || ""));
      return;
    }
    if (!state.chat.messageSelectionModeByKey[conv.key]) return;
    const target = event.target.closest("[data-chat-message-id]");
    if (!target) return;
    const messageId = String(target.getAttribute("data-chat-message-id") || "");
    if (!messageId) return;
    const selectable = String(target.getAttribute("data-chat-message-selectable") || "0") === "1";
    if (!selectable) return;
    const selected = asMap(state.chat.selectedMessageIdsByKey[conv.key]);
    if (selected[messageId]) {
      delete selected[messageId];
    } else {
      selected[messageId] = true;
    }
    state.chat.selectedMessageIdsByKey[conv.key] = selected;
    normalizeMessageSelectionByConversation(conv);
    render();
  }

  function onMessageFullBodyClick(event) {
    onReportPathClick(event);
  }

  function deleteSelectedMessages() {
    const conv = activeConversation();
    if (!conv) return;
    const selectedIds = selectedMessageIds(conv.key);
    if (!selectedIds.length) return;
    const selectedSet = new Set(selectedIds);
    const removable = conv.messages.filter((msg) => selectedSet.has(String(msg.id || "")) && isMessageSelectable(msg));
    if (!removable.length) return;
    const confirmed = window.confirm(`确认删除已选中的 ${removable.length} 条消息吗？`);
    if (!confirmed) return;

    const removeMessageIds = new Set(removable.map((msg) => String(msg.id || "")).filter(Boolean));
    const removeRequestIds = new Set(removable.map((msg) => String(msg.requestId || "")).filter(Boolean));
    const removeQueueItemIds = new Set(removable.map((msg) => String(msg.queueItemId || "")).filter(Boolean));

    conv.messages = conv.messages.filter((msg) => !removeMessageIds.has(String(msg.id || "")));
    conv.queue = conv.queue.filter((item) => {
      const itemRequestId = String(item.requestId || "");
      const itemQueueItemId = String(item.queueItemId || "");
      return !removeRequestIds.has(itemRequestId) && !removeQueueItemIds.has(itemQueueItemId);
    });

    clearMessageSelection(conv.key);
    normalizeQueuePanelByConversation(conv);
    touchConversation(conv);
    render();
    maybeDispatchNext(conv.key);
  }

  function normalizeRestoredConversations(restoredByKey, rawOrder) {
    const normalizedByKey = {};
    const rankByKey = {};

    function rankConversation(conv) {
      const messageCount = Array.isArray(conv.messages) ? conv.messages.length : 0;
      const updatedAt = Date.parse(String(conv.updatedAt || "")) || 0;
      return { messageCount, updatedAt };
    }

    function shouldReplace(existingRank, candidateRank) {
      if (!existingRank) return true;
      if (candidateRank.messageCount !== existingRank.messageCount) {
        return candidateRank.messageCount > existingRank.messageCount;
      }
      return candidateRank.updatedAt >= existingRank.updatedAt;
    }

    Object.entries(asMap(restoredByKey)).forEach(([key, rawConv]) => {
      const conv = asMap(rawConv);
      const fallbackHost = String(key || "").split("::")[0] || "";
      const fallbackTool = String(key || "").split("::").slice(1).join("::") || "";
      const hostId = String(conv.hostId || fallbackHost || "").trim();
      const rawToolId = String(conv.toolId || fallbackTool || "").trim();
      if (!hostId || !rawToolId) return;

      const logicalToolId = mapLogicalToolId(hostId, rawToolId);
      const normalizedKey = chatConversationKey(hostId, logicalToolId);
      if (!normalizedKey) return;

      const candidate = {
        ...conv,
        key: normalizedKey,
        hostId,
        toolId: logicalToolId,
        runtimeToolId: String(conv.runtimeToolId || rawToolId || ""),
        availability: String(conv.availability || "offline"),
        online: false,
      };
      const candidateRank = rankConversation(candidate);
      if (shouldReplace(rankByKey[normalizedKey], candidateRank)) {
        normalizedByKey[normalizedKey] = candidate;
        rankByKey[normalizedKey] = candidateRank;
      }
    });

    const orderSource = Array.isArray(rawOrder) ? rawOrder : [];
    const normalizedOrder = [];
    const seen = new Set();
    orderSource.forEach((rawKey) => {
      const conv = asMap(restoredByKey[rawKey]);
      const fallbackHost = String(rawKey || "").split("::")[0] || "";
      const fallbackTool = String(rawKey || "").split("::").slice(1).join("::") || "";
      const hostId = String(conv.hostId || fallbackHost || "").trim();
      const rawToolId = String(conv.toolId || fallbackTool || "").trim();
      const logicalToolId = mapLogicalToolId(hostId, rawToolId);
      const normalizedKey = chatConversationKey(hostId, logicalToolId);
      if (!normalizedKey || !normalizedByKey[normalizedKey] || seen.has(normalizedKey)) return;
      seen.add(normalizedKey);
      normalizedOrder.push(normalizedKey);
    });

    const remaining = Object.keys(normalizedByKey)
      .filter((key) => !seen.has(key))
      .sort((a, b) => {
        const ta = Date.parse(String(normalizedByKey[a]?.updatedAt || "")) || 0;
        const tb = Date.parse(String(normalizedByKey[b]?.updatedAt || "")) || 0;
        return tb - ta;
      });
    normalizedOrder.push(...remaining);

    return { byKey: normalizedByKey, order: normalizedOrder };
  }

  async function hydrateChatState() {
    try {
      const result = await tauriInvoke("chat_store_bootstrap", {});
      const index = asMap(result && result.index);
      const byKey = asMap(index.conversationsByKey);
      const restoredRaw = {};
      Object.entries(byKey).forEach(([key, rawConv]) => {
        const conv = restoreConversation(rawConv);
        if (conv) restoredRaw[key] = conv;
      });
      const normalized = normalizeRestoredConversations(restoredRaw, index.conversationOrder);
      state.chat.conversationsByKey = normalized.byKey;
      state.chat.conversationOrder = Array.isArray(normalized.order)
        ? normalized.order.filter((key) => normalized.byKey[key])
        : Object.keys(normalized.byKey);
      if (state.chat.conversationOrder.length === 0) {
        state.chat.conversationOrder = Object.keys(normalized.byKey);
      }
      const active = String(index.activeConversationKey || "");
      const activeRawConv = asMap(restoredRaw[active]);
      const activeHostFallback = active.split("::")[0] || "";
      const activeToolFallback = active.split("::").slice(1).join("::") || "";
      const activeKey = chatConversationKey(
        String(activeRawConv.hostId || activeHostFallback || "").trim(),
        mapLogicalToolId(
          String(activeRawConv.hostId || activeHostFallback || "").trim(),
          String(activeRawConv.toolId || activeToolFallback || "").trim(),
        ),
      );
      state.chat.activeConversationKey = normalized.byKey[activeKey]
        ? activeKey
        : (state.chat.conversationOrder[0] || "");
      state.chat.viewMode = "list";
      state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
      state.chat.queuePanelExpandedByKey = {};
      state.chat.composerMediaByKey = {};
      state.chat.recordingConversationKey = "";
      state.chat.recordingPending = false;
      state.chat.messageSelectionModeByKey = {};
      state.chat.selectedMessageIdsByKey = {};
      state.chat.swipedConversationKey = "";
      state.chat.reportViewer = createReportViewerState();
      state.chat.reportTransfersByRequestId = {};
      state.chat.hydrated = true;
      normalizeConversationOnlineState();
      void persistIndex();
      render();
    } catch (error) {
      addLog(`chat bootstrap failed: ${error}`, {
        level: "warn",
        scope: "chat",
        action: "bootstrap",
        outcome: "failed",
      });
      state.chat.viewMode = "list";
      state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
      state.chat.queuePanelExpandedByKey = {};
      state.chat.composerMediaByKey = {};
      state.chat.recordingConversationKey = "";
      state.chat.recordingPending = false;
      state.chat.messageSelectionModeByKey = {};
      state.chat.selectedMessageIdsByKey = {};
      state.chat.swipedConversationKey = "";
      state.chat.reportViewer = createReportViewerState();
      state.chat.reportTransfersByRequestId = {};
      state.chat.hydrated = true;
      normalizeConversationOnlineState();
      render();
    }
  }

  function renderSync() {
    normalizeConversationOnlineState();
  }

  function bindEvents(ui) {
    ui.chatConversationList.addEventListener("click", onChatListClick);
    ui.chatConversationList.addEventListener("touchstart", onChatListTouchStart, { passive: true });
    ui.chatConversationList.addEventListener("touchmove", onChatListTouchMove, { passive: true });
    ui.chatConversationList.addEventListener("touchend", onChatListTouchEnd, { passive: true });
    ui.chatConversationList.addEventListener("touchcancel", onChatListTouchEnd, { passive: true });
    ui.chatQueue.addEventListener("click", onQueueClick);
    ui.chatQueueSummary.addEventListener("click", onQueueSummaryClick);
    ui.chatComposerMediaTray.addEventListener("click", onComposerMediaTrayClick);
    ui.chatMessages.addEventListener("click", onMessagesClick);
    ui.chatMessageFullBody.addEventListener("click", onMessageFullBodyClick);
    ui.chatAttachBtn.addEventListener("click", () => {
      if (ui.chatMediaInput) ui.chatMediaInput.click();
    });
    ui.chatMediaInput.addEventListener("change", () => {
      void onComposerMediaPicked(ui.chatMediaInput);
    });
    ui.chatRecordBtn.addEventListener("click", toggleVoiceRecord);
    ui.chatInput.addEventListener("input", () => onDraftInput(ui.chatInput.value));
    ui.chatSendBtn.addEventListener("click", enqueueMessage);
    ui.chatStopBtn.addEventListener("click", stopRunningMessage);
    ui.chatSelectBtn.addEventListener("click", toggleMessageSelectionMode);
    ui.chatDeleteSelectedBtn.addEventListener("click", deleteSelectedMessages);
    ui.chatBackBtn.addEventListener("click", backToList);
    ui.chatMessageBackBtn.addEventListener("click", closeMessageViewer);
    ui.chatMessageZoomOutBtn.addEventListener("click", () => zoomMessageViewer(-0.1));
    ui.chatMessageZoomInBtn.addEventListener("click", () => zoomMessageViewer(0.1));
  }

  return {
    hydrateChatState,
    renderSync,
    bindEvents,
    enterChatTab,
    backToList,
    onToolChatStarted,
    onToolChatChunk,
    onToolChatFinished,
    onToolReportFetchStarted,
    onToolReportFetchChunk,
    onToolReportFetchFinished,
    enqueueMessage,
    stopRunningMessage,
    openConversation,
    deleteConversationByKey,
    deleteConversationByTool,
    deleteConversationsByHost,
  };
}
