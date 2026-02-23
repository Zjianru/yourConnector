#!/usr/bin/env bash

# 文件职责：
# 1. 安装/卸载 relay+sidecar 一体化节点（Linux）。
# 2. 通过 nginx + lego 提供无域名 WSS（IP 证书 shortlived）。
# 3. 提供状态与诊断命令，支持重装测试与故障排查。

set -euo pipefail

SCRIPT_NAME="relay-sidecar.sh"
REPO="Zjianru/yourConnector"
SERVICE_USER="yourconnector"
SERVICE_GROUP="yourconnector"
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
RELAY_SERVICE="/etc/systemd/system/yc-relay.service"
SIDECAR_SERVICE="/etc/systemd/system/yc-sidecar.service"
RENEW_SERVICE="/etc/systemd/system/yc-cert-renew.service"
RENEW_TIMER="/etc/systemd/system/yc-cert-renew.timer"
RELAYSIDE_LOG_DIR="/var/log/yourconnector"
RELAY_LOG_FILE="${RELAYSIDE_LOG_DIR}/relay.log"
SIDECAR_LOG_FILE="${RELAYSIDE_LOG_DIR}/sidecar.log"

LEGO_VERSION_DEFAULT="v4.18.0"

COMMAND=""
VERSION=""
ACME_EMAIL="${YC_ACME_EMAIL:-}"
PUBLIC_IP=""
ACME_STAGING=0
DRY_RUN=0
YES=0
PURGE=0
FORMAT="text"
ASSET_BASE_URL="${YC_ASSET_BASE_URL:-}"

usage() {
  cat <<'USAGE'
Usage:
  relay-sidecar.sh <command> [options]

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
  --asset-base <url>      Optional release base URL, example: https://<domain>/releases
  --acme-email <email>    Required for install (or env YC_ACME_EMAIL)
  --public-ip <ipv4>      Optional public IPv4 override
  --acme-staging          Use Let's Encrypt staging endpoint
  --dry-run               Print checks/actions without changing system
  --yes                   Skip interactive confirmations
  --purge                 With uninstall, remove config/identity/certs/logs
  --format <text|json>    Output format for doctor (default text)
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
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log "[dry-run] $*"
    return 0
  fi
  "$@"
}

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    fail "this script must run as root (use sudo bash -s -- ...)" 1
  fi
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
      --acme-email)
        ACME_EMAIL="${2:-}"
        shift 2
        ;;
      --asset-base)
        ASSET_BASE_URL="${2:-}"
        shift 2
        ;;
      --public-ip)
        PUBLIC_IP="${2:-}"
        shift 2
        ;;
      --acme-staging)
        ACME_STAGING=1
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

normalize_asset_base_url() {
  if [[ -n "${ASSET_BASE_URL}" ]]; then
    ASSET_BASE_URL="${ASSET_BASE_URL%/}"
  fi
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

confirm() {
  local prompt="$1"
  if [[ "${YES}" -eq 1 ]]; then
    return 0
  fi
  read -r -p "${prompt} [y/N]: " ans
  case "${ans:-}" in
    y|Y|yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

arch_name() {
  local m
  m="$(uname -m)"
  case "$m" in
    x86_64|amd64) echo "amd64" ;;
    aarch64|arm64) echo "arm64" ;;
    *) fail "unsupported architecture: $m" ;;
  esac
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

  # reject private/reserved/loopback/link-local/CGNAT
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

detect_public_ipv4() {
  local candidates=(
    "https://api.ipify.org"
    "https://ifconfig.me/ip"
    "https://checkip.amazonaws.com"
  )

  local candidate=""
  for endpoint in "${candidates[@]}"; do
    candidate="$(curl -fsSL --max-time 5 "$endpoint" 2>/dev/null || true)"
    candidate="$(echo "$candidate" | tr -d '[:space:]')"
    if [[ -n "$candidate" ]] && is_valid_ipv4 "$candidate"; then
      echo "$candidate"
      return 0
    fi
  done
  return 1
}

state_get() {
  local key="$1"
  if [[ ! -f "$STATE_FILE" ]]; then
    return 1
  fi
  sed -n "s/.*\"${key}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$STATE_FILE" | head -n1
}

