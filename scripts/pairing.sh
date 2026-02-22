#!/usr/bin/env bash

# 文件职责：
# 1. 通过 Relay 的 /v1/pair/bootstrap 统一签发配对信息（链接/票据/模拟扫码命令）。
# 2. 输出配对码、配对链接、二维码与 JSON，作为终端配对与调试兜底工具。
# 3. 支持向 iOS 模拟器投递 yc://pair 深链，验证扫码链路。

set -euo pipefail

PAIR_DIR="${HOME}/.config/yourconnector/sidecar"
RELAY_WS_URL="ws://127.0.0.1:18080/v1/ws"
HOST_NAME=""
PAIR_CODE_OVERRIDE=""
QR_PNG_PATH=""
SHOW_MODE="all"
SIMULATE_IOS_SCAN=0
INCLUDE_CODE=0
TICKET_TTL_SEC=300

API_BASE_URL=""
SYSTEM_ID=""
PAIR_TOKEN=""
PAIR_CODE=""
PAIR_TICKET=""
PAIR_LINK=""
SIMCTL_CMD=""
SIGNED_RELAY_WS_URL=""
SIGNED_HOST_NAME=""
NEED_BOOTSTRAP=1

usage() {
  cat <<'EOF_HELP'
用法：
  scripts/pairing.sh [参数]

参数：
  --show <all|code|link|qr|json>   输出模式（默认 all）
  --relay <ws-url>                 指定 relay WS 地址
  --name <host-name>               指定宿主机名称（默认自动探测）
  --pair-dir <dir>                 指定 sidecar 身份目录（默认 ~/.config/yourconnector/sidecar）
  --code <systemId.pairToken>      直接使用给定配对码（不读取本地文件）
  --ttl-sec <seconds>              短时票据有效期（30-3600，默认 300）
  --include-code                   在链接中附带 code（默认关闭）
  --no-code                        不在链接中附带 code（仅 sid+ticket，默认）
  --qr-png <path>                  导出二维码 PNG 到指定路径（依赖 qrencode）
  --simulate-ios-scan              调用 simctl 向当前 booted 模拟器投递 yc://pair 深链
  -h, --help                       显示帮助

示例：
  scripts/pairing.sh --show all
  scripts/pairing.sh --show link --relay ws://10.0.0.2:18080/v1/ws
  scripts/pairing.sh --name "我的 Mac" --qr-png /tmp/yc-pair.png
  scripts/pairing.sh --show json --ttl-sec 180 --no-code
  scripts/pairing.sh --simulate-ios-scan
EOF_HELP
}

require_cmd() {
  local cmd="$1"
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "缺少依赖命令：${cmd}" >&2
    exit 1
  fi
}

normalize_host_name() {
  local raw="$1"
  local compact
  compact="$(printf '%s' "${raw}" | awk '{$1=$1;print}')"
  printf '%s' "${compact}" | cut -c1-64
}

detect_host_name() {
  if [[ -n "${HOST_NAME}" ]]; then
    normalize_host_name "${HOST_NAME}"
    return
  fi

  # 与 sidecar 保持一致优先级：优先 ComputerName，再读系统级名称。
  if [[ -n "${COMPUTERNAME:-}" ]]; then
    normalize_host_name "${COMPUTERNAME}"
    return
  fi

  if command -v scutil >/dev/null 2>&1; then
    for sc_key in ComputerName LocalHostName HostName; do
      local value=""
      value="$(scutil --get "${sc_key}" 2>/dev/null || true)"
      if [[ -n "${value}" ]]; then
        normalize_host_name "${value}"
        return
      fi
    done
  fi

  if command -v hostname >/dev/null 2>&1; then
    local value=""
    value="$(hostname 2>/dev/null || true)"
    if [[ -n "${value}" ]]; then
      normalize_host_name "${value}"
      return
    fi
  fi

  echo "My Mac"
}

pair_code_from_files() {
  local sid_file="${PAIR_DIR}/system-id.txt"
  local ptk_file="${PAIR_DIR}/pair-token.txt"

  if [[ ! -f "${sid_file}" || ! -f "${ptk_file}" ]]; then
    echo "pairing code not ready: 请先启动 sidecar（make run-sidecar）" >&2
    exit 1
  fi

  local sid ptk
  sid="$(tr -d '[:space:]' <"${sid_file}")"
  ptk="$(tr -d '[:space:]' <"${ptk_file}")"
  if [[ -z "${sid}" || -z "${ptk}" ]]; then
    echo "pairing code invalid: ${sid_file} 或 pair-token.txt 为空" >&2
    exit 1
  fi

  echo "${sid}.${ptk}"
}

validate_pair_code() {
  local value="$1"
  if [[ "${value}" != *.* ]]; then
    return 1
  fi
  local sid="${value%%.*}"
  local ptk="${value#*.}"
  [[ -n "${sid}" && -n "${ptk}" ]]
}

