// 文件职责：
// 1. 承载配对执行主链路（preflight -> exchange -> 本地落盘 -> 连接）。
// 2. 承载配对链接导入入口（粘贴、手动、深链、启动参数）。

import { parsePairingLink } from "../services/pairing-link.js";
import { parseRelayWsUrl, relayRequestJson } from "../services/relay-api.js";
import { normalizedDeviceName } from "../utils/platform.js";
import { asMap } from "../utils/type.js";

/**
 * 创建配对执行器（预检、换发、会话落盘、连接触发）。
 * @param {object} deps 依赖集合。
 */
export function createPairingRunner({
  state,
  ui,
  mapPairFailure,
  showPairFailure,
  hostById,
  ensureRuntime,
  recomputeSelections,
  persistConfig,
  createEventId,
  storeHostSession,
  closePairFlow,
  closeHostManageModal,
  connectHost,
  notifyIfDuplicateDisplayName,
  tauriInvoke,
}) {
  async function runPairingFromLink(rawValue, source = "paste") {
    const parsed = parsePairingLink(rawValue);
    if (!parsed) {
      showPairFailure(mapPairFailure("INVALID_LINK", "配对链接格式无效", "请检查链接是否完整。", "paste"));
      return;
    }
    await runPairing(parsed, source);
  }

  async function runPairingFromManual() {
    const relayUrl = String(ui.hostRelayInput.value || "").trim();
    const systemId = String(ui.hostSystemIdInput.value || "").trim();
    const pairTicket = String(ui.hostPairTicketInput.value || "").trim();
    const hostName = String(ui.hostNameInput.value || "").trim();
    if (!relayUrl || !systemId || !pairTicket) {
      showPairFailure(
        mapPairFailure(
          "PAIR_TICKET_INVALID",
          "手动配对信息不完整",
          "请确认 Relay 地址、System ID 与配对票据。",
          "manual",
        ),
      );
      return;
    }
    try {
      parseRelayWsUrl(relayUrl);
    } catch (_) {
      showPairFailure(
        mapPairFailure(
          "RELAY_URL_INVALID",
          "Relay 地址格式无效",
          "请填写 ws:// 或 wss:// 开头的地址。",
          "manual",
        ),
      );
      return;
    }

    await runPairing({ relayUrl, pairCode: "", systemId, pairToken: "", pairTicket, hostName }, "manual");
  }

  async function runPairing(parsed, source) {
    if (state.pairingBusy) return;
    state.pairingBusy = true;

    try {
      const relayUrl = String(parsed.relayUrl || "").trim();
      const systemId = String(parsed.systemId || "").trim();
      const pairToken = String(parsed.pairToken || "").trim();
      const pairTicket = String(parsed.pairTicket || "").trim();
      // App 侧禁止 fallback 到 pairToken，必须走 ticket + 凭证换发。
      if (pairToken && !pairTicket) {
        showPairFailure(
          mapPairFailure(
            "PAIR_TOKEN_NOT_SUPPORTED",
            "当前版本不支持 pairToken 配对",
            "请重新生成包含 sid + ticket 的配对链接。",
            source,
          ),
        );
        return;
      }
      if (!relayUrl || !systemId || !pairTicket) {
        showPairFailure(
          mapPairFailure(
            "PAIR_TICKET_INVALID",
            "配对信息不完整",
            "请重新导入配对信息后重试。",
            source,
          ),
        );
        return;
      }

      const preflightReq = {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          systemId,
          deviceId: state.deviceId,
          pairTicket: pairTicket || undefined,
        }),
      };
      const { resp: preflightResp, body: preflightBody } = await relayRequestJson(
        relayUrl,
        "/pair/preflight",
        preflightReq,
      );
      if (!preflightResp.ok || !preflightBody.ok) {
        const action = source === "manual"
          ? "manual"
          : source === "scan" || source === "gallery"
            ? "scan"
            : "paste";
        showPairFailure(
          mapPairFailure(
            preflightBody.code,
            preflightBody.message,
            preflightBody.suggestion,
            action,
          ),
        );
        return;
      }

      const binding = await tauriInvoke("auth_get_device_binding", { deviceId: state.deviceId });
      const keyId = String(binding.keyId || "");
      const devicePubKey = String(binding.publicKey || "");
      const proofPayload = `pair-exchange\n${systemId}\n${state.deviceId}\n${keyId}`;
      const proofSigned = await tauriInvoke("auth_sign_payload", {
        deviceId: state.deviceId,
        payload: proofPayload,
      });

      const exchangeReq = {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          systemId,
          deviceId: state.deviceId,
          deviceName: normalizedDeviceName(),
          pairTicket: pairTicket || undefined,
          keyId,
          devicePubKey,
          proof: String(proofSigned.signature || ""),
        }),
      };

      const { resp: exchangeResp, body: exchangeBody } = await relayRequestJson(
        relayUrl,
        "/pair/exchange",
        exchangeReq,
      );
      if (!exchangeResp.ok || !exchangeBody.ok) {
        const action = source === "manual"
          ? "manual"
          : source === "scan" || source === "gallery"
            ? "scan"
            : "paste";
        showPairFailure(
          mapPairFailure(
            exchangeBody.code,
            exchangeBody.message,
            exchangeBody.suggestion,
            action,
          ),
        );
        return;
      }

      const exchangeData = asMap(exchangeBody.data);
      const targetHost = hostById(state.pairTargetHostId);
      const existing = state.hosts.find((host) => host.systemId === systemId && host.relayUrl === relayUrl);
      const host = targetHost || existing;
      const nowIso = new Date().toISOString();

      let hostId = "";
      if (host) {
        hostId = host.hostId;
        host.systemId = systemId;
        host.relayUrl = relayUrl;
        host.displayName = String(parsed.hostName || host.displayName || systemId).trim() || systemId;
        host.updatedAt = nowIso;
      } else {
        hostId = `host_${createEventId().slice(4)}`;
        state.hosts.push({
          hostId,
          systemId,
          relayUrl,
          displayName: String(parsed.hostName || systemId).trim() || systemId,
          note: "",
          pairedAt: nowIso,
          updatedAt: nowIso,
          autoConnect: true,
        });
      }

      recomputeSelections();
      state.selectedHostId = hostId;
      state.debugHostId = hostId;

      const runtime = ensureRuntime(hostId);
      runtime.accessToken = String(exchangeData.accessToken || "");
      runtime.refreshToken = String(exchangeData.refreshToken || "");
      runtime.keyId = String(exchangeData.keyId || keyId);
      runtime.credentialId = String(exchangeData.credentialId || "");
      runtime.devicePublicKey = devicePubKey;
      runtime.manualReconnectRequired = false;
      runtime.retryCount = 0;
      runtime.lastError = "";

      await storeHostSession(hostId);
      // 若该宿主机曾在删除补偿队列中，重新配对成功后应立即移除旧补偿任务。
      state.pendingHostDeletes = state.pendingHostDeletes.filter(
        (item) => !(item.systemId === systemId && item.relayUrl === relayUrl),
      );
      persistConfig();

      closePairFlow();
      closeHostManageModal();
      await connectHost(hostId, { manual: true, resetRetry: true });
      notifyIfDuplicateDisplayName(hostId);
    } catch (error) {
      const code = String(error && error.code ? error.code : "").trim();
      showPairFailure(
        mapPairFailure(
          code || "RELAY_UNREACHABLE",
          `配对请求失败：${error}`,
          "请检查网络与 Relay 地址。",
          source,
        ),
      );
    } finally {
      state.pairingBusy = false;
    }
  }

  function bindPairingLinkBridge() {
    window.__YC_HANDLE_PAIR_LINK__ = (rawUrl) => {
      void runPairingFromLink(rawUrl, "deep-link");
    };

    const pending = Array.isArray(window.__YC_PENDING_PAIR_LINKS__)
      ? [...window.__YC_PENDING_PAIR_LINKS__]
      : [];
    window.__YC_PENDING_PAIR_LINKS__ = [];
    pending.forEach((rawUrl) => {
      if (!rawUrl) return;
      void runPairingFromLink(rawUrl, "deep-link");
    });
  }

  function tryApplyLaunchPairingLink() {
    try {
      const launchUrl = new URL(window.location.href);
      if (launchUrl.protocol === "yc:" && launchUrl.hostname === "pair") {
        void runPairingFromLink(launchUrl.toString(), "launch-url");
        return true;
      }
      const relay = String(launchUrl.searchParams.get("relay") || "").trim();
      const code = String(launchUrl.searchParams.get("code") || "").trim();
      const sid = String(launchUrl.searchParams.get("sid") || "").trim();
      const ticket = String(launchUrl.searchParams.get("ticket") || "").trim();
      const name = String(launchUrl.searchParams.get("name") || "").trim();
      if (!relay || (!code && !(sid && ticket))) return false;

      let syntheticLink = `yc://pair?relay=${encodeURIComponent(relay)}`;
      syntheticLink += sid && ticket
        ? `&sid=${encodeURIComponent(sid)}&ticket=${encodeURIComponent(ticket)}`
        : `&code=${encodeURIComponent(code)}`;
      if (name) syntheticLink += `&name=${encodeURIComponent(name)}`;
      void runPairingFromLink(syntheticLink, "launch-url");
      return true;
    } catch (_) {
      // ignore malformed launch url
      return false;
    }
  }

  return {
    runPairingFromLink,
    runPairingFromManual,
    bindPairingLinkBridge,
    tryApplyLaunchPairingLink,
  };
}
