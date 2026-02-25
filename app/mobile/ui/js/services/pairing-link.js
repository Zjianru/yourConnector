// 文件职责：
// 1. 解析手动输入/扫码导入的 yc://pair 链接。
// 2. 提供 pairCode 解析逻辑，兼容 sid+ticket 与历史 code 链路。

import { parseRelayWsUrl } from "./relay-api.js";

/**
 * 规范化粘贴文本，去除富文本与不可见字符干扰。
 * @param {string} rawValue 用户粘贴原文。
 * @returns {string}
 */
function normalizePairingInput(rawValue) {
  return String(rawValue || "")
    .replace(/[\u200B-\u200D\uFEFF]/g, "")
    .replace(/&amp;/gi, "&")
    .replace(/＆/g, "&")
    .replace(/？/g, "?")
    .trim();
}

/**
 * 解析历史 `systemId.pairToken` 形式配对码。
 * @param {string} rawValue 用户输入原文。
 * @returns {{systemId: string, pairToken: string}|null} 成功返回结构化结果。
 */
export function parsePairCode(rawValue) {
  const raw = String(rawValue || "").trim();
  if (!raw) {
    return null;
  }

  const cleaned = raw.replace(/\s+/g, "");
  const splitAt = cleaned.indexOf(".");
  if (splitAt <= 0 || splitAt >= cleaned.length - 1) {
    return null;
  }

  const systemId = cleaned.slice(0, splitAt);
  const pairToken = cleaned.slice(splitAt + 1);
  if (!systemId || !pairToken) {
    return null;
  }
  return { systemId, pairToken };
}

/**
 * 解析 `yc://pair` 配对链接。
 * @param {string} rawValue 用户粘贴文本或扫码结果。
 * @returns {{relayUrl: string, pairCode: string, systemId: string, pairToken: string, pairTicket: string, hostName: string}|null}
 */
export function parsePairingLink(rawValue) {
  const raw = normalizePairingInput(rawValue);
  if (!raw) {
    return null;
  }

  const matched = raw.match(/yc:\/\/pair\?[^\s"'<>]+/i);
  const linkText = matched ? matched[0] : raw;
  if (!/^yc:\/\/pair\?/i.test(linkText)) {
    return null;
  }

  const queryStart = linkText.indexOf("?");
  if (queryStart < 0 || queryStart >= linkText.length - 1) {
    return null;
  }
  const params = new URLSearchParams(linkText.slice(queryStart + 1));

  const relayUrl = String(params.get("relay") || "").trim();
  const pairCode = String(params.get("code") || "").trim();
  const systemIdFromSid = String(params.get("sid") || "").trim();
  const pairTicket = String(params.get("ticket") || "").trim();
  const hostName = String(params.get("name") || "").trim();
  if (!relayUrl) {
    return null;
  }
  try {
    parseRelayWsUrl(relayUrl);
  } catch (_) {
    return null;
  }

  if (systemIdFromSid && pairTicket) {
    return {
      relayUrl,
      pairCode: "",
      systemId: systemIdFromSid,
      pairToken: "",
      pairTicket,
      hostName,
    };
  }

  if (pairCode) {
    const parsedCode = parsePairCode(pairCode);
    if (!parsedCode) {
      return null;
    }
    return {
      relayUrl,
      pairCode,
      systemId: parsedCode.systemId,
      pairToken: parsedCode.pairToken,
      pairTicket: "",
      hostName,
    };
  }

  return null;
}
