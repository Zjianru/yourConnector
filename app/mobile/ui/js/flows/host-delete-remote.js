// 文件职责：
// 1. 执行删除补偿中的远端鉴权刷新与设备吊销。
// 2. 处理 stale/terminal 等删除补偿关键分支。

import { createRuntime, state } from "../state/store.js";
import { relayRequestJson } from "../services/relay-api.js";
import { asMap } from "../utils/type.js";
import { DELETE_TERMINAL_RELAY_CODES, errorWithCode, pendingDeleteDeviceId } from "./host-delete-errors.js";

/**
 * 创建删除补偿远端操作能力。
 * @param {object} deps 依赖集合。
 * @returns {{revokeAndClearPendingHost: Function}}
 */
export function createHostDeleteRemote({
  createEventId,
  tauriInvoke,
  clearHostSession,
}) {
  /**
   * 远端刷新会话（可选）。
   * @param {object} item 待补偿宿主机条目。
   * @param {object} runtimeLike 运行态凭证对象。
   * @returns {Promise<boolean>}
   */
  async function refreshPendingSessionIfPossible(item, runtimeLike) {
    if (!runtimeLike.refreshToken || !runtimeLike.keyId) return false;

    const ts = String(Math.floor(Date.now() / 1000));
    const nonce = createEventId();
    const deviceId = pendingDeleteDeviceId(item);
    const payload = `auth-refresh\n${item.systemId}\n${deviceId}\n${runtimeLike.keyId}\n${ts}\n${nonce}`;
    const signed = await tauriInvoke("auth_sign_payload", { deviceId, payload });

    const { resp, body } = await relayRequestJson(item.relayUrl, "/auth/refresh", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        systemId: item.systemId,
        deviceId,
        refreshToken: runtimeLike.refreshToken,
        keyId: String(signed.keyId || runtimeLike.keyId),
        ts,
        nonce,
        sig: String(signed.signature || ""),
      }),
    });

    if (!resp.ok || !body.ok) {
      const code = String(body && body.code ? body.code : "").trim();
      if (DELETE_TERMINAL_RELAY_CODES.has(code)) {
        throw errorWithCode("DELETE_COMPENSATION_TERMINAL", `${code} ${body.message || "refresh failed"}`);
      }
      return false;
    }

    const data = asMap(body.data);
    runtimeLike.accessToken = String(data.accessToken || runtimeLike.accessToken);
    runtimeLike.refreshToken = String(data.refreshToken || runtimeLike.refreshToken);
    runtimeLike.keyId = String(data.keyId || runtimeLike.keyId);
    runtimeLike.credentialId = String(data.credentialId || runtimeLike.credentialId);

    await tauriInvoke("auth_store_session", {
      session: {
        systemId: item.systemId,
        deviceId,
        accessToken: runtimeLike.accessToken,
        refreshToken: runtimeLike.refreshToken,
        keyId: runtimeLike.keyId,
        credentialId: runtimeLike.credentialId,
      },
    });
    return true;
  }

  /**
   * 吊销并清理单条删除补偿记录。
   * @param {object} item 补偿条目。
   */
  async function revokeAndClearPendingHost(item) {
    const deviceId = pendingDeleteDeviceId(item);
    const session = await tauriInvoke("auth_load_session", { systemId: item.systemId, deviceId });
    if (!session) {
      throw errorWithCode("DELETE_COMPENSATION_NO_SESSION", "本地设备凭证不存在");
    }

    const currentCredentialId = String(session.credentialId || "").trim();
    const currentKeyId = String(session.keyId || "").trim();
    const expectedCredentialId = String(item.expectedCredentialId || "").trim();
    const expectedKeyId = String(item.expectedKeyId || "").trim();
    if (
      (expectedCredentialId && currentCredentialId && expectedCredentialId !== currentCredentialId)
      || (expectedKeyId && currentKeyId && expectedKeyId !== currentKeyId)
    ) {
      throw errorWithCode("DELETE_COMPENSATION_STALE", "stale pending delete");
    }

    const hostRuntime = createRuntime();
    hostRuntime.accessToken = String(session.accessToken || "");
    hostRuntime.refreshToken = String(session.refreshToken || "");
    hostRuntime.keyId = String(session.keyId || "");
    hostRuntime.credentialId = String(session.credentialId || "");
    if (!hostRuntime.accessToken || !hostRuntime.refreshToken || !hostRuntime.keyId) {
      throw new Error("设备凭证不完整");
    }

    await refreshPendingSessionIfPossible(item, hostRuntime);

    const ts = String(Math.floor(Date.now() / 1000));
    const nonce = createEventId();
    const payload = `auth-revoke\n${item.systemId}\n${deviceId}\n${deviceId}\n${hostRuntime.keyId}\n${ts}\n${nonce}`;
    const signed = await tauriInvoke("auth_sign_payload", { deviceId, payload });

    const { resp, body } = await relayRequestJson(item.relayUrl, "/auth/revoke-device", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        systemId: item.systemId,
        deviceId,
        targetDeviceId: deviceId,
        accessToken: hostRuntime.accessToken,
        keyId: String(signed.keyId || hostRuntime.keyId),
        ts,
        nonce,
        sig: String(signed.signature || ""),
      }),
    });

    if (!resp.ok || !body.ok) {
      const code = String(body && body.code ? body.code : "").trim();
      const message = String(body && body.message ? body.message : "吊销失败");
      if (DELETE_TERMINAL_RELAY_CODES.has(code)) {
        throw errorWithCode("DELETE_COMPENSATION_TERMINAL", `${code} ${message}`);
      }
      throw errorWithCode(code || "DELETE_COMPENSATION_RETRYABLE", `${code || resp.status} ${message}`);
    }

    await clearHostSession(item.systemId, deviceId);
  }

  return { revokeAndClearPendingHost };
}
