#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="${ROOT_DIR:-/opt/hyperion-mesh}"
BIN_DIR="${ROOT_DIR}/bin"
TRACE_DIR="${ROOT_DIR}/traces"
SERVICE_FILES=(
  "hyperion-grpc.service"
  "hyperion-card-webhook.service"
)
CLEANUP_SCRIPT="/usr/local/bin/hyperion-mesh-cleanup.sh"
CRON_PATH="/etc/cron.d/hyperion-mesh-cleanup"

log() {
  printf '[deploy-binary] %s\n' "$1"
}

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    echo "This script must run as root." >&2
    exit 1
  fi
}

install_cron_if_missing() {
  if command -v cron >/dev/null 2>&1; then
    log "cron already installed."
    return
  fi

  log "cron not found. Installing cron."
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y cron
}

ensure_runtime_dirs() {
  mkdir -p "${BIN_DIR}" "${TRACE_DIR}"
  chown -R hyperion:hyperion "${ROOT_DIR}" || true
  chmod 0775 "${TRACE_DIR}"
}

configure_cleanup() {
  cat > "${CLEANUP_SCRIPT}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

TRACE_DIR="/opt/hyperion-mesh/traces"
find "${TRACE_DIR}" -type f \( -name '*.log' -o -name '*.json' -o -name '*.jsonl' \) -mmin +2880 -delete
EOF
  chmod 0755 "${CLEANUP_SCRIPT}"

  cat > "${CRON_PATH}" <<'EOF'
SHELL=/bin/bash
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
17 3 * * * root /usr/local/bin/hyperion-mesh-cleanup.sh
EOF
  chmod 0644 "${CRON_PATH}"
  systemctl enable --now cron
}

install_systemd_units() {
  for unit in "${SERVICE_FILES[@]}"; do
    install -D -m 0644 "${ROOT_DIR}/deploy/${unit}" "/etc/systemd/system/${unit}"
  done
}

main() {
  require_root
  id hyperion >/dev/null 2>&1 || useradd -r -s /bin/false hyperion
  ensure_runtime_dirs
  install_cron_if_missing
  install_systemd_units
  configure_cleanup

  chmod +x "${BIN_DIR}/hyperion_grpc_server"
  chmod +x "${BIN_DIR}/hyperion_card_webhook"

  log "Reloading systemd and restarting Hyperion services."
  systemctl daemon-reload
  systemctl enable hyperion-grpc.service hyperion-card-webhook.service
  systemctl restart hyperion-grpc.service
  systemctl restart hyperion-card-webhook.service

  log "Current service status:"
  systemctl --no-pager --full status hyperion-grpc.service || true
  systemctl --no-pager --full status hyperion-card-webhook.service || true
}

main "$@"
