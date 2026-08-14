#!/usr/bin/env bash
#
# Forgepost — rebuild from source and restart the service.
#
# Usage:
#   sudo ./update.sh
#
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "run as root (sudo)" >&2
  exit 2
fi

SRC_DIR="${FORGEPOST_SRC_DIR:-/opt/forgepost/src}"
BIN_DIR="${FORGEPOST_BIN_DIR:-/opt/forgepost}"
export PATH="${FORGEPOST_CARGO_BIN:-/root/.cargo/bin}:$PATH"

if [[ ! -d "$SRC_DIR/.git" ]]; then
  echo "no checkout at $SRC_DIR — run deploy/install.sh first" >&2
  exit 1
fi

echo "==> Pulling latest code"
cd "$SRC_DIR"
git pull --ff-only

echo "==> Building release binary"
cargo build --release --bin forgepost
install -m 0755 target/release/forgepost "$BIN_DIR/forgepost"

echo "==> Restarting service"
systemctl restart forgepost

echo "forgepost updated"
