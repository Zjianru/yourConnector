// 文件职责：
// 1. 管理宿主机会话凭证的 Keychain 读写。
// 2. 执行 accessToken 刷新流程。

import { relayRequestJson } from "../services/relay-api.js";
import { asMap } from "../utils/type.js";

/** 创建连接鉴权能力（会话读取/落盘/刷新）。 */
export function createConnectionAuth({ state, hostById, ensureRuntime, createEventId, tauriInvoke, addLog }) {
  async function loadHostSession(hostId) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (!host || !runtime) {
      return null;
    }
    try {
      const session = await tauriInvoke("auth_load_session", {
        systemId: host.systemId,
        deviceId: state.deviceId,
      });
      if (!session) {
        return null;
      }
      runtime.accessToken = String(session.accessToken || "");
      runtime.refreshToken = String(session.refreshToken || "");
      runtime.keyId = String(session.keyId || "");
      runtime.credentialId = String(session.credentialId || "");
      return session;
    } catch (error) {
      addLog(`load secure session failed (${host.displayName}): ${error}`);
      return null;
    }
  }

  async function storeHostSession(hostId) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (!host || !runtime) {
      return;
    }
    await tauriInvoke("auth_store_session", {
      session: {
        systemId: host.systemId,
        deviceId: state.deviceId,
        accessToken: runtime.accessToken,
        refreshToken: runtime.refreshToken,
        keyId: runtime.keyId,
        credentialId: runtime.credentialId,
      },
    });
  }

  async function clearHostSession(systemId, deviceId = state.deviceId) {
    const normalizedDeviceId = String(deviceId || "").trim() || state.deviceId;
    await tauriInvoke("auth_clear_session", { systemId, deviceId: normalizedDeviceId });
  }

  async function refreshAccessTokenIfPossible(hostId) {
    const host = hostById(hostId);
    const runtime = ensureRuntime(hostId);
    if (!host || !runtime) return false;
    if (!runtime.refreshToken || !runtime.keyId || !host.systemId || !state.deviceId) {
      return false;
    }

    try {
      const ts = String(Math.floor(Date.now() / 1000));
      const nonce = createEventId();
      const payload = `auth-refresh\n${host.systemId}\n${state.deviceId}\n${runtime.keyId}\n${ts}\n${nonce}`;
      const signed = await tauriInvoke("auth_sign_payload", {
        deviceId: state.deviceId,
        payload,
      });

      const { resp, body } = await relayRequestJson(host.relayUrl, "/auth/refresh", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          systemId: host.systemId,
          deviceId: state.deviceId,
          refreshToken: runtime.refreshToken,
          keyId: String(signed.keyId || runtime.keyId),
          ts,
          nonce,
          sig: String(signed.signature || ""),
        }),
      });

      if (!resp.ok || !body.ok) {
        addLog(
          `refresh skipped (${host.displayName}): ${body.code || resp.status} ${body.message || ""}`,
        );
        return false;
      }

      const data = asMap(body.data);
      runtime.accessToken = String(data.accessToken || runtime.accessToken);
      runtime.refreshToken = String(data.refreshToken || runtime.refreshToken);
      runtime.keyId = String(data.keyId || runtime.keyId);
      runtime.credentialId = String(data.credentialId || runtime.credentialId);
      await storeHostSession(hostId);
      return true;
    } catch (error) {
      addLog(`refresh failed (${host.displayName}): ${error}`);
      return false;
    }
  }

  return {
    loadHostSession,
    storeHostSession,
    clearHostSession,
    refreshAccessTokenIfPossible,
  };
}
