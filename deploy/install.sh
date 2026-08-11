#!/usr/bin/env bash
#
# Forgepost — one-command VPS install (Ubuntu 24.04).
#
# Usage:
#   sudo ./install.sh blog.example.com
#
# Steps:
#   * install build deps (cmake/clang needed by aws-lc-sys) + Rust toolchain
#   * clone + build the release binary
#   * create the `forgepost` user and /var/lib/forgepost data dir
#   * write /etc/forgepost/forgepost.env (addr, TLS domain, DB path)
#   * install the systemd unit + nightly backup timer
#   * open ports 80/443 in ufw (and keep SSH)
#
# The app handles HTTPS itself (Let's Encrypt via TLS-ALPN-01 on 443), so no
# nginx/certbot is needed. HTTP :80 is auto-redirected by the app.
#
set -euo pipefail

DOMAIN="${1:-}"
if [[ -z "$DOMAIN" ]]; then
  echo "usage: $0 <domain>" >&2
  exit 2
fi
if [[ $EUID -ne 0 ]]; then
  echo "run as root (sudo)" >&2
  exit 2
fi

REPO_URL="https://github.com/DavidNeurieder/my_blog.git"
SRC_DIR="/opt/forgepost/src"
BIN_DIR="/opt/forgepost"
DATA_DIR="/var/lib/forgepost"
CONF_DIR="/etc/forgepost"

echo "==> Installing build dependencies"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y --no-install-recommends \
  build-essential cmake clang perl pkg-config curl git ca-certificates

echo "==> Installing Rust (minimal)"
if [[ ! -x /root/.cargo/bin/cargo ]]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain stable
fi
export PATH="/root/.cargo/bin:$PATH"

echo "==> Cloning and building Forgepost"
mkdir -p "$SRC_DIR" "$BIN_DIR"
if [[ ! -d "$SRC_DIR/.git" ]]; then
  git clone "$REPO_URL" "$SRC_DIR"
fi
cd "$SRC_DIR"
cargo build --release --bin forgepost
install -m 0755 target/release/forgepost "$BIN_DIR/forgepost"

echo "==> Creating runtime user and data dirs"
if ! id -u forgepost &>/dev/null; then
  useradd --system --home-dir "$DATA_DIR" --shell /usr/sbin/nologin forgepost
fi
mkdir -p "$DATA_DIR/tls" "$DATA_DIR/backups" "$CONF_DIR"
chown -R forgepost:forgepost "$DATA_DIR"

echo "==> Writing $CONF_DIR/forgepost.env"
cat > "$CONF_DIR/forgepost.env" <<EOF
# Forgepost server configuration
FORGEPOST_ADDR=0.0.0.0:443
FORGEPOST_TLS_DOMAIN=$DOMAIN
DATABASE_URL=sqlite://$DATA_DIR/forgepost.db
RUST_LOG=info
EOF
chmod 600 "$CONF_DIR/forgepost.env"

echo "==> Installing systemd units"
install -m 0644 "$SRC_DIR/deploy/forgepost.service" \
  /etc/systemd/system/forgepost.service
install -m 0644 "$SRC_DIR/deploy/forgepost-backup.service" \
  /etc/systemd/system/forgepost-backup.service
install -m 0644 "$SRC_DIR/deploy/forgepost-backup.timer" \
  /etc/systemd/system/forgepost-backup.timer
install -m 0755 "$SRC_DIR/deploy/forgepost-backup.sh" \
  /usr/local/sbin/forgepost-backup.sh

echo "==> Opening firewall (80/tcp, 443/tcp, SSH)"
if command -v ufw >/dev/null 2>&1; then
  ufw allow OpenSSH
  ufw allow 80/tcp
  ufw allow 443/tcp
  ufw --force enable
else
  echo "ufw not present — open 80/443 in your cloud firewall instead" >&2
fi

systemctl daemon-reload
systemctl enable --now forgepost
systemctl enable --now forgepost-backup.timer

echo
echo "==> Done. Next steps:"
echo "   1. Point an A record for $DOMAIN at this machine's IP"
echo "   2. Open https://$DOMAIN and complete the /setup wizard"
echo "   3. Check it:  systemctl status forgepost"
echo "                 journalctl -u forgepost -f"
echo "   4. Updates:   sudo /opt/forgepost/src/deploy/update.sh"
