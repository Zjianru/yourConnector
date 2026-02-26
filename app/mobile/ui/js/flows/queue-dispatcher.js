// 文件职责：
// 1. 提供统一的计时调度层（timeout/interval）。
// 2. 封装“替换已有计时器”语义，减少分散 setTimeout/setInterval。

/**
 * 创建前端调度器。
 * @param {{addLog?: Function}} deps 依赖集合。
 */
export function createQueueDispatcher({ addLog } = {}) {
  function runTask(task, label) {
    try {
      const result = task();
      if (result && typeof result.then === "function") {
        void result.catch((error) => {
          if (typeof addLog === "function") {
            addLog(`[queue_dispatcher] ${label} failed: ${error}`, {
              level: "error",
              scope: "queue_dispatcher",
              action: label,
              outcome: "failed",
              detail: String(error || ""),
            });
          }
        });
      }
    } catch (error) {
      if (typeof addLog === "function") {
        addLog(`[queue_dispatcher] ${label} failed: ${error}`, {
          level: "error",
          scope: "queue_dispatcher",
          action: label,
          outcome: "failed",
          detail: String(error || ""),
        });
      }
    }
  }

  function scheduleTimeout(delayMs, task, label = "timeout_task") {
    return window.setTimeout(() => runTask(task, label), Number(delayMs || 0));
  }

  function replaceTimeout(currentHandle, delayMs, task, label = "timeout_task") {
    if (currentHandle) {
      clearTimeout(currentHandle);
    }
    return scheduleTimeout(delayMs, task, label);
  }

  function cancelTimeout(handle) {
    if (handle) {
      clearTimeout(handle);
    }
  }

  function scheduleInterval(intervalMs, task, label = "interval_task") {
    return window.setInterval(() => runTask(task, label), Number(intervalMs || 0));
  }

  function replaceInterval(currentHandle, intervalMs, task, label = "interval_task") {
    if (currentHandle) {
      clearInterval(currentHandle);
    }
    return scheduleInterval(intervalMs, task, label);
  }

  function cancelInterval(handle) {
    if (handle) {
      clearInterval(handle);
    }
  }

  return {
    scheduleTimeout,
    replaceTimeout,
    cancelTimeout,
    scheduleInterval,
    replaceInterval,
    cancelInterval,
  };
}

