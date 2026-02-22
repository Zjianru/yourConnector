// 文件职责：
// 1. 管理宿主机通知弹窗（普通提示 / 重名引导）。
// 2. 统一弹窗按钮文案与动作状态，避免页面分散维护。

import { asMap } from "../utils/type.js";

/**
 * 创建宿主机提示弹窗能力。
 * @param {{state: object, ui: object}} deps 依赖集合。
 */
export function createHostNoticeModal({ state, ui }) {
  function openHostNoticeModal(title, body, options = {}) {
    const normalized =
      typeof options === "string"
        ? {
            targetHostId: options,
            primaryAction: options ? "edit" : "dismiss",
            primaryLabel: options ? "去修改名称" : "知道了",
            secondaryLabel: options ? "稍后处理" : "",
          }
        : asMap(options);

    const targetHostId = String(normalized.targetHostId || "").trim();
    const primaryAction = String(
      normalized.primaryAction || (targetHostId ? "edit" : "dismiss"),
    ).trim();
    const primaryLabel = String(
      normalized.primaryLabel || (primaryAction === "edit" ? "去修改名称" : "知道了"),
    )
      .trim();
    const secondaryLabel = String(
      normalized.secondaryLabel === undefined
        ? primaryAction === "edit"
          ? "稍后处理"
          : ""
        : normalized.secondaryLabel,
    ).trim();

    ui.hostNoticeTitle.textContent = title;
    ui.hostNoticeBody.textContent = body;
    state.hostNoticeTargetId = targetHostId;
    state.hostNoticePrimaryAction = primaryAction || "dismiss";
    ui.hostNoticePrimaryBtn.textContent = primaryLabel || "知道了";

    if (secondaryLabel) {
      ui.hostNoticeSecondaryBtn.textContent = secondaryLabel;
      ui.hostNoticeSecondaryBtn.style.display = "";
    } else {
      ui.hostNoticeSecondaryBtn.style.display = "none";
    }

    ui.hostNoticeModal.classList.add("show");
  }

  function closeHostNoticeModal() {
    ui.hostNoticeModal.classList.remove("show");
    state.hostNoticeTargetId = "";
    state.hostNoticePrimaryAction = "dismiss";
    ui.hostNoticePrimaryBtn.textContent = "知道了";
    ui.hostNoticeSecondaryBtn.textContent = "稍后处理";
    ui.hostNoticeSecondaryBtn.style.display = "";
  }

  function bindHostNoticeModalEvents({ onEditHost }) {
    ui.hostNoticeClose.addEventListener("click", closeHostNoticeModal);
    ui.hostNoticeSecondaryBtn.addEventListener("click", closeHostNoticeModal);
    ui.hostNoticePrimaryBtn.addEventListener("click", () => {
      const action = state.hostNoticePrimaryAction;
      const hostId = state.hostNoticeTargetId;
      closeHostNoticeModal();
      if (action === "edit" && hostId && typeof onEditHost === "function") {
        onEditHost(hostId);
      }
    });
    ui.hostNoticeModal.addEventListener("click", (event) => {
      if (event.target === ui.hostNoticeModal) {
        closeHostNoticeModal();
      }
    });
  }

  return {
    openHostNoticeModal,
    closeHostNoticeModal,
    bindHostNoticeModalEvents,
  };
}
