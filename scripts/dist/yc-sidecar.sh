#!/usr/bin/env bash

# 文件职责：
# 1. 安装/卸载 sidecar 执行机节点，覆盖 Linux(systemd) 与 macOS(launchd)。
# 2. 用户仅输入 relay 公网 IPv4，脚本内拼接标准 wss 地址并写入 sidecar 配置。
# 3. 提供统一服务管理命令：install/uninstall/status/doctor/start/stop/restart。

set -euo pipefail

SCRIPT_NAME="yc-sidecar.sh"
REPO="Zjianru/yourConnector"
RELEASE_TAG="__YC_RELEASE_TAG__"
DEFAULT_ASSET_BASE="__YC_ASSET_BASE__"

SERVICE_USER_DEFAULT="yourconnector"
SERVICE_GROUP_DEFAULT="yourconnector"
SERVICE_USER="${YC_SERVICE_USER:-$SERVICE_USER_DEFAULT}"
SERVICE_GROUP="${YC_SERVICE_GROUP:-$SERVICE_GROUP_DEFAULT}"

BIN_DIR="/usr/local/bin"
WORK_ROOT="/var/lib/yourconnector"
LOG_DIR="/var/log/yourconnector"
LINUX_SERVICE_FILE="/etc/systemd/system/yc-sidecar.service"
MAC_PLIST="/Library/LaunchDaemons/dev.yourconnector.sidecar.plist"
MAC_SERVICE_LABEL="dev.yourconnector.sidecar"

COMMAND=""
RELAY_IP=""
RELAY_URL=""
ASSET_BASE_URL="${YC_ASSET_BASE_URL:-}"
YES=0
KEEP_DATA=0
DRY_RUN=0
FORMAT="text"
ALLOW_INSECURE_WS=0
EFFECTIVE_RELAY_URL=""

usage() {
  cat <<'USAGE'
Usage:
  yc-sidecar.sh <command> [options]

Commands:
  install
  uninstall
  status
  doctor
  start
  stop
  restart

Options:
  --relay-ip <ipv4>       Required for first install; script builds wss://<ip>/v1/ws
  --relay <wss-url>       Backward-compatible full relay URL input
  --asset-base <url>      Optional override, default from embedded release metadata
  --allow-insecure-ws     Allow ws:// relay (debug only)
  --keep-data             Keep config/identity/logs when uninstall
  --yes                   Skip confirmations
  --dry-run               Print actions without executing
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
    fail "this script must run as root (use sudo)"
  fi
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

current_os() {
  case "$(uname -s)" in
    Linux) echo "linux" ;;
    Darwin) echo "macos" ;;
    *) fail "unsupported OS: $(uname -s)" ;;
  esac
}

arch_name() {
  case "$(uname -m)" in
    x86_64|amd64) echo "amd64" ;;
    aarch64|arm64) echo "arm64" ;;
    *) fail "unsupported architecture: $(uname -m)" ;;
  esac
}

normalize_asset_base_url() {
  if [[ -n "$ASSET_BASE_URL" ]]; then
    ASSET_BASE_URL="${ASSET_BASE_URL%/}"
    return 0
  fi
  ASSET_BASE_URL="${DEFAULT_ASSET_BASE%/}"
  [[ -n "$ASSET_BASE_URL" && "$ASSET_BASE_URL" != "__YC_ASSET_BASE__" ]] || fail "asset base is not configured in script metadata"
}

current_tag() {
  if [[ -n "${YC_RELEASE_TAG:-}" ]]; then
    echo "${YC_RELEASE_TAG}"
    return 0
  fi
  [[ "$RELEASE_TAG" != "__YC_RELEASE_TAG__" ]] || fail "release tag is not embedded in script metadata"
  echo "$RELEASE_TAG"
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
      --relay-ip)
        RELAY_IP="${2:-}"
        shift 2
        ;;
      --relay)
        RELAY_URL="${2:-}"
        shift 2
        ;;
      --asset-base)
        ASSET_BASE_URL="${2:-}"
        shift 2
        ;;
      --allow-insecure-ws)
        ALLOW_INSECURE_WS=1
        shift
        ;;
      --keep-data)
        KEEP_DATA=1
        shift
        ;;
      --yes)
        YES=1
        shift
        ;;
      --dry-run)
        DRY_RUN=1
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

