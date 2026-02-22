// 文件职责：
// 1. 将后端/本地错误码映射为面向用户的配对失败提示。
// 2. 统一主按钮动作建议（扫码/粘贴/手动）。

/**
 * 将配对错误码映射为用户可理解的失败弹窗内容。
 * @param {string} code 错误码。
 * @param {string} message 原始消息。
 * @param {string} suggestion 原始建议。
 * @param {"scan"|"paste"|"manual"|string} primaryAction 建议主操作。
 * @returns {{reason: string, suggestion: string, primaryLabel: string, primaryAction: string}}
 */
export function mapPairFailure(code, message, suggestion, primaryAction) {
  const normalizedCode = String(code || "").trim();
  const fallbackMessage = String(message || "").trim();

  if (normalizedCode === "INVALID_LINK") {
    return {
      reason: "配对链接无效",
      suggestion: "请重新扫码或检查粘贴内容是否完整。",
      primaryLabel: "重新粘贴",
      primaryAction: "paste",
    };
  }
  if (normalizedCode === "PAIR_TICKET_EXPIRED" || normalizedCode === "PAIR_TICKET_REPLAYED") {
    return {
      reason: normalizedCode === "PAIR_TICKET_EXPIRED" ? "配对信息已过期" : "配对二维码已使用",
      suggestion: "请重新扫码获取最新二维码。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  if (normalizedCode === "PAIR_TICKET_INVALID") {
    return {
      reason: "配对信息无效",
      suggestion: "请重新扫码获取最新二维码，或改用手动配对。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  if (normalizedCode === "PAIR_TOKEN_NOT_SUPPORTED") {
    return {
      reason: "配对信息已过时",
      suggestion: "当前版本仅支持 sid + ticket 配对，请重新扫码获取最新链接。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  if (normalizedCode === "SYSTEM_NOT_REGISTERED") {
    return {
      reason: "宿主机未在线",
      suggestion: "请先在宿主机启动 sidecar，再进行配对。",
      primaryLabel: "去手动输入",
      primaryAction: "manual",
    };
  }
  if (normalizedCode === "RELAY_URL_INVALID") {
    return {
      reason: "Relay 地址格式无效",
      suggestion: "请使用 ws:// 或 wss:// 开头的 Relay 地址。",
      primaryLabel: "去手动输入",
      primaryAction: "manual",
    };
  }
  if (normalizedCode === "RELAY_UNREACHABLE") {
    const action = primaryAction === "manual" ? "manual" : primaryAction === "scan" ? "scan" : "paste";
    return {
      reason: "无法连接 Relay",
      suggestion:
        suggestion ||
        "请检查 Relay 地址、宿主机网络，并确认 relay 已启动（make run-relay）。本机调试可尝试 127.0.0.1 与 localhost 两种地址。",
      primaryLabel: action === "manual" ? "去手动输入" : action === "scan" ? "重新扫码" : "重新粘贴",
      primaryAction: action,
    };
  }
  if (normalizedCode === "QR_SCANNER_UNAVAILABLE") {
    return {
      reason: "当前设备不支持实时扫码",
      suggestion: "请改用“从图库导入二维码”或“粘贴配对链接”。",
      primaryLabel: "重新粘贴",
      primaryAction: "paste",
    };
  }
  if (normalizedCode === "CAMERA_UNAVAILABLE") {
    return {
      reason: "无法打开相机",
      suggestion: "请检查相机权限，或改用“从图库导入二维码/粘贴配对链接”。",
      primaryLabel: "重新粘贴",
      primaryAction: "paste",
    };
  }
  if (normalizedCode === "PAIR_TOKEN_MISMATCH") {
    return {
      reason: "配对信息无效",
      suggestion: "请重新生成配对信息后再试。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }
  if (normalizedCode === "ACCESS_TOKEN_EXPIRED" || normalizedCode === "ACCESS_TOKEN_INVALID") {
    return {
      reason: "设备凭证失效",
      suggestion: "请重新扫码配对，更新设备凭证。",
      primaryLabel: "重新扫码",
      primaryAction: "scan",
    };
  }

  return {
    reason: fallbackMessage || "配对失败",
    suggestion: suggestion || "请重试；若仍失败可切换到手动填写配对信息。",
    primaryLabel: primaryAction === "manual" ? "去手动输入" : primaryAction === "scan" ? "重新扫码" : "重试",
    primaryAction: primaryAction || "paste",
  };
}
