#!/usr/bin/env bash
# 文件职责：
# 1. 执行移动端“自调试-自评审-自回归”闭环脚本，尽量减少人工点按依赖。
# 2. 串联门禁检查、服务存活检查、iOS 启动、配对链路与日志异常扫描。
# 3. 产出截图与日志工件，便于定位白屏、按钮失效、未捕获异常等问题。

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IOS_SIM="${IOS_SIM:-iPhone 17 Pro}"
RUN_CHECKS="${RUN_CHECKS:-1}"
AUTO_SCAN="${AUTO_SCAN:-1}"
LAUNCH_APP="${LAUNCH_APP:-1}"

STAMP="$(date +%Y%m%d-%H%M%S)"
ARTIFACT_DIR="${ROOT_DIR}/.tmp/self-debug/${STAMP}"
mkdir -p "${ARTIFACT_DIR}"

info() { printf '[self-debug] %s\n' "$*"; }
warn() { printf '[self-debug][warn] %s\n' "$*" >&2; }

is_relay_running() {
  pgrep -f 'target/.*/yc-relay|cargo run -p yc-relay' >/dev/null 2>&1
}

is_sidecar_running() {
  pgrep -f 'target/.*/yc-sidecar|cargo run -p yc-sidecar' >/dev/null 2>&1
}

maybe_start_services() {
  if is_relay_running; then
    info "relay 已运行"
  else
    info "relay 未运行，后台拉起"
    (
      cd "${ROOT_DIR}"
      nohup cargo run -p yc-relay >"${ARTIFACT_DIR}/relay.log" 2>&1 &
      echo $! >"${ARTIFACT_DIR}/relay.pid"
    )
    sleep 2
  fi

  if is_sidecar_running; then
    info "sidecar 已运行"
  else
    info "sidecar 未运行，后台拉起"
    (
      cd "${ROOT_DIR}"
      nohup cargo run -p yc-sidecar >"${ARTIFACT_DIR}/sidecar.log" 2>&1 &
      echo $! >"${ARTIFACT_DIR}/sidecar.pid"
    )
    sleep 2
  fi
}

collect_ios_logs() {
  local out_file="$1"
  # 聚焦移动端相关日志，并捕捉前端兜底日志与常见 JS 异常关键字。
  xcrun simctl spawn booted log show \
    --style compact \
    --last 6m \
    --predicate 'subsystem == "dev.yourconnector.mobile" OR process CONTAINS[c] "yourConnector" OR eventMessage CONTAINS[c] "[ui_error]" OR eventMessage CONTAINS[c] "TypeError" OR eventMessage CONTAINS[c] "ReferenceError" OR eventMessage CONTAINS[c] "Unhandled"' \
    >"${out_file}" 2>/dev/null || true
}

scan_issues_from_logs() {
  local log_file="$1"
  rg -n \
    -e '\[ui_error\]' \
    -e 'TypeError' \
    -e 'ReferenceError' \
    -e 'Unhandled Promise' \
    -e 'unhandledrejection' \
    "${log_file}" \
    | rg -v "log run noninteractively|args: 'log' 'show'" || true
}

main() {
  info "工件目录: ${ARTIFACT_DIR}"

  if [[ "${RUN_CHECKS}" == "1" ]]; then
    info "执行门禁检查 (make check-all)"
    (cd "${ROOT_DIR}" && make check-all) | tee "${ARTIFACT_DIR}/check-all.log"
  else
    info "跳过门禁检查 (RUN_CHECKS=${RUN_CHECKS})"
  fi

  maybe_start_services

  info "启动 iOS 模拟器: ${IOS_SIM}"
  (cd "${ROOT_DIR}" && make boot-ios-sim IOS_SIM="${IOS_SIM}") | tee "${ARTIFACT_DIR}/boot-ios.log"
  xcrun simctl bootstatus "${IOS_SIM}" -b >/dev/null 2>&1 || true

  if [[ "${LAUNCH_APP}" == "1" ]]; then
    info "启动 mobile app"
    xcrun simctl launch booted dev.yourconnector.mobile >"${ARTIFACT_DIR}/launch-mobile.log" 2>&1 || true
    sleep 2
  fi

  if [[ "${AUTO_SCAN}" == "1" ]]; then
    info "执行模拟扫码链路 (make simulate-ios-scan)"
    (
      cd "${ROOT_DIR}"
      make simulate-ios-scan
    ) >"${ARTIFACT_DIR}/simulate-ios-scan.log" 2>&1 || warn "simulate-ios-scan 失败，详见工件日志"
    sleep 3
  else
    info "跳过模拟扫码 (AUTO_SCAN=${AUTO_SCAN})"
  fi

  info "抓取模拟器截图"
  xcrun simctl io booted screenshot "${ARTIFACT_DIR}/ios-screen.png" >/dev/null 2>&1 || warn "截图失败"

  info "采集并扫描 iOS 日志"
  collect_ios_logs "${ARTIFACT_DIR}/ios.log"
  scan_issues_from_logs "${ARTIFACT_DIR}/ios.log" >"${ARTIFACT_DIR}/issues.log"

  if [[ -s "${ARTIFACT_DIR}/issues.log" ]]; then
    warn "发现潜在异常，请优先查看 ${ARTIFACT_DIR}/issues.log"
    cat "${ARTIFACT_DIR}/issues.log"
    exit 2
  fi

  info "未发现前端异常关键字"
  info "完成。建议检查：${ARTIFACT_DIR}/ios-screen.png 与 ${ARTIFACT_DIR}/ios.log"
}

main "$@"
