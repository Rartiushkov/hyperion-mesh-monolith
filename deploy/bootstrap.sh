#!/usr/bin/env bash
# Bootstrap Hyperion Mesh backend on Ubuntu (DigitalOcean / Hetzner / etc.)
set -euo pipefail

APP_DIR="/opt/hyperion-mesh"
BIN_URL="${BIN_URL:-}"

echo "[Hyperion Mesh] bootstrapping backend..."

# Update system and install dependencies.
sudo apt-get update
sudo apt-get install -y ca-certificates curl

# Create user and directory.
sudo mkdir -p "$APP_DIR"
sudo useradd -r -s /bin/false hyperion || true
sudo chown hyperion:hyperion "$APP_DIR"

# Download prebuilt binary if a URL is provided, otherwise build from source.
if [ -n "$BIN_URL" ]; then
  sudo curl -fsSL "$BIN_URL" -o /usr/local/bin/hyperion_raw_ingress
  sudo chmod +x /usr/local/bin/hyperion_raw_ingress
else
  echo "[Hyperion Mesh] no BIN_URL set; build locally with: cargo build --release"
fi

# Install systemd unit.
sudo cp deploy/hyperion-raw.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable hyperion-raw

# Placeholder .env — fill it with real secrets before start.
if [ ! -f "$APP_DIR/.env" ]; then
  sudo -u hyperion tee "$APP_DIR/.env" >/dev/null <<EOF
# Alchemy RPC for Base Sepolia / Arbitrum Sepolia
ALCHEMY_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY

# Supabase / PostgreSQL connection string
DATABASE_URL=postgresql://hyperion:changeme@localhost:5432/hyperion
EOF
  echo "[Hyperion Mesh] created $APP_DIR/.env — edit it with real secrets"
fi

sudo systemctl start hyperion-raw || true
echo "[Hyperion Mesh] backend installed. Check status: sudo systemctl status hyperion-raw"
