#!/usr/bin/env bash

# 文件职责：
# 1. 安装/卸载 relay 节点，支持 Linux(systemd + nginx + ACME) 与 macOS(launchd)。
# 2. 安装阶段要求用户输入公网 IPv4，并据此输出标准 relay 地址。
# 3. 提供统一命令：install/uninstall/status/doctor/start/stop/restart。

set -euo pipefail

SCRIPT_NAME="yc-relay.sh"
REPO="Zjianru/yourConnector"
RELEASE_TAG="__YC_RELEASE_TAG__"
DEFAULT_ASSET_BASE="__YC_ASSET_BASE__"

SERVICE_USER_DEFAULT="yourconnector"
SERVICE_GROUP_DEFAULT="yourconnector"
SERVICE_USER="${YC_SERVICE_USER:-$SERVICE_USER_DEFAULT}"
SERVICE_GROUP="${YC_SERVICE_GROUP:-$SERVICE_GROUP_DEFAULT}"

BIN_DIR="/usr/local/bin"
WORK_ROOT="/var/lib/yourconnector"
LEGO_PATH="${WORK_ROOT}/lego"
WEBROOT_DIR="${WORK_ROOT}/acme-webroot"
STATE_FILE="${WORK_ROOT}/state.json"
TLS_ROOT="/etc/yourconnector/tls"
TLS_RELEASE_DIR="${TLS_ROOT}/releases"
TLS_ACTIVE_LINK="${TLS_ROOT}/active"
NGINX_CONF="/etc/nginx/conf.d/yourconnector.conf"
RENEW_ENV="/etc/yourconnector/renew.env"
RENEW_SCRIPT="/usr/local/lib/yourconnector/yc-cert-renew.sh"
RELAY_LOG_DIR="/var/log/yourconnector"

LINUX_RELAY_SERVICE="/etc/systemd/system/yc-relay.service"
LINUX_RENEW_SERVICE="/etc/systemd/system/yc-cert-renew.service"
LINUX_RENEW_TIMER="/etc/systemd/system/yc-cert-renew.timer"

MAC_PLIST="/Library/LaunchDaemons/dev.yourconnector.relay.plist"
MAC_SERVICE_LABEL="dev.yourconnector.relay"

LEGO_VERSION_DEFAULT="v4.32.0"

COMMAND=""
ACME_EMAIL="${YC_ACME_EMAIL:-}"
PUBLIC_IP=""
ASSET_BASE_URL="${YC_ASSET_BASE_URL:-}"
YES=0
KEEP_DATA=0
DRY_RUN=0
FORMAT="text"
ACME_STAGING=0

