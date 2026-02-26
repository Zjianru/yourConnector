// 文件职责：
// 1. 封装 Markdown 渲染，统一聊天消息与报告弹窗显示效果。
// 2. 在非代码块文本中识别绝对 .md 路径并转换为可点击报告链接。
// 3. 保持渲染安全边界（关闭原生 HTML）。

import { escapeHtml } from "./dom.js";

const REPORT_LINK_SCHEME = "yc-report://";
const REPORT_PATH_REGEX = /\/[^\s`<>\[\]\(\)"']+\.md\b/g;
const INLINE_CODE_PLACEHOLDER_REGEX = /`[^`]*`/g;

let markdownRenderer = null;

function getMarkdownRenderer() {
  if (markdownRenderer) return markdownRenderer;
  const factory = window.markdownit;
  if (typeof factory !== "function") return null;
  markdownRenderer = factory({
    html: false,
    linkify: true,
    breaks: true,
  });
  return markdownRenderer;
}

function shouldSkipPathMatch(line, start) {
  const hasValidBoundary = start === 0 || /[\s([{"'<>|]/.test(line[start - 1] || "");
  if (!hasValidBoundary) return true;
  const prev = start > 0 ? line[start - 1] : "";
  const prevTwo = start >= 2 ? line.slice(start - 2, start) : "";
  if (prevTwo === "](") return true;
  if (prev === ":") return true;
  return false;
}

function replaceReportPathsInLine(line) {
  const inlineCodeTokens = [];
  const protectedLine = String(line || "").replace(INLINE_CODE_PLACEHOLDER_REGEX, (chunk) => {
    const token = `@@YC_INLINE_CODE_${inlineCodeTokens.length}@@`;
    inlineCodeTokens.push(chunk);
    return token;
  });

  let output = "";
  let cursor = 0;
  let replaced = false;
  REPORT_PATH_REGEX.lastIndex = 0;
  let match = REPORT_PATH_REGEX.exec(protectedLine);
  while (match) {
    const path = String(match[0] || "");
    const start = Number(match.index || 0);
    if (!path || shouldSkipPathMatch(protectedLine, start)) {
      match = REPORT_PATH_REGEX.exec(protectedLine);
      continue;
    }
    output += protectedLine.slice(cursor, start);
    output += `[${path}](${REPORT_LINK_SCHEME}${encodeURIComponent(path)})`;
    cursor = start + path.length;
    replaced = true;
    match = REPORT_PATH_REGEX.exec(protectedLine);
  }
  output += protectedLine.slice(cursor);
  const restored = output.replace(/@@YC_INLINE_CODE_(\d+)@@/g, (_all, rawIndex) => {
    const index = Number(rawIndex);
    return inlineCodeTokens[index] || "";
  });
  return replaced ? restored : String(line || "");
}

function replaceReportPathsOutsideCodeBlocks(text) {
  const normalized = String(text || "")
    .replace(/\r\n/g, "\n")
    .replace(/\\n/g, "\n");
  const lines = normalized.split("\n");
  const output = [];
  let inFence = false;
  let fenceChar = "";
  let fenceLength = 0;

  for (const rawLine of lines) {
    const line = String(rawLine || "");
    const fence = line.match(/^\s*(`{3,}|~{3,})/);
    if (fence) {
      const marker = String(fence[1] || "");
      const char = marker[0] || "";
      const length = marker.length;
      if (!inFence) {
        inFence = true;
        fenceChar = char;
        fenceLength = length;
      } else if (char === fenceChar && length >= fenceLength) {
        inFence = false;
        fenceChar = "";
        fenceLength = 0;
      }
      output.push(line);
      continue;
    }
    if (inFence) {
      output.push(line);
      continue;
    }
    output.push(replaceReportPathsInLine(line));
  }

  return output.join("\n");
}

function rewriteReportLinkAnchors(html) {
  return String(html || "").replace(/<a href="yc-report:\/\/([^"]+)"([^>]*)>/g, (_all, encodedPath, attrs) => {
    let decodedPath = String(encodedPath || "");
    try {
      decodedPath = decodeURIComponent(decodedPath);
    } catch (_) {
      // keep encoded fallback
    }
    return `<a href="#" data-chat-report-path="${escapeHtml(decodedPath)}"${attrs || ""}>`;
  });
}

/**
 * 渲染 Markdown 到安全 HTML。
 * @param {string} rawText 原始消息文本。
 * @returns {string}
 */
export function renderMarkdown(rawText) {
  const source = replaceReportPathsOutsideCodeBlocks(rawText);
  const renderer = getMarkdownRenderer();
  if (!renderer) {
    const escaped = escapeHtml(source).replace(/\n/g, "<br />");
    return `<p>${escaped}</p>`;
  }
  return rewriteReportLinkAnchors(renderer.render(source));
}

/**
 * 从点击事件目标提取报告路径。
 * @param {EventTarget|null} target 点击事件目标。
 * @returns {string}
 */
export function resolveReportPathFromTarget(target) {
  const node = target && typeof target.closest === "function"
    ? target
    : (target && target.parentElement && typeof target.parentElement.closest === "function"
      ? target.parentElement
      : null);
  if (!node) return "";
  const trigger = node.closest("[data-chat-report-path]");
  if (!trigger) return "";
  return String(trigger.getAttribute("data-chat-report-path") || "").trim();
}
