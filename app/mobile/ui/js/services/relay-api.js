// 文件职责：
// 1. 统一 Relay URL 解析与 HTTP API 基址推导。
// 2. 提供带网络容错的 JSON 请求函数，兼容 localhost/127.0.0.1 调试。

function buildHostWithPort(hostname, port) {
  const withBracket = hostname.includes(":") && !hostname.startsWith("[")
    ? `[${hostname}]`
    : hostname;
  return port ? `${withBracket}:${port}` : withBracket;
}

/**
 * 解析并校验 Relay WebSocket 地址。
 * @param {string} relayWsUrl Relay WS URL。
 * @returns {URL} 标准化 URL 实例。
 * @throws {Error} 地址协议不合法时抛错。
 */
export function parseRelayWsUrl(relayWsUrl) {
  const raw = String(relayWsUrl || "").trim();
  const ws = new URL(raw);
  if (ws.protocol !== "ws:" && ws.protocol !== "wss:") {
    const err = new Error(`relay url protocol unsupported: ${ws.protocol}`);
    err.code = "RELAY_URL_INVALID";
    throw err;
  }
  return ws;
}

/**
 * 根据 Relay WS 地址推导 HTTP API 候选基址（含 localhost/127.0.0.1 互通）。
 * @param {string} relayWsUrl Relay WS URL。
 * @returns {string[]} API 基址列表。
 */
export function relayApiBases(relayWsUrl) {
  const ws = parseRelayWsUrl(relayWsUrl);
  const protocol = ws.protocol === "wss:" ? "https:" : "http:";
  const pathname = ws.pathname.endsWith("/ws") ? ws.pathname.slice(0, -3) : ws.pathname;
  const normalizedPath = pathname.replace(/\/+$/, "");

  const hosts = [ws.host];
  const hostName = ws.hostname.toLowerCase();
  if (hostName === "127.0.0.1") {
    hosts.push(buildHostWithPort("localhost", ws.port));
  } else if (hostName === "localhost") {
    hosts.push(buildHostWithPort("127.0.0.1", ws.port));
  }

  return [...new Set(hosts)].map((host) => `${protocol}//${host}${normalizedPath}`);
}

/**
 * 判定是否为网络可达性类错误。
 * @param {unknown} error 异常对象。
 * @returns {boolean}
 */
export function isRelayNetworkError(error) {
  const text = String(error || "");
  return (
    error instanceof TypeError ||
    /failed to fetch|networkerror|load failed|operation not permitted|network request failed/i.test(text)
  );
}

/**
 * 以 JSON 方式请求 Relay API，并在本地调试地址间自动回退。
 * @param {string} relayWsUrl Relay WS URL。
 * @param {string} path 接口路径（以 `/` 开头）。
 * @param {RequestInit} init fetch 参数。
 * @returns {Promise<{resp: Response, body: any, apiBase: string}>}
 */
export async function relayRequestJson(relayWsUrl, path, init) {
  const bases = relayApiBases(relayWsUrl);
  let lastNetworkError = null;

  for (const base of bases) {
    try {
      const resp = await fetch(`${base}${path}`, init);
      const text = await resp.text();
      let body = {};
      if (text) {
        try {
          body = JSON.parse(text);
        } catch (parseError) {
          const err = new Error(`relay response not json: ${parseError}`);
          err.code = "RELAY_RESPONSE_INVALID";
          throw err;
        }
      }
      return { resp, body, apiBase: base };
    } catch (error) {
      if (!isRelayNetworkError(error)) {
        throw error;
      }
      lastNetworkError = error;
    }
  }

  if (lastNetworkError) {
    const err = new Error(`relay unreachable: ${lastNetworkError}`);
    err.code = "RELAY_UNREACHABLE";
    throw err;
  }
  const err = new Error("relay request failed");
  err.code = "RELAY_UNREACHABLE";
  throw err;
}
