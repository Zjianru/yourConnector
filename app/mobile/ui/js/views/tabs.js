// 文件职责：
// 1. 渲染运维/聊天两页签状态。
// 2. 将标签页 UI 切换逻辑从主流程中抽离。

/**
 * 渲染底部标签页状态。
 * @param {object} state 全局状态。
 * @param {object} ui 页面节点集合。
 */
export function renderTabs(state, ui) {
  const onOps = state.activeTab === "ops";
  const onChat = state.activeTab === "chat";
  if (ui.topBar) {
    ui.topBar.classList.toggle("hidden", !onOps);
  }
  ui.opsView.classList.toggle("active", onOps);
  ui.chatView.classList.toggle("active", onChat);
  ui.tabOps.classList.toggle("active", onOps);
  ui.tabChat.classList.toggle("active", onChat);
}
