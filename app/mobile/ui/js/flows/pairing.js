// 文件职责：
// 1. 管理配对流程页面步骤切换（导入/扫码/粘贴/手动）。
// 2. 组装配对执行器与扫码器，向外暴露统一配对接口。

import { DEFAULT_RELAY_WS_URL } from "../state/store.js";
import { mapPairFailure } from "./pairing-errors.js";
import { createPairingRunner } from "./pairing-run.js";
import { createPairingScanner } from "./pairing-scan.js";

/**
 * 创建配对流程编排器（步骤切换 + 事件绑定）。
 * @param {object} deps 依赖集合。
 */
export function createPairingFlow({
  state,
  ui,
  hostById,
  showPairFailure,
  closePairFailureModal,
  closeHostManageModal,
  createEventId,
  ensureRuntime,
  recomputeSelections,
  persistConfig,
  storeHostSession,
  connectHost,
  notifyIfDuplicateDisplayName,
  tauriInvoke,
}) {
  let closePairFlowRef = () => {};

  const runner = createPairingRunner({
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
    closePairFlow: () => closePairFlowRef(),
    closeHostManageModal,
    connectHost,
    notifyIfDuplicateDisplayName,
    tauriInvoke,
  });

  const scanner = createPairingScanner({
    state,
    ui,
    runPairingFromLink: runner.runPairingFromLink,
    showPairFailure,
  });

  function openPairFlow(step = "import", targetHostId) {
    state.pairFlowStep = step;
    if (typeof targetHostId === "string") {
      state.pairTargetHostId = String(targetHostId || "");
    }

    const targetHost = hostById(state.pairTargetHostId);
    ui.hostRelayInput.value = targetHost ? targetHost.relayUrl : DEFAULT_RELAY_WS_URL;
    ui.hostNameInput.value = targetHost ? targetHost.displayName : "";
    ui.hostSystemIdInput.value = targetHost ? targetHost.systemId : "";
    ui.hostPairTicketInput.value = "";

    ui.pairFlowModal.classList.add("show");
    renderPairFlow();
  }

  function closePairFlow() {
    ui.pairFlowModal.classList.remove("show");
    scanner.stopPairScan();
    state.pairTargetHostId = "";
  }

  closePairFlowRef = closePairFlow;

  function renderPairFlow() {
    const step = state.pairFlowStep;
    ui.pairFlowStepImport.style.display = step === "import" ? "block" : "none";
    ui.pairFlowStepPaste.style.display = step === "paste" ? "block" : "none";
    ui.pairFlowStepScan.style.display = step === "scan" ? "block" : "none";
    ui.pairFlowStepManual.style.display = step === "manual" ? "block" : "none";

    const isRePair = Boolean(state.pairTargetHostId);
    ui.pairFlowTitle.textContent = isRePair
      ? step === "manual" ? "重新配对（手动）" : "重新配对"
      : step === "manual" ? "手动填写配对信息" : "导入配对链接";

    if (step === "scan") {
      void scanner.startPairScan();
    } else {
      scanner.stopPairScan();
    }
  }

  function bindPairFlowEvents({ onOpenDebugTab }) {
    ui.pairFlowClose.addEventListener("click", closePairFlow);
    ui.pairFlowModal.addEventListener("click", (event) => {
      if (event.target === ui.pairFlowModal) closePairFlow();
    });

    ui.pairOpenScanBtn.addEventListener("click", () => openPairFlow("scan"));
    ui.pairOpenPasteBtn.addEventListener("click", () => openPairFlow("paste"));
    ui.pairPasteBackBtn.addEventListener("click", () => openPairFlow("import"));
    ui.pairScanBackBtn.addEventListener("click", () => openPairFlow("import"));
    ui.pairManualBackBtn.addEventListener("click", closePairFlow);
    ui.pairPasteSubmitBtn.addEventListener("click", () => {
      runner.runPairingFromLink(ui.pairLinkInput.value, "paste");
    });
    ui.pairManualSubmitBtn.addEventListener("click", runner.runPairingFromManual);
    ui.pairLinkInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        void runner.runPairingFromLink(ui.pairLinkInput.value, "paste");
      }
    });
    ui.pairScanGalleryBtn.addEventListener("click", () => ui.pairScanFileInput.click());
    ui.pairScanFileInput.addEventListener("change", scanner.onPairScanFileSelected);

    if (typeof onOpenDebugTab === "function") {
      ui.openDebugFromSetupBtn.addEventListener("click", onOpenDebugTab);
    }
  }

  function bindFailureActionHandler() {
    return (action) => {
      if (action === "scan") {
        openPairFlow("scan");
      } else if (action === "manual") {
        openPairFlow("manual");
      } else {
        openPairFlow("paste");
      }
      closePairFailureModal();
    };
  }

  function bindPairingLinkBridge() {
    window.__YC_HANDLE_PAIR_LINK__ = (rawUrl) => {
      openPairFlow("import", "");
      void runner.runPairingFromLink(rawUrl, "deep-link");
    };

    const pending = Array.isArray(window.__YC_PENDING_PAIR_LINKS__)
      ? [...window.__YC_PENDING_PAIR_LINKS__]
      : [];
    window.__YC_PENDING_PAIR_LINKS__ = [];
    pending.forEach((rawUrl) => {
      if (!rawUrl) return;
      openPairFlow("import", "");
      void runner.runPairingFromLink(rawUrl, "deep-link");
    });
  }

  function tryApplyLaunchPairingLink() {
    const applied = runner.tryApplyLaunchPairingLink();
    if (applied) {
      openPairFlow("import", "");
    }
  }

  return {
    openPairFlow,
    closePairFlow,
    renderPairFlow,
    bindPairFlowEvents,
    bindFailureActionHandler,
    bindPairingLinkBridge,
    tryApplyLaunchPairingLink,
    runPairingFromLink: runner.runPairingFromLink,
  };
}
