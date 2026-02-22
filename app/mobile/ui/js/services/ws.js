// 文件职责：
// 1. 统一 App 侧 WS 连接 URL 组装逻辑。
// 2. 将协议字段拼装从业务流程中抽离，减少重复代码。

export function buildAppWsUrl({ relayUrl, systemId, deviceId, accessToken, keyId, ts, nonce, sig }) {
  const url = new URL(relayUrl);
  url.searchParams.set("clientType", "app");
  url.searchParams.set("systemId", systemId);
  url.searchParams.set("deviceId", deviceId);
  url.searchParams.set("accessToken", accessToken);
  url.searchParams.set("keyId", keyId);
  url.searchParams.set("ts", ts);
  url.searchParams.set("nonce", nonce);
  url.searchParams.set("sig", sig);
  return url;
}