usage() {
  cat <<'USAGE'
Usage:
  yc-relay.sh <command> [options]

Commands:
  install
  uninstall
  status
  doctor
  start
  stop
  restart

Options:
  --acme-email <email>    Required on Linux install (or env YC_ACME_EMAIL)
  --public-ip <ipv4>      Public IPv4; omit to input interactively during install
  --asset-base <url>      Optional override, default from embedded release metadata
  --acme-staging          Use Let's Encrypt staging endpoint (Linux only)
  --keep-data             Keep config/state/tls/logs when uninstall
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

validate_email() {
  local v="${1:-}"
  [[ "$v" =~ ^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$ ]] || return 1
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

state_get() {
  local key="$1"
  if [[ ! -f "$STATE_FILE" ]]; then
    return 1
  fi
  sed -n "s/.*\"${key}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$STATE_FILE" | head -n1
}

detect_public_ipv4() {
  local endpoints=(
    "https://api.ipify.org"
    "https://ifconfig.me/ip"
    "https://checkip.amazonaws.com"
  )
  local ip=""
  for ep in "${endpoints[@]}"; do
    ip="$(curl -fsSL --max-time 5 "$ep" 2>/dev/null || true)"
    ip="$(echo "$ip" | tr -d '[:space:]')"
    if [[ -n "$ip" ]] && is_valid_ipv4 "$ip"; then
      echo "$ip"
      return 0
    fi
  done
  return 1
}

acquire_public_ip() {
  if [[ -n "$PUBLIC_IP" ]]; then
    is_valid_ipv4 "$PUBLIC_IP" || fail "--public-ip must be a valid public IPv4"
    return 0
  fi

  local from_state input prompt
  from_state="$(state_get public_ip || true)"

  if [[ "$YES" -eq 1 ]]; then
    if [[ -n "$from_state" ]] && is_valid_ipv4 "$from_state"; then
      PUBLIC_IP="$from_state"
      return 0
    fi
    fail "--public-ip <ipv4> is required when --yes is set and no previous value exists" 2
  fi

  [[ -t 0 ]] || fail "--public-ip <ipv4> is required in non-interactive mode" 2
  while true; do
    if [[ -n "$from_state" ]] && is_valid_ipv4 "$from_state"; then
      prompt="输入公网 IPv4（直接回车复用 ${from_state}）: "
    else
      prompt="输入公网 IPv4: "
    fi
    read -r -p "$prompt" input
    input="${input//[[:space:]]/}"
    if [[ -z "$input" ]] && [[ -n "$from_state" ]] && is_valid_ipv4 "$from_state"; then
      PUBLIC_IP="$from_state"
      return 0
    fi
    if is_valid_ipv4 "$input"; then
      PUBLIC_IP="$input"
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

ensure_packages_linux() {
  require_cmd curl
  require_cmd tar
  require_cmd sha256sum
  require_cmd systemctl
  require_cmd openssl
  if command -v nginx >/dev/null 2>&1; then
    return 0
  fi
  if command -v apt-get >/dev/null 2>&1; then
    run_cmd apt-get update
    run_cmd apt-get install -y nginx ca-certificates
  elif command -v dnf >/dev/null 2>&1; then
    run_cmd dnf install -y nginx ca-certificates
  elif command -v yum >/dev/null 2>&1; then
    run_cmd yum install -y nginx ca-certificates
  else
    fail "cannot auto install nginx on this linux distro"
  fi
}

ensure_lego_linux() {
  if command -v lego >/dev/null 2>&1; then
    return 0
  fi
  local arch url name tmp lego_ver
  arch="$(arch_name)"
  lego_ver="${LEGO_VERSION:-$LEGO_VERSION_DEFAULT}"
  name="lego_${lego_ver}_linux_${arch}.tar.gz"
  url="https://github.com/go-acme/lego/releases/download/${lego_ver}/${name}"
  tmp="$(mktemp -d)"
  run_cmd curl -fsSL "$url" -o "${tmp}/${name}"
  run_cmd tar -xzf "${tmp}/${name}" -C "$tmp"
  [[ -f "${tmp}/lego" ]] || fail "lego binary not found in archive"
  run_cmd install -m 0755 "${tmp}/lego" "${BIN_DIR}/lego"
  rm -rf "$tmp"
}

ensure_service_user_linux() {
  if id -u "$SERVICE_USER" >/dev/null 2>&1; then
    return 0
  fi
  run_cmd useradd --system --home "$WORK_ROOT" --shell /usr/sbin/nologin "$SERVICE_USER"
}

ensure_dirs_linux() {
  run_cmd mkdir -p "$WORK_ROOT" "$LEGO_PATH" "$WEBROOT_DIR" "$TLS_RELEASE_DIR" "$(dirname "$RENEW_SCRIPT")" "$RELAY_LOG_DIR" "/etc/yourconnector"
  run_cmd chown -R "$SERVICE_USER:$SERVICE_GROUP" "$WORK_ROOT" "$RELAY_LOG_DIR"
  run_cmd chmod 0700 "$TLS_ROOT" || true
  run_cmd chmod 0700 "$TLS_RELEASE_DIR" || true
}

download_and_install_relay_binary() {
  local os arch tar_name checksums tmp extract found
  os="$(current_os)"
  arch="$(arch_name)"
  tar_name="yc-relay-${os}-${arch}.tar.gz"
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
  found="$(find "$extract" -type f -name yc-relay | head -n1 || true)"
  [[ -n "$found" ]] || fail "yc-relay binary not found in ${tar_name}"
  run_cmd install -m 0755 "$found" "${BIN_DIR}/yc-relay"
  rm -rf "$tmp" "$extract"
}

check_default_server_conflict_linux() {
  local conflict
  conflict="$(grep -RInE 'listen[[:space:]]+80[^;]*default_server|listen[[:space:]]+443[^;]*default_server' /etc/nginx 2>/dev/null | grep -v "${NGINX_CONF}" || true)"
  if [[ -n "$conflict" ]]; then
    fail "existing nginx default_server conflict found:\n${conflict}"
  fi
}

write_nginx_acme_conf_linux() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${NGINX_CONF} (acme-only)"
    return 0
  fi
  cat > "$NGINX_CONF" <<CONF
server {
    listen 80 default_server;
    server_name ${PUBLIC_IP};

    location ^~ /.well-known/acme-challenge/ {
        root ${WEBROOT_DIR};
        try_files \$uri =404;
    }

    location / {
        return 404;
    }
}
CONF
}

write_nginx_full_conf_linux() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${NGINX_CONF} (full)"
    return 0
  fi
  cat > "$NGINX_CONF" <<CONF
server {
    listen 80 default_server;
    server_name ${PUBLIC_IP};

    location ^~ /.well-known/acme-challenge/ {
        root ${WEBROOT_DIR};
        try_files \$uri =404;
    }

    location / {
        return 404;
    }
}

server {
    listen 443 ssl default_server;
    server_name ${PUBLIC_IP};

    ssl_certificate ${TLS_ACTIVE_LINK}/fullchain.pem;
    ssl_certificate_key ${TLS_ACTIVE_LINK}/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;

    location = /healthz {
        proxy_pass http://127.0.0.1:18080/healthz;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }

    location /v1/ws {
        proxy_pass http://127.0.0.1:18080/v1/ws;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }

    location / {
        proxy_pass http://127.0.0.1:18080;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }
}
CONF
}

nginx_reload_linux() {
  run_cmd nginx -t
  run_cmd systemctl enable nginx
  run_cmd systemctl restart nginx
}

write_relay_service_linux() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${LINUX_RELAY_SERVICE}"
    return 0
  fi
  cat > "$LINUX_RELAY_SERVICE" <<SERVICE
[Unit]
Description=yourConnector Relay
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${SERVICE_USER}
Group=${SERVICE_GROUP}
WorkingDirectory=${WORK_ROOT}
Environment=HOME=${WORK_ROOT}
Environment=RELAY_ADDR=127.0.0.1:18080
Environment=RELAY_PUBLIC_WS_URL=wss://${PUBLIC_IP}/v1/ws
ExecStart=${BIN_DIR}/yc-relay run
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
SERVICE
}

write_renew_env_linux() {
  run_cmd mkdir -p /etc/yourconnector
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${RENEW_ENV}"
    return 0
  fi
  cat > "$RENEW_ENV" <<ENV
ACME_EMAIL=${ACME_EMAIL}
PUBLIC_IP=${PUBLIC_IP}
ACME_STAGING=${ACME_STAGING}
WORK_ROOT=${WORK_ROOT}
LEGO_PATH=${LEGO_PATH}
WEBROOT_DIR=${WEBROOT_DIR}
TLS_ROOT=${TLS_ROOT}
TLS_RELEASE_DIR=${TLS_RELEASE_DIR}
TLS_ACTIVE_LINK=${TLS_ACTIVE_LINK}
STATE_FILE=${STATE_FILE}
NGINX_CONF=${NGINX_CONF}
ENV
  chmod 0600 "$RENEW_ENV"
}

write_renew_script_linux() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${RENEW_SCRIPT}"
    return 0
  fi
  cat > "$RENEW_SCRIPT" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
source /etc/yourconnector/renew.env

log() { printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*"; }
fail() { printf '[%s] ERROR: %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" >&2; exit 1; }

is_valid_ipv4() {
  local ip="$1"
  [[ "$ip" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || return 1
  IFS='.' read -r o1 o2 o3 o4 <<<"$ip"
  for o in "$o1" "$o2" "$o3" "$o4"; do (( o >= 0 && o <= 255 )) || return 1; done
  (( o1 == 10 )) && return 1
  (( o1 == 127 )) && return 1
  (( o1 == 0 )) && return 1
  (( o1 == 169 && o2 == 254 )) && return 1
  (( o1 == 172 && o2 >= 16 && o2 <= 31 )) && return 1
  (( o1 == 192 && o2 == 168 )) && return 1
  (( o1 == 100 && o2 >= 64 && o2 <= 127 )) && return 1
  (( o1 >= 224 )) && return 1
  return 0
}

state_get() {
  local key="$1"
  [[ -f "$STATE_FILE" ]] || return 1
  sed -n "s/.*\"${key}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$STATE_FILE" | head -n1
}

write_state() {
  local public_ip="$1"
  local issued_ip="$2"
  local next_retry_at="$3"
  local last_error="$4"
  local now
  now="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  cat > "$STATE_FILE" <<JSON
{
  "public_ip": "${public_ip}",
  "issued_ip": "${issued_ip}",
  "next_retry_at": "${next_retry_at}",
  "last_error": "${last_error}",
  "updated_at": "${now}"
}
JSON
}

cert_validate() {
  local fullchain="$1"
  local privkey="$2"
  local ip="$3"
  openssl x509 -in "$fullchain" -noout -checkend 3600 >/dev/null
  openssl x509 -in "$fullchain" -noout -text | grep -q "IP Address:${ip}"
  local cert_pub key_pub
  cert_pub="$(openssl x509 -in "$fullchain" -pubkey -noout | openssl sha256 | awk '{print $2}')"
  key_pub="$(openssl pkey -in "$privkey" -pubout | openssl sha256 | awk '{print $2}')"
  [[ "$cert_pub" == "$key_pub" ]]
}

issue() {
  local ip="$1"
  export LEGO_PATH
  local server_args=()
  if [[ "${ACME_STAGING:-0}" == "1" ]]; then
    server_args=(--server "https://acme-staging-v02.api.letsencrypt.org/directory")
  fi

  local cert_crt="${LEGO_PATH}/certificates/${ip}.crt"
  local cert_key="${LEGO_PATH}/certificates/${ip}.key"
  local cert_issuer="${LEGO_PATH}/certificates/${ip}.issuer.crt"
  local issued_ip
  issued_ip="$(state_get issued_ip || true)"

  lego "${server_args[@]}" --accept-tos --disable-cn --email "$ACME_EMAIL" --domains "$ip" --http --http.webroot "$WEBROOT_DIR" run --profile shortlived

  [[ -f "$cert_crt" ]] || fail "cert crt missing"
  [[ -f "$cert_key" ]] || fail "cert key missing"
  local release_dir="${TLS_RELEASE_DIR}/$(date -u +%Y%m%d%H%M%S)"
  mkdir -p "$release_dir"
  if [[ -f "$cert_issuer" ]]; then
    cat "$cert_crt" "$cert_issuer" > "${release_dir}/fullchain.pem"
  else
    cp "$cert_crt" "${release_dir}/fullchain.pem"
  fi
  cp "$cert_key" "${release_dir}/privkey.pem"
  chmod 0644 "${release_dir}/fullchain.pem"
  chmod 0600 "${release_dir}/privkey.pem"

  cert_validate "${release_dir}/fullchain.pem" "${release_dir}/privkey.pem" "$ip" || fail "certificate validation failed"
  local old_target=""
  if [[ -L "$TLS_ACTIVE_LINK" ]]; then
    old_target="$(readlink -f "$TLS_ACTIVE_LINK" || true)"
  fi
  ln -sfn "$release_dir" "${TLS_ACTIVE_LINK}.new"
  mv -Tf "${TLS_ACTIVE_LINK}.new" "$TLS_ACTIVE_LINK"
  if ! nginx -t >/dev/null 2>&1 || ! systemctl reload nginx >/dev/null 2>&1; then
    if [[ -n "$old_target" ]]; then
      ln -sfn "$old_target" "${TLS_ACTIVE_LINK}.new"
      mv -Tf "${TLS_ACTIVE_LINK}.new" "$TLS_ACTIVE_LINK"
      systemctl reload nginx >/dev/null 2>&1 || true
    fi
    fail "nginx reload failed after certificate switch"
  fi
  write_state "$ip" "$ip" "" ""
}

main() {
  mkdir -p "$WEBROOT_DIR" "$TLS_RELEASE_DIR" "$LEGO_PATH" "$(dirname "$STATE_FILE")"
  chmod 0700 "$TLS_ROOT" "$TLS_RELEASE_DIR" || true
  [[ -n "$PUBLIC_IP" ]] || fail "PUBLIC_IP missing"
  is_valid_ipv4 "$PUBLIC_IP" || fail "PUBLIC_IP invalid"
  issue "$PUBLIC_IP"
}

main "$@"
SCRIPT
  chmod 0755 "$RENEW_SCRIPT"
}

write_renew_units_linux() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write renew service/timer"
    return 0
  fi
  cat > "$LINUX_RENEW_SERVICE" <<SERVICE
[Unit]
Description=yourConnector TLS shortlived renew
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=${RENEW_SCRIPT}
SERVICE

  cat > "$LINUX_RENEW_TIMER" <<TIMER
[Unit]
Description=Run yourConnector TLS renew every 12h

[Timer]
OnBootSec=10min
OnUnitActiveSec=12h
RandomizedDelaySec=15m
Persistent=true

[Install]
WantedBy=timers.target
TIMER
}

write_acme_probe_linux() {
  run_cmd mkdir -p "${WEBROOT_DIR}/.well-known/acme-challenge"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ACME probe file"
    return 0
  fi
  echo "ok" > "${WEBROOT_DIR}/.well-known/acme-challenge/ping"
  chown -R "$SERVICE_USER:$SERVICE_GROUP" "$WEBROOT_DIR"
}

start_linux_services() {
  run_cmd systemctl daemon-reload
  run_cmd systemctl enable yc-relay.service yc-cert-renew.timer
  run_cmd systemctl restart yc-relay.service
  run_cmd systemctl restart yc-cert-renew.timer
}

write_launchd_plist_macos() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    log "[dry-run] write ${MAC_PLIST}"
    return 0
  fi
  run_cmd mkdir -p "$WORK_ROOT" "$RELAY_LOG_DIR"
  cat > "$MAC_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${MAC_SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${BIN_DIR}/yc-relay</string>
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>RELAY_ADDR</key><string>127.0.0.1:18080</string>
    <key>RELAY_PUBLIC_WS_URL</key><string>wss://${PUBLIC_IP}/v1/ws</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${RELAY_LOG_DIR}/relay.stdout.log</string>
  <key>StandardErrorPath</key><string>${RELAY_LOG_DIR}/relay.stderr.log</string>
</dict>
</plist>
PLIST
  chmod 0644 "$MAC_PLIST"
}

start_macos_service() {
  run_cmd launchctl bootstrap system "$MAC_PLIST" 2>/dev/null || true
  run_cmd launchctl enable "system/${MAC_SERVICE_LABEL}" 2>/dev/null || true
  run_cmd launchctl kickstart -k "system/${MAC_SERVICE_LABEL}"
}

stop_macos_service() {
  run_cmd launchctl bootout system "$MAC_PLIST" 2>/dev/null || true
}

service_start() {
  if [[ "$(current_os)" == "linux" ]]; then
    run_cmd systemctl start nginx yc-relay.service yc-cert-renew.timer
    return 0
  fi
  start_macos_service
}

service_stop() {
  if [[ "$(current_os)" == "linux" ]]; then
    run_cmd systemctl stop yc-cert-renew.timer yc-cert-renew.service yc-relay.service nginx || true
    return 0
  fi
  stop_macos_service
}

service_restart() {
  service_stop
  service_start
}

service_status_text() {
  if [[ "$(current_os)" == "linux" ]]; then
    systemctl is-active yc-relay.service 2>/dev/null || true
    return 0
  fi
  launchctl print "system/${MAC_SERVICE_LABEL}" >/dev/null 2>&1 && echo "active" || echo "inactive"
}

status_cmd() {
  local st
  st="$(service_status_text)"
  echo "yc-relay: ${st:-unknown}"
  [[ "$st" == "active" ]] && exit 0 || exit 1
}

port_listener_count() {
  local port="$1"
  if command -v ss >/dev/null 2>&1; then
    ss -lnt "( sport = :${port} )" 2>/dev/null | tail -n +2 | wc -l | tr -d '[:space:]'
    return 0
  fi
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"${port}" -sTCP:LISTEN 2>/dev/null | tail -n +2 | wc -l | tr -d '[:space:]'
    return 0
  fi
  echo "0"
}

doctor_cmd() {
  local st cert_expiry issued_ip
  st="$(service_status_text)"
  issued_ip="$(state_get issued_ip || true)"
  cert_expiry=""
  if [[ -f "${TLS_ACTIVE_LINK}/fullchain.pem" ]]; then
    cert_expiry="$(openssl x509 -in "${TLS_ACTIVE_LINK}/fullchain.pem" -noout -enddate | cut -d= -f2 || true)"
  fi
  local code=0
  [[ "$st" == "active" ]] || code=1
  if [[ "$FORMAT" == "json" ]]; then
    cat <<JSON
{
  "platform": "$(current_os)",
  "service": "${st:-unknown}",
  "publicIp": "${PUBLIC_IP:-$(state_get public_ip || true)}",
  "issuedIp": "${issued_ip}",
  "ports": {
    "80": "$(port_listener_count 80)",
    "443": "$(port_listener_count 443)"
  },
  "certExpiry": "${cert_expiry}"
}
JSON
  else
    echo "platform: $(current_os)"
    echo "service: ${st:-unknown}"
    echo "public-ip: ${PUBLIC_IP:-$(state_get public_ip || true)}"
    echo "issued-ip: ${issued_ip:-unknown}"
    echo "cert-expiry: ${cert_expiry:-unknown}"
  fi
  exit "$code"
}

install_linux() {
  validate_email "$ACME_EMAIL" || fail "--acme-email <email> is required and must be valid" 2
  ensure_packages_linux
  ensure_lego_linux
  ensure_service_user_linux
  ensure_dirs_linux

  check_default_server_conflict_linux
  write_nginx_acme_conf_linux
  nginx_reload_linux
  write_acme_probe_linux

  download_and_install_relay_binary
  write_relay_service_linux
  write_renew_env_linux
  write_renew_script_linux
  write_renew_units_linux
  run_cmd "$RENEW_SCRIPT"
  write_nginx_full_conf_linux
  nginx_reload_linux
  start_linux_services
}

install_macos() {
  require_cmd launchctl
  download_and_install_relay_binary
  write_launchd_plist_macos
  start_macos_service
}

install_cmd() {
  require_root
  require_cmd curl
  require_cmd tar
  require_cmd sha256sum
  normalize_asset_base_url
  acquire_public_ip
  if [[ "$(current_os)" == "linux" ]]; then
    install_linux
  else
    install_macos
  fi

  log "relay url: wss://${PUBLIC_IP}/v1/ws"
  if [[ "$(current_os)" == "linux" ]]; then
    log "health url: https://${PUBLIC_IP}/healthz"
  else
    log "health url: http://127.0.0.1:18080/healthz"
  fi
}

uninstall_cmd() {
  require_root
  if ! confirm "Uninstall yc-relay?"; then
    log "cancelled"
    exit 0
  fi

  service_stop
  if [[ "$(current_os)" == "linux" ]]; then
    run_cmd systemctl disable yc-relay.service yc-cert-renew.timer || true
    run_cmd rm -f "$LINUX_RELAY_SERVICE" "$LINUX_RENEW_SERVICE" "$LINUX_RENEW_TIMER" "$NGINX_CONF" "$RENEW_SCRIPT" "$RENEW_ENV"
    run_cmd systemctl daemon-reload
  else
    run_cmd rm -f "$MAC_PLIST"
  fi
  run_cmd rm -f "${BIN_DIR}/yc-relay"

  if [[ "$KEEP_DATA" -eq 0 ]]; then
    run_cmd rm -rf /etc/yourconnector "$WORK_ROOT" "$RELAY_LOG_DIR"
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
      --acme-email)
        ACME_EMAIL="${2:-}"
        shift 2
        ;;
      --public-ip)
        PUBLIC_IP="${2:-}"
        shift 2
        ;;
      --asset-base)
        ASSET_BASE_URL="${2:-}"
        shift 2
        ;;
      --acme-staging)
        ACME_STAGING=1
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