write_state() {
  local public_ip="$1"
  local issued_ip="$2"
  local next_retry_at="$3"
  local last_error="$4"
  local now
  now="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  run_cmd mkdir -p "$WORK_ROOT"
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log "[dry-run] write state public_ip=${public_ip} issued_ip=${issued_ip} next_retry_at=${next_retry_at}"
    return 0
  fi
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

ensure_packages() {
  require_cmd curl
  require_cmd tar
  require_cmd openssl
  require_cmd sha256sum
  require_cmd systemctl

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
    fail "cannot install nginx automatically: unsupported package manager"
  fi
}

ensure_lego() {
  if command -v lego >/dev/null 2>&1; then
    return 0
  fi

  local arch
  arch="$(arch_name)"
  local lego_ver="${LEGO_VERSION:-$LEGO_VERSION_DEFAULT}"
  local lego_name="lego_${lego_ver#v}_linux_${arch}.tar.gz"
  local url="https://github.com/go-acme/lego/releases/download/${lego_ver}/${lego_name}"
  local tmp_dir
  tmp_dir="$(mktemp -d)"

  log "installing lego ${lego_ver}"
  run_cmd curl -fsSL "$url" -o "${tmp_dir}/${lego_name}"
  run_cmd tar -xzf "${tmp_dir}/${lego_name}" -C "$tmp_dir"
  [[ -f "${tmp_dir}/lego" ]] || fail "lego binary not found in archive"
  run_cmd install -m 0755 "${tmp_dir}/lego" "${BIN_DIR}/lego"
  rm -rf "$tmp_dir"
}

ensure_service_user() {
  if id -u "$SERVICE_USER" >/dev/null 2>&1; then
    return 0
  fi
  run_cmd useradd --system --home "$WORK_ROOT" --shell /usr/sbin/nologin "$SERVICE_USER"
}

ensure_directories() {
  run_cmd mkdir -p "$WORK_ROOT" "$LEGO_PATH" "$WEBROOT_DIR" "$TLS_RELEASE_DIR" "$(dirname "$RENEW_SCRIPT")" "$RELAYSIDE_LOG_DIR" "/etc/yourconnector"
  run_cmd chown -R "$SERVICE_USER:$SERVICE_GROUP" "$WORK_ROOT" "$RELAYSIDE_LOG_DIR"
  run_cmd chmod 0700 "$TLS_ROOT" || true
  run_cmd chmod 0700 "$TLS_RELEASE_DIR" || true
}

release_url() {
  local file="$1"
  if [[ -n "${ASSET_BASE_URL}" ]]; then
    if [[ "${ASSET_BASE_URL}" == *"{tag}"* ]]; then
      echo "${ASSET_BASE_URL//\{tag\}/${VERSION}}/${file}"
      return 0
    fi
    echo "${ASSET_BASE_URL}/${VERSION}/${file}"
    return 0
  fi
  echo "https://github.com/${REPO}/releases/download/${VERSION}/${file}"
}

download_release_assets() {
  local arch
  arch="$(arch_name)"

  local relay_tar="yc-relay-linux-${arch}.tar.gz"
  local sidecar_tar="yc-sidecar-linux-${arch}.tar.gz"
  local checksums="checksums.txt"

  TMP_ASSET_DIR="$(mktemp -d)"
  run_cmd curl -fsSL "$(release_url "$checksums")" -o "${TMP_ASSET_DIR}/${checksums}"
  run_cmd curl -fsSL "$(release_url "$relay_tar")" -o "${TMP_ASSET_DIR}/${relay_tar}"
  run_cmd curl -fsSL "$(release_url "$sidecar_tar")" -o "${TMP_ASSET_DIR}/${sidecar_tar}"

  verify_checksum "${TMP_ASSET_DIR}/${relay_tar}" "${TMP_ASSET_DIR}/${checksums}"
  verify_checksum "${TMP_ASSET_DIR}/${sidecar_tar}" "${TMP_ASSET_DIR}/${checksums}"

  install_binary_from_tar "${TMP_ASSET_DIR}/${relay_tar}" "yc-relay" "${BIN_DIR}/yc-relay"
  install_binary_from_tar "${TMP_ASSET_DIR}/${sidecar_tar}" "yc-sidecar" "${BIN_DIR}/yc-sidecar"
}

verify_checksum() {
  local file="$1"
  local sum_file="$2"
  local base
  base="$(basename "$file")"
  local line
  line="$(grep "  ${base}$" "$sum_file" || true)"
  [[ -n "$line" ]] || fail "checksum entry missing for ${base}"
  local expected
  expected="$(echo "$line" | awk '{print $1}')"
  local actual
  actual="$(sha256sum "$file" | awk '{print $1}')"
  [[ "$expected" == "$actual" ]] || fail "checksum mismatch for ${base}"
}

install_binary_from_tar() {
  local archive="$1"
  local name="$2"
  local dest="$3"
  local tmp
  tmp="$(mktemp -d)"
  run_cmd tar -xzf "$archive" -C "$tmp"
  local found
  found="$(find "$tmp" -type f -name "$name" | head -n1 || true)"
  [[ -n "$found" ]] || fail "binary ${name} not found in $(basename "$archive")"
  run_cmd install -m 0755 "$found" "$dest"
  rm -rf "$tmp"
}

check_default_server_conflict() {
  local conflict
  conflict="$(grep -RInE 'listen[[:space:]]+80[^;]*default_server|listen[[:space:]]+443[^;]*default_server' /etc/nginx 2>/dev/null | grep -v "${NGINX_CONF}" || true)"
  if [[ -n "$conflict" ]]; then
    fail "existing nginx default_server conflict found:\n${conflict}"
  fi
}

write_nginx_acme_conf() {
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log "[dry-run] write ${NGINX_CONF} (acme-only)"
    return 0
  fi
  cat > "$NGINX_CONF" <<CONF
server {
    listen 80 default_server;
    server_name _;

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

write_nginx_full_conf() {
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log "[dry-run] write ${NGINX_CONF} (full tls)"
    return 0
  fi
  cat > "$NGINX_CONF" <<CONF
server {
    listen 80 default_server;
    server_name _;

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
    server_name _;

    ssl_certificate ${TLS_ACTIVE_LINK}/fullchain.pem;
    ssl_certificate_key ${TLS_ACTIVE_LINK}/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_session_timeout 1d;
    ssl_session_cache shared:SSL:10m;
    ssl_prefer_server_ciphers off;

    location = /healthz {
        proxy_pass http://127.0.0.1:18789/healthz;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }

    location /v1/ws {
        proxy_pass http://127.0.0.1:18789/v1/ws;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }

    location / {
        proxy_pass http://127.0.0.1:18789;
        proxy_set_header Host \$host;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }
}
CONF
}

nginx_reload_safe() {
  run_cmd nginx -t
  run_cmd systemctl enable nginx
  run_cmd systemctl restart nginx
}

write_services() {
  local host_name
  host_name="$(hostname 2>/dev/null || echo relay-host)"

  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log "[dry-run] write systemd services"
    return 0
  fi

  cat > "$RELAY_SERVICE" <<SERVICE
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
Environment=RELAY_ADDR=127.0.0.1:18789
ExecStart=${BIN_DIR}/yc-relay
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
SERVICE

  cat > "$SIDECAR_SERVICE" <<SERVICE
[Unit]
Description=yourConnector Sidecar
After=network-online.target yc-relay.service
Wants=network-online.target

[Service]
Type=simple
User=${SERVICE_USER}
Group=${SERVICE_GROUP}
WorkingDirectory=${WORK_ROOT}
Environment=HOME=${WORK_ROOT}
Environment=RELAY_WS_URL=ws://127.0.0.1:18789/v1/ws
Environment=HOST_NAME=${host_name}
Environment=YC_ALLOW_INSECURE_WS=1
ExecStart=${BIN_DIR}/yc-sidecar run
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
SERVICE

  cat > "$RENEW_SERVICE" <<SERVICE
[Unit]
Description=yourConnector TLS shortlived renew
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=${RENEW_SCRIPT}

[Install]
WantedBy=multi-user.target
SERVICE

  cat > "$RENEW_TIMER" <<SERVICE
[Unit]
Description=Run yourConnector TLS renew every 12h

[Timer]
OnBootSec=10min
OnUnitActiveSec=12h
RandomizedDelaySec=15m
Persistent=true

[Install]
WantedBy=timers.target
SERVICE
}

write_renew_env() {
  run_cmd mkdir -p /etc/yourconnector
  if [[ "${DRY_RUN}" -eq 1 ]]; then
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

write_renew_script() {
  if [[ "${DRY_RUN}" -eq 1 ]]; then
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

detect_public_ipv4() {
  local endpoints=("https://api.ipify.org" "https://ifconfig.me/ip" "https://checkip.amazonaws.com")
  for ep in "${endpoints[@]}"; do
    local ip
    ip="$(curl -fsSL --max-time 5 "$ep" 2>/dev/null || true)"
    ip="$(echo "$ip" | tr -d '[:space:]')"
    if [[ -n "$ip" ]] && is_valid_ipv4 "$ip"; then
      echo "$ip"
      return 0
    fi
  done
  return 1
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

issue_cert() {
  local ip="$1"
  export LEGO_PATH

  local server_args=()
  if [[ "${ACME_STAGING:-0}" == "1" ]]; then
    server_args=(--server "https://acme-staging-v02.api.letsencrypt.org/directory")
  fi

  local cert_crt="${LEGO_PATH}/certificates/${ip}.crt"
  local cert_key="${LEGO_PATH}/certificates/${ip}.key"
  local cert_issuer="${LEGO_PATH}/certificates/${ip}.issuer.crt"

  local need_renew=0
  if [[ -L "${TLS_ACTIVE_LINK}" && -f "${TLS_ACTIVE_LINK}/fullchain.pem" ]]; then
    if ! openssl x509 -in "${TLS_ACTIVE_LINK}/fullchain.pem" -noout -checkend 259200 >/dev/null 2>&1; then
      need_renew=1
    fi
  else
    need_renew=1
  fi

  local issued_ip
  issued_ip="$(state_get issued_ip || true)"
  local force_reissue=0
  if [[ -n "$issued_ip" && "$issued_ip" != "$ip" ]]; then
    force_reissue=1
  fi

  local next_retry_at
  next_retry_at="$(state_get next_retry_at || true)"
  if [[ -n "$next_retry_at" ]]; then
    local now_epoch retry_epoch
    now_epoch="$(date +%s)"
    retry_epoch="$(date -d "$next_retry_at" +%s 2>/dev/null || echo 0)"
    if (( retry_epoch > now_epoch )); then
      log "skip renew before next_retry_at=${next_retry_at}"
      exit 0
    fi
  fi

  if (( need_renew == 0 && force_reissue == 0 )); then
    write_state "$ip" "$issued_ip" "" ""
    log "certificate still valid, skip renew"
    exit 0
  fi

  local out_file
  out_file="$(mktemp)"
  set +e
  if (( force_reissue == 1 )); then
    lego "${server_args[@]}" --accept-tos --email "$ACME_EMAIL" --domains "$ip" --profile shortlived --http --http.webroot "$WEBROOT_DIR" run >"$out_file" 2>&1
    rc=$?
  elif [[ -f "$cert_crt" && -f "$cert_key" ]]; then
    lego "${server_args[@]}" --accept-tos --email "$ACME_EMAIL" --domains "$ip" --profile shortlived --http --http.webroot "$WEBROOT_DIR" renew --days 3 >"$out_file" 2>&1
    rc=$?
    if (( rc != 0 )); then
      lego "${server_args[@]}" --accept-tos --email "$ACME_EMAIL" --domains "$ip" --profile shortlived --http --http.webroot "$WEBROOT_DIR" run >"$out_file" 2>&1
      rc=$?
    fi
  else
    lego "${server_args[@]}" --accept-tos --email "$ACME_EMAIL" --domains "$ip" --profile shortlived --http --http.webroot "$WEBROOT_DIR" run >"$out_file" 2>&1
    rc=$?
  fi
  set -e

  if (( rc != 0 )); then
    local msg
    msg="$(cat "$out_file" | tail -n 10 | tr '\n' ' ' | sed 's/  */ /g')"
    if grep -Eiq 'rate limit|too many requests' "$out_file"; then
      local retry_epoch
      retry_epoch="$(( $(date +%s) + 21600 + (RANDOM % 900) ))"
      write_state "$ip" "$issued_ip" "$(date -u -d "@${retry_epoch}" +"%Y-%m-%dT%H:%M:%SZ")" "rate_limit"
    fi
    rm -f "$out_file"
    fail "lego renew/run failed: ${msg}"
  fi
  rm -f "$out_file"

  [[ -f "$cert_crt" ]] || fail "lego cert crt missing"
  [[ -f "$cert_key" ]] || fail "lego cert key missing"

  local release_dir
  release_dir="${TLS_RELEASE_DIR}/$(date -u +%Y%m%d%H%M%S)"
  mkdir -p "$release_dir"

  if [[ -f "$cert_issuer" ]]; then
    cat "$cert_crt" "$cert_issuer" > "${release_dir}/fullchain.pem"
  else
    cp "$cert_crt" "${release_dir}/fullchain.pem"
  fi
  cp "$cert_key" "${release_dir}/privkey.pem"

  chmod 0600 "${release_dir}/privkey.pem"
  chmod 0644 "${release_dir}/fullchain.pem"

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
  log "certificate ready for ${ip}"
}

main() {
  mkdir -p "$WEBROOT_DIR" "$TLS_RELEASE_DIR" "$LEGO_PATH" "$(dirname "$STATE_FILE")"
  chmod 0700 "$TLS_ROOT" "$TLS_RELEASE_DIR" || true

  local ip="${PUBLIC_IP}"
  if [[ -z "$ip" ]]; then
    ip="$(detect_public_ipv4 || true)"
  fi
  [[ -n "$ip" ]] || fail "cannot determine public ipv4"
  is_valid_ipv4 "$ip" || fail "public ip is not valid/public: ${ip}"

  issue_cert "$ip"
}

main "$@"
SCRIPT

  chmod 0755 "$RENEW_SCRIPT"
}

acquire_public_ip() {
  if [[ -n "$PUBLIC_IP" ]]; then
    is_valid_ipv4 "$PUBLIC_IP" || fail "--public-ip is not a valid public IPv4"
    return 0
  fi

  local from_state
  from_state="$(state_get public_ip || true)"
  if [[ -n "$from_state" ]] && is_valid_ipv4 "$from_state"; then
    PUBLIC_IP="$from_state"
    return 0
  fi

  PUBLIC_IP="$(detect_public_ipv4 || true)"
  [[ -n "$PUBLIC_IP" ]] || fail "cannot detect public IPv4"
  is_valid_ipv4 "$PUBLIC_IP" || fail "detected public IP is invalid: ${PUBLIC_IP}"
}

issue_initial_cert() {
  run_cmd "$RENEW_SCRIPT"
}

write_acme_probe_file() {
  run_cmd mkdir -p "${WEBROOT_DIR}/.well-known/acme-challenge"
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log "[dry-run] write acme probe file"
    return 0
  fi
  echo "ok" > "${WEBROOT_DIR}/.well-known/acme-challenge/ping"
  chown -R "$SERVICE_USER:$SERVICE_GROUP" "$WEBROOT_DIR"
}

start_services() {
  run_cmd systemctl daemon-reload
  run_cmd systemctl enable yc-relay.service yc-sidecar.service yc-cert-renew.timer
  run_cmd systemctl restart yc-relay.service
  run_cmd systemctl restart yc-sidecar.service
  run_cmd systemctl restart yc-cert-renew.timer
}

stop_services() {
  run_cmd systemctl stop yc-cert-renew.timer yc-cert-renew.service yc-sidecar.service yc-relay.service nginx || true
}

status_cmd() {
  local ok=1
  for unit in nginx yc-relay.service yc-sidecar.service yc-cert-renew.timer; do
    if systemctl is-active --quiet "$unit"; then
      echo "$unit: active"
    else
      echo "$unit: inactive"
      ok=0
    fi
  done

  if [[ "$ok" -eq 1 ]]; then
    exit 0
  fi
  exit 1
}

doctor_cmd() {
  local relay_active sidecar_active nginx_active timer_active
  relay_active="$(systemctl is-active yc-relay.service 2>/dev/null || true)"
  sidecar_active="$(systemctl is-active yc-sidecar.service 2>/dev/null || true)"
  nginx_active="$(systemctl is-active nginx 2>/dev/null || true)"
  timer_active="$(systemctl is-active yc-cert-renew.timer 2>/dev/null || true)"

  local public_ip issued_ip next_retry_at
  public_ip="$(state_get public_ip || true)"
  issued_ip="$(state_get issued_ip || true)"
  next_retry_at="$(state_get next_retry_at || true)"

  local cert_expiry=""
  if [[ -f "${TLS_ACTIVE_LINK}/fullchain.pem" ]]; then
    cert_expiry="$(openssl x509 -in "${TLS_ACTIVE_LINK}/fullchain.pem" -noout -enddate | cut -d= -f2 || true)"
  fi

  local code=0
  [[ "$relay_active" == "active" ]] || code=1
  [[ "$sidecar_active" == "active" ]] || code=1
  [[ "$nginx_active" == "active" ]] || code=1
  [[ "$timer_active" == "active" ]] || code=1

  if [[ "$FORMAT" == "json" ]]; then
    cat <<JSON
{
  "ports": {
    "80": "$(port_listener_count 80)",
    "443": "$(port_listener_count 443)"
  },
  "certs": {
    "active": "$(if [[ -f "${TLS_ACTIVE_LINK}/fullchain.pem" ]]; then echo yes; else echo no; fi)",
    "expiry": "${cert_expiry}"
  },
  "ip": {
    "public": "${public_ip}",
    "issued": "${issued_ip}"
  },
  "state": {
    "nextRetryAt": "${next_retry_at}"
  },
  "nginx": {
    "active": "${nginx_active}",
    "configOk": "$(if nginx -t >/dev/null 2>&1; then echo yes; else echo no; fi)"
  },
  "systemd": {
    "relay": "${relay_active}",
    "sidecar": "${sidecar_active}",
    "renewTimer": "${timer_active}"
  },
  "nextRetryAt": "${next_retry_at}"
}
JSON
  else
    echo "relay: ${relay_active}"
    echo "sidecar: ${sidecar_active}"
    echo "nginx: ${nginx_active}"
    echo "renew-timer: ${timer_active}"
    echo "public-ip: ${public_ip:-unknown}"
    echo "issued-ip: ${issued_ip:-unknown}"
    echo "next-retry-at: ${next_retry_at:-none}"
    echo "cert-expiry: ${cert_expiry:-unknown}"
    if nginx -t >/dev/null 2>&1; then
      echo "nginx-config: ok"
    else
      echo "nginx-config: failed"
      code=1
    fi
  fi

  exit "$code"
}

render_pairing() {
  local relay_wss="wss://${PUBLIC_IP}/v1/ws"
  log "pairing info (service user view):"
  if [[ "${DRY_RUN}" -eq 1 ]]; then
    log "[dry-run] run-as ${SERVICE_USER} ${BIN_DIR}/yc-sidecar pairing show --format text --relay ${relay_wss}"
    return 0
  fi

  run_as_service_user "${BIN_DIR}/yc-sidecar" pairing show --format text --relay "$relay_wss" || true
}

install_cmd() {
  [[ -n "$VERSION" ]] || fail "--version <tag> is required for install"
  validate_email "$ACME_EMAIL" || fail "--acme-email <email> is required and must be valid" 2
  normalize_asset_base_url

  require_root
  ensure_packages
  ensure_lego
  ensure_service_user
  ensure_directories
  acquire_public_ip

  check_default_server_conflict
  write_nginx_acme_conf
  nginx_reload_safe
  write_acme_probe_file

  download_release_assets
  write_services
  write_renew_env
  write_renew_script

  issue_initial_cert
  write_nginx_full_conf
  nginx_reload_safe

  start_services

  render_pairing
  log "relay url: wss://${PUBLIC_IP}/v1/ws"
  log "health url: https://${PUBLIC_IP}/healthz"

  if [[ -n "${TMP_ASSET_DIR:-}" ]]; then
    rm -rf "$TMP_ASSET_DIR"
  fi
}

uninstall_cmd() {
  require_root

  if ! confirm "Uninstall relay+sidecar services?"; then
    log "cancelled"
    exit 0
  fi

  stop_services

  run_cmd systemctl disable yc-cert-renew.timer yc-relay.service yc-sidecar.service || true
  run_cmd rm -f "$RELAY_SERVICE" "$SIDECAR_SERVICE" "$RENEW_SERVICE" "$RENEW_TIMER"
  run_cmd systemctl daemon-reload

  run_cmd rm -f "${BIN_DIR}/yc-relay" "${BIN_DIR}/yc-sidecar"

  if [[ "$PURGE" -eq 1 ]]; then
    if ! confirm "Purge config/identity/certs/logs?"; then
      log "purge skipped"
      exit 0
    fi
    run_cmd rm -rf /etc/yourconnector "$WORK_ROOT" "$RELAYSIDE_LOG_DIR" "$NGINX_CONF" "$RENEW_SCRIPT"
  fi
}

start_cmd() {
  require_root
  run_cmd systemctl start nginx yc-relay.service yc-sidecar.service yc-cert-renew.timer
}

restart_cmd() {
  require_root
  run_cmd systemctl restart nginx yc-relay.service yc-sidecar.service yc-cert-renew.timer
}

main() {
  parse_args "$@"

  case "$COMMAND" in
    install) install_cmd ;;
    uninstall) uninstall_cmd ;;
    status) status_cmd ;;
    doctor) doctor_cmd ;;
    start) start_cmd ;;
    stop) require_root; stop_services ;;
    restart) restart_cmd ;;
    *) usage; fail "unsupported command: $COMMAND" ;;
  esac
}

main "$@"
