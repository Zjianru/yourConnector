#!/usr/bin/env bash

# 文件职责：
# 1. 安装/卸载 sidecar 执行机节点（Linux）。
# 2. 对接远端 relay（默认要求 wss://.../v1/ws）。
# 3. 输出配对信息，便于 mobile 添加宿主机。

set -euo pipefail

SCRIPT_NAME="sidecar.sh"
REPO="Zjianru/yourConnector"
SERVICE_USER="yourconnector"
SERVICE_GROUP="yourconnector"
BIN_DIR="/usr/local/bin"
WORK_ROOT="/var/lib/yourconnector"
STATE_DIR="${WORK_ROOT}"
SERVICE_FILE="/etc/systemd/system/yc-sidecar.service"
LOG_DIR="/var/log/yourconnector"
LOG_FILE="${LOG_DIR}/sidecar.log"

COMMAND=""
VERSION=""
RELAY_URL=""
DRY_RUN=0
YES=0
PURGE=0
FORMAT="text"
ALLOW_INSECURE_WS=0
EFFECTIVE_RELAY_URL=""

usage() {
  cat <<'USAGE'
Usage:
  sidecar.sh <command> [options]

Commands:
  install
  uninstall
  status
  doctor
  start
  stop
  restart

Options:
  --version <tag>         Required for install. Example: v0.1.0
  --relay <wss-url>       Required for install unless already set
  --allow-insecure-ws     Allow ws:// relay (debug only)
  --dry-run               Print actions only
  --yes                   Skip confirmations
  --purge                 With uninstall, remove config/identity/logs
  --format <text|json>    Output format for doctor
USAGE
}

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

fail() {
  local code="${2:-1}"
  printf '[%s] ERROR: %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" >&2
  exit "$code"
}

run_cmd() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] $*"
    return 0
  fi
  "$@"
}

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    fail "this script must run as root (use sudo bash -s -- ...)"
  fi
}

parse_args() {
  if [[ "$#" -lt 1 ]]; then
    usage
    exit 1
  fi

  if [[ "$1" == "-h" || "$1" == "--help" || "$1" == "help" ]]; then
    usage
    exit 0
  fi

  COMMAND="$1"
  shift

  while [[ "$#" -gt 0 ]]; do
    case "$1" in
      --version)
        VERSION="${2:-}"
        shift 2
        ;;
      --relay)
        RELAY_URL="${2:-}"
        shift 2
        ;;
      --allow-insecure-ws)
        ALLOW_INSECURE_WS=1
        shift
        ;;
      --dry-run)
        DRY_RUN=1
        shift
        ;;
      --yes)
        YES=1
        shift
        ;;
      --purge)
        PURGE=1
        shift
        ;;
      --format)
        FORMAT="${2:-}"
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

  case "$FORMAT" in
    text|json) ;;
    *) fail "--format must be text or json" ;;
  esac
}

confirm() {
  local prompt="$1"
  if [[ "$YES" -eq 1 ]]; then
    return 0
  fi
  read -r -p "${prompt} [y/N]: " ans
  case "${ans:-}" in
    y|Y|yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

run_as_service_user() {
  if command -v runuser >/dev/null 2>&1; then
    runuser -u "$SERVICE_USER" -- env HOME="$WORK_ROOT" "$@"
    return 0
  fi
  if command -v sudo >/dev/null 2>&1; then
    sudo -u "$SERVICE_USER" HOME="$WORK_ROOT" "$@"
    return 0
  fi
  fail "neither runuser nor sudo is available"
}

arch_name() {
  case "$(uname -m)" in
    x86_64|amd64) echo "amd64" ;;
    aarch64|arm64) echo "arm64" ;;
    *) fail "unsupported arch: $(uname -m)" ;;
  esac
}

ensure_packages() {
  require_cmd curl
  require_cmd tar
  require_cmd sha256sum
  require_cmd systemctl
}

ensure_service_user() {
  if id -u "$SERVICE_USER" >/dev/null 2>&1; then
    return 0
  fi
  run_cmd useradd --system --home "$WORK_ROOT" --shell /usr/sbin/nologin "$SERVICE_USER"
}

ensure_directories() {
  run_cmd mkdir -p "$WORK_ROOT" "$LOG_DIR"
  run_cmd chown -R "$SERVICE_USER:$SERVICE_GROUP" "$WORK_ROOT" "$LOG_DIR"
}

release_url() {
  local file="$1"
  echo "https://github.com/${REPO}/releases/download/${VERSION}/${file}"
}

verify_checksum() {
  local file="$1"
  local checksum_file="$2"
  local base
  base="$(basename "$file")"
  local line
  line="$(grep "  ${base}$" "$checksum_file" || true)"
  [[ -n "$line" ]] || fail "checksum entry missing for ${base}"
  local expected actual
  expected="$(echo "$line" | awk '{print $1}')"
  actual="$(sha256sum "$file" | awk '{print $1}')"
  [[ "$expected" == "$actual" ]] || fail "checksum mismatch for ${base}"
}

install_sidecar_binary() {
  local arch
  arch="$(arch_name)"
  local tar_name="yc-sidecar-linux-${arch}.tar.gz"
  local checksums="checksums.txt"

  local tmp
  tmp="$(mktemp -d)"
  run_cmd curl -fsSL "$(release_url "$checksums")" -o "${tmp}/${checksums}"
  run_cmd curl -fsSL "$(release_url "$tar_name")" -o "${tmp}/${tar_name}"
  verify_checksum "${tmp}/${tar_name}" "${tmp}/${checksums}"

  local extract
  extract="$(mktemp -d)"
  run_cmd tar -xzf "${tmp}/${tar_name}" -C "$extract"
  local found
  found="$(find "$extract" -type f -name yc-sidecar | head -n1 || true)"
  [[ -n "$found" ]] || fail "yc-sidecar binary not found in archive"
  run_cmd install -m 0755 "$found" "${BIN_DIR}/yc-sidecar"
  rm -rf "$tmp" "$extract"
}

