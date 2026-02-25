// 文件职责：
// 1. 统一识别当前移动端平台（iOS / Android）。
// 2. 提供跨平台设备标识前缀与设备名称，避免业务层硬编码 iOS。

/** 识别当前 WebView 运行平台。 */
export function detectMobilePlatform() {
  const ua = String(navigator.userAgent || "").toLowerCase();
  if (ua.includes("android")) {
    return "android";
  }
  if (ua.includes("iphone") || ua.includes("ipad") || ua.includes("ipod")) {
    return "ios";
  }
  return "mobile";
}

/** 生成设备 ID 前缀。 */
export function deviceIdPrefix() {
  const platform = detectMobilePlatform();
  if (platform === "android") {
    return "android";
  }
  if (platform === "ios") {
    return "ios";
  }
  return "mobile";
}

/** 生成配对上报设备名称。 */
export function normalizedDeviceName() {
  const platform = detectMobilePlatform();
  if (platform === "android") {
    return "android_mobile";
  }
  if (platform === "ios") {
    return "ios_mobile";
  }
  return "mobile_tauri";
}
