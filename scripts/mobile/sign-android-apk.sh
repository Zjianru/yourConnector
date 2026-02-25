#!/usr/bin/env bash

# 文件职责：
# 1. 对 Android unsigned APK 执行 zipalign + apksigner。
# 2. 统一本地与 CI 签名流程，避免手工命令漂移。

set -euo pipefail

INPUT_APK=""
OUTPUT_APK=""
KEYSTORE_PATH="${ANDROID_KEYSTORE_PATH:-}"
KEY_ALIAS="${ANDROID_KEY_ALIAS:-}"
STORE_PASS="${ANDROID_KEYSTORE_PASSWORD:-}"
KEY_PASS="${ANDROID_KEY_PASSWORD:-}"
BUILD_TOOLS_DIR="${ANDROID_BUILD_TOOLS_DIR:-}"

usage() {
  cat <<'EOF'
Usage:
  scripts/mobile/sign-android-apk.sh \
    --in <unsigned.apk> \
    --out <signed.apk> \
    --keystore <keystore.jks> \
    --alias <key-alias> \
    --store-pass <store-password> \
    [--key-pass <key-password>] \
    [--build-tools-dir <android-sdk/build-tools/<ver>>]

Env fallback:
  ANDROID_KEYSTORE_PATH
  ANDROID_KEY_ALIAS
  ANDROID_KEYSTORE_PASSWORD
  ANDROID_KEY_PASSWORD
  ANDROID_BUILD_TOOLS_DIR
  ANDROID_SDK_ROOT / ANDROID_HOME
EOF
}

fail() {
  echo "[android-sign] ERROR: $*" >&2
  exit 1
}

require_file() {
  local path="$1"
  [[ -f "$path" ]] || fail "file not found: $path"
}

is_exec() {
  local path="$1"
  [[ -x "$path" ]]
}

latest_build_tools_dir() {
  local sdk_root="$1"
  local base="${sdk_root%/}/build-tools"
  [[ -d "$base" ]] || return 1
  ls -1d "$base"/* 2>/dev/null | sort -V | tail -n1
}

resolve_build_tools_dir() {
  if [[ -n "${BUILD_TOOLS_DIR}" ]]; then
    echo "${BUILD_TOOLS_DIR}"
    return 0
  fi

  local sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"
  if [[ -n "${sdk_root}" ]]; then
    local latest
    latest="$(latest_build_tools_dir "${sdk_root}" || true)"
    if [[ -n "${latest}" ]]; then
      echo "${latest}"
      return 0
    fi
  fi
  return 1
}

parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --in)
        INPUT_APK="${2:-}"
        shift 2
        ;;
      --out)
        OUTPUT_APK="${2:-}"
        shift 2
        ;;
      --keystore)
        KEYSTORE_PATH="${2:-}"
        shift 2
        ;;
      --alias)
        KEY_ALIAS="${2:-}"
        shift 2
        ;;
      --store-pass)
        STORE_PASS="${2:-}"
        shift 2
        ;;
      --key-pass)
        KEY_PASS="${2:-}"
        shift 2
        ;;
      --build-tools-dir)
        BUILD_TOOLS_DIR="${2:-}"
        shift 2
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        fail "unknown option: $1"
        ;;
    esac
  done
}

parse_args "$@"

[[ -n "${INPUT_APK}" ]] || fail "--in is required"
[[ -n "${OUTPUT_APK}" ]] || fail "--out is required"
[[ -n "${KEYSTORE_PATH}" ]] || fail "--keystore is required (or ANDROID_KEYSTORE_PATH)"
[[ -n "${KEY_ALIAS}" ]] || fail "--alias is required (or ANDROID_KEY_ALIAS)"
[[ -n "${STORE_PASS}" ]] || fail "--store-pass is required (or ANDROID_KEYSTORE_PASSWORD)"
if [[ -z "${KEY_PASS}" ]]; then
  KEY_PASS="${STORE_PASS}"
fi

require_file "${INPUT_APK}"
require_file "${KEYSTORE_PATH}"

ZIPALIGN_CMD=""
APKSIGNER_CMD=""

if build_tools="$(resolve_build_tools_dir)"; then
  if is_exec "${build_tools}/zipalign" && is_exec "${build_tools}/apksigner"; then
    ZIPALIGN_CMD="${build_tools}/zipalign"
    APKSIGNER_CMD="${build_tools}/apksigner"
  fi
fi

if [[ -z "${ZIPALIGN_CMD}" ]]; then
  ZIPALIGN_CMD="$(command -v zipalign || true)"
fi
if [[ -z "${APKSIGNER_CMD}" ]]; then
  APKSIGNER_CMD="$(command -v apksigner || true)"
fi

[[ -n "${ZIPALIGN_CMD}" ]] || fail "zipalign not found; set ANDROID_SDK_ROOT or --build-tools-dir"
[[ -n "${APKSIGNER_CMD}" ]] || fail "apksigner not found; set ANDROID_SDK_ROOT or --build-tools-dir"

mkdir -p "$(dirname "${OUTPUT_APK}")"

ALIGNED_TMP="$(mktemp "${TMPDIR:-/tmp}/yc-apk-aligned-XXXXXX.apk")"
trap 'rm -f "${ALIGNED_TMP}"' EXIT

echo "[android-sign] zipalign: ${INPUT_APK}"
"${ZIPALIGN_CMD}" -f -p 4 "${INPUT_APK}" "${ALIGNED_TMP}"

echo "[android-sign] apksigner: ${OUTPUT_APK}"
"${APKSIGNER_CMD}" sign \
  --ks "${KEYSTORE_PATH}" \
  --ks-key-alias "${KEY_ALIAS}" \
  --ks-pass "pass:${STORE_PASS}" \
  --key-pass "pass:${KEY_PASS}" \
  --out "${OUTPUT_APK}" \
  "${ALIGNED_TMP}"

"${APKSIGNER_CMD}" verify --verbose --print-certs "${OUTPUT_APK}" >/dev/null
echo "[android-sign] done: ${OUTPUT_APK}"