relay_api_base() {
  require_cmd python3
  python3 - "$1" <<'PY'
import sys
from urllib.parse import urlparse

raw = (sys.argv[1] or '').strip()
if not raw:
    raise SystemExit('relay ws url empty')
u = urlparse(raw)
if u.scheme not in ('ws', 'wss', 'http', 'https'):
    raise SystemExit(f'unsupported relay url scheme: {u.scheme}')
if not u.netloc:
    raise SystemExit('relay ws url missing host')
protocol = 'https' if u.scheme in ('wss', 'https') else 'http'
path = (u.path or '/').rstrip('/')
if path.endswith('/ws'):
    path = path[:-3]
path = path.rstrip('/')
if not path:
    path = '/v1'
elif not path.endswith('/v1'):
    path = f'{path}/v1'
print(f'{protocol}://{u.netloc}{path}')
PY
}

request_pair_bootstrap() {
  require_cmd curl
  require_cmd jq

  local include_code_json="false"
  if [[ "${INCLUDE_CODE}" -eq 1 ]]; then
    include_code_json="true"
  fi

  local request_body
  request_body="$(jq -nc \
    --arg systemId "${SYSTEM_ID}" \
    --arg pairToken "${PAIR_TOKEN}" \
    --arg hostName "${HOST_NAME}" \
    --arg relayWsUrl "${RELAY_WS_URL}" \
    --argjson includeCode "${include_code_json}" \
    --argjson ttlSec "${TICKET_TTL_SEC}" \
    '{systemId: $systemId, pairToken: $pairToken, hostName: $hostName, relayWsUrl: $relayWsUrl, includeCode: $includeCode, ttlSec: $ttlSec}')"

  local response
  if ! response="$(curl -sS --fail-with-body \
    -H 'content-type: application/json' \
    -X POST \
    -d "${request_body}" \
    "${API_BASE_URL}/pair/bootstrap")"; then
    echo "请求 Relay 配对签发失败：${API_BASE_URL}/pair/bootstrap" >&2
    exit 1
  fi

  local ok
  ok="$(printf '%s' "${response}" | jq -r '.ok // false')"
  if [[ "${ok}" != "true" ]]; then
    local code message suggestion
    code="$(printf '%s' "${response}" | jq -r '.code // "UNKNOWN"')"
    message="$(printf '%s' "${response}" | jq -r '.message // "未知错误"')"
    suggestion="$(printf '%s' "${response}" | jq -r '.suggestion // "请检查 relay 与 sidecar 状态"')"
    echo "Relay 签发失败：${code} - ${message}（${suggestion}）" >&2
    exit 1
  fi

  PAIR_LINK="$(printf '%s' "${response}" | jq -r '.data.pairLink // empty')"
  PAIR_TICKET="$(printf '%s' "${response}" | jq -r '.data.pairTicket // empty')"
  SYSTEM_ID="$(printf '%s' "${response}" | jq -r '.data.systemId // empty')"
  SIGNED_RELAY_WS_URL="$(printf '%s' "${response}" | jq -r '.data.relayWsUrl // empty')"
  SIGNED_HOST_NAME="$(printf '%s' "${response}" | jq -r '.data.hostName // empty')"
  SIMCTL_CMD="$(printf '%s' "${response}" | jq -r '.data.simctlCommand // empty')"

  local maybe_code
  maybe_code="$(printf '%s' "${response}" | jq -r '.data.pairCode // empty')"
  if [[ -n "${maybe_code}" ]]; then
    PAIR_CODE="${maybe_code}"
  fi

  if [[ -z "${PAIR_LINK}" || -z "${PAIR_TICKET}" || -z "${SYSTEM_ID}" ]]; then
    echo "Relay 返回数据不完整：缺少 pairLink/pairTicket/systemId" >&2
    exit 1
  fi
}

print_qr_terminal() {
  local link="$1"
  if ! command -v qrencode >/dev/null 2>&1; then
    echo "二维码输出依赖 qrencode（可选安装：brew install qrencode）" >&2
    return 1
  fi
  qrencode -t ANSIUTF8 "${link}"
}

export_qr_png() {
  local link="$1"
  local output_path="$2"
  require_cmd qrencode
  qrencode -o "${output_path}" "${link}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --show)
      SHOW_MODE="${2:-}"
      shift 2
      ;;
    --relay)
      RELAY_WS_URL="${2:-}"
      shift 2
      ;;
    --name)
      HOST_NAME="${2:-}"
      shift 2
      ;;
    --pair-dir)
      PAIR_DIR="${2:-}"
      shift 2
      ;;
    --code)
      PAIR_CODE_OVERRIDE="${2:-}"
      shift 2
      ;;
    --ttl-sec)
      TICKET_TTL_SEC="${2:-}"
      shift 2
      ;;
    --include-code)
      INCLUDE_CODE=1
      shift
      ;;
    --no-code)
      INCLUDE_CODE=0
      shift
      ;;
    --qr-png)
      QR_PNG_PATH="${2:-}"
      shift 2
      ;;
    --simulate-ios-scan)
      SIMULATE_IOS_SCAN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "未知参数: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "${RELAY_WS_URL}" ]]; then
  echo "relay 地址不能为空（--relay）" >&2
  exit 1
