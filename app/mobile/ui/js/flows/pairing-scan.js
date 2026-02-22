// 文件职责：
// 1. 管理扫码配对（相机实时识别 + 图库识别）。
// 2. 向配对主流程回调已识别到的链接。

/** 创建扫码能力（实时扫描 + 图库导入）。 */
export function createPairingScanner({ state, ui, runPairingFromLink, showPairFailure }) {
  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  function setPairScanStatus(text = "", level = "normal") {
    ui.pairScanStatus.textContent = String(text || "");
    ui.pairScanStatus.style.color = level === "warn" ? "var(--warn)" : "var(--text-sub)";
  }

  function stopPairScan() {
    state.scanning = false;
    if (state.scanStream) {
      const tracks = state.scanStream.getTracks();
      for (const track of tracks) {
        track.stop();
      }
      state.scanStream = null;
    }
    if (ui.pairScanVideo) {
      ui.pairScanVideo.srcObject = null;
    }
  }

  async function scanLoop() {
    while (state.scanning) {
      try {
        if (!state.scanDetector || !ui.pairScanVideo || ui.pairScanVideo.readyState < 2) {
          await sleep(120);
          continue;
        }
        const found = await state.scanDetector.detect(ui.pairScanVideo);
        if (Array.isArray(found) && found.length > 0) {
          const raw = String(found[0].rawValue || "").trim();
          if (raw) {
            await runPairingFromLink(raw, "scan");
            stopPairScan();
            break;
          }
        }
      } catch (_) {
        // 扫码帧允许偶发失败，不中断扫描。
      }
      await sleep(120);
    }
  }

  async function startPairScan() {
    if (state.scanning) {
      return;
    }
    if (typeof window.BarcodeDetector !== "function") {
      setPairScanStatus(
        "当前环境不支持实时扫码，可改用“从图库导入二维码”或“粘贴配对链接”。",
        "warn",
      );
      return;
    }

    try {
      state.scanning = true;
      setPairScanStatus("请将二维码放入取景框，识别后会自动配对。");
      state.scanDetector = state.scanDetector || new window.BarcodeDetector({
        formats: ["qr_code"],
      });
      state.scanStream = await navigator.mediaDevices.getUserMedia({
        video: { facingMode: "environment" },
        audio: false,
      });
      ui.pairScanVideo.srcObject = state.scanStream;
      await ui.pairScanVideo.play();
      void scanLoop();
    } catch (_) {
      stopPairScan();
      setPairScanStatus("无法打开相机，请检查权限后重试。", "warn");
    }
  }

  async function onPairScanFileSelected(event) {
    const file = event.target && event.target.files && event.target.files[0];
    event.target.value = "";
    if (!file) {
      return;
    }
    if (typeof window.BarcodeDetector !== "function") {
      const mapped = {
        reason: "当前环境不支持二维码识别",
        suggestion: "请改用粘贴链接方式。",
        primaryLabel: "重新粘贴",
        primaryAction: "paste",
      };
      showPairFailure(mapped);
      return;
    }
    try {
      const bitmap = await createImageBitmap(file);
      const detector = state.scanDetector || new window.BarcodeDetector({
        formats: ["qr_code"],
      });
      const detected = await detector.detect(bitmap);
      const first = Array.isArray(detected) && detected.length > 0 ? detected[0] : null;
      const rawValue = first && typeof first.rawValue === "string" ? first.rawValue : "";
      if (!rawValue) {
        showPairFailure({
          reason: "未识别到有效二维码",
          suggestion: "请更换清晰图片后重试。",
          primaryLabel: "重新扫码",
          primaryAction: "scan",
        });
        return;
      }
      await runPairingFromLink(rawValue, "gallery");
    } catch (error) {
      showPairFailure({
        reason: `图片识别失败：${error}`,
        suggestion: "请改用扫码或粘贴链接。",
        primaryLabel: "重新粘贴",
        primaryAction: "paste",
      });
    }
  }

  return {
    setPairScanStatus,
    startPairScan,
    stopPairScan,
    onPairScanFileSelected,
  };
}
