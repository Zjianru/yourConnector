// 文件职责：
// 1. 渲染调试页连接状态、宿主机选择与日志。
// 2. 将调试视图逻辑从主流程中抽离，减少主文件体积。

/** 渲染调试面板内容。 */
export function renderDebugPanel(state, ui, visibleHosts, hostById, ensureRuntime, maskSecret, escapeHtml) {
  const hosts = visibleHosts();
  const hostId = state.debugHostId;
  const host = hostById(hostId);
  const runtime = ensureRuntime(hostId);

  ui.debugHostSelect.innerHTML = hosts
    .map(
      (item) =>
        `<option value="${escapeHtml(item.hostId)}" ${item.hostId === hostId ? "selected" : ""}>` +
        `${escapeHtml(item.displayName)}</option>`,
    )
    .join("");

  const status = runtime && runtime.connected
    ? "Connected"
    : runtime && runtime.connecting
      ? "Connecting"
      : "Disconnected";
  ui.debugStatus.textContent = `Status: ${status}`;
  ui.debugEvents.textContent = `Events IN: ${state.eventIn} · OUT: ${state.eventOut}`;
  ui.debugIdentity.textContent = `Host: ${host ? host.displayName : "--"} · System: ${host ? host.systemId : "--"} · `
    + `AccessToken: ${maskSecret(runtime ? runtime.accessToken : "")} · Device: ${state.deviceId || "--"}`;

  ui.connectBtnDebug.disabled = !host || (runtime && (runtime.connected || runtime.connecting));
  ui.disconnectBtnDebug.disabled = !host || !(runtime && runtime.connected);
  ui.rebindControllerBtn.disabled = !host || !(runtime && runtime.connected);

  ui.logBox.innerHTML = state.logs.map((line) => `<div class="log-item">${escapeHtml(line)}</div>`).join("");
  if (document.activeElement !== ui.messageInput) {
    ui.messageInput.value = state.message;
  }
}