fi
if ! [[ "${TICKET_TTL_SEC}" =~ ^[0-9]+$ ]] || [[ "${TICKET_TTL_SEC}" -lt 30 || "${TICKET_TTL_SEC}" -gt 3600 ]]; then
  echo "ttl 非法：请使用 30-3600 秒（--ttl-sec）" >&2
  exit 1
fi

PAIR_CODE="${PAIR_CODE_OVERRIDE}"
if [[ -z "${PAIR_CODE}" ]]; then
  PAIR_CODE="$(pair_code_from_files)"
fi
if ! validate_pair_code "${PAIR_CODE}"; then
  echo "配对码格式无效：${PAIR_CODE}" >&2
  exit 1
fi

SYSTEM_ID="${PAIR_CODE%%.*}"
PAIR_TOKEN="${PAIR_CODE#*.}"
HOST_NAME="$(detect_host_name)"
if [[ "${SHOW_MODE}" == "code" && "${SIMULATE_IOS_SCAN}" -eq 0 && -z "${QR_PNG_PATH}" ]]; then
  NEED_BOOTSTRAP=0
fi

if [[ "${NEED_BOOTSTRAP}" -eq 1 ]]; then
  API_BASE_URL="$(relay_api_base "${RELAY_WS_URL}")"
  request_pair_bootstrap
fi

if [[ -n "${QR_PNG_PATH}" ]]; then
  if [[ "${NEED_BOOTSTRAP}" -ne 1 ]]; then
    API_BASE_URL="$(relay_api_base "${RELAY_WS_URL}")"
    request_pair_bootstrap
    NEED_BOOTSTRAP=1
  fi
  export_qr_png "${PAIR_LINK}" "${QR_PNG_PATH}"
fi

case "${SHOW_MODE}" in
  all)
    printf '%s\n' "宿主机名称: ${SIGNED_HOST_NAME:-${HOST_NAME}}"
    printf '%s\n' "systemId: ${SYSTEM_ID}"
    printf '%s\n' "配对码: ${PAIR_CODE}"
    printf '%s\n' "短时票据: ${PAIR_TICKET}"
    printf '%s\n' "Relay WS: ${SIGNED_RELAY_WS_URL:-${RELAY_WS_URL}}"
    printf '%s\n' "配对链接: ${PAIR_LINK}"
    printf '%s\n' "模拟扫码: ${SIMCTL_CMD:-xcrun simctl openurl booted \"${PAIR_LINK}\"}"
    print_qr_terminal "${PAIR_LINK}" || true
    ;;
  code)
    printf '%s\n' "${PAIR_CODE}"
    ;;
  link)
    if [[ "${NEED_BOOTSTRAP}" -ne 1 ]]; then
      API_BASE_URL="$(relay_api_base "${RELAY_WS_URL}")"
      request_pair_bootstrap
      NEED_BOOTSTRAP=1
    fi
    printf '%s\n' "${PAIR_LINK}"
    ;;
  qr)
    if [[ "${NEED_BOOTSTRAP}" -ne 1 ]]; then
      API_BASE_URL="$(relay_api_base "${RELAY_WS_URL}")"
      request_pair_bootstrap
      NEED_BOOTSTRAP=1
    fi
    print_qr_terminal "${PAIR_LINK}"
    ;;
  json)
    if [[ "${NEED_BOOTSTRAP}" -ne 1 ]]; then
      API_BASE_URL="$(relay_api_base "${RELAY_WS_URL}")"
      request_pair_bootstrap
      NEED_BOOTSTRAP=1
    fi
    require_cmd jq
    jq -n \
      --arg hostName "${SIGNED_HOST_NAME:-${HOST_NAME}}" \
      --arg systemId "${SYSTEM_ID}" \
      --arg relay "${SIGNED_RELAY_WS_URL:-${RELAY_WS_URL}}" \
      --arg code "${PAIR_CODE}" \
      --arg ticket "${PAIR_TICKET}" \
      --arg link "${PAIR_LINK}" \
      --arg simctlCommand "${SIMCTL_CMD:-xcrun simctl openurl booted \"${PAIR_LINK}\"}" \
      '{hostName: $hostName, systemId: $systemId, relay: $relay, code: $code, ticket: $ticket, link: $link, simctlCommand: $simctlCommand}'
    ;;
  *)
    echo "不支持的 --show 模式: ${SHOW_MODE}" >&2
    exit 1
    ;;
esac

if [[ "${SIMULATE_IOS_SCAN}" -eq 1 ]]; then
  if [[ "${NEED_BOOTSTRAP}" -ne 1 ]]; then
    API_BASE_URL="$(relay_api_base "${RELAY_WS_URL}")"
    request_pair_bootstrap
    NEED_BOOTSTRAP=1
  fi
  echo "simulate scan: ${PAIR_LINK}"
  xcrun simctl openurl booted "${PAIR_LINK}"
fi
