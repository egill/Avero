#!/bin/bash
set -e

# Configuration
HOST="avero@100.65.110.63"
SITE="avero"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "Deploying gateway-tui to Avero..."

echo "Syncing source..."
rsync -avz --exclude target --exclude .git "$SCRIPT_DIR/" "$HOST:~/gateway/"

echo "Building on server..."
ssh "$HOST" "source ~/.cargo/env && cd ~/gateway && cargo build --release --bin gateway-tui"

echo "Installing binary..."
ssh "$HOST" "cp ~/gateway/target/release/gateway-tui /opt/avero/gateway/target/release/"

echo "Done."
echo "Run: ssh $HOST '/opt/avero/gateway/target/release/gateway-tui --config /opt/avero/gateway/config/$SITE.toml'"