is_valid_ipv4() {
  local ip="$1"
  [[ "$ip" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || return 1
  IFS='.' read -r o1 o2 o3 o4 <<<"$ip"
  for o in "$o1" "$o2" "$o3" "$o4"; do
    (( o >= 0 && o <= 255 )) || return 1
  done
  if (( o1 == 10 )); then return 1; fi
  if (( o1 == 127 )); then return 1; fi
  if (( o1 == 0 )); then return 1; fi
  if (( o1 == 169 && o2 == 254 )); then return 1; fi
  if (( o1 == 172 && o2 >= 16 && o2 <= 31 )); then return 1; fi
  if (( o1 == 192 && o2 == 168 )); then return 1; fi
  if (( o1 == 100 && o2 >= 64 && o2 <= 127 )); then return 1; fi
  if (( o1 >= 224 )); then return 1; fi
  return 0
}

relay_url_from_ip() {
  local ip="$1"
  echo "wss://${ip}/v1/ws"
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

read_persisted_relay_url() {
  local config_path="${WORK_ROOT}/.config/yourconnector/sidecar/config.json"
  [[ -f "$config_path" ]] || return 1
  sed -n 's/.*"relayWsUrl"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$config_path" | head -n1
}

resolve_relay_url_for_install() {
  if [[ -n "$RELAY_IP" ]]; then
    is_valid_ipv4 "$RELAY_IP" || fail "--relay-ip must be a valid public IPv4"
    EFFECTIVE_RELAY_URL="$(relay_url_from_ip "$RELAY_IP")"
    return 0
  fi
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

  if [[ "$YES" -eq 1 ]]; then
    fail "--relay-ip <ipv4> is required when --yes is set and no persisted relay exists" 2
  fi
  [[ -t 0 ]] || fail "--relay-ip <ipv4> is required in non-interactive mode" 2

  local input
  while true; do
    read -r -p "输入 relay 公网 IPv4: " input
    input="${input//[[:space:]]/}"
    if is_valid_ipv4 "$input"; then
      EFFECTIVE_RELAY_URL="$(relay_url_from_ip "$input")"
      return 0
    fi
    log "输入无效，请输入可公网访问的 IPv4（例如 47.95.30.225）"
  done
}

release_asset_url() {
  local file="$1"
  local tag
  tag="$(current_tag)"
  echo "${ASSET_BASE_URL}/${tag}/${file}"
}

ensure_service_user_linux() {
  if id -u "$SERVICE_USER" >/dev/null 2>&1; then
    return 0
  fi
  run_cmd useradd --system --home "$WORK_ROOT" --shell /usr/sbin/nologin "$SERVICE_USER"
}

ensure_work_dirs() {
  run_cmd mkdir -p "$WORK_ROOT" "$LOG_DIR"
  if [[ "$(current_os)" == "linux" ]]; then
    run_cmd chown -R "$SERVICE_USER:$SERVICE_GROUP" "$WORK_ROOT" "$LOG_DIR"
  fi
}

download_and_install_binary() {
  local os arch tar_name checksums tmp extract found
  os="$(current_os)"
  arch="$(arch_name)"

  tar_name="yc-sidecar-${os}-${arch}.tar.gz"
  checksums="checksums.txt"

  tmp="$(mktemp -d)"
  extract="$(mktemp -d)"
  run_cmd curl -fsSL "$(release_asset_url "$checksums")" -o "${tmp}/${checksums}"
  run_cmd curl -fsSL "$(release_asset_url "$tar_name")" -o "${tmp}/${tar_name}"

  if [[ "$DRY_RUN" -eq 0 ]]; then
    (
      cd "$tmp"
      sha256sum -c "$checksums" --ignore-missing
    ) || fail "checksum verify failed for ${tar_name}"
  else
    log "[dry-run] verify checksum for ${tar_name}"
  fi

  run_cmd tar -xzf "${tmp}/${tar_name}" -C "$extract"
  found="$(find "$extract" -type f -name yc-sidecar | head -n1 || true)"
  [[ -n "$found" ]] || fail "yc-sidecar binary not found in ${tar_name}"
  run_cmd install -m 0755 "$found" "${BIN_DIR}/yc-sidecar"

  rm -rf "$tmp" "$extract"
}

write_systemd_service() {
  local host_name
  host_name="$(hostname 2>/dev/null || echo sidecar-host)"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${LINUX_SERVICE_FILE}"
    return 0
  fi
  cat > "$LINUX_SERVICE_FILE" <<SERVICE
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

write_launchd_plist() {
  local host_name
  host_name="$(scutil --get ComputerName 2>/dev/null || hostname 2>/dev/null || echo sidecar-host)"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${MAC_PLIST}"
    return 0
  fi
  cat > "$MAC_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${MAC_SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${BIN_DIR}/yc-sidecar</string>
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key><string>${WORK_ROOT}</string>
    <key>HOST_NAME</key><string>${host_name}</string>
    <key>YC_ALLOW_INSECURE_WS</key><string>${ALLOW_INSECURE_WS}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${LOG_DIR}/sidecar.stdout.log</string>
  <key>StandardErrorPath</key><string>${LOG_DIR}/sidecar.stderr.log</string>
</dict>
</plist>
PLIST
  chmod 0644 "$MAC_PLIST"
}

set_relay_config() {
  local extra_flag=()
  if [[ "$ALLOW_INSECURE_WS" -eq 1 ]]; then
    extra_flag+=(--allow-insecure-ws)
  fi
  if [[ "$(current_os)" == "linux" ]]; then
    run_as_service_user "${BIN_DIR}/yc-sidecar" relay set "$EFFECTIVE_RELAY_URL" "${extra_flag[@]}"
    run_as_service_user "${BIN_DIR}/yc-sidecar" relay test "$EFFECTIVE_RELAY_URL" "${extra_flag[@]}"
  else
    HOME="$WORK_ROOT" "${BIN_DIR}/yc-sidecar" relay set "$EFFECTIVE_RELAY_URL" "${extra_flag[@]}"
    HOME="$WORK_ROOT" "${BIN_DIR}/yc-sidecar" relay test "$EFFECTIVE_RELAY_URL" "${extra_flag[@]}"
  fi
}

service_start() {
  if [[ "$(current_os)" == "linux" ]]; then
    run_cmd systemctl daemon-reload
    run_cmd systemctl enable yc-sidecar.service
    run_cmd systemctl restart yc-sidecar.service
    return 0
  fi
  run_cmd launchctl bootstrap system "$MAC_PLIST" 2>/dev/null || true
  run_cmd launchctl enable "system/${MAC_SERVICE_LABEL}" 2>/dev/null || true
  run_cmd launchctl kickstart -k "system/${MAC_SERVICE_LABEL}"
}

service_stop() {
  if [[ "$(current_os)" == "linux" ]]; then
    run_cmd systemctl stop yc-sidecar.service || true
    return 0
  fi
  run_cmd launchctl bootout system "$MAC_PLIST" 2>/dev/null || true
}

service_restart() {
  service_stop
  service_start
}

service_status_text() {
  if [[ "$(current_os)" == "linux" ]]; then
    systemctl is-active yc-sidecar.service 2>/dev/null || true
    return 0
  fi
  launchctl print "system/${MAC_SERVICE_LABEL}" >/dev/null 2>&1 && echo "active" || echo "inactive"
}

render_pairing_banner() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] yc-sidecar pairing show --format text --relay ${EFFECTIVE_RELAY_URL}"
    return 0
  fi
  if [[ "$(current_os)" == "linux" ]]; then
    run_as_service_user "${BIN_DIR}/yc-sidecar" pairing show --format text --relay "$EFFECTIVE_RELAY_URL" || true
  else
    HOME="$WORK_ROOT" "${BIN_DIR}/yc-sidecar" pairing show --format text --relay "$EFFECTIVE_RELAY_URL" || true
  fi
}

status_cmd() {
  local status
  status="$(service_status_text)"
  echo "yc-sidecar: ${status:-unknown}"
  [[ "$status" == "active" ]] && exit 0 || exit 1
}

doctor_cmd() {
  local service_status relay_url config_path
  service_status="$(service_status_text)"
  relay_url="$(read_persisted_relay_url || true)"
  config_path="${WORK_ROOT}/.config/yourconnector/sidecar/config.json"

  local code=0
  [[ "$service_status" == "active" ]] || code=1

  if [[ "$FORMAT" == "json" ]]; then
    cat <<JSON
{
  "service": "${service_status:-unknown}",
  "relayWsUrl": "${relay_url}",
  "configPath": "${config_path}",
  "platform": "$(current_os)"
}
JSON
  else
    echo "platform: $(current_os)"
    echo "service: ${service_status:-unknown}"
    echo "relay: ${relay_url:-unknown}"
    echo "config: ${config_path}"
  fi
  exit "$code"
}

install_cmd() {
  require_root
  require_cmd curl
  require_cmd tar
  require_cmd sha256sum
  normalize_asset_base_url
  resolve_relay_url_for_install

  if [[ "$(current_os)" == "linux" ]]; then
    require_cmd systemctl
    ensure_service_user_linux
  else
    require_cmd launchctl
  fi
  ensure_work_dirs

  download_and_install_binary
  if [[ "$(current_os)" == "linux" ]]; then
    write_systemd_service
  else
    write_launchd_plist
  fi
  set_relay_config
  service_start
  render_pairing_banner
}

uninstall_cmd() {
  require_root
  if ! confirm "Uninstall yc-sidecar?"; then
    log "cancelled"
    exit 0
  fi

  service_stop
  if [[ "$(current_os)" == "linux" ]]; then
    run_cmd systemctl disable yc-sidecar.service || true
    run_cmd rm -f "$LINUX_SERVICE_FILE"
    run_cmd systemctl daemon-reload
  else
    run_cmd rm -f "$MAC_PLIST"
  fi
  run_cmd rm -f "${BIN_DIR}/yc-sidecar"

  if [[ "$KEEP_DATA" -eq 0 ]]; then
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
    start) require_root; service_start ;;
    stop) require_root; service_stop ;;
    restart) require_root; service_restart ;;
    *) usage; fail "unsupported command: $COMMAND" ;;
  esac
}

main "$@"
