// 文件职责：
// 1. 封装 Markdown 渲染，统一聊天消息与报告弹窗显示效果。
// 2. 在非代码块文本中识别绝对 .md 路径并转换为可点击报告链接。
// 3. 保持渲染安全边界（关闭原生 HTML）。

import { escapeHtml } from "./dom.js";

const REPORT_LINK_SCHEME = "yc-report://";
const REPORT_PATH_REGEX = /(?:\/|~\/)[^\s`<>\[\]\(\)"']+\.md\b/g;
const INLINE_CODE_PLACEHOLDER_REGEX = /`[^`]*`/g;
const SENSITIVE_RULE_MARKDOWN_BASENAMES = new Set([
  "agents.md",
  "tools.md",
  "identity.md",
  "user.md",
  "heartbeat.md",
  "bootstrap.md",
  "memory.md",
  "soul.md",
]);

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

function isSensitiveRuleMarkdownPath(path) {
  const normalized = String(path || "").trim().toLowerCase();
  if (!normalized.endsWith(".md")) return false;
  const fileName = normalized.split("/").pop() || "";
  if (!SENSITIVE_RULE_MARKDOWN_BASENAMES.has(fileName)) return false;
  return !normalized.includes("/output/")
    && !normalized.includes("/report/")
    && !normalized.includes("/reports/");
}

function normalizeReportPath(raw) {
  let path = String(raw || "").trim();
  if (!path) return "";

  if (path.startsWith(REPORT_LINK_SCHEME)) {
    path = path.slice(REPORT_LINK_SCHEME.length);
    try {
      path = decodeURIComponent(path);
    } catch (_) {
      // keep raw value
    }
  } else if (path.toLowerCase().startsWith("file://")) {
    try {
      const url = new URL(path);
      path = decodeURIComponent(url.pathname || "");
    } catch (_) {
      return "";
    }
  } else {
    try {
      path = decodeURIComponent(path);
    } catch (_) {
      // keep raw value
    }
  }

  if (!path.startsWith("/") && !path.startsWith("~/")) return "";
  if (!/\.md$/i.test(path)) return "";
  if (isSensitiveRuleMarkdownPath(path)) return "";
  return path;
}

function decodeReportPathFromHref(rawHref) {
  const href = String(rawHref || "").trim();
  if (!href) return "";
  return normalizeReportPath(href);
}

function replaceReportPathsInLine(line) {
  const inlineCodeTokens = [];
  let inlineReplaced = false;
  const protectedLine = String(line || "").replace(INLINE_CODE_PLACEHOLDER_REGEX, (chunk) => {
    const inline = String(chunk || "");
    const inlinePath = normalizeReportPath(inline.slice(1, -1));
    const token = `@@YC_INLINE_CODE_${inlineCodeTokens.length}@@`;
    if (inlinePath) {
      inlineReplaced = true;
      inlineCodeTokens.push(`[${inlinePath}](${REPORT_LINK_SCHEME}${encodeURIComponent(inlinePath)})`);
      return token;
    }
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
    const normalizedPath = normalizeReportPath(path);
    const start = Number(match.index || 0);
    if (!normalizedPath || shouldSkipPathMatch(protectedLine, start)) {
      match = REPORT_PATH_REGEX.exec(protectedLine);
      continue;
    }
    output += protectedLine.slice(cursor, start);
    output += `[${normalizedPath}](${REPORT_LINK_SCHEME}${encodeURIComponent(normalizedPath)})`;
    cursor = start + path.length;
    replaced = true;
    match = REPORT_PATH_REGEX.exec(protectedLine);
  }
  output += protectedLine.slice(cursor);
  const restored = output.replace(/@@YC_INLINE_CODE_(\d+)@@/g, (_all, rawIndex) => {
    const index = Number(rawIndex);
    return inlineCodeTokens[index] || "";
  });
  return (replaced || inlineReplaced) ? restored : String(line || "");
}

function rewriteMarkdownReportLinkTargets(line) {
  return String(line || "").replace(/\]\(([^)\s]+)\)/g, (all, rawHref) => {
    const reportPath = decodeReportPathFromHref(rawHref);
    if (!reportPath) {
      return all;
    }
    return `](${REPORT_LINK_SCHEME}${encodeURIComponent(reportPath)})`;
  });
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
    const rewrittenLine = rewriteMarkdownReportLinkTargets(line);
    output.push(replaceReportPathsInLine(rewrittenLine));
  }

  return output.join("\n");
}

function rewriteReportLinkAnchors(html) {
  return String(html || "").replace(/<a href="([^"]+)"([^>]*)>/g, (all, href, attrs) => {
    const reportPath = decodeReportPathFromHref(href);
    if (!reportPath) {
      return all;
    }
    return `<a href="#" data-chat-report-path="${escapeHtml(reportPath)}"${attrs || ""}>`;
  });
}

function rewriteReportPathCodeBlocks(html) {
  return String(html || "").replace(/<pre><code>([\s\S]*?)<\/code><\/pre>/g, (all, rawCode) => {
    const normalizedCode = String(rawCode || "")
      .replace(/&lt;/g, "<")
      .replace(/&gt;/g, ">")
      .replace(/&amp;/g, "&")
      .replace(/&#39;/g, "'")
      .replace(/&quot;/g, "\"")
      .trim();
    if (!normalizedCode) return all;
    const singleLine = normalizedCode.split("\n").map((line) => line.trim()).filter(Boolean);
    if (singleLine.length !== 1) return all;
    const reportPath = normalizeReportPath(singleLine[0]);
    if (!reportPath) return all;
    return `<p><a href="#" class="chat-report-code-link" data-chat-report-path="${escapeHtml(reportPath)}">${escapeHtml(reportPath)}</a></p>`;
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
  const rendered = renderer.render(source);
  return rewriteReportLinkAnchors(rewriteReportPathCodeBlocks(rendered));
}

/**
 * 归一化并校验报告路径（仅允许可预览的报告文件）。
 * @param {string} raw 原始路径。
 * @returns {string}
 */
export function normalizeReportPathForPreview(raw) {
  return normalizeReportPath(raw);
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
