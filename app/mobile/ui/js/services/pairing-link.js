// 文件职责：
// 1. 解析手动输入/扫码导入的 yc://pair 链接。
// 2. 提供 pairCode 解析逻辑，兼容 sid+ticket 与历史 code 链路。

import { parseRelayWsUrl } from "./relay-api.js";

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
  const raw = String(rawValue || "").trim();
  if (!raw) {
    return null;
  }

  const matched = raw.match(/yc:\/\/pair\?[^ "'<>]+/i);
  const linkText = matched ? matched[0] : raw;
  let parsedUrl = null;
  try {
    parsedUrl = new URL(linkText);
  } catch (_) {
    return null;
  }

  if (parsedUrl.protocol !== "yc:" || parsedUrl.hostname !== "pair") {
    return null;
  }

  const relayUrl = String(parsedUrl.searchParams.get("relay") || "").trim();
  const pairCode = String(parsedUrl.searchParams.get("code") || "").trim();
  const systemIdFromSid = String(parsedUrl.searchParams.get("sid") || "").trim();
  const pairTicket = String(parsedUrl.searchParams.get("ticket") || "").trim();
  const hostName = String(parsedUrl.searchParams.get("name") || "").trim();
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
