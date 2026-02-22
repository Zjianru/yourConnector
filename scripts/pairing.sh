#!/usr/bin/env bash

# 文件职责：
# 1. 统一输出配对码、配对链接和二维码（终端/PNG）。
# 2. 支持通过参数覆盖 relay、宿主机名称等信息，便于开发与演示。
# 3. 提供 iOS 模拟器扫码投递能力，复用同一条 yc://pair 深链。

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

usage() {
  cat <<'EOF'
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
EOF
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
  # 压缩多余空白并裁剪长度，避免配对链接过长。
  local compact
  compact="$(printf '%s' "${raw}" | awk '{$1=$1;print}')"
  printf '%s' "${compact}" | cut -c1-64
}

detect_host_name() {
  if [[ -n "${HOST_NAME}" ]]; then
    normalize_host_name "${HOST_NAME}"
    return
  fi

  for key in COMPUTERNAME HOSTNAME; do
    if [[ -n "${!key:-}" ]]; then
      normalize_host_name "${!key}"
      return
    fi
  done

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

url_encode() {
  require_cmd jq
  printf '%s' "$1" | jq -sRr @uri
}

build_pairing_link() {
  local relay_url="$1"
  local system_id="$2"
  local pairing_ticket="$3"
  local pairing_code="$4"
  local host_name="$5"

  local relay_enc sid_enc ticket_enc
  relay_enc="$(url_encode "${relay_url}")"
  sid_enc="$(url_encode "${system_id}")"
  ticket_enc="$(url_encode "${pairing_ticket}")"

  local link="yc://pair?relay=${relay_enc}&sid=${sid_enc}&ticket=${ticket_enc}"
  if [[ "${INCLUDE_CODE}" -eq 1 ]]; then
    local code_enc
    code_enc="$(url_encode "${pairing_code}")"
    link="${link}&code=${code_enc}"
  fi
  if [[ -n "${host_name}" ]]; then
    local name_enc
    name_enc="$(url_encode "${host_name}")"
    link="${link}&name=${name_enc}"
  fi
  echo "${link}"
}

base64url_encode_text() {
  printf '%s' "$1" \
    | openssl base64 -A \
    | tr '+/' '-_' \
    | tr -d '='
}

base64url_hmac_sha256() {
  local key="$1"
  local text="$2"
  printf '%s' "${text}" \
    | openssl dgst -sha256 -mac HMAC -macopt "key:${key}" -binary \
    | openssl base64 -A \
    | tr '+/' '-_' \
    | tr -d '='
}

generate_pair_ticket() {
  local system_id="$1"
  local pair_token="$2"
  local now exp nonce payload_json payload_b64 sig_b64

  now="$(date +%s)"
  exp="$((now + TICKET_TTL_SEC))"
  if command -v uuidgen >/dev/null 2>&1; then
    nonce="$(uuidgen | tr '[:upper:]' '[:lower:]' | tr -d '-')"
  else
    nonce="$(date +%s)-$RANDOM-$RANDOM"
  fi

  require_cmd jq
  payload_json="$(jq -nc \
    --arg sid "${system_id}" \
    --argjson iat "${now}" \
    --argjson exp "${exp}" \
    --arg nonce "${nonce}" \
    '{sid: $sid, iat: $iat, exp: $exp, nonce: $nonce}')"
  payload_b64="$(base64url_encode_text "${payload_json}")"
  sig_b64="$(base64url_hmac_sha256 "${pair_token}" "${payload_b64}")"
  echo "pct_v1.${payload_b64}.${sig_b64}"
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
PAIR_TICKET=""
PAIR_LINK=""
SIMCTL_CMD=""

NEED_LINK=1
if [[ "${SHOW_MODE}" == "code" && "${SIMULATE_IOS_SCAN}" -eq 0 && -z "${QR_PNG_PATH}" ]]; then
  NEED_LINK=0
fi
if [[ "${NEED_LINK}" -eq 1 ]]; then
  PAIR_TICKET="$(generate_pair_ticket "${SYSTEM_ID}" "${PAIR_TOKEN}")"
  PAIR_LINK="$(build_pairing_link "${RELAY_WS_URL}" "${SYSTEM_ID}" "${PAIR_TICKET}" "${PAIR_CODE}" "${HOST_NAME}")"
  SIMCTL_CMD="xcrun simctl openurl booted \"${PAIR_LINK}\""
fi

if [[ -n "${QR_PNG_PATH}" ]]; then
  if [[ "${NEED_LINK}" -ne 1 ]]; then
    PAIR_TICKET="$(generate_pair_ticket "${SYSTEM_ID}" "${PAIR_TOKEN}")"
    PAIR_LINK="$(build_pairing_link "${RELAY_WS_URL}" "${SYSTEM_ID}" "${PAIR_TICKET}" "${PAIR_CODE}" "${HOST_NAME}")"
  fi
  export_qr_png "${PAIR_LINK}" "${QR_PNG_PATH}"
fi

case "${SHOW_MODE}" in
  all)
    printf '%s\n' "宿主机名称: ${HOST_NAME}"
    printf '%s\n' "systemId: ${SYSTEM_ID}"
    printf '%s\n' "配对码: ${PAIR_CODE}"
    printf '%s\n' "短时票据: ${PAIR_TICKET}"
    printf '%s\n' "配对链接: ${PAIR_LINK}"
    printf '%s\n' "模拟扫码: ${SIMCTL_CMD}"
    print_qr_terminal "${PAIR_LINK}" || true
    ;;
  code)
    printf '%s\n' "${PAIR_CODE}"
    ;;
  link)
    printf '%s\n' "${PAIR_LINK}"
    ;;
  qr)
    print_qr_terminal "${PAIR_LINK}"
    ;;
  json)
    require_cmd jq
    jq -n \
      --arg hostName "${HOST_NAME}" \
      --arg systemId "${SYSTEM_ID}" \
      --arg relay "${RELAY_WS_URL}" \
      --arg code "${PAIR_CODE}" \
      --arg ticket "${PAIR_TICKET}" \
      --arg link "${PAIR_LINK}" \
      '{hostName: $hostName, systemId: $systemId, relay: $relay, code: $code, ticket: $ticket, link: $link}'
    ;;
  *)
    echo "不支持的 --show 模式: ${SHOW_MODE}" >&2
    exit 1
    ;;
esac

if [[ "${SIMULATE_IOS_SCAN}" -eq 1 ]]; then
  echo "simulate scan: ${PAIR_LINK}"
  xcrun simctl openurl booted "${PAIR_LINK}"
fi
