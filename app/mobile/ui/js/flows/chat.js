// 文件职责：
// 1. 管理聊天会话状态（队列、运行中、消息流）。
// 2. 对接 WS 聊天事件与 Tauri 文件存储。

import { asMap, asListOfMap } from "../utils/type.js";
import { resolveReportPathFromTarget, normalizeReportPathForPreview } from "../utils/markdown.js";
import { buildLaunchConfirmDraft, parseLaunchConfirmFromText } from "../utils/launch-proposal.js";
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
  requestToolLaunch,
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
  const pendingMediaStageByKey = {};
  const dispatchingByConversationKey = {};
  const MEDIA_STAGE_TIMEOUT_MS = 30_000;
  const MEDIA_INPUT_ACCEPT = "image/*,video/*";
  const FILE_INPUT_ACCEPT = ".pdf,.txt,.md,.json,.csv,.zip,.tar,.gz,.log,.yaml,.yml,.toml,.xml,.doc,.docx,.xls,.xlsx,.ppt,.pptx,.rtf,.js,.ts,.tsx,.jsx,.py,.rs,.go,.java,.kt,.swift,.c,.cpp,.h,.hpp,.sh";

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
  if (typeof state.chat.attachmentMenuOpen !== "boolean") {
    state.chat.attachmentMenuOpen = false;
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
  if (!state.chat.launchRequestsById || typeof state.chat.launchRequestsById !== "object") {
    state.chat.launchRequestsById = {};
  }

  let suppressOpenUntil = 0;
  let documentPointerListenerBound = false;
  let uiRefs = null;
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

  function isMediaPartType(type) {
    const normalized = String(type || "").trim().toLowerCase();
    return normalized === "image" || normalized === "video" || normalized === "audio";
  }

  function mediaStageKey(hostId, conversationKey, requestId, mediaId) {
    return [
      String(hostId || "").trim(),
      String(conversationKey || "").trim(),
      String(requestId || "").trim(),
      String(mediaId || "").trim(),
    ].join("::");
  }

  function registerMediaStagePromise(hostId, conversationKey, requestId, mediaId) {
    const key = mediaStageKey(hostId, conversationKey, requestId, mediaId);
    if (!key || key.endsWith("::")) return null;
    if (pendingMediaStageByKey[key]) {
      clearTimeout(pendingMediaStageByKey[key].timer);
      delete pendingMediaStageByKey[key];
    }
    return new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => {
        delete pendingMediaStageByKey[key];
        reject({
          ok: false,
          code: "MEDIA_STAGE_TIMEOUT",
          reason: "附件暂存超时，请重试",
          mediaId: String(mediaId || ""),
        });
      }, MEDIA_STAGE_TIMEOUT_MS);
      pendingMediaStageByKey[key] = {
        resolve,
        reject,
        timer,
      };
    });
  }

  function settleMediaStagePromise(hostId, payload, result) {
    const data = asMap(payload);
    const key = mediaStageKey(hostId, data.conversationKey, data.requestId, data.mediaId);
    const pending = pendingMediaStageByKey[key];
    if (!pending) return false;
    clearTimeout(pending.timer);
    delete pendingMediaStageByKey[key];
    if (result && result.ok) pending.resolve(result);
    else pending.reject(result || { ok: false, code: "MEDIA_STAGE_NOT_FOUND", reason: "附件暂存失败" });
    return true;
  }

  function rejectMediaStagePromise(hostId, conversationKey, requestId, mediaId, error) {
    const key = mediaStageKey(hostId, conversationKey, requestId, mediaId);
    const pending = pendingMediaStageByKey[key];
    if (!pending) return false;
    clearTimeout(pending.timer);
    delete pendingMediaStageByKey[key];
    pending.reject(error || {
      ok: false,
      code: "MEDIA_STAGE_NOT_FOUND",
      reason: "附件暂存失败",
      mediaId: String(mediaId || ""),
    });
    return true;
  }

  function clearPendingMediaStageByConversation(conversationKey) {
    const token = `::${String(conversationKey || "").trim()}::`;
    if (!token || token === "::::") return;
    Object.keys(pendingMediaStageByKey).forEach((key) => {
      if (!key.includes(token)) return;
      const pending = pendingMediaStageByKey[key];
      if (!pending) return;
      clearTimeout(pending.timer);
      delete pendingMediaStageByKey[key];
      pending.reject({
        ok: false,
        code: "MEDIA_STAGE_NOT_FOUND",
        reason: "会话已清理，已取消附件暂存",
      });
    });
  }

  function updateContentPartStageState(parts, mediaId, patch = {}) {
    const normalizedMediaId = String(mediaId || "").trim();
    if (!normalizedMediaId || !Array.isArray(parts)) return false;
    let changed = false;
    for (let i = 0; i < parts.length; i += 1) {
      const part = asMap(parts[i]);
      if (!isMediaPartType(part.type)) continue;
      if (String(part.mediaId || "").trim() !== normalizedMediaId) continue;
      parts[i] = normalizeContentPart({ ...part, ...patch }) || part;
      changed = true;
    }
    return changed;
  }

  function syncStageStateToConversation(conv, requestId, mediaId, patch = {}) {
    if (!conv) return false;
    const normalizedRequestId = String(requestId || "").trim();
    if (!normalizedRequestId) return false;
    let changed = false;
    if (Array.isArray(conv.queue)) {
      conv.queue.forEach((item) => {
        if (String(item.requestId || "") !== normalizedRequestId) return;
        const parts = normalizeContentParts(item.content);
        if (updateContentPartStageState(parts, mediaId, patch)) {
          item.content = parts;
          changed = true;
        }
      });
    }
    if (conv.running && String(conv.running.requestId || "") === normalizedRequestId) {
      const runningParts = normalizeContentParts(conv.running.content);
      if (updateContentPartStageState(runningParts, mediaId, patch)) {
        conv.running.content = stripTransientContentFields(runningParts);
        changed = true;
      }
    }
    if (Array.isArray(conv.messages)) {
      conv.messages.forEach((msg) => {
        if (msg.role !== "user") return;
        if (String(msg.requestId || "") !== normalizedRequestId) return;
        const parts = normalizeContentParts(msg.content);
        if (updateContentPartStageState(parts, mediaId, patch)) {
          msg.content = stripTransientContentFields(parts);
          changed = true;
        }
      });
    }
    return changed;
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

  function formatFileSize(bytes) {
    const value = Number(bytes || 0);
    if (!Number.isFinite(value) || value <= 0) return "--";
    if (value < 1024) return `${Math.trunc(value)}B`;
    if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)}KB`;
    if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)}MB`;
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)}GB`;
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
      const mediaId = String(part.mediaId || "").trim();
      const mime = String(part.mime || "").trim();
      const size = Number(part.size || 0);
      const fileName = String(part.fileName || "").trim();
      if (mediaId) normalized.mediaId = mediaId;
      if (mime) normalized.mime = mime;
      if (Number.isFinite(size) && size > 0) normalized.size = Math.trunc(size);
      if (pathHint) normalized.pathHint = pathHint;
      if (text) normalized.text = text;
      if (fileName) normalized.fileName = fileName;
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
    const stagedMediaId = String(part.stagedMediaId || "").trim();
    const stageErrorCode = String(part.stageErrorCode || "").trim();
    const stageErrorReason = String(part.stageErrorReason || "").trim();
    const stageStatus = String(part.stageStatus || "").trim();
    const stageProgress = Number(part.stageProgress ?? 0);
    const previewUrl = String(part.previewUrl || "").trim();
    const fileName = String(part.fileName || "").trim();

    if (mediaId) normalized.mediaId = mediaId;
    if (mime) normalized.mime = mime;
    if (Number.isFinite(size) && size > 0) normalized.size = Math.trunc(size);
    if (Number.isFinite(durationMs) && durationMs > 0) normalized.durationMs = Math.trunc(durationMs);
    if (pathHint) normalized.pathHint = pathHint;
    if (text) normalized.text = text;
    if (dataBase64) normalized.dataBase64 = dataBase64;
    if (stagedMediaId) normalized.stagedMediaId = stagedMediaId;
    if (stageErrorCode) normalized.stageErrorCode = stageErrorCode;
    if (stageErrorReason) normalized.stageErrorReason = stageErrorReason;
    if (stageStatus) normalized.stageStatus = stageStatus;
    if (Number.isFinite(stageProgress) && stageProgress > 0) normalized.stageProgress = Math.trunc(stageProgress);
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
      delete next.stageProgress;
      delete next.stageStatus;
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

  function setAttachmentMenuOpen(nextOpen) {
    const normalized = Boolean(nextOpen);
    if (Boolean(state.chat.attachmentMenuOpen) === normalized) {
      return false;
    }
    state.chat.attachmentMenuOpen = normalized;
    return true;
  }

  function closeAttachmentMenu() {
    return setAttachmentMenuOpen(false);
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
    const menuClosed = closeAttachmentMenu();
    if (inputElement) {
      inputElement.value = "";
    }
    if (!conv || files.length === 0) {
      if (menuClosed) render();
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
    let unsupportedCount = 0;
    let emptyCount = 0;
    let oversizeCount = 0;
    let failedCount = 0;
    let overflowCount = 0;
    for (const file of files) {
      if (nextItems.length >= limit) {
        overflowCount += 1;
        continue;
      }
      const mime = String(file.type || "").trim();
      const mediaType = inferMediaType(mime, file.name);
      if (mediaType !== "image" && mediaType !== "video") {
        unsupportedCount += 1;
        continue;
      }
      if (Number(file.size || 0) <= 0) {
        emptyCount += 1;
        continue;
      }
      if (Number(file.size || 0) > maxBytes) {
        oversizeCount += 1;
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
        failedCount += 1;
        addLog({
          type: "warn",
          scope: "chat-media",
          msg: "处理照片/视频附件失败",
          data: { name: file?.name || "", error: String(error || "") },
        });
      }
    }
    const notices = [];
    if (unsupportedCount > 0) notices.push(`已忽略 ${unsupportedCount} 个非图片/视频文件`);
    if (emptyCount > 0) notices.push(`${emptyCount} 个附件大小为 0`);
    if (oversizeCount > 0) notices.push(`${oversizeCount} 个附件超过 25MB`);
    if (failedCount > 0) notices.push(`${failedCount} 个附件处理失败`);
    if (overflowCount > 0) notices.push(`附件上限 ${limit} 个，已忽略其余 ${overflowCount} 个`);
    if (notices.length > 0) {
      conv.error = notices.join("；");
    }
    state.chat.composerMediaByKey[conv.key] = nextItems;
    touchConversation(conv, { persist: false });
    render();
  }

  async function onComposerFilePicked(inputElement) {
    const conv = activeConversation();
    const files = Array.from(inputElement?.files || []);
    const menuClosed = closeAttachmentMenu();
    if (inputElement) {
      inputElement.value = "";
    }
    if (!conv || files.length === 0) {
      if (menuClosed) render();
      return;
    }

    const current = composerMediaForKey(conv.key);
    const limit = 10;
    if (current.length >= limit) {
      conv.error = `附件上限 ${limit} 个`;
      render();
      return;
    }

    const maxBytes = 50 * 1024 * 1024;
    const nextItems = [...current];
    for (const file of files) {
      if (nextItems.length >= limit) break;
      const mediaType = inferMediaType(file.type, file.name);
      if (mediaType === "image" || mediaType === "video") {
        conv.error = "图片/视频请从“照片/视频”入口选择";
        continue;
      }
      const size = Number(file.size || 0);
      if (size <= 0) {
        conv.error = "文件大小为 0，无法发送";
        continue;
      }
      if (size > maxBytes) {
        conv.error = "单个文件超过 50MB，请压缩后重试";
        continue;
      }
      nextItems.push(normalizeContentPart({
        type: "fileRef",
        mediaId: createId("file"),
        pathHint: file.name,
        fileName: file.name,
        mime: String(file.type || "").trim(),
        size,
        text: `${file.name} (${formatFileSize(size)})`,
      }));
    }

    state.chat.composerMediaByKey[conv.key] = nextItems;
    touchConversation(conv, { persist: false });
    render();
  }

  function toggleComposerAudioPreview(mediaId) {
    const normalizedId = String(mediaId || "").trim();
    if (!normalizedId || !uiRefs?.chatComposerMediaTray) return;
    const audio = Array.from(uiRefs.chatComposerMediaTray.querySelectorAll("[data-chat-audio-id]"))
      .find((node) => String(node.getAttribute("data-chat-audio-id") || "") === normalizedId);
    if (!(audio instanceof HTMLAudioElement)) return;
    if (audio.paused) {
      void audio.play();
      return;
    }
    audio.pause();
    audio.currentTime = 0;
  }

  async function startVoiceRecord() {
    if (recordState.pending || recordState.recording) return;
    const conv = activeConversation();
    if (!conv) return;
    closeAttachmentMenu();
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
      const mimeCandidates = [
        "audio/webm;codecs=opus",
        "audio/webm",
        "audio/ogg;codecs=opus",
        "audio/mp4",
      ];
      let preferredMime = "";
      if (window.MediaRecorder && typeof window.MediaRecorder.isTypeSupported === "function") {
        preferredMime = mimeCandidates.find((candidate) => window.MediaRecorder.isTypeSupported(candidate)) || "";
      }
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
        const recorderMime = String(recorder.mimeType || "").trim().toLowerCase();
        const isVideoLike = recorderMime.startsWith("video/");
        const mime = isVideoLike
          ? "audio/webm"
          : (String(recorder.mimeType || preferredMime || "audio/webm").trim() || "audio/webm");
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
            mime: "audio/webm",
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

  function rememberLaunchRequest(requestId, payload = {}) {
    const id = String(requestId || "").trim();
    if (!id) return;
    state.chat.launchRequestsById[id] = {
      requestId: id,
      conversationKey: String(payload.conversationKey || ""),
      hostId: String(payload.hostId || ""),
      toolName: String(payload.toolName || ""),
      cwd: String(payload.cwd || ""),
      messageId: String(payload.messageId || ""),
    };
  }

  function forgetLaunchRequest(requestId) {
    const id = String(requestId || "").trim();
    if (!id) return;
    delete state.chat.launchRequestsById[id];
  }

  function resolveLaunchConversationKey(hostId, payload) {
    const byPayload = String(payload.conversationKey || "").trim();
    if (byPayload) return byPayload;
    const requestId = String(payload.requestId || "").trim();
    if (requestId) {
      const remembered = asMap(state.chat.launchRequestsById[requestId]);
      const key = String(remembered.conversationKey || "").trim();
      if (key) return key;
    }
    return "";
  }

  function appendLaunchSystemMessage(conv, payload, status, text) {
    if (!conv) return;
    const requestId = String(payload.requestId || "").trim();
    const remembered = requestId ? asMap(state.chat.launchRequestsById[requestId]) : {};
    const messageId = String(remembered.messageId || "").trim();
    let target = null;
    if (messageId) {
      target = conv.messages.find((msg) => String(msg.id || "") === messageId) || null;
    }
    if (!target && requestId) {
      target = conv.messages.find((msg) => String(msg.requestId || "") === requestId && msg.role === "system") || null;
    }
    if (!target) {
      target = {
        id: createId("msg"),
        role: "system",
        text: "",
        status,
        ts: new Date().toISOString(),
        requestId,
        meta: {},
      };
      conv.messages.push(target);
      if (requestId) {
        rememberLaunchRequest(requestId, {
          ...remembered,
          conversationKey: conv.key,
          hostId: conv.hostId,
          messageId: target.id,
        });
      }
    }
    target.status = status;
    target.text = text;
    target.ts = new Date().toISOString();
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
    state.chat.attachmentMenuOpen = false;
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
    state.chat.attachmentMenuOpen = false;
    state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
    state.chat.viewMode = "list";
    render();
  }

  function enterChatTab() {
    const conv = activeConversation();
    if (conv) clearMessageSelection(conv.key);
    if (conv) stopVoiceRecordForConversation(conv.key);
    state.chat.swipedConversationKey = "";
    state.chat.attachmentMenuOpen = false;
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
    clearPendingMediaStageByConversation(normalizedKey);
    stopVoiceRecordForConversation(normalizedKey);
    clearComposerMediaByKey(normalizedKey);
    Object.entries(asMap(state.chat.launchRequestsById)).forEach(([requestId, row]) => {
      if (String(row?.conversationKey || "") === normalizedKey) {
        delete state.chat.launchRequestsById[requestId];
      }
    });
    if (String(state.chat.messageViewer.conversationKey || "") === normalizedKey) {
      state.chat.messageViewer = { conversationKey: "", messageId: "", scale: 1 };
    }

    if (state.chat.conversationOrder.length === 0) {
      state.chat.viewMode = "list";
    }
    state.chat.attachmentMenuOpen = false;
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
    clearPendingMediaStageByConversation(normalizedKey);
    stopVoiceRecordForConversation(normalizedKey);
    clearComposerMediaByKey(normalizedKey);
    Object.entries(asMap(state.chat.launchRequestsById)).forEach(([requestId, row]) => {
      if (String(row?.conversationKey || "") === normalizedKey) {
        delete state.chat.launchRequestsById[requestId];
      }
    });
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

  async function stageMediaPartForQueueItem(conv, item, runtimeToolId, part) {
    const mediaId = String(part.mediaId || "").trim();
    const mime = String(part.mime || "").trim();
    const dataBase64 = String(part.dataBase64 || "").trim();
    if (!mediaId || !mime || !dataBase64) {
      return {
        ok: false,
        mediaId,
        code: "MEDIA_STAGE_NOT_FOUND",
        reason: "附件内容不完整，无法暂存",
      };
    }
    const pending = registerMediaStagePromise(conv.hostId, conv.key, item.requestId, mediaId);
    if (!pending) {
      return {
        ok: false,
        mediaId,
        code: "MEDIA_STAGE_NOT_FOUND",
        reason: "附件暂存队列初始化失败",
      };
    }
    const sent = sendSocketEvent(
      conv.hostId,
      "tool_media_stage_request",
      {
        toolId: runtimeToolId,
        conversationKey: conv.key,
        requestId: item.requestId,
        mediaId,
        mime,
        dataBase64,
        pathHint: String(part.pathHint || part.fileName || "").trim(),
      },
      {
        action: "tool_media_stage_request",
        traceId: item.requestId.replace(/^req_/, "trc_"),
        toolId: runtimeToolId,
      },
    );
    if (!sent) {
      rejectMediaStagePromise(conv.hostId, conv.key, item.requestId, mediaId, {
        ok: false,
        mediaId,
        code: "MEDIA_STAGE_NOT_FOUND",
        reason: "发送暂存请求失败：宿主机未连接",
      });
      return {
        ok: false,
        mediaId,
        code: "MEDIA_STAGE_NOT_FOUND",
        reason: "发送暂存请求失败：宿主机未连接",
      };
    }
    try {
      return await pending;
    } catch (error) {
      const err = asMap(error);
      return {
        ok: false,
        mediaId,
        code: String(err.code || "MEDIA_STAGE_NOT_FOUND"),
        reason: String(err.reason || "附件暂存失败"),
      };
    }
  }

  function applyQueueItemContentUpdate(conv, item, content) {
    const normalized = cloneContentParts(content);
    item.content = normalized;
    const queued = conv.queue.find((row) => String(row.queueItemId || "") === String(item.queueItemId || ""));
    if (queued) queued.content = cloneContentParts(normalized);
    const userMsg = conv.messages.find((msg) => String(msg.queueItemId || "") === String(item.queueItemId || ""));
    if (userMsg) userMsg.content = stripTransientContentFields(normalized);
  }

  async function prepareOutboundContent(conv, item, runtimeToolId) {
    const content = normalizeContentParts(item.content);
    const mediaParts = content.filter((part) => (
      isMediaPartType(part.type)
      && String(part.mediaId || "").trim()
      && !String(part.stagedMediaId || "").trim()
      && String(part.dataBase64 || "").trim()
    ));
    if (!mediaParts.length) {
      return { content, stageFailures: [] };
    }

    mediaParts.forEach((part) => {
      part.stageStatus = "staging";
      part.stageProgress = 0;
      delete part.stageErrorCode;
      delete part.stageErrorReason;
    });
    applyQueueItemContentUpdate(conv, item, content);
    render();

    const results = await Promise.all(mediaParts.map((part) => (
      stageMediaPartForQueueItem(conv, item, runtimeToolId, part)
    )));

    const failures = [];
    results.forEach((row) => {
      const mediaId = String(row.mediaId || "").trim();
      if (!mediaId) return;
      const target = content.find((part) => String(part.mediaId || "").trim() === mediaId);
      if (!target) return;
      if (row.ok) {
        target.stagedMediaId = String(row.stagedMediaId || "").trim();
        target.stageStatus = "staged";
        target.stageProgress = 100;
        delete target.dataBase64;
        delete target.stageErrorCode;
        delete target.stageErrorReason;
      } else {
        const reason = String(row.reason || "附件暂存失败");
        const code = String(row.code || "MEDIA_STAGE_NOT_FOUND");
        target.stageStatus = "failed";
        target.stageProgress = 0;
        delete target.stagedMediaId;
        const allowInlineFallback = code === "MEDIA_STAGE_NOT_FOUND" || code === "MEDIA_STAGE_TIMEOUT";
        if (allowInlineFallback && String(target.dataBase64 || "").trim()) {
          target.stageStatus = "fallback_inline";
          delete target.stageErrorCode;
          delete target.stageErrorReason;
        } else {
          target.stageErrorCode = code;
          target.stageErrorReason = reason;
          delete target.dataBase64;
          failures.push({ mediaId, code, reason });
        }
      }
    });

    applyQueueItemContentUpdate(conv, item, content);
    return { content, stageFailures: failures };
  }

  async function dispatchQueueHead(conversationKey) {
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

    const { content: stagedContent, stageFailures } = await prepareOutboundContent(conv, item, runtimeToolId);
    const outboundContent = normalizeContentParts(stagedContent);
    const text = String(item.text || "").trim() || contentSummaryText(outboundContent);
    const hasTextPart = outboundContent.some((part) => part.type === "text" && String(part.text || "").trim());
    const hasSendableMedia = outboundContent.some((part) => (
      isMediaPartType(part.type)
      && (String(part.stagedMediaId || "").trim() || String(part.dataBase64 || "").trim())
    ));
    const hasFileRef = outboundContent.some((part) => part.type === "fileRef");
    if (!hasTextPart && !hasSendableMedia && !hasFileRef) {
      conv.error = stageFailures.length > 0
        ? `附件暂存失败：${stageFailures.map((row) => row.reason).join("；")}`
        : "发送失败：消息为空";
      const userMsg = conv.messages.find((msg) => String(msg.queueItemId || "") === item.queueItemId);
      if (userMsg) userMsg.status = "failed";
      conv.queue.shift();
      normalizeQueuePanelByConversation(conv);
      touchConversation(conv);
      render();
      window.setTimeout(() => maybeDispatchNext(conv.key), 0);
      return;
    }

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
      userMsg.content = stripTransientContentFields(outboundContent);
    }
    conv.error = "";
    touchConversation(conv);
    render();
  }

  function maybeDispatchNext(conversationKey) {
    const key = String(conversationKey || "").trim();
    if (!key) return;
    if (dispatchingByConversationKey[key]) return;
    dispatchingByConversationKey[key] = true;
    void dispatchQueueHead(key)
      .catch((error) => {
        const conv = state.chat.conversationsByKey[key];
        if (conv) {
          conv.error = `发送失败：${error}`;
          touchConversation(conv);
          render();
        }
      })
      .finally(() => {
        delete dispatchingByConversationKey[key];
      });
  }

  function dispatchLaunchRequest(conv, rawDraft, launchConfirm) {
    const toolName = String(launchConfirm?.toolName || "").trim();
    const cwd = String(launchConfirm?.cwd || "").trim();
    if (!toolName || !cwd) {
      conv.error = "启动提案内容不完整，请重新引用后再试。";
      render();
      return;
    }
    const requestId = createId("lch");
    const userMessage = {
      id: createId("msg"),
      role: "user",
      text: String(rawDraft || "").trim() || buildLaunchConfirmDraft({ toolName, cwd }),
      status: "completed",
      ts: new Date().toISOString(),
      requestId,
    };
    const systemMessage = {
      id: createId("msg"),
      role: "system",
      text: `已提交启动请求：${toolName} @ ${cwd}`,
      status: "sending",
      ts: new Date().toISOString(),
      requestId,
      meta: {},
    };
    conv.messages.push(userMessage);
    conv.messages.push(systemMessage);
    rememberLaunchRequest(requestId, {
      conversationKey: conv.key,
      hostId: conv.hostId,
      toolName,
      cwd,
      messageId: systemMessage.id,
    });

    let sent = false;
    if (typeof requestToolLaunch === "function") {
      const result = requestToolLaunch(conv.hostId, {
        toolName,
        cwd,
        requestId,
        conversationKey: conv.key,
      });
      sent = Boolean(result && result.ok);
    } else {
      sent = sendSocketEvent(
        conv.hostId,
        "tool_launch_request",
        {
          toolName,
          cwd,
          requestId,
          conversationKey: conv.key,
        },
        {
          action: "tool_launch_request",
          traceId: requestId.replace(/^lch_/, "trc_"),
        },
      );
    }

    if (!sent) {
      systemMessage.status = "failed";
      systemMessage.text = `启动请求发送失败：${toolName} @ ${cwd}`;
      conv.error = "发送失败：宿主机未连接";
      forgetLaunchRequest(requestId);
    } else {
      conv.error = "";
    }

    conv.draft = "";
    clearComposerMediaByKey(conv.key);
    closeAttachmentMenu();
    touchConversation(conv);
    render();
  }

  function enqueueMessage() {
    const conv = activeConversation();
    if (!conv) return;
    const menuClosed = closeAttachmentMenu();
    const text = String(conv.draft || "").trim();
    const mediaParts = cloneContentParts(activeComposerMedia());
    const launchConfirm = parseLaunchConfirmFromText(text);
    if (launchConfirm && mediaParts.length === 0) {
      dispatchLaunchRequest(conv, text, launchConfirm);
      return;
    }
    const content = [];
    if (text) {
      content.push({ type: "text", text });
    }
    mediaParts.forEach((part) => content.push(part));
    if (content.length === 0) {
      if (menuClosed) render();
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

  function onToolMediaStageProgress(hostId, payload) {
    const data = asMap(payload);
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    const requestId = String(data.requestId || "").trim();
    const mediaId = String(data.mediaId || "").trim();
    const progress = Math.max(0, Math.min(100, Math.trunc(Number(data.progress || 0))));
    if (!requestId || !mediaId) return;
    const changed = syncStageStateToConversation(conv, requestId, mediaId, {
      stageStatus: progress >= 100 ? "staged" : "staging",
      stageProgress: progress,
    });
    if (changed) {
      touchConversation(conv, { persist: false });
      render();
    }
  }

  function onToolMediaStageFinished(hostId, payload) {
    const data = asMap(payload);
    const requestId = String(data.requestId || "").trim();
    const mediaId = String(data.mediaId || "").trim();
    const stagedMediaId = String(data.stagedMediaId || "").trim();
    if (!requestId || !mediaId) return;
    settleMediaStagePromise(hostId, data, {
      ok: true,
      mediaId,
      stagedMediaId,
      relativePath: String(data.relativePath || "").trim(),
      stagedPath: String(data.stagedPath || "").trim(),
      expiresAt: String(data.expiresAt || "").trim(),
      mime: String(data.mime || "").trim(),
      size: Number(data.size || 0),
      pathHint: String(data.pathHint || "").trim(),
    });
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    syncStageStateToConversation(conv, requestId, mediaId, {
      stageStatus: "staged",
      stageProgress: 100,
      stagedMediaId,
      mime: String(data.mime || "").trim(),
      size: Number(data.size || 0),
      pathHint: String(data.pathHint || "").trim(),
      stageErrorCode: "",
      stageErrorReason: "",
      dataBase64: "",
    });
    touchConversation(conv, { persist: false });
    render();
  }

  function onToolMediaStageFailed(hostId, payload) {
    const data = asMap(payload);
    const requestId = String(data.requestId || "").trim();
    const mediaId = String(data.mediaId || "").trim();
    const code = String(data.code || "MEDIA_STAGE_NOT_FOUND").trim();
    const reason = String(data.reason || "附件暂存失败").trim();
    if (!requestId || !mediaId) return;
    settleMediaStagePromise(hostId, data, {
      ok: false,
      mediaId,
      code,
      reason,
    });
    const conv = ensureConversationByPayload(hostId, data);
    if (!conv) return;
    syncStageStateToConversation(conv, requestId, mediaId, {
      stageStatus: "failed",
      stageProgress: 0,
      stageErrorCode: code,
      stageErrorReason: reason,
      stagedMediaId: "",
    });
    touchConversation(conv, { persist: false });
    render();
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

  function resolveLaunchConversation(hostId, payload) {
    const data = asMap(payload);
    const conversationKey = resolveLaunchConversationKey(hostId, data);
    if (conversationKey && state.chat.conversationsByKey[conversationKey]) {
      return state.chat.conversationsByKey[conversationKey];
    }
    const requestId = String(data.requestId || "").trim();
    if (requestId) {
      const remembered = asMap(state.chat.launchRequestsById[requestId]);
      const rememberedKey = String(remembered.conversationKey || "").trim();
      if (rememberedKey && state.chat.conversationsByKey[rememberedKey]) {
        return state.chat.conversationsByKey[rememberedKey];
      }
    }
    return activeConversation();
  }

  function onToolLaunchStarted(hostId, payload) {
    const data = asMap(payload);
    const conv = resolveLaunchConversation(hostId, data);
    if (!conv) return false;
    const toolName = String(data.toolName || "").trim();
    const cwd = String(data.cwd || "").trim();
    const requestId = String(data.requestId || "").trim();
    appendLaunchSystemMessage(
      conv,
      data,
      "sending",
      `启动流程开始：${toolName || "tool"} @ ${cwd || "--"}`,
    );
    if (requestId) {
      rememberLaunchRequest(requestId, {
        conversationKey: conv.key,
        hostId: conv.hostId,
        toolName,
        cwd,
      });
    }
    touchConversation(conv);
    render();
    return true;
  }

  function onToolLaunchFinished(hostId, payload) {
    const data = asMap(payload);
    const conv = resolveLaunchConversation(hostId, data);
    if (!conv) return false;
    const requestId = String(data.requestId || "").trim();
    const toolName = String(data.toolName || "").trim();
    const cwd = String(data.cwd || "").trim();
    const pidText = Number.isFinite(Number(data.pid)) ? ` (pid ${Number(data.pid)})` : "";
    const reason = String(data.reason || "").trim();
    appendLaunchSystemMessage(
      conv,
      data,
      "completed",
      reason || `启动完成：${toolName || "tool"} @ ${cwd || "--"}${pidText}`,
    );
    conv.error = "";
    if (requestId) {
      forgetLaunchRequest(requestId);
    }
    touchConversation(conv);
    render();
    return true;
  }

  function onToolLaunchFailed(hostId, payload) {
    const data = asMap(payload);
    const conv = resolveLaunchConversation(hostId, data);
    if (!conv) return false;
    const requestId = String(data.requestId || "").trim();
    const toolName = String(data.toolName || "").trim();
    const cwd = String(data.cwd || "").trim();
    const reason = String(data.reason || "").trim() || "启动失败";
    appendLaunchSystemMessage(
      conv,
      data,
      "failed",
      `${reason}（${toolName || "tool"} @ ${cwd || "--"}）`,
    );
    conv.error = reason;
    if (requestId) {
      forgetLaunchRequest(requestId);
    }
    touchConversation(conv);
    render();
    return true;
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
    const previewBtn = event.target.closest("[data-chat-preview-audio]");
    if (previewBtn) {
      const mediaId = String(previewBtn.getAttribute("data-chat-preview-audio") || "").trim();
      if (mediaId) toggleComposerAudioPreview(mediaId);
      return;
    }
    const removeBtn = event.target.closest("[data-chat-remove-media]");
    if (!removeBtn) return;
    const mediaId = String(removeBtn.getAttribute("data-chat-remove-media") || "").trim();
    if (!mediaId) return;
    removeComposerMediaItem(mediaId);
  }

  function toggleAttachmentMenu() {
    const conv = activeConversation();
    if (!conv) return;
    if (String(conv.availability || "").toLowerCase() === "invalid") {
      return;
    }
    const changed = setAttachmentMenuOpen(!Boolean(state.chat.attachmentMenuOpen));
    if (changed) render();
  }

  function onAttachMenuClick(event) {
    const origin = event.target instanceof Element ? event.target : null;
    if (!origin) return;
    const target = origin.closest("[data-chat-attach-action]");
    if (!target) return;
    const action = String(target.getAttribute("data-chat-attach-action") || "").trim();
    closeAttachmentMenu();
    render();
    if (action === "media") {
      if (uiRefs?.chatMediaInput) {
        uiRefs.chatMediaInput.value = "";
        uiRefs.chatMediaInput.setAttribute("accept", MEDIA_INPUT_ACCEPT);
        uiRefs.chatMediaInput.click();
      }
      return;
    }
    if (action === "file") {
      if (uiRefs?.chatFileInput) {
        uiRefs.chatFileInput.value = "";
        uiRefs.chatFileInput.setAttribute("accept", FILE_INPUT_ACCEPT);
        uiRefs.chatFileInput.click();
      }
    }
  }

  function onDocumentPointerDown(event) {
    if (!state.chat.attachmentMenuOpen) return;
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return;
    if (target.closest(".chat-attach-group")) return;
    if (closeAttachmentMenu()) {
      render();
    }
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
    const launchQuoteBtn = event.target.closest("[data-chat-launch-quote]");
    if (launchQuoteBtn) {
      if (state.chat.messageSelectionModeByKey[conv.key]) return;
      const toolName = String(launchQuoteBtn.getAttribute("data-chat-launch-tool") || "").trim();
      const cwd = String(launchQuoteBtn.getAttribute("data-chat-launch-cwd") || "").trim();
      const draft = buildLaunchConfirmDraft({ toolName, cwd });
      if (!draft) return;
      const existing = String(conv.draft || "").trim();
      conv.draft = existing ? `${existing}\n${draft}` : draft;
      conv.error = "";
      render();
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
      state.chat.attachmentMenuOpen = false;
      state.chat.recordingConversationKey = "";
      state.chat.recordingPending = false;
      state.chat.messageSelectionModeByKey = {};
      state.chat.selectedMessageIdsByKey = {};
      state.chat.swipedConversationKey = "";
      state.chat.reportViewer = createReportViewerState();
      state.chat.reportTransfersByRequestId = {};
      state.chat.launchRequestsById = {};
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
      state.chat.attachmentMenuOpen = false;
      state.chat.recordingConversationKey = "";
      state.chat.recordingPending = false;
      state.chat.messageSelectionModeByKey = {};
      state.chat.selectedMessageIdsByKey = {};
      state.chat.swipedConversationKey = "";
      state.chat.reportViewer = createReportViewerState();
      state.chat.reportTransfersByRequestId = {};
      state.chat.launchRequestsById = {};
      state.chat.hydrated = true;
      normalizeConversationOnlineState();
      render();
    }
  }

  function renderSync() {
    normalizeConversationOnlineState();
  }

  function bindEvents(ui) {
    uiRefs = ui;
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
    ui.chatAttachBtn.addEventListener("click", toggleAttachmentMenu);
    ui.chatAttachMenu?.addEventListener("click", onAttachMenuClick);
    ui.chatMediaInput?.addEventListener("change", () => {
      void onComposerMediaPicked(ui.chatMediaInput);
    });
    ui.chatFileInput?.addEventListener("change", () => {
      void onComposerFilePicked(ui.chatFileInput);
    });
    if (!documentPointerListenerBound) {
      document.addEventListener("pointerdown", onDocumentPointerDown, true);
      documentPointerListenerBound = true;
    }
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
    onToolMediaStageProgress,
    onToolMediaStageFinished,
    onToolMediaStageFailed,
    onToolChatStarted,
    onToolChatChunk,
    onToolChatFinished,
    onToolReportFetchStarted,
    onToolReportFetchChunk,
    onToolReportFetchFinished,
    onToolLaunchStarted,
    onToolLaunchFinished,
    onToolLaunchFailed,
    enqueueMessage,
    stopRunningMessage,
    openConversation,
    deleteConversationByKey,
    deleteConversationByTool,
    deleteConversationsByHost,
  };
}
