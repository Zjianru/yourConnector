// 文件职责：
// 1. 管理配对失败弹窗展示状态。
// 2. 统一主按钮动作回调，避免流程层重复绑定。

/**
 * 创建配对失败弹窗能力。
 * @param {{state: object, ui: object}} deps 依赖集合。
 */
export function createPairFailureModal({ state, ui }) {
  function closePairFailureModal() {
    ui.pairFailureModal.classList.remove("show");
  }

  function showPairFailure(mapped) {
    state.pairFailurePrimaryAction = mapped.primaryAction;
    ui.pairFailureReason.textContent = mapped.reason;
    ui.pairFailureSuggestion.textContent = mapped.suggestion;
    ui.pairFailurePrimaryBtn.textContent = mapped.primaryLabel;
    ui.pairFailureModal.classList.add("show");
  }

  function bindPairFailureModalEvents({ onPrimaryAction }) {
    ui.pairFailureClose.addEventListener("click", closePairFailureModal);
    ui.pairFailureSecondaryBtn.addEventListener("click", closePairFailureModal);
    ui.pairFailurePrimaryBtn.addEventListener("click", () => {
      const action = state.pairFailurePrimaryAction;
      closePairFailureModal();
      if (typeof onPrimaryAction === "function") {
        onPrimaryAction(action);
      }
    });
  }

  return {
    closePairFailureModal,
    showPairFailure,
    bindPairFailureModalEvents,
  };
}
