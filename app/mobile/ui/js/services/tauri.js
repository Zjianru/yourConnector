// 文件职责：
// 1. 封装 Tauri invoke 的兼容调用入口。
// 2. 屏蔽 v1/v2 API 差异，避免业务层分散处理。

/**
 * 调用 Tauri 命令（兼容 v2 与历史 API）。
 * @param {string} command 命令名。
 * @param {object} payload 入参对象。
 * @returns {Promise<any>}
 */
export async function tauriInvoke(command, payload = {}) {
  const invokeV2 = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
  if (typeof invokeV2 === "function") {
    return invokeV2(command, payload);
  }
  const invokeLegacy = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
  if (typeof invokeLegacy === "function") {
    return invokeLegacy(command, payload);
  }
  throw new Error("Tauri invoke 不可用");
}
