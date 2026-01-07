#!/bin/bash
set -e

HOST="avero@100.80.187.4"  # Update with actual Avero host
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "🖥️  Deploying gateway-tui to Avero"

echo "📦 Syncing source..."
rsync -avz --exclude target --exclude .git "$SCRIPT_DIR/" "$HOST:~/gateway-poc-new/"

echo "🔨 Building TUI on server..."
ssh "$HOST" "source ~/.cargo/env && cd ~/gateway-poc-new && cargo build --release --bin gateway-tui"

echo "🔄 Copying binary..."
ssh "$HOST" "cp ~/gateway-poc-new/target/release/gateway-tui /opt/avero/gateway-poc/target/release/"

echo "✅ Deployed gateway-tui to Avero"
echo "Run with: ssh $HOST '/opt/avero/gateway-poc/target/release/gateway-tui --config /opt/avero/gateway-poc/config/avero.toml'"