write_service() {
  local host_name
  host_name="$(hostname 2>/dev/null || echo sidecar-host)"

  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${SERVICE_FILE}"
    return 0
  fi

  cat > "$SERVICE_FILE" <<SERVICE
[Unit]
Description=yourConnector Sidecar
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${SERVICE_USER}
Group=${SERVICE_GROUP}
WorkingDirectory=${WORK_ROOT}
Environment=HOME=${WORK_ROOT}
Environment=HOST_NAME=${host_name}
Environment=YC_ALLOW_INSECURE_WS=${ALLOW_INSECURE_WS}
ExecStart=${BIN_DIR}/yc-sidecar run
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
SERVICE
}

read_persisted_relay_url() {
  local config_path="${WORK_ROOT}/.config/yourconnector/sidecar/config.json"
  [[ -f "$config_path" ]] || return 1
  sed -n 's/.*"relayWsUrl"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$config_path" | head -n1
}

resolve_install_relay_url() {
  if [[ -n "$RELAY_URL" ]]; then
    EFFECTIVE_RELAY_URL="$RELAY_URL"
    return 0
  fi

  local persisted
  persisted="$(read_persisted_relay_url || true)"
  if [[ -n "$persisted" ]]; then
    EFFECTIVE_RELAY_URL="$persisted"
    return 0
  fi

  fail "--relay <wss-url> is required for install when no persisted relay exists"
}

set_relay_config() {
  resolve_install_relay_url

  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] set relay ${EFFECTIVE_RELAY_URL}"
    return 0
  fi

  local optional_flag=()
  if [[ "$ALLOW_INSECURE_WS" -eq 1 ]]; then
    optional_flag+=(--allow-insecure-ws)
  fi

  run_as_service_user "${BIN_DIR}/yc-sidecar" relay set "$EFFECTIVE_RELAY_URL" "${optional_flag[@]}"
  run_as_service_user "${BIN_DIR}/yc-sidecar" relay test "$EFFECTIVE_RELAY_URL" "${optional_flag[@]}"
}

render_pairing() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] yc-sidecar pairing show --format text --relay ${EFFECTIVE_RELAY_URL}"
    return 0
  fi
  run_as_service_user "${BIN_DIR}/yc-sidecar" pairing show --format text --relay "$EFFECTIVE_RELAY_URL" || true
}

start_service() {
  run_cmd systemctl daemon-reload
  run_cmd systemctl enable yc-sidecar.service
  run_cmd systemctl restart yc-sidecar.service
}

stop_service() {
  run_cmd systemctl stop yc-sidecar.service || true
}

status_cmd() {
  local active
  active="$(systemctl is-active yc-sidecar.service 2>/dev/null || true)"
  echo "yc-sidecar.service: ${active:-unknown}"
  [[ "$active" == "active" ]] && exit 0 || exit 1
}

doctor_cmd() {
  local active
  active="$(systemctl is-active yc-sidecar.service 2>/dev/null || true)"
  local relay
  relay="$(runuser -u "$SERVICE_USER" -- env HOME="$WORK_ROOT" "${BIN_DIR}/yc-sidecar" relay 2>/dev/null | sed -n 's/^relay (effective): //p' | head -n1 || true)"

  local code=0
  [[ "$active" == "active" ]] || code=1

  if [[ "$FORMAT" == "json" ]]; then
    cat <<JSON
{
  "systemd": {
    "sidecar": "${active:-unknown}"
  },
  "relay": "${relay}",
  "configPath": "${WORK_ROOT}/.config/yourconnector/sidecar/config.json"
}
JSON
  else
    echo "sidecar: ${active:-unknown}"
    echo "relay: ${relay:-unknown}"
    echo "config: ${WORK_ROOT}/.config/yourconnector/sidecar/config.json"
  fi

  exit "$code"
}

install_cmd() {
  [[ -n "$VERSION" ]] || fail "--version <tag> is required for install"
  require_root
  ensure_packages
  ensure_service_user
  ensure_directories
  install_sidecar_binary
  write_service
  set_relay_config
  start_service
  render_pairing
}

uninstall_cmd() {
  require_root

  if ! confirm "Uninstall sidecar service?"; then
    log "cancelled"
    exit 0
  fi

  stop_service
  run_cmd systemctl disable yc-sidecar.service || true
  run_cmd rm -f "$SERVICE_FILE"
  run_cmd systemctl daemon-reload
  run_cmd rm -f "${BIN_DIR}/yc-sidecar"

  if [[ "$PURGE" -eq 1 ]]; then
    if ! confirm "Purge sidecar config/identity/logs?"; then
      log "purge skipped"
      exit 0
    fi
    run_cmd rm -rf "$WORK_ROOT/.config/yourconnector/sidecar" "$LOG_DIR"
  fi
}

main() {
  parse_args "$@"

  case "$COMMAND" in
    install) install_cmd ;;
    uninstall) uninstall_cmd ;;
    status) status_cmd ;;
    doctor) doctor_cmd ;;
    start) require_root; start_service ;;
    stop) require_root; stop_service ;;
    restart) require_root; stop_service; start_service ;;
    *) usage; fail "unsupported command: $COMMAND" ;;
  esac
}

main "$@"
